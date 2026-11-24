//! A set over a template (AGENT-SETS, item 22 of the owner's second
//! answers of 2026-11-14): what `PUT /v1/agents/{id}/config` (`openalpaca
//! agents set`) stores for a template's id, and the one builder that puts
//! it over the template for a run, the idle instance, `GET …/config` and
//! the `agent` row alike.
//!
//! A set is kept as the fields it changes over the template's own config
//! (`AgentConfigFile`, as JSON), not as a copy of the whole: a later edit
//! of the template reaches every field the set leaves alone, and a field
//! set back to the template's value leaves the set. It is applied as a
//! JSON merge patch (RFC 7386: an object merges key by key, `null` removes
//! the key, anything else replaces it), and computed as the patch that
//! takes the template's config to the one the set put.

use super::super::config::AgentConfigFile;
use super::super::subagent::{AgentStatus, SubAgent};
use super::super::template::AgentTemplate;
use serde_json::{Map, Value};

/// The config a template's own instance shows, before any set.
fn template_config(template: &AgentTemplate) -> AgentConfigFile {
    let id = &template.frontmatter.id;
    AgentConfigFile::from_subagent(&template.to_subagent(id, ""))
}

/// The set that takes `template`'s config to `config` — `None` when they
/// say the same. The id is the template's, whatever `config` names.
pub(crate) fn set_over(
    template: &AgentTemplate,
    mut config: AgentConfigFile,
) -> Result<Option<Value>, String> {
    config.agent.id = template.frontmatter.id.clone();
    let base = to_value(&template_config(template))?;
    let wanted = to_value(&config)?;
    Ok(diff(&base, &wanted))
}

/// `template`'s config with `set` over it.
pub(crate) fn effective_config(
    template: &AgentTemplate,
    set: Option<&Value>,
) -> Result<AgentConfigFile, String> {
    let config = template_config(template);
    let Some(set) = set else {
        return Ok(config);
    };
    let mut value = to_value(&config)?;
    merge(&mut value, set);
    serde_json::from_value(value).map_err(|e| format!("the stored set does not apply: {e}"))
}

/// An instance of `template` under `instance_id`: running `task`, or idle
/// with none, with `set` over the template. With no set it is the
/// template's own instance, built as it always was. A set that no longer
/// applies — a stored one the template's fields have outgrown — is logged
/// and the template runs as it is.
pub(crate) fn build_instance(
    template: &AgentTemplate,
    instance_id: &str,
    task: Option<&str>,
    set: Option<&Value>,
) -> SubAgent {
    let own = |task: Option<&str>| {
        let mut agent = template.to_subagent(instance_id, task.unwrap_or(""));
        if task.is_none() {
            agent.status = AgentStatus::Idle;
            agent.current_task = None;
        }
        agent
    };
    if set.is_none() {
        return own(task);
    }
    let config = match effective_config(template, set) {
        Ok(config) => config,
        Err(e) => {
            tracing::warn!(
                "Agent {}: {e}; its runs use the template as it is",
                template.frontmatter.id
            );
            return own(task);
        }
    };
    let mut agent = config.into_subagent();
    agent.id = instance_id.to_string();
    agent.template_id = template.frontmatter.id.clone();
    // A config names its capabilities, not their categories: a capability
    // the template's own instance has keeps its category and proficiency
    // (an injected one is `infrastructure`, which the prompt's fingerprint
    // hashes), so a set changes a run only in the fields it sets.
    let own = own(task);
    for capability in &mut agent.capabilities {
        if let Some(theirs) = own.capabilities.iter().find(|c| c.name == capability.name) {
            capability.category = theirs.category.clone();
            capability.proficiency = theirs.proficiency;
        }
    }
    match task {
        Some(task) => {
            agent.status = AgentStatus::Busy {
                task_id: task.to_string(),
            };
            agent.current_task = Some(task.to_string());
        }
        None => {
            agent.status = AgentStatus::Idle;
            agent.current_task = None;
        }
    }
    agent
}

fn to_value(config: &AgentConfigFile) -> Result<Value, String> {
    serde_json::to_value(config).map_err(|e| format!("Failed to serialize config: {e}"))
}

/// The merge patch that takes `base` to `wanted`, `None` when equal. A key
/// `wanted` lacks, or holds `null`, where `base` has a value is `null` in
/// the patch; an object is compared key by key, anything else whole.
fn diff(base: &Value, wanted: &Value) -> Option<Value> {
    match (base, wanted) {
        (Value::Object(base), Value::Object(wanted)) => {
            let mut patch = Map::new();
            for (key, value) in wanted {
                match base.get(key) {
                    Some(old) => {
                        if let Some(changed) = diff(old, value) {
                            patch.insert(key.clone(), changed);
                        }
                    }
                    None if !value.is_null() => {
                        patch.insert(key.clone(), value.clone());
                    }
                    None => {}
                }
            }
            for (key, old) in base {
                if !wanted.contains_key(key) && !old.is_null() {
                    patch.insert(key.clone(), Value::Null);
                }
            }
            (!patch.is_empty()).then_some(Value::Object(patch))
        }
        _ if base == wanted => None,
        _ => Some(wanted.clone()),
    }
}

/// RFC 7386's merge of `patch` into `target`.
fn merge(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    if let Value::Object(target) = target {
        for (key, value) in patch {
            if value.is_null() {
                target.remove(key);
            } else {
                merge(target.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_patch_of_two_configs_merges_back_into_the_second() {
        let base = json!({"a": 1, "b": {"c": 2, "d": null, "e": [1]}, "f": {"g": 1}, "h": null});
        let wanted = json!({"a": 1, "b": {"c": 3, "d": "x", "e": [2]}, "f": null, "h": {"i": 1}});
        let patch = diff(&base, &wanted).unwrap();
        assert_eq!(
            patch,
            json!({"b": {"c": 3, "d": "x", "e": [2]}, "f": null, "h": {"i": 1}})
        );
        let mut merged = base.clone();
        merge(&mut merged, &patch);
        // `f: null` removes the key, which a config reads as `None`.
        assert_eq!(
            merged,
            json!({"a": 1, "b": {"c": 3, "d": "x", "e": [2]}, "h": {"i": 1}})
        );
        assert_eq!(diff(&base, &base), None);
    }

    /// A set changes a run in the fields it sets and nowhere else: with a
    /// model set, every bundled template's run has the capabilities of its
    /// own instance — names, categories and proficiencies, which the
    /// prompt's fingerprint hashes — and the rest of its definition.
    #[test]
    fn a_set_changes_a_run_only_in_the_fields_it_sets() {
        for template in crate::bundled::bundled_templates() {
            let id = template.frontmatter.id.clone();
            let mut config = template_config(&template);
            config
                .llm
                .get_or_insert(crate::agent::config::AgentLlmConfigFile {
                    model: None,
                    fallback_models: None,
                    overrides: None,
                })
                .model = Some("model-set".to_string());
            let set = set_over(&template, config).unwrap();
            assert!(set.is_some(), "{id}");

            let mut run = build_instance(&template, &id, Some("t1"), set.as_ref());
            let own = template.to_subagent(&id, "t1");
            assert_eq!(run.llm_config.model.as_deref(), Some("model-set"), "{id}");
            run.llm_config.model = own.llm_config.model.clone();
            assert_eq!(
                serde_json::to_value(&run.capabilities).unwrap(),
                serde_json::to_value(&own.capabilities).unwrap(),
                "{id}"
            );
            assert_eq!(format!("{run:?}"), format!("{own:?}"), "{id}");
        }
    }
}
