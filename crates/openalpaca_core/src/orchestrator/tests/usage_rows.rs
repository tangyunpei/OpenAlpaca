//! USAGE-PER-REQUEST (item 2 of the owner's second answers of 2026-11-14):
//! the `llm_usage` row of a chat turn and of a skill's invocation is priced
//! from the costs its requests were counted at, one by one — never from the
//! turn's summed tokens, which can pass a long-prompt threshold no request
//! passed.
//!
//! Every figure here is the test's own: the rows are a synthetic id with an
//! invented threshold and invented prices, so no provider's price is pinned.

use super::*;
use openalpaca_llm::{
    ChatResponse, FinishReason, LlmError, LlmProvider, LongPromptRate, Role, Usage,
};

/// The synthetic row every response here names.
const PRICED: &str = "usage-rows-test-model";

/// `PRICED`'s rates: $1 / $5 per million, and $2 / $10 once a request's
/// prompt is longer than 1,000 tokens.
const LONG_ABOVE: u32 = 1_000;

/// What the loop sends after a cut answer (`agentic_loop`, `MaxTokens`).
const CONTINUE: &str = "Your previous response was truncated";

/// A provider that answers in as many requests as it has usages: every
/// request but the last is cut at its output limit (`MaxTokens`, with
/// text), so the loop asks again with no tool involved. Which request this
/// is comes from the request itself (the continuation prompts it carries),
/// so a call made beside the turn takes nothing from the script.
struct PricedRounds {
    usages: Vec<Usage>,
}

impl LlmProvider for PricedRounds {
    fn name(&self) -> &str {
        "priced-rounds"
    }

    fn supports_tools(&self) -> bool {
        true
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let asked_again = request
            .messages
            .iter()
            .filter(|m| matches!(m.role, Role::User) && m.content.starts_with(CONTINUE))
            .count();
        let last = self.usages.len() - 1;
        let n = asked_again.min(last);
        Ok(ChatResponse {
            content: format!("part {n}"),
            tool_calls: vec![],
            model: PRICED.to_string(),
            usage: self.usages[n].clone(),
            finish_reason: if n < last {
                FinishReason::MaxTokens
            } else {
                FinishReason::Stop
            },
            thinking: None,
            echo: None,
            parts: None,
        })
    }
}

fn usage(input: u32, output: u32) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        ..Default::default()
    }
}

/// A router over [`PricedRounds`] whose registry prices [`PRICED`] at the
/// test's rates, a long-prompt rate included and no cache figures (a read
/// at 0.1x, the tracker's figure for a row that states none).
fn priced_router(usages: Vec<Usage>) -> Arc<openalpaca_llm::LlmRouter> {
    let router = openalpaca_llm::LlmRouter::single_provider(
        Arc::new(PricedRounds { usages }),
        openalpaca_llm::ProviderType::Anthropic,
        "claude-sonnet-5-5".to_string(),
    );
    let registry = router.model_registry();
    let mut info = registry
        .get_model_info("claude-sonnet-5-5")
        .expect("the default model has a row");
    info.input_price_per_million = 1.0;
    info.output_price_per_million = 5.0;
    info.long_prompt = Some(LongPromptRate {
        above_input_tokens: LONG_ABOVE,
        input_price_per_million: 2.0,
        output_price_per_million: 10.0,
    });
    info.cache_rates = None;
    registry.register(PRICED.to_string(), info);
    Arc::new(router)
}

/// The one `orchestrator` usage row the turn wrote.
fn the_row(
    db: &openalpaca_storage::Database,
) -> openalpaca_storage::repository::llm_usage::LlmCallLog {
    let rows = openalpaca_storage::repository::LlmUsageRepository::new(db)
        .get_agent_usage("orchestrator", 10)
        .unwrap();
    assert_eq!(rows.len(), 1, "one row for the turn: {rows:?}");
    rows.into_iter().next().unwrap()
}

