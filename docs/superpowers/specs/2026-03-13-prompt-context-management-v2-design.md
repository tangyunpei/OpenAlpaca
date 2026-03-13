# Prompt & Context Management v2 — Design Spec

**Date:** 2026-03-13
**Branch:** AgenticLoopOptimization
**Status:** Draft
**Scope:** Unified prompt assembly, context sourcing/budgeting/lifecycle, graduated compaction, inter-agent context flow

---

## 1. Problem Statement

The current prompt and context management system has grown organically across multiple phases. While individual components work (persona rendering, memory retrieval, compaction pipeline, context budget tracking), they are not integrated into a cohesive system. This leads to:

1. **SimpleQuery doesn't pass ContextBudgetManager to the agentic loop** — computed but passed as `None`, so compaction never triggers on the primary chat path.
2. **No pre-flight token enforcement** — prompts can exceed the model's context window. The budget manager tracks the window but nothing prevents sending an oversized request.
3. **Memory retrieval budget is hardcoded (2000 chars)** — not connected to ContextBudgetManager; static regardless of available space.
4. **Prompt section budgets are scattered** — Identity=300 chars, User=1000 chars, Skill=4000 tokens, Memory=2000 chars — each hardcoded separately, not derived from available context window.
5. **Skill context sources are unbounded** — no per-source token limit on injected files/globs.
6. **Compaction is binary** — either no compaction or full heuristic/LLM compression. No graduated approach.
7. **No unified prompt section tracking** — each execution path (SimpleQuery, DAG, Pipeline, LeadAgent, SkillInvocation) builds prompts differently with ad-hoc string concatenation.
8. **Sub-agents get thin prompts** — pipeline steps and DAG nodes lack conversation context, memories, and user profile. ContextPackage exists but is underused.

## 2. Architecture Overview

Two-layer architecture separating **what context to include** from **how to assemble a prompt**:

```
┌─────────────────────────────────────────────────┐
│                 Execution Path                  │
│   (SimpleQuery / Pipeline / DAG / LeadAgent)    │
└───────────────┬─────────────────────────────────┘
                │ creates ContextRequest
                ▼
┌─────────────────────────────────────────────────┐
│              ContextManager                     │
│                                                 │
│  Sources:    Memory, Conversation, Skills,      │
│              Workspace, UserProfile             │
│                                                 │
│  Pipeline:   Resolve → Score → Budget → Render  │
│                                                 │
│  Output:     ContextBundle (scored sections)     │
│              ContextPackage (distilled for       │
│              sub-agents)                         │
└───────────────┬─────────────────────────────────┘
                │ feeds ContextBundle
                ▼
┌─────────────────────────────────────────────────┐
│              PromptBuilder                      │
│                                                 │
│  Static:     SystemPersona, AgentPersona,       │
│              Identity, Bootstrap                │
│                                                 │
│  Dynamic:    ContextBundle sections             │
│              (memories, summary, skill refs)    │
│                                                 │
│  Infra:      Tools, Connectors, MessageSource   │
│                                                 │
│  Enforcer:   Pre-flight budget check,           │
│              adaptive section trimming          │
│                                                 │
│  Output:     BuiltPrompt (system msg +          │
│              context msgs + budget registry)    │
└───────────────┬─────────────────────────────────┘
                │ feeds BuiltPrompt
                ▼
┌─────────────────────────────────────────────────┐
│           Agentic Loop                          │
│                                                 │
│  Runtime:    Graduated compaction (5 tiers)      │
│              Context lifecycle tracking          │
│              Budget-aware message accumulation   │
└─────────────────────────────────────────────────┘
```

### Design Principles

1. **Budget-first** — Every piece of content has a token cost and a priority. When budget is tight, lower-priority items trim first.
2. **Single assembly path** — All execution paths use the same PromptBuilder. Path differences are expressed as configuration (which sections to include), not different code paths.
3. **Resolve → Score → Budget → Render** — Context sources go through a deterministic pipeline rather than ad-hoc injection.
4. **Lifecycle awareness** — Context tracks whether it's been seen, whether it's stale, and whether it needs refresh.
5. **Minimum exposure** — Sub-agents receive only what they need via `distill()`, respecting access control and adapting to their model's context window.

## 3. ContextManager

The ContextManager owns the full lifecycle of context: what to fetch, how to rank it, how much space it gets, and when it's stale.

### 3.0 Constructor & Dependencies

```rust
pub struct ContextManager {
    sources: Vec<Box<dyn ContextSource>>,
    config: Arc<ArcSwap<DaemonConfig>>,
}

impl ContextManager {
    /// Build from shared daemon state. Called once at startup, stored in Orchestrator.
    pub fn new(
        db: Option<Arc<Database>>,
        embedder: Option<Arc<dyn Embedder>>,
        user_document: Arc<RwLock<Option<UserDocument>>>,
        skill_catalog: Arc<SkillCatalog>,
        config: Arc<ArcSwap<DaemonConfig>>,
    ) -> Self {
        let mut sources: Vec<Box<dyn ContextSource>> = Vec::new();

        if let Some(ref db) = db {
            sources.push(Box::new(MemorySource::new(db.clone(), embedder.clone())));
        }
        sources.push(Box::new(ConversationSource));
        sources.push(Box::new(UserProfileSource::new(user_document)));
        sources.push(Box::new(SkillContextSource::new(skill_catalog)));
        sources.push(Box::new(WorkspaceSource::new(db)));

        Self { sources, config }
    }

    fn autocompact_buffer(&self, model_window: usize) -> usize {
        let ratio = self.config.load().orchestrator.context.autocompact_buffer_ratio;
        (model_window as f64 * ratio) as usize
    }
}
```

