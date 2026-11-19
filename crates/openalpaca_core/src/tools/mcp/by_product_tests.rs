//! M85: an MCP tool's images, audio and embedded blobs kept as by-products
//! (DU-R6 (b)), through the serializer the registry's MCP arm calls.
//!
//! Every test runs on a guarded home and a temp database (`GuardedProject`):
//! nothing reaches the developer's store.

use std::sync::Arc;

use arc_swap::ArcSwap;
use base64::Engine as _;
use openalpaca_mcp::{CallToolResult, ContentBlock, ResourceContents};
use openalpaca_storage::{ArtifactKind, ArtifactQuery, ArtifactRecord, ArtifactStore, Database};

use super::bridge::{SerializeOptions, serialize_call_result};
use crate::daemon_config::DaemonConfig;
use crate::test_util::GuardedProject;
use crate::tools::builtins::ByProductSink;
use crate::tools::registry::ToolContext;

const TOOL: &str = "srv__shoot";
const OWNER: &str = "owner-test";

/// A guarded home with one project, a database and a sink over it whose
/// `max_artifact_bytes` is `cap`.
struct Fixture {
    gp: GuardedProject,
    _db_dir: tempfile::TempDir,
    db: Database,
    sink: ByProductSink,
}

impl Fixture {
    fn new() -> Self {
        Self::with_cap(1024)
    }

    fn with_cap(cap: u64) -> Self {
        let gp = GuardedProject::new();
        let db_dir = tempfile::tempdir().unwrap();
        let db = Database::open(&db_dir.path().join("test.db")).unwrap();
        let mut config = DaemonConfig::default();
        config.execution.artifacts.max_artifact_bytes = cap;
        let sink = ByProductSink::new(db.clone(), Some(Arc::new(ArcSwap::from_pointee(config))));
        Self {
            gp,
            _db_dir: db_dir,
            db,
            sink,
        }
    }

    /// A turn with no project, of `OWNER`.
    fn ctx(&self) -> ToolContext {
        ToolContext {
            owner_id: Some(OWNER.to_string()),
            ..self.gp.scratch_ctx()
        }
    }

    fn serialize_as(
        &self,
        tool: &str,
        ctx: &ToolContext,
        result: CallToolResult,
    ) -> Result<String, String> {
        serialize_call_result(
            result,
            &SerializeOptions::for_call(tool, Some(&self.sink), Some(ctx)),
        )
    }

    fn serialize(&self, result: CallToolResult) -> Result<String, String> {
        self.serialize_as(TOOL, &self.ctx(), result)
    }

    /// Every by-product of `OWNER` the Library lists.
    fn library(&self) -> Vec<ArtifactRecord> {
        ArtifactStore::new(&self.db)
            .list(&ArtifactQuery::new(OWNER))
            .unwrap()
            .0
    }

    fn record(&self, id: &str) -> ArtifactRecord {
        ArtifactStore::new(&self.db)
            .get(id, OWNER)
            .unwrap()
            .unwrap()
    }
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Bytes that are not UTF-8, so a text-only path could not have kept them.
fn bytes(n: usize, seed: u8) -> Vec<u8> {
    (0..n)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed) | 0x80)
        .collect()
}

fn blob(data: &str, mime: Option<&str>) -> ContentBlock {
    ContentBlock::resource(ResourceContents::BlobResourceContents {
        uri: "file:///report.bin".to_string(),
        mime_type: mime.map(str::to_string),
        blob: data.to_string(),
        meta: None,
    })
}

fn result(content: Vec<ContentBlock>) -> CallToolResult {
    CallToolResult::success(content)
}

/// The artifact id a kept block's line names.
fn id_of(line: &str) -> String {
    let rest = line
        .split_once("saved as artifact \"")
        .unwrap_or_else(|| panic!("not a kept line: {line}"))
        .1;
    rest.split_once('"').unwrap().0.to_string()
}

/// The plan's gate: an image, an audio clip and an embedded blob become three
/// by-products whose bytes equal the decoded input byte for byte; each line
/// names the id, the absolute path, the MIME type and the size; the Library
/// lists the three.
#[test]
fn an_image_an_audio_clip_and_a_blob_are_kept_byte_for_byte() {
    let fx = Fixture::new();
    let png = bytes(300, 1);
    let wav = bytes(200, 2);
    let pdf = bytes(100, 3);
    let out = fx
        .serialize(result(vec![
            ContentBlock::text("here they are"),
            ContentBlock::image(b64(&png), "image/png"),
            ContentBlock::audio(b64(&wav), "audio/wav"),
            blob(&b64(&pdf), Some("application/pdf")),
        ]))
        .unwrap();

    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 4, "{out}");
    assert_eq!(lines[0], "here they are");
    for (line, what, input, mime) in [
        (lines[1], "image", &png, "image/png"),
        (lines[2], "audio", &wav, "audio/wav"),
        (lines[3], "embedded resource", &pdf, "application/pdf"),
    ] {
        let record = fx.record(&id_of(line));
        assert_eq!(
            line,
            format!(
                "[{what} saved as artifact \"{}\" at {} ({mime}, {} bytes)]",
                record.id,
                record.storage_path,
                input.len()
            )
        );
        assert!(std::path::Path::new(&record.storage_path).is_absolute());
        assert_eq!(
            &std::fs::read(&record.storage_path).unwrap(),
            input,
            "{what}"
        );
    }
    assert_eq!(fx.library().len(), 3, "the Library lists the three");
}

