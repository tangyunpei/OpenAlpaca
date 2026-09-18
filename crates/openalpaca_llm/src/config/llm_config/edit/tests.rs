//! M3: a write to `llm.toml` must not cost the owner their file.

use super::*;
use crate::config::llm_config::{read_config_with_text, write_config};

/// The file the daemon actually seeds on first boot. Read from the template
/// itself rather than a copy, so the thing these tests protect is the thing
/// that ships.
const SEEDED: &str = include_str!("../../../../../../scripts/release/templates/config/llm.toml");

fn parse(text: &str) -> LlmRouterConfig {
    toml::from_str(text).expect("the seeded template parses")
}

fn enabled(config: &LlmRouterConfig, provider: &str) -> Option<bool> {
    config
        .providers
        .as_ref()
        .and_then(|p| p.get(provider))
        .and_then(|p| p.enabled)
}

/// The write the GUI's one switch performs. Before M3 it re-serialised the
/// whole document: every comment gone, `[providers.*]` re-sorted, and the
/// owner's explanation of what `default_model = ""` means deleted.
#[test]
fn enabling_a_provider_keeps_the_comments_and_the_order() {
    let before = parse(SEEDED);
    let mut after = before.clone();
    after
        .providers
        .as_mut()
        .expect("the template declares providers")
        .get_mut("ollama")
        .expect("the template declares ollama")
        .enabled = Some(true);

    let written = render_config_preserving(SEEDED, &before, &after).expect("the edit succeeds");

    // The bit moved, and nothing else did.
    assert_eq!(enabled(&parse(&written), "ollama"), Some(true));
    assert_eq!(enabled(&parse(&written), "anthropic"), Some(false));

    for comment in [
        "# Local models, served by the Ollama you run yourself.",
        "# Turning this on is the only action needed: the daemon then asks the running",
        "# Empty means \"whatever is installed\": the router picks a discovered model,",
        "# The output ceiling for one answer. A local model bills nothing, and a",
        "# Wall clock for one non-streaming LLM call, for any provider that does not set",
    ] {
        assert!(
            written.contains(comment),
            "the write erased a comment line: {comment}"
        );
    }

    // Section order, exactly as the owner reads it.
    let order: Vec<&str> = written
        .lines()
        .filter(|l| l.starts_with('['))
        .map(|l| l.trim())
        .collect();
    let expected: Vec<&str> = SEEDED
        .lines()
        .filter(|l| l.starts_with('['))
        .map(|l| l.trim())
        .collect();
    assert_eq!(order, expected, "the write reshuffled the file");

    // Key order inside the block that changed.
    let block = written
        .split("[providers.ollama]")
        .nth(1)
        .expect("the ollama block is still there");
    let keys: Vec<&str> = block
        .lines()
        .take_while(|l| !l.trim_start().starts_with('['))
        .filter_map(|l| l.split_once('=').map(|(k, _)| k.trim()))
        .collect();
    assert_eq!(
        keys,
        vec![
            "enabled",
            "base_url",
            "strategy",
            "default_model",
            "default_max_tokens",
            "request_timeout_secs"
        ]
    );
    assert!(written.contains("enabled = true"));
}

/// A key these types do not model is the owner's, not ours to delete — the
/// difference between "preserve unknown keys" and "honour a removal" is the
/// before/after pair, not a guess.
#[test]
fn a_key_this_crate_does_not_model_survives() {
    let source = concat!(
        "# top of file\n",
        "[orchestrator]\n",
        "model = \"claude-haiku-4-5-20251001\"\n",
        "\n",
        "[providers.ollama]\n",
        "enabled = false\n",
        "# a knob from a newer build than this one\n",
        "keep_alive_secs = 900\n",
        "\n",
        "[something_else]\n",
        "note = \"hand written\"\n",
    );
    let before = parse(source);
    let mut after = before.clone();
    after
        .providers
        .as_mut()
        .unwrap()
        .get_mut("ollama")
        .unwrap()
        .enabled = Some(true);

    let written = render_config_preserving(source, &before, &after).expect("the edit succeeds");

    assert!(written.contains("keep_alive_secs = 900"));
    assert!(written.contains("# a knob from a newer build than this one"));
    assert!(written.contains("[something_else]"));
    assert!(written.contains("note = \"hand written\""));
    assert!(written.contains("# top of file"));
    assert_eq!(enabled(&parse(&written), "ollama"), Some(true));
}

/// A trailing comment belongs to the line it is on, and survives the value
/// under it changing.
#[test]
fn a_trailing_comment_survives_its_value_changing() {
    let source = "[providers.ollama]\nenabled = false # off until you say so\n";
    let before = parse(source);
    let mut after = before.clone();
    after
        .providers
        .as_mut()
        .unwrap()
        .get_mut("ollama")
        .unwrap()
        .enabled = Some(true);

    let written = render_config_preserving(source, &before, &after).expect("the edit succeeds");
    assert_eq!(written, "[providers.ollama]\nenabled = true # off until you say so\n");
}

/// A key the types *did* know about and no longer emit was dropped on purpose.
#[test]
fn a_deliberate_removal_is_honoured() {
    let source = concat!(
        "[orchestrator]\n",
        "model = \"claude-haiku-4-5-20251001\"\n",
        "fallback_models = [\"claude-sonnet-4-6\"]\n",
    );
    let before = parse(source);
    let mut after = before.clone();
    after.orchestrator.as_mut().unwrap().fallback_models = None;

    let written = render_config_preserving(source, &before, &after).expect("the edit succeeds");
    assert!(
        !written.contains("fallback_models"),
        "the cleared field is still in the file: {written}"
    );
    assert!(written.contains("claude-haiku-4-5-20251001"));
}

