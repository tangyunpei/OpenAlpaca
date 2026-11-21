//! Key management subcommand handlers for `llm keys`.

use anyhow::Result;
use clap::{Args, Subcommand};
use colored::Colorize;
use dialoguer::{Input, Select, theme::ColorfulTheme};
use serde::{Deserialize, Serialize};

use crate::client::DaemonClient;
use crate::output::{OutputFormat, TableRow, print_list, status_color, truncate};

#[derive(Args)]
pub struct KeysArgs {
    #[command(subcommand)]
    pub command: KeysCommands,
}

#[derive(Subcommand)]
#[command(defer = true)]
pub enum KeysCommands {
    /// List all API keys
    List {
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,
    },
    /// Add a new API key
    Add {
        /// Provider (anthropic, openai, ollama)
        #[arg(long)]
        provider: Option<String>,
        /// Secret key value
        #[arg(long)]
        secret: Option<String>,
        /// Key priority
        #[arg(long, default_value = "primary")]
        priority: String,
        /// Source of the key
        #[arg(long)]
        source: Option<String>,
        /// Notes
        #[arg(long)]
        notes: Option<String>,
    },
    /// Remove an API key
    Remove {
        /// Provider name
        provider: String,
        /// Key ID
        key_id: String,
    },
    /// Validate an API key. A provider that needs none says so instead
    Validate {
        /// Provider name
        #[arg(long)]
        provider: String,
        /// Secret key to validate. Not needed for a keyless provider
        #[arg(long)]
        secret: Option<String>,
    },
    /// Set a key as primary for its provider
    SetPrimary {
        /// Provider name
        provider: String,
        /// Key ID to make primary
        key_id: String,
    },
    /// Reorder a provider's keys
    Reorder {
        /// Provider name
        provider: String,
        /// Key IDs in desired order; keys not named keep their order after them
        key_ids: Vec<String>,
    },
}

/// One row of `openalpaca llm keys list`.
///
/// The three field names below are the daemon's (`KeyInfo`), not this CLI's
/// guesses: it used to read `key["key_id"]` where the route serializes `id`,
/// and `provider["primary_key_id"]`, which no response has ever carried — so
/// KEY_ID printed blank and PRIORITY always said `Fallback`.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct KeyEntry {
    provider: String,
    key_id: String,
    #[serde(default)]
    masked_secret: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    status: Option<String>,
    /// A provider that needs no key at all (L1) — one row saying so, rather
    /// than a provider missing from the listing entirely.
    #[serde(default)]
    keyless: bool,
}

impl TableRow for KeyEntry {
    fn headers() -> Vec<(&'static str, usize)> {
        vec![
            ("PROVIDER", 12),
            ("KEY_ID", 15),
            ("SECRET", 20),
            ("PRIORITY", 10),
            ("SOURCE", 15),
            ("STATUS", 10),
        ]
    }

    fn table_row(&self) -> String {
        if self.keyless {
            return format!(
                "{:<12} {:<15} {:<20} {:<10} {:<15} {:<10}",
                self.provider, "-", "no key needed", "-", "local", "ok"
            );
        }
        let masked = self.masked_secret.as_deref().unwrap_or("***");
        let priority = self.priority.as_deref().unwrap_or("-");
        let source = self.source.as_deref().unwrap_or("-");
        let status = self.status.as_deref().unwrap_or("unknown");

        format!(
            "{:<12} {:<15} {:<20} {:<10} {:<15} {:<10}",
            self.provider,
            truncate(&self.key_id, 13),
            masked,
            priority,
            source,
            status_color(status),
        )
    }
}