/// An audio clip keeps the server's MIME type on its row, as the image does;
/// the line prints the row's.
#[test]
fn the_row_records_the_servers_mime_type() {
    let fx = Fixture::new();
    let out = fx
        .serialize(result(vec![ContentBlock::audio(
            b64(&bytes(10, 4)),
            "audio/wav",
        )]))
        .unwrap();
    let record = fx.record(&id_of(&out));
    assert_eq!(record.mime_type, "audio/wav");
    assert!(out.contains("(audio/wav, 10 bytes)"), "{out}");
}

/// An embedded text resource reaches the model as its text: nothing is saved.
#[test]
fn an_embedded_text_resource_becomes_its_text() {
    let fx = Fixture::new();
    let out = fx
        .serialize(result(vec![ContentBlock::resource(
            ResourceContents::text("the file's words", "file:///notes.txt"),
        )]))
        .unwrap();
    assert_eq!(out, "the file's words");
    assert!(fx.library().is_empty());
}

/// A block over `max_artifact_bytes`, and one that is not base64, save
/// nothing and say why; the call still succeeds.
#[test]
fn an_over_limit_block_and_invalid_base64_save_nothing_and_say_why() {
    let fx = Fixture::with_cap(1024);
    let out = fx
        .serialize(result(vec![
            ContentBlock::image(b64(&bytes(1025, 5)), "image/png"),
            ContentBlock::image("this is not base64!", "image/png"),
        ]))
        .unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0],
        "[image: nothing was saved: the image is 1025 bytes, over the 1024-byte limit for one \
         artifact ([execution.artifacts] max_artifact_bytes)]"
    );
    assert!(
        lines[1].starts_with("[image: nothing was saved: the image's data is not valid base64 ("),
        "{}",
        lines[1]
    );
    assert!(fx.library().is_empty());
}

/// The cap is decided on the real decoded length: one byte under and at the
/// cap are kept, one byte over is refused naming its real size, and a block
/// far over is refused before it is decoded.
#[test]
fn the_cap_is_decided_on_the_decoded_length() {
    let fx = Fixture::with_cap(1024);
    let out = fx
        .serialize(result(vec![
            ContentBlock::image(b64(&bytes(1023, 6)), "image/png"),
            ContentBlock::image(b64(&bytes(1024, 7)), "image/png"),
            ContentBlock::image(b64(&bytes(1025, 8)), "image/png"),
            ContentBlock::image(b64(&bytes(4000, 9)), "image/png"),
        ]))
        .unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert!(
        lines[0].ends_with("(image/png, 1023 bytes)]"),
        "{}",
        lines[0]
    );
    assert!(
        lines[1].ends_with("(image/png, 1024 bytes)]"),
        "{}",
        lines[1]
    );
    assert!(
        lines[2].contains("the image is 1025 bytes, over the 1024-byte limit"),
        "{}",
        lines[2]
    );
    assert!(
        lines[3].contains("the image is at least 4000 bytes, over"),
        "{}",
        lines[3]
    );
    assert_eq!(fx.library().len(), 2);
}

/// A server that shells out to `base64` wraps its lines, and one may leave
/// the padding off: both decode to the same bytes.
#[test]
fn wrapped_and_unpadded_base64_decode() {
    let fx = Fixture::new();
    let input = bytes(301, 10);
    let encoded = b64(&input);
    assert!(encoded.ends_with('='), "the fixture has padding to drop");
    let wrapped: String = encoded
        .as_bytes()
        .chunks(76)
        .map(|line| format!("{}\r\n", std::str::from_utf8(line).unwrap()))
        .collect();
    let unpadded = encoded.trim_end_matches('=').to_string();
    let out = fx
        .serialize(result(vec![
            ContentBlock::image(format!(" {wrapped}\t"), "image/png"),
            ContentBlock::image(unpadded, "image/png"),
        ]))
        .unwrap();
    for line in out.lines() {
        let record = fx.record(&id_of(line));
        assert_eq!(std::fs::read(&record.storage_path).unwrap(), input);
    }
}

