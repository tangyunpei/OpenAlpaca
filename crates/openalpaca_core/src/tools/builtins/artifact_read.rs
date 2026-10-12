//! `artifact_read` — read a by-product back by its id (storage design §14,
//! R41), modelled on `read_result`: the same pages, the same footer shape.
//!
//! A by-product is what `artifact_write` saves — and `file_write` with no
//! project — in OpenAlpaca's artifact store. A result file written into the
//! project is a pointer row (ST-12) that `file_read` reads where it is, so
//! this tool refuses one. The scope is the turn's owner, and a by-product of
//! the turn's project or of no project: another project's is refused, so a
//! conversation never reads into a project it is not in. An upload of the
//! owner's (`origin = upload`) reads like a by-product: it is the owner's own
//! file, and refusing it would guard nothing.

use std::sync::Arc;

use async_trait::async_trait;
use openalpaca_llm::ToolDefinition;
use openalpaca_storage::store::project_root_of;
use openalpaca_storage::{ArtifactError, ArtifactKind, ArtifactStore, Database};

use super::annotations_for_builtin;
use super::artifact_write::scope_from_context;
use super::read_result::{DEFAULT_LIMIT, MAX_LIMIT, page_bounds, read_page_bytes};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolContext};

struct ArtifactReadTool {
    /// Absent (tests, the CLI) refuses at call time, as `artifact_write` does.
    db: Option<Database>,
}

#[async_trait]
impl BuiltInTool for ArtifactReadTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        Err("artifact_read requires execution context — use execute_with_context".to_string())
    }

    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| "artifact_read requires database context".to_string())?;
        let id = arguments
            .get("artifact_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "Missing required parameter: artifact_id".to_string())?;
        let owner = ctx.owner_id.as_deref().ok_or_else(|| {
            "artifact_read cannot read this turn's artifacts: it has no owner.".to_string()
        })?;
        let unreadable = |e: String| format!("artifact_read: '{id}' could not be read: {e}");

        let record = ArtifactStore::new(db)
            .get(id, owner)
            .map_err(|e| unreadable(format!("{e:#}")))?
            .ok_or_else(|| format!("artifact_read: no artifact '{id}' of yours."))?;

        // A by-product of no project is the owner's from any turn, so the
        // turn's project is looked up only for one of a project (S5's M3, as
        // `send` does): that one reads only from a turn whose project — the
        // address artifact_write records for it, `project_root_of` — is its.
        if let Some(root) = record.project_root.as_deref() {
            let turn_project = project_root_of(&scope_from_context(ctx))
                .map_err(|e| unreadable(format!("{e:#}")))?;
            if turn_project.as_deref() != Some(root) {
                return Err(format!(
                    "artifact_read: '{id}' belongs to another project ({root}); read it from a \
                     conversation in that project."
                ));
            }
        }
        if record.in_project {
            let rel = record.rel_path.as_deref().unwrap_or(&record.name);
            return Err(format!(
                "artifact_read: '{id}' is a file in the project ({rel}); read it with file_read."
            ));
        }
        let kind = record
            .kind
            .unwrap_or_else(|| ArtifactKind::for_mime(&record.mime_type));
        if matches!(kind, ArtifactKind::Image | ArtifactKind::Binary) {
            let article = if kind == ArtifactKind::Image { "an" } else { "a" };
            return Err(format!(
                "artifact_read: '{id}' is {article} {} artifact; artifact_read returns text only.",
                kind.as_str()
            ));
        }

        // The store's read: it stats the head and, when the bytes were edited
        // by hand, records them as a version first — so the version reported
        // is the rotated record's, not the one read above. Hashing is
        // blocking work, so the whole call runs on the blocking pool.
        let store_db = db.clone();
        let owned_id = id.to_string();
        let resolved = tokio::task::spawn_blocking(move || {
            ArtifactStore::new(&store_db).resolve_content_with_edit(&owned_id, None)
        })
        .await
        .map_err(|e| unreadable(e.to_string()))?;
        let (path, rotated) = match resolved {
            Ok(found) => found,
            Err(e) => {
                return Err(match e.downcast_ref::<ArtifactError>() {
                    Some(ArtifactError::Gone { path, .. }) => {
                        format!("artifact_read: the bytes of '{id}' are gone ({path}).")
                    }
                    Some(ArtifactError::NotFound { .. }) => {
                        format!("artifact_read: no artifact '{id}' of yours.")
                    }
                    _ => unreadable(format!("{e:#}")),
                });
            }
        };
        let version = rotated.as_ref().map_or(record.version, |r| r.version);

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
        let (buf, total) = read_page_bytes(&path, offset, limit).await.map_err(|_| {
            format!(
                "artifact_read: the bytes of '{id}' are gone ({}).",
                path.display()
            )
        })?;
        let from = (offset as usize).min(total);
        let (start, end) = page_bounds(&buf, limit, from + buf.len() >= total);
        let (page_start, page_end) = (from + start, from + end);

        let mut out = String::from_utf8_lossy(&buf[start..end]).into_owned();
        out.push_str(&format!(
            "\n\n[artifact_read: bytes {page_start}–{page_end} of {total} from {}, version {version}",
            record.name
        ));
        if page_end < total {
            out.push_str(&format!("; next offset={page_end}"));
        }
        out.push(']');
        Ok(out)
    }
}