/// Flatten `GET /v1/settings/llm` into one row per key — plus one row for a
/// provider that needs none.
pub(super) fn key_rows(config: &serde_json::Value) -> Vec<KeyEntry> {
    let mut keys = Vec::new();
    let Some(providers) = config["providers"].as_object() else {
        return keys;
    };
    for (provider_name, provider_data) in providers {
        let provider_keys = provider_data["keys"].as_array().map(Vec::as_slice);
        // Absent means a daemon too old to say; every provider it knew needed
        // a key, so that is the reading which describes it.
        let requires_key = provider_data["requires_key"].as_bool().unwrap_or(true);
        if !requires_key && provider_keys.is_none_or(<[_]>::is_empty) {
            keys.push(KeyEntry {
                provider: provider_name.clone(),
                key_id: String::new(),
                masked_secret: None,
                priority: None,
                source: None,
                status: None,
                keyless: true,
            });
            continue;
        }
        for key in provider_keys.unwrap_or_default() {
            keys.push(KeyEntry {
                provider: provider_name.clone(),
                key_id: key["id"].as_str().unwrap_or("-").to_string(),
                masked_secret: key["masked_secret"].as_str().map(str::to_string),
                priority: key["priority"].as_str().map(str::to_string),
                source: key["source"].as_str().map(str::to_string),
                status: key["status"].as_str().map(str::to_string),
                keyless: false,
            });
        }
    }
    keys
}

pub(super) async fn run_keys(args: KeysArgs) -> Result<()> {
    match args.command {
        KeysCommands::List { format } => keys_list(format).await,
        KeysCommands::Add {
            provider,
            secret,
            priority,
            source,
            notes,
        } => keys_add(provider, secret, priority, source, notes).await,
        KeysCommands::Remove { provider, key_id } => keys_remove(&provider, &key_id).await,
        KeysCommands::Validate { provider, secret } => {
            keys_validate(&provider, secret.as_deref()).await
        }
        KeysCommands::SetPrimary { provider, key_id } => keys_set_primary(&provider, &key_id).await,
        KeysCommands::Reorder { provider, key_ids } => keys_reorder(&provider, key_ids).await,
    }
}

async fn keys_list(format: OutputFormat) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let config: serde_json::Value = client.get("/v1/settings/llm").await?;
    print_list(&key_rows(&config), format);
    Ok(())
}

async fn keys_add(
    provider: Option<String>,
    secret: Option<String>,
    priority: String,
    source: Option<String>,
    notes: Option<String>,
) -> Result<()> {
    let theme = ColorfulTheme::default();

    // Interactive fallback for missing provider
    let provider = match provider {
        Some(p) => p,
        None => {
            let providers = vec!["anthropic", "openai", "ollama"];
            let idx = Select::with_theme(&theme)
                .with_prompt("Provider")
                .items(&providers)
                .default(0)
                .interact()?;
            providers[idx].to_string()
        }
    };

    // Interactive fallback for missing secret
    let secret = match secret {
        Some(s) => s,
        None => {
            let s: String = dialoguer::Password::with_theme(&theme)
                .with_prompt("API key secret")
                .interact()?;
            s
        }
    };

    // Auto-validate the key
    print!("{}", "Validating key... ".dimmed());
    let client = DaemonClient::connect().await?;
    let validate_body = validate_body(&provider, &secret);
    match client
        .post::<_, serde_json::Value>("/v1/settings/llm/validate", &validate_body)
        .await
    {
        Ok(result) => {
            let valid = result["valid"].as_bool().unwrap_or(false);
            if valid {
                println!("{}", "valid".green());
            } else {
                println!("{}", "invalid".red());
                // `format_error` is the field `KeyValidationResult` carries;
                // `message` never existed, so the daemon's reason was dropped
                // and every failure read the same.
                let msg = result["format_error"]
                    .as_str()
                    .unwrap_or("Key validation failed");
                println!("  {}", msg);
            }
        }
        Err(e) => {
            println!("{} ({})", "could not validate".yellow(), e);
        }
    }

    // Interactive priority if not specified via flag
    let priority = if priority == "primary" || priority == "fallback" {
        priority
    } else {
        let options = vec!["primary", "fallback"];
        let idx = Select::with_theme(&theme)
            .with_prompt("Priority")
            .items(&options)
            .default(0)
            .interact()?;
        options[idx].to_string()
    };

    // Interactive source
    let source = match source {
        Some(s) => s,
        None => {
            let sources = vec![
                "API Console",
                "Claude Code",
                "Codex",
                "Environment",
                "Other",
            ];
            let idx = Select::with_theme(&theme)
                .with_prompt("Source")
                .items(&sources)
                .default(0)
                .interact()?;
            sources[idx].to_string()
        }
    };

    // Notes
    let notes = match notes {
        Some(n) => n,
        None => {
            let n: String = Input::with_theme(&theme)
                .with_prompt("Notes (optional)")
                .allow_empty(true)
                .interact_text()?;
            n
        }
    };

    // Submit. The id field is `id` — `key_id` was silently dropped by serde and
    // the daemon minted a `key_<uuid8>` instead, so the id printed back by
    // `keys list` was never the one asked for.
    let id = format!("{}_{}", provider, chrono::Utc::now().timestamp());
    let body = add_key_body(&provider, &id, &secret, &source, &notes, &priority);

    let _: serde_json::Value = client.put("/v1/settings/llm", &body).await?;
    println!("{} Key added for {}", "✓".green(), provider);
    Ok(())
}

