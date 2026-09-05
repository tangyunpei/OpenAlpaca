//! `GET /v1/status` — the store roots and the caller's project (§4.7 item 4).
//!
//! The property under test is that every path comes from
//! `openalpaca_storage::store`, not from a literal joined onto a root, and
//! that `project_root` is the *request's* project — `null` unless the caller
//! actually named one.

use super::*;

use crate::test_util::HomeStoreGuard;
use axum::body::to_bytes;
use tempfile::TempDir;

fn headers_with(path: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(path) = path {
        headers.insert("x-workspace-path", path.parse().unwrap());
    }
    headers
}

async fn body_of(response: Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn reports_the_three_store_roots_from_the_store_module() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().canonicalize().unwrap().join(".openalpaca");
    let _guard = HomeStoreGuard::set(&home);

    let response = status_handler(headers_with(None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_of(response).await;

    assert_eq!(body["home_root"], home.to_string_lossy().as_ref());
    assert_eq!(
        body["state_dir"],
        home.join("state").to_string_lossy().as_ref()
    );
    assert_eq!(
        body["db_path"],
        home.join("state")
            .join("openalpaca.db")
            .to_string_lossy()
            .as_ref()
    );
    // No header, no project — and the key is present, not omitted, so a client
    // can tell "no project" from "old daemon".
    assert!(body.get("project_root").is_some());
    assert!(body["project_root"].is_null());
}

#[tokio::test]
async fn reports_the_project_the_request_names() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let _guard = HomeStoreGuard::set(&root.join("home").join(".openalpaca"));

    let project = root.join("checkout");
    std::fs::create_dir(&project).unwrap();
    std::fs::create_dir(project.join(".git")).unwrap();
    let nested = project.join("crates").join("core");
    std::fs::create_dir_all(&nested).unwrap();

    // A path *inside* the project resolves up to the project root — the same
    // walk a chat turn does, so the answer names where artifacts would land.
    let response = status_handler(headers_with(nested.to_str())).await;
    let body = body_of(response).await;
    assert_eq!(body["project_root"], project.to_string_lossy().as_ref());
}

#[tokio::test]
async fn a_path_that_is_no_project_reports_null() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let _guard = HomeStoreGuard::set(&root.join("home").join(".openalpaca"));

    let loose = root.join("Downloads");
    std::fs::create_dir(&loose).unwrap();

    let body = body_of(status_handler(headers_with(loose.to_str())).await).await;
    assert!(
        body["project_root"].is_null(),
        "a directory with no marker above it is not a project: {body}"
    );
}

/// The T24 carry-over, visible from the route: `$HOME` is not a project, so a
/// header pointing under it reports `null` rather than claiming the whole home
/// directory.
#[tokio::test]
async fn a_path_under_the_home_store_reports_null() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().canonicalize().unwrap().join("home");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(home.join(".openalpaca")).unwrap();
    let _guard = HomeStoreGuard::set(&home.join(".openalpaca"));

    let documents = home.join("Documents");
    std::fs::create_dir(&documents).unwrap();

    let body = body_of(status_handler(headers_with(documents.to_str())).await).await;
    assert!(
        body["project_root"].is_null(),
        "the home store is not a project: {body}"
    );
}
