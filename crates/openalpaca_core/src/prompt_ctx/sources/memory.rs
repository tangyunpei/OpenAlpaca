//! The automatic per-turn memory recall as a context source (design §5):
//! `MemoryService::recall` over the owner's partitions, one section per
//! delivered note, keyed by the note's id, revision and reason — the same
//! `memory_used` entry the turn reports.
//!
//! Two halves, because the memory section leaves the cached dynamic-context
//! layer (design §5; review finding 3): [`MemorySource`] resolves the notes
//! into the bundle like any other source, and [`memory_turn`] renders the
//! sections the context manager kept into the one user message the caller
//! appends per turn, outside Layer 3's fingerprint. `compose::dynamic_context`
//! skips `ContextKind::Memory` for the same reason, so a repeated question on
//! a lane never reads the previous turn's notes back out of the cache.

use crate::daemon_config::{DaemonConfig, RecallMode};
use crate::memory::service::{
    Attribution, Capabilities, MemoryError, MemoryService, MemoryUsed, PendingSuggestion,
    RECALL_TOTAL_BYTES, RecallBudget, Surface, UsedReason, WorkspaceHint, render_delivery,
};
use crate::prompt_ctx::section::{
    ContextBundle, ContextKey, ContextKind, ContextSection, InjectionMode, SectionPriority,
    TrustLevel,
};
use crate::prompt_ctx::sources::{ContextRequest, ContextSource, ExecutionPath};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// The `MemorySource` name, and the `source` of every section it emits.
pub const SOURCE_NAME: &str = "memory";

/// The longest a reply waits on memory (scale slice S7, M4): the per-turn
/// recall, and on its own the pending suggestions a turn is told about.
/// Past it the turn continues without that part, as it does when memory
/// fails, and the read finishes on its blocking worker — so what it was
/// doing (a stat scan's re-index, say) is done for the next turn.
///
/// Chosen from the L8 benches (M5 Max, release): the healthy recall p95 is
/// 59 ms at 10K notes (81 ms with a tombstone, before S3), 497 ms at 100K
/// learned-size notes and 661 ms at 100K average-size ones;
/// `pending_suggestions` is about 300 ms at 100K. The waits that had no
/// bound were whole-partition re-indexes inside a turn (3.3 s at 10K, 43 s
/// at 100K, before S1), a boot rebuild (2.9 s / 33 s, before S6) and an
/// uncached read of a 100K partition after an external edit (~3 s). 1.5 s
/// is more than twice the slowest healthy p95 and under every one of
/// those.
pub const REPLY_MEMORY_DEADLINE: std::time::Duration = std::time::Duration::from_millis(1_500);

/// Runs `work` on the blocking pool for at most `bound`: its answer, or
/// `None` — with one WARN naming the context's partitions and the time
/// waited, never a note's text — when the deadline passed first.
async fn within_reply_deadline<T: Send + 'static>(
    what: &'static str,
    bound: std::time::Duration,
    partitions: impl FnOnce() -> Vec<String>,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<Result<T, tokio::task::JoinError>> {
    let started = std::time::Instant::now();
    match tokio::time::timeout(bound, tokio::task::spawn_blocking(work)).await {
        Ok(joined) => Some(joined),
        Err(_) => {
            tracing::warn!(
                partitions = ?partitions(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                "memory {what} ran past the reply's deadline; the turn continues without it"
            );
            None
        }
    }
}

pub struct MemorySource {
    /// The daemon's service, installed after construction
    /// (`Orchestrator::set_memory_service`); `None` until then, and nothing
    /// is recalled.
    service: Arc<RwLock<Option<Arc<MemoryService>>>>,
    /// The live daemon config: `[orchestrator.memory] recall_mode` is read
    /// on every turn (design 09 §13.2), so a hot reload takes effect on the
    /// next one.
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
}

impl MemorySource {
    pub fn new(
        service: Arc<RwLock<Option<Arc<MemoryService>>>>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
    ) -> Self {
        Self {
            service,
            daemon_config,
        }
    }

    fn service(&self) -> Option<Arc<MemoryService>> {
        self.service.read().ok().and_then(|guard| guard.clone())
    }
}

#[async_trait]
impl ContextSource for MemorySource {
    fn name(&self) -> &'static str {
        SOURCE_NAME
    }

    async fn resolve(&self, request: &ContextRequest) -> Vec<ContextSection> {
        self.resolve_within(request, REPLY_MEMORY_DEADLINE).await
    }
}

