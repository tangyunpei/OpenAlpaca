use super::*;
use openalpaca_core::events::SystemEvent;

fn update(kind: DocumentKind, hash: &str) -> SystemEvent {
    let actor = "test".to_owned();
    let mode = "replace".to_owned();
    let content_sha256 = hash.to_owned();
    let timestamp = chrono::Utc::now();
    match kind {
        DocumentKind::Soul => SystemEvent::SoulUpdated {
            actor,
            mode,
            content_sha256,
            backup_path: None,
            timestamp,
        },
        DocumentKind::User => SystemEvent::UserProfileUpdated {
            actor,
            mode,
            content_sha256,
            modified_sections: vec![],
            backup_path: None,
            timestamp,
        },
        DocumentKind::Identity => SystemEvent::IdentityUpdated {
            actor,
            mode,
            content_sha256,
            backup_path: None,
            timestamp,
        },
    }
}

#[tokio::test]
async fn every_document_records_only_successfully_applied_updates() {
    for kind in [
        DocumentKind::Soul,
        DocumentKind::User,
        DocumentKind::Identity,
    ] {
        let (tx, rx) = broadcast::channel(4);
        let hashes = new_recent_hashes();
        tx.send(update(kind, "failed")).unwrap();
        tx.send(update(kind, "applied")).unwrap();
        drop(tx);
        let mut calls = 0;
        document_reload_events(rx, kind, hashes.clone(), CancellationToken::new(), || {
            calls += 1;
            if calls == 1 {
                anyhow::bail!("invalid document")
            }
            Ok(())
        })
        .await;
        assert_eq!(calls, 2);
        let mut ring = hashes.lock().await;
        assert!(!consume_hash(&mut ring, "failed"));
        assert!(consume_hash(&mut ring, "applied"));
        assert!(!consume_hash(&mut ring, "applied"));
    }
}

#[tokio::test]
async fn subscriber_ignores_other_documents_and_survives_a_lagged_receiver() {
    let (tx, rx) = broadcast::channel(1);
    tx.send(update(DocumentKind::Soul, "missed")).unwrap();
    tx.send(update(DocumentKind::Identity, "latest")).unwrap();
    drop(tx);
    let hashes = new_recent_hashes();
    let mut calls = 0;
    document_reload_events(
        rx,
        DocumentKind::Identity,
        hashes.clone(),
        CancellationToken::new(),
        || {
            calls += 1;
            Ok(())
        },
    )
    .await;
    assert_eq!(calls, 1);
    assert_eq!(
        hashes.lock().await.front().map(String::as_str),
        Some("latest")
    );

    let (tx, rx) = broadcast::channel(1);
    tx.send(update(DocumentKind::User, "unrelated")).unwrap();
    drop(tx);
    document_reload_events(
        rx,
        DocumentKind::Soul,
        hashes,
        CancellationToken::new(),
        || panic!("a different document must not reload"),
    )
    .await;
}

#[tokio::test]
async fn cancelled_subscriber_exits_while_the_bus_is_still_open() {
    let (_tx, rx) = broadcast::channel(1);
    let cancel = CancellationToken::new();
    cancel.cancel();
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        document_reload_events(rx, DocumentKind::Soul, new_recent_hashes(), cancel, || {
            panic!("cancelled")
        }),
    )
    .await
    .expect("subscriber must exit");
}

#[tokio::test]
async fn watcher_consumes_recorded_bytes_once_and_leaves_external_edits_visible() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("USER.md");
    let hashes = new_recent_hashes();
    std::fs::write(&path, "written by agent").unwrap();
    record_hash(&mut *hashes.lock().await, content_hash("written by agent"));
    assert!(consume_document_write(&hashes, &path).await);
    assert!(!consume_document_write(&hashes, &path).await);
    std::fs::write(&path, "external edit").unwrap();
    assert!(!consume_document_write(&hashes, &path).await);
    assert!(!consume_document_write(&hashes, &dir.path().join("missing")).await);
}

#[test]
fn bounded_ring_keeps_duplicate_writes_as_distinct_events() {
    let mut ring = VecDeque::new();
    record_hash(&mut ring, "oldest".into());
    for _ in 0..OWN_WRITE_RING {
        record_hash(&mut ring, "same".into());
    }
    assert_eq!(ring.len(), OWN_WRITE_RING);
    assert!(!consume_hash(&mut ring, "oldest"));
    for _ in 0..OWN_WRITE_RING {
        assert!(consume_hash(&mut ring, "same"));
    }
    assert!(!consume_hash(&mut ring, "same"));
}
