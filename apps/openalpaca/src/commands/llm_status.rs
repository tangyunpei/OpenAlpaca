//! Status, usage, models, credentials, backends, and strategy handlers for `llm` subcommand.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, format_usd, print_list, status_color};

use super::llm::truncate;

// ── Local deserialization structs ────────────────────────────────

/// A row of `GET /v1/models`, named exactly as the route names it.
///
/// It used to read `input_price_per_1m` / `output_price_per_1m`, which the
/// daemon has never sent (`input_price_per_million`), so every price printed
/// `-` and `--format json` echoed two nulls under names that do not exist. The
/// old spellings stay as aliases; a **free** local model is precisely the case
/// where `$0.00` and `-` must not be confused.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ModelEntry {
    #[serde(alias = "model_id")]
    id: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    context_window: Option<i64>,
    #[serde(alias = "input_price_per_1m", default)]
    input_price_per_million: Option<f64>,
    #[serde(alias = "output_price_per_1m", default)]
    output_price_per_million: Option<f64>,
    /// Whether the model can be given tools. A local install is where this
    /// matters: an agent run needs them, and `ollama pull` will happily fetch a
    /// model that has none.
    #[serde(default)]
    supports_tools: Option<bool>,
}

/// `GET /v1/settings/llm` — read here for one field per provider.
#[derive(Debug, Deserialize, Default)]
pub(super) struct LlmSettingsSnapshot {
    #[serde(default)]
    providers: BTreeMap<String, SettingsProvider>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SettingsProvider {
    /// Whether this provider needs an API key at all (L1). A local one does
    /// not, and an empty key list there means "no key needed", never "broken".
    #[serde(default = "requires_key_default")]
    requires_key: bool,
}

/// A daemon too old to send the field had only keyed providers, so the
/// conservative default is the one that describes it.
fn requires_key_default() -> bool {
    true
}

impl LlmSettingsSnapshot {
    /// `provider → does it need an API key at all` (L1).
    ///
    /// The one fact two commands need off this route: `llm status` paints key
    /// health with it, and `llm keys validate` refuses to post a secret to a
    /// provider that has no use for one.
    pub(super) fn requires_key_map(&self) -> BTreeMap<String, bool> {
        self.providers
            .iter()
            .map(|(name, info)| (name.clone(), info.requires_key))
            .collect()
    }
}

/// `GET /v1/status`'s `llm` block (L3) — absent on a daemon that has no router,
/// and on one built before the field existed.
#[derive(Debug, Deserialize)]
pub(super) struct LlmStatusInfo {
    #[serde(default)]
    pub default_model_routable: bool,
    #[serde(default)]
    pub effective_default_model: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct DaemonStatusSnapshot {
    #[serde(default)]
    llm: Option<LlmStatusInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct UsageEntry {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    input_tokens: Option<i64>,
    #[serde(default)]
    output_tokens: Option<i64>,
    #[serde(default)]
    cost_usd: Option<f64>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DailyUsageEntry {
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    total_requests: Option<i64>,
    #[serde(default)]
    total_input_tokens: Option<i64>,
    #[serde(default)]
    total_output_tokens: Option<i64>,
    #[serde(default)]
    total_cost_usd: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct CredentialEntry {
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    expires_at: Option<i64>,
    #[serde(default)]
    auto_refresh: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct BackendEntry {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    available: Option<bool>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ProviderUsageEntry {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    total_cost_usd: Option<f64>,
    #[serde(default)]
    total_tokens: Option<i64>,
    #[serde(default)]
    total_requests: Option<i64>,
    #[serde(default)]
    health: Option<String>,
}

// ── TableRow impls ──────────────────────────────────────────────

impl TableRow for ModelEntry {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("PROVIDER", 12),
            ("MODEL", 35),
            ("CONTEXT", 10),
            ("TOOLS", 6),
            ("INPUT $/1M", 12),
            ("OUTPUT $/1M", 12),
        ]
    }

    fn table_row(&self) -> String {
        let provider = self.provider.as_deref().unwrap_or("-");
        let context = self
            .context_window
            .map(|c| format!("{}K", c / 1000))
            .unwrap_or_else(|| "-".to_string());
        // Absent is "-", not "no": a daemon too old to send the flag has not
        // said the model cannot be given tools.
        let tools = match self.supports_tools {
            Some(true) => "yes",
            Some(false) => "no",
            None => "-",
        };
        let price = |value: Option<f64>| {
            value
                .map(|p| format!("${p:.2}"))
                .unwrap_or_else(|| "-".to_string())
        };

        format!(
            "{:<12} {:<35} {:<10} {:<6} {:<12} {:<12}",
            provider,
            truncate(&self.id, 33),
            context,
            tools,
            price(self.input_price_per_million),
            price(self.output_price_per_million),
        )
    }
}

impl TableRow for UsageEntry {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("TIME", 20),
            ("AGENT", 15),
            ("MODEL", 25),
            ("TOKENS", 15),
            ("COST", 10),
            ("STATUS", 10),
        ]
    }

    fn table_row(&self) -> String {
        let time = self
            .timestamp
            .as_deref()
            .map(|t| t.chars().take(19).collect::<String>())
            .unwrap_or_else(|| "-".to_string());
        let agent = self.agent_id.as_deref().unwrap_or("-");
        let model = self.model.as_deref().unwrap_or("-");
        let tokens = format!(
            "{}/{}",
            self.input_tokens.unwrap_or(0),
            self.output_tokens.unwrap_or(0)
        );
        let cost = format_usd(self.cost_usd.unwrap_or(0.0));
        let status = self.status.as_deref().unwrap_or("unknown");

        format!(
            "{:<20} {:<15} {:<25} {:<15} {:<10} {:<10}",
            time,
            truncate(agent, 13),
            truncate(model, 23),
            tokens,
            cost,
            status_color(status),
        )
    }
}

impl TableRow for DailyUsageEntry {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("DATE", 12),
            ("AGENT", 15),
            ("MODEL", 25),
            ("REQUESTS", 10),
            ("TOKENS", 18),
            ("COST", 10),
        ]
    }