fn assert_cost(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-12,
        "{what}: recorded ${actual}, expected ${expected}"
    );
}

/// The costs the `orchestrator` turn's `LlmCallCompleted` events carried.
fn announced(rx: &mut tokio::sync::broadcast::Receiver<SystemEvent>) -> Vec<f64> {
    let mut costs = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let SystemEvent::LlmCallCompleted {
            agent_id, cost_usd, ..
        } = event
            && agent_id == "orchestrator"
        {
            costs.push(cost_usd);
        }
    }
    costs
}

/// A main-loop turn on `router`, with a database, through the front door;
/// with the costs its `LlmCallCompleted` events announced.
async fn main_loop_turn(
    router: Arc<openalpaca_llm::LlmRouter>,
    content: &str,
) -> (tempfile::TempDir, openalpaca_storage::Database, Vec<f64>) {
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let orch = make_orchestrator_with_llm_agents_and_config(
        router,
        vec![],
        DaemonConfig::default(),
        Some(db.clone()),
    );
    let mut rx = orch.bus.subscribe();
    send_tool_mode(&orch, Uuid::new_v4(), content).await;
    orch.background.wait_idle().await;
    let costs = announced(&mut rx);
    (dir, db, costs)
}

/// **The defect.** Two requests of 600 prompt tokens each: neither passes
/// the 1,000-token threshold, so each was counted at the standard rate
/// ($0.0011: 600 at $1 and 100 at $5 per million). Their sum, 1,200, does
/// pass it — and the row used to price the sum, at the long rate ($0.0044).
#[tokio::test]
async fn a_turn_of_two_requests_under_the_threshold_is_recorded_at_the_standard_rate() {
    let router = priced_router(vec![usage(600, 100), usage(600, 100)]);
    let (_dir, db, announced) = main_loop_turn(router.clone(), "explain the borrow checker").await;

    let row = the_row(&db);
    assert_eq!((row.input_tokens, row.output_tokens), (1_200, 200));
    assert_eq!(row.model, PRICED);
    assert_cost(
        row.cost_usd,
        0.0022,
        "two requests each under the threshold, at the standard rate",
    );
    assert_eq!(announced.len(), 1, "one event for the turn: {announced:?}");
    assert_cost(announced[0], 0.0022, "the turn's event says the same");
    let counted = router
        .cost_tracker
        .get_agent_usage("orchestrator")
        .await
        .expect("the turn's requests were counted")
        .total_cost_usd;
    assert_cost(row.cost_usd, counted, "the row says what the caps counted");
}

/// The same on the skill tier: a skill's invocation writes its own row.
#[tokio::test]
async fn a_skill_turn_of_two_requests_under_the_threshold_is_recorded_at_the_standard_rate() {
    let tmp = tempfile::tempdir().unwrap();
    let skill_dir = tmp.path().join("writer");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        r#"---
name: "Writer"
description: "Writes at length"
invoke:
  slash: "/write"
  mode: "auto"
---

## Instructions

Write.
"#,
    )
    .unwrap();
    let catalog = skill_catalog::SkillCatalog::new();
    catalog.scan_directory(tmp.path(), crate::middleware::skill::SkillScope::Project);
    let dir = tempfile::tempdir().unwrap();
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).unwrap();
    let router = priced_router(vec![usage(600, 100), usage(600, 100)]);
    let bus = EventBus::default();
    let gate = make_security_gate(&bus);
    let orch = Orchestrator::new(
        Arc::new(SharedContext::new()),
        Arc::new(LaneManager::new()),
        bus,
        SystemPersona::default(),
        Some(router),
        LoopConfig::default(),
        gate,
        make_tool_registry(),
        Some(db.clone()),
        Arc::new(catalog),
        Arc::new(skill_router::SkillRouter::new(0.65, 0.45)),
        Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
    );

    let mut rx = orch.bus.subscribe();
    orch.handle_skill_invocation(
        Uuid::new_v4(),
        "cli",
        "Writer",
        "write it all",
        "test:cli",
        None,
        &ConversationContext::default(),
        None,
        &crate::memory::scope_context::MemoryScopeContext::new(None),
        None,
        false,
        None,
        false,
        None,
        None,
    )
    .await
    .expect("the skill runs");

    let row = the_row(&db);
    assert_eq!((row.input_tokens, row.output_tokens), (1_200, 200));
    assert_cost(
        row.cost_usd,
        0.0022,
        "two requests each under the threshold, at the standard rate",
    );
    let announced = announced(&mut rx);
    assert_eq!(announced.len(), 1, "one event for the turn: {announced:?}");
    assert_cost(announced[0], 0.0022, "the turn's event says the same");
}