pub(super) fn artifact_read_tool(db: Option<Database>) -> RegisteredTool {
    RegisteredTool {
        definition: ToolDefinition {
            name: "artifact_read".to_string(),
            description: "Read a by-product back by its id: a draft, a hand-off or another file \
                saved with artifact_write (or with file_write when there is no project). Pages \
                like read_result. A file written into the project is not a by-product — read it \
                with file_read."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "artifact_id": {
                        "type": "string",
                        "description": "The id of a by-product, as artifact_write's result, a workspace entry's file_asset_id or task_status names it."
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Byte offset to start from. Defaults to 0; a partial page tells you the next offset."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum bytes to return. Defaults to 8192, capped at 65536."
                    }
                },
                "required": ["artifact_id"]
            }),
            // `offset` and `limit` are optional, which strict mode cannot
            // express, so the tool is not marked (CLAUDE.md Gotchas).
            strict: None,
            input_examples: Some(vec![serde_json::json!({
                "artifact_id": "0b6f3c2a-6d1e-4c5b-9a8f-2e7d4c1b0a93",
                "offset": 8192,
                "limit": 8192
            })]),
        },
        backend: ToolBackend::BuiltIn(Arc::new(ArtifactReadTool { db })),
        // R42: comes with `read_result`, which every shipped template names,
        // so a seeded install's agents read by-products with no template edit.
        provides_capabilities: vec!["read_result".into(), "artifact_read".into()],
        exempt_from_timeout: false,
        annotations: annotations_for_builtin("artifact_read"),
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::GuardedProject;
    use openalpaca_storage::NewProjectFile;
    use serde_json::json;

    const OWNER: &str = "owner-test";

    /// A guarded home with one registered project, and a temp database.
    struct Fixture {
        gp: GuardedProject,
        _db_dir: tempfile::TempDir,
        db: Database,
    }

    impl Fixture {
        fn new() -> Self {
            let gp = GuardedProject::new();
            let db_dir = tempfile::tempdir().unwrap();
            let db = Database::open(&db_dir.path().join("test.db")).unwrap();
            Self {
                gp,
                _db_dir: db_dir,
                db,
            }
        }

        /// A turn of `OWNER` in the project.
        fn project_ctx(&self) -> ToolContext {
            ToolContext {
                owner_id: Some(OWNER.to_string()),
                ..self.gp.ctx()
            }
        }

        /// A turn of `OWNER` with no project.
        fn scratch_ctx(&self) -> ToolContext {
            ToolContext {
                owner_id: Some(OWNER.to_string()),
                ..self.gp.scratch_ctx()
            }
        }

        /// Save a by-product the way an agent does, and return its id.
        async fn save(&self, ctx: &ToolContext, name: &str, kind: &str, content: &str) -> String {
            let tool = super::super::artifact_write::artifact_write_tool(Some(self.db.clone()), None);
            let out = call(
                &tool,
                ctx,
                json!({"name": name, "kind": kind, "content": content}),
            )
            .await
            .unwrap();
            let v: serde_json::Value = serde_json::from_str(&out).unwrap();
            v["artifact_id"].as_str().unwrap().to_string()
        }

        async fn read(&self, ctx: &ToolContext, args: serde_json::Value) -> Result<String, String> {
            call(&artifact_read_tool(Some(self.db.clone())), ctx, args).await
        }

        fn record(&self, id: &str) -> openalpaca_storage::ArtifactRecord {
            ArtifactStore::new(&self.db).get(id, OWNER).unwrap().unwrap()
        }
    }

    async fn call(
        tool: &RegisteredTool,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<String, String> {
        match &tool.backend {
            ToolBackend::BuiltIn(handler) => handler.execute_with_context(&args, ctx).await,
            _ => panic!("{} is a builtin", tool.definition.name),
        }
    }

    /// R41: a by-product of this project, read back by the id artifact_write
    /// returned — its text, then one footer naming the bytes, the file and
    /// the version.
    #[tokio::test]
    async fn a_by_product_of_this_project_is_read_back_by_its_id() {
        let fx = Fixture::new();
        let body = "# Draft\n\nfirst cut\n";
        let id = fx.save(&fx.project_ctx(), "draft.md", "markdown", body).await;

        let out = fx.read(&fx.project_ctx(), json!({"artifact_id": id})).await.unwrap();

        let n = body.len();
        assert_eq!(
            out,
            format!(
                "{body}\n\n[artifact_read: bytes 0–{n} of {n} from {}, version 1]",
                fx.record(&id).name
            )
        );
    }

    /// Pages like read_result: 8192 bytes by default, a partial page names
    /// the next offset, the last page names none.
    #[tokio::test]
    async fn a_long_by_product_pages_like_read_result() {
        let fx = Fixture::new();
        let body: String = (0..20_000).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        let id = fx.save(&fx.project_ctx(), "long.md", "markdown", &body).await;
        let name = fx.record(&id).name;
        let ctx = fx.project_ctx();

        let first = fx.read(&ctx, json!({"artifact_id": id})).await.unwrap();
        assert!(first.as_bytes().starts_with(&body.as_bytes()[..8192]));
        assert!(first.ends_with(&format!(
            "[artifact_read: bytes 0–8192 of 20000 from {name}, version 1; next offset=8192]"
        )));

        let last = fx
            .read(&ctx, json!({"artifact_id": id, "offset": 19_990, "limit": 100}))
            .await
            .unwrap();
        assert!(last.as_bytes().starts_with(&body.as_bytes()[19_990..]));
        assert!(last.ends_with(&format!(
            "[artifact_read: bytes 19990–20000 of 20000 from {name}, version 1]"
        )));
    }

    /// A by-product saved with no project is in the home store: its owner
    /// reads it from any turn, with a project or without one.
    #[tokio::test]
    async fn a_by_product_saved_with_no_project_is_read_from_any_turn_of_its_owner() {
        let fx = Fixture::new();
        let id = fx.save(&fx.scratch_ctx(), "notes.md", "markdown", "loose notes\n").await;
        assert_eq!(fx.record(&id).project_root, None, "the home store");

        for ctx in [fx.scratch_ctx(), fx.project_ctx()] {
            let out = fx.read(&ctx, json!({"artifact_id": id})).await.unwrap();
            assert!(out.starts_with("loose notes\n\n\n[artifact_read: "), "{out}");
        }
    }

    /// S5's M3: a by-product of no project is read without looking the
    /// turn's project up — from a turn whose project cannot be looked up
    /// (here a root `projects::lookup` cannot resolve: a folder that is not
    /// there, then `..`), it still reads, where it was refused "could not be
    /// read" for a lookup it does not need.
    #[tokio::test]
    async fn a_by_product_of_no_project_is_read_even_when_the_turns_project_cannot_be_looked_up() {
        let fx = Fixture::new();
        let id = fx
            .save(&fx.scratch_ctx(), "notes.md", "markdown", "loose notes\n")
            .await;
        let unresolvable = fx.gp.root.join("no-such-folder").join("..");
        let ctx = ToolContext {
            request_workspace_root: Some(unresolvable.to_string_lossy().into_owned()),
            ..fx.project_ctx()
        };
        assert!(
            project_root_of(&scope_from_context(&ctx)).is_err(),
            "the turn's project cannot be looked up"
        );

        let out = fx.read(&ctx, json!({"artifact_id": id})).await.unwrap();

        assert!(
            out.starts_with("loose notes\n\n\n[artifact_read: "),
            "{out}"
        );
    }

    /// C-S5-3 (S5's M1): a by-product edited by hand reads back as it is on
    /// disk, and the footer names the version the read recorded for the
    /// edit — the rotated record's, not the one looked up before the read.
    #[tokio::test]
    async fn a_by_product_edited_by_hand_reads_back_its_new_bytes_as_version_2() {
        let fx = Fixture::new();
        let id = fx
            .save(&fx.project_ctx(), "draft.md", "markdown", "first cut\n")
            .await;
        let path = fx.record(&id).storage_path;
        let edited = "second cut, edited by hand\n";
        std::fs::write(&path, edited).unwrap();

        let out = fx
            .read(&fx.project_ctx(), json!({"artifact_id": id}))
            .await
            .unwrap();

        let n = edited.len();
        assert_eq!(
            out,
            format!(
                "{edited}\n\n[artifact_read: bytes 0–{n} of {n} from {}, version 2]",
                fx.record(&id).name
            )
        );
    }

    /// Owner-scoped: another owner's id, like an id nobody has, is not found.
    #[tokio::test]
    async fn another_owners_by_product_is_not_found() {
        let fx = Fixture::new();
        let id = fx.save(&fx.project_ctx(), "draft.md", "markdown", "mine\n").await;
        let stranger = ToolContext {
            owner_id: Some("someone-else".to_string()),
            ..fx.project_ctx()
        };

        assert_eq!(
            fx.read(&stranger, json!({"artifact_id": id})).await.unwrap_err(),
            format!("artifact_read: no artifact '{id}' of yours.")
        );
        assert_eq!(
            fx.read(&fx.project_ctx(), json!({"artifact_id": "no-such-id"}))
                .await
                .unwrap_err(),
            "artifact_read: no artifact 'no-such-id' of yours."
        );
    }

    /// A by-product of another project is refused, naming it; from a turn in
    /// that project it reads.
    #[tokio::test]
    async fn a_by_product_of_another_project_is_refused() {
        let fx = Fixture::new();
        let other = tempfile::tempdir().unwrap();
        let other_root = other.path().canonicalize().unwrap();
        openalpaca_storage::store::projects::register(&other_root).unwrap();
        let other_ctx = ToolContext {
            request_workspace_root: Some(other_root.to_string_lossy().into_owned()),
            workspace_id: Some(other_root.to_string_lossy().into_owned()),
            ..fx.project_ctx()
        };
        let id = fx.save(&other_ctx, "plan.md", "markdown", "theirs\n").await;

        assert_eq!(
            fx.read(&fx.project_ctx(), json!({"artifact_id": id})).await.unwrap_err(),
            format!(
                "artifact_read: '{id}' belongs to another project ({}); read it from a \
                 conversation in that project.",
                other_root.display()
            )
        );
        assert!(fx.read(&other_ctx, json!({"artifact_id": id})).await.is_ok());
    }

    /// R41: a result file written into the project (a pointer row, ST-12) is
    /// file_read's, and the refusal names its path in the project. The row is
    /// written by R35's method, never by hand (R39).
    #[tokio::test]
    async fn a_result_file_in_the_project_is_refused_naming_file_read() {
        let fx = Fixture::new();
        std::fs::create_dir_all(fx.gp.root.join("src")).unwrap();
        std::fs::write(fx.gp.root.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
        let root = fx.gp.root.to_string_lossy().into_owned();
        let (row, inserted) = ArtifactStore::new(&fx.db)
            .record_project_file(NewProjectFile {
                owner_id: OWNER,
                project_root: &root,
                rel_path: "src/lib.rs",
                content: b"pub fn a() {}\n",
                task_id: None,
                agent_id: None,
            })
            .unwrap();
        assert!(inserted && row.in_project);

        assert_eq!(
            fx.read(&fx.project_ctx(), json!({"artifact_id": row.id}))
                .await
                .unwrap_err(),
            format!(
                "artifact_read: '{}' is a file in the project (src/lib.rs); read it with file_read.",
                row.id
            )
        );
    }

    /// Text only: an image or a binary by-product is refused, naming its kind.
    #[tokio::test]
    async fn an_image_or_binary_by_product_is_refused_as_not_text() {
        let fx = Fixture::new();
        for (kind, article) in [("image", "an"), ("binary", "a")] {
            let id = fx
                .save(&fx.project_ctx(), &format!("{kind}-thing"), kind, "not text at all")
                .await;
            assert_eq!(
                fx.read(&fx.project_ctx(), json!({"artifact_id": id})).await.unwrap_err(),
                format!(
                    "artifact_read: '{id}' is {article} {kind} artifact; artifact_read returns text only."
                )
            );
        }
    }

    /// A row whose bytes are gone says so, with where they were.
    #[tokio::test]
    async fn a_by_product_whose_bytes_are_gone_says_so() {
        let fx = Fixture::new();
        let id = fx.save(&fx.project_ctx(), "draft.md", "markdown", "soon gone\n").await;
        let path = fx.record(&id).storage_path;
        std::fs::remove_file(&path).unwrap();

        let err = fx.read(&fx.project_ctx(), json!({"artifact_id": id})).await.unwrap_err();

        assert!(
            err.starts_with(&format!("artifact_read: the bytes of '{id}' are gone (")),
            "{err}"
        );
        let file = std::path::Path::new(&path).file_name().unwrap().to_string_lossy().into_owned();
        assert!(err.ends_with(&format!("{file}).")), "{err}");
    }

    /// No owner, no read: every artifact belongs to one.
    #[tokio::test]
    async fn a_turn_with_no_owner_cannot_read_an_artifact() {
        let fx = Fixture::new();
        let ctx = ToolContext {
            owner_id: None,
            ..fx.project_ctx()
        };
        assert_eq!(
            fx.read(&ctx, json!({"artifact_id": "anything"})).await.unwrap_err(),
            "artifact_read cannot read this turn's artifacts: it has no owner."
        );
    }
}