`ContextManager` lives in `openalpaca_core` (same crate as Orchestrator). It depends on `openalpaca_storage` (for `Database`) and `openalpaca_llm` (for `Embedder`) — both are existing dependencies of `openalpaca_core`.

### 3.1 Context Request

Each execution path creates a `ContextRequest` describing what context it needs:

```rust
pub struct ContextRequest {
    /// The user's input — used for relevance scoring
    pub query: String,
    /// Classified intent — influences source priorities
    pub intent: Intent,
    /// Which execution path is asking
    pub path: ExecutionPath,
    /// Active skill (if skill invocation)
    pub skill: Option<Arc<SkillDocument>>,
    /// Owner for memory scoping
    pub owner_id: Option<String>,
    /// Memory scope cascade
    pub scope: MemoryScopeContext,
    /// Model context window (tokens)
    pub model_context_window: usize,
    /// Tokens already claimed by static sections
    /// (persona, identity, tools — PromptBuilder reports this)
    pub reserved_tokens: usize,
}

pub enum ExecutionPath {
    SimpleQuery,
    SocialQuery,
    SkillInvocation { skill_id: String },
    PipelineStep { step: usize, total: usize },
    DagNode { node_id: String },
    LeadAgent,
}
```

### 3.2 Context Sources

Each source is a trait implementor that resolves content independently:

```rust
#[async_trait]
pub trait ContextSource: Send + Sync {
    /// Unique name for telemetry and dedup
    fn name(&self) -> &'static str;

    /// Resolve context relevant to this request.
    /// Returns sections ordered by relevance (highest first).
    async fn resolve(&self, request: &ContextRequest) -> Vec<ContextSection>;

    /// Which execution paths this source is active for.
    fn active_for(&self, path: &ExecutionPath) -> bool { true }
}
```

Five concrete sources:

| Source | Resolves | Priority Range | Active For |
|--------|----------|---------------|------------|
| `MemorySource` | Hybrid FTS+vector search, top-K ranked by relevance | Normal–High | All except SocialQuery |
| `ConversationSource` | Session summary + recent message digest | High | All except SocialQuery |
| `SkillContextSource` | Skill reference files, `context.sources` from frontmatter | Normal | SkillInvocation only |
| `WorkspaceSource` | Previous agent outputs, cached artifacts | Low–Normal | Pipeline, DAG, LeadAgent |
| `UserProfileSource` | USER.md rendered profile block | Normal | All except SocialQuery |

### 3.3 Context Section

The unit of context flowing through the system:

```rust
pub struct ContextSection {
    /// Source that produced this section
    pub source: &'static str,
    /// Kind for dedup and access control
    pub kind: ContextKind,
    /// Rendered content ready for prompt injection
    pub content: String,
    /// Token cost (bytes/4 heuristic, overridable)
    pub token_estimate: usize,
    /// Priority for trimming under budget pressure
    pub priority: SectionPriority,
    /// Relevance score from source (0.0–1.0, for ordering within priority tier)
    pub relevance: f32,
    /// Unique key for lifecycle tracking (dedup, seen detection)
    pub key: ContextKey,
    /// How to render in prompt (system msg, user msg, or inline)
    pub injection: InjectionMode,
}

pub enum ContextKind {
    Memory,
    ConversationSummary,
    SkillReference,
    WorkspaceArtifact,
    UserProfile,
}

/// Ordering: Critical > High > Normal > Low > Optional.
/// Variants are declared in ASCENDING order so that derived `Ord`
/// gives `Optional(0) < Low(1) < Normal(2) < High(3) < Critical(4)`.
/// Use `b.priority.cmp(&a.priority)` for highest-first sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SectionPriority {
    Optional,   // dropped entirely under pressure (bootstrap, skills catalog)
    Low,        // trimmed first (workspace artifacts, skill file refs)
    Normal,     // trimmed proportionally (user profile, memories, summary)
    High,       // trimmed last (identity, active skill body)
    Critical,   // never trimmed (system persona, safety)
}

pub enum InjectionMode {
    /// Appended to system prompt string
    SystemPrompt,
    /// Injected as a separate ChatMessage::system()
    SystemMessage,
    /// Injected as ChatMessage::user() with untrusted context wrapping
    UserMessage { tag: String, trust: TrustLevel },
}

pub enum TrustLevel {
    /// System-generated, no wrapping needed
    Trusted,
    /// User-derived or retrieved, wrap with context boundary tags
    Untrusted,
}

/// Stable key for dedup and lifecycle tracking
#[derive(Hash, Eq, PartialEq, Clone)]
pub enum ContextKey {
    Memory(i64),
    ConversationSummary,
    SkillSource(String, String),    // (skill_id, source_path)
    WorkspaceArtifact(String),
    UserProfile,
    Package(String),
}
```

### 3.4 Budget-Aware Resolution

Available tokens for context = model window − reserved tokens (static sections) − autocompact buffer.

Budget allocation is proportional by priority tier, not fixed per section:

