//! Automatic memory extraction from a chat turn (design §6 *Promotion and
//! correction*, owner decision MD-2; plan 3.4).
//!
//! After a turn is answered, the configured conversation model is asked what
//! durable knowledge the turn carried, and each item is written through
//! [`MemoryService`] on the `Extraction` surface:
//!
//! - what the **user said in their own words** becomes an **active** note
//!   with origin `extracted_user` — the *recently learned* list, one-step
//!   undo;
//! - what was **inferred from the assistant's reply** becomes a
//!   **candidate** with origin `extracted_other`, outside recall until it is
//!   confirmed.
//!
//! **The input is the message as typed** — `HandleRequest.content`, never the
//! model input, which on an attachment turn carries the files' extracted
//! text (U2). An uploaded document therefore cannot mint an
//! `extracted_user` note: the extractor never sees it.
//!
//! Bounded per turn by count (`[orchestrator.costs]
//! extract_max_items_per_turn`, the items past it dropped and counted) and by
//! cost (the existing frequency gate and daily cap); an item under
//! `memory_confidence_threshold` is dropped and the number is never stored.
//! It runs as a tracked job that stops on the orchestrator's shutdown token,
//! attributed to the lane, the request and the source.
//!
//! What this file used to do — write "profile" traits into `USER.md` — is
//! gone (MD-3): `USER.md` is the owner's to edit, through `update_persona`
//! during onboarding and `openalpaca persona edit user` / Settings → Persona
//! after it.

use crate::daemon_config::DaemonConfig;
use crate::memory::service::{
    Attribution, Capabilities, ExtractionProvenance, MemoryError, MemoryService, StoreInput,
    Surface, WorkspaceHint,
};
use crate::memory::task_extraction::parse_json_response;
use arc_swap::ArcSwap;
use openalpaca_llm::{ChatMessage, LlmRouter, RequestContext, RouterRequest};
use openalpaca_storage::Database;
use openalpaca_storage::memory_files::{Dedup, NoteKind, NoteSource, SourceType};
use openalpaca_storage::repository::LlmUsageRepository;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// The usage label the extractor's calls are billed and gated under.
pub(crate) const AGENT_LABEL: &str = "orchestrator_user_extract";

/// What the extractor needs from the orchestrator.
pub(crate) struct TurnExtractionDeps {
    pub db: Database,
    pub router: Arc<LlmRouter>,
    pub daemon_config: Arc<ArcSwap<DaemonConfig>>,
    pub memory_service: Arc<MemoryService>,
    /// Per-lane turn counter for the frequency gate.
    pub turn_counter: Arc<Mutex<HashMap<String, usize>>>,
    /// The orchestrator's shutdown token: a cancelled extraction writes
    /// nothing.
    pub cancel: CancellationToken,
}

/// One answered turn, as the extractor sees it.
pub(crate) struct ExtractionTurn {
    /// The turn's owner: a `global_user` row, or the service refuses.
    pub owner_id: String,
    pub lane_key: String,
    pub source: String,
    pub request_id: Uuid,
    /// The MD-5 root the request resolved, never the daemon's CWD.
    pub project_root: Option<PathBuf>,
    /// The message **as typed** (`HandleRequest.content`).
    pub user_message: String,
    pub assistant_response: String,
}

