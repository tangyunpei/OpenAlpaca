//! Task-output memory extraction (design §6 *Promotion and correction*,
//! owner decision MD-2; plan 3.4): what a finished run's output teaches,
//! written through [`MemoryService`] on the `Extraction` surface as
//! **candidates** — origin `extracted_other`, outside recall until the user
//! confirms one. Tool output is never the user's own words, so nothing here
//! ever becomes an active note.
//!
//! What produces nothing: a **failed or cancelled** run (the caller never
//! starts the extraction, and the extractor refuses one anyway), an
//! **assistant-only completion claim** — an item the model marks
//! `basis: "claim"`, the run's output asserting that something was done
//! rather than showing what was learned — and an owner that is not a
//! `global_user` row (a `system` run has no partition, design §3).
//!
//! Each item carries the chat extractor's aliases — the other language
//! included — and `fact_key`, cleaned by the same rules, and the prompt
//! lists the keys already in use so a run reuses them (design 09 §2.7). An
//! item never replaces anything: it is a candidate, and it names no target.
//!
//! Bounded per run by count (`[orchestrator.costs]
//! task_extract_max_items_per_run`, the items past it dropped and counted)
//! and by cost (`task_extract_max_daily_cost_usd`, per UTC day, read from
//! `llm_usage_daily`); an item under
//! `memory_confidence_threshold` is dropped. Every note cites the task row
//! as its source. It runs as a tracked job on the orchestrator's
//! `BackgroundJobs`, attributed to the task, and stops on the shutdown token.

use arc_swap::ArcSwap;
use openalpaca_llm::{ChatMessage, LlmRouter, RequestContext, RouterRequest};
use openalpaca_storage::Database;
use openalpaca_storage::memory_files::{Dedup, NoteKind, NoteSource, SourceType};
use openalpaca_storage::repository::LlmUsageRepository;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::daemon_config::DaemonConfig;
use crate::memory::service::{
    Attribution, Capabilities, ExtractionProvenance, MemoryError, MemoryService, StoreInput,
    Surface, WorkspaceHint,
};

/// The usage label the extractor's calls are billed and gated under.
pub(crate) const AGENT_LABEL: &str = "orchestrator_task_extract";

/// What the extractor needs from the dispatcher.
pub(crate) struct TaskExtractionDeps {
    pub db: Database,
    pub router: Arc<LlmRouter>,
    pub daemon_config: Arc<ArcSwap<DaemonConfig>>,
    pub memory_service: Arc<MemoryService>,
    /// The orchestrator's shutdown token: a cancelled extraction writes
    /// nothing.
    pub cancel: CancellationToken,
}

/// One finished run, as the extractor sees it.
pub struct TaskExtractionParams {
    /// The run's `created_by`: a `global_user` row, or nothing is written.
    pub owner_id: String,
    pub task_id: String,
    pub task_description: String,
    pub task_output: String,
    /// Which dispatch path ran it: `"lead_agent"`.
    pub source_path: String,
    /// The MD-5 root the run's request resolved, never the daemon's CWD.
    pub project_root: Option<PathBuf>,
    /// Whether the run finished successfully. A failed or cancelled run
    /// teaches nothing (design §6).
    pub success: bool,
}

