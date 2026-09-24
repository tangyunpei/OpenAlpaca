//! `GET|PUT /v1/persona/user` — the status codes the route owns and what
//! each leaves on disk. Driven over `UserDocumentWriter` directly: an
//! `AppState` is not constructible in a unit test, and the handlers are one
//! field read over these two functions.

use super::*;

use axum::body::to_bytes;
use openalpaca_core::bus::EventBus;
use openalpaca_core::events::SystemEvent;
use std::cell::RefCell;
use tempfile::TempDir;

const DOC: &str = "---\ntitle: \"USER.md\"\nsummary: \"u\"\nread_when:\n  - y\n---\n\n\
    ## Identity\n\n* Name: Junpei\n\n## Expertise & Background\n\nRust\n";

struct Fixture {
    dir: TempDir,
    writer: UserDocumentWriter,
    bus: EventBus,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("USER.md"), DOC).unwrap();
        let bus = EventBus::new(16);
        let writer = UserDocumentWriter {
            user_path: dir.path().join("USER.md"),
            backup_dir: dir.path().join("backups"),
            max_backups: Some(2),
            bus: bus.clone(),
        };
        Self { dir, writer, bus }
    }

    fn file(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("USER.md")).unwrap()
    }
}

fn headers_with(if_match: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(value) = if_match {
        headers.insert(header::IF_MATCH, value.parse().unwrap());
    }
    headers
}

fn etag_of(response: &Response) -> Option<String> {
    response
        .headers()
        .get(header::ETAG)
        .map(|v| v.to_str().unwrap().to_string())
}

async fn body_of(response: Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn edited() -> String {
    DOC.replace("Rust", "Rust and Zig")
}

#[tokio::test]
async fn get_answers_the_file_with_its_sha256_as_etag() {
    let f = Fixture::new();
    let response = read_user_document(&f.writer);
    assert_eq!(response.status(), StatusCode::OK);
    let expected = format!("\"{}\"", UserDocumentWriter::sha256(DOC));
    assert_eq!(etag_of(&response), Some(expected));
    assert_eq!(body_of(response).await["content"], DOC);
}

#[tokio::test]
async fn put_without_if_match_is_428_and_the_file_is_unchanged() {
    let f = Fixture::new();
    let swapped = RefCell::new(false);
    let response = replace_user_document(
        &f.writer,
        &headers_with(None),
        PersonaUserBody { content: edited() },
        |_| *swapped.borrow_mut() = true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::PRECONDITION_REQUIRED);
    let body = body_of(response).await;
    assert_eq!(body["error"]["code"], "PERSONA_IF_MATCH_REQUIRED");
    assert_eq!(f.file(), DOC);
    assert!(
        !*swapped.borrow(),
        "nothing was written, nothing is swapped"
    );
}

/// **Plan 3.4's evidence row.** A stale `If-Match` is 409 and the file is
/// unchanged; the response names the current revision so the editor can
/// reload, and its `ETag` is that revision.
#[tokio::test]
async fn put_with_a_stale_if_match_is_409_and_the_file_is_unchanged() {
    let f = Fixture::new();
    let current = UserDocumentWriter::sha256(DOC);
    let stale = UserDocumentWriter::sha256("what the editor was opened on");
    let mut events = f.bus.subscribe();
    let swapped = RefCell::new(false);
    let response = replace_user_document(
        &f.writer,
        &headers_with(Some(&format!("\"{stale}\""))),
        PersonaUserBody { content: edited() },
        |_| *swapped.borrow_mut() = true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(etag_of(&response), Some(format!("\"{current}\"")));
    let body = body_of(response).await;
    assert_eq!(body["error"]["code"], "PERSONA_REVISION_CONFLICT");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.contains(&current), "{message}");
    assert!(message.contains(&stale), "{message}");
    assert_eq!(f.file(), DOC, "the file is unchanged");
    assert!(
        !f.dir.path().join("backups").exists(),
        "no backup was taken"
    );
    assert!(!*swapped.borrow());
    assert!(events.try_recv().is_err(), "nothing was announced");
}

#[tokio::test]
async fn put_with_the_current_if_match_replaces_the_file_announces_and_swaps() {
    let f = Fixture::new();
    let current = UserDocumentWriter::sha256(DOC);
    let mut events = f.bus.subscribe();
    let swapped: RefCell<Option<UserDocument>> = RefCell::new(None);
    let response = replace_user_document(
        &f.writer,
        &headers_with(Some(&format!("\"{current}\""))),
        PersonaUserBody { content: edited() },
        |doc| *swapped.borrow_mut() = Some(doc),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let new_revision = UserDocumentWriter::sha256(&edited());
    assert_eq!(etag_of(&response), Some(format!("\"{new_revision}\"")));
    assert_eq!(body_of(response).await["etag"], new_revision);
    assert_eq!(f.file(), edited());
    // The tool's own path: the previous file is under the backup dir.
    let backups: Vec<_> = std::fs::read_dir(f.dir.path().join("backups"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), DOC);
    // The live copy the prompt reads is the new one.
    assert_eq!(
        swapped.borrow().as_ref().map(|d| d.expertise.clone()),
        Some("Rust and Zig".to_string())
    );
    match events.try_recv() {
        Ok(SystemEvent::UserProfileUpdated {
            actor,
            content_sha256,
            ..
        }) => {
            assert_eq!(actor, "user");
            assert_eq!(content_sha256, new_revision);
        }
        other => panic!("expected UserProfileUpdated, got {other:?}"),
    }
    // A GET after the PUT reads the new revision.
    let after = read_user_document(&f.writer);
    assert_eq!(etag_of(&after), Some(format!("\"{new_revision}\"")));
}

#[tokio::test]
async fn put_with_an_invalid_body_is_400_and_the_file_is_unchanged() {
    let f = Fixture::new();
    let current = UserDocumentWriter::sha256(DOC);
    for bad in ["", "   ", "no frontmatter and no sections"] {
        let response = replace_user_document(
            &f.writer,
            &headers_with(Some(&format!("\"{current}\""))),
            PersonaUserBody {
                content: bad.to_string(),
            },
            |_| panic!("an invalid body must not be swapped in"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{bad:?}");
        assert_eq!(body_of(response).await["error"]["code"], "PERSONA_INVALID");
    }
    assert_eq!(f.file(), DOC);
}

#[test]
fn if_match_reads_the_quoted_bare_and_weak_forms() {
    let sha = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    for form in [
        format!("\"{sha}\""),
        sha.to_string(),
        format!("W/\"{sha}\""),
    ] {
        assert_eq!(
            if_match(&headers_with(Some(&form))).as_deref(),
            Some(sha),
            "{form}"
        );
    }
    assert_eq!(if_match(&headers_with(Some("\"\""))), None);
    assert_eq!(if_match(&headers_with(None)), None);
}