/// The server's MIME string is untrusted: only `type/subtype` of token
/// characters is a MIME type (lower-cased, parameters dropped); anything else
/// is none, so the bytes are the store's binary kind.
#[test]
fn an_invalid_mime_string_is_treated_as_none() {
    let fx = Fixture::new();
    let out = fx
        .serialize(result(vec![
            ContentBlock::image(b64(&bytes(8, 11)), "image/png\"><script>"),
            ContentBlock::image(b64(&bytes(8, 12)), "not a mime type"),
            ContentBlock::image(b64(&bytes(8, 13)), "image/png/extra"),
            ContentBlock::image(b64(&bytes(8, 14)), "IMAGE/PNG; name=\"x.exe\""),
        ]))
        .unwrap();
    let records: Vec<ArtifactRecord> = out.lines().map(|l| fx.record(&id_of(l))).collect();
    for record in &records[..3] {
        assert_eq!(record.mime_type, "application/octet-stream", "{record:?}");
        assert_eq!(record.kind, Some(ArtifactKind::Binary));
        assert!(record.name.ends_with(".bin"), "{}", record.name);
    }
    assert_eq!(records[3].mime_type, "image/png");
    assert_eq!(records[3].kind, Some(ArtifactKind::Image));
    assert!(records[3].name.ends_with(".png"), "{}", records[3].name);
}

/// The store's existing rule names each kind (`ArtifactKind::for_mime` and
/// R21's extension sets), as it does for a model-written artifact: a pdf, a
/// tar, an HTML, a Python, a JSON or a CSV blob is openable (the owner's
/// OQ-MCP-HTML); audio, SVG, plain text and a blob with no MIME type are
/// Reveal-only `.bin` files.
#[test]
fn the_kind_and_extension_follow_the_stores_rule() {
    let fx = Fixture::new();
    let cases: [(Option<&str>, ArtifactKind, &str, bool); 11] = [
        (Some("image/png"), ArtifactKind::Image, "png", true),
        (Some("image/svg+xml"), ArtifactKind::Image, "bin", false),
        (Some("audio/mpeg"), ArtifactKind::Binary, "bin", false),
        (Some("application/pdf"), ArtifactKind::Binary, "pdf", true),
        (Some("text/html"), ArtifactKind::Html, "html", true),
        (Some("text/plain"), ArtifactKind::Code, "bin", false),
        (None, ArtifactKind::Binary, "bin", false),
        (Some("application/x-tar"), ArtifactKind::Binary, "tar", true),
        (Some("text/x-python"), ArtifactKind::Code, "py", true),
        (Some("application/json"), ArtifactKind::Code, "json", true),
        (Some("text/csv"), ArtifactKind::Table, "csv", true),
    ];
    let content = cases
        .iter()
        .enumerate()
        .map(|(i, (mime, ..))| blob(&b64(&bytes(16, i as u8)), *mime))
        .collect();
    let out = fx.serialize(result(content)).unwrap();
    for (line, (mime, kind, ext, openable)) in out.lines().zip(cases) {
        let record = fx.record(&id_of(line));
        assert_eq!(record.kind, Some(kind), "{mime:?}");
        assert!(
            record.name.ends_with(&format!(".{ext}")),
            "{mime:?}: {}",
            record.name
        );
        assert_eq!(record.openable(), openable, "{mime:?}");
    }
}

/// The kind follows the server's MIME string on every block type: an image
/// block labelled `text/html` is kept as an openable HTML file
/// (OQ-MCP-HTML).
#[test]
fn an_image_block_labelled_html_is_kept_as_html() {
    let fx = Fixture::new();
    let out = fx
        .serialize(result(vec![ContentBlock::image(
            b64(b"<html><script>alert(1)</script></html>"),
            "text/html",
        )]))
        .unwrap();
    let record = fx.record(&id_of(&out));
    assert_eq!(record.kind, Some(ArtifactKind::Html));
    assert!(record.name.ends_with(".html"), "{}", record.name);
    assert!(record.openable());
}

