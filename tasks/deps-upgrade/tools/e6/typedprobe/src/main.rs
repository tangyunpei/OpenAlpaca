//! typedprobe — E6's typed probes. Each input goes through the tree's own
//! loader, compiled from this checkout's source, and one line per input says
//! whether it loaded: `<index>\tok|err`. Nothing else is printed — no value, no
//! key, no path, no error text — and nothing is written.
//!
//!   typedprobe <kind> <file>...
//!
//! kind        loader                                                           input
//! daemon      `load_daemon_config` (daemon_config/mod.rs:135)                  a daemon.toml
//! llm-edit    `read_config_with_text` → `render_config_preserving` (M3)        an llm.toml
//! mcp         `McpConfig::load` (tools/mcp/config.rs:179)                      an mcp.toml
//! agent-toml  `toml::from_str::<AgentConfigFile>` (services/agents.rs:98)      a legacy agents/*.toml
//! plugin      `PluginManifest::from_dir_for_load` (manifest.rs:137)            a plugin.toml (its directory is probed)
//! skill       `parse_skill_markdown` (middleware/skill/mod.rs:60)              a SKILL.md
//! agent-md    `parse_agent_markdown` (agent/template/mod.rs:384)               an agents/*.md
//! soul|user|identity|bootstrap  `parse_<kind>_markdown`                        the persona document of that name
//!
//! `ok` for `daemon` means more than "no error": the real loader's result is
//! the typed parse of the file (not the defaults it falls back to), its
//! `orchestrator.memory.carry_over_turns` is present and equals the file's own
//! value when the file sets one in the accepted 0..=20 range, and no key named
//! `episode_max_per_day` or `compaction_model` exists anywhere in the typed
//! config — the schema changes plan-rev3 §2.1 names. `ok` for `llm-edit`
//! means the file reads, an unchanged edit hands its bytes back untouched,
//! `render_config` serialises it, and one M3 toggle (a provider's `enabled`)
//! edits to a text that re-parses to exactly the toggled config.

use std::path::Path;

use openalpaca_core::agent::AgentConfigFile;
use openalpaca_core::agent::template::parse_agent_markdown;
use openalpaca_core::daemon_config::{DaemonConfig, load_daemon_config};
use openalpaca_core::middleware::bootstrap::parse_bootstrap_markdown;
use openalpaca_core::middleware::identity::parse_identity_markdown;
use openalpaca_core::middleware::skill::parse_skill_markdown;
use openalpaca_core::middleware::soul::parse_soul_markdown;
use openalpaca_core::middleware::user::parse_user_markdown;
use openalpaca_core::tools::mcp::config::McpConfig;
use openalpaca_llm::LlmRouterConfig;
use openalpaca_llm::config::llm_config::{read_config, render_config, render_config_preserving};
use openalpaca_plugins::PluginManifest;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((kind, files)) = args.split_first() else {
        usage();
    };
    if files.is_empty() {
        usage();
    }
    let probe: fn(&Path) -> Option<()> = match kind.as_str() {
        "daemon" => probe_daemon,
        "llm-edit" => probe_llm_edit,
        "mcp" => |p| McpConfig::load(p).ok().map(drop),
        "agent-toml" => |p| toml::from_str::<AgentConfigFile>(&read(p)?).ok().map(drop),
        "plugin" => |p| {
            PluginManifest::from_dir_for_load(p.parent()?)
                .ok()
                .map(drop)
        },
        "skill" => |p| parse_skill_markdown(&read(p)?).ok().map(drop),
        "agent-md" => |p| parse_agent_markdown(&read(p)?).ok().map(drop),
        "soul" => |p| parse_soul_markdown(&read(p)?).ok().map(drop),
        "user" => |p| parse_user_markdown(&read(p)?).ok().map(drop),
        "identity" => |p| parse_identity_markdown(&read(p)?).ok().map(drop),
        "bootstrap" => |p| parse_bootstrap_markdown(&read(p)?).ok().map(drop),
        _ => usage(),
    };
    for (i, file) in files.iter().enumerate() {
        let word = if probe(Path::new(file)).is_some() {
            "ok"
        } else {
            "err"
        };
        println!("{}\t{word}", i + 1);
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: typedprobe <daemon|llm-edit|mcp|agent-toml|plugin|skill|agent-md|soul|user|identity|bootstrap> <file>..."
    );
    std::process::exit(2);
}

fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn probe_daemon(path: &Path) -> Option<()> {
    let content = read(path)?;
    // The loader's own parse, `daemon_config/mod.rs:140-144`, without the two
    // warn probes it runs on the raw document first.
    let mut typed: DaemonConfig = toml::from_str::<toml::Value>(&content)
        .and_then(|value| value.try_into::<DaemonConfig>())
        .ok()?;
    typed.validate();
    // The real loader returns the defaults on any failure (`:148-157`); it
    // counts as `ok` only when what it returns is this file's typed parse.
    let loaded = load_daemon_config(path);
    let loaded = serde_json::to_value(&loaded).ok()?;
    if loaded != serde_json::to_value(&typed).ok()? {
        return None;
    }
    // The schema this wave's typed probes exist for (plan-rev3 §2.1): the key
    // the continuity design added is present; the two keys that were removed
    // exist nowhere in the type.
    let carry_over_turns = loaded
        .pointer("/orchestrator/memory/carry_over_turns")?
        .as_u64()?;
    if has_key(&loaded, "episode_max_per_day") || has_key(&loaded, "compaction_model") {
        return None;
    }
    // A value the file sets, within the range `validate()` accepts, is the one
    // that was loaded.
    let raw: toml::Value = toml::from_str(&content).ok()?;
    if let Some(set) = raw
        .get("orchestrator")
        .and_then(|o| o.get("memory"))
        .and_then(|m| m.get("carry_over_turns"))
        .and_then(toml::Value::as_integer)
        && (0..=20).contains(&set)
        && carry_over_turns != set as u64
    {
        return None;
    }
    Some(())
}

fn has_key(value: &serde_json::Value, name: &str) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.contains_key(name) || map.values().any(|v| has_key(v, name))
        }
        serde_json::Value::Array(items) => items.iter().any(|v| has_key(v, name)),
        _ => false,
    }
}

fn probe_llm_edit(path: &Path) -> Option<()> {
    // `read_config_with_text` is `read_config` plus the bytes it read
    // (`llm_config/mod.rs:85-98`); the bytes are read here the same way.
    let existing = read(path)?;
    let before: LlmRouterConfig = read_config(path).ok()?;
    // Nothing changed: the bytes come back untouched (`edit.rs:38-41`).
    if render_config_preserving(&existing, &before, &before).ok()? != existing {
        return None;
    }
    render_config(&before).ok()?;
    // The M3 edit the GUI's provider toggle makes: one `enabled` flips.
    let mut after = before.clone();
    let providers = after.providers.get_or_insert_with(Default::default);
    let name = providers
        .keys()
        .min()
        .cloned()
        .unwrap_or_else(|| "ollama".to_string());
    let provider = providers.entry(name).or_default();
    provider.enabled = Some(!provider.enabled.unwrap_or(false));
    let rendered = render_config_preserving(&existing, &before, &after).ok()?;
    let reparsed: LlmRouterConfig = toml::from_str(&rendered).ok()?;
    (toml::Value::try_from(&reparsed).ok()? == toml::Value::try_from(&after).ok()?).then_some(())
}
