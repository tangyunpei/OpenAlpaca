use crate::AppState;
use axum::http::StatusCode;
use axum::{Json, extract::State, response::IntoResponse};
use openalpaca_storage::{ConversationRepository, Database, IdentityRepository};
use rand::RngExt;
use rand::distr::Alphanumeric;
use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Serialize)]
pub struct LinkTokenResponse {
    pub token: String,
}

/// POST /v1/auth/link
/// Generate a new link token for the default user (mock for now or from auth)
pub async fn generate_link_token_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let repo = IdentityRepository::new(&state.db);

    // Generate a secure random token
    let token: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(8)
        .map(char::from)
        .collect();
    let token = token.to_uppercase();

    let global_user_id = state.local_user_id.as_str();

    match repo.create_link_token(global_user_id, &token) {
        Ok(_) => (StatusCode::OK, Json(LinkTokenResponse { token })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ── GET /v1/me ────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct MeResponse {
    pub user_id: String,
    pub default_lane_key: String,
    pub sources: Vec<String>,
}

/// Distinct `conversations.source` values across every conversation owned by
/// `owner_id` (GAP-16). Reads chosen: reuse `list_conversations_for_owner`
/// with no source filter and an unbounded limit, then dedupe in-process,
/// rather than adding a new `SELECT DISTINCT` repository method — a desktop
/// assistant's per-user conversation count stays small enough that fetching
/// every row and folding it into a set is simpler than a second query shape,
/// and correct where any capped `LIMIT` would risk missing an older source.
fn distinct_sources_for_owner(db: &Database, owner_id: &str) -> anyhow::Result<Vec<String>> {
    let repo = ConversationRepository::new(db);
    let conversations = repo.list_conversations_for_owner(owner_id, None, i64::MAX, 0)?;
    let sources: BTreeSet<String> = conversations.into_iter().map(|c| c.source).collect();
    Ok(sources.into_iter().collect())
}

/// GET /v1/me — identity + the sources this user has conversations under.
/// Protected route (behind `auth_middleware`); read-only.
pub async fn get_me_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match distinct_sources_for_owner(&state.db, &state.local_user_id) {
        Ok(sources) => Json(MeResponse {
            user_id: state.local_user_id.clone(),
            default_lane_key: state.default_lane_key.clone(),
            sources,
        })
        .into_response(),
        Err(e) => super::api_error(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string())
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("create tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open test db");
        (dir, db)
    }

    #[test]
    fn distinct_sources_for_owner_dedupes_and_sorts() {
        let (_dir, db) = test_db();
        let repo = ConversationRepository::new(&db);
        repo.get_or_create_conversation("alice:gui", "gui").unwrap();
        repo.get_or_create_conversation("alice:telegram", "telegram")
            .unwrap();
        // A second gui conversation must not duplicate the "gui" entry.
        repo.get_or_create_conversation("alice:gui2", "gui").unwrap();

        let sources = distinct_sources_for_owner(&db, "alice").unwrap();
        assert_eq!(sources, vec!["gui".to_string(), "telegram".to_string()]);
    }

    #[test]
    fn distinct_sources_for_owner_excludes_other_owners() {
        let (_dir, db) = test_db();
        let repo = ConversationRepository::new(&db);
        repo.get_or_create_conversation("alice:gui", "gui").unwrap();
        repo.get_or_create_conversation("bob:discord", "discord")
            .unwrap();

        let sources = distinct_sources_for_owner(&db, "alice").unwrap();
        assert_eq!(sources, vec!["gui".to_string()]);
    }

    #[test]
    fn distinct_sources_for_owner_empty_when_no_conversations() {
        let (_dir, db) = test_db();
        let sources = distinct_sources_for_owner(&db, "nobody").unwrap();
        assert!(sources.is_empty());
    }
}