/// What one extraction did, counted without note bodies (design §8).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ExtractionReport {
    /// Notes written: `active + candidates`.
    pub stored: usize,
    /// Active `extracted_user` notes — the user's own words.
    pub active: usize,
    /// `extracted_other` candidates — inferred from the assistant's reply.
    pub candidates: usize,
    /// Items an existing note already held byte for byte.
    pub deduplicated: usize,
    /// Items past `extract_max_items_per_turn`.
    pub dropped_over_cap: usize,
    /// Items under `memory_confidence_threshold`.
    pub dropped_below_threshold: usize,
    /// Items with no content or an unknown kind.
    pub dropped_invalid: usize,
    /// Items the service refused.
    pub failed: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExtractionOutcome {
    /// A gate closed before any model call: the reason names it.
    Skipped(&'static str),
    /// No model could answer (§7b P10): one WARN, the turn is unaffected.
    Unavailable,
    /// The shutdown token fired before the answer was written.
    Cancelled,
    Extracted(ExtractionReport),
}

/// Runs one turn's extraction. Every failure is logged, none propagates.
pub(crate) async fn extract_turn_memories(
    deps: &TurnExtractionDeps,
    turn: ExtractionTurn,
) -> ExtractionOutcome {
    let dcfg = deps.daemon_config.load();

    // Content filter: skip short messages and slash commands.
    if turn.user_message.len() < dcfg.orchestrator.costs.extract_min_content_len
        || turn.user_message.starts_with('/')
    {
        return ExtractionOutcome::Skipped("content_filter");
    }

    // Frequency gate: only extract every N turns per lane.
    {
        let mut counter = deps
            .turn_counter
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let count = counter.entry(turn.lane_key.clone()).or_insert(0);
        *count += 1;
        if *count < dcfg.orchestrator.costs.extract_every_n_turns {
            return ExtractionOutcome::Skipped("frequency_gate");
        }
        *count = 0;
    }

    // The owner must be a `global_user` row (design §3): a `system` turn or
    // an unlinked external identity has no partition, and no model call is
    // spent finding that out.
    let hint = match &turn.project_root {
        Some(root) => WorkspaceHint::Root(root.as_path()),
        None => WorkspaceHint::None,
    };
    let ctx = match deps.memory_service.context(
        &turn.owner_id,
        hint,
        Surface::Extraction,
        Capabilities::EXTRACTION,
        Attribution {
            lane_key: Some(turn.lane_key.clone()),
            task_id: None,
            request_id: Some(turn.request_id),
            source: Some(turn.source.clone()),
        },
    ) {
        Ok(ctx) => ctx,
        Err(MemoryError::NoOwner(_)) => {
            tracing::debug!(lane_key = %turn.lane_key, "memory extraction: no owner, nothing to extract");
            return ExtractionOutcome::Skipped("no_owner");
        }
        Err(e) => {
            tracing::warn!("memory extraction: context refused: {e}");
            return ExtractionOutcome::Skipped("context_refused");
        }
    };

    // Budget pre-check — agent-specific cost under `AGENT_LABEL`.
    let extract_cost = deps
        .router
        .cost_tracker
        .get_agent_usage(AGENT_LABEL)
        .await
        .map(|s| s.total_cost_usd)
        .unwrap_or(0.0);
    if extract_cost > dcfg.orchestrator.costs.extract_max_daily_cost_usd {
        tracing::debug!("memory extraction skipped: cost ${extract_cost:.2} exceeds cap");
        return ExtractionOutcome::Skipped("cost_cap");
    }

    let max_items = dcfg.orchestrator.costs.extract_max_items_per_turn.max(1);
    let threshold = dcfg.orchestrator.memory.memory_confidence_threshold;

    // Truncate inputs to keep the prompt small.
    let user_trunc: String = turn.user_message.chars().take(800).collect();
    let asst_trunc: String = turn.assistant_response.chars().take(400).collect();

    let user_prompt = format!(
        "<conversation_turn>\nUser: {user_trunc}\nAssistant: {asst_trunc}\n</conversation_turn>\n\n\
         Extract the durable knowledge this turn carries about the user: their \
         preferences, facts about them or their work, decisions they made, procedures \
         they follow, references they gave, or an episode worth remembering.\n\n\
         <output_schema>\n\
         {{\n\
           \"extractions\": [\n\
             {{\n\
               \"title\": \"a short title, at most 80 characters\",\n\
               \"content\": \"one concise statement\",\n\
               \"kind\": \"preference\" | \"fact\" | \"decision\" | \"procedure\" | \"reference\" | \"episode\",\n\
               \"stated_by\": \"user\" | \"assistant\",\n\
               \"confidence\": 0.0 to 1.0\n\
             }}\n\
           ]\n\
         }}\n\
         </output_schema>\n\n\
         <extraction_rules>\n\
         - \"stated_by\": \"user\" ONLY when the user said it in their own words in this turn.\n\
         - \"stated_by\": \"assistant\" when it is inferred from the assistant's reply or from \
         anything the user did not say themselves.\n\
         - Only extract what is clearly stated or strongly implied. Do not invent.\n\
         - Skip pleasantries, the question itself, and anything true only for this one reply.\n\
         - Be conservative: prefer fewer high-confidence extractions over many low-confidence ones.\n\
         - At most {max_items} extractions, most important first.\n\
         - If nothing can be extracted, return {{\"extractions\": []}}.\n\
         </extraction_rules>"
    );

    if let Some(ref m) = dcfg.orchestrator.costs.extraction_model {
        tracing::debug!(model = %m, "memory extraction using configured model");
    }
    let request = RouterRequest {
        model: dcfg.orchestrator.costs.extraction_model.clone(),
        messages: Arc::new(vec![
            ChatMessage::system(
                "<role>You are the memory extractor for OpenAlpaca.</role>\n\
                 <output_format>Output ONLY valid JSON matching the schema. No markdown fences, no commentary.</output_format>",
            ),
            ChatMessage::user(&user_prompt),
        ]),
        tools: Arc::new(vec![]),
        temperature: Some(0.0),
        max_tokens: Some(512),
        context: RequestContext {
            agent_id: Some(AGENT_LABEL.to_string()),
            task_id: None,
        },
        tool_choice: None,
        tools_token_estimate: None,
        enable_caching: false,
        // M2: a JSON object's worth of budget, and a local thinking model
        // spends all of it reasoning before it writes a character. An
        // internal utility call wants no reasoning.
        thinking: Some(openalpaca_llm::ThinkingConfig::Disabled),
        context_management: None,
        fallback_models: Vec::new(),
        ephemeral_system_notice: None,
    };
    drop(dcfg);

    let call_start = std::time::Instant::now();
    let response = tokio::select! {
        biased;
        _ = deps.cancel.cancelled() => {
            tracing::debug!(lane_key = %turn.lane_key, "memory extraction cancelled before the model answered");
            return ExtractionOutcome::Cancelled;
        }
        result = deps.router.complete(request) => match result {
            Ok(r) => r,
            Err(e) => {
                // §7b P10: one WARN, the turn is unaffected.
                tracing::warn!("memory extraction unavailable: {e}");
                return ExtractionOutcome::Unavailable;
            }
        },
    };
    let latency_ms = call_start.elapsed().as_millis() as i64;

    // Record LLM usage.
    let actual_model = response.model.as_str();
    let resolved_provider = deps
        .router
        .model_registry()
        .resolve_provider(actual_model)
        .map(|p| p.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let call_cost = deps.router.cost_tracker.calculate_cost(
        actual_model,
        response.usage.input_tokens as u32,
        response.usage.output_tokens as u32,
    );
    let usage_repo = LlmUsageRepository::new(&deps.db);

    let parsed = match parse_json_response(&response.content) {
        Some(v) => v,
        None => {
            tracing::warn!("memory extraction: malformed JSON from the model");
            let _ = usage_repo.record_and_log(
                AGENT_LABEL,
                None,
                &resolved_provider,
                actual_model,
                response.usage.input_tokens as i32,
                response.usage.output_tokens as i32,
                call_cost,
                latency_ms,
                "error",
                Some("JSON parse failure"),
            );
            return ExtractionOutcome::Extracted(ExtractionReport::default());
        }
    };
    if let Err(e) = usage_repo.record_and_log(
        AGENT_LABEL,
        None,
        &resolved_provider,
        actual_model,
        response.usage.input_tokens as i32,
        response.usage.output_tokens as i32,
        call_cost,
        latency_ms,
        "success",
        None,
    ) {
        tracing::warn!("Failed to persist extraction LLM usage: {e}");
    }

    if deps.cancel.is_cancelled() {
        return ExtractionOutcome::Cancelled;
    }

    let extractions = parsed
        .get("extractions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Source evidence: the typed message's own row, when the gateway has
    // persisted it (design §4: `sources` cite DB ids).
    let sources = message_row(&deps.db, &turn.lane_key, &turn.user_message)
        .map(|id| {
            vec![NoteSource {
                kind: SourceType::Message,
                id: id.to_string(),
                observed_at: None,
            }]
        })
        .unwrap_or_default();

    let mut report = ExtractionReport::default();
    for extraction in &extractions {
        let Some(item) = read_item(extraction) else {
            report.dropped_invalid += 1;
            continue;
        };
        if item.confidence < threshold {
            report.dropped_below_threshold += 1;
            continue;
        }
        if report.stored + report.deduplicated + report.failed >= max_items {
            report.dropped_over_cap += 1;
            continue;
        }
        let provenance = if item.stated_by_user {
            ExtractionProvenance::UserWords
        } else {
            ExtractionProvenance::OtherOutput
        };
        let input = StoreInput {
            title: item.title,
            body: item.body,
            kind: Some(item.kind),
            sources: sources.clone(),
            provenance: Some(provenance),
            ..Default::default()
        };
        match deps.memory_service.store(&ctx, input) {
            Ok(out) if out.dedup == Dedup::Existing => report.deduplicated += 1,
            Ok(_) => {
                report.stored += 1;
                if item.stated_by_user {
                    report.active += 1;
                } else {
                    report.candidates += 1;
                }
            }
            Err(e) => {
                tracing::warn!("memory extraction: the service refused a note: {e}");
                report.failed += 1;
            }
        }
    }

    tracing::info!(
        lane_key = %turn.lane_key,
        request_id = %turn.request_id,
        stored = report.stored,
        active = report.active,
        candidates = report.candidates,
        deduplicated = report.deduplicated,
        dropped_over_cap = report.dropped_over_cap,
        dropped_below_threshold = report.dropped_below_threshold,
        dropped_invalid = report.dropped_invalid,
        failed = report.failed,
        cost_usd = call_cost,
        "memory extraction finished"
    );
    ExtractionOutcome::Extracted(report)
}

/// One item of the model's answer, validated.
struct Item {
    title: String,
    body: String,
    kind: NoteKind,
    stated_by_user: bool,
    confidence: f64,
}

/// Reads one `extractions[]` entry. `None` is an invalid item: no content,
/// or a kind that is not one of the six. A missing kind is a `fact`.
fn read_item(value: &serde_json::Value) -> Option<Item> {
    let content = value.get("content")?.as_str()?.trim();
    if content.is_empty() {
        return None;
    }
    let kind = match value.get("kind").and_then(|v| v.as_str()) {
        None | Some("") => NoteKind::Fact,
        Some(k) => NoteKind::parse(k.trim())?,
    };
    let stated_by_user = value
        .get("stated_by")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s.trim().eq_ignore_ascii_case("user"));
    let confidence = value
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().take(80).collect::<String>())
        .unwrap_or_else(|| content.chars().take(80).collect());
    Some(Item {
        body: format!("# {title}\n\n{content}\n"),
        title,
        kind,
        stated_by_user,
        confidence,
    })
}

/// The newest `conversation_messages` row that is this lane's user message
/// with exactly this content — the row the gateway wrote for the turn — or
/// `None` when nothing matches (a caller that never persisted the turn).
fn message_row(db: &Database, lane_key: &str, content: &str) -> Option<i64> {
    db.with_connection(|conn| {
        let id: Option<i64> = conn
            .query_row(
                "SELECT id FROM conversation_messages \
                 WHERE lane_key = ?1 AND role = 'user' AND content = ?2 \
                 ORDER BY id DESC LIMIT 1",
                (lane_key, content),
                |row| row.get(0),
            )
            .ok();
        Ok(id)
    })
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::service::{ListInput, ReadInput};
    use crate::test_util::{HomeStoreGuard, RecordingProvider, router_recording};
    use openalpaca_llm::ThinkingConfig;
    use openalpaca_storage::IdentityRepository;
    use openalpaca_storage::memory_files::{NoteStatus, Origin};
    use openalpaca_storage::memory_index::MemoryIndex;

    const OWNER: &str = "user1";

    /// A temp home store, a database with the owner's `global_user` row,
    /// a service over an in-memory index, and a recording router.
    struct Scene {
        _env: HomeStoreGuard,
        _root: tempfile::TempDir,
        db: Database,
        service: Arc<MemoryService>,
        provider: Arc<RecordingProvider>,
        cancel: CancellationToken,
    }

    impl Scene {
        fn new(reply: &str) -> Self {
            let root = tempfile::tempdir().unwrap();
            let home = root.path().join("home");
            std::fs::create_dir_all(&home).unwrap();
            let env = HomeStoreGuard::set(&home.canonicalize().unwrap());
            let db = Database::open(&root.path().join("t.db")).unwrap();
            IdentityRepository::new(&db)
                .create_global_user(OWNER, None)
                .unwrap();
            let service =
                MemoryService::new(Some(db.clone()), MemoryIndex::open_in_memory().unwrap());
            service.rebuild_index().unwrap();
            Self {
                _env: env,
                _root: root,
                db,
                service: Arc::new(service),
                provider: RecordingProvider::new(reply),
                cancel: CancellationToken::new(),
            }
        }

        fn deps(&self) -> TurnExtractionDeps {
            let mut cfg = DaemonConfig::default();
            // One turn is enough for these tests; the frequency gate is not
            // what is under test.
            cfg.orchestrator.costs.extract_every_n_turns = 1;
            TurnExtractionDeps {
                db: self.db.clone(),
                router: router_recording(self.provider.clone()),
                daemon_config: Arc::new(ArcSwap::from_pointee(cfg)),
                memory_service: self.service.clone(),
                turn_counter: Arc::new(Mutex::new(HashMap::new())),
                cancel: self.cancel.clone(),
            }
        }

        fn turn(&self, owner: &str, message: &str) -> ExtractionTurn {
            ExtractionTurn {
                owner_id: owner.to_string(),
                lane_key: format!("{owner}:cli"),
                source: "cli".to_string(),
                request_id: Uuid::new_v4(),
                project_root: None,
                user_message: message.to_string(),
                assistant_response: "Noted.".to_string(),
            }
        }

        fn user_ctx(&self) -> crate::memory::service::MemoryAccessContext {
            self.service
                .context(
                    OWNER,
                    WorkspaceHint::None,
                    Surface::UserSurface,
                    Capabilities::ALL,
                    Attribution::default(),
                )
                .unwrap()
        }

        fn active(&self) -> Vec<crate::memory::service::ListItem> {
            self.service
                .list(&self.user_ctx(), ListInput::default())
                .unwrap()
                .items
        }

        fn candidates(&self) -> Vec<crate::memory::service::ListItem> {
            self.service
                .list(
                    &self.user_ctx(),
                    ListInput {
                        status: Some(NoteStatus::Candidate),
                        include_candidates: true,
                        ..Default::default()
                    },
                )
                .unwrap()
                .items
        }
    }

    const USER_ITEM: &str = r#"{"extractions": [
        {"title": "Editor preference", "content": "The user prefers vim for editing.",
         "kind": "preference", "stated_by": "user", "confidence": 0.9}
    ]}"#;

    const ASSISTANT_ITEM: &str = r#"{"extractions": [
        {"title": "Probably uses Rust", "content": "The user's project is probably written in Rust.",
         "kind": "fact", "stated_by": "assistant", "confidence": 0.8}
    ]}"#;

    /// **MD-2.** A preference stated in the user's own words becomes an
    /// active note with origin `extracted_user`, in recall and on the
    /// recently-learned list.
    #[tokio::test]
    async fn a_user_stated_preference_becomes_an_active_extracted_user_note() {
        let scene = Scene::new(USER_ITEM);
        let outcome = extract_turn_memories(
            &scene.deps(),
            scene.turn(OWNER, "I prefer vim for editing, always have."),
        )
        .await;
        assert_eq!(
            outcome,
            ExtractionOutcome::Extracted(ExtractionReport {
                stored: 1,
                active: 1,
                ..Default::default()
            })
        );

        let active = scene.active();
        assert_eq!(active.len(), 1, "{active:?}");
        assert_eq!(active[0].title, "Editor preference");
        assert_eq!(active[0].status, NoteStatus::Active);
        assert_eq!(active[0].origin, Origin::ExtractedUser);
        assert_eq!(active[0].kind, NoteKind::Preference);
        assert!(scene.candidates().is_empty());

        let learned = scene
            .service
            .recently_learned(&scene.user_ctx(), None, None)
            .unwrap();
        assert_eq!(learned.len(), 1);
        assert_eq!(learned[0].id, active[0].id);
    }

    /// **MD-2.** Something inferred from the assistant's reply is a
    /// candidate — outside the active list until it is confirmed.
    #[tokio::test]
    async fn an_assistant_claim_becomes_a_candidate() {
        let scene = Scene::new(ASSISTANT_ITEM);
        let outcome = extract_turn_memories(
            &scene.deps(),
            scene.turn(
                OWNER,
                "Can you look at my project and tell me what it does?",
            ),
        )
        .await;
        assert_eq!(
            outcome,
            ExtractionOutcome::Extracted(ExtractionReport {
                stored: 1,
                candidates: 1,
                ..Default::default()
            })
        );
        assert!(scene.active().is_empty(), "a candidate is not active");
        let candidates = scene.candidates();
        assert_eq!(candidates.len(), 1, "{candidates:?}");
        assert_eq!(candidates[0].status, NoteStatus::Candidate);
        assert_eq!(candidates[0].origin, Origin::ExtractedOther);
        assert!(
            scene
                .service
                .recently_learned(&scene.user_ctx(), None, None)
                .unwrap()
                .is_empty(),
            "recently learned lists the user's own words only"
        );
    }

    /// **Design §4 *Initial limits*.** Five notes per turn: the sixth item
    /// the model returns is dropped and counted, never stored.
    #[tokio::test]
    async fn the_sixth_item_of_a_turn_is_dropped_and_counted() {
        let items: Vec<String> = (1..=6)
            .map(|i| {
                format!(
                    r#"{{"title": "Preference {i}", "content": "The user prefers option {i}.",
                        "kind": "preference", "stated_by": "user", "confidence": 0.9}}"#
                )
            })
            .collect();
        let reply = format!(r#"{{"extractions": [{}]}}"#, items.join(","));
        let scene = Scene::new(&reply);
        let deps = scene.deps();
        assert_eq!(
            deps.daemon_config
                .load()
                .orchestrator
                .costs
                .extract_max_items_per_turn,
            5
        );

        let outcome = extract_turn_memories(
            &deps,
            scene.turn(OWNER, "Here are the six things I prefer, in order."),
        )
        .await;
        assert_eq!(
            outcome,
            ExtractionOutcome::Extracted(ExtractionReport {
                stored: 5,
                active: 5,
                dropped_over_cap: 1,
                ..Default::default()
            })
        );
        let titles: Vec<String> = scene.active().into_iter().map(|i| i.title).collect();
        assert_eq!(titles.len(), 5, "{titles:?}");
        assert!(!titles.contains(&"Preference 6".to_string()), "{titles:?}");
    }

    /// The acceptance floor: an item under `memory_confidence_threshold` is
    /// dropped and counted, and the number itself is never stored.
    #[tokio::test]
    async fn an_item_under_the_confidence_floor_is_dropped_and_counted() {
        let scene = Scene::new(
            r#"{"extractions": [
                {"title": "Maybe tea", "content": "The user might like tea.",
                 "kind": "preference", "stated_by": "user", "confidence": 0.3}
            ]}"#,
        );
        let outcome = extract_turn_memories(
            &scene.deps(),
            scene.turn(OWNER, "I had a cup of something warm this morning."),
        )
        .await;
        assert_eq!(
            outcome,
            ExtractionOutcome::Extracted(ExtractionReport {
                dropped_below_threshold: 1,
                ..Default::default()
            })
        );
        assert!(scene.active().is_empty());
        assert!(scene.candidates().is_empty());
    }

    /// Design §3: the owner is a `global_user` row. A `system` turn has no
    /// partition, so nothing is extracted and no model call is spent.
    #[tokio::test]
    async fn a_turn_with_no_owner_extracts_nothing_and_calls_no_model() {
        let scene = Scene::new(USER_ITEM);
        let outcome = extract_turn_memories(
            &scene.deps(),
            scene.turn("system", "I prefer vim for editing, always have."),
        )
        .await;
        assert_eq!(outcome, ExtractionOutcome::Skipped("no_owner"));
        assert!(scene.provider.first_request_opt().is_none());
        assert!(scene.active().is_empty());
    }

    /// A cancelled extraction writes nothing — the shutdown token is
    /// observed before the model is asked.
    #[tokio::test]
    async fn a_cancelled_extraction_writes_nothing() {
        let scene = Scene::new(USER_ITEM);
        scene.cancel.cancel();
        let outcome = extract_turn_memories(
            &scene.deps(),
            scene.turn(OWNER, "I prefer vim for editing, always have."),
        )
        .await;
        assert_eq!(outcome, ExtractionOutcome::Cancelled);
        assert!(scene.provider.first_request_opt().is_none());
        assert!(scene.active().is_empty());
    }

    /// The typed message's own row is the note's source (design §4:
    /// `sources` cite DB ids, never session-log offsets).
    #[tokio::test]
    async fn the_note_cites_the_typed_messages_row_when_the_turn_was_persisted() {
        let scene = Scene::new(USER_ITEM);
        let message = "I prefer vim for editing, always have.";
        let row: i64 = scene
            .db
            .with_connection(|c| {
                c.execute(
                    "INSERT INTO conversation_messages (lane_key, role, content) VALUES (?1, 'user', ?2)",
                    (format!("{OWNER}:cli"), message),
                )?;
                Ok(c.last_insert_rowid())
            })
            .unwrap();

        extract_turn_memories(&scene.deps(), scene.turn(OWNER, message)).await;

        let active = scene.active();
        assert_eq!(active.len(), 1);
        let note = scene
            .service
            .read(
                &scene.user_ctx(),
                ReadInput {
                    id: active[0].id,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(note.sources.len(), 1, "{:?}", note.sources);
        assert_eq!(note.sources[0].kind, SourceType::Message);
        assert_eq!(note.sources[0].id, row.to_string());
    }

    /// The exact-dedup rule (§7b P8): the same item twice is one note, and
    /// the second is counted as deduplicated.
    #[tokio::test]
    async fn the_same_item_twice_is_one_note() {
        let scene = Scene::new(USER_ITEM);
        let deps = scene.deps();
        extract_turn_memories(
            &deps,
            scene.turn(OWNER, "I prefer vim for editing, always have."),
        )
        .await;
        let outcome = extract_turn_memories(
            &deps,
            scene.turn(OWNER, "I prefer vim for editing, always have."),
        )
        .await;
        assert_eq!(
            outcome,
            ExtractionOutcome::Extracted(ExtractionReport {
                deduplicated: 1,
                ..Default::default()
            })
        );
        assert_eq!(scene.active().len(), 1);
    }

    /// **M2.** The extractor asks for no reasoning: on a thinking model the
    /// whole small budget went into reasoning and the answer came back
    /// empty — reported, wrongly, as "malformed JSON".
    #[tokio::test]
    async fn the_extractor_asks_for_no_reasoning() {
        let scene = Scene::new(r#"{"extractions": []}"#);
        extract_turn_memories(
            &scene.deps(),
            scene.turn(
                OWNER,
                "I moved to Tokyo last week and I prefer concise answers.",
            ),
        )
        .await;
        let request = scene.provider.first_request();
        assert!(
            matches!(request.thinking, Some(ThinkingConfig::Disabled)),
            "the extractor must ask for no reasoning, got {:?}",
            request.thinking
        );
        assert_eq!(request.max_tokens, Some(512), "the small budget is intact");
    }

    /// **M2, second half.** When the budget really is spent before an answer
    /// is written, the router says so (`LlmError::EmptyCompletion`) and the
    /// extractor reports *that* rather than filing "malformed JSON" against
    /// the model.
    #[tokio::test]
    async fn an_answer_the_reasoning_ate_is_not_filed_as_malformed_json() {
        let scene = Scene::new("");
        let deps = TurnExtractionDeps {
            router: router_recording(RecordingProvider::spent_budget()),
            ..scene.deps()
        };
        let outcome = extract_turn_memories(
            &deps,
            scene.turn(
                OWNER,
                "I moved to Tokyo last week and I prefer concise answers.",
            ),
        )
        .await;
        assert_eq!(outcome, ExtractionOutcome::Unavailable);
        let logs = LlmUsageRepository::new(&scene.db)
            .get_agent_usage(AGENT_LABEL, 10)
            .expect("read the call log");
        assert!(
            logs.iter().all(|l| l
                .error_message
                .as_deref()
                .is_none_or(|m| !m.contains("JSON parse"))),
            "an empty completion must not be filed as malformed JSON: {:?}",
            logs.iter()
                .map(|l| l.error_message.clone())
                .collect::<Vec<_>>()
        );
    }
}