    fn table_row(&self) -> String {
        let date = self.date.as_deref().unwrap_or("-");
        let agent = self.agent_id.as_deref().unwrap_or("-");
        let model = self.model.as_deref().unwrap_or("-");
        let requests = self.total_requests.unwrap_or(0);
        let tokens = format!(
            "{}/{}",
            self.total_input_tokens.unwrap_or(0),
            self.total_output_tokens.unwrap_or(0)
        );
        let cost = format_usd(self.total_cost_usd.unwrap_or(0.0));

        format!(
            "{:<12} {:<15} {:<25} {:<10} {:<18} {:<10}",
            date,
            truncate(agent, 13),
            truncate(model, 23),
            requests,
            tokens,
            cost,
        )
    }
}

impl TableRow for CredentialEntry {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("SOURCE", 14),
            ("PROVIDER", 12),
            ("STATUS", 12),
            ("EXPIRES", 22),
            ("REFRESH", 8),
        ]
    }

    fn table_row(&self) -> String {
        let source = self.source.as_deref().unwrap_or("-");
        let provider = self.provider.as_deref().unwrap_or("-");
        let status = self.status.as_deref().unwrap_or("unknown");
        let expires = self
            .expires_at
            .map(|ts| {
                chrono::DateTime::from_timestamp(ts, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| format!("{}", ts))
            })
            .unwrap_or_else(|| "-".to_string());
        let refresh = if self.auto_refresh.unwrap_or(false) {
            "yes"
        } else {
            "no"
        };

        format!(
            "{:<14} {:<12} {:<12} {:<22} {:<8}",
            source,
            provider,
            status_color(status),
            expires,
            refresh,
        )
    }
}

impl TableRow for BackendEntry {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("NAME", 14),
            ("AVAILABLE", 10),
            ("PATH", 35),
            ("ENABLED", 8),
        ]
    }

    fn table_row(&self) -> String {
        let name = self.name.as_deref().unwrap_or("-");
        let available = if self.available.unwrap_or(false) {
            "yes".green().to_string()
        } else {
            "no".dimmed().to_string()
        };
        let path = self.path.as_deref().unwrap_or("-");
        let enabled = if self.enabled.unwrap_or(false) {
            "yes"
        } else {
            "no"
        };

        format!(
            "{:<14} {:<10} {:<35} {:<8}",
            name,
            available,
            truncate(path, 33),
            enabled,
        )
    }
}