/// A block the file has never had is added, and the rest is left alone.
#[test]
fn a_new_block_is_added_without_disturbing_the_old_ones() {
    let source = concat!(
        "# mine\n",
        "[orchestrator]\n",
        "model = \"claude-haiku-4-5-20251001\"\n",
    );
    let before = parse(source);
    let mut after = before.clone();
    after.web_search = Some(crate::config::WebSearchConfig {
        api_key: String::new(),
        timeout_secs: 20,
    });

    let written = render_config_preserving(source, &before, &after).expect("the edit succeeds");
    let reparsed = parse(&written);
    assert_eq!(reparsed.web_search.as_ref().unwrap().timeout_secs, 20);
    assert!(written.contains("# mine"));
    assert!(written.contains("model = \"claude-haiku-4-5-20251001\""));
}

/// A write that changes nothing changes nothing — not even whitespace.
#[test]
fn an_unchanged_config_is_written_back_byte_for_byte() {
    let before = parse(SEEDED);
    let after = before.clone();
    let written = render_config_preserving(SEEDED, &before, &after).expect("the edit succeeds");
    assert_eq!(written, SEEDED);
}

/// The CLI's `openalpaca config set ai.*` path shares the writer, because
/// `write_config` is what it calls (M3).
#[test]
fn the_cli_write_path_preserves_comments() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("llm.toml");
    std::fs::write(&path, SEEDED).expect("seed the file");

    let (_, mut config) = read_config_with_text(&path).expect("read it back");
    config
        .providers
        .as_mut()
        .unwrap()
        .get_mut("ollama")
        .unwrap()
        .enabled = Some(true);
    write_config(&path, &config).expect("the write succeeds");

    let written = std::fs::read_to_string(&path).expect("read what was written");
    assert!(written.contains("# Local models, served by the Ollama you run yourself."));
    assert!(written.contains("enabled = true"));
    assert_eq!(enabled(&parse(&written), "ollama"), Some(true));
}

/// And a file that does not exist yet is still written from a full render —
/// there is no document to preserve, and `openalpaca config set` on a fresh
/// machine must still produce one.
#[test]
fn a_missing_file_is_written_from_scratch() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let path = dir.path().join("llm.toml");

    let mut config = LlmRouterConfig::default();
    config.orchestrator = Some(crate::config::OrchestratorLlmConfig {
        model: "local-model".to_string(),
        fallback_models: None,
    });
    write_config(&path, &config).expect("the write succeeds");

    let written = std::fs::read_to_string(&path).expect("the file exists now");
    assert_eq!(
        parse(&written).orchestrator.unwrap().model,
        "local-model"
    );
}

/// A hand-written file that spells its tables inline. `TableLike::insert`
/// unwraps the item as a value for an inline table, so handing it a
/// `[section]` table panics — a config write must never do that, whatever the
/// owner's formatting.
#[test]
fn an_inline_table_is_edited_inline_and_never_panics() {
    let source = concat!(
        "# inline by hand\n",
        "providers = { ollama = { enabled = false, strategy = \"round_robin\" } }\n",
    );
    let before = parse(source);

    // Change a key inside the inline block.
    let mut after = before.clone();
    after
        .providers
        .as_mut()
        .unwrap()
        .get_mut("ollama")
        .unwrap()
        .enabled = Some(true);
    let written = render_config_preserving(source, &before, &after).expect("the edit succeeds");
    assert_eq!(enabled(&parse(&written), "ollama"), Some(true));
    assert!(written.contains("# inline by hand"));

    // And add a whole provider block the inline table has never had.
    let mut after = parse(&written);
    let added = after.providers.as_mut().unwrap();
    let mut anthropic = added.get("ollama").cloned().expect("a shape to copy");
    anthropic.enabled = Some(false);
    added.insert("anthropic".to_string(), anthropic);

    let written2 =
        render_config_preserving(&written, &parse(&written), &after).expect("the edit succeeds");
    let reparsed = parse(&written2);
    assert_eq!(enabled(&reparsed, "anthropic"), Some(false));
    assert_eq!(enabled(&reparsed, "ollama"), Some(true));
}

/// Editing the key list must not collapse `[[providers.x.keys]]` sections into
/// an inline `keys = [{ … }]` array. The array is not a value in the document,
/// and converting it would rewrite a block the write never touched the shape of.
#[test]
fn an_array_of_tables_keeps_its_sections() {
    let source = concat!(
        "[providers.anthropic]\n",
        "enabled = true\n",
        "\n",
        "# the one I use\n",
        "[[providers.anthropic.keys]]\n",
        "id = \"key_one\"\n",
        "secret_env = \"ANTHROPIC_API_KEY\"\n",
        "priority = \"primary\"\n",
    );
    let before = parse(source);
    let mut after = before.clone();
    let keys = after
        .providers
        .as_mut()
        .unwrap()
        .get_mut("anthropic")
        .unwrap()
        .keys
        .as_mut()
        .expect("the block declares a key");
    let mut second = keys[0].clone();
    second.id = "key_two".to_string();
    keys.push(second);

    let written = render_config_preserving(source, &before, &after).expect("the edit succeeds");

    assert!(
        written.contains("[[providers.anthropic.keys]]"),
        "the key list was rewritten inline: {written}"
    );
    assert!(
        !written.contains("keys = ["),
        "the key list was rewritten inline: {written}"
    );
    assert!(written.contains("key_two"));
    assert!(written.contains("# the one I use"));
    let reparsed = parse(&written);
    assert_eq!(
        reparsed.providers.unwrap()["anthropic"]
            .keys
            .as_ref()
            .unwrap()
            .len(),
        2
    );
}