/// What one extraction did, counted without note bodies (design §8).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TaskExtractionReport {
    /// Candidates written.
    pub stored: usize,
    /// Items an existing note already held byte for byte.
    pub deduplicated: usize,
    /// Items past `task_extract_max_items_per_run`.
    pub dropped_over_cap: usize,
    /// Items under `memory_confidence_threshold`.
    pub dropped_below_threshold: usize,
    /// Assistant-only completion claims (`basis: "claim"`).
    pub dropped_claims: usize,
    /// Items with no content or an unknown kind.
    pub dropped_invalid: usize,
    /// Items the service refused.
    pub failed: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TaskExtractionOutcome {
    /// A gate closed before any model call: the reason names it.
    Skipped(&'static str),
    /// No model could answer (§7b P10): one WARN, the run is unaffected.
    Unavailable,
    /// The shutdown token fired before the answer was written.
    Cancelled,
    Extracted(TaskExtractionReport),
}

/// Runs one finished run's extraction. Every failure is logged, none
/// propagates.
pub(crate) async fn extract_task_memories(
    deps: &TaskExtractionDeps,
    params: TaskExtractionParams,
) -> TaskExtractionOutcome {
    if !params.success {
        return TaskExtractionOutcome::Skipped("not_successful");
    }
    let dcfg = deps.daemon_config.load();
    if !dcfg.orchestrator.costs.task_extract_enabled {
        return TaskExtractionOutcome::Skipped("disabled");
    }

    // Guard: skip short output.
    let min_len = dcfg.orchestrator.costs.task_extract_min_content_len;
    if params.task_output.len() < min_len {
        tracing::debug!(
            "Task extraction skipped: output too short ({} < {min_len})",
            params.task_output.len()
        );
        return TaskExtractionOutcome::Skipped("content_filter");
    }

    // The owner must be a `global_user` row (design §3): a `system` run —
    // `/v1/command process` and every workflow it starts — has no partition,
    // and no model call is spent finding that out.
    let hint = match &params.project_root {
        Some(root) => WorkspaceHint::Root(root.as_path()),
        None => WorkspaceHint::None,
    };
    let ctx = match deps.memory_service.context(
        &params.owner_id,
        hint,
        Surface::Extraction,
        Capabilities::EXTRACTION,
        Attribution {
            lane_key: None,
            task_id: Some(params.task_id.clone()),
            request_id: None,
            source: Some(params.source_path.clone()),
        },
    ) {
        Ok(ctx) => ctx,
        Err(MemoryError::NoOwner(_)) => {
            tracing::debug!(task_id = %params.task_id, "task extraction: no owner, nothing to extract");
            return TaskExtractionOutcome::Skipped("no_owner");
        }
        Err(e) => {
            tracing::warn!(task_id = %params.task_id, "task extraction: context refused: {e}");
            return TaskExtractionOutcome::Skipped("context_refused");
        }
    };

    // Guard: the daily cost cap, read durably for today's UTC date (Learning
    // v2, design 09 §1.8, ruling LR8). The in-memory cost tracker is seeded
    // once at boot and never reset, so "daily" used to mean "since boot".
    let db = deps.db.clone();
    let extract_cost =
        tokio::task::spawn_blocking(move || LlmUsageRepository::new(&db).usage_today(AGENT_LABEL))
            .await
            .ok()
            .and_then(Result::ok)
            .map_or(0.0, |(_, cost)| cost);
    let max_daily = dcfg.orchestrator.costs.task_extract_max_daily_cost_usd;
    if extract_cost > max_daily {
        tracing::debug!(
            "Task extraction skipped: cost ${extract_cost:.2} exceeds cap ${max_daily:.2}"
        );
        return TaskExtractionOutcome::Skipped("cost_cap");
    }

    let max_items = dcfg
        .orchestrator
        .costs
        .task_extract_max_items_per_run
        .max(1);

    // The keys in use (design 09 §2.7), read like the chat extractor's
    // `known_keys`: from the index, on the blocking pool, recorded nowhere.
    let service = deps.memory_service.clone();
    let keys_ctx = ctx.clone();
    let view =
        tokio::task::spawn_blocking(move || service.learning_view(&keys_ctx, &[], &[])).await;
    let known_keys = match view {
        Ok(Ok(view)) => crate::memory::learning::render_keys(
            &view.keys,
            crate::memory::learning::MAX_KEYS_BYTES,
        ),
        failed => {
            tracing::debug!(task_id = %params.task_id, "task extraction: no known keys: {failed:?}");
            String::new()
        }
    };
    let threshold = dcfg.orchestrator.memory.memory_confidence_threshold;

    // Build the extraction prompt.
    let desc_trunc: String = params.task_description.chars().take(500).collect();
    let output_trunc: String = params.task_output.chars().take(2000).collect();

    let user_prompt = format!(
        "## Task Output\nTask: {desc_trunc}\nOutput:\n{output_trunc}\n\n\
         <known_keys>{known_keys}</known_keys>\n\n\
         Extract the durable knowledge this run's output shows: findings about the \
         project or its environment, decisions taken, procedures that worked, references \
         worth keeping.\n\
         Output ONLY a JSON object with this schema:\n\
         {{\n\
           \"extractions\": [\n\
             {{\n\
               \"title\": \"a short title, at most 80 characters\",\n\
               \"content\": \"one concise statement\",\n\
               \"kind\": \"fact\" | \"preference\" | \"decision\" | \"procedure\" | \"reference\" | \"episode\",\n\
               \"basis\": \"evidence\" | \"claim\",\n\
               \"confidence\": 0.0 to 1.0,\n\
               \"aliases\": [\"other names a later question might use\"],\n\
               \"fact_key\": \"subject.attribute\" | null\n\
             }}\n\
           ]\n\
         }}\n\n\
         Rules:\n\
         - \"basis\": \"evidence\" when the output shows the thing itself — a tool result, \
         a file's contents, a command's output, an observed value.\n\
         - \"basis\": \"claim\" when the output merely asserts that something was done or is \
         true, with nothing in it to show it. Claims are never stored; mark them honestly.\n\
         - Do not extract that the task was completed, what the assistant did, or the \
         task's own instructions.\n\
         - \"aliases\": other names a later question might use, including the other \
         language (Chinese for an English statement, English for a Chinese one), plus \
         common alternative spellings and proper nouns. 0 to 8; never the title.\n\
         - \"fact_key\": <subject>.<attribute>[.<qualifier>] in lowercase English, the \
         project's name as the subject for a fact about a project. Reuse a key from \
         known_keys when the item is about the same attribute; mint a new one only when \
         none fits. Never put the value in the key.\n\
         - Only extract what is clearly stated or strongly implied. Do not invent.\n\
         - Prefer fewer high-quality extractions over many low-quality ones.\n\
         - At most {max_items} extractions, most useful first.\n\
         - If nothing can be extracted, return {{\"extractions\": []}}."
    );

    let request = RouterRequest {
        // `[orchestrator.costs] extraction_model` is the one key for
        // background extraction, and it names this extractor's model as it
        // does the chat turn's (plan 3.4; before it this call always took the
        // router default). `None` is still the router default.
        model: dcfg.orchestrator.costs.extraction_model.clone(),
        messages: Arc::new(vec![
            ChatMessage::system(
                "You are the knowledge extractor for task outputs in OpenAlpaca. \
                 Output ONLY valid JSON matching the schema. \
                 No markdown fences, no commentary.",
            ),
            ChatMessage::user(&user_prompt),
        ]),
        tools: Arc::new(vec![]),
        temperature: Some(0.0),
        // Aliases and a key per item did not fit 512 tokens (design 09 §2.7).
        max_tokens: Some(1024),
        context: RequestContext {
            agent_id: Some(AGENT_LABEL.to_string()),
            task_id: Some(params.task_id.clone()),
        },
        tool_choice: None,
        tools_token_estimate: None,
        enable_caching: false,
        // M2: an internal utility call, capped at 1,024 tokens — no reasoning.
        thinking: Some(openalpaca_llm::ThinkingConfig::Disabled),
        context_management: None,
        fallback_models: Vec::new(),
        ephemeral_system_notice: None,
    };
    drop(dcfg);

    // LLM call, stopped by the shutdown token.
    let call_start = std::time::Instant::now();
    let response = tokio::select! {
        biased;
        _ = deps.cancel.cancelled() => {
            tracing::debug!(task_id = %params.task_id, "task extraction cancelled before the model answered");
            return TaskExtractionOutcome::Cancelled;
        }
        result = deps.router.complete(request) => match result {
            Ok(r) => r,
            Err(e) => {
                // §7b P10: one WARN, the run is unaffected.
                tracing::warn!(task_id = %params.task_id, "task extraction unavailable: {e}");
                return TaskExtractionOutcome::Unavailable;
            }
        },
    };
    let latency_ms = call_start.elapsed().as_millis() as i64;

    // Record LLM usage.
    let actual_model = response.model.clone();
    let resolved_provider = deps
        .router
        .model_registry()
        .resolve_provider(&actual_model)
        .map(|p| p.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let call_cost = deps.router.cost_tracker.calculate_cost(
        &actual_model,
        response.usage.input_tokens as u32,
        response.usage.output_tokens as u32,
    );
    let usage_repo = LlmUsageRepository::new(&deps.db);

    let parsed = run_answer(
        &response.content,
        response.finish_reason == openalpaca_llm::FinishReason::MaxTokens,
    );
    let parsed = match parsed {
        Some(v) => v,
        None => {
            if let Err(e) = usage_repo.record_and_log(
                AGENT_LABEL,
                Some(&params.task_id),
                &resolved_provider,
                &actual_model,
                response.usage.input_tokens as i32,
                response.usage.output_tokens as i32,
                call_cost,
                latency_ms,
                "error",
                Some("JSON parse failure"),
            ) {
                tracing::warn!("Failed to persist task extraction LLM usage: {e}");
            }
            tracing::warn!(task_id = %params.task_id, "task extraction: malformed JSON from the model");
            return TaskExtractionOutcome::Extracted(TaskExtractionReport::default());
        }
    };
    if let Err(e) = usage_repo.record_and_log(
        AGENT_LABEL,
        Some(&params.task_id),
        &resolved_provider,
        &actual_model,
        response.usage.input_tokens as i32,
        response.usage.output_tokens as i32,
        call_cost,
        latency_ms,
        "success",
        None,
    ) {
        tracing::warn!("Failed to persist task extraction LLM usage: {e}");
    }

    if deps.cancel.is_cancelled() {
        return TaskExtractionOutcome::Cancelled;
    }

    let extractions = parsed
        .get("extractions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Every candidate cites the run (design §4: `sources` cite DB ids). The
    // service verifies the row exists; a run that was never persisted is
    // refused per item and counted as failed.
    let sources = vec![NoteSource {
        kind: SourceType::Task,
        id: params.task_id.clone(),
        observed_at: None,
    }];

    let mut report = TaskExtractionReport::default();
    for extraction in &extractions {
        let Some(item) = read_item(extraction) else {
            report.dropped_invalid += 1;
            continue;
        };
        if item.claim {
            report.dropped_claims += 1;
            continue;
        }
        if item.confidence < threshold {
            report.dropped_below_threshold += 1;
            continue;
        }
        if report.stored + report.deduplicated + report.failed >= max_items {
            report.dropped_over_cap += 1;
            continue;
        }
        let input = StoreInput {
            title: item.title,
            body: item.body,
            kind: Some(item.kind),
            aliases: item.aliases,
            fact_key: item.fact_key,
            sources: sources.clone(),
            // Tool output, never the user's words: a candidate (MD-2).
            provenance: Some(ExtractionProvenance::OtherOutput),
            ..Default::default()
        };
        match deps.memory_service.store(&ctx, input) {
            Ok(out) if out.dedup == Dedup::Existing => report.deduplicated += 1,
            Ok(_) => report.stored += 1,
            Err(e) => {
                tracing::warn!(task_id = %params.task_id, "task extraction: the service refused a note: {e}");
                report.failed += 1;
            }
        }
    }

    tracing::info!(
        task_id = %params.task_id,
        source_path = %params.source_path,
        stored = report.stored,
        deduplicated = report.deduplicated,
        dropped_over_cap = report.dropped_over_cap,
        dropped_below_threshold = report.dropped_below_threshold,
        dropped_claims = report.dropped_claims,
        dropped_invalid = report.dropped_invalid,
        failed = report.failed,
        cost_usd = call_cost,
        "task extraction finished"
    );
    TaskExtractionOutcome::Extracted(report)
}

/// One item of the model's answer, validated.
struct Item {
    title: String,
    body: String,
    kind: NoteKind,
    /// `basis: "claim"` — an assertion with nothing in the output to show
    /// it. Never stored.
    claim: bool,
    confidence: f64,
    /// Rules 7–8 of the chat extractor (design 09 §2.4, §2.7).
    aliases: Vec<String>,
    fact_key: Option<String>,
}

/// Reads one `extractions[]` entry. `None` is an invalid item: no content,
/// or a kind that is not one of the six. A missing kind is a `fact`; a
/// missing basis is a claim — the model has to say it saw the thing; and a
/// missing confidence is `0.0`, under any positive floor, so an item the
/// model did not rate is dropped rather than admitted at an invented `0.7`
/// (the pre-3.4 default). Fail closed: a candidate that was never rated is
/// not worth the user's confirmation prompt.
fn read_item(value: &serde_json::Value) -> Option<Item> {
    let content = value.get("content")?.as_str()?.trim();
    if content.is_empty() {
        return None;
    }
    let kind = match value.get("kind").and_then(|v| v.as_str()) {
        None | Some("") => NoteKind::Fact,
        Some(k) => NoteKind::parse(k.trim())?,
    };
    let claim = !value
        .get("basis")
        .and_then(|v| v.as_str())
        .is_some_and(|b| b.trim().eq_ignore_ascii_case("evidence"));
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
        aliases: crate::memory::learning::clean_aliases(value.get("aliases"), &title),
        fact_key: crate::memory::learning::clean_fact_key(value.get("fact_key")),
        title,
        kind,
        claim,
        confidence,
    })
}

/// The run's answer. LQ13: an answer that ended on its own is unwrapped —
/// a value found in its prose only with the answer's shape
/// ([`is_extractions_answer`]); one `cut` at its output ceiling is held to
/// the forms read before (the L8 fix slice's P1).
fn run_answer(content: &str, cut: bool) -> Option<serde_json::Value> {
    if cut {
        return parse_json_strict(content);
    }
    read_json_answer(content, is_extractions_answer)
        .filter(|answer| answer.whole || is_extractions_answer(&answer.value))
        .map(|answer| answer.value)
}

/// A run's answer, by its shape (LQ13): `{"extractions": [...]}` whose
/// every entry carries the `content` an item needs — so a fragment of the
/// run's own output quoted in the model's prose is not taken for it.
fn is_extractions_answer(value: &serde_json::Value) -> bool {
    value
        .get("extractions")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .all(|i| i.get("content").is_some_and(serde_json::Value::is_string))
        })
}

