//! Live-model steer-routing eval harness (Routing V2 follow-up).
//!
//! Replays the labeled mid-workflow corpus at
//! `scripts/eval/steer_routing/corpus.json` through a real `Orchestrator`
//! (real LLM router from `config/llm.toml`) with a fake active workflow
//! registered on each lane, and scores routing by observing what actually
//! happened — steering-inbox delivery (`steer_workflow` / `/steer `),
//! `lane_followups` writes (`queue_followup`), deterministic-tier mode
//! strings, or a plain inline answer.
//!
//! Default `cargo test` runs only the corpus-validation test; the live eval
//! is `#[ignore]`d AND gated on `OPENALPACA_LIVE_EVAL=1` plus a resolvable
//! `llm.toml`, so it can never spend money by accident.
//!
//! Run (see `scripts/eval/steer_routing/README.md` for the full runbook):
//!
//! ```sh
//! OPENALPACA_LIVE_EVAL=1 cargo test -p openalpaca_core --features live-eval \
//!     --test steer_routing_eval -- --ignored steer_routing_eval --nocapture
//! ```
//!
//! NOTE on scope: the original Routing V2 spec called for a "cost comparison
//! vs planner" leg. The planner ladder was deleted in Routing V2 Phase 5, so
//! there is nothing left to compare against — this harness instead reports an
//! absolute cost/latency baseline (per-turn ack_ms + token usage from the
//! `llm_usage` / `orchestrator_latency` tables).

use serde::Deserialize;
use std::path::PathBuf;

// ─── Corpus ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Expected {
    /// Correction/new guidance for the running workflow → `steer_workflow`
    /// (or the deterministic `/steer ` prefix for `deterministic` items).
    Steer,
    /// Unrelated message → answered inline, no workflow tool call.
    Answer,
    /// Work for after the workflow finishes → `queue_followup`.
    Queue,
    /// Control command that must NEVER reach the model (task ops / `/steer `).
    Deterministic,
}

impl Expected {
    fn as_str(self) -> &'static str {
        match self {
            Expected::Steer => "steer",
            Expected::Answer => "answer",
            Expected::Queue => "queue",
            Expected::Deterministic => "deterministic",
        }
    }
}

#[derive(Debug, Deserialize)]
struct CorpusItem {
    id: String,
    /// The mid-workflow user message to replay.
    message: String,
    /// Short description of the fake active workflow (used as its title).
    context: String,
    expected: Expected,
}

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/eval/steer_routing/corpus.json")
}

fn load_corpus() -> Vec<CorpusItem> {
    let path = corpus_path();
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()))
}

/// Always-on guard: the corpus must load and be structurally valid, so a bad
/// edit is caught by plain `cargo test -p openalpaca_core` without any keys.
#[test]
fn steer_routing_corpus_parses_and_validates() {
    let corpus = load_corpus();
    assert!(
        corpus.len() >= 20,
        "corpus should stay a meaningful size (got {})",
        corpus.len()
    );

    let mut seen = std::collections::HashSet::new();
    let mut label_counts = std::collections::HashMap::new();
    for item in &corpus {
        assert!(!item.id.trim().is_empty(), "empty id");
        assert!(seen.insert(item.id.clone()), "duplicate id: {}", item.id);
        assert!(!item.message.trim().is_empty(), "{}: empty message", item.id);
        assert!(!item.context.trim().is_empty(), "{}: empty context", item.id);
        *label_counts.entry(item.expected.as_str()).or_insert(0usize) += 1;

        // Deterministic items are slash commands (they must be answered by
        // the deterministic tier); model-routed items must NOT be.
        if item.expected == Expected::Deterministic {
            assert!(
                item.message.starts_with('/'),
                "{}: deterministic items must be slash commands",
                item.id
            );
        } else {
            assert!(
                !item.message.starts_with('/'),
                "{}: model-routed items must not be slash commands",
                item.id
            );
        }
    }

    for label in ["steer", "answer", "queue", "deterministic"] {
        assert!(
            label_counts.get(label).copied().unwrap_or(0) >= 3,
            "corpus needs >= 3 '{label}' items (got {label_counts:?})"
        );
    }
}