/// **Unchanged:** a turn of one request is priced as that request, the long
/// rate included when its own prompt passes the threshold (1,200 at $2 and
/// 100 at $10 per million).
#[tokio::test]
async fn a_one_request_turn_past_the_threshold_keeps_the_long_rate() {
    let router = priced_router(vec![usage(1_200, 100)]);
    let (_dir, db, _) = main_loop_turn(router, "explain the borrow checker").await;

    let row = the_row(&db);
    assert_eq!((row.input_tokens, row.output_tokens), (1_200, 100));
    assert_cost(row.cost_usd, 0.0034, "one request past the threshold");
}

/// **Unchanged:** the social fast path runs one request, and its row is
/// that request's cost.
#[tokio::test]
async fn a_social_turn_is_priced_as_its_one_request() {
    let router = priced_router(vec![usage(600, 100)]);
    let (_dir, db, _) = main_loop_turn(router, "thanks").await;

    let row = the_row(&db);
    assert_eq!((row.input_tokens, row.output_tokens), (600, 100));
    assert_cost(row.cost_usd, 0.0011, "one request under the threshold");
}

/// The social fast path asks for no caching, but the response's usage can
/// still report cache reads (the OpenAI arm fills them from every response,
/// whatever the request asked), and its row prices them as the tracker
/// counted them: 600 prompt tokens, 400 read from the cache — 200 at $1,
/// 400 at $0.10 and 100 at $5 per million.
#[tokio::test]
async fn a_social_turns_cache_reads_are_recorded_as_they_were_counted() {
    let mut cached = usage(600, 100);
    cached.cache_read_input_tokens = 400;
    let router = priced_router(vec![cached]);
    let (_dir, db, _) = main_loop_turn(router.clone(), "thanks").await;

    let row = the_row(&db);
    assert_eq!((row.input_tokens, row.output_tokens), (600, 100));
    assert_cost(row.cost_usd, 0.00074, "social cache reads");
    let counted = router
        .cost_tracker
        .get_agent_usage("orchestrator")
        .await
        .expect("the turn's request was counted")
        .total_cost_usd;
    assert_cost(row.cost_usd, counted, "the row says what the caps counted");
}

/// **What the per-request cost brings with it:** a request's cache reads
/// are priced as the tracker counted them (0.1x the input rate on a row
/// that states no figure), where the row used to price every prompt token
/// at the full input rate. 600 prompt tokens, 400 read from the cache:
/// 200 at $1, 400 at $0.10 and 100 at $5 per million.
#[tokio::test]
async fn a_turns_cache_reads_are_recorded_as_they_were_counted() {
    let mut cached = usage(600, 100);
    cached.cache_read_input_tokens = 400;
    let router = priced_router(vec![cached]);
    let (_dir, db, _) = main_loop_turn(router.clone(), "explain the borrow checker").await;

    let row = the_row(&db);
    assert_cost(row.cost_usd, 0.00074, "cache reads at the row's multiple");
    let counted = router
        .cost_tracker
        .get_agent_usage("orchestrator")
        .await
        .expect("the turn's request was counted")
        .total_cost_usd;
    assert_cost(row.cost_usd, counted, "the row says what the caps counted");
}