/// The JSON value an answer carries (owner decision LQ13, 2026-09-26),
/// whatever the caller's shape: [`parse_json_answer`] with no shape
/// preferred. Shared with the chat-turn extractor and the conversation
/// summaries, which pass their own shape.
pub fn parse_json_response(content: &str) -> Option<serde_json::Value> {
    parse_json_answer(content, |_| true)
}

/// The JSON value an answer carries (owner decision LQ13, 2026-09-26), as
/// [`read_json_answer`] finds it, however it was found.
pub fn parse_json_answer(
    content: &str,
    expected: impl Fn(&serde_json::Value) -> bool,
) -> Option<serde_json::Value> {
    read_json_answer(content, expected).map(|answer| answer.value)
}

/// An answer's JSON value, and how it was found.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonAnswer {
    pub value: serde_json::Value,
    /// Read as the whole answer, or as a fenced block — the forms read
    /// before LQ13 — rather than found among prose. A caller holds a value
    /// found among prose to the shape it expects: a quoted fragment or a
    /// draft is not an answer.
    pub whole: bool,
}

/// The JSON value an answer carries (owner decision LQ13, 2026-09-26;
/// tightened in repair round 1 of the L8 product slice):
///
/// 1. The whole answer, read as JSON or as exactly one fenced block, is the
///    answer — whatever its strings say.
/// 2. Else the answer is the text after the last `</think>`: the model's
///    reasoning comes before it, whether or not it opened with `<think>`.
///    When a `</think>` ends the answer — what a local thinking model leaves
///    when its thinking is switched off — the text before it is the answer,
///    read whole only (as JSON, or as one fenced block). An opening
///    `<think>` never closed is reasoning with no answer after it: `None`.
///    A tag inside a JSON value is text, not a marker (repair round 2).
/// 3. That text is read whole, or from a fenced block, as before LQ13.
/// 4. Else the complete top-level values found in its prose are taken: the
///    last one of the shape `expected` names, else the last — which a
///    caller then refuses for its shape ([`JsonAnswer::whole`] is false). A
///    value the prose opens and never finishes — a truncated one — makes
///    the whole answer unreadable. A bracket that closes but is not JSON is
///    skipped whole, so nothing inside it is taken; one that never closes
///    is skipped by itself, so a stray `[` in the prose does not hide the
///    answer after it — and a value nested in a malformed wrapper that never
///    closes is then found as the prose's next value.
///
/// `None` when there is no value.
pub fn read_json_answer(
    content: &str,
    expected: impl Fn(&serde_json::Value) -> bool,
) -> Option<JsonAnswer> {
    let whole = |value| JsonAnswer { value, whole: true };
    if let Some(value) = read_whole(content.trim()) {
        return Some(whole(value));
    }
    let (text, prose) = answer_text(content)?;
    if !prose {
        return read_whole(text).map(whole);
    }
    if let Some(value) = parse_json_strict(text) {
        return Some(whole(value));
    }
    let (mut values, _) = top_level_values(text)?;
    let pick = values
        .iter()
        .rposition(expected)
        .unwrap_or(values.len() - 1);
    Some(JsonAnswer {
        value: values.swap_remove(pick),
        whole: false,
    })
}