```rust
impl ContextManager {
    pub async fn resolve(&self, request: &ContextRequest) -> ContextBundle {
        // 1. Compute available budget
        let available = request.model_context_window
            - request.reserved_tokens
            - self.autocompact_buffer(request.model_context_window);

        // 2. Resolve all active sources in parallel
        let active: Vec<&dyn ContextSource> = self.sources.iter()
            .filter(|s| s.active_for(&request.path))
            .map(|s| s.as_ref())
            .collect();
        let results = futures_util::future::join_all(
            active.iter().map(|s| s.resolve(&request))
        ).await;
        let mut sections: Vec<ContextSection> = results.into_iter().flatten().collect();

        // 3. Sort: highest priority first, then highest relevance within tier
        //    SectionPriority derives Ord with Optional(0) < Critical(4),
        //    so b.cmp(a) gives descending (Critical first).
        sections.sort_by(|a, b| {
            b.priority.cmp(&a.priority)
                .then(b.relevance.partial_cmp(&a.relevance)
                    .unwrap_or(std::cmp::Ordering::Equal))
        });

        // 4. Greedy fill: include while budget allows
        let mut used = 0usize;
        let mut included = Vec::new();
        for section in sections {
            if used + section.token_estimate <= available {
                used += section.token_estimate;
                included.push(section);
            } else if section.priority >= SectionPriority::Normal {
                // High-priority section doesn't fit fully — truncate to remaining
                let remaining = available.saturating_sub(used);
                if remaining > 100 {
                    let truncated = Self::truncate_section(section, remaining);
                    used += truncated.token_estimate;
                    included.push(truncated);
                }
            }
            // Low/Optional sections that don't fit are silently dropped
        }

        ContextBundle {
            sections: included,
            total_tokens: used,
            available_budget: available,
        }
    }

    /// Pre-compute static section token estimates so the caller can populate
    /// `ContextRequest.reserved_tokens` before calling `resolve()`.
    ///
    /// This breaks the chicken-and-egg: callers build static sections first,
    /// estimate their cost, then pass `reserved_tokens` into the context request.
    ///
    /// Usage:
    /// ```rust
    /// let reserved = PromptBuilder::new(window)
    ///     .system_persona(&soul)
    ///     .agent_persona(&agent)
    ///     .tools(&tool_defs)
    ///     .estimate_static_tokens();
    /// let request = ContextRequest { reserved_tokens: reserved, ... };
    /// let bundle = context_manager.resolve(request).await;
    /// // Now add dynamic sections and build
    /// builder.context_bundle(&bundle).build();
    /// ```
    pub fn estimate_static_tokens(&self) -> usize;
}
```

### 3.5 Context Lifecycle Tracking

Tracks what context has been injected into the current conversation to avoid redundant re-injection:

```rust
pub struct ContextLifecycle {
    /// Context keys already in the conversation, with the message index where injected
    seen: HashMap<ContextKey, SeenEntry>,
}

struct SeenEntry {
    message_index: usize,
    injected_at: Instant,
    token_cost: usize,
}

impl ContextLifecycle {
    /// Should this section be re-injected?
    ///
    /// The caller resolves the staleness threshold per `ContextKind` from config:
    /// - `ContextKind::Memory` → `lifecycle.memory_stale_after`
    /// - `ContextKind::UserProfile` → `lifecycle.profile_stale_after`
    /// - `ContextKind::ConversationSummary` → `lifecycle.summary_stale_after`
    /// - Others → `Duration::MAX` (never re-inject automatically)
    pub fn should_inject(&self, key: &ContextKey, staleness_threshold: Duration) -> bool {
        match self.seen.get(key) {
            None => true,
            Some(entry) => entry.injected_at.elapsed() > staleness_threshold,
        }
    }

    /// Record that a section was injected
    pub fn mark_injected(&mut self, key: ContextKey, message_index: usize, tokens: usize) {
        self.seen.insert(key, SeenEntry {
            message_index,
            injected_at: Instant::now(),
            token_cost: tokens,
        });
    }

    /// How many tokens of seen context are in messages before `index`?
    /// Used by compaction to know what's safe to summarize.
    pub fn tokens_before(&self, index: usize) -> usize {
        self.seen.values()
            .filter(|e| e.message_index < index)
            .map(|e| e.token_cost)
            .sum()
    }
}
```

The lifecycle lives in the agentic loop's state (per-conversation), not in the ContextManager itself (which is shared).

## 4. PromptBuilder

Takes static sections (persona, identity, tools) + dynamic sections (from ContextBundle) and assembles a final prompt with pre-flight budget enforcement.

### 4.1 Core API

```rust
pub struct PromptBuilder {
    model_context_window: usize,
    sections: Vec<PromptSection>,
}

struct PromptSection {
    name: &'static str,
    content: String,
    token_estimate: usize,
    priority: SectionPriority,
    target: SectionTarget,
}

enum SectionTarget {
    /// Concatenated into the system prompt string
    SystemPromptBlock,
    /// Emitted as a separate ChatMessage
    Message(ChatMessage),
}

/// The output of PromptBuilder::build()
pub struct BuiltPrompt {
    /// The assembled system prompt (single string)
    pub system_message: String,
    /// Additional context messages (user profile, memories, summary)
    pub context_messages: Vec<ChatMessage>,
    /// Section breakdown for ContextBudgetManager registration
    pub section_registry: Vec<(&'static str, usize)>,
    /// Total tokens consumed by the prompt (before conversation messages)
    pub total_prompt_tokens: usize,
    /// Tokens remaining for conversation + response
    pub remaining_for_conversation: usize,
}
```

### 4.2 Builder Chain

```rust
impl PromptBuilder {
    pub fn new(model_context_window: usize) -> Self;