impl TableRow for ProviderUsageEntry {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("PROVIDER", 14),
            ("COST", 12),
            ("TOKENS", 12),
            ("REQUESTS", 10),
            ("HEALTH", 10),
        ]
    }

    fn table_row(&self) -> String {
        let provider = self.provider.as_deref().unwrap_or("-");
        let cost = format_usd(self.total_cost_usd.unwrap_or(0.0));
        let tokens = self.total_tokens.unwrap_or(0);
        let requests = self.total_requests.unwrap_or(0);
        let health = self.health.as_deref().unwrap_or("unknown");

        format!(
            "{:<14} {:<12} {:<12} {:<10} {:<10}",
            provider,
            cost,
            tokens,
            requests,
            status_color(health),
        )
    }
}

// ── Rendering (pure, so the honesty rules are testable) ─────────

/// What the `Model:` line says, given the daemon's own verdict on it (L3).
///
/// `[orchestrator] model` is allowed to name a model this install cannot serve
/// — every shipped agent template pins a Claude id, and on an Ollama-only
/// machine the fallback ladder answers with something else. Printing only the
/// configured id there is a lie by omission: the owner reads a Claude name and
/// gets a local reply.
pub(super) fn model_line(configured: &str, llm: Option<&LlmStatusInfo>) -> String {
    match llm {
        // A daemon that does not say (no router, or one built before the
        // field) gets the benefit of the doubt rather than an invented claim.
        None => configured.to_string(),
        Some(status) if status.default_model_routable => configured.to_string(),
        Some(status) => match status.effective_default_model.as_deref() {
            Some(effective) => format!("{configured} — not available, using {effective}"),
            // The fix is a verb, not a hint: `config set` is the CLI's own way
            // to turn a provider on (the `ai.*` keys write `llm.toml` through
            // the config schema, and the daemon picks the edit up live), and
            // saying "enable a provider" without naming it sent an owner
            // hunting for a verb under `llm` that is not there.
            None => format!(
                "{configured} — not available, and no model is. Turn one on: \
                 `openalpaca config set ai.ollama.enabled true` (a local Ollama needs \
                 no key — `ollama pull <model>` first), or Settings → Models in the GUI. \
                 `openalpaca llm models --refresh` re-reads the catalogue."
            ),
        },
    }
}