/// The JSON value an answer cut at its output ceiling carries (owner
/// decision LQ20 (f), 2026-09-28), for a caller whose forms read before
/// LQ13 ([`parse_json_strict`]) found none: the text after the model's
/// reasoning ([`answer_text`]), when it **ends** on one complete top-level
/// value of the shape `expected` names — what the ceiling cut was a closing
/// fence. A value opened and never closed, a value more text follows (the
/// model was still writing: a quote of the prompt's own `{"items": []}` in
/// its reasoning), and a value of another shape are not taken, so no part
/// of a cut answer is rescued. `None` when there is no such value.
///
/// Its callers: chat learning's extraction, and — the continuity fix
/// round's ruling F7 (design 11 rev 1.7) — the working-state writer
/// (`WorkingState::parse`), which shares this read of a cut answer.
/// Task-output extraction and a conversation summary's own call (owner
/// decision CQ-11, `memory::learning`'s `parse_cut_summary`) keep the
/// strict read.
pub fn read_cut_answer(
    content: &str,
    expected: impl Fn(&serde_json::Value) -> bool,
) -> Option<serde_json::Value> {
    let (text, prose) = answer_text(content)?;
    if !prose {
        return read_whole(text).filter(|value| expected(value));
    }
    let (values, end) = top_level_values(text)?;
    let last = values.into_iter().last()?;
    (end == text.len() && expected(&last)).then_some(last)
}

