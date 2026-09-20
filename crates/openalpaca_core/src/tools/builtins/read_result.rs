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
/// just emptied — and, since [`read_page_bytes`] seeks, small enough that it
/// cannot fill the daemon's memory either.
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
        let session_dir = self.root()?.join(crate::session_log::session_dir_name(session_id));
        let results_dir = session_dir.join(crate::session_log::RESULTS_DIR);
        // A session that has never spilled has no directory — that is a
        // missing result, not a broken tool.
        let path =
            match openalpaca_storage::store::confine_to_root(&results_dir, &results_dir.join(&name))
            {
                Ok(path) => path,
                Err(_) => return Err(not_there(&session_dir, &name).await),
            };

        let offset = arguments
            .get("offset")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_LIMIT)
            .clamp(1, MAX_LIMIT);

        // One seek and one bounded read: the page is `limit` bytes wherever it
        // sits in the file, and a 256 MB spill costs the same as a 4 KB one.
        let (buf, total) = match read_page_bytes(&path, offset, limit).await {
            Ok(read) => read,
            // The file was there for `confine_to_root` and is not now, or it
            // cannot be opened at all — answer it the way a miss is answered.
            Err(_) => return Err(not_there(&session_dir, &name).await),
        };
        let from = (offset as usize).min(total);
        let (start, end) = page_bounds(&buf, limit, from + buf.len() >= total);
        let (page_start, page_end) = (from + start, from + end);

        let mut out = String::from_utf8_lossy(&buf[start..end]).into_owned();
        out.push_str(&format!(
            "\n\n[read_result: bytes {page_start}–{page_end} of {total} from {name}"
        ));
        if page_end < total {
            out.push_str(&format!("; next offset={page_end}"));
        }
        out.push(']');
        Ok(out)
    }
}

/// Read at most `limit + 4` bytes from `offset`, plus the file's own size.
///
/// The seek is the point: `offset`/`limit` applied to a file already in RAM
/// bounds the *context*, not the daemon — paging a 256 MB spill (the per-session
/// cap, so a representable size) in 8 KB pages read 256 MB, 32 000 times over.
/// The four extra bytes are what the forward char-boundary snap at the tail
/// needs. `total` comes from this file's metadata rather than a second
/// `metadata()` call, so the size reported is the size that was read from.
pub(super) async fn read_page_bytes(
    path: &std::path::Path,
    offset: u64,
    limit: usize,
) -> std::io::Result<(Vec<u8>, usize)> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let mut file = tokio::fs::File::open(path).await?;
    let total = file.metadata().await?.len();
    let from = offset.min(total);
    file.seek(std::io::SeekFrom::Start(from)).await?;
    let want = (limit as u64).saturating_add(4).min(total - from);
    let mut buf = Vec::with_capacity(want as usize);
    (&mut file).take(want).read_to_end(&mut buf).await?;
    Ok((buf, total as usize))
}

/// The slice of a page buffer to hand back: `[start, end)`, snapped **forward**
/// to char boundaries.
///
/// Forward at both ends, so a page never splits a character and the next page's
/// `offset` — the end this one reports — resumes exactly where it stopped.
/// Nothing between two consecutive pages is lost.
///
/// `at_eof` says whether the buffer reaches the end of the file. When it does
/// not and the forward snap runs out of buffer — only reachable for a
/// caller-supplied offset that is itself mid-character — the end backs off to
/// the previous boundary instead: those bytes lead the next page rather than
/// being handed back as half a character.
fn page_bounds(buf: &[u8], limit: usize, at_eof: bool) -> (usize, usize) {
    let mut start = 0;
    while start < buf.len() && is_continuation(buf[start]) {
        start += 1;
    }
    let mut end = start.saturating_add(limit).min(buf.len());
    while end < buf.len() && is_continuation(buf[end]) {
        end += 1;
    }
    if end == buf.len() && !at_eof && end > start {
        end -= 1;
        while end > start && is_continuation(buf[end]) {
            end -= 1;
        }
    }
    (start, end)
}

/// A UTF-8 continuation byte is `10xxxxxx`; anything else starts a character.
/// Done on raw bytes so a spill that is not valid UTF-8 still pages.
fn is_continuation(byte: u8) -> bool {
    (byte & 0xC0) == 0x80
}

/// The plain refusal: this session has no such spilled result.
fn missing(name: &str) -> String {
    format!("read_result: no spilled result '{name}' for this session")
}

/// The refusal for a reference whose file is not on disk.
///
/// A spill can fail to be written *after* the loop handed the model the stub
/// (a full disk, a permission change): the writer's record then carries
/// `spill_error` and the `spill_ref` it could not honour. Consulting the log
/// costs a scan of the session's segments, which is why it happens only on the
/// miss — and it is the difference between telling the model what happened and
/// telling it the reference it read from its own context does not exist.
async fn not_there(session_dir: &std::path::Path, name: &str) -> String {
    let rel = format!("{}/{name}", crate::session_log::RESULTS_DIR);
    let dir = session_dir.to_path_buf();
    let probe = rel.clone();
    let failure = tokio::task::spawn_blocking(move || {
        crate::session_log::spill_failure(&dir, &probe).ok().flatten()
    })
    .await
    .ok()
    .flatten();
    match failure {
        Some(error) => format!(
            "read_result: the spill for 'file:{rel}' was not written — the session log \
             record's spill_error says: {error}. The first 2 KB are in the transcript; \
             the rest of that result is gone."
        ),
        None => missing(name),
    }
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
