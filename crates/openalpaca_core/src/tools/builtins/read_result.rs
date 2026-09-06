//! `read_result` — page a tool result the session spilled to `results/`
//! (plan §5.4's "Spill, don't truncate").
//!
//! The counterpart to the spill: above `[orchestrator.sessions]
//! tool_result_inline_bytes` the model is handed a stub naming
//! `result_ref=file:results/…`, and this is what turns that reference back
//! into bytes.
//!
//! **Scope is the whole point.** §5.4 specifies "a **scoped** builtin
//! `read_result(result_ref, offset?, limit?)` [that] resolves only inside the
//! current session's `results/` (session id from `ToolContext`; `file_read` is
//! **not** widened to the home root)". So the reference is not a path: it is a
//! single file name under one session's spill directory, and everything else —
//! a `..`, a nested path, an absolute path, another session's reference — is
//! refused before the filesystem is touched. [`confine_to_root`] then catches
//! the one case the grammar cannot: a symlink planted under the sessions root.
//!
//! **Owner decision T15 is pending and implemented as no change**: the tool is
//! registered here and the stub is emitted on every surface, but the
//! `read_result` capability is appended to **no** allowlist. An agent that was
//! not granted it is refused by the gate, fail-closed, like any other.

use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolContext};
use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use std::path::PathBuf;
use std::sync::Arc;

use super::annotations_for_builtin;

/// The default page, in bytes. Big enough to be worth a round trip, small
/// enough that paging into a 200 MB result cannot refill the context the spill
/// just emptied.
const DEFAULT_LIMIT: usize = 8 * 1024;

/// The largest page a single call will return.
const MAX_LIMIT: usize = 64 * 1024;

struct ReadResultTool {
    /// The sessions root, when a caller pins one (tests). Production leaves it
    /// `None` and resolves the home store's `sessions/` at call time, so
    /// registering the tool creates nothing on disk.
    sessions_root: Option<PathBuf>,
}

impl ReadResultTool {
    fn root(&self) -> Result<PathBuf, String> {
        match self.sessions_root {
            Some(ref root) => Ok(root.clone()),
            None => openalpaca_storage::store::sessions_dir()
                .map_err(|e| format!("read_result cannot resolve the session store: {e}")),
        }
    }
}

#[async_trait]
impl BuiltInTool for ReadResultTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("read_result requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let reference = arguments
            .get("result_ref")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: result_ref".to_string())?;
        // The session the call belongs to *is* the scope. Off a session there
        // is no `results/` to read, and falling back to a root would be the
        // widening §5.4 rules out.
        let session_id = ctx.session_id.as_deref().ok_or_else(|| {
            "read_result is scoped to a session, and this call has none".to_string()
        })?;

        let name = result_file_name(reference)?;
        let results_dir = self
            .root()?
            .join(crate::session_log::session_dir_name(session_id))
            .join(crate::session_log::RESULTS_DIR);
        // A session that has never spilled has no directory — that is a
        // missing result, not a broken tool.
        let path = openalpaca_storage::store::confine_to_root(
            &results_dir,
            &results_dir.join(&name),
        )
        .map_err(|_| {
            format!("read_result: no spilled result '{name}' for this session")
        })?;

        let total = tokio::fs::metadata(&path)
            .await
            .map_err(|_| format!("read_result: no spilled result '{name}' for this session"))?
            .len() as usize;

        let offset = arguments
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT);

        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| format!("read_result: failed to read '{name}': {e}"))?;
        let page = page_of(&bytes, offset, limit);
        let end = offset.min(bytes.len()) + page.len();

        let mut out = String::from_utf8_lossy(page).into_owned();
        out.push_str(&format!(
            "\n\n[read_result: bytes {}–{} of {} from {}",
            offset.min(total),
            end,
            total,
            name
        ));
        if end < total {
            out.push_str(&format!("; next offset={end}"));
        }
        out.push(']');
        Ok(out)
    }
}

/// Slice `[offset, offset+limit)`, snapped **forward** to char boundaries.
///
/// Forward at both ends, so a page never splits a character and the next
/// page's `offset` — the end this one reports — resumes exactly where it
/// stopped. Nothing between two consecutive pages is lost.
fn page_of(bytes: &[u8], offset: usize, limit: usize) -> &[u8] {
    let mut start = offset.min(bytes.len());
    while start < bytes.len() && !is_char_boundary(bytes, start) {
        start += 1;
    }
    let mut end = start.saturating_add(limit).min(bytes.len());
    while end < bytes.len() && !is_char_boundary(bytes, end) {
        end += 1;
    }
    &bytes[start..end]
}

/// A UTF-8 continuation byte is `10xxxxxx`; anything else starts a character.
/// Done on raw bytes so a spill that is not valid UTF-8 still pages.
fn is_char_boundary(bytes: &[u8], index: usize) -> bool {
    index == 0 || index >= bytes.len() || (bytes[index] & 0xC0) != 0x80
}

/// Turn `file:results/<name>` (or the bare `results/<name>`) into `<name>`.
///
/// The grammar is deliberately narrow: one file name, directly under this
/// session's `results/`. A path component, a `..`, an absolute path or a
/// reference to anything else is not a result reference and is refused here,
/// before any path is built from it.
fn result_file_name(reference: &str) -> Result<String, String> {
    let rest = reference.strip_prefix("file:").unwrap_or(reference);
    let name = rest
        .strip_prefix(&format!("{}/", crate::session_log::RESULTS_DIR))
        .ok_or_else(|| refusal(reference))?;
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.starts_with('.')
    {
        return Err(refusal(reference));
    }
    Ok(name.to_string())
}

fn refusal(reference: &str) -> String {
    format!(
        "read_result: '{reference}' is not a result reference — pass the \
         result_ref from a spilled tool result, which names one file under \
         this session's results/"
    )
}

pub(super) fn read_result_tool(sessions_root: Option<PathBuf>) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "read_result".to_string(),
            description: "Page a tool result that was too large to return inline. When a \
                result is spilled you are given a stub ending in \
                'result_ref=file:results/<name>'; pass that reference here with an \
                optional byte offset and limit to read the rest. Only results spilled by \
                the current session can be read — this is not a general file reader."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "result_ref": {
                        "type": "string",
                        "description": "The reference from a spilled tool result's stub, e.g. 'file:results/000184-a1b2c3d4-web_fetch.txt'."
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Byte offset to start reading from. Defaults to 0; a partial page tells you the next offset."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum bytes to return. Defaults to 8192, capped at 65536."
                    }
                },
                "required": ["result_ref"]
            }),
            strict: Some(true),
            input_examples: Some(vec![serde_json::json!({
                "result_ref": "file:results/000184-a1b2c3d4-shell_execute.txt",
                "offset": 32768,
                "limit": 8192
            })]),
        },
        backend: ToolBackend::BuiltIn(Arc::new(ReadResultTool { sessions_root })),
        provides_capabilities: vec!["read_result".into()],
        exempt_from_timeout: false,
        annotations: annotations_for_builtin("read_result"),
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }
}