impl MemorySource {
    /// [`ContextSource::resolve`] with the recall bounded by `deadline`
    /// ([`REPLY_MEMORY_DEADLINE`] on the reply path).
    pub(crate) async fn resolve_within(
        &self,
        request: &ContextRequest,
        deadline: std::time::Duration,
    ) -> Vec<ContextSection> {
        if request.query.trim().is_empty() {
            return vec![];
        }
        let Some(service) = self.service() else {
            return vec![];
        };
        let Some(owner_id) = request.owner_id.clone() else {
            return vec![];
        };
        // Design §7a, "the same turn's automatic recall": the owner from the
        // request, the project root the request resolved (R22 — never the
        // daemon's CWD), `ModelTool` and read-only. Nothing is recorded
        // here: the caller records what it delivers (`memory_turn`).
        let root = request
            .scope
            .request_workspace_root
            .as_deref()
            .map(PathBuf::from);
        let hint = match &root {
            Some(root) => WorkspaceHint::Root(root.as_path()),
            None => WorkspaceHint::None,
        };
        let ctx = match service.context(
            &owner_id,
            hint,
            Surface::ModelTool,
            Capabilities::READ_ONLY,
            Attribution::default(),
        ) {
            Ok(ctx) => ctx,
            // Design §3: not a `global_user` row (`system`, `provider:id`) —
            // no partition, nothing recalled, by construction.
            Err(MemoryError::NoOwner(_)) => return vec![],
            Err(e) => {
                tracing::warn!(error = %e, "memory recall: context refused");
                return vec![];
            }
        };
        let query = request.query.clone();
        // The index block tells the model to read with `memory_read`; a
        // skill turn that cannot call it gets excerpts (L10b review 1).
        let mode = match &request.path {
            ExecutionPath::SkillInvocation { .. } if !skill_can_read_memory(request) => {
                RecallMode::Excerpts
            }
            _ => self.daemon_config.load().orchestrator.memory.recall_mode,
        };
        let budget = RecallBudget::for_mode(mode);
        let partitions = {
            let (service, ctx) = (service.clone(), ctx.clone());
            move || service.partition_keys(&ctx)
        };
        let recalled = within_reply_deadline("recall", deadline, partitions, move || {
            service.recall(&ctx, &query, budget)
        })
        .await;
        let result = match recalled {
            None => return vec![],
            Some(Ok(Ok(result))) => result,
            Some(Ok(Err(e))) => {
                tracing::warn!(error = %e, "memory recall failed; the turn runs without it");
                return vec![];
            }
            Some(Err(e)) => {
                tracing::warn!(error = %e, "memory recall did not complete; the turn runs without it");
                return vec![];
            }
        };
        // One section per note, keyed by its `memory_used` entry: each
        // excerpt, then each line of the index block (design 09 §13.2),
        // whose header `memory_turn` writes before the first listed note it
        // renders.
        let used = result.memory_used();
        let contents = result
            .notes
            .iter()
            .map(|note| note.render())
            .chain(result.listed.iter().map(|note| note.render()));
        used.into_iter()
            .zip(contents)
            .map(|(used, content)| ContextSection {
                source: SOURCE_NAME,
                kind: ContextKind::Memory,
                token_estimate: content.len() / 4,
                content,
                priority: SectionPriority::Normal,
                // The service already ranked them (design §5 step 6); the
                // manager's sort is stable, so equal relevance keeps that
                // order.
                relevance: 1.0,
                key: ContextKey::Memory(used),
                injection: InjectionMode::UserMessage {
                    tag: "memory_context".to_string(),
                    trust: TrustLevel::Untrusted,
                },
            })
            .collect()
    }
}

/// Whether the skill a skill-path request invokes can call `memory_read`.
/// The skill tier offers a file-based skill exactly the tools its frontmatter
/// resolves (`orchestrator/skill/invocation.rs`): `requires_capabilities`
/// when it names any, else `tools.allow`, less `tools.deny`. No skill
/// document offers nothing.
fn skill_can_read_memory(request: &ContextRequest) -> bool {
    const TOOL: &str = "memory_read";
    let Some(skill) = request.skill.as_deref() else {
        return false;
    };
    let front = &skill.frontmatter;
    let granted = if front.requires_capabilities.is_empty() {
        &front.tools.allow
    } else {
        &front.requires_capabilities
    };
    granted.iter().any(|name| name == TOOL) && !front.tools.deny.iter().any(|name| name == TOOL)
}

/// What a turn appends for memory after compose, outside the cached layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryTurn {
    /// The one user message: the kept sections in rank order, wrapped as
    /// quoted data under the service's trust label (design §5 step 8).
    pub content: String,
    /// The turn's `memory_used`, in the same order — exactly the notes in
    /// `content`, which is what the turn reports (design §7a).
    pub used: Vec<MemoryUsed>,
}

impl MemoryTurn {
    /// The message's cost in the context budget, by the runner's `len / 4`.
    pub fn token_estimate(&self) -> usize {
        self.content.len() / 4
    }
}

/// The per-turn memory message from the sections a context manager kept, or
/// `None` when it kept none. Rendered here, not in Layer 3, so it is never
/// cached by query text.
pub fn memory_turn(bundle: &ContextBundle) -> Option<MemoryTurn> {
    memory_turn_with(bundle, None)
}

/// A section that is a line of the index block (design 09 §13.2), not an
/// excerpt.
fn is_listed(section: &ContextSection) -> bool {
    matches!(&section.key, ContextKey::Memory(used) if used.reason == UsedReason::Index)
}

/// The kept sections as one delivery: the excerpts in rank order, then the
/// listed notes under the index block's header, then `suggestions`.
fn render_sections(sections: &[&ContextSection], suggestions: Option<&str>) -> String {
    let (listed, excerpts): (Vec<&ContextSection>, Vec<&ContextSection>) =
        sections.iter().partition(|s| is_listed(s));
    render_delivery(
        excerpts.iter().map(|s| s.content.as_str()),
        listed.iter().map(|s| s.content.as_str()),
        suggestions,
    )
}