// ─── Live eval ──────────────────────────────────────────────────────────────

/// Resolve `llm.toml` the way the runbook documents it: explicit
/// `OPENALPACA_CONFIG_DIR` first, then the repo's `config/` directory.
fn resolve_llm_toml() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("OPENALPACA_CONFIG_DIR") {
        let p = PathBuf::from(dir).join("llm.toml");
        if p.exists() {
            return Some(p);
        }
    }
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/llm.toml");
    if p.exists() { Some(p) } else { None }
}

fn accuracy_bar() -> f64 {
    std::env::var("OPENALPACA_EVAL_ACCURACY_BAR")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.8)
}

struct EvalRow {
    id: String,
    expected: &'static str,
    actual: &'static str,
    mode: String,
    ack_ms: u64,
    llm_calls: u32,
    tokens_in: u64,
    tokens_out: u64,
    cost_usd: f64,
    tools: Vec<String>,
    reply_ok: bool,
}

/// Live steer-routing eval. `#[ignore]`d: even `cargo test -- --ignored`
/// exits early (without spending money) unless `OPENALPACA_LIVE_EVAL=1`.
///
/// Requires a resolvable `llm.toml` with working keys AND the `live-eval`
/// feature (which compiles the LLM providers into this test binary).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live LLM eval — set OPENALPACA_LIVE_EVAL=1, provide llm.toml, build with --features live-eval (runbook: scripts/eval/steer_routing/README.md)"]
async fn steer_routing_eval() {
    use arc_swap::ArcSwap;
    use openalpaca_core::bus::EventBus;
    use openalpaca_core::context::{SharedContext, TaskEntryStatus};
    use openalpaca_core::daemon_config::DaemonConfig;
    use openalpaca_core::events::SystemEvent;
    use openalpaca_core::lane::LaneManager;
    use openalpaca_core::middleware::prompt::SystemPersona;
    use openalpaca_core::orchestrator::{Orchestrator, skill_catalog, skill_router};
    use openalpaca_core::runner::{LoopConfig, SteeringInbox};
    use openalpaca_core::security::gate::SecurityGate;
    use openalpaca_core::security::policy::{Principal, Scope};
    use openalpaca_core::security::sandbox::SandboxManager;
    use openalpaca_core::tools::ToolRegistry;
    use openalpaca_storage::repository::LlmUsageRepository;
    use openalpaca_storage::repository::orchestrator_latency::OrchestratorLatencyRepository;
    use std::sync::Arc;
    use tokio::sync::broadcast::error::TryRecvError;
    use uuid::Uuid;

    // Double gate: never make live calls unless the operator opted in.
    if std::env::var("OPENALPACA_LIVE_EVAL").as_deref() != Ok("1") {
        eprintln!(
            "steer_routing_eval: skipped — set OPENALPACA_LIVE_EVAL=1 to run the live eval \
             (no LLM calls were made)."
        );
        return;
    }
    let llm_toml = resolve_llm_toml().unwrap_or_else(|| {
        panic!(
            "steer_routing_eval: no llm.toml found (checked $OPENALPACA_CONFIG_DIR and \
             <repo>/config/llm.toml). See scripts/eval/steer_routing/README.md."
        )
    });
    let router = Arc::new(openalpaca_llm::build_router(&llm_toml).unwrap_or_else(|e| {
        panic!(
            "steer_routing_eval: failed to build LLM router from {}: {e}\n\
             (providers are feature-gated — build with `--features live-eval`; \
             for secret_ref keys run the daemon's reverse migration first or use secret_env)",
            llm_toml.display()
        )
    }));
    eprintln!(
        "steer_routing_eval: router loaded from {} (default model: {})",
        llm_toml.display(),
        router.default_model()
    );

    let corpus = load_corpus();

    // ── Boot an Orchestrator with the real router + a scratch DB ────────
    let tmp = tempfile::tempdir().expect("tempdir");
    let db = openalpaca_storage::Database::open(&tmp.path().join("eval.db")).expect("open db");

    let shared = Arc::new(SharedContext::new());
    let lanes = Arc::new(LaneManager::new());
    let bus = EventBus::default();
    let registry = Arc::new(ToolRegistry::default());
    let sandbox = Arc::new(SandboxManager::with_defaults(registry.clone(), bus.clone()));
    let gate = Arc::new(SecurityGate::new(sandbox));

    let mut cfg = DaemonConfig::default();
    // Safety valve: each eval lane carries exactly one (fake) active
    // workflow, so with the cap at 1 a misrouted `start_workflow` is refused
    // deterministically at the per-lane limit instead of dispatching a real
    // (costly) lead-agent workflow. The refusal still shows up as a
    // `start_workflow` tool call, so misroutes remain observable.
    cfg.orchestrator.routing.max_workflows_per_lane = 1;

    let orch = Orchestrator::new(
        shared.clone(),
        lanes,
        bus.clone(),
        SystemPersona::default(),
        Some(router),
        LoopConfig::default(),
        gate,
        registry,
        Some(db.clone()),
        None, // no embedder — memory_store degrades gracefully, memory_search stays off
        Arc::new(skill_catalog::SkillCatalog::new()),
        Arc::new(skill_router::SkillRouter::new(0.65, 0.45)),
        Arc::new(ArcSwap::from_pointee(cfg)),
    );

    // ── Replay the corpus ───────────────────────────────────────────────
    let mut rows: Vec<EvalRow> = Vec::with_capacity(corpus.len());
    for (i, item) in corpus.iter().enumerate() {
        // Fresh lane + fresh fake workflow per item: no cross-item state.
        let lane_key = format!("evaluser{i}:cli");
        let task_id = format!("eval-wf-{i}");
        shared
            .task_registry
            .register(task_id.clone(), item.context.clone());
        shared
            .task_registry
            .update_status(&task_id, TaskEntryStatus::Running);
        shared.register_workflow_for_lane(&lane_key, &task_id);
        let inbox = Arc::new(SteeringInbox::new(16));
        shared.register_steering_inbox(&task_id, inbox.clone());

        let mut rx = bus.subscribe();
        let request_id = Uuid::new_v4();
        let reply = orch
            .handle_message(
                request_id,
                "cli".to_string(),
                item.message.clone(),
                // System principal: passes the trust gate and keeps the
                // background user-trait extraction (a real LLM call) off.
                Principal::System,
                Scope::Global,
                lane_key.clone(),
                None,
                None,
            )
            .await;

        // Score by observing what actually happened during the turn.
        let mut mode = String::from("?");
        let mut ack_ms = 0u64;
        let mut steered = false;
        let mut queued = false;
        let mut started = false;
        let mut llm_calls = 0u32;
        let mut tokens_in = 0u64;
        let mut tokens_out = 0u64;
        let mut cost_usd = 0f64;
        let mut tools: Vec<String> = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(SystemEvent::OrchestrationStage {
                    request_id: rid,
                    mode: m,
                    ack_ms: a,
                    ..
                }) if rid == request_id => {
                    mode = m;
                    ack_ms = a;
                }
                Ok(SystemEvent::WorkflowSteered { task_id: t, .. }) if t == task_id => {
                    steered = true;
                }
                Ok(SystemEvent::FollowupQueued { lane_key: l, .. }) if l == lane_key => {
                    queued = true;
                }
                Ok(SystemEvent::WorkflowStarted { .. }) => started = true,
                Ok(SystemEvent::ToolExecuted { tool_name, .. }) => tools.push(tool_name),
                Ok(SystemEvent::LlmCallCompleted {
                    agent_id,
                    input_tokens,
                    output_tokens,
                    cost_usd: c,
                    ..
                }) if agent_id == "orchestrator" => {
                    llm_calls += 1;
                    tokens_in += input_tokens as u64;
                    tokens_out += output_tokens as u64;
                    cost_usd += c;
                }
                Ok(_) => {}
                Err(TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
        let inbox_got_message = !inbox.drain_all().is_empty();

        // Precedence: mode first (the deterministic `/steer ` path also
        // pushes into the inbox), then inbox delivery, then followup rows.
        let actual: &'static str = if mode != "main_loop" {
            "deterministic"
        } else if steered || inbox_got_message {
            "steer"
        } else if queued {
            "queue"
        } else if started || tools.iter().any(|t| t == "start_workflow") {
            // A start_workflow attempt mid-workflow is always a misroute for
            // this corpus (the cap refuses it, but the call itself scores).
            "start_workflow"
        } else {
            "answer"
        };

        rows.push(EvalRow {
            id: item.id.clone(),
            expected: item.expected.as_str(),
            actual,
            mode,
            ack_ms,
            llm_calls,
            tokens_in,
            tokens_out,
            cost_usd,
            tools,
            reply_ok: reply.is_ok(),
        });

        // Tidy up the fake workflow before the next item.
        inbox.close_and_drain();
        shared.deregister_workflow_for_lane(&lane_key, &task_id);
    }

    // ── Report ──────────────────────────────────────────────────────────
    println!("\n== steer_routing_eval: per-item verdicts ==");
    let mut correct = 0usize;
    for row in &rows {
        let ok = row.actual == row.expected;
        if ok {
            correct += 1;
        }
        println!(
            "[{}] {:<10} expected={:<13} actual={:<14} mode={:<16} ack_ms={:<6} \
             llm_calls={} tokens={}/{} cost=${:.4} tools={:?}{}",
            if ok { " OK " } else { "MISS" },
            row.id,
            row.expected,
            row.actual,
            row.mode,
            row.ack_ms,
            row.llm_calls,
            row.tokens_in,
            row.tokens_out,
            row.cost_usd,
            row.tools,
            if row.reply_ok { "" } else { " (turn returned Err)" },
        );
    }
    let accuracy = correct as f64 / rows.len() as f64;
    let bar = accuracy_bar();

    // Absolute cost/latency baseline from the persisted tables (the "cost
    // comparison vs planner" leg of the old spec is obsolete — the planner
    // was deleted in Routing V2 Phase 5).
    let usage = LlmUsageRepository::new(&db)
        .get_all_usage(10_000)
        .expect("read llm_usage");
    let total_in: i64 = usage.iter().map(|u| u.input_tokens as i64).sum();
    let total_out: i64 = usage.iter().map(|u| u.output_tokens as i64).sum();
    let total_cost: f64 = usage.iter().map(|u| u.cost_usd).sum();
    let mean_latency: f64 = if usage.is_empty() {
        0.0
    } else {
        usage.iter().filter_map(|u| u.latency_ms).sum::<i64>() as f64 / usage.len() as f64
    };
    println!("\n== cost/latency baseline (llm_usage table) ==");
    println!(
        "{} LLM calls, {} in / {} out tokens, ${:.4} total, mean loop latency {:.0} ms",
        usage.len(),
        total_in,
        total_out,
        total_cost,
        mean_latency
    );
    println!("\n== ack latency by mode (orchestrator_latency table) ==");
    for agg in OrchestratorLatencyRepository::new(&db)
        .aggregate_by_mode(None, None)
        .expect("read orchestrator_latency")
    {
        println!(
            "mode={:<18} count={:<3} mean_ack={:.0}ms p50={}ms p95={}ms",
            agg.mode, agg.count, agg.mean_ack_ms, agg.p50_total_ms, agg.p95_total_ms
        );
    }
    println!(
        "\naccuracy: {}/{} = {:.1}% (bar {:.1}% — override via OPENALPACA_EVAL_ACCURACY_BAR)",
        correct,
        rows.len(),
        accuracy * 100.0,
        bar * 100.0
    );

    assert!(
        accuracy >= bar,
        "steer-routing accuracy {:.1}% below the bar {:.1}% ({} of {} items misrouted)",
        accuracy * 100.0,
        bar * 100.0,
        rows.len() - correct,
        rows.len()
    );
}
