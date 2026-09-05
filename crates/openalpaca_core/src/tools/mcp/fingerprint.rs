//! `config_fingerprint` — the diff key half that says *the declaration itself
//! changed* (extension design §3.3 E2, §3.4 trigger 2, §10 case 15, X-11).
//!
//! Edge case 15's per-server diff key is **presence + `enabled` bit +
//! fingerprint**. The fingerprint has to answer one question honestly: did the
//! owner change what this server *is*? So it is computed over a canonical
//! rendering of the **parsed block, not of its bytes** — a comment, a blank
//! line or a key-order edit changes nothing; a value edit does.
//!
//! **No secret enters the hash**, which is why no salt and no keyed hash are
//! needed: every `env.*` value, every `extra_headers.*` value and a literal
//! `auth.bearer` are replaced by the fixed marker `<masked>` (keys kept).
//! Those three are the only places a credential byte can appear in a block
//! (`config.rs` — `bearer_env`/`api_key_env` are name-only), so the preimage
//! covers structure, `command`/`args`/`url`/`cwd`/timeouts, env and header
//! *names* and the auth *kind*, and nothing else.
//!
//! Consequence, stated because the design states it: a rotated credential
//! **value** under an unchanged name is invisible to the watcher by design. It
//! is picked up by `reload` or `enable`, which re-resolve the env at E2 — which
//! is why env-var indirection stays the recommended declaration shape.

use super::config::McpServerConfig;

/// The masking marker. Fixed, so two different secrets under the same key
/// produce the same preimage.
const MASKED: &str = "<masked>";

/// blake3 over the canonical, masked rendering of one `[servers.<name>]` block.
///
/// Returns the hex digest. A block that cannot be re-serialised (which no
/// `McpServerConfig` can — every field type serialises) degrades to a digest
/// over the error text rather than panicking: a fingerprint that always differs
/// costs at most a redundant reload, never a wrong one.
pub fn config_fingerprint(server: &McpServerConfig) -> String {
    let preimage = match toml::Value::try_from(server) {
        Ok(value) => canonical_render(&mask(value)),
        Err(e) => {
            tracing::warn!(error = %e, "MCP block could not be rendered for its fingerprint");
            format!("unrenderable: {e}")
        }
    };
    blake3::hash(preimage.as_bytes()).to_hex().to_string()
}

/// Replace every credential-bearing value with [`MASKED`], keeping keys.
fn mask(value: toml::Value) -> toml::Value {
    let toml::Value::Table(mut table) = value else {
        return value;
    };
    for key in ["env", "extra_headers"] {
        if let Some(toml::Value::Table(inner)) = table.get_mut(key) {
            let names: Vec<String> = inner.keys().cloned().collect();
            for name in names {
                inner.insert(name, toml::Value::String(MASKED.to_string()));
            }
        }
    }
    // `auth` is untagged: only the `{ bearer = "…" }` shape carries a literal.
    // `bearer_env` / `api_key_env` are names, and `api_key_header` is a header
    // name — all three are structure, not secret.
    if let Some(toml::Value::Table(auth)) = table.get_mut("auth")
        && let Some(slot) = auth.get_mut("bearer")
    {
        *slot = toml::Value::String(MASKED.to_string());
    }
    toml::Value::Table(table)
}