/// The part of an answer that is the answer, and whether values may be
/// found among its prose (step 2 of [`read_json_answer`]): the text after
/// the last `</think>`; the text before a `</think>` that ends the answer,
/// read whole only; `None` when an opening `<think>` is never closed. A tag
/// inside a JSON value — a string the answer's own JSON quotes, fenced or
/// not — is text, not a marker ([`think_markers`]; repair round 2 of the L8
/// product slice).
fn answer_text(content: &str) -> Option<(&str, bool)> {
    let body = content.trim();
    let markers = think_markers(body);
    let last_close = markers
        .iter()
        .rev()
        .find(|m| m.1 == Think::Close)
        .map(|m| m.0);
    let opened = |range: std::ops::Range<usize>| {
        markers
            .iter()
            .any(|&(at, kind)| kind == Think::Open && range.contains(&at))
    };
    match last_close {
        Some(at) => match body[at + Think::Close.tag().len()..].trim() {
            "" => (!opened(0..at)).then(|| (body[..at].trim(), false)),
            after => (!opened(at..body.len())).then_some((after, true)),
        },
        None => (!opened(0..body.len())).then_some((body, true)),
    }
}

/// A `<think>` or `</think>` tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Think {
    Open,
    Close,
}

impl Think {
    fn tag(self) -> &'static str {
        match self {
            Think::Open => "<think>",
            Think::Close => "</think>",
        }
    }
}

