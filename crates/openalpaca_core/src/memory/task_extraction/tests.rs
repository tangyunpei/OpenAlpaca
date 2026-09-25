use super::*;

#[test]
fn test_parse_json_response_raw() {
    let input = r#"{"extractions": [{"content": "test", "kind": "fact"}]}"#;
    let result = parse_json_response(input);
    assert!(result.is_some());
    let arr = result.unwrap()["extractions"].as_array().unwrap().len();
    assert_eq!(arr, 1);
}

#[test]
fn test_parse_json_response_fenced() {
    let input = "```json\n{\"extractions\": []}\n```";
    let result = parse_json_response(input);
    assert!(result.is_some());
}

#[test]
fn test_parse_json_response_plain_fence() {
    let input = "```\n{\"extractions\": [{\"content\": \"hello\"}]}\n```";
    let result = parse_json_response(input);
    assert!(result.is_some());
}

#[test]
fn test_parse_json_response_invalid() {
    let input = "This is not JSON at all";
    assert!(parse_json_response(input).is_none());
}

#[test]
fn test_parse_json_response_with_whitespace() {
    let input = "  \n  {\"extractions\": []}  \n  ";
    let result = parse_json_response(input);
    assert!(result.is_some());
}

mod through_the_service {
    //! **MD-2, plan 3.4.** A finished run's output becomes `extracted_other`
    //! candidates through the service; a failed run, a `system` owner and an
    //! assistant-only claim produce nothing; the eleventh item is dropped
    //! and counted.

    use super::super::*;
    use crate::memory::service::{ListInput, ReadInput};
    use crate::test_util::{HomeStoreGuard, RecordingProvider, router_recording};
    use openalpaca_llm::ThinkingConfig;
    use openalpaca_storage::IdentityRepository;
    use openalpaca_storage::memory_files::{NoteStatus, Origin};
    use openalpaca_storage::memory_index::MemoryIndex;

    const OWNER: &str = "user1";
    const TASK: &str = "task-1";

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
            // The run the notes will cite: `verify_sources` checks the row.
            db.with_connection(|c| {
                c.execute(
                    "INSERT INTO task (id, title, created_by, source_lane) VALUES (?1, 'a run', ?2, 'user1:cli')",
                    (TASK, OWNER),
                )?;
                Ok(())
            })
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

        fn deps(&self) -> TaskExtractionDeps {
            TaskExtractionDeps {
                db: self.db.clone(),
                router: router_recording(self.provider.clone()),
                daemon_config: Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
                memory_service: self.service.clone(),
                cancel: self.cancel.clone(),
            }
        }

        fn params(&self, owner: &str, success: bool) -> TaskExtractionParams {
            TaskExtractionParams {
                owner_id: owner.to_string(),
                task_id: TASK.to_string(),
                task_description: "survey the store layout".to_string(),
                task_output:
                    "The store keeps its database under state/ and its notes under memory/. \
                              Every content directory is created with mode 0700."
                        .to_string(),
                source_path: "lead_agent".to_string(),
                project_root: None,
                success,
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

    const EVIDENCE_ITEM: &str = r#"{"extractions": [
        {"title": "Store layout", "content": "The database lives under state/ and notes under memory/.",
         "kind": "fact", "basis": "evidence", "confidence": 0.9}
    ]}"#;

