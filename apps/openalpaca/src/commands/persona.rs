//! `openalpaca persona show|edit user` — `USER.md` from the command line
//! (design §6 *USER.md*, §7a; owner decision MD-3).
//!
//! ```text
//! openalpaca persona show user    # GET /v1/persona/user, the Markdown as the daemon holds it
//! openalpaca persona edit user    # GET → $EDITOR → PUT with If-Match; on 409 prints both
//!                                 # revisions and names the temp file holding your text
//! ```
//!
//! `USER.md` is the owner's to edit: the background extractor stopped
//! writing learned facts into it, so what the file says is what someone put
//! there — the onboarding conversation through `update_persona`, or this.
//! Both verbs ride the two routes; the CLI never touches the file itself,
//! and a concurrent write — a bootstrap turn's `update_persona`, the app's
//! Settings → Persona — shows up as the daemon's `409
//! PERSONA_REVISION_CONFLICT`, never as a silent overwrite. On that 409 the
//! edited text is **kept** in a named temp file rather than thrown away,
//! because an editor session is work.

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand, ValueEnum};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::client::DaemonClient;

#[derive(Args)]
pub struct PersonaArgs {
    #[command(subcommand)]
    pub command: PersonaCommands,
}

#[derive(Subcommand)]
pub enum PersonaCommands {
    /// Print a persona document as the daemon holds it
    Show {
        /// Which document (`user` is USER.md)
        target: PersonaTarget,
    },
    /// Open a persona document in $EDITOR and save it back through the daemon
    Edit {
        /// Which document (`user` is USER.md)
        target: PersonaTarget,
    },
}

/// The documents with a user-directed route. `USER.md` is the one the
/// design gives an editor (MD-3); the value is an enum so `soul` and
/// `identity` are a parse error today rather than a silent `user`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum PersonaTarget {
    /// USER.md — who the daemon is talking to
    User,
}

pub async fn run(args: PersonaArgs) -> Result<()> {
    let client = DaemonClient::connect()?;
    match args.command {
        PersonaCommands::Show {
            target: PersonaTarget::User,
        } => {
            let doc = fetch_user(&client).await?;
            print!("{}", doc.content);
            if !doc.content.ends_with('\n') {
                println!();
            }
            Ok(())
        }
        PersonaCommands::Edit {
            target: PersonaTarget::User,
        } => match edit_user(&client, &open_in_editor).await? {
            EditOutcome::Unchanged => {
                println!("No changes.");
                Ok(())
            }
            EditOutcome::Saved { etag } => {
                println!("{} USER.md saved (revision {}).", "✓".green(), short(&etag));
                Ok(())
            }
            EditOutcome::Conflict {
                ours,
                theirs,
                kept_at,
            } => {
                eprintln!(
                    "{} USER.md changed while you were editing it: you edited revision {}, the daemon now holds {}.",
                    "✗".red(),
                    short(&ours),
                    short(&theirs)
                );
                eprintln!(
                    "Your text is kept at {} — run `openalpaca persona show user` to see the current version, then merge and edit again.",
                    kept_at.display()
                );
                std::process::exit(1);
            }
        },
    }
}

// ── The wire ─────────────────────────────────────────────────────

/// `GET /v1/persona/user`'s body.
#[derive(Debug, Deserialize)]
struct PersonaUserResponse {
    content: String,
}

/// `PUT /v1/persona/user`'s body.
#[derive(Debug, Serialize)]
struct PersonaUserBody<'a> {
    content: &'a str,
}

/// `PUT /v1/persona/user`'s answer.
#[derive(Debug, Deserialize)]
struct PersonaUserWritten {
    etag: String,
}

/// What a `GET` handed back: the Markdown and the revision it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersonaDocument {
    pub content: String,
    /// The `ETag`, unquoted.
    pub etag: String,
}

