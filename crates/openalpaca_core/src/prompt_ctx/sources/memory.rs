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

use crate::memory::service::{
    Attribution, Capabilities, MemoryError, MemoryService, MemoryUsed, PendingSuggestion,
    RECALL_TOTAL_BYTES, RecallBudget, Surface, WorkspaceHint, wrap_delivery,
};
use crate::prompt_ctx::section::{
    ContextBundle, ContextKey, ContextKind, ContextSection, InjectionMode, SectionPriority,
    TrustLevel,
};
use crate::prompt_ctx::sources::{ContextRequest, ContextSource};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// The `MemorySource` name, and the `source` of every section it emits.
pub const SOURCE_NAME: &str = "memory";

pub struct MemorySource {
    /// The daemon's service, installed after construction
    /// (`Orchestrator::set_memory_service`); `None` until then, and nothing
    /// is recalled.
    service: Arc<RwLock<Option<Arc<MemoryService>>>>,
}

impl MemorySource {
    pub fn new(service: Arc<RwLock<Option<Arc<MemoryService>>>>) -> Self {
        Self { service }
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
        let recalled = tokio::task::spawn_blocking(move || {
            service.recall(&ctx, &query, RecallBudget::default())
        })
        .await;
        let result = match recalled {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "memory recall failed; the turn runs without it");
                return vec![];
            }
            Err(e) => {
                tracing::warn!(error = %e, "memory recall did not complete; the turn runs without it");
                return vec![];
            }
        };
        result
            .notes
            .into_iter()
            .map(|note| {
                let used = MemoryUsed {
                    id: note.id,
                    revision: note.revision.clone(),
                    reason: note.reason,
                };
                let content = note.render();
                ContextSection {
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
                }
            })
            .collect()
    }
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

/// [`memory_turn`], with a `<memory_suggestions>` block after the notes
/// (design 09 §4.5): delivered in the same quoted-data wrapper under the
/// trust label, and counted against the same 8 KiB — the lowest-ranked
/// recalled notes give way to it, and `used` names only what stayed.
pub fn memory_turn_with(bundle: &ContextBundle, suggestions: Option<&str>) -> Option<MemoryTurn> {
    let mut sections: Vec<&ContextSection> = bundle.memory_sections().collect();
    if let Some(block) = suggestions {
        while !sections.is_empty()
            && wrap_delivery(
                sections
                    .iter()
                    .map(|s| s.content.as_str())
                    .chain(std::iter::once(block)),
            )
            .len()
                > RECALL_TOTAL_BYTES
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
    let content = wrap_delivery(
        sections
            .iter()
            .map(|s| s.content.as_str())
            .chain(suggestions),
    );
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
    let listed = tokio::task::spawn_blocking(move || {
        service.pending_suggestions(&access, SUGGESTIONS_LISTED, &raised)
    })
    .await;
    match listed {
        Ok(Ok(items)) => render_memory_suggestions(&items),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "memory suggestions could not be read; the turn runs without them");
            None
        }
        Err(e) => {
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
    use crate::memory::scope_context::MemoryScopeContext;
    use crate::memory::service::StoreInput;
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
        Scene {
            _env: env,
            _root: root,
            service,
            source: MemorySource::new(slot),
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
        let uninstalled = MemorySource::new(Arc::new(RwLock::new(None)));
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
}