    /// A run's finding becomes a candidate — `extracted_other`, outside the
    /// active list, citing the run — never an active note.
    #[tokio::test]
    async fn a_finished_runs_finding_becomes_a_candidate_citing_the_run() {
        let scene = Scene::new(EVIDENCE_ITEM);
        let outcome = extract_task_memories(&scene.deps(), scene.params(OWNER, true)).await;
        assert_eq!(
            outcome,
            TaskExtractionOutcome::Extracted(TaskExtractionReport {
                stored: 1,
                ..Default::default()
            })
        );
        assert!(
            scene.active().is_empty(),
            "tool output is never an active note"
        );
        let candidates = scene.candidates();
        assert_eq!(candidates.len(), 1, "{candidates:?}");
        assert_eq!(candidates[0].title, "Store layout");
        assert_eq!(candidates[0].status, NoteStatus::Candidate);
        assert_eq!(candidates[0].origin, Origin::ExtractedOther);

        let note = scene
            .service
            .read(
                &scene.user_ctx(),
                ReadInput {
                    id: candidates[0].id,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(note.sources.len(), 1, "{:?}", note.sources);
        assert_eq!(note.sources[0].kind, SourceType::Task);
        assert_eq!(note.sources[0].id, TASK);
    }

    /// A failed or cancelled run teaches nothing: no note, no model call.
    #[tokio::test]
    async fn a_failed_run_produces_nothing() {
        let scene = Scene::new(EVIDENCE_ITEM);
        let outcome = extract_task_memories(&scene.deps(), scene.params(OWNER, false)).await;
        assert_eq!(outcome, TaskExtractionOutcome::Skipped("not_successful"));
        assert!(scene.provider.first_request_opt().is_none());
        assert!(scene.candidates().is_empty());
    }

    /// An assistant-only completion claim — the output asserting that
    /// something was done, with nothing to show it — never becomes a fact.
    #[tokio::test]
    async fn an_assistant_only_claim_produces_nothing() {
        let scene = Scene::new(
            r#"{"extractions": [
                {"title": "Tests pass", "content": "All the project's tests pass now.",
                 "kind": "fact", "basis": "claim", "confidence": 0.95},
                {"title": "Refactor done", "content": "The module was refactored as requested.",
                 "kind": "episode", "confidence": 0.9}
            ]}"#,
        );
        let outcome = extract_task_memories(&scene.deps(), scene.params(OWNER, true)).await;
        assert_eq!(
            outcome,
            TaskExtractionOutcome::Extracted(TaskExtractionReport {
                dropped_claims: 2,
                ..Default::default()
            })
        );
        assert!(scene.candidates().is_empty());
        assert!(scene.active().is_empty());
    }

    /// **Design §4 *Initial limits*.** Ten candidates per run: the eleventh
    /// item is dropped and counted.
    #[tokio::test]
    async fn the_eleventh_item_of_a_run_is_dropped_and_counted() {
        let items: Vec<String> = (1..=11)
            .map(|i| {
                format!(
                    r#"{{"title": "Finding {i}", "content": "The project's component {i} uses PostgreSQL.",
                        "kind": "fact", "basis": "evidence", "confidence": 0.9}}"#
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
                .task_extract_max_items_per_run,
            10
        );

        let outcome = extract_task_memories(&deps, scene.params(OWNER, true)).await;
        assert_eq!(
            outcome,
            TaskExtractionOutcome::Extracted(TaskExtractionReport {
                stored: 10,
                dropped_over_cap: 1,
                ..Default::default()
            })
        );
        let titles: Vec<String> = scene.candidates().into_iter().map(|i| i.title).collect();
        assert_eq!(titles.len(), 10, "{titles:?}");
        assert!(!titles.contains(&"Finding 11".to_string()), "{titles:?}");
    }

    /// Design §3: a `system` run — `/v1/command process` and every workflow
    /// it starts — has no partition; nothing is written and no model call is
    /// spent.
    #[tokio::test]
    async fn a_system_run_produces_nothing_and_calls_no_model() {
        let scene = Scene::new(EVIDENCE_ITEM);
        let outcome = extract_task_memories(&scene.deps(), scene.params("system", true)).await;
        assert_eq!(outcome, TaskExtractionOutcome::Skipped("no_owner"));
        assert!(scene.provider.first_request_opt().is_none());
        assert!(scene.candidates().is_empty());
    }

    /// The shutdown token is observed before the model is asked.
    #[tokio::test]
    async fn a_cancelled_extraction_writes_nothing() {
        let scene = Scene::new(EVIDENCE_ITEM);
        scene.cancel.cancel();
        let outcome = extract_task_memories(&scene.deps(), scene.params(OWNER, true)).await;
        assert_eq!(outcome, TaskExtractionOutcome::Cancelled);
        assert!(scene.provider.first_request_opt().is_none());
        assert!(scene.candidates().is_empty());
    }