/// The `Key Health:` block, one line per key — or per provider when it has
/// none to speak of (L12).
///
/// A keyless provider used to render `✗ ollama`, because the block read a
/// `{provider: [KeyStatus]}` map as if each value were a key object and
/// `is_valid` was missing from an *array*. So the one provider that is working
/// exactly as designed was the one marked broken, and its key id printed blank.
/// `requires_key` (L1) is what tells the two apart.
pub(super) fn key_health_lines(
    key_health: &serde_json::Value,
    requires_key: &BTreeMap<String, bool>,
) -> Vec<String> {
    let mut providers: BTreeSet<&str> = requires_key.keys().map(String::as_str).collect();
    if let Some(map) = key_health.as_object() {
        providers.extend(map.keys().map(String::as_str));
    }

    let mut lines = Vec::new();
    for provider in providers {
        // Absent from `GET /v1/settings/llm` means the daemon did not describe
        // it; assume it is keyed, which is what every cloud provider is.
        if requires_key.get(provider) == Some(&false) {
            lines.push(format!("  {} {} — no key needed", "·".dimmed(), provider));
            continue;
        }

        let statuses = key_health
            .get(provider)
            .and_then(|v| v.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default();
        if statuses.is_empty() {
            lines.push(format!("  {} {} — no key configured", "✗".red(), provider));
            continue;
        }
        for status in statuses {
            let id = status["id"].as_str().unwrap_or("-");
            let health = status["health"].as_str().unwrap_or("unknown");
            let available = status["is_available"].as_bool().unwrap_or(false);
            let mark = if health == "healthy" && available {
                "✓".green()
            } else {
                "✗".red()
            };
            lines.push(format!("  {mark} {provider} / {id} ({health})"));
        }
    }
    lines
}

// ── Handler functions ───────────────────────────────────────────

pub(super) async fn llm_status(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;

    let key_health: serde_json::Value = client.get("/v1/settings/llm/status").await?;
    let orch_config: serde_json::Value = client.get("/v1/orchestrator/config").await?;
    let settings: LlmSettingsSnapshot = client.get("/v1/settings/llm").await.unwrap_or_default();
    // `/v1/status` is the one surface carrying the effective model (L3). A
    // daemon that refuses it — or is too old for the block — leaves the
    // configured id standing on its own, which is what this printed before.
    let daemon_status: DaemonStatusSnapshot = client.get("/v1/status").await.unwrap_or_default();
    let requires_key: BTreeMap<String, bool> = settings.requires_key_map();

    match format {
        OutputFormat::Json => {
            let combined = serde_json::json!({
                "key_health": key_health,
                "orchestrator": orch_config,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&combined).unwrap_or_default()
            );
        }
        OutputFormat::Table => {
            println!("{}", "LLM Status".bold());
            println!();
            println!(
                "  {} {}",
                "Model:".dimmed(),
                model_line(
                    orch_config["model"].as_str().unwrap_or("-"),
                    daemon_status.llm.as_ref(),
                )
            );

            if let Some(fallbacks) = orch_config["fallback_models"].as_array()
                && !fallbacks.is_empty()
            {
                let names: Vec<&str> = fallbacks.iter().filter_map(|m| m.as_str()).collect();
                println!("  {} {}", "Fallbacks:".dimmed(), names.join(", "));
            }

            println!(
                "  {} {}",
                "Active agents:".dimmed(),
                orch_config["active_agents"].as_i64().unwrap_or(0)
            );
            println!(
                "  {} {}",
                "Active tasks:".dimmed(),
                orch_config["active_tasks"].as_i64().unwrap_or(0)
            );
            println!(
                "  {} {}",
                "Daily cost:".dimmed(),
                format_usd(orch_config["daily_cost_usd"].as_f64().unwrap_or(0.0))
            );
            println!();

            println!("{}", "Key Health:".dimmed());
            for line in key_health_lines(&key_health, &requires_key) {
                println!("{line}");
            }
        }
    }
    Ok(())
}

pub(super) async fn llm_usage(
    agent: Option<String>,
    date: Option<String>,
    key: Option<String>,
    daily: bool,
    format: OutputFormat,
) -> Result<()> {
    let client = DaemonClient::connect()?;

    if daily {
        let mut path = "/v1/llm/usage/daily?limit=30".to_string();
        if let Some(ref a) = agent {
            path.push_str(&format!("&agent_id={}", a));
        }
        if let Some(ref d) = date {
            path.push_str(&format!("&date={}", d));
        }
        let entries: Vec<DailyUsageEntry> = client.get(&path).await?;
        print_list(&entries, format);
    } else {
        let mut path = "/v1/llm/usage?limit=50".to_string();
        if let Some(ref a) = agent {
            path.push_str(&format!("&agent_id={}", a));
        }
        if let Some(ref k) = key {
            path.push_str(&format!("&key_id={}", k));
        }
        let entries: Vec<UsageEntry> = client.get(&path).await?;
        print_list(&entries, format);
    }
    Ok(())
}

/// `openalpaca llm models [--refresh]` — the catalogue the router can route to.
///
/// `--refresh` is `POST /v1/models/refresh`: every loaded provider is asked
/// what it can serve, keyless ones included (L1/L2), so a model pulled with
/// `ollama pull` appears without restarting the daemon or writing a line of
/// `llm.toml`. Without it the catalogue is read as it stands.
pub(super) async fn llm_models(refresh: bool, format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let models: Vec<ModelEntry> = if refresh {
        client
            .post("/v1/models/refresh", &serde_json::json!({}))
            .await?
    } else {
        client.get("/v1/models").await?
    };

    // An empty catalogue is a real state with a real cause — every provider is
    // off, or the one that is on could not be asked — and `No items found.` on
    // its own sends nobody anywhere.
    if models.is_empty() && matches!(format, OutputFormat::Table) {
        println!("{}", "No models are registered on this daemon.".dimmed());
        println!(
            "{}",
            "Enable a provider (`openalpaca llm keys add`, or Settings → Models). \
             For a local one, `ollama pull <model>` then `openalpaca llm models --refresh` \
             — it needs no key."
                .dimmed()
        );
        return Ok(());
    }

    print_list(&models, format);
    Ok(())
}

pub(super) async fn llm_credentials(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let creds: Vec<CredentialEntry> = client.get("/v1/settings/llm/credentials").await?;

    if creds.is_empty() {
        println!(
            "{}",
            "No discovered credentials. Install Claude Code or Codex CLI to auto-detect.".dimmed()
        );
        return Ok(());
    }

    print_list(&creds, format);
    Ok(())
}

pub(super) async fn llm_backends(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let backends: Vec<BackendEntry> = client.get("/v1/settings/llm/cli-backends").await?;

    if backends.is_empty() {
        println!("{}", "No CLI backends configured.".dimmed());
        return Ok(());
    }

    print_list(&backends, format);
    Ok(())
}

pub(super) async fn llm_provider_usage(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect()?;
    let usage: Vec<ProviderUsageEntry> = client.get("/v1/settings/llm/providers/usage").await?;

    if usage.is_empty() {
        println!("{}", "No provider usage data available.".dimmed());
        return Ok(());
    }

    print_list(&usage, format);
    Ok(())
}

pub(super) async fn llm_strategy(provider: &str, strategy: &str) -> Result<()> {
    let client = DaemonClient::connect()?;

    // Fetch current orchestrator config
    let config: serde_json::Value = client.get("/v1/orchestrator/config").await?;

    // Build update with strategy field
    let mut update = serde_json::Map::new();
    if let Some(model) = config["model"].as_str() {
        update.insert(
            "model".to_string(),
            serde_json::Value::String(model.to_string()),
        );
    }
    if let Some(fallbacks) = config.get("fallback_models") {
        update.insert("fallback_models".to_string(), fallbacks.clone());
    }
    update.insert(
        "strategy".to_string(),
        serde_json::json!({
            "provider": provider,
            "type": strategy,
        }),
    );

    let _: serde_json::Value = client
        .put(
            "/v1/orchestrator/config",
            &serde_json::Value::Object(update),
        )
        .await?;
    println!(
        "{} Strategy set to '{}' for provider '{}'",
        "✓".green(),
        strategy,
        provider
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() {
        colored::control::set_override(false);
    }

    fn requires(pairs: &[(&str, bool)]) -> BTreeMap<String, bool> {
        pairs
            .iter()
            .map(|(name, needs)| ((*name).to_string(), *needs))
            .collect()
    }

    /// L12: the one provider designed to hold no key was the one `llm status`
    /// marked broken. `key_health` is `{provider: [KeyStatus]}`; the block read
    /// each value as a key object and asked it for `is_valid`, which an array
    /// does not have, so every provider rendered `✗` with a blank id.
    #[test]
    fn a_keyless_provider_needs_no_key_rather_than_failing() {
        plain();
        let health = serde_json::json!({ "ollama": [] });

        let lines = key_health_lines(&health, &requires(&[("ollama", false)]));

        assert_eq!(lines, vec!["  · ollama — no key needed"]);
        assert!(!lines[0].contains('✗'), "{lines:?}");
    }

    /// The same empty list on a provider that *does* need a key is a real
    /// problem and still reads as one — the fix must discriminate, not excuse.
    #[test]
    fn a_keyed_provider_with_no_key_still_says_so() {
        plain();
        let health = serde_json::json!({ "anthropic": [] });

        let lines = key_health_lines(&health, &requires(&[("anthropic", true)]));

        assert_eq!(lines, vec!["  ✗ anthropic — no key configured"]);
    }

    #[test]
    fn a_real_key_reports_its_id_and_its_health() {
        plain();
        let health = serde_json::json!({
            "anthropic": [
                { "id": "key_ab12", "health": "healthy", "consecutive_rate_limits": 0, "is_available": true },
                { "id": "key_cd34", "health": "rate_limited", "consecutive_rate_limits": 3, "is_available": false },
            ],
            "ollama": [],
        });

        let lines = key_health_lines(
            &health,
            &requires(&[("anthropic", true), ("ollama", false)]),
        );

        assert_eq!(
            lines,
            vec![
                "  ✓ anthropic / key_ab12 (healthy)",
                "  ✗ anthropic / key_cd34 (rate_limited)",
                "  · ollama — no key needed",
            ]
        );
        assert!(
            !lines.iter().any(|line| line.contains("/ -")),
            "no blank key id anywhere: {lines:?}"
        );
    }

    /// A daemon that does not describe its providers gets the conservative
    /// reading, not an invented "no key needed".
    #[test]
    fn a_provider_the_settings_route_did_not_describe_is_treated_as_keyed() {
        plain();
        let health = serde_json::json!({ "openai": [] });

        assert_eq!(
            key_health_lines(&health, &BTreeMap::new()),
            vec!["  ✗ openai — no key configured"]
        );
    }

    /// L12: `Daily cost: $-0.0000`. Nothing was refunded — IEEE negative zero,
    /// which a column of free local calls lands on, carries its sign into
    /// `{:.4}`.
    #[test]
    fn a_free_day_costs_zero_not_minus_zero() {
        assert_eq!(crate::output::format_usd(-0.0), "$0.0000");
        assert_eq!(crate::output::format_usd(0.0), "$0.0000");
        assert_eq!(crate::output::format_usd(0.0301), "$0.0301");
        // A genuinely negative figure is not hidden.
        assert_eq!(crate::output::format_usd(-0.5), "$-0.5000");
    }

    /// L3: `[orchestrator] model` may name a model this install cannot serve —
    /// every shipped template pins a Claude id. Printing it alone tells the
    /// owner a Claude answered.
    #[test]
    fn the_model_line_names_what_would_really_answer() {
        let unroutable = LlmStatusInfo {
            default_model_routable: false,
            effective_default_model: Some("qwen3:8b".to_string()),
        };
        assert_eq!(
            model_line("claude-haiku-4-5", Some(&unroutable)),
            "claude-haiku-4-5 — not available, using qwen3:8b"
        );

        let routable = LlmStatusInfo {
            default_model_routable: true,
            effective_default_model: Some("claude-haiku-4-5".to_string()),
        };
        assert_eq!(
            model_line("claude-haiku-4-5", Some(&routable)),
            "claude-haiku-4-5",
            "a configured model that works is reported plainly"
        );
    }

    #[test]
    fn nothing_routable_names_the_fix_rather_than_a_substitute() {
        let nothing = LlmStatusInfo {
            default_model_routable: false,
            effective_default_model: None,
        };
        let line = model_line("claude-haiku-4-5", Some(&nothing));
        assert!(line.contains("no model is"), "{line}");
        assert!(line.contains("ollama pull"), "{line}");
        assert!(line.contains("Settings → Models"), "{line}");
        // M7: the CLI's own way to turn a provider on is a verb that exists,
        // and this is where an owner with no routable model reads it.
        assert!(
            line.contains("openalpaca config set ai.ollama.enabled true"),
            "{line}"
        );
    }

    /// A daemon with no LLM block says nothing, and neither does this.
    #[test]
    fn a_daemon_that_does_not_say_is_not_second_guessed() {
        assert_eq!(model_line("claude-haiku-4-5", None), "claude-haiku-4-5");
    }

    /// The price columns read the names the route actually sends. They read
    /// `input_price_per_1m`, which no response has carried, so a free local
    /// model printed `-` where it should print `$0.00`.
    #[test]
    fn a_model_rows_price_and_tool_support_come_off_the_route_as_sent() {
        plain();
        let row: ModelEntry = serde_json::from_value(serde_json::json!({
            "id": "qwen3:8b",
            "provider": "ollama",
            "context_window": 262144,
            "input_price_per_million": 0.0,
            "output_price_per_million": 0.0,
            "supports_tools": true,
        }))
        .expect("a `GET /v1/models` row");

        let rendered = row.table_row();
        assert!(rendered.contains("qwen3:8b"), "{rendered}");
        assert!(rendered.contains("262K"), "{rendered}");
        assert!(rendered.contains("yes"), "{rendered}");
        assert_eq!(
            rendered.matches("$0.00").count(),
            2,
            "free is $0.00, never `-`: {rendered}"
        );
    }

    /// A daemon too old for `supports_tools` has not claimed the model lacks
    /// them.
    #[test]
    fn an_absent_tool_flag_is_unknown_not_no() {
        plain();
        let row: ModelEntry = serde_json::from_value(serde_json::json!({
            "id": "m", "provider": "p", "context_window": 8192,
            "input_price_per_million": 1.0, "output_price_per_million": 2.0,
        }))
        .expect("an older row");
        let rendered = row.table_row();
        assert!(!rendered.contains("no"), "{rendered}");
        assert!(rendered.contains(" -"), "{rendered}");
    }
}
