//! Writing `llm.toml` back without erasing what the owner wrote in it (M3).
//!
//! Every write used to go out through [`render_config`](super::render_config),
//! which serialises the whole typed config from scratch. That is a correct TOML
//! file and a lossy one: the comments explaining what `default_model = ""`
//! means, the blank lines grouping the providers, the owner's own key order and
//! any key these types do not model are all gone the first time the GUI toggles
//! a provider.
//!
//! So a write is an *edit* instead. The typed config is still what the callers
//! mutate — nothing about them changes — but the bytes are produced by applying
//! the difference between the config as it was read and the config as it is now
//! onto the document on disk, key by key, through `toml_edit`. A toggle changes
//! one `enabled = false` to `enabled = true` and touches nothing else.
//!
//! The before/after pair is what makes "preserve unknown keys" and "honour a
//! deliberate removal" different things: a key the document has and *neither*
//! side mentions is not ours to delete, while a key that was in `before` and is
//! gone from `after` was removed on purpose.

use super::router_config::LlmRouterConfig;
use crate::error::LlmError;
use toml_edit::{DocumentMut, Item, TableLike};

/// The text to write for `after`, keeping everything about `existing` that
/// `after` does not contradict.
///
/// `before` must be `existing` as this crate's types parsed it — the caller has
/// it already, because it is what the mutation was applied to.
pub fn render_config_preserving(
    existing: &str,
    before: &LlmRouterConfig,
    after: &LlmRouterConfig,
) -> Result<String, LlmError> {
    let before_value = to_table(before)?;
    let after_value = to_table(after)?;

    // Nothing changed: hand back the file untouched, byte for byte.
    if before_value == after_value {
        return Ok(existing.to_string());
    }

    let mut doc: DocumentMut = existing.parse().map_err(|e| {
        LlmError::Config(format!("Failed to parse the config for editing: {e}"))
    })?;
    // The same content rendered from scratch — the source of well-formed
    // `toml_edit` items for the keys that did change.
    let rendered: DocumentMut = super::render_config(after)?.parse().map_err(|e| {
        LlmError::Config(format!("Failed to re-parse the rendered config: {e}"))
    })?;

    apply_delta(
        doc.as_table_mut(),
        &before_value,
        &after_value,
        rendered.as_table(),
    );

    Ok(doc.to_string())
}

fn to_table(config: &LlmRouterConfig) -> Result<toml::Table, LlmError> {
    match toml::Value::try_from(config)
        .map_err(|e| LlmError::Config(format!("Failed to serialize config: {e}")))?
    {
        toml::Value::Table(table) => Ok(table),
        other => Err(LlmError::Config(format!(
            "The config serialized as {} rather than a table",
            other.type_str()
        ))),
    }
}

/// Rewrite `target` so it says what `after` says, touching only the keys whose
/// value actually moved between `before` and `after`.
///
/// `rendered` is `after` serialised, and supplies the replacement items.
///
/// Works the same on a `[section]` and on a hand-written `providers = { … }`:
/// `TableLike` covers both, and an inline table converts a table item to an
/// inline one on the way in (`Item::into_value`), so the owner's formatting
/// decides the shape rather than this code.
fn apply_delta(
    target: &mut dyn TableLike,
    before: &toml::Table,
    after: &toml::Table,
    rendered: &dyn TableLike,
) {
    for (key, after_value) in after.iter() {
        let before_value = before.get(key);
        if before_value == Some(after_value) {
            // Untouched by this write — and therefore untouched in the file,
            // comments, spacing, position and all.
            continue;
        }

        let Some(rendered_item) = rendered.get(key) else {
            continue;
        };

        // A table on both sides: descend rather than replace, so a change to
        // one key inside `[providers.ollama]` does not rewrite the block.
        if let toml::Value::Table(after_sub) = after_value
            && let Some(rendered_sub) = rendered_item.as_table_like()
        {
            // A block the file does not have yet starts as an empty table and
            // is filled key by key below, so it arrives with ordinary
            // formatting instead of the source document's positions.
            if target.get(key).and_then(Item::as_table_like).is_none() {
                target.insert(key, Item::Table(toml_edit::Table::new()));
            }
            let empty = toml::Table::new();
            let before_sub = match before_value {
                Some(toml::Value::Table(t)) => t,
                _ => &empty,
            };
            if let Some(target_sub) = target.get_mut(key).and_then(Item::as_table_like_mut) {
                apply_delta(target_sub, before_sub, after_sub, rendered_sub);
                continue;
            }
        }

        set_preserving_decor(target, key, rendered_item.clone());
    }

    // A key this crate's types knew about and no longer emit was removed on
    // purpose (a deleted API key, a cleared override). A key they never knew
    // about is the owner's, and stays.
    for key in before.keys() {
        if !after.contains_key(key) {
            target.remove(key);
        }
    }
}

/// Put `new_item` at `key` without disturbing how the file writes that key.
///
/// `TableLike::insert` re-formats an existing key, which drops the comment
/// lines standing above it — exactly what this module exists to keep. So an
/// existing entry is replaced through `get_mut`, and the shape the file already
/// uses for that key is what it keeps: a value stays a value, carrying its own
/// spacing and trailing comment, and a `[[section]]` array of tables stays one
/// rather than collapsing into an inline `[{ … }]` (`Item::into_value` would
/// happily convert it).
fn set_preserving_decor(target: &mut dyn TableLike, key: &str, mut new_item: Item) {
    let Some(existing) = target.get_mut(key) else {
        target.insert(key, new_item);
        return;
    };

    if existing.is_value() {
        new_item = match new_item.into_value() {
            Ok(value) => Item::Value(value),
            Err(item) => item,
        };
    }
    carry_decor(existing, &mut new_item);
    *existing = new_item;
}

/// Move the formatting of what was in the file onto what replaces it.
///
/// For a plain value that is its spacing and any trailing comment. For a
/// `[[section]]` array it is the comment standing above each entry — which
/// lives in that entry's own decor, so a changed key list would otherwise come
/// back with the owner's notes stripped. Matched up by position, because
/// formatting is formatting: a wrong guess is cosmetic, and there is nothing
/// else to match on.
fn carry_decor(old: &Item, new: &mut Item) {
    match (old, new) {
        (Item::Value(old), Item::Value(new)) => *new.decor_mut() = old.decor().clone(),
        (Item::Table(old), Item::Table(new)) => {
            *new.decor_mut() = old.decor().clone();
            new.set_position(old.position());
        }
        (Item::ArrayOfTables(old), Item::ArrayOfTables(new)) => {
            for (was, is) in old.iter().zip(new.iter_mut()) {
                *is.decor_mut() = was.decor().clone();
                is.set_position(was.position());
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
