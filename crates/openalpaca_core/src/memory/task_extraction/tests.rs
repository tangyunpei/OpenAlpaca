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

/// LQ13 (the L8 product slice's P3): the one JSON value an answer carries is
/// read out of a `<think>` block or prose around it; of several, the last.
/// A truncated value is never read, and prose alone is still no answer.
#[test]
fn test_parse_json_response_unwraps_thinking_and_prose() {
    let answer = r#"{"extractions": [{"content": "test"}]}"#;
    for input in [
        format!("<think>\nDraft: {{\"extractions\": []}}\n</think>\n{answer}"),
        format!("{answer}\n</think>"),
        format!("Here is what the run produced:\n{answer}\nDone."),
    ] {
        let v = parse_json_response(&input).unwrap_or_else(|| panic!("{input}"));
        assert_eq!(v["extractions"].as_array().unwrap().len(), 1, "{input}");
    }
    assert!(parse_json_response(r#"Here: {"extractions": [{"content": "te"#).is_none());
    assert!(parse_json_response("Nothing worth keeping came out of this run.").is_none());
}

/// LQ13's reading (repair round 1 of the L8 product slice): the text after
/// the last `</think>` is the answer; one `</think>` that ends the answer
/// leaves the text before it, read whole; an opening `<think>` never closed
/// is no answer; and a value found in prose is taken only with the shape
/// the caller expects — here `{"extractions": [...]}` whose every entry
/// carries `content` — so a quoted fragment or a draft the reasoning holds
/// is not the answer.
#[test]
fn a_value_in_prose_or_reasoning_is_taken_only_with_the_expected_shape() {
    let answer = r#"{"extractions": [{"content": "test"}]}"#;
    let read = |input: &str| read_json_answer(input, is_extractions_answer);
    for input in [
        format!("Maybe {{\"extractions\": [{{\"content\": \"draft\"}}]}}\n</think>\n{answer}"),
        format!(
            "Quoted: {{\"extractions\": [{{\"title\": \"no content\"}}]}}. Then {answer}. And {{\"n\": 1}}."
        ),
    ] {
        let found = read(&input).unwrap_or_else(|| panic!("{input}"));
        assert_eq!(found.value["extractions"][0]["content"], "test", "{input}");
    }
    for input in [
        format!("<think>draft {answer} hmm"),
        format!("Draft {answer}\n</think>\nNothing to keep."),
        format!("Draft {answer} hmm, no.\n</think>"),
    ] {
        assert!(read(&input).is_none(), "{input}");
    }
    // A quoted fragment of the wrong shape, alone in prose, is found — its
    // caller refuses it, as it would refuse it whole.
    let quoted = read(r#"The output said {"extractions": [{"title": "x"}]}."#).unwrap();
    assert!(!quoted.whole && !is_extractions_answer(&quoted.value));
    // Whole, it is read as before LQ13.
    let whole = read(r#"{"extractions": [{"title": "x"}]}"#).unwrap();
    assert!(whole.whole);
}

/// LQ13's scan, pinned where it is a trade-off: a bracket that never closes
/// is skipped by itself — a stray `[` in prose does not hide the answer
/// after it, and a value nested in a malformed wrapper that never closes is
/// then found as prose's next value. A bracket that closes but is not JSON
/// is skipped whole, so nothing inside it is taken.
#[test]
fn an_unclosed_bracket_is_skipped_by_itself_and_a_closed_one_whole() {
    let answer = r#"{"extractions": [{"content": "test"}]}"#;
    for input in [
        format!("See [n1 and then {answer}"),
        format!(r#"Draft: {{"answer": oops, "inner": {answer}"#),
    ] {
        let found =
            read_json_answer(&input, is_extractions_answer).unwrap_or_else(|| panic!("{input}"));
        assert_eq!(found.value["extractions"][0]["content"], "test", "{input}");
    }
    assert!(
        read_json_answer(
            &format!(r#"Draft: {{"answer": oops, "inner": {answer}}} and no more."#),
            is_extractions_answer
        )
        .is_none()
    );
}

/// A run's answer whose own JSON quotes the tags is read (repair round 2 of
/// the L8 product slice): a `<think>` or `</think>` inside a JSON value is
/// text, not a marker, so a fenced answer — the form read before LQ13 —
/// still reads, as does one after the model's reasoning or a prose lead.
#[test]
fn a_run_answer_that_quotes_the_think_tags_is_read() {
    let answer = r#"{"extractions": [{"content": "The build strips <think> and </think> tags."}]}"#;
    for input in [
        format!("```json\n{answer}\n```"),
        format!("The run produced:\n```json\n{answer}\n```"),
        format!("<think>ok</think>\n```json\n{answer}\n```"),
        format!("The run produced: {answer}"),
    ] {
        let found = run_answer(&input, false).unwrap_or_else(|| panic!("{input}"));
        assert_eq!(
            found["extractions"][0]["content"], "The build strips <think> and </think> tags.",
            "{input}"
        );
    }
    assert!(run_answer(&format!("<think>Draft: {answer}"), false).is_none());
}

/// The run's answer as `extract_task_memories` reads it (repair round 2 of
/// the L8 product slice): a value found in prose is the answer only with
/// the `{"extractions": [...]}` shape, every entry carrying `content` — a
/// quoted fragment alone in the prose is refused, as it would be whole; an
/// answer cut at its ceiling is held to the forms read before LQ13.
#[test]
fn a_quoted_fragment_alone_in_prose_is_not_the_runs_answer() {
    let answer = r#"{"extractions": [{"content": "test"}]}"#;
    assert!(
        run_answer(
            r#"The output said {"extractions": [{"title": "x"}]}. Done."#,
            false
        )
        .is_none()
    );
    // Whole, the same fragment is the answer, as before LQ13; its item is
    // then dropped as invalid.
    assert!(run_answer(r#"{"extractions": [{"title": "x"}]}"#, false).is_some());
    assert!(run_answer(&format!("The run produced: {answer}"), false).is_some());
    assert!(run_answer(&format!("The run produced: {answer}"), true).is_none());
    assert!(run_answer(&format!("```json\n{answer}\n```"), true).is_some());
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
            Self::with(RecordingProvider::new(reply))
        }

        fn with(provider: Arc<RecordingProvider>) -> Self {
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
                provider,
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

    /// ST-21: a run whose project's data was deleted since it ran — its root
    /// no longer a registered project — is not extracted, and no model call
    /// is spent finding that out.
    #[tokio::test]
    async fn a_run_whose_projects_data_was_deleted_makes_no_call() {
        let scene = Scene::new(EVIDENCE_ITEM);
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let registered = openalpaca_storage::store::projects::register(&root).unwrap();
        std::fs::remove_dir_all(&registered.dir).unwrap();
        let mut params = scene.params(OWNER, true);
        params.project_root = Some(root.clone());

        let outcome = extract_task_memories(&scene.deps(), params).await;
        assert_eq!(outcome, TaskExtractionOutcome::Skipped("root_gone"));
        assert!(scene.provider.first_request_opt().is_none());
        assert!(!registered.dir.exists());
    }

    /// Residual X5: the project check waits for `projects::lock()` — held by
    /// a re-point or a delete for as long as its transaction or its folder
    /// removal takes — on the blocking pool, never on a runtime worker. On a
    /// current-thread runtime a check made in place would stop the test's
    /// own task from running until the holder let go by itself.
    #[tokio::test(flavor = "current_thread")]
    async fn the_project_check_waits_for_the_projects_lock_off_the_runtime() {
        use openalpaca_storage::store::projects;
        use std::time::Duration;
        let scene = Scene::new(EVIDENCE_ITEM);
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let registered = projects::register(&root).unwrap();
        std::fs::remove_dir_all(&registered.dir).unwrap();
        let mut params = scene.params(OWNER, true);
        params.project_root = Some(root.clone());

        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let holder = std::thread::spawn(move || {
            let _held = projects::lock();
            held_tx.send(()).unwrap();
            // Let go when the runtime's task says so — or, with the runtime
            // stuck on the lock, once this gives up waiting.
            release_rx.recv_timeout(Duration::from_secs(5)).is_ok()
        });
        held_rx.recv().unwrap();
        let deps = scene.deps();
        let extraction = tokio::spawn(async move { extract_task_memories(&deps, params).await });

        // The extraction runs until it waits for the lock.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = release_tx.send(());
        let released_by_this_task = holder.join().unwrap();

        assert!(
            released_by_this_task,
            "the extraction held the runtime's only thread while it waited for projects::lock()"
        );
        assert_eq!(
            extraction.await.unwrap(),
            TaskExtractionOutcome::Skipped("root_gone")
        );
        assert!(scene.provider.first_request_opt().is_none());
    }

    /// ST-21: a missing project's run — its folder deleted, its record kept —
    /// is still extracted into the project's notes at home.
    #[tokio::test]
    async fn a_missing_projects_run_is_still_extracted() {
        let scene = Scene::new(EVIDENCE_ITEM);
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let registered = openalpaca_storage::store::projects::register(&root).unwrap();
        drop(project);
        let mut params = scene.params(OWNER, true);
        params.project_root = Some(root.clone());

        let outcome = extract_task_memories(&scene.deps(), params).await;
        assert_eq!(
            outcome,
            TaskExtractionOutcome::Extracted(TaskExtractionReport {
                stored: 1,
                ..Default::default()
            })
        );
        assert!(!root.exists(), "the user's folder is not re-created");
        assert!(registered.dir.join("memory").is_dir());
    }

    /// The S4 review's I1: a run whose project is mid re-point — its marker
    /// still in `project.json`, so no folder records the run's root — is no
    /// deleted project, but it has no partition to learn into either: it is
    /// skipped as held, not as gone, and no model call is spent. (A run has
    /// no cursor to hold, so its extraction is not tried again.)
    #[tokio::test]
    async fn a_run_whose_project_is_mid_repoint_makes_no_call_and_is_not_gone() {
        use openalpaca_storage::project_life::{RepointStep, repoint_with};
        let scene = Scene::new(EVIDENCE_ITEM);
        let old_dir = tempfile::tempdir().unwrap();
        let old = old_dir.path().canonicalize().unwrap();
        let new_dir = tempfile::tempdir().unwrap();
        let new = new_dir.path().canonicalize().unwrap();
        openalpaca_storage::store::projects::register(&old).unwrap();
        repoint_with(&scene.db, &old, &new, &mut |at| at != RepointStep::Marked).unwrap_err();
        let mut params = scene.params(OWNER, true);
        params.project_root = Some(old.clone());

        let outcome = extract_task_memories(&scene.deps(), params).await;
        assert_eq!(outcome, TaskExtractionOutcome::Skipped("root_held"));
        assert!(scene.provider.first_request_opt().is_none());
    }

    /// LQ13 keeps the L8 fix slice's P1 for a run (repair round 1 of the L8
    /// product slice): an answer stopped at its output ceiling is held to
    /// the forms read before LQ13, so the `extractions` object in the prose
    /// of a cut answer is not read — it is a JSON parse failure and nothing
    /// is stored. The same answer ended on its own is read.
    #[tokio::test]
    async fn a_cut_answer_is_not_read_out_of_its_prose() {
        let answer = format!("The run found one thing:\n{EVIDENCE_ITEM}\nAnd then the");
        let cut = Scene::with(RecordingProvider::cut(answer.clone()));
        let outcome = extract_task_memories(&cut.deps(), cut.params(OWNER, true)).await;
        assert_eq!(
            outcome,
            TaskExtractionOutcome::Extracted(TaskExtractionReport::default())
        );
        assert!(cut.candidates().is_empty());
        // One scene at a time: each holds the home store's guard.
        drop(cut);

        let ended = Scene::new(&answer);
        let outcome = extract_task_memories(&ended.deps(), ended.params(OWNER, true)).await;
        assert_eq!(
            outcome,
            TaskExtractionOutcome::Extracted(TaskExtractionReport {
                stored: 1,
                ..Default::default()
            })
        );
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

    /// **Design 09 §2.7.** Task-output extraction shares the chat
    /// extractor's item fields: the prompt lists the keys in use, and the
    /// aliases and the `fact_key` an item carries pass the same cleaning
    /// (rules 7–8) onto the candidate — which stays a candidate, replacing
    /// nothing.
    #[tokio::test]
    async fn task_candidates_carry_aliases_and_fact_key() {
        let scene = Scene::new(
            r#"{"extractions": [
                {"title": "Store layout", "content": "The database lives under state/ and notes under memory/.",
                 "kind": "fact", "basis": "evidence", "confidence": 0.9,
                 "aliases": ["state directory", "状态目录", "store layout", "State Directory"],
                 "fact_key": "OpenAlpaca.Store.Layout"}
            ]}"#,
        );
        scene
            .service
            .store(
                &scene.user_ctx(),
                crate::memory::service::StoreInput {
                    title: "Content directories".into(),
                    body: "# Content directories\n\nEvery content directory is 0700.\n".into(),
                    kind: Some(NoteKind::Fact),
                    fact_key: Some("openalpaca.store.modes".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let outcome = extract_task_memories(&scene.deps(), scene.params(OWNER, true)).await;
        assert!(
            matches!(&outcome, TaskExtractionOutcome::Extracted(r) if r.stored == 1),
            "{outcome:?}"
        );
        let prompt = scene
            .provider
            .first_request()
            .messages
            .iter()
            .map(|m| m.content.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            prompt.contains("<known_keys>openalpaca.store.modes</known_keys>"),
            "{prompt}"
        );
        let candidates = scene.candidates();
        assert_eq!(candidates.len(), 1, "{candidates:?}");
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
        assert_eq!(
            note.frontmatter["aliases"],
            serde_json::json!(["state directory", "状态目录"])
        );
        assert_eq!(
            note.frontmatter["fact_key"],
            serde_json::json!("openalpaca.store.layout")
        );
        assert!(note.supersedes.is_empty());
        assert_eq!(note.status, NoteStatus::Candidate);
    }

    /// **M2.** Task-output knowledge extraction is an internal utility call
    /// on a 1,024-token budget (design 09 §2.7: aliases and a key per item
    /// did not fit 512), and asks for no reasoning.
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
        assert_eq!(request.max_tokens, Some(1024));
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