async fn keys_remove(provider: &str, key_id: &str) -> Result<()> {
    let confirm = dialoguer::Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt(format!("Remove key '{}/{}' ?", provider, key_id))
        .default(false)
        .interact()?;

    if !confirm {
        println!("Cancelled.");
        return Ok(());
    }

    let client = DaemonClient::connect().await?;
    let _: serde_json::Value = client
        .delete_req(&format!("/v1/settings/llm/keys/{}/{}", provider, key_id))
        .await?;
    println!("{} Key removed: {}/{}", "✓".green(), provider, key_id);
    Ok(())
}

/// What `llm keys validate` should do, before it does anything (M7).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ValidatePlan {
    /// The provider needs no key. Nothing is posted and nothing is graded.
    NoKeyNeeded,
    /// Ask the daemon to grade this secret.
    Grade,
    /// A keyed provider with nothing to grade.
    MissingSecret,
}

/// `requires_key` is the daemon's own word (L1); a provider it did not
/// describe is assumed keyed, which is what every cloud provider is.
pub(super) fn validate_plan(requires_key: Option<bool>, secret: Option<&str>) -> ValidatePlan {
    if requires_key == Some(false) {
        return ValidatePlan::NoKeyNeeded;
    }
    match secret {
        Some(secret) if !secret.trim().is_empty() => ValidatePlan::Grade,
        _ => ValidatePlan::MissingSecret,
    }
}

/// What a keyless provider is told, in place of a verdict on a key it has not
/// got.
pub(super) fn no_key_needed_line(provider: &str) -> String {
    format!(
        "{provider} needs no API key, so there is nothing to validate. It is reached over its \
         own endpoint; `openalpaca llm models` lists what it can serve, and `openalpaca llm \
         status` says whether it is loaded."
    )
}