    // ── Static sections (system prompt blocks) ──
    pub fn system_persona(&mut self, persona: &SystemPersona) -> &mut Self;  // Critical
    pub fn agent_persona(&mut self, persona: &AgentPersona) -> &mut Self;    // Critical
    pub fn identity(&mut self, doc: &IdentityDocument) -> &mut Self;         // High
    pub fn bootstrap(&mut self, doc: &BootstrapDocument) -> &mut Self;       // Optional
    pub fn skills_catalog(&mut self, catalog: &str) -> &mut Self;            // Optional

    // ── Infrastructure sections ──
    pub fn tools(&mut self, tool_defs: &[ToolDefinition]) -> &mut Self;      // High
    pub fn connector_guidance(                                                // Normal
        &mut self, statuses: &[(String, String)], sendable: Option<&[String]>,
    ) -> &mut Self;
    pub fn message_source(&mut self, source: &str) -> &mut Self;             // Normal

    // ── Dynamic sections (from ContextBundle) ──
    pub fn context_bundle(&mut self, bundle: &ContextBundle) -> &mut Self;

    // ── Pre-resolve estimation ──
    /// Estimate total tokens of sections added so far (static sections).
    /// Call this BEFORE `context_bundle()` to get `reserved_tokens` for
    /// `ContextRequest`, breaking the chicken-and-egg dependency.
    pub fn estimate_static_tokens(&self) -> usize;

