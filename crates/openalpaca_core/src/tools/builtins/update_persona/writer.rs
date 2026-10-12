//! The `USER.md` replace path a user-directed surface writes through —
//! `PUT /v1/persona/user`, and so `openalpaca persona edit user` and
//! Settings → Persona (design §6 *USER.md*, §7a; owner decision MD-3).
//!
//! It is the same path the model's `update_persona target=user
//! mode=replace` takes: the same validation, `backup_if_exists`,
//! `prune_backups`, `atomic_write`, then `SystemEvent::UserProfileUpdated`
//! — with the actor the caller names, so the audit line says who wrote the
//! file. A read hands back the bytes with their SHA-256, which is the
//! `ETag` the route answers with and the `If-Match` it checks.

use crate::bus::EventBus;
use crate::events::SystemEvent;
use crate::middleware::user::{UserDocument, parse_user_markdown};
use std::path::PathBuf;

use super::common;

/// Where `USER.md` and its backups live, and the bus the write is announced
/// on — the daemon's persona paths, as `PersonaToolContext` carries them.
#[derive(Clone)]
pub struct UserDocumentWriter {
    pub user_path: PathBuf,
    pub backup_dir: PathBuf,
    pub max_backups: Option<usize>,
    pub bus: EventBus,
}

/// The current file, as a reader holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDocumentRead {
    pub content: String,
    /// Lowercase hex SHA-256 of `content`'s bytes.
    pub content_sha256: String,
}

/// What a replace left behind.
#[derive(Debug, Clone)]
pub struct UserDocumentWritten {
    pub content_sha256: String,
    pub backup_path: Option<String>,
    /// The parsed document, for the caller that swaps the live copy.
    pub document: UserDocument,
}

impl UserDocumentWriter {
    /// `update_persona target=user mode=replace`'s validation: not empty,
    /// and it parses as a user document.
    pub fn validate(content: &str) -> Result<UserDocument, String> {
        if content.trim().is_empty() {
            return Err("USER content is empty.".to_string());
        }
        parse_user_markdown(content).map_err(|e| format!("USER validation failed: {e}"))
    }

    /// The SHA-256 the route uses for `ETag` and `If-Match`.
    pub fn sha256(content: &str) -> String {
        common::sha256(content)
    }

    /// The file's current bytes and their hash.
    pub fn read(&self) -> std::io::Result<UserDocumentRead> {
        let content = std::fs::read_to_string(&self.user_path)?;
        Ok(UserDocumentRead {
            content_sha256: common::sha256(&content),
            content,
        })
    }

    /// Validates, backs the current file up, prunes the backups, writes
    /// atomically and publishes `UserProfileUpdated` as `actor`.
    pub async fn replace(&self, content: &str, actor: &str) -> Result<UserDocumentWritten, String> {
        let document = Self::validate(content)?;
        let content_sha256 = common::sha256(content);
        let backup_path =
            common::backup_if_exists(&self.user_path, &self.backup_dir, "USER").await?;
        common::prune_backups(&self.backup_dir, self.max_backups, "USER").await;
        common::atomic_write(&self.user_path, content, ".USER.md.tmp").await?;
        self.bus.publish(SystemEvent::UserProfileUpdated {
            actor: actor.to_string(),
            mode: "replace".to_string(),
            content_sha256: content_sha256.clone(),
            modified_sections: vec!["all".to_string()],
            backup_path: backup_path.clone(),
            timestamp: chrono::Utc::now(),
        });
        Ok(UserDocumentWritten {
            content_sha256,
            backup_path,
            document,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "---\ntitle: \"USER.md\"\nsummary: \"u\"\nread_when:\n  - y\n---\n\n\
        ## Identity\n\n* Name: Junpei\n\n## Expertise & Background\n\nRust\n";

    fn writer(dir: &std::path::Path, bus: EventBus) -> UserDocumentWriter {
        UserDocumentWriter {
            user_path: dir.join("USER.md"),
            backup_dir: dir.join("backups"),
            max_backups: Some(2),
            bus,
        }
    }

    #[tokio::test]
    async fn a_replace_backs_up_writes_atomically_and_announces_the_actor() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("USER.md"), DOC).unwrap();
        let bus = EventBus::new(16);
        let mut events = bus.subscribe();
        let w = writer(dir.path(), bus);

        let before = w.read().unwrap();
        assert_eq!(before.content, DOC);
        assert_eq!(before.content_sha256, UserDocumentWriter::sha256(DOC));

        let edited = DOC.replace("Rust", "Rust and Zig");
        let written = w.replace(&edited, "user").await.unwrap();
        assert_eq!(written.content_sha256, UserDocumentWriter::sha256(&edited));
        assert_eq!(written.document.expertise, "Rust and Zig");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("USER.md")).unwrap(),
            edited
        );
        let backup = written
            .backup_path
            .expect("the previous file was backed up");
        assert_eq!(std::fs::read_to_string(backup).unwrap(), DOC);
        assert!(
            !dir.path().join(".USER.md.tmp").exists(),
            "the temp file is renamed away"
        );

        match events.try_recv() {
            Ok(SystemEvent::UserProfileUpdated {
                actor,
                mode,
                content_sha256,
                ..
            }) => {
                assert_eq!(actor, "user");
                assert_eq!(mode, "replace");
                assert_eq!(content_sha256, written.content_sha256);
            }
            other => panic!("expected UserProfileUpdated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_invalid_document_is_refused_and_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("USER.md"), DOC).unwrap();
        let w = writer(dir.path(), EventBus::new(16));
        assert!(w.replace("   ", "user").await.is_err());
        assert!(
            w.replace("no frontmatter, no sections", "user")
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("USER.md")).unwrap(),
            DOC
        );
        assert!(
            !dir.path().join("backups").exists(),
            "nothing was backed up"
        );
    }
}