/// Where the `<think>` and `</think>` tags stand in `text` outside every
/// complete JSON value, in order: a value that begins at an `{` or `[` is
/// stepped over whole, so a tag a JSON string quotes is not one. A value
/// that runs unfinished to the end is a valid JSON prefix, which holds a
/// tag only inside a string, so the scan ends there.
fn think_markers(text: &str) -> Vec<(usize, Think)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(offset) = text[at..].find(['<', '{', '[']) {
        let start = at + offset;
        let rest = &text[start..];
        if let Some(kind) = [Think::Open, Think::Close]
            .into_iter()
            .find(|kind| rest.starts_with(kind.tag()))
        {
            out.push((start, kind));
            at = start + kind.tag().len();
            continue;
        }
        if rest.starts_with('<') {
            at = start + 1;
            continue;
        }
        let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<serde_json::Value>();
        match stream.next() {
            Some(Ok(_)) => at = start + stream.byte_offset(),
            Some(Err(e)) if e.is_eof() => break,
            _ => at = start + 1,
        }
    }
    out
}

/// The text as one JSON value, or as one fenced block and nothing else.
fn read_whole(text: &str) -> Option<serde_json::Value> {
    if let Ok(value) = serde_json::from_str(text) {
        return Some(value);
    }
    let fenced = text.strip_prefix("```")?.strip_suffix("```")?;
    let fenced = fenced.strip_prefix("json").unwrap_or(fenced);
    serde_json::from_str(fenced.trim()).ok()
}

