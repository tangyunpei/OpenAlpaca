//! Response parsing, JSON extraction, retry loop, and lightweight classification.

use super::types::{PlanError, PlannedAssignment, PlannerLimits, TaskPlan};
use super::TaskPlanner;
use crate::agent::subagent::SubAgent;
use openalpaca_llm::{ChatMessage, LlmRouter, RequestContext, RouterRequest};
use std::sync::Arc;
use std::time::Duration;

// ── JSON extraction ─────────────────────────────────────────────────

/// Extract a JSON block from LLM output that may contain surrounding prose.
///
/// Handles (in order):
/// 1. Markdown ` ```json ... ``` ` fences
/// 2. Markdown ` ``` ... ``` ` fences
/// 3. Brace-matching fallback: outermost `{ ... }` respecting string literals
/// 4. Returns trimmed input unchanged if nothing matches
pub(crate) fn extract_json_block(content: &str) -> &str {
    let trimmed = content.trim();

    // Try ```json ... ``` first
    if let Some(start) = trimmed.find("```json") {
        let after_fence = &trimmed[start + 7..];
        if let Some(end) = after_fence.find("```") {
            return after_fence[..end].trim();
        }
    }

    // Try ``` ... ```
    if let Some(start) = trimmed.find("```") {
        let after_fence = &trimmed[start + 3..];
        if let Some(end) = after_fence.find("```") {
            return after_fence[..end].trim();
        }
    }

    // Brace-matching fallback: find outermost { ... }
    if let Some(json_slice) = find_outermost_braces(trimmed) {
        return json_slice;
    }

    trimmed
}