/// Validate a key — or say that this provider has none to validate.
///
/// Posting a secret to a keyless provider used to print `✗ Key is invalid`,
/// which is a verdict on a key that does not exist and should not be sent:
/// the one provider designed to need nothing was the one reported broken.
async fn keys_validate(provider: &str, secret: Option<&str>) -> Result<()> {
    let client = DaemonClient::connect().await?;
    // A daemon that refuses the settings route tells us nothing about the
    // provider, and "keyed" is the safe reading of nothing.
    let settings: crate::commands::llm_status::LlmSettingsSnapshot = client
        .get("/v1/settings/llm")
        .await
        .unwrap_or_default();
    let requires_key = settings.requires_key_map().get(provider).copied();

    let secret = match validate_plan(requires_key, secret) {
        ValidatePlan::NoKeyNeeded => {
            println!("{} {}", "·".dimmed(), no_key_needed_line(provider));
            return Ok(());
        }
        ValidatePlan::MissingSecret => anyhow::bail!(
            "{provider} needs an API key: pass the one to check as `--secret <key>`."
        ),
        // Checked non-empty by the plan.
        ValidatePlan::Grade => secret.unwrap_or_default(),
    };

    let body = validate_body(provider, secret);

    let result: serde_json::Value = client.post("/v1/settings/llm/validate", &body).await?;

    let valid = result["valid"].as_bool().unwrap_or(false);
    if valid {
        println!("{} Key is {}", "✓".green(), "valid".green());
    } else {
        println!("{} Key is {}", "✗".red(), "invalid".red());
    }

    // Print additional details if available. The field names are
    // `KeyValidationResult`'s — `rate_limits` and `models_available`, not the
    // singular/reversed spellings this read before, which never matched.
    if let Some(tier) = result["tier"].as_str() {
        println!("  {} {}", "Tier:".dimmed(), tier);
    }
    if let Some(rate_limit) = result["rate_limits"].as_str() {
        println!("  {} {}", "Rate limit:".dimmed(), rate_limit);
    }
    if let Some(models) = result["models_available"].as_array()
        && !models.is_empty()
    {
        let names: Vec<&str> = models.iter().filter_map(|m| m.as_str()).collect();
        println!("  {} {}", "Models:".dimmed(), names.join(", "));
    }

    Ok(())
}

/// `POST /v1/settings/llm/validate`'s body, from `llm keys add` and
/// `llm keys validate`.
pub(crate) fn validate_body(provider: &str, secret: &str) -> serde_json::Value {
    serde_json::json!({
        "provider": provider,
        "secret": secret,
    })
}

/// `PUT /v1/settings/llm`'s body, from `llm keys add`. The id field is
/// `id` — `key_id` was silently dropped by serde and the daemon minted a
/// `key_<uuid8>` instead, so the id printed back by `keys list` was never
/// the one asked for.
pub(crate) fn add_key_body(
    provider: &str,
    id: &str,
    secret: &str,
    source: &str,
    notes: &str,
    priority: &str,
) -> serde_json::Value {
    serde_json::json!({
        "provider": provider,
        "key": {
            "id": id,
            "secret": secret,
            "source": source,
            "notes": notes,
            "priority": priority,
        }
    })
}