    // ── Build with enforcement ──
    pub fn build(mut self) -> BuiltPrompt;
}
```

Each execution path uses the same builder with different sections included:

```
                    SimpleQuery   Skill   Pipeline   DAG   LeadAgent   Social
system_persona          ✓          ✓        ✓        ✓       ✓          ✓
agent_persona           ✓          ✓        ✓        ✓       ✓          ✓
identity                ✓          ✓        —        —       —          —
bootstrap               ✓          —        —        —       —          —
skills_catalog          ✓          —        —        —       —          —
tools                   ✓          ✓        ✓        ✓       ✓          —
connector_guidance      ✓          ✓        ✓        ✓       ✓          —
message_source          ✓          ✓        —        —       —          ✓
context_bundle          ✓          ✓        ✓        ✓       —          —
```

### 4.3 Pre-Flight Budget Enforcement

`build()` ensures total prompt tokens fit within the model's context window, leaving room for conversation and response:

```rust
pub fn build(mut self) -> BuiltPrompt {
    // Reserve at minimum 25% of window for conversation + response
    let max_prompt_tokens = (self.model_context_window as f64 * 0.75) as usize;

    let total: usize = self.sections.iter().map(|s| s.token_estimate).sum();
    if total > max_prompt_tokens {
        self.trim_to_budget(max_prompt_tokens);
    }

    // Assemble system prompt blocks + context messages
    let mut system_prompt = String::new();
    let mut context_messages = Vec::new();
    let mut section_registry = Vec::new();

    for section in &self.sections {
        section_registry.push((section.name, section.token_estimate));
        match &section.target {
            SectionTarget::SystemPromptBlock => {
                system_prompt.push_str(&section.content);
                system_prompt.push('\n');
            }
            SectionTarget::Message(msg) => {
                context_messages.push(msg.clone());
            }
        }
    }

    let total_prompt_tokens: usize = self.sections
        .iter().map(|s| s.token_estimate).sum();

    BuiltPrompt {
        system_message: system_prompt,
        context_messages,
        section_registry,
        total_prompt_tokens,
        remaining_for_conversation: self.model_context_window
            .saturating_sub(total_prompt_tokens),
    }
}
```

### 4.4 Adaptive Trimming

When sections exceed budget, trim from lowest priority up:

1. **Phase 1:** Drop `Optional` sections entirely (bootstrap, skills catalog)
2. **Phase 2:** Drop `Low` sections entirely (workspace artifacts, skill file refs)
3. **Phase 3:** Proportionally truncate `Normal` sections (user profile, memories, summary, connectors)
4. **Phase 4:** Proportionally truncate `High` sections as last resort (identity, tools, active skill)
5. **Critical sections are never trimmed** — if Critical alone exceeds 75% of window, log a warning

Proportional truncation: each section in the tier gets `section_tokens * (tier_budget / tier_total_tokens)` — preserving the ratio of each section's original allocation.

### 4.5 Integration with ContextBudgetManager

`BuiltPrompt.section_registry` feeds directly into the existing `ContextBudgetManager`:

```rust
let built = prompt_builder.build();
let mut budget = ContextBudgetManager::new(model_window, &budget_config);
for (name, tokens) in &built.section_registry {
    budget.register_section(name, *tokens);
}
// budget.fixed_zone_tokens() is now accurate
// budget.should_compact() will fire at the right time
```

This closes the gap where sections were never registered.

## 5. Graduated Compaction

Replaces binary compaction with a tiered system where lighter strategies are tried first.

### 5.1 Compaction Tiers

```rust
pub enum CompactionTier {
    None,                   // No action
    TruncateToolResults,    // Truncate tool results to 50%
    DropMultimedia,         // Drop images, audio, documents → text placeholders
    DiscardSocial,          // Remove social message pairs
    HeuristicSummary,       // Round-grouped summary (existing compress_context)
    LlmSummary,             // LLM-based 3-phase pipeline
}
```

### 5.2 Tier Selection

The budget manager reports which tier is appropriate based on utilization of the **full model context window** (not the compaction trigger). This ensures the percentages in the config map directly to observable window usage:

```rust
impl ContextBudgetManager {
    pub fn compaction_tier(&self, message_tokens: usize) -> CompactionTier {
        let total = self.fixed_zone_tokens() + message_tokens;
        // Utilization is against the FULL model window, so 70% means
        // 70% of the model's total context capacity.
        let utilization = total as f64 / self.model_context_window as f64;

        match utilization {
            u if u < 0.60 => CompactionTier::None,
            u if u < 0.70 => CompactionTier::TruncateToolResults,
            u if u < 0.75 => CompactionTier::DropMultimedia,
            u if u < 0.80 => CompactionTier::DiscardSocial,
            u if u < 0.85 => CompactionTier::HeuristicSummary,
            _              => CompactionTier::LlmSummary,
        }
    }
}
```

### 5.3 Graduated Compactor

Tries the selected tier, checks if utilization dropped enough, escalates if not:

```rust
pub struct GraduatedCompactor;

impl GraduatedCompactor {
    /// Run graduated compaction. Takes trait objects for LLM-based tiers
    /// (not `LlmBackend` directly) so this module doesn't depend on
    /// `agentic_loop/backend.rs`'s `pub(super)` type.
    pub async fn compact(
        messages: &mut Vec<ChatMessage>,
        budget: &ContextBudgetManager,
        extractor: &dyn MemoryExtractor,
        summarizer: &dyn Summarizer,
        lifecycle: &mut ContextLifecycle,
    ) -> CompactionReport {
        let mut report = CompactionReport::new();
        let initial_tokens = estimate_messages_tokens(messages) as usize;
        let mut current_tier = budget.compaction_tier(initial_tokens);

        while current_tier != CompactionTier::None {
            let before = messages.len();

            match current_tier {
                CompactionTier::TruncateToolResults => {
                    Self::truncate_tool_results(messages);
                }
                CompactionTier::DropMultimedia => {
                    Self::drop_multimedia(messages);
                }
                CompactionTier::DiscardSocial => {
                    CompactionPipeline::discard_social(
                        messages, budget.min_recent_messages()
                    );
                }
                CompactionTier::HeuristicSummary => {
                    compress_context(messages, 0, Some(budget));
                }
                CompactionTier::LlmSummary => {
                    let owned = std::mem::take(messages);
                    let result = CompactionPipeline::compact(
                        owned,
                        budget.min_recent_messages(),
                        extractor,
                        summarizer,
                    ).await;
                    for mem in &result.extracted_memories {
                        report.extracted_memories.push(mem.clone());
                    }
                    *messages = result.compacted_messages;
                }
                CompactionTier::None => break,
            };

            let after_tokens = estimate_messages_tokens(messages) as usize;
            report.record_tier(current_tier, before, messages.len(), after_tokens);

            // Check if sufficient
            let next_tier = budget.compaction_tier(after_tokens);
            if next_tier >= current_tier {
                // Escalate to next tier. If we're already at LlmSummary
                // (the final tier), stop — nothing heavier to try.
                match current_tier.next() {
                    Some(higher) => current_tier = higher,
                    None => {
                        tracing::warn!(
                            after_tokens,
                            "All compaction tiers exhausted; utilization still high"
                        );
                        break;
                    }
                }
            } else if next_tier == CompactionTier::None {
                break;
            } else {
                current_tier = next_tier;
            }
        }

        report.final_tokens = estimate_messages_tokens(messages) as usize;
        report
    }
}
```

### 5.4 Tier Implementations

**Tier 1 — TruncateToolResults:** Tool result messages are often the largest (file contents, search results, command output). Truncating to 50% reclaims significant space — the LLM has already processed the full result in a previous round.

**Tier 2 — DropMultimedia:** Images (1590 tokens/tile), audio (~25 tokens/sec), and document attachments consume significant tokens. Once processed in a prior round, replace with text placeholders.

**Tiers 3–5** reuse existing implementations: `CompactionPipeline::discard_social()`, `compress_context()`, `CompactionPipeline::compact()`.

### 5.5 Practical Impact

Example with a 200K token window (thresholds measured against full window):

| Scenario | Binary (current) | Graduated |
|----------|-------------------|-----------|
| 120K tokens (60%) | Nothing | Tier 1: truncate tool results → ~100K |
| 140K tokens (70%) | Nothing | Tier 2: drop multimedia → ~110K |
| 150K tokens (75%) | Nothing | Tier 3: social discard → ~135K |
| 160K tokens (80%) | Nothing | Tier 4: heuristic summary → ~90K |
| 170K tokens (85%) | Full compression → ~80K (lossy) | Tier 5: LLM summary → ~85K (high quality) |

The current system jumps from "do nothing" to "lossy full compression." Graduated compaction preserves more context for longer.

## 6. Inter-Agent Context Flow

How context passes from parent → sub-agent with budget adaptation and access control.

### 6.1 Context Distillation

```rust
impl ContextManager {
    pub fn distill(
        &self,
        parent_bundle: &ContextBundle,
        agent_constraints: &AgentConstraints,
        sub_agent_window: usize,
        task_description: &str,
        handoff: Option<&HandoffContext>,
    ) -> ContextPackage {
        // 1. Budget: sub-agent gets at most 40% of its window for context
        let context_budget = (sub_agent_window as f64 * 0.40) as usize;

        // 2. Access control: filter out denied sections
        let denied = &agent_constraints.denied_sections;
        let allowed_sections: Vec<&ContextSection> = parent_bundle.sections.iter()
            .filter(|s| !denied.contains(&s.kind.as_str().to_string()))
            .collect();

        // 3. Build package sections with adjusted priorities
        let mut package_sections = Vec::new();

        // Task assignment (Critical — always included)
        package_sections.push(PackageSection {
            kind: PackageSectionKind::TaskDescription,
            content: task_description.to_string(),
            token_estimate: task_description.len() / 4,
            priority: SectionPriority::Critical,
        });

        // Predecessor output (High — pipeline/DAG handoff)
        if let Some(handoff) = handoff {
            package_sections.push(PackageSection {
                kind: PackageSectionKind::PredecessorOutput,
                content: handoff.format(),
                token_estimate: handoff.token_estimate(),
                priority: SectionPriority::High,
            });
        }

        // Re-map parent sections with sub-agent-appropriate priorities
        for section in allowed_sections {
            let sub_priority = match section.kind {
                ContextKind::ConversationSummary => SectionPriority::High,
                ContextKind::Memory => SectionPriority::Normal,
                ContextKind::WorkspaceArtifact => SectionPriority::Normal,
                ContextKind::UserProfile => SectionPriority::Low,
                ContextKind::SkillReference => continue, // sub-agents have own skills
            };
            package_sections.push(PackageSection {
                kind: PackageSectionKind::from(section.kind),
                content: section.content.clone(),
                token_estimate: section.token_estimate,
                priority: sub_priority,
            });
        }

        // 4. Budget-fill with greedy algorithm
        //    Sort: highest priority first (b.cmp(a)), then smallest sections
        //    first within a tier (a.cmp(b) on tokens) to fit more items.
        package_sections.sort_by(|a, b| {
            b.priority.cmp(&a.priority)
                .then(a.token_estimate.cmp(&b.token_estimate))
        });

        let mut used = 0usize;
        let mut included = Vec::new();
        for section in package_sections {
            if used + section.token_estimate <= context_budget {
                used += section.token_estimate;
                included.push(section);
            } else if section.priority >= SectionPriority::Normal {
                let remaining = context_budget.saturating_sub(used);
                if remaining > 100 {
                    let truncated = Self::truncate_package_section(section, remaining);
                    used += truncated.token_estimate;
                    included.push(truncated);
                }
            }
        }

        ContextPackage {
            sections: included,
            total_tokens: used,
            budget: context_budget,
            sub_agent_window,
        }
    }
}
```

### 6.2 Handoff Context

Structured context from pipeline predecessors:

```rust
pub struct HandoffContext {
    /// Which agent produced this
    pub producer: AgentSummary,
    /// The task that was assigned
    pub task_assigned: String,
    /// The agent's output
    pub output: String,
    /// Key decisions or findings
    pub decisions: Vec<String>,
    /// What this agent flagged for the next agent
    pub handoff_notes: Option<String>,
}

pub struct AgentSummary {
    pub name: String,
    pub role: String,
    pub step: usize,
}

impl HandoffContext {
    pub fn format(&self) -> String {
        // Renders as <predecessor_output agent="..." role="..." step="...">
        // with task, output, decisions, and handoff notes
    }