/// The complete JSON values that begin at an `{` or `[` outside any value,
/// in order, and the byte where the last one ends. `None` when there is
/// none, or when one begins and runs to the end unfinished — a truncated
/// answer is never read in part. A bracket that does not begin a value —
/// prose such as `[n1]`, or malformed JSON — is skipped to its matching
/// bracket, so no value inside it is taken; one with no matching bracket is
/// skipped by itself.
fn top_level_values(text: &str) -> Option<(Vec<serde_json::Value>, usize)> {
    let mut out = Vec::new();
    let mut at = 0;
    let mut end = 0;
    while let Some(offset) = text[at..].find(['{', '[']) {
        let start = at + offset;
        let rest = &text[start..];
        let mut stream = serde_json::Deserializer::from_str(rest).into_iter::<serde_json::Value>();
        match stream.next() {
            Some(Ok(value)) => {
                out.push(value);
                at = start + stream.byte_offset();
                end = at;
            }
            Some(Err(e)) if e.is_eof() => return None,
            _ => at = start + matching_bracket_end(rest).unwrap_or(1),
        }
    }
    (!out.is_empty()).then_some((out, end))
}

/// How many bytes of `text` — which starts with `{` or `[` — run to the
/// bracket that closes it, strings read as JSON strings; `None` when none
/// does.
fn matching_bracket_end(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// The forms read before LQ13: the whole answer as JSON, or a fenced block.
/// What an answer cut at its output ceiling is held to (the L8 fix slice's
/// P1: a cut answer is reported as cut, never rescued in part).
pub fn parse_json_strict(content: &str) -> Option<serde_json::Value> {
    let trimmed = content.trim();
    if let Ok(v) = serde_json::from_str(trimmed) {
        return Some(v);
    }
    // Try ```json fence
    if let Some(start) = trimmed.find("```json") {
        let after = &trimmed[start + 7..];
        if let Some(end) = after.find("```")
            && let Ok(v) = serde_json::from_str(after[..end].trim())
        {
            return Some(v);
        }
    }
    // Try plain ``` fence
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        if let Some(end) = after.find("```")
            && let Ok(v) = serde_json::from_str(after[..end].trim())
        {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests;