/// Render a `toml::Value` with **every table's keys sorted by this function**,
/// not by `toml::Table`'s own ordering — which flips with the `preserve_order`
/// feature and would silently change every fingerprint in the install.
fn canonical_render(value: &toml::Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

fn write_value(out: &mut String, value: &toml::Value) {
    match value {
        toml::Value::Table(table) => {
            let mut keys: Vec<&String> = table.keys().collect();
            keys.sort();
            out.push('{');
            for key in keys {
                out.push_str(key);
                out.push('=');
                write_value(out, &table[key]);
                out.push(';');
            }
            out.push('}');
        }
        toml::Value::Array(items) => {
            // Order is meaningful here — `args` is a command line.
            out.push('[');
            for item in items {
                write_value(out, item);
                out.push(',');
            }
            out.push(']');
        }
        toml::Value::String(s) => {
            out.push('"');
            out.push_str(s);
            out.push('"');
        }
        other => out.push_str(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::mcp::config::McpConfig;

    fn block(toml_text: &str) -> McpServerConfig {
        McpConfig::parse(toml_text)
            .expect("parse")
            .servers
            .remove("x")
            .expect("server x")
    }

    const STDIO: &str = r#"
        [servers.x]
        transport = "stdio"
        command = "npx"
        args = ["-y", "server"]
        env = { TOKEN = "secret-one" }
    "#;

    #[test]
    fn comments_blank_lines_and_key_order_do_not_change_the_fingerprint() {
        let plain = block(STDIO);
        let noisy = block(
            r#"
            # a comment nobody should hash

            [servers.x]
            env = { TOKEN = "secret-one" }
            args = ["-y", "server"]

            command = "npx"   # trailing comment
            transport = "stdio"
        "#,
        );
        assert_eq!(config_fingerprint(&plain), config_fingerprint(&noisy));
    }

    #[test]
    fn a_rotated_env_value_under_the_same_name_changes_nothing() {
        let before = block(STDIO);
        let after = block(
            r#"
            [servers.x]
            transport = "stdio"
            command = "npx"
            args = ["-y", "server"]
            env = { TOKEN = "secret-two" }
        "#,
        );
        assert_eq!(config_fingerprint(&before), config_fingerprint(&after));
    }

    #[test]
    fn an_env_key_rename_does_change_the_fingerprint() {
        let before = block(STDIO);
        let after = block(
            r#"
            [servers.x]
            transport = "stdio"
            command = "npx"
            args = ["-y", "server"]
            env = { OTHER = "secret-one" }
        "#,
        );
        assert_ne!(config_fingerprint(&before), config_fingerprint(&after));
    }

    #[test]
    fn a_command_edit_changes_the_fingerprint() {
        let before = block(STDIO);
        let after = block(
            r#"
            [servers.x]
            transport = "stdio"
            command = "node"
            args = ["-y", "server"]
            env = { TOKEN = "secret-one" }
        "#,
        );
        assert_ne!(config_fingerprint(&before), config_fingerprint(&after));
    }

    #[test]
    fn an_arg_reorder_changes_the_fingerprint() {
        let before = block(STDIO);
        let after = block(
            r#"
            [servers.x]
            transport = "stdio"
            command = "npx"
            args = ["server", "-y"]
            env = { TOKEN = "secret-one" }
        "#,
        );
        assert_ne!(config_fingerprint(&before), config_fingerprint(&after));
    }

    #[test]
    fn the_enabled_bit_is_part_of_the_block_but_the_diff_key_carries_it_separately() {
        // Stated so nobody removes the bit from edge case 15's diff key on the
        // theory that the fingerprint already covers it: it does, but the two
        // halves drive different verbs.
        let on = block(STDIO);
        let off = block(
            r#"
            [servers.x]
            transport = "stdio"
            command = "npx"
            args = ["-y", "server"]
            env = { TOKEN = "secret-one" }
            enabled = false
        "#,
        );
        assert_ne!(config_fingerprint(&on), config_fingerprint(&off));
    }

    #[test]
    fn a_literal_bearer_never_reaches_the_preimage() {
        let one = block(
            r#"
            [servers.x]
            transport = "http"
            url = "https://example.com/mcp"
            auth = { bearer = "token-one" }
        "#,
        );
        let two = block(
            r#"
            [servers.x]
            transport = "http"
            url = "https://example.com/mcp"
            auth = { bearer = "token-two" }
        "#,
        );
        assert_eq!(config_fingerprint(&one), config_fingerprint(&two));

        // The auth *kind* is structure and must still be visible.
        let env_auth = block(
            r#"
            [servers.x]
            transport = "http"
            url = "https://example.com/mcp"
            auth = { bearer_env = "TOKEN" }
        "#,
        );
        assert_ne!(config_fingerprint(&one), config_fingerprint(&env_auth));
    }

    #[test]
    fn an_extra_header_value_is_masked_but_its_name_is_not() {
        let one = block(
            r#"
            [servers.x]
            transport = "http"
            url = "https://example.com/mcp"
            extra_headers = { "X-Key" = "a" }
        "#,
        );
        let two = block(
            r#"
            [servers.x]
            transport = "http"
            url = "https://example.com/mcp"
            extra_headers = { "X-Key" = "b" }
        "#,
        );
        let renamed = block(
            r#"
            [servers.x]
            transport = "http"
            url = "https://example.com/mcp"
            extra_headers = { "X-Other" = "a" }
        "#,
        );
        assert_eq!(config_fingerprint(&one), config_fingerprint(&two));
        assert_ne!(config_fingerprint(&one), config_fingerprint(&renamed));
    }

    #[test]
    fn the_fingerprint_is_stable_across_calls() {
        let one = block(STDIO);
        assert_eq!(config_fingerprint(&one), config_fingerprint(&one));
    }
}