/// Two images in one result get two ids, and so do two calls of one tool,
/// even under a registry name at the providers' 64-character cap: the call's
/// tag and the block's number are never cut out of the title.
#[test]
fn every_block_of_every_call_is_its_own_artifact() {
    let fx = Fixture::new();
    let long = format!(
        "srv__{}",
        "x".repeat(super::bridge::PROVIDER_TOOL_NAME_MAX - 5)
    );
    assert_eq!(long.len(), super::bridge::PROVIDER_TOOL_NAME_MAX);
    let mut ids = Vec::new();
    for tool in [TOOL, TOOL, long.as_str(), long.as_str()] {
        let out = fx
            .serialize_as(
                tool,
                &fx.ctx(),
                result(vec![
                    ContentBlock::image(b64(&bytes(12, 15)), "image/png"),
                    ContentBlock::image(b64(&bytes(12, 16)), "image/png"),
                ]),
            )
            .unwrap();
        ids.extend(out.lines().map(id_of));
    }
    let mut distinct = ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), 8, "{ids:?}");
    for id in &ids {
        assert_eq!(
            fx.record(id).version,
            1,
            "no block became a version of another"
        );
    }
    assert_eq!(fx.library().len(), 8);
}

/// With no context, no sink or no owner nothing is saved, and the line says
/// so; the call is not failed.
#[test]
fn no_context_no_sink_or_no_owner_saves_nothing_and_says_so() {
    let fx = Fixture::new();
    let image = || result(vec![ContentBlock::image(b64(&bytes(4, 17)), "image/png")]);
    let ctx = fx.ctx();

    let no_ctx = serialize_call_result(
        image(),
        &SerializeOptions::for_call(TOOL, Some(&fx.sink), None),
    );
    assert_eq!(
        no_ctx.as_deref(),
        Ok(
            "[image: nothing was saved: the call carries no tool context, so the bytes have no \
            owner or store]"
        )
    );
    let no_sink =
        serialize_call_result(image(), &SerializeOptions::for_call(TOOL, None, Some(&ctx)));
    assert_eq!(
        no_sink.as_deref(),
        Ok("[image: nothing was saved: no artifact store is wired to this tool registry]")
    );
    assert_eq!(
        serialize_call_result(image(), &SerializeOptions::default()),
        no_ctx,
        "the default options carry neither"
    );
    let ownerless = ToolContext {
        owner_id: None,
        ..fx.ctx()
    };
    assert_eq!(
        fx.serialize_as(TOOL, &ownerless, image()).as_deref(),
        Ok(format!(
            "[image: nothing was saved: {TOOL} cannot save an artifact on this turn: it has no \
             owner context (it was not sent on a person's behalf), and every artifact belongs \
             to an owner.]"
        )
        .as_str()),
        "the store's refusal names the real tool"
    );
    assert!(fx.library().is_empty());
}

/// A kept block is announced once on the turn's bus, so the Library hears
/// it; a block that is not kept announces nothing.
#[test]
fn each_kept_block_is_announced_once() {
    let fx = Fixture::new();
    let bus = crate::bus::EventBus::new(16);
    let mut rx = bus.subscribe();
    let ctx = ToolContext {
        event_bus: Some(bus.clone()),
        ..fx.ctx()
    };
    let out = fx
        .serialize_as(
            TOOL,
            &ctx,
            result(vec![
                ContentBlock::image(b64(&bytes(4, 18)), "image/png"),
                ContentBlock::image("%%%", "image/png"),
                blob(&b64(&bytes(4, 19)), Some("application/zip")),
            ]),
        )
        .unwrap();
    let mut announced = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let crate::events::SystemEvent::ArtifactWritten { artifact_id, .. } = event {
            announced.push(artifact_id);
        }
    }
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(announced, vec![id_of(lines[0]), id_of(lines[2])]);
}

/// A result the server marks as an error keeps its blocks like any other
/// (the entry names no exception) and stays an error.
#[test]
fn an_error_result_keeps_its_blocks_and_stays_an_error() {
    let fx = Fixture::new();
    let err = fx
        .serialize(CallToolResult::error(vec![
            ContentBlock::text("failed; here is the screen"),
            ContentBlock::image(b64(&bytes(4, 20)), "image/png"),
        ]))
        .unwrap_err();
    let line = err.lines().nth(1).unwrap();
    assert_eq!(fx.record(&id_of(line)).size_bytes, 4);
}

/// A by-product of a project turn lands in that project's store folder,
/// never in the project's own folder.
#[test]
fn a_project_turns_block_is_a_by_product_of_that_project() {
    let fx = Fixture::new();
    let ctx = ToolContext {
        owner_id: Some(OWNER.to_string()),
        ..fx.gp.ctx()
    };
    let out = fx
        .serialize_as(
            TOOL,
            &ctx,
            result(vec![ContentBlock::image(b64(&bytes(4, 21)), "image/png")]),
        )
        .unwrap();
    let record = fx.record(&id_of(&out));
    assert_eq!(
        record.project_root.as_deref(),
        Some(fx.gp.root.to_string_lossy().as_ref())
    );
    assert!(!record.in_project);
    assert!(
        !std::path::Path::new(&record.storage_path).starts_with(&fx.gp.root),
        "nothing of OpenAlpaca's own is written in the project's folder"
    );
}
