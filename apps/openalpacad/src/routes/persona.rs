//! `GET|PUT /v1/persona/user` — the user-directed `USER.md` editor (design
//! §6 *USER.md*, §7a; owner decisions MD-3 and MD-9; plan 3.4).
//!
//! `USER.md` is reserved for deliberately configured identity and
//! interaction instructions: the background extractor stopped writing
//! learned facts into it, and this is how the owner edits it —
//! `openalpaca persona edit user` and Settings → Persona both ride these two
//! routes.
//!
//! - `GET` answers the raw Markdown as `{"content"}` with
//!   `ETag: "<sha256>"`, the lowercase hex SHA-256 of the file's bytes.
//! - `PUT` takes the new Markdown as `{"content"}` and **`If-Match`** — the
//!   header, never a body field. Absent is `428 PERSONA_IF_MATCH_REQUIRED`.
//!   A value that is not the file's current hash (or `*`, RFC 7232's
//!   any-current-representation) is `409
//!   PERSONA_REVISION_CONFLICT`, the message naming the current revision and
//!   the response carrying it as `ETag`, and the file is unchanged — a
//!   concurrent bootstrap `update_persona` changes the hash, so a stale
//!   editor gets the 409 rather than overwriting it. A body that
//!   `update_persona target=user mode=replace` would refuse is `400
//!   PERSONA_INVALID`. The write is the tool's own path
//!   (`UserDocumentWriter`: backup, prune, atomic write, then
//!   `UserProfileUpdated` with actor `user`), and the route swaps the
//!   daemon's live `user_document` itself, so a `GET` after a `PUT` is
//!   consistent before the file watcher fires. The answer is `{"etag"}`
//!   plus the same `ETag` header.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use openalpaca_core::middleware::user::UserDocument;
use openalpaca_core::tools::builtins::UserDocumentWriter;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::api_error;
use crate::AppState;

/// `PUT /v1/persona/user`'s body.
#[derive(Debug, Deserialize)]
pub struct PersonaUserBody {
    /// The whole document, raw Markdown.
    pub content: String,
}

/// `GET /v1/persona/user`'s body.
#[derive(Debug, Serialize)]
pub struct PersonaUserResponse {
    pub content: String,
}

/// `PUT /v1/persona/user`'s body on success.
#[derive(Debug, Serialize)]
pub struct PersonaUserWritten {
    /// The new revision — the same value as the `ETag` header.
    pub etag: String,
}

/// `GET /v1/persona/user`.
pub async fn get_persona_user_handler(State(state): State<Arc<AppState>>) -> Response {
    read_user_document(&state.persona_user)
}

/// `PUT /v1/persona/user`.
pub async fn put_persona_user_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PersonaUserBody>,
) -> Response {
    replace_user_document(&state.persona_user, &headers, body, |doc| {
        state.orchestrator.update_user_document(Some(doc))
    })
    .await
}

/// The whole `GET`, over the writer alone (an `AppState` is not
/// constructible in a unit test).
pub(crate) fn read_user_document(writer: &UserDocumentWriter) -> Response {
    match writer.read() {
        Ok(read) => with_etag(
            (
                StatusCode::OK,
                Json(PersonaUserResponse {
                    content: read.content,
                }),
            )
                .into_response(),
            &read.content_sha256,
        ),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "PERSONA_UNREADABLE",
            format!("USER.md could not be read: {e}"),
        ),
    }
}

/// The whole `PUT`, over the writer alone. `on_written` receives the parsed
/// document once the file is on disk — the handler swaps the daemon's live
/// copy with it.
pub(crate) async fn replace_user_document(
    writer: &UserDocumentWriter,
    headers: &HeaderMap,
    body: PersonaUserBody,
    on_written: impl FnOnce(UserDocument),
) -> Response {
    let Some(expected) = if_match(headers) else {
        return api_error(
            StatusCode::PRECONDITION_REQUIRED,
            "PERSONA_IF_MATCH_REQUIRED",
            "send If-Match: \"<etag>\" with the revision you edited (GET /v1/persona/user answers it as ETag)",
        );
    };
    // Validate before the revision check so a bad body is refused as a bad
    // body even when the editor is also stale.
    if let Err(e) = UserDocumentWriter::validate(&body.content) {
        return api_error(StatusCode::BAD_REQUEST, "PERSONA_INVALID", e);
    }
    let current = match writer.read() {
        Ok(read) => read.content_sha256,
        Err(e) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "PERSONA_UNREADABLE",
                format!("USER.md could not be read: {e}"),
            );
        }
    };
    // RFC 7232 §3.1: `*` matches any current representation, and a seeded
    // `USER.md` — the read above succeeded — is one.
    if expected != "*" && expected != current {
        return with_etag(
            api_error(
                StatusCode::CONFLICT,
                "PERSONA_REVISION_CONFLICT",
                format!(
                    "USER.md changed since you opened it: you edited revision {expected}, the current revision is {current}. Reload to see the current version — your text is kept."
                ),
            ),
            &current,
        );
    }
    match writer.replace(&body.content, "user").await {
        Ok(written) => {
            on_written(written.document);
            with_etag(
                (
                    StatusCode::OK,
                    Json(PersonaUserWritten {
                        etag: written.content_sha256.clone(),
                    }),
                )
                    .into_response(),
                &written.content_sha256,
            )
        }
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "PERSONA_WRITE_FAILED",
            format!("USER.md could not be written: {e}"),
        ),
    }
}

/// The `If-Match` header's revision: the quoted form (`"<sha>"`), the bare
/// form, and a weak validator (`W/"<sha>"`) all name the same revision.
pub(crate) fn if_match(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::IF_MATCH)?.to_str().ok()?.trim();
    let raw = raw.strip_prefix("W/").unwrap_or(raw);
    let raw = raw.trim_matches('"').trim();
    (!raw.is_empty()).then(|| raw.to_string())
}

fn with_etag(mut response: Response, sha256: &str) -> Response {
    if let Ok(value) = HeaderValue::from_str(&format!("\"{sha256}\"")) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

#[cfg(test)]
mod tests;