/// How an edit ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditOutcome {
    /// The editor closed on the same bytes: nothing was sent.
    Unchanged,
    /// Saved; `etag` is the new revision.
    Saved { etag: String },
    /// The daemon's `409`: `ours` is the revision that was edited, `theirs`
    /// the one the daemon holds now, and `kept_at` the file holding the
    /// edited text.
    Conflict {
        ours: String,
        theirs: String,
        kept_at: PathBuf,
    },
}

pub(crate) async fn fetch_user(client: &DaemonClient) -> Result<PersonaDocument> {
    let response = client.get_raw("/v1/persona/user").await?;
    let etag =
        etag_of(&response).context("the daemon answered GET /v1/persona/user without an ETag")?;
    let body: PersonaUserResponse = response
        .json()
        .await
        .context("GET /v1/persona/user: unreadable body")?;
    Ok(PersonaDocument {
        content: body.content,
        etag,
    })
}

/// `GET` → a temp file → `open_editor` → `PUT` with `If-Match`.
///
/// `open_editor` is whatever opens the file and returns once it is closed:
/// `$VISUAL`/`$EDITOR` for a person, a closure for a test.
pub(crate) async fn edit_user(
    client: &DaemonClient,
    open_editor: &dyn Fn(&Path) -> Result<()>,
) -> Result<EditOutcome> {
    let before = fetch_user(client).await?;

    let file = editor_buffer("openalpaca-persona-user-")?;
    std::fs::write(file.path(), &before.content)
        .with_context(|| format!("could not write {}", file.path().display()))?;

    open_editor(file.path())?;

    let edited = std::fs::read_to_string(file.path())
        .with_context(|| format!("could not read {} back", file.path().display()))?;
    if edited == before.content {
        return Ok(EditOutcome::Unchanged);
    }

    let response = client
        .put_raw_with_headers(
            "/v1/persona/user",
            &PersonaUserBody { content: &edited },
            &[("If-Match", format!("\"{}\"", before.etag))],
        )
        .await?;
    match response.status().as_u16() {
        200 => {
            let written: PersonaUserWritten = response
                .json()
                .await
                .context("PUT /v1/persona/user: unreadable body")?;
            Ok(EditOutcome::Saved { etag: written.etag })
        }
        409 => {
            let theirs = etag_of(&response).unwrap_or_else(|| "unknown".to_string());
            // Keep the text: `into_temp_path().keep()` disarms the deleter.
            let kept_at = file
                .into_temp_path()
                .keep()
                .context("could not keep the edited text")?;
            Ok(EditOutcome::Conflict {
                ours: before.etag,
                theirs,
                kept_at,
            })
        }
        _ => Err(crate::client::error_from_response(response).await),
    }
}

/// The unquoted `ETag`, if the response carries one.
fn etag_of(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            let v = v.trim();
            let v = v.strip_prefix("W/").unwrap_or(v);
            v.trim_matches('"').to_string()
        })
        .filter(|v| !v.is_empty())
}

/// The first 12 characters of a revision — enough to tell two apart on a
/// terminal.
fn short(etag: &str) -> String {
    etag.chars().take(12).collect()
}

/// A new file for the editor, `<prefix>….md`, in the store's `state/editor/`
/// (0700, `openalpaca_storage::store::editor_dir`) — never in `$TMPDIR`,
/// where a kept text was out of reach of every cleanup the store has
/// (N-STORE-1). It is removed when the handle drops, or kept with
/// `into_temp_path().keep()` when an edit is refused or conflicts. Shared with
/// `openalpaca memory add|edit`.
pub(crate) fn editor_buffer(prefix: &str) -> Result<tempfile::NamedTempFile> {
    let dir = openalpaca_storage::store::editor_dir()
        .context("could not make the folder for the editor's file")?;
    tempfile::Builder::new()
        .prefix(prefix)
        .suffix(".md")
        .tempfile_in(&dir)
        .with_context(|| {
            format!(
                "could not create a file for the editor in {}",
                dir.display()
            )
        })
}