    /// `task_extract_enabled = false` is the kill switch: no call, nothing
    /// written.
    #[tokio::test]
    async fn the_kill_switch_stops_the_extraction_before_any_call() {
        let scene = Scene::new(EVIDENCE_ITEM);
        let mut cfg = DaemonConfig::default();
        cfg.orchestrator.costs.task_extract_enabled = false;
        let deps = TaskExtractionDeps {
            daemon_config: Arc::new(ArcSwap::from_pointee(cfg)),
            ..scene.deps()
        };
        let outcome = extract_task_memories(&deps, scene.params(OWNER, true)).await;
        assert_eq!(outcome, TaskExtractionOutcome::Skipped("disabled"));
        assert!(scene.provider.first_request_opt().is_none());
    }

    /// **M2.** Task-output knowledge extraction is an internal utility call
    /// on a 512-token budget, and asks for no reasoning.
    #[tokio::test]
    async fn the_task_extractor_asks_for_no_reasoning() {
        let scene = Scene::new(r#"{"extractions": []}"#);
        extract_task_memories(&scene.deps(), scene.params(OWNER, true)).await;
        let request = scene.provider.first_request();
        assert!(
            matches!(request.thinking, Some(ThinkingConfig::Disabled)),
            "the task extractor must ask for no reasoning, got {:?}",
            request.thinking
        );
        assert_eq!(request.max_tokens, Some(512));
        // Attributed to the run: the usage row names it.
        let logs = LlmUsageRepository::new(&scene.db)
            .get_agent_usage(AGENT_LABEL, 10)
            .expect("read the call log");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].task_id.as_deref(), Some(TASK));
    }
    /// **The daily-cap defect (design 09 §1.8, ruling LR8).** The gate reads
    /// the durable `llm_usage_daily` for today's UTC date: yesterday's spend
    /// binds nothing, today's does, and the in-memory cost tracker — seeded
    /// once at boot and never reset — is not consulted at all.
    #[tokio::test]
    async fn the_daily_cost_cap_reads_the_utc_day_from_llm_usage_daily() {
        let daily = |date: String, cost: f64| openalpaca_storage::LlmUsageDaily {
            date,
            agent_id: AGENT_LABEL.to_string(),
            model: "m".to_string(),
            total_requests: 3,
            total_input_tokens: 10,
            total_output_tokens: 10,
            total_cost_usd: cost,
        };
        let today = chrono::Utc::now().date_naive();
        let yesterday = today - chrono::Duration::days(1);

        // Yesterday's spend, far over the 0.50 cap, and a tracker that still
        // remembers it: today is a new day, and the run is extracted.
        {
            let scene = Scene::new(EVIDENCE_ITEM);
            LlmUsageRepository::new(&scene.db)
                .upsert_daily_usage(&daily(yesterday.format("%Y-%m-%d").to_string(), 9.0))
                .unwrap();
            let deps = scene.deps();
            deps.router
                .cost_tracker
                .record(&openalpaca_llm::CallRecord {
                    agent_id: AGENT_LABEL.to_string(),
                    task_id: None,
                    model: "m".to_string(),
                    input_tokens: 1,
                    output_tokens: 1,
                    cost_usd: 9.0,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                })
                .await;
            let outcome = extract_task_memories(&deps, scene.params(OWNER, true)).await;
            assert!(
                matches!(outcome, TaskExtractionOutcome::Extracted(_)),
                "{outcome:?}"
            );
        } // One scene at a time: each holds the home-store lock.

        // Today's spend over the cap binds, with no call.
        let scene = Scene::new(EVIDENCE_ITEM);
        LlmUsageRepository::new(&scene.db)
            .upsert_daily_usage(&daily(today.format("%Y-%m-%d").to_string(), 0.75))
            .unwrap();
        let outcome = extract_task_memories(&scene.deps(), scene.params(OWNER, true)).await;
        assert_eq!(outcome, TaskExtractionOutcome::Skipped("cost_cap"));
        assert!(scene.provider.first_request_opt().is_none());
    }
}