/// [`memory_turn`], with a `<memory_suggestions>` block after the notes
/// (design 09 §4.5): delivered in the same quoted-data wrapper under the
/// trust label, and counted against the same 8 KiB — the lowest-ranked
/// notes give way to it, the listed ones first (design 09 §13.2), and
/// `used` names only what stayed.
pub fn memory_turn_with(bundle: &ContextBundle, suggestions: Option<&str>) -> Option<MemoryTurn> {
    // Excerpts first, then the listed notes, each in the order the manager
    // kept them: the order they are rendered in, and the order they give way
    // in, from the end.
    let (listed, mut sections): (Vec<&ContextSection>, Vec<&ContextSection>) =
        bundle.memory_sections().partition(|s| is_listed(s));
    sections.extend(listed);
    if let Some(block) = suggestions {
        while !sections.is_empty()
            && render_sections(&sections, Some(block)).len() > RECALL_TOTAL_BYTES
        {
            sections.pop();
        }
    }
    if sections.is_empty() && suggestions.is_none() {
        return None;
    }
    let used = sections
        .iter()
        .filter_map(|s| match &s.key {
            ContextKey::Memory(used) => Some(used.clone()),
            _ => None,
        })
        .collect();
    let content = render_sections(&sections, suggestions);
    Some(MemoryTurn { content, used })
}

/// How many pending suggestions one turn is told about (design 09 §4.5).
pub const SUGGESTIONS_LISTED: usize = 3;

/// The most a `<memory_suggestions>` block carries (design 09 §4.5; 1 KiB
/// in the design, which one full line — two UUIDs and two 64-hex revisions,
/// 331 bytes before its titles — cannot fit three times: see the design's
/// revision log).
pub const SUGGESTIONS_BLOCK_MAX_BYTES: usize = 1536;

const SUGGESTIONS_OPEN: &str = "<memory_suggestions>\n";
const SUGGESTIONS_CLOSE: &str = "</memory_suggestions>\n";

/// The `<memory_suggestions>` block for `items`, newest first, as many lines
/// as fit [`SUGGESTIONS_BLOCK_MAX_BYTES`] up to [`SUGGESTIONS_LISTED`];
/// `None` when there is nothing to list. It carries no instruction: when to
/// call `memory_confirm` is in that tool's description, because
/// instructions inside quoted data are ignored by design.
pub fn render_memory_suggestions(items: &[PendingSuggestion]) -> Option<String> {
    let mut body = String::new();
    let fixed = SUGGESTIONS_OPEN.len() + SUGGESTIONS_CLOSE.len();
    for item in items.iter().take(SUGGESTIONS_LISTED) {
        let line = item.render();
        if fixed + body.len() + line.len() > SUGGESTIONS_BLOCK_MAX_BYTES {
            break;
        }
        body.push_str(&line);
    }
    (!body.is_empty()).then(|| format!("{SUGGESTIONS_OPEN}{body}{SUGGESTIONS_CLOSE}"))
}

/// The block for one main-loop turn that offers `memory_confirm` (design 09
/// §4.5): read with the turn's own `ModelTool` read context, the
/// suggestions already raised on its lane left out. Any failure — no owner,
/// a refused context, an index error — leaves the turn without the block,
/// never without an answer.
pub async fn memory_suggestions_block(
    service: Arc<MemoryService>,
    ctx: &crate::tools::registry::ToolContext,
) -> Option<String> {
    memory_suggestions_block_within(service, ctx, REPLY_MEMORY_DEADLINE).await
}