    /// Merge multiple predecessors (for DAG nodes with multiple dependencies)
    pub fn merge(handoffs: &[HandoffContext]) -> HandoffContext { ... }
}
```

### 6.3 ContextPackage v2

Replaces `ContextPackageBuilder` with a type that PromptBuilder can consume directly:

```rust
pub struct ContextPackage {
    pub sections: Vec<PackageSection>,
    pub total_tokens: usize,
    pub budget: usize,
    pub sub_agent_window: usize,
}

pub struct PackageSection {
    pub kind: PackageSectionKind,
    pub content: String,
    pub token_estimate: usize,
    pub priority: SectionPriority,
}

pub enum PackageSectionKind {
    TaskDescription,
    PredecessorOutput,
    ConversationSummary,
    Memory,
    WorkspaceArtifact,
    UserProfile,
}

impl ContextPackage {
    /// Convert to ContextBundle for PromptBuilder consumption.
    ///
    /// Injection mode mapping (security-relevant):
    /// - TaskDescription → SystemPrompt (trusted, system-generated)
    /// - PredecessorOutput → UserMessage { trust: Untrusted } (agent output, potential injection)
    /// - ConversationSummary → UserMessage { trust: Untrusted } (user-derived)
    /// - Memory → UserMessage { trust: Untrusted } (retrieved content)
    /// - WorkspaceArtifact → UserMessage { trust: Untrusted } (external content)
    /// - UserProfile → SystemMessage (loaded from config, trusted)
    pub fn to_bundle(&self) -> ContextBundle {
        ContextBundle {
            sections: self.sections.iter().map(|s| {
                let injection = match s.kind {
                    PackageSectionKind::TaskDescription => InjectionMode::SystemPrompt,
                    PackageSectionKind::UserProfile => InjectionMode::SystemMessage,
                    PackageSectionKind::PredecessorOutput => InjectionMode::UserMessage {
                        tag: "predecessor_output".to_string(),
                        trust: TrustLevel::Untrusted,
                    },
                    PackageSectionKind::ConversationSummary => InjectionMode::UserMessage {
                        tag: "conversation_summary".to_string(),
                        trust: TrustLevel::Untrusted,
                    },
                    PackageSectionKind::Memory => InjectionMode::UserMessage {
                        tag: "retrieved_memory".to_string(),
                        trust: TrustLevel::Untrusted,
                    },
                    PackageSectionKind::WorkspaceArtifact => InjectionMode::UserMessage {
                        tag: "workspace_artifact".to_string(),
                        trust: TrustLevel::Untrusted,
                    },
                };
                ContextSection {
                    source: s.kind.source_name(),
                    kind: s.kind.to_context_kind(),
                    content: s.content.clone(),
                    token_estimate: s.token_estimate,
                    priority: s.priority,
                    relevance: 1.0,
                    key: ContextKey::Package(s.kind.key_name()),
                    injection,
                }
            }).collect(),
            total_tokens: self.total_tokens,
            available_budget: self.budget,
        }
    }
}
```

### 6.4 Flow Per Execution Path

**SimpleQuery:** `ContextManager::resolve(SimpleQuery)` → `ContextBundle` → `PromptBuilder` → `BuiltPrompt` → agentic loop.

**Pipeline:** Each step gets a distilled package enriched with predecessor output via `HandoffContext`. Step N+1 knows what step N did and why.

**DAG:** Each node gets a distilled package. Nodes with dependencies get merged predecessor outputs via `HandoffContext::merge()`.

**Lead Agent:** Gets a full `ContextBundle`. When it spawns sub-agents via `spawn_subagent`, the handler calls `distill()` to create a right-sized package.

### 6.5 What Changes From Today

| Aspect | Current | New |
|--------|---------|-----|
| Pipeline context | Raw output pasted as workspace text | Structured `HandoffContext` with agent identity, task, decisions |
| DAG context | `ContextPackageBuilder` (manual, often empty) | Auto-distilled from parent bundle with budget adjustment |
| Sub-agent memories | First pipeline step gets DB memories, rest don't | Parent's relevant memories propagated via distillation |
| Window adaptation | None — same content regardless of model size | Budget adjusted to 40% of sub-agent's actual context window |
| Access control | `denied_sections` in builder (rarely used) | Always applied via `AgentConstraints` in distillation |
| Lead → sub-agent | No context flow (sub-agents start blank) | Distilled package from lead agent's bundle |

## 7. Module Layout

```
crates/openalpaca_core/src/
├── context/                          # NEW — replaces context_budget/
│   ├── mod.rs                        #   Public API
│   ├── manager.rs                    #   ContextManager (resolve, distill)
│   ├── sources/                      #   ContextSource trait + implementations
│   │   ├── mod.rs
│   │   ├── memory.rs                 #   MemorySource
│   │   ├── conversation.rs           #   ConversationSource
│   │   ├── skill.rs                  #   SkillContextSource
│   │   ├── workspace.rs              #   WorkspaceSource
│   │   └── user_profile.rs           #   UserProfileSource
│   ├── section.rs                    #   ContextSection, ContextKind, SectionPriority
│   ├── lifecycle.rs                  #   ContextLifecycle
│   ├── package.rs                    #   ContextPackage v2, HandoffContext
│   ├── budget.rs                     #   ContextBudgetManager (extended)
│   └── compaction/                   #   Graduated compaction
│       ├── mod.rs                    #   GraduatedCompactor
│       ├── tiers.rs                  #   CompactionTier, tier implementations
│       └── pipeline.rs               #   MOVED from context_budget/compaction.rs
│
├── prompt/                           # NEW
│   ├── mod.rs                        #   Public API
│   ├── builder.rs                    #   PromptBuilder
│   ├── trimming.rs                   #   Adaptive trimming logic
│   └── sections.rs                   #   Section renderers
│
├── middleware/                        # KEPT — renderers become thinner
│   ├── prompt.rs                     #   KEPT: format_tool_guidance, etc.
│   │                                 #   REMOVED: PromptAssembler
│   ├── soul/mod.rs                   #   KEPT unchanged
│   ├── identity/mod.rs               #   KEPT unchanged
│   ├── user/mod.rs                   #   KEPT unchanged
│   ├── bootstrap/mod.rs              #   KEPT unchanged
│   ├── skill/mod.rs                  #   KEPT unchanged
│   └── guard.rs                      #   KEPT unchanged
│
├── context_budget/                   # DEPRECATED — forwarding re-exports
│   └── mod.rs                        #   pub use crate::context::*
```

## 8. What Gets Replaced vs. Adapted

| Component | Action | Detail |
|-----------|--------|--------|
| `PromptAssembler` | **Replaced** by `PromptBuilder` | |
| `ContextBudgetManager` | **Extended** | Add `compaction_tier()`, keep all existing methods |
| `ContextPackage` + `ContextPackageBuilder` | **Replaced** by v2 | New `ContextPackage` with `to_bundle()` |
| `CompactionPipeline` | **Kept** as tier 5 | Moved to `context/compaction/pipeline.rs` |
| `compress_context()` | **Kept** as tier 4 | Stays in agentic loop context module |
| `discard_social()` | **Kept** as tier 3 | Called by `GraduatedCompactor` |
| `MemoryExtractor` + `Summarizer` traits | **Kept** | On `LlmBackend`, unchanged |
| `estimate_messages_tokens()` | **Kept** | Unchanged |
| All `format_*` functions | **Kept** | Called by PromptBuilder internally |
| All middleware renderers | **Kept** | Pure functions, no changes |

## 9. Configuration

New section in `daemon.toml`:

```toml
[orchestrator.context]
# Maximum fraction of context window used for prompt
max_prompt_ratio = 0.75
# Fraction of window reserved for autocompact buffer
autocompact_buffer_ratio = 0.165

[orchestrator.context.compaction_thresholds]
# Utilization triggers for each tier (fraction of full model context window)
truncate_tool_results = 0.60
drop_multimedia = 0.70
discard_social = 0.75
heuristic_summary = 0.80
llm_summary = 0.85

[orchestrator.context.section_budgets]
# Maximum fraction of context budget per section type (caps, not reservations)
user_profile = 0.05
memories = 0.15
conversation_summary = 0.10
skill_references = 0.15
workspace_artifacts = 0.10

[orchestrator.context.lifecycle]
# Staleness thresholds for re-injection (seconds)
memory_stale_after = 300
profile_stale_after = 600
summary_stale_after = 120

[orchestrator.context.distillation]
# Fraction of sub-agent's window available for context
sub_agent_context_ratio = 0.40
```

## 10. Phased Migration

Six phases, each independently shippable and testable:

### Phase 1: Foundation (no behavior change)
- Create `context/` module with `ContextSection`, `SectionPriority`, `ContextBundle` types
- Create `prompt/` module with `PromptBuilder`
- `PromptBuilder` calls existing middleware renderers internally
- Unit tests for PromptBuilder (section ordering, trimming, budget enforcement)
- No call sites changed

### Phase 2: ContextManager + Sources (no behavior change)
- Implement `ContextSource` trait
- Build `MemorySource`, `ConversationSource`, `UserProfileSource`
- Build `SkillContextSource`, `WorkspaceSource`
- `ContextManager::resolve()` with budget-aware greedy fill
- Integration tests: resolve returns correct sections per ExecutionPath

### Phase 3: Wire SimpleQuery (first behavior change)
- SimpleQuery uses `ContextManager::resolve()` + `PromptBuilder`
- Pass `ContextBudgetManager` to agentic loop (closes gap #1)
- `BuiltPrompt.section_registry` auto-registers with budget manager
- Replace manual prompt assembly code in simple_query_handler
- **Preserved behaviors** (not in PromptBuilder — remain in handler):
  - `adapt_parts_for_model()` — multimodal part conversion per model capability
  - `try_direct_send()` — bypass agentic loop for simple send commands
  - `apply_send_keepalive` — keep-alive for send tool invocations
  - `loop_overrides` — deep-query tier config adjustments
  These stay in the handler as pre/post-processing around PromptBuilder.
- Smoke test: verify all critical prompt sections are present (persona, identity,
  tools, memories, connectors) and token totals are within 10% of previous output

### Phase 4: Graduated Compaction
- Add `compaction_tier()` to `ContextBudgetManager`
- Implement `GraduatedCompactor` with tiers 1–5
- Add `ContextLifecycle` to agentic loop state
- Replace binary compaction block in `agentic_loop_inner`
- Legacy fallback path unchanged (no budget → old behavior)

### Phase 5: Inter-Agent Context Flow
- Implement `ContextPackage` v2 + `HandoffContext`
- `ContextManager::distill()` for sub-agent packages
- Wire Pipeline steps with `HandoffContext`
- Wire DAG nodes with distilled packages
- Wire Lead Agent sub-agent spawning with distillation
- Delete old `ContextPackageBuilder`
- **Event migration**: Update `SystemEvent::ContextPackageBuilt` fields to match new
  `ContextPackage` shape. Update `event_bridge.rs` WebSocket serialization and
  GUI `ServerEvent` TypeScript type. The event keeps the same variant name but
  fields change from `{sections_included, total_tokens, memories_count}` to
  `{sections: Vec<(kind, tokens)>, total_tokens, budget, sub_agent_window}`.

### Phase 6: Wire Remaining Paths + Cleanup
- Skill invocation uses `ContextManager` + `PromptBuilder`
- Lead agent prompt uses `PromptBuilder`
- Delete `PromptAssembler`
- Delete `context_budget/` (forwarding re-exports, then remove)
- Telemetry: emit `ContextBudgetComputed` with full section breakdown for all paths
- Move hardcoded budgets to `daemon.toml` configuration

## 11. Backward Compatibility

- `context_budget/` module becomes a re-export shim during migration, removed in Phase 6
- `ContextBudgetManager` is extended, not replaced — all existing callers continue working
- Middleware renderers are unchanged — external code keeps working
- Agentic loop signature unchanged — `context_budget: Option<&ContextBudgetManager>` stays, but now always `Some` for production paths
- `LoopConfig` gains optional `compaction_thresholds` field (defaults to current behavior)
- Legacy compaction path (`else if config.max_context_tokens > 0`) preserved for Direct backend / tests

## 12. Out of Scope

- **Actual tokenizer** (tiktoken-rs) — bytes/4 heuristic is sufficient; real token counts from API responses correct estimates each round
- **Dynamic section priority based on intent** — premature; fixed priorities with configurable caps are sufficient
- **Cross-conversation context persistence** — per-conversation only; persistent memory handled by existing memory system
- **Agent verbosity modifiers** — defer to a future iteration