/// A provider's key ids in their order, as `GET /v1/settings/llm` lists
/// them (`[]` for a provider it does not list).
pub(crate) fn current_key_order(config: &serde_json::Value, provider: &str) -> Vec<String> {
    config["providers"][provider]["keys"]
        .as_array()
        .map(|keys| {
            keys.iter()
                .filter_map(|k| k["id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `PUT /v1/settings/llm/keys/reorder`'s body from `llm keys set-primary`,
/// as the route's `ReorderKeysRequest` reads it: the provider's keys in
/// the order `config` (the `GET /v1/settings/llm` just read) lists them,
/// unchanged, and the key to make primary (*Rev 142*'s item 7, (b)).
pub(crate) fn set_primary_body(
    provider: &str,
    key_id: &str,
    config: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "provider": provider,
        "key_order": current_key_order(config, provider),
        "primary_key_id": key_id,
    })
}

/// `PUT /v1/settings/llm/keys/reorder`'s body from `llm keys reorder`, as
/// the route's `ReorderKeysRequest` reads it: the order asked for, and no
/// primary, so every key keeps its priority.
pub(crate) fn reorder_body(provider: &str, key_ids: &[String]) -> serde_json::Value {
    serde_json::json!({
        "provider": provider,
        "key_order": key_ids,
        "primary_key_id": null,
    })
}

/// What `llm keys set-primary` and `llm keys reorder` check before they
/// send anything, against the provider's key order `config` (the
/// `GET /v1/settings/llm` just read) lists (*Rev 147*'s item 2, (b)): a
/// provider the daemon does not list, one that holds no key, an empty order
/// asked for, and a key id the provider does not hold are refused, naming
/// what the provider holds — the route would take each and change nothing,
/// or, for `set-primary`, make every key a fallback. On success, the
/// provider's keys the order leaves out, which the route keeps after the
/// listed ones in their old order.
pub(crate) fn check_key_ids(
    config: &serde_json::Value,
    provider: &str,
    key_ids: &[String],
) -> Result<Vec<String>, String> {
    let Some(listed) = config["providers"].as_object() else {
        return Err("the daemon listed no providers".to_string());
    };
    if !listed.contains_key(provider) {
        let names: Vec<&str> = listed.keys().map(String::as_str).collect();
        return Err(format!(
            "unknown provider '{provider}': the daemon lists {}",
            names.join(", ")
        ));
    }
    let order = current_key_order(config, provider);
    if order.is_empty() {
        return Err(format!("{provider} holds no keys"));
    }
    let holds = || format!("{provider} holds {}", order.join(", "));
    if key_ids.is_empty() {
        return Err(format!("no key ids given: {}", holds()));
    }
    for id in key_ids {
        if !order.contains(id) {
            return Err(format!("{provider} holds no key '{id}': {}", holds()));
        }
    }
    Ok(order
        .into_iter()
        .filter(|id| !key_ids.contains(id))
        .collect())
}

async fn keys_set_primary(provider: &str, key_id: &str) -> Result<()> {
    let client = DaemonClient::connect().await?;
    keys_set_primary_with(&client, provider, key_id).await
}

async fn keys_set_primary_with(client: &DaemonClient, provider: &str, key_id: &str) -> Result<()> {
    let config: serde_json::Value = client.get("/v1/settings/llm").await?;
    check_key_ids(&config, provider, &[key_id.to_string()]).map_err(anyhow::Error::msg)?;
    let body = set_primary_body(provider, key_id, &config);
    let _: serde_json::Value = client.put("/v1/settings/llm/keys/reorder", &body).await?;
    println!("{} Set {} as primary for {}", "✓".green(), key_id, provider);
    Ok(())
}

async fn keys_reorder(provider: &str, key_ids: Vec<String>) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let line = keys_reorder_with(&client, provider, &key_ids).await?;
    println!("{} {line}", "✓".green());
    Ok(())
}

/// `llm keys reorder` past the connection: the order read and checked
/// ([`check_key_ids`]), then sent; the line it ends on.
async fn keys_reorder_with(
    client: &DaemonClient,
    provider: &str,
    key_ids: &[String],
) -> Result<String> {
    let config: serde_json::Value = client.get("/v1/settings/llm").await?;
    let unlisted = check_key_ids(&config, provider, key_ids).map_err(anyhow::Error::msg)?;
    let body = reorder_body(provider, key_ids);
    let _: serde_json::Value = client.put("/v1/settings/llm/keys/reorder", &body).await?;
    Ok(reordered_line(&unlisted))
}

/// What `llm keys reorder` says once the order is sent: the keys it left
/// out, if any, keep their order after the ones it named.
pub(crate) fn reordered_line(unlisted: &[String]) -> String {
    if unlisted.is_empty() {
        "Keys reordered".to_string()
    } else {
        format!(
            "Keys reordered; not named, so kept after them in their old order: {}",
            unlisted.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() {
        colored::control::set_override(false);
    }

    /// M7: `llm keys validate --provider ollama` used to post a secret and
    /// print `✗ Key is invalid` — a verdict on a key the provider is designed
    /// not to have. Nothing is sent now, and the answer says why.
    #[test]
    fn a_keyless_provider_has_nothing_to_validate() {
        assert_eq!(
            validate_plan(Some(false), None),
            ValidatePlan::NoKeyNeeded,
            "no key needed, and none was asked for"
        );
        assert_eq!(
            validate_plan(Some(false), Some("sk-whatever")),
            ValidatePlan::NoKeyNeeded,
            "a secret typed at a keyless provider is still not posted"
        );

        let line = no_key_needed_line("ollama");
        assert!(line.contains("needs no API key"), "{line}");
        assert!(line.contains("nothing to validate"), "{line}");
    }

    /// A keyed provider is unaffected, and a provider the daemon did not
    /// describe is treated as keyed — the safe reading of silence.
    #[test]
    fn a_keyed_provider_is_still_graded_and_still_needs_the_key() {
        assert_eq!(validate_plan(Some(true), Some("sk-1")), ValidatePlan::Grade);
        assert_eq!(validate_plan(None, Some("sk-1")), ValidatePlan::Grade);
        assert_eq!(
            validate_plan(Some(true), None),
            ValidatePlan::MissingSecret,
            "nothing to grade"
        );
        assert_eq!(
            validate_plan(None, Some("   ")),
            ValidatePlan::MissingSecret,
            "whitespace is not a key"
        );
    }

    /// L12: KEY_ID printed blank and PRIORITY always said `Fallback` — the CLI
    /// read `key["key_id"]` where `KeyInfo` serializes `id`, and a
    /// `primary_key_id` no response has ever carried.
    #[test]
    fn a_keys_row_reads_the_names_the_daemon_sends() {
        plain();
        let config = serde_json::json!({
            "providers": {
                "anthropic": {
                    "enabled": true,
                    "requires_key": true,
                    "key_selection_strategy": "round_robin",
                    "keys": [{
                        "id": "key_ab12cd34",
                        "masked_secret": "sk-…7f3a",
                        "priority": "primary",
                        "source": "API Console",
                        "status": "healthy",
                    }],
                },
            },
        });

        let rows = key_rows(&config);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].key_id, "key_ab12cd34");
        let rendered = rows[0].table_row();
        assert!(rendered.contains("key_ab12cd34"), "{rendered}");
        assert!(rendered.contains("primary"), "{rendered}");
        assert!(!rendered.contains("Fallback"), "{rendered}");
    }

    /// A provider that needs no key (L1) gets one honest row, not a blank one
    /// and not silence.
    #[test]
    fn a_keyless_provider_gets_a_row_that_says_it_needs_none() {
        plain();
        let config = serde_json::json!({
            "providers": {
                "ollama": {
                    "enabled": true,
                    "requires_key": false,
                    "key_selection_strategy": "round_robin",
                    "keys": [],
                },
            },
        });

        let rows = key_rows(&config);
        assert_eq!(rows.len(), 1);
        let rendered = rows[0].table_row();
        assert!(rendered.contains("ollama"), "{rendered}");
        assert!(rendered.contains("no key needed"), "{rendered}");
        assert!(!rendered.contains("unknown"), "{rendered}");
    }

    /// A keyed provider with nothing configured is still absent from the list,
    /// exactly as before — the keyless row is not a licence to invent rows.
    #[test]
    fn a_keyed_provider_with_no_keys_contributes_no_row() {
        plain();
        let config = serde_json::json!({
            "providers": {
                "openai": {
                    "enabled": false,
                    "requires_key": true,
                    "key_selection_strategy": "round_robin",
                    "keys": [],
                },
            },
        });

        assert!(key_rows(&config).is_empty());
    }

    /// A daemon too old to send `requires_key` described only keyed providers.
    #[test]
    fn a_provider_that_does_not_say_is_treated_as_keyed() {
        plain();
        let config = serde_json::json!({
            "providers": { "ollama": { "enabled": true, "keys": [] } },
        });

        assert!(key_rows(&config).is_empty());
    }

    /// The `llm keys` verbs' bodies, pinned whole for the daemon's
    /// `routes::settings::keys_cli_tests`, which drives them through
    /// `PUT /v1/settings/llm/keys/reorder`'s own handler.
    const KEYS_BODIES: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/commands/llm_keys/keys-bodies.json"
    );

    /// `GET /v1/settings/llm` with two `openai` keys, `key-1` first.
    fn two_keys() -> serde_json::Value {
        serde_json::json!({
            "providers": {
                "openai": {
                    "enabled": true,
                    "requires_key": true,
                    "keys": [
                        { "id": "key-1", "priority": "primary" },
                        { "id": "key-2", "priority": "fallback" },
                    ],
                },
            },
        })
    }

    /// *Rev 142*'s item 7, (b): `set-primary` sends the provider's keys in
    /// the order it just read, and the key to make primary; `reorder` the
    /// order asked for and no primary — `{provider, key_order,
    /// primary_key_id}`, the keys `ReorderKeysRequest` reads.
    #[test]
    fn the_keys_verbs_send_what_the_reorder_route_reads() {
        let config = two_keys();
        assert_eq!(current_key_order(&config, "openai"), ["key-1", "key-2"]);
        assert!(current_key_order(&config, "anthropic").is_empty());
        let set_primary = set_primary_body("openai", "key-2", &config);
        assert_eq!(
            set_primary,
            serde_json::json!({
                "provider": "openai",
                "key_order": ["key-1", "key-2"],
                "primary_key_id": "key-2",
            })
        );
        let order = ["key-2".to_string(), "key-1".to_string()];
        let reorder = reorder_body("openai", &order);
        assert_eq!(
            reorder,
            serde_json::json!({
                "provider": "openai",
                "key_order": ["key-2", "key-1"],
                "primary_key_id": null,
            })
        );
        for body in [&set_primary, &reorder] {
            serde_json::from_value::<openalpaca_llm::config::settings_types::ReorderKeysRequest>(
                body.clone(),
            )
            .unwrap_or_else(|e| panic!("the route's type refuses {body}: {e}"));
        }

        let built = serde_json::json!({ "set_primary": set_primary, "reorder": reorder });
        let pinned: serde_json::Value =
            serde_json::from_slice(&std::fs::read(KEYS_BODIES).expect("the pinned bodies"))
                .expect("JSON");
        assert_eq!(
            built, pinned,
            "the keys verbs' bodies moved: rewrite {KEYS_BODIES} with them, and run the \
             daemon's routes::settings::keys_cli_tests"
        );
    }

    /// The order is read where `GET /v1/settings/llm`'s sample holds a key's
    /// id: `providers.<provider>.keys[].id`.
    #[test]
    fn the_key_order_is_read_where_the_settings_sample_holds_it() {
        let sample: serde_json::Value = serde_json::from_slice(
            &std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../openalpaca-gui/src/lib/api/contract/responses/get-settings-llm.json"
            ))
            .expect("the sample"),
        )
        .expect("JSON");
        assert_eq!(current_key_order(&sample, "*"), [""]);
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// *Rev 147*'s item 2, (b): what the route would take silently — an
    /// unknown provider, one with no keys, an empty order, a key id the
    /// provider does not hold — is refused, naming what it holds; a
    /// partial order is taken, the keys it leaves out named.
    #[test]
    fn the_keys_verbs_refuse_what_the_route_would_take_silently() {
        let mut config = two_keys();
        config["providers"]["anthropic"] = serde_json::json!({ "keys": [] });
        let refused = |provider: &str, asked: &[&str]| {
            check_key_ids(&config, provider, &ids(asked)).unwrap_err()
        };
        assert_eq!(
            refused("opneai", &["key-1"]),
            "unknown provider 'opneai': the daemon lists anthropic, openai"
        );
        assert_eq!(refused("anthropic", &["key-1"]), "anthropic holds no keys");
        assert_eq!(
            refused("openai", &[]),
            "no key ids given: openai holds key-1, key-2"
        );
        assert_eq!(
            refused("openai", &["key-1", "key-3"]),
            "openai holds no key 'key-3': openai holds key-1, key-2"
        );
        assert_eq!(
            check_key_ids(&config, "openai", &ids(&["key-2", "key-1"])),
            Ok(vec![])
        );
        assert_eq!(
            check_key_ids(&config, "openai", &ids(&["key-2"])),
            Ok(ids(&["key-1"]))
        );
        assert_eq!(reordered_line(&[]), "Keys reordered");
        assert_eq!(
            reordered_line(&ids(&["key-1"])),
            "Keys reordered; not named, so kept after them in their old order: key-1"
        );
    }

    /// The settings read and a PUT answer the verb must not reach when it
    /// refuses, from a server the test stops once the verb has returned.
    /// No `EnvSandbox`: the `_with` verbs reach the daemon only through the
    /// `DaemonClient::for_tests` they are given — `get`, `put`, the pure
    /// body and check functions, `println!` — and never resolve the home
    /// store or the config directory, so these tests leave `ENV_LOCK` to
    /// the tests that need it.
    fn settings_then_put() -> (std::net::SocketAddr, crate::test_util::StoppableServer) {
        use crate::test_util::{json_answer, serve_until_stopped};
        serve_until_stopped(vec![
            json_answer("200 OK", "settings", &two_keys().to_string()),
            json_answer("200 OK", "put", "{}"),
        ])
    }

    /// The one request a refused verb sent: the settings read, and no PUT,
    /// though one was on offer.
    fn only_the_read(requests: &[String]) {
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert!(requests[0].starts_with("GET /v1/settings/llm HTTP/1.1"));
        assert!(
            !requests.iter().any(|r| r.starts_with("PUT ")),
            "{requests:?}"
        );
    }

    /// `set-primary` with a key id the provider does not hold reads the
    /// order and stops there: no PUT, so no key is made a fallback.
    #[tokio::test]
    async fn set_primary_with_an_unknown_key_sends_no_put() {
        let (addr, server) = settings_then_put();
        let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");
        let err = keys_set_primary_with(&client, "openai", "key-9")
            .await
            .unwrap_err()
            .to_string();
        let requests = server.stop();
        assert_eq!(
            err,
            "openai holds no key 'key-9': openai holds key-1, key-2"
        );
        only_the_read(&requests);
    }

    /// `reorder` reads the order first too: a mistyped provider never
    /// reaches the route.
    #[tokio::test]
    async fn reorder_with_an_unknown_provider_sends_no_put() {
        let (addr, server) = settings_then_put();
        let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");
        let err = keys_reorder_with(&client, "opneai", &ids(&["key-1"]))
            .await
            .unwrap_err()
            .to_string();
        let requests = server.stop();
        assert_eq!(err, "unknown provider 'opneai': the daemon lists openai");
        only_the_read(&requests);
    }

    /// A partial order is sent as asked, after the read, and the line says
    /// which keys keep their place after it.
    #[tokio::test]
    async fn reorder_reads_then_sends_a_partial_order_and_says_so() {
        let (addr, server) = settings_then_put();
        let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");
        let line = keys_reorder_with(&client, "openai", &ids(&["key-2"]))
            .await
            .unwrap();
        let requests = server.stop();
        assert_eq!(requests.len(), 2, "{requests:?}");
        assert!(requests[0].starts_with("GET /v1/settings/llm HTTP/1.1"));
        assert!(requests[1].starts_with("PUT /v1/settings/llm/keys/reorder HTTP/1.1"));
        assert!(
            requests[1]
                .ends_with(r#"{"key_order":["key-2"],"primary_key_id":null,"provider":"openai"}"#),
            "{}",
            requests[1]
        );
        assert_eq!(
            line,
            "Keys reordered; not named, so kept after them in their old order: key-1"
        );
    }
}