/// `$VISUAL`, else `$EDITOR`, else `vi`, run through the shell so a value
/// with arguments (`code --wait`) works; the file is passed as `$1`. Shared
/// with `openalpaca memory add|edit`, whose round trip is the same shape.
pub(crate) fn open_in_editor(path: &Path) -> Result<()> {
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".to_string());
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("openalpaca-persona")
        .arg(path)
        .status()
        .with_context(|| format!("could not run the editor `{editor}`"))?;
    if !status.success() {
        bail!("the editor `{editor}` exited with {status}; nothing was saved");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{EnvSandbox, json_answer, serve};
    use clap::Parser;

    #[derive(Parser)]
    struct Harness {
        #[command(subcommand)]
        command: PersonaCommands,
    }

    fn try_parse(args: &[&str]) -> Result<PersonaCommands, clap::Error> {
        Harness::try_parse_from(std::iter::once("openalpaca").chain(args.iter().copied()))
            .map(|h| h.command)
    }

    #[test]
    fn show_and_edit_take_user_and_nothing_else() {
        assert!(matches!(
            try_parse(&["show", "user"]),
            Ok(PersonaCommands::Show {
                target: PersonaTarget::User
            })
        ));
        assert!(matches!(
            try_parse(&["edit", "user"]),
            Ok(PersonaCommands::Edit {
                target: PersonaTarget::User
            })
        ));
        // SOUL.md and IDENTITY.md have no user-directed route (MD-3 gives
        // USER.md the editor), so they are a parse error, not a `user`.
        assert!(try_parse(&["show", "soul"]).is_err());
        assert!(try_parse(&["edit", "identity"]).is_err());
        assert!(try_parse(&["edit"]).is_err());
    }

    const DOC: &str = "---\ntitle: \"USER.md\"\nsummary: \"u\"\nread_when:\n  - y\n---\n\n\
        ## Identity\n\n* Name: Junpei\n";
    const OLD_ETAG: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const NEW_ETAG: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    const OTHER_ETAG: &str = "3333333333333333333333333333333333333333333333333333333333333333";

    fn get_answer() -> String {
        json_answer(
            "200 OK",
            OLD_ETAG,
            &serde_json::json!({ "content": DOC }).to_string(),
        )
    }

    fn append_line(path: &Path) -> Result<()> {
        let mut text = std::fs::read_to_string(path)?;
        text.push_str("\n## Notes\n\nLikes llamas.\n");
        std::fs::write(path, text)?;
        Ok(())
    }

    /// The round trip: GET, the editor's change, PUT carrying `If-Match`
    /// with the fetched revision and the edited text, and the new revision
    /// back. The temp file is gone afterwards.
    #[tokio::test]
    async fn edit_sends_the_edited_text_with_the_fetched_revision_as_if_match() {
        // The editor buffer is made under the store's `state/` (N-STORE-1).
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvSandbox::enter(tmp.path());
        let (addr, server) = serve(vec![
            get_answer(),
            json_answer(
                "200 OK",
                NEW_ETAG,
                &serde_json::json!({ "etag": NEW_ETAG }).to_string(),
            ),
        ]);
        let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");
        let touched = std::sync::Mutex::new(None);
        let editor = |p: &Path| {
            *touched.lock().unwrap() = Some(p.to_path_buf());
            append_line(p)
        };

        let outcome = edit_user(&client, &editor).await.expect("the edit runs");
        assert_eq!(
            outcome,
            EditOutcome::Saved {
                etag: NEW_ETAG.to_string()
            }
        );
        let temp = touched
            .lock()
            .unwrap()
            .clone()
            .expect("the editor was opened");
        assert!(!temp.exists(), "a saved edit leaves no temp file behind");

        let requests = server.join().expect("the server finishes");
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0].starts_with("GET /v1/persona/user "),
            "{}",
            requests[0]
        );
        let put = &requests[1];
        assert!(put.starts_with("PUT /v1/persona/user "), "{put}");
        assert!(
            put.to_ascii_lowercase()
                .contains(&format!("if-match: \"{OLD_ETAG}\"")),
            "the PUT carries the fetched revision as If-Match:\n{put}"
        );
        let body_start = put.find("\r\n\r\n").unwrap() + 4;
        let body: serde_json::Value = serde_json::from_str(&put[body_start..]).unwrap();
        assert!(
            body["content"]
                .as_str()
                .unwrap()
                .ends_with("Likes llamas.\n"),
            "{body}"
        );
        assert!(
            body.get("expected_revision").is_none(),
            "the header, never a body field"
        );
    }

    /// **Plan 3.8's `edit` rule, on `persona edit`.** On the daemon's 409
    /// the edited text is kept in a named temp file, and both revisions are
    /// reported.
    #[tokio::test]
    async fn a_409_keeps_the_edited_text_in_a_named_temp_file() {
        // The editor buffer is made under the store's `state/` (N-STORE-1).
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvSandbox::enter(tmp.path());
        let conflict = serde_json::json!({
            "error": {
                "code": "PERSONA_REVISION_CONFLICT",
                "message": format!("USER.md changed since you opened it: you edited revision {OLD_ETAG}, the current revision is {OTHER_ETAG}.")
            }
        })
        .to_string();
        let (addr, server) = serve(vec![
            get_answer(),
            json_answer("409 Conflict", OTHER_ETAG, &conflict),
        ]);
        let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");

        let outcome = edit_user(&client, &append_line)
            .await
            .expect("a 409 is an outcome, not an error");
        let EditOutcome::Conflict {
            ours,
            theirs,
            kept_at,
        } = outcome
        else {
            panic!("expected a conflict, got {outcome:?}");
        };
        assert_eq!(ours, OLD_ETAG);
        assert_eq!(theirs, OTHER_ETAG);
        assert!(
            kept_at.exists(),
            "the edited text is kept at {}",
            kept_at.display()
        );
        let kept = std::fs::read_to_string(&kept_at).unwrap();
        assert!(kept.ends_with("Likes llamas.\n"), "{kept}");
        assert!(
            kept_at
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("openalpaca-persona-user-"),
            "{}",
            kept_at.display()
        );
        std::fs::remove_file(&kept_at).ok();
        server.join().expect("the server finishes");
    }

    /// An editor that changes nothing sends nothing.
    #[tokio::test]
    async fn an_unchanged_document_is_not_sent() {
        // The editor buffer is made under the store's `state/` (N-STORE-1).
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvSandbox::enter(tmp.path());
        let (addr, server) = serve(vec![get_answer()]);
        let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");
        let outcome = edit_user(&client, &|_: &Path| Ok(())).await.unwrap();
        assert_eq!(outcome, EditOutcome::Unchanged);
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 1, "only the GET");
    }

    /// N-STORE-1 (Z8-03): the file `persona edit user` hands the editor — the
    /// one a conflicting edit keeps — is made in the store's `state/editor/`,
    /// a 0700 folder, never in `$TMPDIR`.
    #[tokio::test]
    async fn the_edit_buffer_is_made_in_the_stores_state_editor_folder_at_0700() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvSandbox::enter(tmp.path());
        let (addr, server) = serve(vec![get_answer()]);
        let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");
        let opened = std::sync::Mutex::new(None);
        let editor = |p: &Path| {
            *opened.lock().unwrap() = Some(p.to_path_buf());
            Ok(())
        };

        let outcome = edit_user(&client, &editor).await.unwrap();
        assert_eq!(outcome, EditOutcome::Unchanged);

        let buffer = opened
            .lock()
            .unwrap()
            .clone()
            .expect("the editor was opened");
        let folder = openalpaca_storage::store::home_root()
            .unwrap()
            .join("state")
            .join("editor");
        assert_eq!(
            buffer.parent(),
            Some(folder.as_path()),
            "{}",
            buffer.display()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&folder).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{}", folder.display());
        }
        server.join().unwrap();
    }

    #[tokio::test]
    async fn show_reads_the_content_and_the_etag() {
        let (addr, server) = serve(vec![get_answer()]);
        let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");
        let doc = fetch_user(&client).await.unwrap();
        assert_eq!(doc.content, DOC);
        assert_eq!(doc.etag, OLD_ETAG);
        server.join().unwrap();
    }
}