/// Find the outermost `{ ... }` in the string, respecting JSON string literals.
fn find_outermost_braces(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let mut start = None;
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, &b) in bytes.iter().enumerate() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if in_string {
            if b == b'\\' {
                escape_next = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0
                    && let Some(s_idx) = start
                {
                    return Some(&s[s_idx..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

// ── Response parsing ────────────────────────────────────────────────

impl TaskPlanner {
    /// Parse the LLM response into a TaskPlan.
    pub(super) fn parse_response(content: &str) -> Result<TaskPlan, PlanError> {
        let json_str = Self::extract_json(content);

        // Primary: direct parse
        let plan = if let Ok(plan) = serde_json::from_str::<TaskPlan>(json_str) {
            plan
        } else if let Ok(obj) = serde_json::from_str::<serde_json::Value>(json_str) {
            // Fallback: LLM may have wrapped the plan in a parent object
            if let Some(classification) = obj.get("classification").and_then(|v| v.as_str()) {
                TaskPlan {
                    classification: classification.to_string(),
                    title: obj.get("title").and_then(|v| v.as_str()).map(String::from),
                    assignments: obj
                        .get("assignments")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default(),
                    reasoning: obj
                        .get("reasoning")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    dag: obj
                        .get("dag")
                        .and_then(|v| serde_json::from_value(v.clone()).ok()),
                    use_lead_agent: obj
                        .get("use_lead_agent")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true),
                    auto_promotion_reason: None,
                    execution_mode: obj
                        .get("execution_mode")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    predictability_score: obj.get("predictability_score").and_then(|v| v.as_f64()),
                }
            } else {
                return Err(PlanError::MalformedResponse(format!(
                    "Failed to parse JSON: missing field `classification` (input: {})",
                    &content.chars().take(200).collect::<String>()
                )));
            }
        } else {
            return Err(PlanError::MalformedResponse(format!(
                "Failed to parse JSON: missing field `classification` (input: {})",
                &content.chars().take(200).collect::<String>()
            )));
        };

        // V2 protocol: when execution_mode is present, use it to resolve the
        // execution path authoritatively.
        if let Some(ref mode) = plan.execution_mode {
            match mode.as_str() {
                "lead_agent" => {
                    return Ok(TaskPlan {
                        use_lead_agent: true,
                        dag: None,
                        ..plan
                    });
                }
                "dag" => {
                    if plan.dag.is_some() {
                        return Ok(TaskPlan {
                            use_lead_agent: false,
                            ..plan
                        });
                    }
                    tracing::warn!(
                        classification = %plan.classification,
                        "execution_mode='dag' but no DAG provided, falling through to heuristics"
                    );
                }
                "pipeline" => {
                    return Ok(TaskPlan {
                        use_lead_agent: false,
                        dag: None,
                        ..plan
                    });
                }
                _ => {
                    tracing::warn!(
                        classification = %plan.classification,
                        execution_mode = %mode,
                        "Unknown execution_mode value, falling through to heuristics"
                    );
                }
            }
        }

        // Mutual exclusivity: if planner returned both use_lead_agent and a DAG,
        // strip the DAG (lead agent takes priority as the safer single-orchestrator path).
        if plan.use_lead_agent && plan.dag.is_some() {
            tracing::warn!(
                classification = %plan.classification,
                "Stripping DAG: use_lead_agent=true and dag both present"
            );
            return Ok(TaskPlan {
                dag: None,
                auto_promotion_reason: Some("mutual_exclusivity_stripped".into()),
                ..plan
            });
        }

        // Safety net: if the LLM returned complex_task but provided no execution
        // path, auto-promote to lead_agent.
        if plan.classification == "complex_task"
            && plan.assignments.is_empty()
            && plan.dag.is_none()
            && !plan.use_lead_agent
        {
            tracing::warn!(
                classification = %plan.classification,
                reasoning = ?plan.reasoning,
                title = ?plan.title,
                "Auto-promoting to lead agent: planner returned complex_task with no \
                 assignments, no DAG, and use_lead_agent=false."
            );
            return Ok(TaskPlan {
                use_lead_agent: true,
                auto_promotion_reason: Some("empty_complex_task".into()),
                ..plan
            });
        }

        Ok(plan)
    }

    /// Extract JSON from a response that may be wrapped in markdown code fences
    /// or surrounded by prose.
    pub(super) fn extract_json(content: &str) -> &str {
        extract_json_block(content)
    }
}

// ── Planning retry loop ─────────────────────────────────────────────

/// Shared retry loop for hierarchical planning.
pub(super) async fn plan_inner(
    router: &LlmRouter,
    messages: Vec<ChatMessage>,
    limits: PlannerLimits,
    idle_agents: &[SubAgent],
) -> Result<TaskPlan, PlanError> {
    let mut last_error = PlanError::MalformedResponse("no attempts made".to_string());
    let mut last_error_msg: Option<String> = None;
    let deadline = Duration::from_secs(limits.timeout_secs);

    for attempt in 0..=limits.max_retries {
        let mut attempt_messages = messages.clone();
        if attempt > 0
            && let Some(ref err) = last_error_msg
        {
            attempt_messages.push(ChatMessage::user(&format!(
                "Your previous response was invalid: {}. Respond with ONLY a valid JSON object.",
                err
            )));
        }
        let request = RouterRequest {
            model: None,
            messages: Arc::new(attempt_messages),
            tools: Arc::new(vec![]),
            temperature: Some(attempt as f32 * 0.1),
            max_tokens: Some(limits.max_tokens),
            context: RequestContext::default(),
            tool_choice: None,
            tools_token_estimate: None,
            enable_caching: false,
            thinking: None,
        };

        let response = tokio::time::timeout(deadline, router.complete(request))
            .await
            .map_err(|_| PlanError::Timeout(limits.timeout_secs))?
            .map_err(|e| PlanError::LlmError(e.to_string()))?;

        let response_content = response.content.clone();

        match TaskPlanner::parse_response(&response.content) {
            Ok(plan) => {
                if let Some(ref dag) = plan.dag
                    && let Err(e) = dag.validate(idle_agents)
                {
                    // Try to salvage as sequential pipeline via topological order.
                    let salvageable = !e.contains("cycle") && !e.contains("unknown agent");
                    if salvageable && !plan.assignments.is_empty() {
                        tracing::warn!(
                            dag_error = %e,
                            "DAG validation failed, falling back to flat assignments"
                        );
                        return Ok(TaskPlan {
                            dag: None,
                            auto_promotion_reason: Some(
                                "dag_validation_salvaged_pipeline".into(),
                            ),
                            ..plan
                        });
                    }

                    // Try extracting topological order as pipeline assignments
                    if salvageable {
                        let topo = dag.topological_order();
                        if !topo.is_empty() {
                            let pipeline_assignments: Vec<PlannedAssignment> = topo
                                .iter()
                                .filter_map(|nid| dag.nodes.iter().find(|n| n.node_id == *nid))
                                .map(|node| PlannedAssignment {
                                    agent_id: node.agent_id.clone(),
                                    agent_name: node.agent_name.clone(),
                                    role_description: node.description.clone(),
                                    matched_skills: vec![],
                                })
                                .collect();
                            if !pipeline_assignments.is_empty() {
                                tracing::warn!(
                                    dag_error = %e,
                                    pipeline_steps = pipeline_assignments.len(),
                                    "DAG validation failed, salvaging as sequential pipeline"
                                );
                                return Ok(TaskPlan {
                                    dag: None,
                                    assignments: pipeline_assignments,
                                    use_lead_agent: false,
                                    auto_promotion_reason: Some(
                                        "dag_to_pipeline_salvage".into(),
                                    ),
                                    ..plan
                                });
                            }
                        }
                    }

                    // Fallback: promote to lead agent
                    let promoted = plan.assignments.is_empty() && !plan.use_lead_agent;
                    if promoted {
                        tracing::warn!(
                            dag_error = %e,
                            "Auto-promoting to lead agent: DAG validation failed with no salvage path"
                        );
                    }
                    return Ok(TaskPlan {
                        dag: None,
                        use_lead_agent: plan.use_lead_agent || promoted,
                        auto_promotion_reason: Some("dag_validation_failed".into()),
                        ..plan
                    });
                }
                return Ok(plan);
            }
            Err(PlanError::MalformedResponse(msg)) => {
                tracing::warn!(
                    "Hierarchical plan attempt {}/{} returned malformed response: {msg}",
                    attempt + 1,
                    limits.max_retries + 1,
                );
                last_error_msg = Some(msg.clone());
                last_error = PlanError::MalformedResponse(msg);
                // Fail fast: if response has no JSON structure at all
                if !response_content.contains('{') {
                    tracing::warn!(
                        "Response contains no JSON structure, skipping remaining retries"
                    );
                    break;
                }
            }
            Err(e) => return Err(e),
        }
    }

    Err(last_error)
}

// ── Lightweight classification ──────────────────────────────────────

impl TaskPlanner {
    /// Lightweight LLM classification: uses a minimal prompt (~200 tokens) to determine
    /// if a message is simple_query or complex_task.
    pub async fn classify_lightweight(
        router: &LlmRouter,
        triage_model: Option<&str>,
        user_message: &str,
        timeout_secs: u64,
    ) -> Result<String, PlanError> {
        let messages = vec![
            ChatMessage::system(
                "Classify the user message. Respond ONLY with a JSON object:\n\
                 {\"classification\": \"simple_query\"} or {\"classification\": \"complex_task\"}\n\
                 simple_query = greetings, questions, conversation.\n\
                 complex_task = multi-step tasks needing agent work.",
            ),
            ChatMessage::user(user_message),
        ];
        let request = RouterRequest {
            model: triage_model.map(|s| s.to_string()),
            messages: Arc::new(messages),
            tools: Arc::new(vec![]),
            temperature: Some(0.0),
            max_tokens: Some(50),
            context: RequestContext::default(),
            tool_choice: None,
            tools_token_estimate: None,
            enable_caching: false,
            thinking: None,
        };
        let deadline = Duration::from_secs(timeout_secs);
        let response = tokio::time::timeout(deadline, router.complete(request))
            .await
            .map_err(|_| PlanError::Timeout(timeout_secs))?
            .map_err(|e| PlanError::LlmError(e.to_string()))?;

        let json_str = extract_json_block(&response.content);
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str)
            && let Some(c) = val.get("classification").and_then(|v| v.as_str())
        {
            return Ok(c.to_string());
        }
        Err(PlanError::MalformedResponse(
            "lightweight classification failed".into(),
        ))
    }
}