/// [`memory_suggestions_block`] with the read bounded by `deadline`: past
/// it, the turn goes without the block (scale slice S7).
pub(crate) async fn memory_suggestions_block_within(
    service: Arc<MemoryService>,
    ctx: &crate::tools::registry::ToolContext,
    deadline: std::time::Duration,
) -> Option<String> {
    let access = match service.context_for_tool(ctx, Capabilities::READ_ONLY) {
        Ok(access) => access,
        Err(MemoryError::NoOwner(_)) => return None,
        Err(e) => {
            tracing::warn!(error = %e, "memory suggestions: context refused");
            return None;
        }
    };
    let raised = ctx
        .lane_key
        .as_deref()
        .map(|lane| service.raised_on(lane))
        .unwrap_or_default();
    let partitions = {
        let (service, access) = (service.clone(), access.clone());
        move || service.partition_keys(&access)
    };
    let listed = within_reply_deadline("suggestions", deadline, partitions, move || {
        service.pending_suggestions(&access, SUGGESTIONS_LISTED, &raised)
    })
    .await;
    match listed {
        None => None,
        Some(Ok(Ok(items))) => render_memory_suggestions(&items),
        Some(Ok(Err(e))) => {
            tracing::warn!(error = %e, "memory suggestions could not be read; the turn runs without them");
            None
        }
        Some(Err(e)) => {
            tracing::warn!(error = %e, "memory suggestions did not complete; the turn runs without them");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    //! Fixture 21 (`tests/fixtures/memory/21-query-natural-question-prefer-vim.json`)
    //! through the source: a temp home store, the owner as a `global_user`
    //! row, an in-memory index.

    use super::*;
    use crate::daemon_config::{DaemonConfig, RecallMode};
    use crate::memory::scope_context::MemoryScopeContext;
    use crate::memory::service::StoreInput;
    use crate::memory::service::{INDEX_BLOCK_HEADER, wrap_delivery};
    use crate::orchestrator::intent::Intent;
    use crate::prompt_ctx::sources::ExecutionPath;
    use crate::test_util::HomeStoreGuard;
    use openalpaca_storage::memory_files::NoteKind;
    use openalpaca_storage::memory_index::MemoryIndex;
    use openalpaca_storage::{Database, IdentityRepository};

    const ALICE: &str = "u_example_alice";

    struct Scene {
        _env: HomeStoreGuard,
        _root: tempfile::TempDir,
        service: Arc<MemoryService>,
        source: MemorySource,
        /// The live config the source reads `recall_mode` from on each turn.
        config: Arc<ArcSwap<DaemonConfig>>,
    }

    impl Scene {
        fn set_recall_mode(&self, mode: RecallMode) {
            let mut config = (**self.config.load()).clone();
            config.orchestrator.memory.recall_mode = mode;
            self.config.store(Arc::new(config));
        }
    }

    fn scene() -> Scene {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let env = HomeStoreGuard::set(&home.canonicalize().unwrap());
        let db = Database::open(&root.path().join("db.sqlite")).unwrap();
        IdentityRepository::new(&db)
            .create_global_user(ALICE, None)
            .unwrap();
        let service = MemoryService::new(Some(db), MemoryIndex::open_in_memory().unwrap());
        service.rebuild_index().unwrap();
        let service = Arc::new(service);
        let slot = Arc::new(RwLock::new(Some(service.clone())));
        let config = Arc::new(ArcSwap::from_pointee(DaemonConfig::default()));
        Scene {
            _env: env,
            _root: root,
            service,
            source: MemorySource::new(slot, config.clone()),
            config,
        }
    }

    fn request(query: &str, owner: Option<&str>) -> ContextRequest {
        ContextRequest {
            query: query.to_string(),
            intent: Intent::SimpleQuery {
                query: query.to_string(),
            },
            path: ExecutionPath::SimpleQuery,
            skill: None,
            owner_id: owner.map(str::to_string),
            scope: MemoryScopeContext::global_only(),
            model_context_window: 8192,
            reserved_tokens: 0,
        }
    }

    fn store(scene: &Scene, title: &str, body: &str) -> crate::memory::service::StoreOutput {
        let owner = scene
            .service
            .context(
                ALICE,
                WorkspaceHint::None,
                Surface::UserSurface,
                Capabilities::ALL,
                Attribution::default(),
            )
            .unwrap();
        scene
            .service
            .store(
                &owner,
                StoreInput {
                    title: title.into(),
                    body: body.into(),
                    kind: Some(NoteKind::Preference),
                    ..Default::default()
                },
            )
            .unwrap()
    }

    /// Fixture 21 step 2: the natural question recalls the vim note — one
    /// section, keyed by the note's id and revision with reason `recall` —
    /// and not the coffee decoy; `memory_turn` renders exactly that.
    #[tokio::test]
    async fn the_natural_question_fixture_recalls_through_the_service() {
        let scene = scene();
        let vim = store(
            &scene,
            "Editor preference",
            "# Editor preference\n\nUser prefers vim for editing.\n",
        );
        let coffee = store(
            &scene,
            "Coffee",
            "# Coffee\n\nUser drinks espresso after lunch.\n",
        );

        let sections = scene
            .source
            .resolve(&request("what editor do I prefer?", Some(ALICE)))
            .await;
        assert_eq!(sections.len(), 1, "{sections:?}");
        assert_eq!(sections[0].kind, ContextKind::Memory);
        assert_eq!(
            sections[0].key,
            ContextKey::Memory(MemoryUsed {
                id: vim.id,
                revision: vim.revision.clone(),
                reason: crate::memory::service::UsedReason::Recall,
            })
        );
        assert!(sections[0].content.contains("User prefers vim for editing"));
        assert!(!sections[0].content.contains(&coffee.id.to_string()));

        let bundle = ContextBundle {
            total_tokens: sections.iter().map(|s| s.token_estimate).sum(),
            sections,
            available_budget: 8192,
        };
        let turn = memory_turn(&bundle).expect("one note was kept");
        assert!(turn.content.starts_with("<memory_context>\n"));
        assert!(turn.content.contains(crate::memory::service::TRUST_LABEL));
        assert!(turn.content.contains("User prefers vim for editing"));
        assert!(turn.content.ends_with("</memory_context>\n"));
        assert_eq!(turn.used.len(), 1);
        assert_eq!(turn.used[0].id, vim.id);
        assert_eq!(turn.token_estimate(), turn.content.len() / 4);
        // Nothing recorded by resolving: the caller records what it delivered.
        assert!(scene.service.recently_used(10).is_empty());
    }

    /// No owner (a `System` principal, an unlinked external identity), an
    /// owner that is not a `global_user` row, an empty query, or a service
    /// not installed yet: nothing is recalled, nothing fails.
    #[tokio::test]
    async fn nothing_is_recalled_without_an_owner_a_query_or_a_service() {
        let scene = scene();
        store(
            &scene,
            "Editor preference",
            "# Editor preference\n\nUser prefers vim for editing.\n",
        );
        assert!(
            scene
                .source
                .resolve(&request("what editor do I prefer?", None))
                .await
                .is_empty()
        );
        assert!(
            scene
                .source
                .resolve(&request("what editor do I prefer?", Some("system")))
                .await
                .is_empty()
        );
        assert!(
            scene
                .source
                .resolve(&request("   ", Some(ALICE)))
                .await
                .is_empty()
        );
        let uninstalled = MemorySource::new(Arc::new(RwLock::new(None)), scene.config.clone());
        assert!(
            uninstalled
                .resolve(&request("what editor do I prefer?", Some(ALICE)))
                .await
                .is_empty()
        );
        assert!(memory_turn(&ContextBundle::empty()).is_none());
    }

    fn section(id: u128, content: String) -> ContextSection {
        ContextSection {
            source: SOURCE_NAME,
            kind: ContextKind::Memory,
            token_estimate: content.len() / 4,
            content,
            priority: SectionPriority::Normal,
            relevance: 1.0,
            key: ContextKey::Memory(MemoryUsed {
                id: openalpaca_storage::memory_files::NoteId::parse(&format!(
                    "00000000-0000-4000-8000-{id:012x}"
                ))
                .unwrap(),
                revision: "a".repeat(64),
                reason: crate::memory::service::UsedReason::Recall,
            }),
            injection: InjectionMode::UserMessage {
                tag: "memory_context".to_string(),
                trust: TrustLevel::Untrusted,
            },
        }
    }

    /// Design 09 §4.5: `<memory_suggestions>` rides the memory message, under
    /// the trust label, after the notes — and counts against the same 8 KiB:
    /// the lowest-ranked recalled note gives way, and `used` names only the
    /// notes that stayed. Without a block the turn is exactly `memory_turn`'s.
    #[test]
    fn the_suggestions_block_counts_against_the_memory_budget() {
        let sections = vec![
            section(
                1,
                format!("[memory id=1]\n{}\n[/memory]\n", "a".repeat(3000)),
            ),
            section(
                2,
                format!("[memory id=2]\n{}\n[/memory]\n", "b".repeat(3000)),
            ),
            section(
                3,
                format!("[memory id=3]\n{}\n[/memory]\n", "c".repeat(1500)),
            ),
        ];
        let bundle = ContextBundle {
            total_tokens: sections.iter().map(|s| s.token_estimate).sum(),
            sections,
            available_budget: 8192,
        };
        let plain = memory_turn(&bundle).unwrap();
        assert_eq!(plain.used.len(), 3);
        assert!(plain.content.len() <= RECALL_TOTAL_BYTES);
        assert_eq!(memory_turn_with(&bundle, None), Some(plain.clone()));

        let block = format!(
            "<memory_suggestions>\n{}\n</memory_suggestions>\n",
            "s".repeat(1200)
        );
        let turn = memory_turn_with(&bundle, Some(&block)).unwrap();
        assert!(
            turn.content.len() <= RECALL_TOTAL_BYTES,
            "{}",
            turn.content.len()
        );
        assert_eq!(turn.used.len(), 2, "the lowest-ranked note gave way");
        assert_eq!(turn.used, plain.used[..2].to_vec());
        assert!(turn.content.starts_with("<memory_context>\n"));
        assert!(turn.content.contains(crate::memory::service::TRUST_LABEL));
        assert!(
            turn.content
                .ends_with(&format!("{block}</memory_context>\n")),
            "after the notes, inside the wrapper"
        );
        assert!(!turn.content.contains("ccc"));

        // With nothing recalled, the block alone is the turn's memory message.
        let alone = memory_turn_with(&ContextBundle::empty(), Some(&block)).unwrap();
        assert!(alone.used.is_empty());
        assert!(alone.content.contains(&block));
        assert!(memory_turn_with(&ContextBundle::empty(), None).is_none());
    }

    /// A subagent's package (`ContextManager::distill` → `to_bundle`) keys
    /// its sections by package name, not by note: the memory it carries is
    /// still rendered for the turn, and nothing is reported as used.
    #[test]
    fn a_distilled_memory_section_is_rendered_but_reports_no_used_entry() {
        use crate::prompt_ctx::package::{ContextPackage, PackageSection, PackageSectionKind};
        let package = ContextPackage {
            sections: vec![PackageSection {
                kind: PackageSectionKind::Memory,
                content: "[memory id=x]\nUser prefers vim.\n[/memory]\n".to_string(),
                token_estimate: 8,
                priority: SectionPriority::Normal,
            }],
            total_tokens: 8,
            budget: 100,
            sub_agent_window: 8192,
        };
        let turn = memory_turn(&package.to_bundle()).expect("the section is memory");
        assert!(turn.content.contains("User prefers vim."));
        assert!(turn.used.is_empty());
    }

    // ── The per-turn delivery, byte for byte (design 09 §13.2, L10b) ─────

    /// `ALICE`'s owner key: the directory her home partition lives in.
    const ALICE_KEY: &str = "32acea7eb739b08afd959db91e46d450";

    /// One note file in the canon (design §4), dropped into the partition by
    /// hand so that its id, its dates — and so its revision and its path —
    /// are the same on every run.
    struct FixedNote {
        id: &'static str,
        title: &'static str,
        kind: &'static str,
        created_at: &'static str,
        observed_at: Option<&'static str>,
        window: Option<(&'static str, &'static str)>,
        pinned: bool,
        source: Option<&'static str>,
        body: &'static str,
    }

    impl FixedNote {
        fn file(&self) -> String {
            let window = self.window.unwrap_or(("null", "null"));
            let sources = match self.source {
                Some(id) => format!("sources:\n  - type: message\n    id: {id}\n"),
                None => "sources: []\n".to_string(),
            };
            format!(
                "---\nschema: 1\nid: {id}\ntitle: {title}\nkind: {kind}\nstatus: active\nfact_key: null\naliases: []\ntags: []\ncreated_at: {created}\nupdated_at: {created}\nobserved_at: {observed}\nvalid_from: {from}\nvalid_to: {to}\nreview_after: null\npinned: {pinned}\n{sources}supersedes: []\n---\n{body}",
                id = self.id,
                title = self.title,
                kind = self.kind,
                created = self.created_at,
                observed = self.observed_at.unwrap_or("null"),
                from = window.0,
                to = window.1,
                pinned = self.pinned,
                body = self.body,
            )
        }
    }

    /// A pinned note, five notes the golden question matches and one it does
    /// not: more matches than the index-first delivery excerpts (design 09
    /// §13.2), so the two deliveries differ on this scene.
    const GOLDEN_NOTES: &[FixedNote] = &[
        FixedNote {
            id: "00000000-0000-4000-8000-00000000a001",
            title: "Working hours",
            kind: "preference",
            created_at: "2026-09-01T09:00:00Z",
            observed_at: None,
            window: None,
            pinned: true,
            source: None,
            body: "# Working hours\n\nThe user works from 09:00 to 17:00 Pacific time.\n",
        },
        FixedNote {
            id: "00000000-0000-4000-8000-00000000a002",
            title: "Editor preference",
            kind: "preference",
            created_at: "2026-09-02T10:00:00Z",
            observed_at: None,
            window: None,
            pinned: false,
            source: Some("41"),
            body: "# Editor preference\n\n## Knowledge\nThe user prefers vim for editing.\n",
        },
        FixedNote {
            id: "00000000-0000-4000-8000-00000000a003",
            title: "Editor theme",
            kind: "preference",
            created_at: "2026-09-03T10:00:00Z",
            observed_at: None,
            window: None,
            pinned: false,
            source: None,
            body: "# Editor theme\n\nThe editor uses the solarized dark theme.\n",
        },
        FixedNote {
            id: "00000000-0000-4000-8000-00000000a004",
            title: "Editor font",
            kind: "fact",
            created_at: "2026-09-04T10:00:00Z",
            observed_at: None,
            window: Some(("2026-01-01T00:00:00Z", "2099-01-01T00:00:00Z")),
            pinned: false,
            source: None,
            body: "# Editor font\n\nThe editor font is JetBrains Mono at 14 pt.\n",
        },
        FixedNote {
            id: "00000000-0000-4000-8000-00000000a005",
            title: "Editor plugins",
            kind: "reference",
            created_at: "2026-09-05T10:00:00Z",
            observed_at: None,
            window: None,
            pinned: false,
            source: None,
            body: "# Editor plugins\n\nThe editor runs the fzf plugin and a git gutter.\n",
        },
        FixedNote {
            id: "00000000-0000-4000-8000-00000000a006",
            title: "Editor keymap",
            kind: "procedure",
            created_at: "2026-09-06T10:00:00Z",
            observed_at: Some("2026-08-30T08:00:00Z"),
            window: None,
            pinned: false,
            source: None,
            body: "# Editor keymap\n\nThe leader key in the editor is the space bar.\n",
        },
        FixedNote {
            id: "00000000-0000-4000-8000-00000000a007",
            title: "Coffee",
            kind: "preference",
            created_at: "2026-09-07T10:00:00Z",
            observed_at: None,
            window: None,
            pinned: false,
            source: None,
            body: "# Coffee\n\nThe user drinks espresso after lunch.\n",
        },
    ];

    /// The golden question.
    const GOLDEN_QUERY: &str = "what editor do I prefer?";

    /// [`scene`] with [`GOLDEN_NOTES`] in Alice's home partition, the index
    /// in step with them.
    fn golden_scene() -> Scene {
        let scene = scene();
        // The partition's control snapshot comes with its first service
        // write; this note matches nothing the golden question asks.
        store(
            &scene,
            "Parking",
            "# Parking\n\nThe car is parked in bay 12.\n",
        );
        let notes = scene
            ._root
            .path()
            .join("home")
            .canonicalize()
            .unwrap()
            .join("memory")
            .join("owners")
            .join(ALICE_KEY)
            .join("notes");
        for note in GOLDEN_NOTES {
            let id = openalpaca_storage::memory_files::NoteId::parse(note.id).unwrap();
            let name = openalpaca_storage::memory_files::Note::file_name_for(id, note.title);
            std::fs::write(notes.join(name), note.file()).unwrap();
        }
        let owner = scene
            .service
            .context(
                ALICE,
                WorkspaceHint::None,
                Surface::UserSurface,
                Capabilities::ALL,
                Attribution::default(),
            )
            .unwrap();
        scene.service.sync(&owner).unwrap();
        scene
    }

    /// The turn's memory message and its `memory_used`, as one text: what
    /// the model reads, then what the turn reports.
    fn golden_text(turn: &MemoryTurn) -> String {
        let mut out = turn.content.clone();
        out.push_str("=== memory_used ===\n");
        for used in &turn.used {
            out.push_str(&format!("{} {}\n", used.id, used.reason.as_str()));
        }
        out
    }

    /// The golden question's turn on [`golden_scene`], through the source
    /// and the renderer the main loop and the skill tier use.
    async fn golden_turn(scene: &Scene) -> MemoryTurn {
        golden_turn_for(scene, &request(GOLDEN_QUERY, Some(ALICE))).await
    }

    /// [`golden_turn`] for a given request.
    async fn golden_turn_for(scene: &Scene, request: &ContextRequest) -> MemoryTurn {
        let sections = scene.source.resolve(request).await;
        let bundle = ContextBundle {
            total_tokens: sections.iter().map(|s| s.token_estimate).sum(),
            sections,
            available_budget: 8192,
        };
        memory_turn(&bundle).expect("the golden question recalls notes")
    }

    /// **Design 09 §13.2: `recall_mode = "excerpts"` is today's delivery,
    /// byte for byte.** The per-turn memory message — the wrapper, the trust
    /// label, the pinned note, then every recalled note as an excerpt under
    /// its full header — and the turn's `memory_used`, as they were before
    /// index-first recall existed (characterized at `a90202ed`).
    #[tokio::test]
    async fn excerpts_mode_is_todays_delivery_byte_for_byte() {
        let scene = golden_scene();
        scene.set_recall_mode(RecallMode::Excerpts);
        let turn = golden_turn(&scene).await;
        assert_eq!(
            golden_text(&turn),
            include_str!("memory_recall_excerpts.golden.txt")
        );
    }

    /// **Design 09 §13.2: `recall_mode = "index"`, the default.** The same
    /// question on the same notes: the pinned note and the top 2 recalled
    /// notes as excerpts, byte for byte as the excerpts delivery renders
    /// them, then the index block — its one header line and one line per
    /// further note, `<id> <date> <kind> “<title>” — <reason>`, the date
    /// `observed_at` when the note has one — inside the same wrapper under
    /// the same trust label. `memory_used` names the listed notes last, with
    /// reason `index`.
    #[tokio::test]
    async fn index_mode_is_pinned_two_excerpts_and_the_index_block() {
        let scene = golden_scene();
        assert_eq!(
            scene.config.load().orchestrator.memory.recall_mode,
            RecallMode::Index,
            "index is the default"
        );
        let turn = golden_turn(&scene).await;
        let golden = include_str!("memory_recall_index.golden.txt");
        assert_eq!(golden_text(&turn), golden);
        // The excerpts are the excerpts delivery's first three blocks, and
        // nothing in the index block is a body.
        let excerpts = include_str!("memory_recall_excerpts.golden.txt");
        let (head, _) = golden.split_once(INDEX_BLOCK_HEADER).unwrap();
        assert!(excerpts.starts_with(head), "{head}");
        assert!(turn.content.len() <= RECALL_TOTAL_BYTES);
    }

    /// The golden question as a file-based skill's turn: the skill path,
    /// carrying the skill's frontmatter as the skill tier does
    /// (`orchestrator/skill/invocation.rs`).
    fn skill_request(
        requires_capabilities: &[&str],
        allow: &[&str],
        deny: &[&str],
    ) -> ContextRequest {
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut frontmatter = crate::middleware::skill::types::SkillFrontmatter {
            name: "Explain code".into(),
            description: "Explain a piece of code".into(),
            requires_capabilities: names(requires_capabilities),
            ..Default::default()
        };
        frontmatter.tools.allow = names(allow);
        frontmatter.tools.deny = names(deny);
        ContextRequest {
            path: ExecutionPath::SkillInvocation {
                skill_id: "explain-code".into(),
            },
            skill: Some(Arc::new(crate::middleware::skill::types::SkillDocument {
                frontmatter,
                body: String::new(),
                sections: Default::default(),
            })),
            ..request(GOLDEN_QUERY, Some(ALICE))
        }
    }

    /// **A skill that cannot call `memory_read` gets the excerpts delivery**
    /// (L10b review, finding 1). The skill tier offers a file-based skill
    /// exactly the tools its frontmatter resolves — `requires_capabilities`
    /// when it names any, else `tools.allow`, less `tools.deny` — and every
    /// shipped skill names only `file_read` or `shell_execute`: under the
    /// default `index` such a turn would get two excerpts and a block headed
    /// "read with memory_read" it cannot follow. It gets today's bytes
    /// instead; a skill that can call `memory_read` follows the key.
    #[tokio::test]
    async fn a_skill_that_cannot_call_memory_read_gets_the_excerpts_delivery() {
        let scene = golden_scene();
        assert_eq!(
            scene.config.load().orchestrator.memory.recall_mode,
            RecallMode::Index,
            "index is the default"
        );
        let excerpts = include_str!("memory_recall_excerpts.golden.txt");
        let index = include_str!("memory_recall_index.golden.txt");

        // `/explain-code`'s own shape, and a skill that denies the tool.
        for request in [
            skill_request(&["file_read"], &["file_read"], &[]),
            skill_request(&["memory_read"], &[], &["memory_read"]),
            // `tools.allow` is not read when `requires_capabilities` names
            // anything.
            skill_request(&["file_read"], &["memory_read"], &[]),
        ] {
            let turn = golden_turn_for(&scene, &request).await;
            assert!(!turn.content.contains(INDEX_BLOCK_HEADER));
            assert_eq!(golden_text(&turn), excerpts);
        }
        // The skill path with no skill document offers no `memory_read`.
        let mut bare = skill_request(&[], &[], &[]);
        bare.skill = None;
        assert_eq!(golden_text(&golden_turn_for(&scene, &bare).await), excerpts);

        // A skill that can call it: by capability, or by the legacy name.
        for request in [
            skill_request(&["memory_read"], &[], &[]),
            skill_request(&[], &["memory_read"], &[]),
        ] {
            assert_eq!(golden_text(&golden_turn_for(&scene, &request).await), index);
        }
    }

    fn keyed(id: u128, reason: UsedReason, content: &str) -> ContextSection {
        let mut s = section(id, content.to_string());
        if let ContextKey::Memory(used) = &mut s.key {
            used.reason = reason;
        }
        s
    }

    /// The index block is rendered from the sections a context manager
    /// kept: its header goes before the first listed note wherever the
    /// manager put it, only when one is kept, and a `<memory_suggestions>`
    /// block that needs the room takes it from the listed notes first —
    /// lowest ranked, then the header with the last of them.
    #[test]
    fn listed_sections_render_under_one_header_and_give_way_first() {
        let line = |n: u128| format!("{n} 2026-09-01T00:00:00Z fact “Note {n}” — term\n");
        let bundle = ContextBundle {
            sections: vec![
                keyed(1, UsedReason::Index, &line(1)),
                keyed(
                    2,
                    UsedReason::Recall,
                    &format!("[memory id=2]\n{}\n[/memory]\n", "b".repeat(3000)),
                ),
                keyed(3, UsedReason::Index, &line(3)),
            ],
            total_tokens: 0,
            available_budget: 8192,
        };
        let turn = memory_turn(&bundle).unwrap();
        let (excerpts, index) = turn.content.split_once(INDEX_BLOCK_HEADER).unwrap();
        assert!(excerpts.contains("[memory id=2]"), "{}", turn.content);
        assert_eq!(index, format!("{}{}</memory_context>\n", line(1), line(3)));
        let reasons: Vec<UsedReason> = turn.used.iter().map(|u| u.reason).collect();
        assert_eq!(
            reasons,
            [UsedReason::Recall, UsedReason::Index, UsedReason::Index]
        );

        // A block that leaves room for the excerpt but for no line: both
        // listed notes give way, and the header with them.
        let room = RECALL_TOTAL_BYTES - wrap_delivery([bundle.sections[1].content.as_str()]).len();
        let block = format!(
            "<memory_suggestions>\n{}\n</memory_suggestions>\n",
            "s".repeat(room - 45)
        );
        let turn = memory_turn_with(&bundle, Some(&block)).unwrap();
        assert!(
            turn.content.len() <= RECALL_TOTAL_BYTES,
            "{}",
            turn.content.len()
        );
        assert!(
            !turn.content.contains(INDEX_BLOCK_HEADER),
            "{}",
            turn.content
        );
        assert!(turn.content.contains("[memory id=2]"));
        assert_eq!(turn.used.len(), 1);
        assert_eq!(turn.used[0].reason, UsedReason::Recall);
    }

    // ------------------------------------------------------------------------
    // Scale slice S7: a reply never waits on memory without a bound
    // ------------------------------------------------------------------------

    /// Holds the service's reply-path reads still — recall and the pending
    /// suggestions wait at their start — until the returned sender is
    /// dropped or sent to. The lock is held on its own thread, never across
    /// an `.await` of the test.
    fn stall_reads(service: &MemoryService) -> std::sync::mpsc::Sender<()> {
        let gates = service.gates.clone();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let (held, holding) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let _hold = gates.reads.write().unwrap();
            held.send(()).unwrap();
            let _ = released.recv();
        });
        holding.recv().unwrap();
        release
    }

    /// The bound is a constant, chosen from the L8 benches: over twice the
    /// slowest healthy recall p95 measured (661 ms at 100K notes), and
    /// under every unbounded case they found (2.9 s and up).
    #[test]
    fn the_reply_memory_deadline_is_one_and_a_half_seconds() {
        assert_eq!(
            REPLY_MEMORY_DEADLINE,
            std::time::Duration::from_millis(1_500)
        );
    }

    /// A recall that has not answered by the deadline is left behind: the
    /// turn gets no memory sections — as it does on a recall error — with
    /// one WARN naming the partition and the time waited, and no note text.
    /// The same recall, not held, delivers.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn a_recall_past_the_deadline_leaves_the_turn_without_memory() {
        let scene = scene();
        store(
            &scene,
            "Editor preference",
            "# Editor preference\n\nThe user prefers vim for secret-marmalade work.\n",
        );
        let release = stall_reads(&scene.service);
        let started = std::time::Instant::now();
        let sections = scene
            .source
            .resolve_within(
                &request("what editor do I prefer?", Some(ALICE)),
                std::time::Duration::from_millis(100),
            )
            .await;
        assert!(sections.is_empty(), "the turn continues without memory");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        drop(release);
        let home_key = openalpaca_storage::memory_files::home_partition_key(
            &openalpaca_storage::memory_files::owner_key(ALICE),
        );
        logs_assert(|lines: &[&str]| {
            let warns: Vec<&&str> = lines
                .iter()
                .filter(|l| l.contains("WARN") && l.contains("reply's deadline"))
                .collect();
            match warns.as_slice() {
                [line] if line.contains(&home_key)
                    && line.contains("elapsed_ms")
                    && !line.contains("marmalade")
                    && !line.contains("Editor") =>
                {
                    Ok(())
                }
                other => Err(format!("one WARN with the partition and no text: {other:?}")),
            }
        });
        let sections = scene
            .source
            .resolve_within(
                &request("what editor do I prefer?", Some(ALICE)),
                std::time::Duration::from_secs(30),
            )
            .await;
        assert_eq!(sections.len(), 1, "unheld, the recall delivers");
    }

    /// The pending suggestions a turn is told about are bound the same way:
    /// past the deadline the turn goes without the block.
    #[tokio::test]
    async fn pending_suggestions_past_the_deadline_leave_the_turn_without_the_block() {
        let scene = scene();
        let ctx = crate::tools::registry::ToolContext {
            principal: Some(crate::security::policy::Principal::User {
                global_id: ALICE.to_string(),
            }),
            owner_id: Some(ALICE.to_string()),
            lane_key: Some(format!("{ALICE}:gui")),
            source: Some("gui".into()),
            ..Default::default()
        };
        let release = stall_reads(&scene.service);
        let started = std::time::Instant::now();
        let block = memory_suggestions_block_within(
            scene.service.clone(),
            &ctx,
            std::time::Duration::from_millis(100),
        )
        .await;
        assert!(block.is_none());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        drop(release);
    }
}
