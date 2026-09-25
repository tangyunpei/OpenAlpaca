//! `memory_files` unit tests: the key derivations (design §2), the note canon
//! (§4), the snapshot (§7a), and — as the partition lands — confinement,
//! modes, `.history`, the mutation protocol with a crash at every step (§6),
//! derived supersession and the management operations.
//!
//! Nothing here touches a real root: every partition lives under a temp
//! directory, and the home store is pointed at one through
//! `OPENALPACA_HOME_STORE` for the duration of the test.

use super::*;
use chrono::{TimeZone, Utc};

// ============================================================================
// Keys (design §2; the literal values are the fixtures README's)
// ============================================================================

#[test]
fn owner_key_is_first_32_hex_of_sha256_over_utf8() {
    assert_eq!(
        owner_key("u_example_alice"),
        "32acea7eb739b08afd959db91e46d450"
    );
    assert_eq!(
        owner_key("u_example_bob"),
        "5c357145f3a583c080c9f1d8ee520e71"
    );
    assert_eq!(owner_key("system"), "bbc5e661e106c6dcd8dc6dd186454c2f");
    assert_eq!(
        owner_key("u_example_alice_reborn"),
        "9bfb77d3249642b6ba22fe7dcade07a0"
    );
    assert_eq!(owner_key("").len(), OWNER_KEY_HEX);
    assert_eq!(owner_key("用户").len(), OWNER_KEY_HEX);
}

/// Design 09 §2.5: the change test compares a note's `## Knowledge`
/// section — or, for a note learned before L2, the body below its title.
#[test]
fn knowledge_text_reads_the_section_or_the_body_below_the_title() {
    let sectioned = "# Editor preference\n\n## Knowledge\nSince June the user edits with Helix.\nBefore that, vim.\n\n## Evidence and limits\nFrom what you said on 2026-09-24.\n";
    assert_eq!(
        knowledge_text(sectioned),
        "Since June the user edits with Helix.\nBefore that, vim."
    );
    assert_eq!(
        knowledge_text("# Editor preference\n\nThe user prefers vim.\n"),
        "The user prefers vim."
    );
    assert_eq!(knowledge_text("no title, just text"), "no title, just text");
    assert_eq!(
        knowledge_text("# T\n\n### knowledge\nnot the section\n"),
        "### knowledge\nnot the section"
    );
}

/// NFKC, case-fold, whitespace collapsed, trailing punctuation removed.
#[test]
fn normalize_statement_ignores_case_spacing_width_and_trailing_punctuation() {
    assert_eq!(
        normalize_statement("The user  edits\nwith VIM."),
        normalize_statement("the user edits with vim")
    );
    assert_eq!(
        normalize_statement("用户使用ＶＩＭ。"),
        normalize_statement("用户使用vim")
    );
    assert_ne!(
        normalize_statement("The user edits with vim."),
        normalize_statement("The user edits with Helix.")
    );
}

#[test]
fn partition_keys_have_the_design_shape() {
    let owner = owner_key("u_example_alice");
    assert_eq!(
        home_partition_key(&owner),
        "home-32acea7eb739b08afd959db91e46d450"
    );
    let dir = "/tmp/x/.openalpaca/memory/owners/32acea7eb739b08afd959db91e46d450";
    let key = project_partition_key(&owner, dir);
    assert!(key.starts_with("proj-32acea7eb739b08afd959db91e46d450-"));
    assert_eq!(key.len(), "proj-".len() + OWNER_KEY_HEX + 1 + DIR_KEY_HEX);
    assert_eq!(&key["proj-".len() + OWNER_KEY_HEX + 1..], dir_key(dir));
    assert_eq!(dir_key(dir), &sha256_hex(dir.as_bytes())[..DIR_KEY_HEX]);
    // Two different directories, two different keys; the same one, the same.
    assert_ne!(
        dir_key(dir),
        dir_key("/tmp/y/.openalpaca/memory/owners/abc")
    );
    assert_eq!(project_partition_key(&owner, dir), key);
}

#[test]
fn revision_is_lowercase_hex_sha256_of_exact_bytes() {
    let rev = revision_of(b"abc");
    assert_eq!(
        rev,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert!(is_revision(&rev));
    assert!(!is_revision(&rev.to_uppercase()));
    assert!(!is_revision("abc"));
}

// ============================================================================
// The note canon (design §4)
// ============================================================================

/// The design §4 example, verbatim.
const EXAMPLE: &str = "---
schema: 1
id: 55e811b6-3220-4758-a47b-034bd1023f38
title: Build and test commands for this workspace
kind: procedure
status: active
fact_key: build.validation
aliases:
  - checks
  - validation
  - 测试
  - 编译
tags:
  - development
created_at: 2026-09-23T10:00:00Z
updated_at: 2026-09-23T10:00:00Z
observed_at: 2026-09-23T09:55:00Z
valid_from: null
valid_to: null
review_after: null
pinned: false
sources:
  - type: task
    id: 7c1e0b2a-4d61-4f0e-9a3c-2b7f1d5e8c90
    observed_at: 2026-09-23T09:55:00Z
supersedes: []
---
# Build and test commands for this workspace

## Knowledge
Run the documented focused checks for the component being changed.

## Evidence and limits
The cited run checked one component. It did not certify the entire application.

## Recheck when
The build configuration or test entry points change.
";

fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
}

#[test]
fn design_example_round_trips_byte_identically() {
    let note = Note::parse(EXAMPLE.as_bytes()).expect("the design example parses");
    assert_eq!(note.id.to_string(), "55e811b6-3220-4758-a47b-034bd1023f38");
    assert_eq!(note.title, "Build and test commands for this workspace");
    assert_eq!(note.kind, NoteKind::Procedure);
    assert_eq!(note.status, NoteStatus::Active);
    assert_eq!(note.fact_key.as_deref(), Some("build.validation"));
    assert_eq!(note.aliases, ["checks", "validation", "测试", "编译"]);
    assert_eq!(note.tags, ["development"]);
    assert_eq!(note.created_at, ts(2026, 9, 23, 10, 0, 0));
    assert_eq!(note.observed_at, Some(ts(2026, 9, 23, 9, 55, 0)));
    assert_eq!(note.valid_from, None);
    assert!(!note.pinned);
    assert_eq!(note.sources.len(), 1);
    assert_eq!(note.sources[0].kind, SourceType::Task);
    assert_eq!(note.sources[0].id, "7c1e0b2a-4d61-4f0e-9a3c-2b7f1d5e8c90");
    assert_eq!(note.sources[0].observed_at, Some(ts(2026, 9, 23, 9, 55, 0)));
    assert!(note.supersedes.is_empty());
    assert!(note.unknown.is_empty());
    assert!(note.body.starts_with("# Build and test commands"));
    assert_eq!(
        note.serialize(),
        EXAMPLE,
        "parse then write is byte-identical"
    );
    assert_eq!(note.revision(), revision_of(EXAMPLE.as_bytes()));
    assert_eq!(
        note.file_name(),
        "55e811b6-3220-4758-a47b-034bd1023f38--build-and-test-commands-for-this-workspace.md"
    );
}

#[test]
fn unknown_keys_are_preserved_verbatim_after_the_known_ones() {
    let text = "---
schema: 1
id: 55e811b6-3220-4758-a47b-034bd1023f38
title: T
kind: fact
status: active
fact_key: null
aliases: []
tags: []
created_at: 2026-09-23T10:00:00Z
updated_at: 2026-09-23T10:00:00Z
observed_at: null
valid_from: null
valid_to: null
review_after: null
pinned: false
sources: []
supersedes: []
x_custom: some value
x_block:
  - one
  - two
x_map:
  nested: 1
  other: [a, b]
---
body
";
    let note = Note::parse(text.as_bytes()).unwrap();
    assert_eq!(
        note.unknown
            .iter()
            .map(|u| u.key.as_str())
            .collect::<Vec<_>>(),
        ["x_custom", "x_block", "x_map"]
    );
    assert_eq!(note.unknown[1].raw, "x_block:\n  - one\n  - two\n");
    assert_eq!(note.serialize(), text);
}

#[test]
fn unknown_keys_written_before_known_ones_move_after_them_once() {
    let text = "---
x_first: 1
schema: 1
id: 55e811b6-3220-4758-a47b-034bd1023f38
title: T
kind: fact
status: active
created_at: 2026-09-23T10:00:00Z
updated_at: 2026-09-23T10:00:00Z
---
body
";
    let note = Note::parse(text.as_bytes()).unwrap();
    let canon = note.serialize();
    assert!(canon.ends_with("supersedes: []\nx_first: 1\n---\nbody\n"));
    // The second pass is stable.
    let again = Note::parse(canon.as_bytes()).unwrap();
    assert_eq!(again.serialize(), canon);
    assert_eq!(again, note);
}

#[test]
fn cjk_aliases_and_title_round_trip() {
    let mut note = Note::parse(EXAMPLE.as_bytes()).unwrap();
    note.title = "构建与测试命令 — für die Werkstatt".to_string();
    note.aliases = vec!["测试".into(), "编译".into(), "🚀 launch".into()];
    note.body = "# 标题\n\n正文。\n".to_string();
    let canon = note.serialize();
    let again = Note::parse(canon.as_bytes()).unwrap();
    assert_eq!(again, note);
    assert_eq!(again.serialize(), canon);
    assert!(canon.contains("title: 构建与测试命令 — für die Werkstatt\n"));
    assert!(canon.contains("  - 🚀 launch\n"));
}

#[test]
fn comments_are_dropped_and_the_result_is_stable() {
    let text = "---
# a comment before everything
schema: 1
id: 55e811b6-3220-4758-a47b-034bd1023f38
title: T # trailing comment
kind: fact
status: active
aliases:
  # inside a block
  - a
created_at: 2026-09-23T10:00:00Z
updated_at: 2026-09-23T10:00:00Z
---
body # not a comment: the body is bytes
";
    let note = Note::parse(text.as_bytes()).unwrap();
    assert_eq!(note.title, "T");
    assert_eq!(note.aliases, ["a"]);
    let canon = note.serialize();
    assert!(!canon.contains("comment before"));
    assert!(!canon.contains("inside a block"));
    assert!(canon.ends_with("---\nbody # not a comment: the body is bytes\n"));
    assert_eq!(Note::parse(canon.as_bytes()).unwrap().serialize(), canon);
}

#[test]
fn scalars_that_yaml_would_misread_are_quoted_and_read_back() {
    let mut note = Note::parse(EXAMPLE.as_bytes()).unwrap();
    for title in [
        "null",
        "- leading dash",
        "key: value",
        "a # hash",
        "  padded  ",
        "\"quoted\"",
        "tab\there",
        "multi\nline",
        "[flow]",
        "@at",
        "../../../etc/passwd",
        "42",
        "true",
    ] {
        note.title = title.to_string();
        note.aliases = vec![title.to_string()];
        let canon = note.serialize();
        let again = Note::parse(canon.as_bytes()).unwrap_or_else(|e| panic!("{title:?}: {e}"));
        assert_eq!(again.title, title, "title {title:?}");
        assert_eq!(again.aliases, [title], "alias {title:?}");
        assert_eq!(again.serialize(), canon);
    }
    // Plain where YAML reads it back unchanged.
    note.title = "42".into();
    assert!(note.serialize().contains("title: 42\n"));
    note.title = "../../../etc/passwd".into();
    assert!(note.serialize().contains("title: ../../../etc/passwd\n"));
}

#[test]
fn single_quoted_and_flow_lists_are_read() {
    let text = "---
schema: 1
id: 55e811b6-3220-4758-a47b-034bd1023f38
title: 'it''s quoted'
kind: fact
status: active
aliases: [a, \"b, c\", 'd']
tags:
- no-indent
- item
created_at: 2026-09-23T10:00:00+02:00
updated_at: 2026-09-23T10:00:00.5Z
pinned: True
---
";
    let note = Note::parse(text.as_bytes()).unwrap();
    assert_eq!(note.title, "it's quoted");
    assert_eq!(note.aliases, ["a", "b, c", "d"]);
    assert_eq!(note.tags, ["no-indent", "item"]);
    assert_eq!(
        note.created_at,
        ts(2026, 9, 23, 8, 0, 0),
        "offset normalized to UTC"
    );
    assert!(note.pinned);
    assert_eq!(note.body, "");
    let canon = note.serialize();
    assert!(canon.contains("created_at: 2026-09-23T08:00:00Z\n"));
    assert!(
        canon.contains("updated_at: 2026-09-23T10:00:00Z\n"),
        "sub-seconds dropped"
    );
}

#[test]
fn malformed_files_are_refused_with_a_reason() {
    let base = |patch: &dyn Fn(&mut String)| {
        let mut s = EXAMPLE.to_string();
        patch(&mut s);
        s
    };
    let cases: Vec<(&str, String)> = vec![
        ("no frontmatter", "# just markdown\n".to_string()),
        ("no closing", "---\nschema: 1\nid: x\n".to_string()),
        (
            "missing id",
            base(&|s| *s = s.replace("id: 55e811b6-3220-4758-a47b-034bd1023f38\n", "")),
        ),
        (
            "path id",
            base(&|s| {
                *s = s.replace(
                    "id: 55e811b6-3220-4758-a47b-034bd1023f38",
                    "id: ../../state/memory-control/home.json",
                )
            }),
        ),
        (
            "uppercase id",
            base(&|s| {
                *s = s.replace(
                    "id: 55e811b6-3220-4758-a47b-034bd1023f38",
                    "id: 55E811B6-3220-4758-A47B-034BD1023F38",
                )
            }),
        ),
        (
            "duplicate key",
            base(&|s| *s = s.replace("kind: procedure\n", "kind: procedure\nkind: fact\n")),
        ),
        (
            "bad kind",
            base(&|s| *s = s.replace("kind: procedure", "kind: memo")),
        ),
        (
            "bad status",
            base(&|s| *s = s.replace("status: active", "status: superseded")),
        ),
        (
            "bad timestamp",
            base(&|s| *s = s.replace("created_at: 2026-09-23T10:00:00Z", "created_at: yesterday")),
        ),
        (
            "bad schema",
            base(&|s| *s = s.replace("schema: 1", "schema: 2")),
        ),
        (
            "bad supersedes",
            base(&|s| *s = s.replace("supersedes: []", "supersedes:\n  - not-a-uuid")),
        ),
        (
            "empty title",
            base(&|s| {
                *s = s.replace(
                    "title: Build and test commands for this workspace",
                    "title: \"\"",
                )
            }),
        ),
        (
            "unknown source field",
            base(&|s| *s = s.replace("    id: 7c1e0b2a", "    who: me\n    id: 7c1e0b2a")),
        ),
        (
            "not utf8",
            String::from_utf8_lossy(&[0xff, 0xfe]).into_owned(),
        ),
    ];
    for (name, text) in cases {
        let bytes = if name == "not utf8" {
            vec![b'-', b'-', b'-', b'\n', 0xff, 0xfe]
        } else {
            text.into_bytes()
        };
        let result = Note::parse(&bytes);
        assert!(result.is_err(), "{name} must be malformed");
        let reason = result.unwrap_err().reason;
        assert!(!reason.is_empty(), "{name} names a reason");
    }
}

#[test]
fn oversized_files_are_refused_at_parse_and_at_write() {
    let mut note = Note::parse(EXAMPLE.as_bytes()).unwrap();
    note.body = "x".repeat(MAX_NOTE_BYTES + 1);
    let bytes = note.serialize();
    let e = Note::parse(bytes.as_bytes()).unwrap_err();
    assert!(e.reason.contains("oversized"), "{e}");
    let e = note.validate_for_write().unwrap_err();
    assert!(e.contains("32"), "{e}");
    // Exactly at the limit is fine.
    let header = note.serialize().len() - note.body.len();
    note.body = "x".repeat(MAX_NOTE_BYTES - header);
    assert_eq!(note.serialize().len(), MAX_NOTE_BYTES);
    assert!(note.validate_for_write().is_ok());
    assert!(Note::parse(note.serialize().as_bytes()).is_ok());
    note.body.push('x');
    assert!(note.validate_for_write().is_err());
}

#[test]
fn invalid_fact_key_in_a_file_is_absent_and_refused_on_write() {
    let text = EXAMPLE.replace("fact_key: build.validation", "fact_key: Build Validation!");
    let note = Note::parse(text.as_bytes()).unwrap();
    assert_eq!(
        note.fact_key, None,
        "treated as absent when found in a file"
    );
    let mut note = Note::parse(EXAMPLE.as_bytes()).unwrap();
    note.fact_key = Some("Build".into());
    assert!(note.validate_for_write().unwrap_err().contains("fact_key"));
    for ok in ["a", "build.validation", "a-b_c.d", &"a".repeat(64)] {
        assert!(is_valid_fact_key(ok), "{ok}");
    }
    for bad in ["", ".a", "-a", "A", "a b", &"a".repeat(65), "a/b"] {
        assert!(!is_valid_fact_key(bad), "{bad}");
    }
}

#[test]
fn write_limits_are_enforced() {
    let mut note = Note::parse(EXAMPLE.as_bytes()).unwrap();
    note.title = "t".repeat(MAX_TITLE_CHARS + 1);
    assert!(note.validate_for_write().unwrap_err().contains("title"));
    note.title = "ok".into();
    note.aliases = vec!["a".into(); MAX_LIST_ENTRIES + 1];
    assert!(note.validate_for_write().unwrap_err().contains("aliases"));
    note.aliases.clear();
    note.sources = vec![note.sources[0].clone(); MAX_SOURCES + 1];
    assert!(note.validate_for_write().unwrap_err().contains("sources"));
}

#[test]
fn file_names_follow_the_grammar() {
    let id = NoteId::parse("55e811b6-3220-4758-a47b-034bd1023f38").unwrap();
    assert_eq!(
        Note::file_name_for(id, "../../../etc/passwd"),
        "55e811b6-3220-4758-a47b-034bd1023f38--etc-passwd.md"
    );
    assert_eq!(
        Note::file_name_for(id, "   "),
        "55e811b6-3220-4758-a47b-034bd1023f38--note.md"
    );
    assert_eq!(
        Note::file_name_for(id, "测试"),
        "55e811b6-3220-4758-a47b-034bd1023f38--note.md"
    );
    let long = Note::file_name_for(id, &"word ".repeat(40));
    let slug = long
        .trim_end_matches(".md")
        .split_once("--")
        .unwrap()
        .1
        .to_string();
    assert!(slug.len() <= SLUG_MAX_BYTES, "{slug}");
    assert_eq!(Note::id_from_file_name(&long), Some(id));
    assert_eq!(Note::id_from_file_name("readme.md"), None);
    assert_eq!(Note::id_from_file_name("not-a-uuid--x.md"), None);
}

#[test]
fn note_ids_are_canonical_only() {
    assert!(NoteId::parse("55e811b6-3220-4758-a47b-034bd1023f38").is_ok());
    assert!(NoteId::parse("55E811B6-3220-4758-A47B-034BD1023F38").is_err());
    assert!(NoteId::parse("55e811b632204758a47b034bd1023f38").is_err());
    assert!(NoteId::parse("../x").is_err());
    let fresh = NoteId::new();
    assert_eq!(NoteId::parse(&fresh.to_string()).unwrap(), fresh);
}

#[test]
fn kinds_default_scopes_per_rule_2() {
    assert_eq!(NoteKind::Preference.default_scope(), NoteScope::Global);
    for k in [
        NoteKind::Fact,
        NoteKind::Decision,
        NoteKind::Procedure,
        NoteKind::Reference,
        NoteKind::Episode,
    ] {
        assert_eq!(k.default_scope(), NoteScope::Project, "{k}");
    }
}

#[test]
fn a_patch_touches_only_what_it_names() {
    let note = Note::parse(EXAMPLE.as_bytes()).unwrap();
    let mut patched = note.clone();
    NotePatch {
        aliases: Some(vec!["vi".into()]),
        ..Default::default()
    }
    .apply(&mut patched);
    assert_eq!(patched.aliases, ["vi"]);
    assert_eq!(patched.body, note.body);
    assert_eq!(patched.title, note.title);
    let mut cleared = note.clone();
    NotePatch {
        fact_key: Some(None),
        ..Default::default()
    }
    .apply(&mut cleared);
    assert_eq!(cleared.fact_key, None);
    assert!(NotePatch::default().is_empty());
}

// ============================================================================
// The control snapshot (design §7a)
// ============================================================================

/// The design §7a example, verbatim.
const SNAPSHOT_EXAMPLE: &str = r#"{
  "schema": 1,
  "partition_key": "proj-3f9a1c2d4e5b6a7f8091a2b3c4d5e6f7-0a1b2c3d4e5f6071",
  "scope": "project",
  "owner_key": "3f9a1c2d4e5b6a7f8091a2b3c4d5e6f7",
  "owner_id": "u_01J8Z3Q9K7X2M4N6P8R0S2T4V6",
  "root": "/Users/x/dev/app/.openalpaca/memory/owners/3f9a1c2d4e5b6a7f8091a2b3c4d5e6f7",
  "created_at": "2026-09-23T10:00:00Z",
  "updated_at": "2026-09-23T10:05:00Z",
  "moved_from": null,
  "repaired_at": null,
  "pending": null,
  "notes": {
    "55e811b6-3220-4758-a47b-034bd1023f38": {
      "revision": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
      "origin": "extracted_user",
      "written_at": "2026-09-23T10:05:00Z"
    }
  },
  "tombstones": [
    {
      "id": "0c7b2e4a-9d13-4c8e-b1f5-6a2d3e4f5a6b",
      "origin": "user",
      "supersedes": ["d4e5f6a7-b8c9-4d0e-9f1a-2b3c4d5e6f70"],
      "suppressed_sources": ["message:4812", "task:7c1e0b2a-4d61-4f0e-9a3c-2b7f1d5e8c90"],
      "forgotten_at": "2026-09-23T10:02:00Z"
    }
  ]
}
"#;

#[test]
fn snapshot_example_parses_and_reserializes_with_the_design_keys_in_order() {
    let snapshot: ControlSnapshot = serde_json::from_str(SNAPSHOT_EXAMPLE).unwrap();
    assert_eq!(snapshot.schema, SNAPSHOT_SCHEMA);
    assert_eq!(snapshot.scope, SnapshotScope::Project);
    assert_eq!(snapshot.notes.len(), 1);
    let id = NoteId::parse("55e811b6-3220-4758-a47b-034bd1023f38").unwrap();
    assert_eq!(snapshot.notes[&id].origin, Origin::ExtractedUser);
    assert_eq!(snapshot.tombstones[0].suppressed_sources.len(), 2);
    assert!(snapshot.is_source_suppressed("message:4812"));
    assert!(!snapshot.is_source_suppressed("message:4813"));
    let json = snapshot.to_json();
    let again: ControlSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(again, snapshot);
    let keys: Vec<&str> = json
        .lines()
        .filter(|l| l.starts_with("  \""))
        .map(|l| l.trim().split('"').nth(1).unwrap())
        .collect();
    assert_eq!(
        keys,
        [
            "schema",
            "partition_key",
            "scope",
            "owner_key",
            "owner_id",
            "root",
            "created_at",
            "updated_at",
            "moved_from",
            "repaired_at",
            "pending",
            "notes",
            "tombstones"
        ]
    );
}

#[test]
fn snapshot_unknown_fields_and_schemas_are_invalid() {
    let with_extra =
        SNAPSHOT_EXAMPLE.replace("\"schema\": 1,", "\"schema\": 1,\n  \"extra\": true,");
    assert!(serde_json::from_str::<ControlSnapshot>(&with_extra).is_err());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.json");
    std::fs::write(
        &path,
        SNAPSHOT_EXAMPLE.replace("\"schema\": 1", "\"schema\": 7"),
    )
    .unwrap();
    assert!(matches!(
        ControlSnapshot::load(&path),
        Err(SnapshotError::Invalid(_))
    ));
    std::fs::write(&path, "{ truncated").unwrap();
    assert!(matches!(
        ControlSnapshot::load(&path),
        Err(SnapshotError::Invalid(_))
    ));
    std::fs::remove_file(&path).unwrap();
    assert!(ControlSnapshot::load(&path).unwrap().is_none());
}

#[test]
fn snapshot_save_is_atomic_private_and_compacts() {
    let dir = tempfile::tempdir().unwrap();
    let control = dir.path().join("memory-control");
    let path = control.join("home-abc.json");
    let mut snapshot: ControlSnapshot = serde_json::from_str(SNAPSHOT_EXAMPLE).unwrap();
    let dup = snapshot.tombstones[0].clone();
    snapshot.tombstones.push(dup);
    snapshot.tombstones[0]
        .suppressed_sources
        .push("message:4812".to_string());
    snapshot.save(&path, ts(2026, 9, 23, 11, 0, 0)).unwrap();
    let loaded = ControlSnapshot::load(&path).unwrap().unwrap();
    assert_eq!(loaded.tombstones.len(), 1, "duplicate tombstones merged");
    assert_eq!(
        loaded.tombstones[0].suppressed_sources,
        ["message:4812", "task:7c1e0b2a-4d61-4f0e-9a3c-2b7f1d5e8c90"]
    );
    assert_eq!(loaded.updated_at, ts(2026, 9, 23, 11, 0, 0));
    #[cfg(unix)]
    {
        assert_eq!(fsx::mode_of(&control), Some(0o700));
        assert_eq!(fsx::mode_of(&path), Some(0o600));
    }
    let stray: Vec<_> = std::fs::read_dir(&control)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(stray.is_empty(), "no temp file is left behind");
}

#[test]
fn snapshot_binding_is_checked() {
    let snapshot: ControlSnapshot = serde_json::from_str(SNAPSHOT_EXAMPLE).unwrap();
    assert!(
        snapshot
            .check_binding(&snapshot.partition_key, &snapshot.owner_key, &snapshot.root)
            .is_ok()
    );
    assert!(
        snapshot
            .check_binding(&snapshot.partition_key, &snapshot.owner_key, "/elsewhere")
            .is_err()
    );
    assert!(
        snapshot
            .check_binding("home-x", &snapshot.owner_key, &snapshot.root)
            .is_err()
    );
}

#[test]
fn origin_ranks_are_strictly_ordered() {
    let order = [
        Origin::User,
        Origin::ExtractedUser,
        Origin::Imported,
        Origin::External,
        Origin::ExtractedOther,
    ];
    for pair in order.windows(2) {
        assert!(
            pair[0].rank() > pair[1].rank(),
            "{:?} > {:?}",
            pair[0],
            pair[1]
        );
    }
    for o in Origin::ALL {
        assert_eq!(Origin::parse(o.as_str()), Some(*o));
    }
}

// ============================================================================
// Partition fixtures
// ============================================================================

use crate::store::StoreScope;
use crate::store::tests::HomeStoreGuard;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use tempfile::{TempDir, tempdir};

/// A temp home root (through `OPENALPACA_HOME_STORE`), a temp project and a
/// temp "outside" directory symlinks can point at.
struct Sandbox {
    _home: TempDir,
    _env: HomeStoreGuard,
    home: PathBuf,
    project: TempDir,
    outside: TempDir,
    /// One clock per sandbox, shared by every open, so a "restart" continues
    /// the same time line instead of starting it over.
    clock: Arc<dyn Fn() -> chrono::DateTime<Utc> + Send + Sync>,
}

const ALICE: &str = "u_example_alice";
const BOB: &str = "u_example_bob";

impl Sandbox {
    fn new() -> Self {
        let home = tempdir().unwrap();
        let canonical = home.path().canonicalize().unwrap();
        let env = HomeStoreGuard::set(&canonical);
        Self {
            _home: home,
            _env: env,
            home: canonical,
            project: tempdir().unwrap(),
            outside: tempdir().unwrap(),
            clock: Arc::new(ticking_clock()),
        }
    }

    fn clock(&self) -> impl Fn() -> chrono::DateTime<Utc> + Send + Sync + 'static {
        let clock = Arc::clone(&self.clock);
        move || clock()
    }

    fn project_root(&self) -> PathBuf {
        self.project.path().canonicalize().unwrap()
    }

    fn project_scope(&self) -> StoreScope {
        StoreScope::Project(self.project_root())
    }

    fn open_home(&self, owner: &str) -> Partition {
        Partition::open(&StoreScope::Home, owner)
            .unwrap()
            .with_clock(self.clock())
    }

    fn open_project(&self, owner: &str) -> Result<Partition, MemoryFilesError> {
        Partition::open(&self.project_scope(), owner).map(|p| p.with_clock(self.clock()))
    }

    fn outside_is_empty(&self) -> bool {
        std::fs::read_dir(self.outside.path())
            .unwrap()
            .next()
            .is_none()
    }
}

/// A clock that advances one second per call from a fixed instant, so every
/// `written_at` and `updated_at` is distinct and ordered.
fn ticking_clock() -> impl Fn() -> chrono::DateTime<Utc> + Send + Sync + 'static {
    let counter = Arc::new(AtomicI64::new(0));
    move || {
        let n = counter.fetch_add(1, Ordering::SeqCst);
        ts(2026, 9, 23, 10, 0, 0) + chrono::Duration::seconds(n)
    }
}

fn new_note(title: &str, body: &str, kind: NoteKind) -> NewNote {
    NewNote {
        title: title.to_string(),
        body: body.to_string(),
        kind: Some(kind),
        ..Default::default()
    }
}

fn note_files(p: &Partition) -> Vec<String> {
    p.scan().unwrap().into_iter().map(|e| e.file_name).collect()
}

fn read_file(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

// ============================================================================
// Layout, modes, .gitignore (design §2)
// ============================================================================

#[test]
fn open_creates_nothing_and_the_first_write_lays_out_privately() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    assert_eq!(p.health(), PartitionHealth::Absent);
    assert!(!sb.home.join("memory").exists(), "a read never creates");
    assert!(!p.snapshot_path().exists());
    assert!(p.load().unwrap().views.is_empty());

    let created = p
        .create(
            new_note(
                "Coffee order",
                "# Coffee\n\nFlat white.\n",
                NoteKind::Preference,
            ),
            Actor::UserSurface,
        )
        .unwrap();
    assert_eq!(created.dedup, Dedup::Created);
    assert_eq!(created.origin, Origin::User);
    assert_eq!(p.health(), PartitionHealth::Ready);
    let expected_root = sb.home.join("memory").join("owners").join(owner_key(ALICE));
    assert_eq!(p.root(), expected_root);
    assert!(p.notes_dir().is_dir() && p.history_dir().is_dir());
    let files = note_files(&p);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0], format!("{}--coffee-order.md", created.id));
    let snapshot = p.snapshot().unwrap();
    assert_eq!(snapshot.partition_key, format!("home-{}", owner_key(ALICE)));
    assert_eq!(snapshot.owner_id, ALICE);
    assert_eq!(snapshot.root, expected_root.to_string_lossy());
    assert_eq!(snapshot.notes[&created.id].revision, created.revision);
    assert_eq!(snapshot.notes[&created.id].origin, Origin::User);
    assert!(snapshot.pending.is_none());
    assert_eq!(
        p.snapshot_path(),
        sb.home
            .join("state")
            .join("memory-control")
            .join(format!("{}.json", snapshot.partition_key))
    );
    assert!(p.root().join("MEMORY.md").is_file());
    #[cfg(unix)]
    {
        for dir in [
            sb.home.join("memory"),
            sb.home.join("memory/owners"),
            p.root().to_path_buf(),
            p.notes_dir().to_path_buf(),
            p.history_dir().to_path_buf(),
            p.history_dir().join(created.id.to_string()),
            sb.home.join("state"),
            sb.home.join("state/memory-control"),
        ] {
            assert_eq!(fsx::mode_of(&dir), Some(0o700), "{}", dir.display());
        }
        for file in [
            p.notes_dir().join(&files[0]),
            p.history_dir()
                .join(created.id.to_string())
                .join(format!("{}.md", created.revision)),
            p.snapshot_path().to_path_buf(),
            p.root().join("MEMORY.md"),
        ] {
            assert_eq!(fsx::mode_of(&file), Some(0o600), "{}", file.display());
        }
    }
    // The revision is the file's exact bytes.
    let bytes = read_file(&p.notes_dir().join(&files[0]));
    assert_eq!(revision_of(&bytes), created.revision);
    // Reopening reads it back, same key, no recovery churn.
    let again = sb.open_home(ALICE);
    assert_eq!(again.partition_key(), p.partition_key());
    let view = again.read(&created.id).unwrap();
    assert_eq!(view.revision, created.revision);
    assert_eq!(view.derived.origin, Origin::User);
    assert!(view.derived.recorded);
}

#[test]
fn project_partition_is_created_lazily_with_gitignore_and_modes() {
    let sb = Sandbox::new();
    let p = sb.open_project(ALICE).unwrap();
    assert_eq!(p.health(), PartitionHealth::Absent);
    assert!(
        !sb.project_root().join(".openalpaca").exists(),
        "a read never creates the store"
    );
    let key_before = p.partition_key().to_string();

    let created = p
        .create(
            new_note(
                "Test command",
                "# Test\n\nRun cargo test.\n",
                NoteKind::Procedure,
            ),
            Actor::ModelTool,
        )
        .unwrap();
    assert_eq!(created.origin, Origin::ExtractedUser);
    let store = sb.project_root().join(".openalpaca");
    let memory = store.join("memory");
    assert_eq!(
        read_file(&memory.join(".gitignore")),
        PROJECT_GITIGNORE_TEXT.as_bytes()
    );
    assert!(
        store.join("README.md").is_file(),
        "content_dir seeded the project store"
    );
    assert!(
        memory
            .join("owners")
            .join(owner_key(ALICE))
            .join("notes")
            .is_dir()
    );
    assert!(
        memory
            .join("owners")
            .join(owner_key(ALICE))
            .join(".history")
            .is_dir()
    );
    assert!(key_before.starts_with(&format!("proj-{}-", owner_key(ALICE))));
    #[cfg(unix)]
    {
        assert_eq!(fsx::mode_of(&memory), Some(0o700));
        assert_eq!(fsx::mode_of(p.root()), Some(0o700));
        assert_eq!(fsx::mode_of(p.notes_dir()), Some(0o700));
        assert_eq!(fsx::mode_of(&memory.join(".gitignore")), Some(0o600));
        let file = p.notes_dir().join(&note_files(&p)[0]);
        assert_eq!(fsx::mode_of(&file), Some(0o600));
    }
    // The key is stable before and after the store exists, and after the
    // artifact path lays out the rest of `.openalpaca/`.
    crate::store::content_dir(&sb.project_scope(), crate::store::ContentKind::Artifacts).unwrap();
    let again = sb.open_project(ALICE).unwrap();
    assert_eq!(again.partition_key(), key_before);
    assert_eq!(again.snapshot().unwrap().notes.len(), 1);
    assert_eq!(read_file(&memory.join(".gitignore")), b"*\n");
}

#[test]
fn a_pre_existing_project_gitignore_with_other_content_is_replaced() {
    let sb = Sandbox::new();
    let memory = sb.project_root().join(".openalpaca").join("memory");
    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(memory.join(".gitignore"), "!*\n").unwrap();
    let _p = sb.open_project(ALICE).unwrap();
    assert_eq!(
        read_file(&memory.join(".gitignore")),
        b"*\n",
        "re-asserted on open"
    );
    // Absent: written on open too, since `memory/` exists.
    std::fs::remove_file(memory.join(".gitignore")).unwrap();
    let _p = sb.open_project(ALICE).unwrap();
    assert_eq!(read_file(&memory.join(".gitignore")), b"*\n");
    // A symlink where the file belongs is replaced by the real file, and the
    // link's target is not written through.
    #[cfg(unix)]
    {
        std::fs::remove_file(memory.join(".gitignore")).unwrap();
        let target = sb.outside.path().join("elsewhere");
        std::fs::write(&target, "!*\n").unwrap();
        symlink(&target, &memory.join(".gitignore"));
        let _p = sb.open_project(ALICE).unwrap();
        assert!(
            !std::fs::symlink_metadata(memory.join(".gitignore"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(read_file(&memory.join(".gitignore")), b"*\n");
        assert_eq!(read_file(&target), b"!*\n", "the link target is untouched");
    }
}

// ============================================================================
// Confinement (design §3 rule 1, §6)
// ============================================================================

#[cfg(unix)]
#[test]
fn a_link_or_file_at_any_component_refuses_the_project_partition() {
    let key = owner_key(ALICE);
    let components = [
        ".openalpaca",
        ".openalpaca/memory",
        ".openalpaca/memory/owners",
        &format!(".openalpaca/memory/owners/{key}"),
        &format!(".openalpaca/memory/owners/{key}/notes"),
        &format!(".openalpaca/memory/owners/{key}/.history"),
    ];
    for component in components {
        let sb = Sandbox::new();
        let link = sb.project_root().join(component);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(sb.outside.path(), &link);
        let err = sb
            .open_project(ALICE)
            .err()
            .unwrap_or_else(|| panic!("{component} must refuse"));
        match &err {
            MemoryFilesError::PathRefused {
                component: named,
                reason,
            } => {
                assert_eq!(named, &link, "names the component");
                assert!(reason.contains("symbolic link"), "{reason}");
            }
            other => panic!("{component}: expected PathRefused, got {other:?}"),
        }
        assert!(
            err.to_string()
                .contains(component.rsplit('/').next().unwrap())
        );
        assert!(
            sb.outside_is_empty(),
            "{component}: nothing written through the link"
        );
        assert!(!sb.home.join("memory").exists());
    }
    // A regular file where `memory` should be a directory.
    let sb = Sandbox::new();
    let store = sb.project_root().join(".openalpaca");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join("memory"), "not a directory\n").unwrap();
    let err = sb.open_project(ALICE).expect_err("a file refuses");
    assert!(
        matches!(&err, MemoryFilesError::PathRefused { reason, .. } if reason == "not a directory")
    );
    assert_eq!(read_file(&store.join("memory")), b"not a directory\n");
    assert_eq!(err.class(), "fixture:path_refused");
}

#[cfg(unix)]
#[test]
fn a_link_below_the_home_root_refuses_the_home_partition_too() {
    let sb = Sandbox::new();
    symlink(sb.outside.path(), &sb.home.join("memory"));
    let err = Partition::open(&StoreScope::Home, ALICE).expect_err("refused");
    assert!(matches!(err, MemoryFilesError::PathRefused { .. }));
    assert!(sb.outside_is_empty());
}

#[cfg(unix)]
#[test]
fn a_symlinked_note_file_is_not_read_written_or_listed() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    let c = p
        .create(
            new_note("Real", "# Real\n\nbody\n", NoteKind::Fact),
            Actor::UserSurface,
        )
        .unwrap();
    // A link in notes/ pointing at a real note elsewhere.
    let target = sb.outside.path().join("planted.md");
    let planted = Note::parse(EXAMPLE.as_bytes()).unwrap();
    std::fs::write(&target, planted.serialize()).unwrap();
    symlink(&target, &p.notes_dir().join(planted.file_name()));
    let scanned = p.scan().unwrap();
    assert_eq!(scanned.len(), 1, "the link is not listed");
    assert_eq!(scanned[0].id, Some(c.id));
    assert!(matches!(
        p.read(&planted.id),
        Err(MemoryFilesError::NotFound(_))
    ));
    // A hard link alias of a real note is refused as malformed, not served.
    let real = p.notes_dir().join(&note_files(&p)[0]);
    let alias = p.notes_dir().join(format!("{}--alias.md", c.id));
    std::fs::hard_link(&real, &alias).unwrap();
    let loaded = p.load().unwrap();
    assert!(
        loaded.views.is_empty(),
        "both names of the inode are refused"
    );
    assert_eq!(loaded.malformed.len(), 2);
    assert!(
        loaded
            .malformed
            .iter()
            .all(|m| m.reason.contains("hard links"))
    );
}

#[test]
fn the_scanner_lists_only_regular_md_notes() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    let c = p
        .create(
            new_note("One", "# One\n", NoteKind::Fact),
            Actor::UserSurface,
        )
        .unwrap();
    std::fs::write(p.notes_dir().join(".hidden.md"), "x").unwrap();
    std::fs::write(p.notes_dir().join(".x.md.123-4.tmp"), "x").unwrap();
    std::fs::write(p.notes_dir().join("notes.txt"), "x").unwrap();
    std::fs::create_dir(p.notes_dir().join("sub.md")).unwrap();
    std::fs::write(p.notes_dir().join("by-hand.md"), "not frontmatter").unwrap();
    let scanned = p.scan().unwrap();
    let mut names: Vec<&str> = scanned.iter().map(|e| e.file_name.as_str()).collect();
    names.sort();
    let mut want = [format!("{}--one.md", c.id), "by-hand.md".to_string()];
    want.sort();
    assert_eq!(names, want.iter().map(String::as_str).collect::<Vec<_>>());
    let one = scanned
        .iter()
        .find(|e| e.id == Some(c.id))
        .expect("the note is listed with its id");
    assert!(
        scanned
            .iter()
            .any(|e| e.file_name == "by-hand.md" && e.id.is_none())
    );
    assert!(one.size > 0);
    assert!(one.mtime.is_some());
    #[cfg(unix)]
    assert!(one.inode > 0);
    let loaded = p.load().unwrap();
    assert_eq!(loaded.views.len(), 1);
    assert_eq!(loaded.malformed.len(), 1);
    assert!(loaded.malformed[0].path.ends_with("by-hand.md"));
}

// ============================================================================
// Mutations (design §6, §7a)
// ============================================================================

#[test]
fn history_keeps_the_last_five_service_written_revisions_current_included() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    let c = p
        .create(
            new_note("Counter", "# Counter\n\n0\n", NoteKind::Fact),
            Actor::UserSurface,
        )
        .unwrap();
    let mut revisions = vec![c.revision.clone()];
    let mut current = c.revision.clone();
    for i in 1..=7 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        let u = p
            .update(
                &c.id,
                &current,
                UpdateAction::Edit(NotePatch {
                    body: Some(format!("# Counter\n\n{i}\n")),
                    ..Default::default()
                }),
                Actor::UserSurface,
            )
            .unwrap();
        assert!(u.written);
        assert_ne!(u.revision, current);
        current = u.revision.clone();
        revisions.push(u.revision);
    }
    let kept: Vec<String> = p
        .history(&c.id)
        .unwrap()
        .into_iter()
        .map(|(r, _)| r)
        .collect();
    assert_eq!(kept.len(), HISTORY_KEEP);
    assert_eq!(kept[0], current, "newest first, the current one included");
    for old in &revisions[..revisions.len() - HISTORY_KEEP] {
        assert!(!kept.contains(old), "{old} was pruned");
    }
    for recent in &revisions[revisions.len() - HISTORY_KEEP..] {
        assert!(kept.contains(recent), "{recent} is kept");
    }
    assert_eq!(
        p.history_bytes(&c.id, &current).unwrap().unwrap(),
        read_file(&p.notes_dir().join(&note_files(&p)[0]))
    );
    assert!(p.history_bytes(&c.id, "not-a-revision").unwrap().is_none());
    assert_eq!(
        note_files(&p).len(),
        1,
        "one file per note, never one per revision"
    );
}

#[test]
fn revision_checks_no_op_patches_and_retitles() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    let c = p
        .create(
            new_note("Standup time", "# Standup\n\n09:30\n", NoteKind::Fact),
            Actor::ModelTool,
        )
        .unwrap();
    let stale = "f".repeat(64);
    let err = p
        .update(&c.id, &stale, UpdateAction::Archive, Actor::ModelTool)
        .unwrap_err();
    match err {
        MemoryFilesError::RevisionConflict {
            id,
            current_revision,
        } => {
            assert_eq!(id, c.id);
            assert_eq!(current_revision, c.revision);
        }
        other => panic!("{other:?}"),
    }
    assert!(
        p.snapshot().unwrap().pending.is_none(),
        "refused before step 1"
    );
    // A no-op patch: no write, same revision.
    let u = p
        .update(
            &c.id,
            &c.revision,
            UpdateAction::Edit(NotePatch::default()),
            Actor::ModelTool,
        )
        .unwrap();
    assert!(!u.written);
    assert_eq!(u.revision, c.revision);
    let u = p
        .update(
            &c.id,
            &c.revision,
            UpdateAction::Edit(NotePatch {
                body: Some("# Standup\n\n09:30\n".into()),
                ..Default::default()
            }),
            Actor::ModelTool,
        )
        .unwrap();
    assert!(!u.written, "the same body is a no-op");
    assert_eq!(p.history(&c.id).unwrap().len(), 1);
    // A retitle renames the file; the id is the identity.
    let u = p
        .update(
            &c.id,
            &c.revision,
            UpdateAction::Edit(NotePatch {
                title: Some("Daily standup".into()),
                ..Default::default()
            }),
            Actor::ModelTool,
        )
        .unwrap();
    assert!(u.written);
    assert_eq!(note_files(&p), [format!("{}--daily-standup.md", c.id)]);
    let view = p.read(&c.id).unwrap();
    assert_eq!(view.revision, u.revision);
    assert_eq!(view.note.title, "Daily standup");
    assert_eq!(
        view.note.body, "# Standup\n\n09:30\n",
        "untouched body byte-for-byte"
    );
    // The second of two racing edits loses visibly.
    let err = p
        .update(&c.id, &c.revision, UpdateAction::Archive, Actor::ModelTool)
        .unwrap_err();
    assert!(matches!(err, MemoryFilesError::RevisionConflict { .. }));
    assert!(matches!(
        p.update(
            &NoteId::new(),
            &c.revision,
            UpdateAction::Archive,
            Actor::UserSurface
        ),
        Err(MemoryFilesError::NotFound(_))
    ));
}

#[test]
fn oversized_and_invalid_inputs_are_refused_before_any_write() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    let err = p
        .create(
            new_note(
                "Too large",
                &"x".repeat(MAX_NOTE_BYTES + 1),
                NoteKind::Reference,
            ),
            Actor::ModelTool,
        )
        .unwrap_err();
    assert_eq!(err.class(), "invalid_input");
    assert!(err.to_string().contains("32"));
    assert_eq!(p.health(), PartitionHealth::Absent, "nothing was laid out");
    assert!(matches!(
        p.create(
            NewNote {
                title: "x".into(),
                body: "b".into(),
                kind: None,
                ..Default::default()
            },
            Actor::ModelTool
        ),
        Err(MemoryFilesError::InvalidInput(_))
    ));
    assert!(matches!(
        p.create(
            NewNote {
                fact_key: Some("Bad Key".into()),
                ..new_note("t", "b", NoteKind::Fact)
            },
            Actor::ModelTool
        ),
        Err(MemoryFilesError::InvalidInput(_))
    ));
    // Exactly at the limit is accepted and read back untruncated.
    let probe = new_note("At the limit", "", NoteKind::Reference);
    let header = {
        let mut n = Note::parse(EXAMPLE.as_bytes()).unwrap();
        n.title = probe.title.clone();
        n.kind = NoteKind::Reference;
        n.fact_key = None;
        n.aliases.clear();
        n.tags.clear();
        n.observed_at = None;
        n.sources.clear();
        n.body = String::new();
        n.serialize().len()
    };
    let at_limit = new_note(
        "At the limit",
        &"y".repeat(MAX_NOTE_BYTES - header),
        NoteKind::Reference,
    );
    let c = p.create(at_limit, Actor::ModelTool).unwrap();
    let bytes = read_file(&p.notes_dir().join(&note_files(&p)[0]));
    assert_eq!(bytes.len(), MAX_NOTE_BYTES);
    assert_eq!(
        p.read(&c.id).unwrap().note.body.len(),
        MAX_NOTE_BYTES - header
    );
}

#[test]
fn exact_dedup_is_within_one_partition_only() {
    let sb = Sandbox::new();
    let home = sb.open_home(ALICE);
    let first = home
        .create(
            new_note(
                "Indentation",
                "# Indentation\n\nTabs.\n",
                NoteKind::Preference,
            ),
            Actor::UserSurface,
        )
        .unwrap();
    let again = home
        .create(
            new_note(
                "Indentation",
                "# Indentation\n\nTabs.\n",
                NoteKind::Preference,
            ),
            Actor::ModelTool,
        )
        .unwrap();
    assert_eq!(again.dedup, Dedup::Existing);
    assert_eq!(again.id, first.id);
    assert_eq!(again.revision, first.revision);
    assert_eq!(home.load().unwrap().views.len(), 1);
    // A different kind is not a duplicate; neither is the same text in
    // another partition.
    let other = home
        .create(
            new_note(
                "Indentation",
                "# Indentation\n\nTabs.\n",
                NoteKind::Decision,
            ),
            Actor::ModelTool,
        )
        .unwrap();
    assert_eq!(other.dedup, Dedup::Created);
    let project = sb.open_project(ALICE).unwrap();
    let elsewhere = project
        .create(
            new_note(
                "Indentation",
                "# Indentation\n\nTabs.\n",
                NoteKind::Preference,
            ),
            Actor::ModelTool,
        )
        .unwrap();
    assert_eq!(elsewhere.dedup, Dedup::Created);
    assert_ne!(elsewhere.id, first.id);
    let bob = sb.open_home(BOB);
    assert!(matches!(
        bob.read(&first.id),
        Err(MemoryFilesError::NotFound(_))
    ));
}

#[test]
fn authority_rules_of_g2_are_enforced_by_the_file_layer() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    let user_note = p
        .create(
            new_note("Editor", "# Editor\n\nvim\n", NoteKind::Preference),
            Actor::UserSurface,
        )
        .unwrap();
    let candidate = p
        .create(
            NewNote {
                status: Some(NoteStatus::Candidate),
                ..new_note(
                    "Staging URL",
                    "# Staging\n\nchanged, says a tool\n",
                    NoteKind::Fact,
                )
            },
            Actor::Extraction {
                origin: Origin::ExtractedOther,
            },
        )
        .unwrap();
    assert_eq!(candidate.origin, Origin::ExtractedOther);

    // A tool call may not edit, archive or supersede a user note.
    for action in [
        UpdateAction::Edit(NotePatch {
            body: Some("# Editor\n\nhelix\n".into()),
            ..Default::default()
        }),
        UpdateAction::Archive,
    ] {
        let err = p
            .update(&user_note.id, &user_note.revision, action, Actor::ModelTool)
            .unwrap_err();
        assert_eq!(err.class(), "authority_conflict");
        let msg = err.to_string();
        assert!(
            msg.contains(&user_note.id.to_string())
                && msg.contains("Memory view")
                && msg.contains("openalpaca memory"),
            "{msg}"
        );
    }
    let err = p
        .create(
            NewNote {
                supersedes: Some((user_note.id, user_note.revision.clone())),
                ..new_note("Editor", "# Editor\n\nhelix\n", NoteKind::Preference)
            },
            Actor::ModelTool,
        )
        .unwrap_err();
    assert_eq!(err.class(), "authority_conflict");
    assert_eq!(
        p.read(&user_note.id).unwrap().revision,
        user_note.revision,
        "nothing written"
    );
    assert_eq!(p.load().unwrap().views.len(), 2);

    // Confirm is refused from a tool call and works from a user surface.
    let err = p
        .update(
            &candidate.id,
            &candidate.revision,
            UpdateAction::Confirm,
            Actor::ModelTool,
        )
        .unwrap_err();
    assert_eq!(err.class(), "confirm_refused");
    assert!(
        err.to_string()
            .contains(&format!("openalpaca memory confirm {}", candidate.id))
    );
    assert_eq!(
        p.read(&candidate.id).unwrap().note.status,
        NoteStatus::Candidate
    );
    let confirmed = p
        .update(
            &candidate.id,
            &candidate.revision,
            UpdateAction::Confirm,
            Actor::UserSurface,
        )
        .unwrap();
    assert_eq!(confirmed.status, NoteStatus::Active);
    let view = p.read(&candidate.id).unwrap();
    assert_eq!(view.derived.origin, Origin::User);
    assert_eq!(view.derived.effective_status, EffectiveStatus::Active);
    // Confirming an active note is an input error, not a silent success.
    assert!(matches!(
        p.update(
            &candidate.id,
            &confirmed.revision,
            UpdateAction::Confirm,
            Actor::UserSurface
        ),
        Err(MemoryFilesError::InvalidInput(_))
    ));

    // A user-surface edit of a model note re-records it as user.
    let model_note = p
        .create(
            new_note("Lunch", "# Lunch\n\nnoodles\n", NoteKind::Preference),
            Actor::ModelTool,
        )
        .unwrap();
    assert_eq!(model_note.origin, Origin::ExtractedUser);
    let u = p
        .update(
            &model_note.id,
            &model_note.revision,
            UpdateAction::Edit(NotePatch {
                aliases: Some(vec!["noodle bar".into()]),
                ..Default::default()
            }),
            Actor::UserSurface,
        )
        .unwrap();
    assert_eq!(
        p.snapshot().unwrap().notes[&model_note.id].origin,
        Origin::User
    );
    assert_eq!(
        p.snapshot().unwrap().notes[&model_note.id].revision,
        u.revision
    );
    // And a supersede through a user surface creates B with A named.
    let b = p
        .create(
            NewNote {
                supersedes: Some((user_note.id, user_note.revision.clone())),
                ..new_note("Editor", "# Editor\n\nhelix\n", NoteKind::Preference)
            },
            Actor::UserSurface,
        )
        .unwrap();
    assert_eq!(b.superseded, [user_note.id]);
    let a = p.read(&user_note.id).unwrap();
    assert_eq!(a.revision, user_note.revision, "A's file is not touched");
    assert_eq!(a.derived.effective_status, EffectiveStatus::Superseded);
    assert_eq!(a.derived.superseded_by, [b.id]);
    // A moved target is a conflict and nothing is created.
    let err = p
        .create(
            NewNote {
                supersedes: Some((b.id, "f".repeat(64))),
                ..new_note("Editor", "# Editor\n\nnano\n", NoteKind::Preference)
            },
            Actor::UserSurface,
        )
        .unwrap_err();
    assert_eq!(err.class(), "revision_conflict");
    assert!(matches!(
        p.create(
            NewNote {
                supersedes: Some((NoteId::new(), "f".repeat(64))),
                ..new_note("x", "b", NoteKind::Fact)
            },
            Actor::UserSurface
        ),
        Err(MemoryFilesError::NotFound(_))
    ));
}

#[test]
fn concurrent_creates_on_one_partition_are_both_recorded() {
    let sb = Sandbox::new();
    let p = Arc::new(sb.open_home(ALICE));
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let p = Arc::clone(&p);
            std::thread::spawn(move || {
                p.create(
                    new_note(
                        &format!("Save {i}"),
                        &format!("# Save {i}\n\nbody\n"),
                        NoteKind::Fact,
                    ),
                    Actor::ModelTool,
                )
                .unwrap()
            })
        })
        .collect();
    let created: Vec<Created> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let snapshot = p.snapshot().unwrap();
    assert!(snapshot.pending.is_none());
    for c in &created {
        assert_eq!(snapshot.notes[&c.id].revision, c.revision);
    }
    assert_eq!(note_files(&p).len(), 4);
    assert_eq!(p.load().unwrap().views.len(), 4);
}

// ============================================================================
// The recovery table (design §6) with a crash at every step
// ============================================================================

/// Seeds a home partition with the fixture-15 shape and returns it.
fn seeded_for_recovery(sb: &Sandbox) -> (Partition, Created, Created, Created, Created) {
    let p = sb.open_home(ALICE);
    let update_target = p
        .create(
            new_note(
                "Update target",
                "# Update target\n\nVersion one of the body.\n",
                NoteKind::Fact,
            ),
            Actor::UserSurface,
        )
        .unwrap();
    let archive_target = p
        .create(
            new_note(
                "Archive target",
                "# Archive target\n\nTo be archived.\n",
                NoteKind::Fact,
            ),
            Actor::UserSurface,
        )
        .unwrap();
    let confirm_target = p
        .create(
            NewNote {
                status: Some(NoteStatus::Candidate),
                ..new_note(
                    "Confirm target",
                    "# Confirm target\n\nA candidate.\n",
                    NoteKind::Fact,
                )
            },
            Actor::Extraction {
                origin: Origin::ExtractedOther,
            },
        )
        .unwrap();
    let forget_target = p
        .create(
            NewNote {
                sources: vec![NoteSource {
                    kind: SourceType::Message,
                    id: "6001".into(),
                    observed_at: None,
                }],
                ..new_note(
                    "Forget target",
                    "# Forget target\n\nTo be forgotten.\n",
                    NoteKind::Fact,
                )
            },
            Actor::ModelTool,
        )
        .unwrap();
    (
        p,
        update_target,
        archive_target,
        confirm_target,
        forget_target,
    )
}

fn edit_body(body: &str) -> UpdateAction {
    UpdateAction::Edit(NotePatch {
        body: Some(body.to_string()),
        ..Default::default()
    })
}

fn file_for(p: &Partition, id: &NoteId) -> Option<PathBuf> {
    p.scan()
        .unwrap()
        .into_iter()
        .find(|e| e.id == Some(*id))
        .map(|e| e.path)
}

#[test]
fn create_crash_after_each_step_recovers_per_the_table() {
    for point in [
        CrashPoint::WriteAfter1,
        CrashPoint::WriteAfter2a,
        CrashPoint::WriteAfter2,
        CrashPoint::WriteAfter3,
    ] {
        let sb = Sandbox::new();
        let (p, ..) = seeded_for_recovery(&sb);
        let before = p.snapshot().unwrap();
        p.set_crash_point(Some(point));
        let err = p
            .create(
                new_note(
                    "Created then crashed",
                    "# Created\n\nBody.\n",
                    NoteKind::Fact,
                ),
                Actor::UserSurface,
            )
            .unwrap_err();
        assert!(matches!(err, MemoryFilesError::Crashed(pt) if pt == point));
        let on_disk = ControlSnapshot::load(p.snapshot_path()).unwrap().unwrap();
        let (id, to_rev) = match point {
            CrashPoint::WriteAfter3 => {
                assert!(on_disk.pending.is_none());
                let (id, rec) = on_disk
                    .notes
                    .iter()
                    .find(|(id, _)| !before.notes.contains_key(id))
                    .unwrap();
                (*id, rec.revision.clone())
            }
            _ => {
                let pending = on_disk.pending.clone().expect("pending set at step 1");
                assert_eq!(pending.from_rev, None);
                assert_eq!(pending.origin, Origin::User);
                assert!(
                    !on_disk.notes.contains_key(&pending.id),
                    "not recorded before step 3"
                );
                (pending.id, pending.to_rev)
            }
        };
        let history = p
            .history_dir()
            .join(id.to_string())
            .join(format!("{to_rev}.md"));
        match point {
            CrashPoint::WriteAfter1 => {
                assert!(file_for(&p, &id).is_none());
                assert!(!history.exists());
            }
            CrashPoint::WriteAfter2a => {
                assert!(
                    file_for(&p, &id).is_none(),
                    "history copy before the rename"
                );
                assert!(history.is_file());
            }
            CrashPoint::WriteAfter2 | CrashPoint::WriteAfter3 => {
                let file = file_for(&p, &id).expect("written");
                assert_eq!(revision_of(&read_file(&file)), to_rev);
                assert!(history.is_file());
            }
            _ => unreachable!(),
        }
        drop(p);
        // Restart: recovery at service start.
        let p = sb.open_home(ALICE);
        let after = p.snapshot().unwrap();
        assert!(after.pending.is_none(), "{point:?}: pending cleared");
        match point {
            CrashPoint::WriteAfter1 | CrashPoint::WriteAfter2a => {
                assert!(!after.notes.contains_key(&id), "{point:?}: never recorded");
                assert!(file_for(&p, &id).is_none());
                assert!(p.load().unwrap().views.iter().all(|v| v.note.id != id));
                if point == CrashPoint::WriteAfter2a {
                    assert!(history.is_file(), "the bounded history copy is left");
                }
            }
            CrashPoint::WriteAfter2 | CrashPoint::WriteAfter3 => {
                let rec = &after.notes[&id];
                assert_eq!(rec.revision, to_rev, "{point:?}: step 3 finished");
                assert_eq!(rec.origin, Origin::User);
                let views: Vec<_> = p
                    .load()
                    .unwrap()
                    .views
                    .into_iter()
                    .filter(|v| v.note.id == id)
                    .collect();
                assert_eq!(views.len(), 1, "listed once, no duplicate");
                assert!(views[0].derived.recorded);
            }
            _ => unreachable!(),
        }
        assert_eq!(
            after.notes.len() + usize::from(!after.notes.contains_key(&id)),
            before.notes.len() + 1
        );
        assert_eq!(after.tombstones.len(), 0);
    }
}

#[test]
fn update_archive_confirm_crash_after_each_step_recovers_per_the_table() {
    #[derive(Clone, Copy)]
    enum Which {
        Update,
        Archive,
        Confirm,
    }
    let cases = [
        (Which::Update, CrashPoint::WriteAfter1),
        (Which::Update, CrashPoint::WriteAfter2a),
        (Which::Update, CrashPoint::WriteAfter2),
        (Which::Update, CrashPoint::WriteAfter3),
        (Which::Archive, CrashPoint::WriteAfter1),
        (Which::Archive, CrashPoint::WriteAfter2a),
        (Which::Archive, CrashPoint::WriteAfter2),
        (Which::Archive, CrashPoint::WriteAfter3),
        (Which::Confirm, CrashPoint::WriteAfter1),
        (Which::Confirm, CrashPoint::WriteAfter2a),
        (Which::Confirm, CrashPoint::WriteAfter2),
        (Which::Confirm, CrashPoint::WriteAfter3),
    ];
    for (which, point) in cases {
        let sb = Sandbox::new();
        let (p, update_target, archive_target, confirm_target, _) = seeded_for_recovery(&sb);
        let (target, action, old_origin, new_origin) = match which {
            Which::Update => (
                &update_target,
                edit_body("# Update target\n\nVersion two of the body.\n"),
                Origin::User,
                Origin::User,
            ),
            Which::Archive => (
                &archive_target,
                UpdateAction::Archive,
                Origin::User,
                Origin::User,
            ),
            Which::Confirm => (
                &confirm_target,
                UpdateAction::Confirm,
                Origin::ExtractedOther,
                Origin::User,
            ),
        };
        let from_rev = target.revision.clone();
        let path_before = file_for(&p, &target.id).unwrap();
        p.set_crash_point(Some(point));
        let err = p
            .update(&target.id, &from_rev, action, Actor::UserSurface)
            .unwrap_err();
        assert!(matches!(err, MemoryFilesError::Crashed(pt) if pt == point));
        let on_disk = ControlSnapshot::load(p.snapshot_path()).unwrap().unwrap();
        let to_rev = match point {
            CrashPoint::WriteAfter3 => {
                assert!(on_disk.pending.is_none());
                on_disk.notes[&target.id].revision.clone()
            }
            _ => {
                let pending = on_disk.pending.clone().expect("pending");
                assert_eq!(pending.id, target.id);
                assert_eq!(pending.from_rev.as_deref(), Some(from_rev.as_str()));
                assert_eq!(pending.origin, new_origin);
                assert_eq!(
                    on_disk.notes[&target.id].revision, from_rev,
                    "record untouched until step 3"
                );
                assert_eq!(on_disk.notes[&target.id].origin, old_origin);
                pending.to_rev
            }
        };
        assert_ne!(to_rev, from_rev);
        let history = p
            .history_dir()
            .join(target.id.to_string())
            .join(format!("{to_rev}.md"));
        let file_now = revision_of(&read_file(&path_before));
        match point {
            CrashPoint::WriteAfter1 => {
                assert_eq!(file_now, from_rev);
                assert!(!history.exists());
            }
            CrashPoint::WriteAfter2a => {
                assert_eq!(file_now, from_rev, "history copied before the rename");
                assert!(history.is_file());
            }
            CrashPoint::WriteAfter2 | CrashPoint::WriteAfter3 => {
                assert_eq!(file_now, to_rev);
                assert!(history.is_file());
            }
            _ => unreachable!(),
        }
        drop(p);
        let p = sb.open_home(ALICE);
        let after = p.snapshot().unwrap();
        assert!(after.pending.is_none());
        let rec = &after.notes[&target.id];
        let view = p.read(&target.id).unwrap();
        match point {
            CrashPoint::WriteAfter1 | CrashPoint::WriteAfter2a => {
                assert_eq!(rec.revision, from_rev, "{point:?}: the old revision stands");
                assert_eq!(rec.origin, old_origin);
                assert_eq!(view.revision, from_rev);
                assert_eq!(history.exists(), point == CrashPoint::WriteAfter2a);
                match which {
                    Which::Update => assert!(view.note.body.contains("Version one")),
                    Which::Archive => assert_eq!(view.note.status, NoteStatus::Active),
                    Which::Confirm => assert_eq!(view.note.status, NoteStatus::Candidate),
                }
            }
            CrashPoint::WriteAfter2 | CrashPoint::WriteAfter3 => {
                assert_eq!(rec.revision, to_rev, "{point:?}: step 3 finished");
                assert_eq!(rec.origin, new_origin);
                assert_eq!(view.revision, to_rev);
                assert!(view.derived.recorded);
                match which {
                    Which::Update => assert!(view.note.body.contains("Version two")),
                    Which::Archive => assert_eq!(view.note.status, NoteStatus::Archived),
                    Which::Confirm => {
                        assert_eq!(view.note.status, NoteStatus::Active);
                        assert_eq!(view.derived.origin, Origin::User);
                    }
                }
            }
            _ => unreachable!(),
        }
        assert_eq!(
            p.load()
                .unwrap()
                .views
                .iter()
                .filter(|v| v.note.id == target.id)
                .count(),
            1
        );
    }
}

#[test]
fn an_external_edit_that_races_a_crashed_write_downgrades_to_external() {
    let sb = Sandbox::new();
    let (p, target, ..) = seeded_for_recovery(&sb);
    p.set_crash_point(Some(CrashPoint::WriteAfter2));
    let _ = p.update(
        &target.id,
        &target.revision,
        edit_body("# Update target\n\nThe service's new body.\n"),
        Actor::UserSurface,
    );
    let pending = ControlSnapshot::load(p.snapshot_path())
        .unwrap()
        .unwrap()
        .pending
        .unwrap();
    let path = file_for(&p, &target.id).unwrap();
    let mut bytes = read_file(&path);
    bytes.extend_from_slice(b"\nAn editor wrote this after the crash.\n");
    std::fs::write(&path, &bytes).unwrap();
    drop(p);
    let p = sb.open_home(ALICE);
    let snapshot = p.snapshot().unwrap();
    assert!(snapshot.pending.is_none());
    let rec = &snapshot.notes[&target.id];
    assert_eq!(rec.origin, Origin::External);
    assert_eq!(rec.revision, revision_of(&bytes));
    assert!(
        p.history_bytes(&target.id, &pending.to_rev)
            .unwrap()
            .is_some(),
        "the service's own bytes are in .history"
    );
    let view = p.read(&target.id).unwrap();
    assert_eq!(view.derived.origin, Origin::External);
    assert!(
        view.note
            .body
            .contains("An editor wrote this after the crash")
    );
}

#[test]
fn forget_crash_after_each_step_recovers_and_tombstones_survive_restarts() {
    for point in [CrashPoint::ForgetAfter1, CrashPoint::ForgetAfter2] {
        let sb = Sandbox::new();
        let (p, _, _, _, target) = seeded_for_recovery(&sb);
        assert!(p.history_dir().join(target.id.to_string()).is_dir());
        p.set_crash_point(Some(point));
        let err = p
            .forget(&[(target.id, target.revision.clone())])
            .unwrap_err();
        assert!(matches!(err, MemoryFilesError::Crashed(pt) if pt == point));
        let on_disk = ControlSnapshot::load(p.snapshot_path()).unwrap().unwrap();
        assert_eq!(on_disk.tombstones.len(), 1, "tombstone first");
        let t = &on_disk.tombstones[0];
        assert_eq!(t.id, target.id);
        assert_eq!(t.origin, Origin::ExtractedUser);
        assert!(t.supersedes.is_empty());
        assert_eq!(t.suppressed_sources, ["message:6001"]);
        assert!(!on_disk.notes.contains_key(&target.id));
        match point {
            CrashPoint::ForgetAfter1 => {
                assert!(
                    file_for(&p, &target.id).is_some(),
                    "the file is still there"
                );
                assert!(p.history_dir().join(target.id.to_string()).is_dir());
            }
            _ => {
                assert!(file_for(&p, &target.id).is_none());
                assert!(!p.history_dir().join(target.id.to_string()).exists());
            }
        }
        drop(p);
        for _ in 0..2 {
            let p = sb.open_home(ALICE);
            assert!(
                file_for(&p, &target.id).is_none(),
                "{point:?}: a tombstoned file is deleted at start"
            );
            assert!(!p.history_dir().join(target.id.to_string()).exists());
            let snapshot = p.snapshot().unwrap();
            assert_eq!(snapshot.tombstones.len(), 1, "tombstones survive restarts");
            assert!(!snapshot.notes.contains_key(&target.id));
            assert!(matches!(
                p.read(&target.id),
                Err(MemoryFilesError::NotFound(_))
            ));
            // A forget of a gone id is not_found whatever revision is sent.
            let outcomes = p.forget(&[(target.id, "f".repeat(64))]).unwrap();
            assert_eq!(outcomes[0].status, ForgetStatus::NotFound);
        }
    }
}

#[test]
fn forget_is_per_target_and_names_the_current_revision_on_conflict() {
    let sb = Sandbox::new();
    let (p, a, b, ..) = seeded_for_recovery(&sb);
    let outcomes = p
        .forget(&[
            (a.id, "f".repeat(64)),
            (b.id, b.revision.clone()),
            (NoteId::new(), "f".repeat(64)),
        ])
        .unwrap();
    assert_eq!(
        outcomes[0].status,
        ForgetStatus::RevisionConflict {
            current_revision: a.revision.clone()
        }
    );
    assert_eq!(outcomes[1].status, ForgetStatus::Forgotten);
    assert_eq!(outcomes[2].status, ForgetStatus::NotFound);
    assert!(file_for(&p, &a.id).is_some());
    assert!(file_for(&p, &b.id).is_none());
    assert_eq!(p.snapshot().unwrap().tombstones.len(), 1);
}

// ============================================================================
// Quarantine and repair (design §2, §6 Management operations)
// ============================================================================

#[test]
fn a_lost_or_invalid_snapshot_quarantines_the_partition_until_repair() {
    let sb = Sandbox::new();
    let (p, a, _, _, forgotten) = seeded_for_recovery(&sb);
    p.forget(&[(forgotten.id, forgotten.revision.clone())])
        .unwrap();
    let snapshot_path = p.snapshot_path().to_path_buf();
    drop(p);

    for corrupt in [false, true] {
        if corrupt {
            std::fs::write(&snapshot_path, "{ not json").unwrap();
        } else {
            std::fs::remove_file(&snapshot_path).unwrap();
        }
        let p = sb.open_home(ALICE);
        let PartitionHealth::Quarantined(reason) = p.health() else {
            panic!("quarantined");
        };
        assert!(!reason.is_empty());
        assert!(p.snapshot().is_none());
        // Empty controls are never initialized over existing notes.
        assert_eq!(std::fs::exists(&snapshot_path).unwrap(), corrupt);
        // Mutations and reconciliation are refused; explicit reads work, unverified.
        let err = p
            .create(new_note("x", "b", NoteKind::Fact), Actor::UserSurface)
            .unwrap_err();
        assert_eq!(err.class(), "quarantined");
        assert!(err.to_string().contains("doctor --repair"));
        assert!(matches!(
            p.reconcile(),
            Err(MemoryFilesError::Quarantined { .. })
        ));
        assert!(matches!(
            p.update(
                &a.id,
                &a.revision,
                UpdateAction::Archive,
                Actor::UserSurface
            ),
            Err(MemoryFilesError::Quarantined { .. })
        ));
        let view = p.read(&a.id).unwrap();
        assert!(view.unverified);
        assert_eq!(view.derived.origin, Origin::External);
        assert!(!view.derived.recorded);
        assert_eq!(p.load().unwrap().health, p.health());
        // Repair: only with the current owner's key.
        let err = p.repair(&owner_key(BOB)).unwrap_err();
        assert!(err.to_string().contains("adopt"));
        let report = p.repair(&owner_key(ALICE)).unwrap();
        assert!(report.rebuilt);
        assert_eq!(report.notes_recorded, 3);
        assert!(report.malformed.is_empty());
        assert_eq!(p.health(), PartitionHealth::Ready);
        let snapshot = p.snapshot().unwrap();
        assert_eq!(snapshot.notes.len(), 3);
        assert!(
            snapshot
                .notes
                .values()
                .all(|r| r.origin == Origin::External)
        );
        assert!(
            snapshot.tombstones.is_empty(),
            "a rebuilt snapshot starts with no tombstones"
        );
        assert!(snapshot.repaired_at.is_some());
        assert!(snapshot.pending.is_none());
        assert_eq!(snapshot.root, p.root_canonical());
        assert_eq!(
            p.read(&a.id).unwrap().revision,
            a.revision,
            "no note file was rewritten"
        );
        let again = p.repair(&owner_key(ALICE)).unwrap();
        assert!(!again.rebuilt, "a healthy partition has nothing to rebuild");
        assert_eq!(
            p.snapshot().unwrap(),
            snapshot,
            "and is left exactly as it was"
        );
        // Back to Ready across a restart.
        drop(p);
        assert_eq!(sb.open_home(ALICE).health(), PartitionHealth::Ready);
    }
    // A snapshot bound to another directory is invalid too.
    let mut s = ControlSnapshot::load(&snapshot_path).unwrap().unwrap();
    s.root = "/elsewhere".into();
    std::fs::write(&snapshot_path, s.to_json()).unwrap();
    assert!(
        matches!(sb.open_home(ALICE).health(), PartitionHealth::Quarantined(r) if r.contains("binding"))
    );
}

#[test]
fn repair_refuses_a_duplicate_id_naming_both_files_and_touches_nothing() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    p.initialize().unwrap();
    let id = NoteId::parse("00000014-0000-4000-8000-0000000000dd").unwrap();
    let one = "---\nschema: 1\nid: 00000014-0000-4000-8000-0000000000dd\ntitle: Duplicate one\nkind: fact\nstatus: active\ncreated_at: 2026-09-23T11:00:00Z\nupdated_at: 2026-09-23T11:00:00Z\n---\n# Duplicate one\n";
    let two = one.replace("Duplicate one", "Duplicate two");
    let p1 = p.notes_dir().join(format!("{id}--duplicate-one.md"));
    let p2 = p.notes_dir().join(format!("{id}--duplicate-two.md"));
    std::fs::write(&p1, one).unwrap();
    std::fs::write(&p2, &two).unwrap();
    let loaded = p.load().unwrap();
    assert!(loaded.views.is_empty(), "neither is served");
    assert_eq!(loaded.duplicates, [(id, vec![p1.clone(), p2.clone()])]);
    assert!(matches!(p.read(&id), Err(MemoryFilesError::NotFound(_))));
    assert!(
        p.reconcile().unwrap().is_empty(),
        "a duplicate id is never recorded"
    );
    assert!(!p.snapshot().unwrap().notes.contains_key(&id));
    std::fs::remove_file(p.snapshot_path()).unwrap();
    drop(p);
    let p = sb.open_home(ALICE);
    assert!(matches!(p.health(), PartitionHealth::Quarantined(_)));
    let err = p.repair(&owner_key(ALICE)).unwrap_err();
    assert_eq!(err.class(), "fixture:duplicate_id");
    let msg = err.to_string();
    assert!(
        msg.contains(&id.to_string())
            && msg.contains("duplicate-one")
            && msg.contains("duplicate-two"),
        "{msg}"
    );
    assert_eq!(read_file(&p1), one.as_bytes());
    assert_eq!(read_file(&p2), two.as_bytes());
    assert!(matches!(p.health(), PartitionHealth::Quarantined(_)));
}

#[test]
fn malformed_files_are_kept_excluded_and_never_rewritten() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    p.initialize().unwrap();
    let traversal = "---\nschema: 1\nid: ../../state/memory-control/home.json\ntitle: Traversal\nkind: fact\nstatus: active\ncreated_at: 2026-09-23T10:03:00Z\nupdated_at: 2026-09-23T10:03:00Z\n---\n# Traversal\n";
    let path = p
        .notes_dir()
        .join("00000018-0000-4000-8000-0000000000a2--traversal.md");
    std::fs::write(&path, traversal).unwrap();
    let dropped = Note::parse(EXAMPLE.as_bytes()).unwrap();
    let dropped_path = p.notes_dir().join(dropped.file_name());
    std::fs::write(&dropped_path, dropped.serialize()).unwrap();
    let loaded = p.load().unwrap();
    assert_eq!(loaded.malformed.len(), 1);
    assert_eq!(loaded.malformed[0].path, path);
    assert!(loaded.malformed[0].reason.contains("id"));
    assert_eq!(loaded.views.len(), 1);
    assert_eq!(loaded.views[0].derived.origin, Origin::External);
    assert!(!loaded.views[0].derived.recorded);
    let changes = p.reconcile().unwrap();
    assert_eq!(
        changes,
        [ReconcileChange {
            id: dropped.id,
            kind: ReconcileChangeKind::Added,
            revision: Some(dropped.revision())
        }]
    );
    assert_eq!(
        p.snapshot().unwrap().notes[&dropped.id].origin,
        Origin::External
    );
    assert_eq!(
        read_file(&path),
        traversal.as_bytes(),
        "malformed file untouched"
    );
    assert_eq!(
        read_file(&dropped_path),
        dropped.serialize().as_bytes(),
        "external file untouched"
    );
    assert!(
        p.reconcile().unwrap().is_empty(),
        "a second scan finds nothing"
    );
}

// ============================================================================
// Reconciliation of external changes (design §5 Freshness)
// ============================================================================

#[test]
fn reconcile_reports_edits_deletions_and_new_files_once() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    let coffee = p
        .create(
            new_note(
                "Coffee order",
                "# Coffee\n\nflat white\n",
                NoteKind::Preference,
            ),
            Actor::UserSurface,
        )
        .unwrap();
    let parking = p
        .create(
            new_note("Parking spot", "# Parking\n\nbay 12\n", NoteKind::Fact),
            Actor::UserSurface,
        )
        .unwrap();
    assert!(p.reconcile().unwrap().is_empty());
    // Edit behind the service's back: verification sees it before any scan.
    let coffee_path = file_for(&p, &coffee.id).unwrap();
    let mut bytes = read_file(&coffee_path);
    bytes.extend_from_slice(b"\nSince September: oat-milk latte.\n");
    std::fs::write(&coffee_path, &bytes).unwrap();
    let view = p.read(&coffee.id).unwrap();
    assert_eq!(
        view.derived.origin,
        Origin::External,
        "a dirty note is never served as the service's"
    );
    assert!(!view.derived.recorded);
    assert!(
        view.note.body.contains("oat-milk latte"),
        "served from the re-read file"
    );
    // Delete behind its back.
    std::fs::remove_file(file_for(&p, &parking.id).unwrap()).unwrap();
    assert!(matches!(
        p.read(&parking.id),
        Err(MemoryFilesError::NotFound(_))
    ));
    // A hand-written new file.
    let gym = Note {
        id: NoteId::new(),
        title: "Gym days".into(),
        body: "# Gym\n\nMon, Thu\n".into(),
        ..Note::parse(EXAMPLE.as_bytes()).unwrap()
    };
    std::fs::write(p.notes_dir().join(gym.file_name()), gym.serialize()).unwrap();

    let mut changes = p.reconcile().unwrap();
    changes.sort_by_key(|c| c.id);
    let mut expected = vec![
        ReconcileChange {
            id: coffee.id,
            kind: ReconcileChangeKind::Modified,
            revision: Some(revision_of(&bytes)),
        },
        ReconcileChange {
            id: parking.id,
            kind: ReconcileChangeKind::Removed,
            revision: None,
        },
        ReconcileChange {
            id: gym.id,
            kind: ReconcileChangeKind::Added,
            revision: Some(gym.revision()),
        },
    ];
    expected.sort_by_key(|c| c.id);
    assert_eq!(changes, expected);
    let snapshot = p.snapshot().unwrap();
    assert_eq!(snapshot.notes[&coffee.id].origin, Origin::External);
    assert_eq!(snapshot.notes[&coffee.id].revision, revision_of(&bytes));
    assert!(!snapshot.notes.contains_key(&parking.id));
    assert_eq!(snapshot.notes[&gym.id].origin, Origin::External);
    assert!(p.read(&gym.id).unwrap().derived.recorded);
    // A rewrite with identical bytes changes nothing; a second scan is silent.
    std::fs::write(&coffee_path, &bytes).unwrap();
    assert!(p.reconcile().unwrap().is_empty());
    // An absent partition has nothing to reconcile and creates nothing.
    let bob = sb.open_home(BOB);
    assert!(bob.reconcile().unwrap().is_empty());
    assert_eq!(bob.health(), PartitionHealth::Absent);
}

// ============================================================================
// Derived supersession, authority, same fact_key (design §6, §7b)
// ============================================================================

fn mk(id: u8, status: NoteStatus, fact_key: Option<&str>, supersedes: &[u8], updated: u32) -> Note {
    let mut n = Note::parse(EXAMPLE.as_bytes()).unwrap();
    n.id = NoteId::parse(&format!("00000024-0000-4000-8000-0000000000{id:02x}")).unwrap();
    n.status = status;
    n.fact_key = fact_key.map(str::to_string);
    n.supersedes = supersedes
        .iter()
        .map(|s| NoteId::parse(&format!("00000024-0000-4000-8000-0000000000{s:02x}")).unwrap())
        .collect();
    n.updated_at = ts(2026, 9, 1, 10, 0, 0) + chrono::Duration::days(i64::from(updated));
    n.sources.clear();
    n
}

fn id_of(n: u8) -> NoteId {
    NoteId::parse(&format!("00000024-0000-4000-8000-0000000000{n:02x}")).unwrap()
}

#[test]
fn derived_supersession_follows_the_three_conditions_and_tombstones() {
    // Fixture 24's shape, in memory.
    let notes = vec![
        mk(0x01, NoteStatus::Active, Some("office.location"), &[], 0), // O1: forgotten R1 supersedes it
        mk(0x03, NoteStatus::Active, None, &[], 0),                    // O2
        mk(0x04, NoteStatus::Archived, None, &[0x03], 9), // R2: archived, supersedes O2
        mk(0x05, NoteStatus::Active, None, &[], 0),       // C0
        mk(0x06, NoteStatus::Candidate, None, &[0x05], 4), // K: candidate names C0
        mk(0x07, NoteStatus::Active, None, &[], 0),       // I: imported
        mk(0x08, NoteStatus::Active, None, &[0x07], 14),  // X: external names I
        mk(0x09, NoteStatus::Active, None, &[], 0),       // G: user; P (other partition) names it
        mk(0x0a, NoteStatus::Active, Some("team.meeting_day"), &[], 11), // W1: extracted_user, newer
        mk(0x0b, NoteStatus::Active, Some("team.meeting_day"), &[], 1),  // W2: user, older
    ];
    let mut snapshot = ControlSnapshot::new(
        "home-x",
        SnapshotScope::Home,
        "x",
        "u",
        "/r",
        ts(2026, 9, 1, 0, 0, 0),
    );
    let origins = [
        (0x01, Origin::ExtractedUser),
        (0x03, Origin::ExtractedUser),
        (0x04, Origin::ExtractedUser),
        (0x05, Origin::ExtractedUser),
        (0x06, Origin::ExtractedOther),
        (0x07, Origin::Imported),
        (0x08, Origin::External),
        (0x09, Origin::User),
        (0x0a, Origin::ExtractedUser),
        (0x0b, Origin::User),
    ];
    for (n, origin) in origins {
        let note = notes.iter().find(|x| x.id == id_of(n)).unwrap();
        snapshot.notes.insert(
            note.id,
            NoteRecord {
                revision: note.revision(),
                origin,
                written_at: note.updated_at,
            },
        );
    }
    snapshot.tombstones.push(Tombstone {
        id: id_of(0x02),
        origin: Origin::ExtractedUser,
        supersedes: vec![id_of(0x01)],
        suppressed_sources: vec!["message:2401".into()],
        forgotten_at: ts(2026, 9, 20, 10, 0, 0),
    });
    let revisions: Vec<String> = notes.iter().map(Note::revision).collect();
    let inputs: Vec<NoteInput<'_>> = notes
        .iter()
        .zip(&revisions)
        .map(|(note, revision)| NoteInput { note, revision })
        .collect();
    let d = derive(&inputs, &snapshot);
    let get = |n: u8| &d.notes[&id_of(n)];

    // A forgotten replacement does not revive its predecessor.
    assert_eq!(get(0x01).effective_status, EffectiveStatus::Superseded);
    assert_eq!(get(0x01).superseded_by, [id_of(0x02)]);
    // Nor does an archived one.
    assert_eq!(get(0x03).effective_status, EffectiveStatus::Superseded);
    assert_eq!(get(0x03).superseded_by, [id_of(0x04)]);
    assert_eq!(get(0x04).effective_status, EffectiveStatus::Archived);
    // A candidate supersedes nothing.
    assert_eq!(get(0x05).effective_status, EffectiveStatus::Active);
    assert_eq!(get(0x06).effective_status, EffectiveStatus::Candidate);
    assert!(!get(0x06).conflict);
    // external < imported: reported, not honored.
    assert_eq!(get(0x07).effective_status, EffectiveStatus::Active);
    assert!(get(0x07).superseded_by.is_empty());
    assert!(!get(0x07).conflict);
    assert_eq!(get(0x08).effective_status, EffectiveStatus::Active);
    assert!(get(0x08).conflict);
    assert_eq!(get(0x08).unhonored_links, [id_of(0x07)]);
    assert!(
        get(0x08).recallable(),
        "an un-honored link alone keeps a note recallable"
    );
    // A cross-partition link (P is not among these notes) is ignored.
    assert_eq!(get(0x09).effective_status, EffectiveStatus::Active);
    assert!(!get(0x09).conflict);
    // Same fact_key: user beats the newer extracted_user; loser flagged.
    assert_eq!(get(0x0b).same_key_loser_to, None);
    assert!(!get(0x0b).conflict);
    assert_eq!(get(0x0a).same_key_loser_to, Some(id_of(0x0b)));
    assert!(get(0x0a).conflict);
    assert!(!get(0x0a).recallable());
    assert_eq!(
        get(0x0a).effective_status,
        EffectiveStatus::Active,
        "the loser stays on disk and active"
    );
    // O1 is not in the office.location group once superseded.
    assert_eq!(get(0x01).same_key_loser_to, None);
    // doctor: one row per pair.
    assert_eq!(
        d.conflicts,
        [
            Conflict {
                kind: ConflictKind::SupersedesAuthority,
                note: id_of(0x08),
                target: id_of(0x07)
            },
            Conflict {
                kind: ConflictKind::SameFactKey,
                note: id_of(0x0a),
                target: id_of(0x0b)
            },
        ]
    );
}

#[test]
fn a_same_key_loser_with_an_unhonored_link_is_reported_once() {
    // Fixture 18: M1 (external) names U (user) and shares its fact_key.
    let u = mk(0x01, NoteStatus::Active, Some("deploy.approval"), &[], 0);
    let m1 = mk(
        0xa1,
        NoteStatus::Active,
        Some("deploy.approval"),
        &[0x01],
        2,
    );
    let mut snapshot = ControlSnapshot::new(
        "home-x",
        SnapshotScope::Home,
        "x",
        "u",
        "/r",
        ts(2026, 9, 1, 0, 0, 0),
    );
    snapshot.notes.insert(
        u.id,
        NoteRecord {
            revision: u.revision(),
            origin: Origin::User,
            written_at: u.updated_at,
        },
    );
    let (ru, rm) = (u.revision(), m1.revision());
    let d = derive(
        &[
            NoteInput {
                note: &u,
                revision: &ru,
            },
            NoteInput {
                note: &m1,
                revision: &rm,
            },
        ],
        &snapshot,
    );
    assert_eq!(
        d.notes[&m1.id].origin,
        Origin::External,
        "unrecorded = external"
    );
    assert!(d.notes[&m1.id].conflict);
    assert_eq!(d.notes[&m1.id].same_key_loser_to, Some(u.id));
    assert!(!d.notes[&m1.id].recallable());
    assert!(!d.notes[&u.id].conflict);
    assert_eq!(d.notes[&u.id].effective_status, EffectiveStatus::Active);
    assert_eq!(
        d.conflicts,
        [Conflict {
            kind: ConflictKind::SupersedesAuthority,
            note: m1.id,
            target: u.id
        }]
    );
    // Equal authority: the link is honored, so no same-key tie is left to break.
    snapshot.notes.insert(
        m1.id,
        NoteRecord {
            revision: m1.revision(),
            origin: Origin::User,
            written_at: m1.updated_at,
        },
    );
    let d = derive(
        &[
            NoteInput {
                note: &u,
                revision: &ru,
            },
            NoteInput {
                note: &m1,
                revision: &rm,
            },
        ],
        &snapshot,
    );
    assert_eq!(d.notes[&u.id].effective_status, EffectiveStatus::Superseded);
    assert!(d.conflicts.is_empty());
}

/// Design §3 rule 3 (§7c R9): two active `user` notes on one `fact_key`
/// tie on authority, so the note **written last** wins — and editing the
/// older one afterwards (a typo fix) does not flip it back, because the tie
/// reads `created_at`, not `updated_at`.
#[test]
fn a_same_key_tie_goes_to_the_note_written_last_and_an_edit_does_not_flip_it() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    let port = |body: &str| NewNote {
        fact_key: Some("lantern.service.port".into()),
        ..new_note("Lantern port", body, NoteKind::Fact)
    };
    let old = p
        .create(
            port("# Port\n\nLantern listens on 8080.\n"),
            Actor::UserSurface,
        )
        .unwrap();
    let new = p
        .create(
            port("# Port\n\nLantern listens on 8790.\n"),
            Actor::UserSurface,
        )
        .unwrap();
    assert_eq!(
        p.read(&old.id).unwrap().derived.same_key_loser_to,
        Some(new.id)
    );
    assert_eq!(p.read(&new.id).unwrap().derived.same_key_loser_to, None);

    let edited = p
        .update(
            &old.id,
            &old.revision,
            UpdateAction::Edit(NotePatch {
                body: Some("# Port\n\nLantern listens on port 8080.\n".into()),
                ..Default::default()
            }),
            Actor::UserSurface,
        )
        .unwrap();
    assert!(edited.written);
    let old_view = p.read(&old.id).unwrap();
    assert!(old_view.note.updated_at > p.read(&new.id).unwrap().note.updated_at);
    assert_eq!(
        old_view.derived.same_key_loser_to,
        Some(new.id),
        "an edit of the older note does not make it the newer knowledge"
    );
    assert_eq!(p.read(&new.id).unwrap().derived.same_key_loser_to, None);
}

/// Design §3 rule 3 (§7c R9): when both tied notes say when their fact was
/// observed, the newer observation wins even if it was written first.
#[test]
fn a_same_key_tie_goes_to_the_newer_observation_when_both_have_one() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    let db = |body: &str, observed: chrono::DateTime<Utc>| NewNote {
        fact_key: Some("lantern.staging.db".into()),
        observed_at: Some(observed),
        ..new_note("Staging database", body, NoteKind::Fact)
    };
    let june = p
        .create(
            db(
                "# DB\n\nlantern_stg on pg-staging-2\n",
                ts(2026, 6, 20, 10, 0, 0),
            ),
            Actor::UserSurface,
        )
        .unwrap();
    let march = p
        .create(
            db(
                "# DB\n\nlantern_staging on pg-staging-1\n",
                ts(2026, 3, 2, 10, 0, 0),
            ),
            Actor::UserSurface,
        )
        .unwrap();
    assert_eq!(
        p.read(&march.id).unwrap().derived.same_key_loser_to,
        Some(june.id)
    );
    assert_eq!(p.read(&june.id).unwrap().derived.same_key_loser_to, None);
}

/// The observation clock is read only when **every** note tied on authority
/// has an `observed_at`; otherwise the group is read on `created_at`. A
/// pairwise rule ("observed_at when both have one") is not an order over
/// three notes — here it would cycle — so the winner would depend on the
/// order the partition listed its files.
#[test]
fn a_same_key_group_reads_one_clock_whatever_the_input_order() {
    let at = |day: u32| ts(2026, 9, day, 10, 0, 0);
    let note = |id: u8, observed: Option<u32>, created: u32, updated: u32| {
        let mut n = mk(
            id,
            NoteStatus::Active,
            Some("team.meeting_day"),
            &[],
            updated,
        );
        n.observed_at = observed.map(|d| ts(2026, d, 1, 0, 0, 0));
        n.created_at = at(created);
        n
    };
    let derive_all = |notes: &[Note]| {
        let mut snapshot = ControlSnapshot::new(
            "home-x",
            SnapshotScope::Home,
            "x",
            "u",
            "/r",
            ts(2026, 9, 1, 0, 0, 0),
        );
        for n in notes {
            snapshot.notes.insert(
                n.id,
                NoteRecord {
                    revision: n.revision(),
                    origin: Origin::User,
                    written_at: n.updated_at,
                },
            );
        }
        let revisions: Vec<String> = notes.iter().map(Note::revision).collect();
        let inputs: Vec<NoteInput<'_>> = notes
            .iter()
            .zip(&revisions)
            .map(|(note, revision)| NoteInput { note, revision })
            .collect();
        let d = derive(&inputs, &snapshot);
        let winners: Vec<NoteId> = d
            .notes
            .values()
            .filter(|n| n.same_key_loser_to.is_none())
            .map(|n| n.id)
            .collect();
        assert_eq!(winners.len(), 1, "one winner per group");
        winners[0]
    };
    // One note has no observation: created_at decides — 0x02, written last
    // — although 0x01 was observed later and 0x03 was edited last.
    let mixed = [
        note(0x01, Some(8), 1, 0),
        note(0x02, Some(1), 3, 1),
        note(0x03, None, 2, 9),
    ];
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let notes: Vec<Note> = order.iter().map(|&i| mixed[i].clone()).collect();
        assert_eq!(derive_all(&notes), id_of(0x02), "input order {order:?}");
    }
    // Every note observed: the newest observation decides.
    let observed = [
        note(0x01, Some(8), 1, 0),
        note(0x02, Some(1), 3, 1),
        note(0x03, Some(5), 2, 9),
    ];
    assert_eq!(derive_all(&observed), id_of(0x01));
    // Every note observed at the same instant: `created_at` is next, not
    // `updated_at` — 0x09, written last, wins although 0x08 was edited last.
    let same_observation = [note(0x08, Some(5), 1, 9), note(0x09, Some(5), 3, 2)];
    assert_eq!(derive_all(&same_observation), id_of(0x09));
    // Written in the same second and never observed: the newer edit, then
    // the lowest id.
    let same_second = [note(0x04, None, 1, 2), note(0x05, None, 1, 7)];
    assert_eq!(derive_all(&same_second), id_of(0x05));
    let full_tie = [note(0x07, None, 1, 3), note(0x06, None, 1, 3)];
    assert_eq!(derive_all(&full_tie), id_of(0x06));
}

#[test]
fn source_forgotten_is_derived_and_an_edit_lifts_it() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    let src = |id: &str| NoteSource {
        kind: SourceType::Message,
        id: id.into(),
        observed_at: None,
    };
    let colour = p
        .create(
            NewNote {
                sources: vec![src("4812")],
                ..new_note(
                    "Favourite colour",
                    "# Colour\n\ngreen\n",
                    NoteKind::Preference,
                )
            },
            Actor::ModelTool,
        )
        .unwrap();
    let birthday = p
        .create(
            NewNote {
                sources: vec![src("4812")],
                ..new_note("Birthday", "# Birthday\n\n14 October\n", NoteKind::Fact)
            },
            Actor::ModelTool,
        )
        .unwrap();
    let other = p
        .create(
            NewNote {
                sources: vec![src("4813")],
                ..new_note("Other", "# Other\n\nx\n", NoteKind::Fact)
            },
            Actor::ModelTool,
        )
        .unwrap();
    // The clock ticks per call, so the forget is later than both records.
    let outcomes = p.forget(&[(colour.id, colour.revision.clone())]).unwrap();
    assert_eq!(outcomes[0].status, ForgetStatus::Forgotten);
    assert!(p.snapshot().unwrap().is_source_suppressed("message:4812"));
    let view = p.read(&birthday.id).unwrap();
    assert!(view.derived.source_forgotten);
    assert!(!view.derived.recallable(), "out of automatic recall");
    assert_eq!(
        view.derived.effective_status,
        EffectiveStatus::Active,
        "still readable"
    );
    assert!(!p.read(&other.id).unwrap().derived.source_forgotten);
    // Survives a restart; an edit through a user surface lifts it.
    drop(p);
    let p = sb.open_home(ALICE);
    assert!(p.read(&birthday.id).unwrap().derived.source_forgotten);
    let u = p
        .update(
            &birthday.id,
            &birthday.revision,
            edit_body("# Birthday\n\n14 October (confirmed)\n"),
            Actor::UserSurface,
        )
        .unwrap();
    assert!(u.written);
    let view = p.read(&birthday.id).unwrap();
    assert!(!view.derived.source_forgotten);
    assert_eq!(view.derived.origin, Origin::User);
    assert!(
        view.note.sources.iter().any(|s| s.id == "4812"),
        "the citation stays"
    );

    // Boundary: the clock is to the second, so a record and a forget can
    // share one. A note written in the forget's second is suppressed too (the
    // safe direction), an edit in that same second stays suppressed, and the
    // next second's edit lifts it.
    let offset = Arc::new(AtomicI64::new(0));
    let clock = {
        let offset = Arc::clone(&offset);
        move || ts(2026, 9, 23, 12, 0, 0) + chrono::Duration::seconds(offset.load(Ordering::SeqCst))
    };
    let p = Partition::open(&StoreScope::Home, BOB)
        .unwrap()
        .with_clock(clock);
    let gone = p
        .create(
            NewNote {
                sources: vec![src("77")],
                ..new_note("Gone", "# Gone\n\nx\n", NoteKind::Fact)
            },
            Actor::ModelTool,
        )
        .unwrap();
    let kept = p
        .create(
            NewNote {
                sources: vec![src("77")],
                ..new_note("Kept", "# Kept\n\ny\n", NoteKind::Fact)
            },
            Actor::ModelTool,
        )
        .unwrap();
    p.forget(&[(gone.id, gone.revision.clone())]).unwrap();
    let snapshot = p.snapshot().unwrap();
    assert_eq!(
        snapshot.notes[&kept.id].written_at, snapshot.tombstones[0].forgotten_at,
        "the record and the forget share a second"
    );
    assert!(
        p.read(&kept.id).unwrap().derived.source_forgotten,
        "a same-second write is suppressed"
    );
    let u = p
        .update(
            &kept.id,
            &kept.revision,
            edit_body("# Kept\n\ny (same second)\n"),
            Actor::UserSurface,
        )
        .unwrap();
    assert!(u.written);
    assert!(
        p.read(&kept.id).unwrap().derived.source_forgotten,
        "an edit in the forget's second stays suppressed"
    );
    offset.store(1, Ordering::SeqCst);
    let u = p
        .update(
            &kept.id,
            &u.revision,
            edit_body("# Kept\n\ny (next second)\n"),
            Actor::UserSurface,
        )
        .unwrap();
    assert!(u.written);
    assert!(
        !p.read(&kept.id).unwrap().derived.source_forgotten,
        "the next second's edit lifts it"
    );
}

#[test]
fn forgetting_a_replacement_keeps_its_predecessor_superseded_on_disk() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    let old = p
        .create(
            NewNote {
                fact_key: Some("user.timezone".into()),
                ..new_note("Timezone", "# TZ\n\nBerlin\n", NoteKind::Fact)
            },
            Actor::ModelTool,
        )
        .unwrap();
    let new = p
        .create(
            NewNote {
                fact_key: Some("user.timezone".into()),
                supersedes: Some((old.id, old.revision.clone())),
                sources: vec![NoteSource {
                    kind: SourceType::Message,
                    id: "4813".into(),
                    observed_at: None,
                }],
                ..new_note("Timezone", "# TZ\n\nTokyo\n", NoteKind::Fact)
            },
            Actor::ModelTool,
        )
        .unwrap();
    assert_eq!(p.read(&old.id).unwrap().derived.superseded_by, [new.id]);
    p.forget(&[(new.id, new.revision.clone())]).unwrap();
    let snapshot = p.snapshot().unwrap();
    let t = snapshot.tombstone(&new.id).unwrap();
    assert_eq!(t.supersedes, [old.id]);
    assert_eq!(t.origin, Origin::ExtractedUser);
    assert_eq!(t.suppressed_sources, ["message:4813"]);
    let view = p.read(&old.id).unwrap();
    assert_eq!(
        view.derived.effective_status,
        EffectiveStatus::Superseded,
        "not revived"
    );
    assert_eq!(view.derived.superseded_by, [new.id]);
    // Archiving a replacement does not revive either.
    let older = p
        .create(
            new_note("Parking", "# Parking\n\n18:00\n", NoteKind::Fact),
            Actor::ModelTool,
        )
        .unwrap();
    let newer = p
        .create(
            NewNote {
                supersedes: Some((older.id, older.revision.clone())),
                ..new_note("Parking", "# Parking\n\n19:00\n", NoteKind::Fact)
            },
            Actor::ModelTool,
        )
        .unwrap();
    p.update(
        &newer.id,
        &newer.revision,
        UpdateAction::Archive,
        Actor::ModelTool,
    )
    .unwrap();
    assert_eq!(
        p.read(&older.id).unwrap().derived.effective_status,
        EffectiveStatus::Superseded
    );
    assert_eq!(
        p.read(&newer.id).unwrap().derived.effective_status,
        EffectiveStatus::Archived
    );
    // Restoring is an explicit edit that removes the link — here, forgetting
    // the archived replacement still keeps the link through its tombstone.
    p.forget(&[(newer.id, p.read(&newer.id).unwrap().revision)])
        .unwrap();
    assert_eq!(
        p.read(&older.id).unwrap().derived.effective_status,
        EffectiveStatus::Superseded
    );
}

#[test]
fn a_cross_partition_link_is_ignored() {
    let sb = Sandbox::new();
    let alice = sb.open_home(ALICE);
    let a1 = alice
        .create(
            new_note("Indentation", "# I\n\ntabs\n", NoteKind::Preference),
            Actor::UserSurface,
        )
        .unwrap();
    let bob = sb.open_home(BOB);
    bob.initialize().unwrap();
    // Bob's file names Alice's note by hand.
    let mut b1 = Note::parse(EXAMPLE.as_bytes()).unwrap();
    b1.id = NoteId::new();
    b1.supersedes = vec![a1.id];
    std::fs::write(bob.notes_dir().join(b1.file_name()), b1.serialize()).unwrap();
    bob.reconcile().unwrap();
    assert_eq!(
        alice.read(&a1.id).unwrap().derived.effective_status,
        EffectiveStatus::Active
    );
    assert!(alice.load().unwrap().conflicts.is_empty());
    assert!(bob.load().unwrap().conflicts.is_empty());
    assert!(!bob.read(&b1.id).unwrap().derived.conflict);
    // And a project partition's link to a home note is ignored the same way.
    let project = sb.open_project(ALICE).unwrap();
    let mut p1 = Note::parse(EXAMPLE.as_bytes()).unwrap();
    p1.id = NoteId::new();
    p1.supersedes = vec![a1.id];
    project.initialize().unwrap();
    std::fs::write(project.notes_dir().join(p1.file_name()), p1.serialize()).unwrap();
    project.reconcile().unwrap();
    assert_eq!(
        alice.read(&a1.id).unwrap().derived.effective_status,
        EffectiveStatus::Active
    );
}

// ============================================================================
// Home-store-wide operations: adopt, listings, move snapshots (design §6)
// ============================================================================

#[test]
fn adopt_renames_the_home_and_every_project_snapshot_together() {
    let sb = Sandbox::new();
    let old_owner = "u_example_alice";
    let home = sb.open_home(old_owner);
    let h = home
        .create(
            new_note("Home note", "# H\n", NoteKind::Preference),
            Actor::UserSurface,
        )
        .unwrap();
    let project = sb.open_project(old_owner).unwrap();
    let pn = project
        .create(
            new_note("Project note", "# P\n", NoteKind::Fact),
            Actor::UserSurface,
        )
        .unwrap();
    let old_key = owner_key(old_owner);
    let old_project_key = project.partition_key().to_string();
    drop(home);
    drop(project);
    // Orphaned: a new owner id after a factory reset.
    let reborn = "u_example_alice_reborn";
    assert_eq!(
        list_owner_keys(&StoreScope::Home).unwrap(),
        std::slice::from_ref(&old_key)
    );
    let adopted = adopt(&old_key, reborn, ts(2026, 9, 24, 0, 0, 0)).unwrap();
    assert_eq!(adopted.len(), 2);
    let new_key = owner_key(reborn);
    assert!(adopted.contains(&format!("home-{new_key}")));
    assert!(
        adopted
            .iter()
            .any(|k| k.starts_with(&format!("proj-{new_key}-")))
    );
    assert_eq!(
        list_owner_keys(&StoreScope::Home).unwrap(),
        std::slice::from_ref(&new_key)
    );
    assert_eq!(
        list_owner_keys(&sb.project_scope()).unwrap(),
        std::slice::from_ref(&new_key)
    );
    let snapshots = list_snapshots().unwrap();
    assert_eq!(snapshots.len(), 2);
    assert!(snapshots.iter().all(|(_, s)| {
        let s = s.as_ref().unwrap();
        s.owner_key == new_key && s.owner_id == reborn && s.root.contains(&new_key)
    }));
    assert!(
        !snapshots
            .iter()
            .any(|(p, _)| p.to_string_lossy().contains(&old_project_key))
    );
    // The renamed partitions open Ready and serve their notes with their records.
    let home = sb.open_home(reborn);
    assert_eq!(home.health(), PartitionHealth::Ready);
    assert!(home.read(&h.id).unwrap().derived.recorded);
    let project = sb.open_project(reborn).unwrap();
    assert_eq!(project.health(), PartitionHealth::Ready);
    assert!(project.read(&pn.id).unwrap().derived.recorded);
    assert_eq!(home.snapshot().unwrap().notes[&h.id].origin, Origin::User);
    // Nothing of the old key remains, and adopting it again is an error.
    assert!(matches!(
        adopt(&old_key, reborn, ts(2026, 9, 24, 0, 0, 0)),
        Err(MemoryFilesError::InvalidInput(_))
    ));
    assert!(matches!(
        adopt(&new_key, reborn, ts(2026, 9, 24, 0, 0, 0)),
        Err(MemoryFilesError::InvalidInput(_))
    ));
}

#[test]
fn adopt_refuses_the_whole_command_on_one_collision() {
    let sb = Sandbox::new();
    let old = sb.open_home(ALICE);
    old.create(new_note("Old", "# O\n", NoteKind::Fact), Actor::UserSurface)
        .unwrap();
    let old_project = sb.open_project(ALICE).unwrap();
    old_project
        .create(
            new_note("Old project", "# OP\n", NoteKind::Fact),
            Actor::UserSurface,
        )
        .unwrap();
    // The current owner already has a project partition there.
    let reborn = "u_example_alice_reborn";
    let current_project = sb.open_project(reborn).unwrap();
    current_project
        .create(
            new_note("Mine", "# M\n", NoteKind::Fact),
            Actor::UserSurface,
        )
        .unwrap();
    drop((old, old_project, current_project));
    let before = list_snapshots().unwrap();
    let err = adopt(&owner_key(ALICE), reborn, ts(2026, 9, 24, 0, 0, 0)).unwrap_err();
    assert_eq!(err.class(), "fixture:adopt_collision");
    assert!(err.to_string().contains(&owner_key(reborn)));
    // Nothing partially adopted: the home partition of the old key is untouched.
    assert!(
        sb.home
            .join("memory/owners")
            .join(owner_key(ALICE))
            .is_dir()
    );
    assert!(
        !sb.home
            .join("memory/owners")
            .join(owner_key(reborn))
            .exists()
    );
    let after = list_snapshots().unwrap();
    assert_eq!(
        before.iter().map(|(p, _)| p).collect::<Vec<_>>(),
        after.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
}

/// The argument of `adopt` is an owner key (design §6), never a path: anything
/// but 32 lowercase hex characters is refused before a directory is joined,
/// so `../../state` cannot rename the store's `state/` into an owner directory.
#[test]
fn adopt_refuses_a_key_that_is_not_an_owner_key() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    let created = p
        .create(
            new_note("Home note", "# H\n", NoteKind::Fact),
            Actor::UserSurface,
        )
        .unwrap();
    let snapshot_path = p.snapshot_path().to_path_buf();
    drop(p);
    let state = sb.home.join("state");
    let bob_dir = sb.home.join("memory/owners").join(owner_key(BOB));
    let before = list_snapshots().unwrap();
    let upper = owner_key(ALICE).to_uppercase();
    for bad in [
        "../../state",
        "../..",
        "..",
        "",
        "state",
        upper.as_str(),
        "0123456789abcdef",
        "0123456789abcdef0123456789abcdef0",
        "0123456789abcdef0123456789abcdeg",
    ] {
        let err = adopt(bad, BOB, ts(2026, 9, 24, 0, 0, 0)).unwrap_err();
        assert!(
            matches!(err, MemoryFilesError::InvalidInput(_)),
            "{bad:?}: {err}"
        );
        assert!(err.to_string().contains("owner key"), "{bad:?}: {err}");
        assert!(
            state.is_dir() && snapshot_path.is_file(),
            "{bad:?}: state/ was moved"
        );
        assert!(!bob_dir.exists(), "{bad:?}: something was adopted");
    }
    let after = list_snapshots().unwrap();
    assert_eq!(
        before.iter().map(|(p, _)| p).collect::<Vec<_>>(),
        after.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    assert_eq!(
        list_owner_keys(&StoreScope::Home).unwrap(),
        std::slice::from_ref(&owner_key(ALICE))
    );
    // Alice's partition is intact and still serves its note with its record.
    let p = sb.open_home(ALICE);
    assert_eq!(p.health(), PartitionHealth::Ready);
    assert!(p.read(&created.id).unwrap().derived.recorded);
    // The predicate itself.
    assert!(is_owner_key(&owner_key(ALICE)));
    assert!(!is_owner_key(&upper));
    assert!(!is_owner_key("../../state"));
}

/// When the memory library is the first thing to touch a store — a loader, a
/// first `repair` or `adopt` on a copied-in partition — `state/` is created
/// private (0700) like every directory it makes, never with the umask default.
#[cfg(unix)]
#[test]
fn repair_and_adopt_create_state_privately_when_nothing_else_has() {
    let sb = Sandbox::new();
    let p = sb.open_home(ALICE);
    let created = p
        .create(
            new_note("Home note", "# H\n", NoteKind::Fact),
            Actor::UserSurface,
        )
        .unwrap();
    drop(p);
    let state = sb.home.join("state");
    // A partition copied in from elsewhere: notes, no control, no `state/`.
    std::fs::remove_dir_all(&state).unwrap();
    let p = sb.open_home(ALICE);
    assert!(matches!(p.health(), PartitionHealth::Quarantined(_)));
    assert!(!state.exists(), "opening creates nothing");
    let report = p.repair(&owner_key(ALICE)).unwrap();
    assert!(report.rebuilt);
    assert_eq!(fsx::mode_of(&state), Some(0o700));
    assert_eq!(fsx::mode_of(&state.join("memory-control")), Some(0o700));
    assert_eq!(fsx::mode_of(p.snapshot_path()), Some(0o600));
    assert!(p.read(&created.id).unwrap().derived.recorded);
    drop(p);
    // The same for `adopt` of a partition that has no snapshot at all.
    std::fs::remove_dir_all(&state).unwrap();
    let adopted = adopt(&owner_key(ALICE), BOB, ts(2026, 9, 24, 0, 0, 0)).unwrap();
    assert_eq!(adopted, [format!("home-{}", owner_key(BOB))]);
    assert_eq!(fsx::mode_of(&state), Some(0o700));
    assert_eq!(fsx::mode_of(&state.join("memory-control")), Some(0o700));
    let p = sb.open_home(BOB);
    assert_eq!(p.health(), PartitionHealth::Ready);
    assert_eq!(fsx::mode_of(p.snapshot_path()), Some(0o600));
    assert!(
        !p.read(&created.id).unwrap().derived.recorded,
        "external now"
    );
}

/// `snapshot_path_for` is public and takes text: only the two partition-key
/// shapes of design §2 reach `state/memory-control/`, so `../../x` is an
/// `InvalidInput`, never a path outside the control directory.
#[test]
fn snapshot_path_for_refuses_a_key_that_is_not_a_partition_key() {
    let sb = Sandbox::new();
    let key = owner_key(ALICE);
    let control = sb.home.join("state").join("memory-control");
    let home_key = format!("home-{key}");
    assert!(is_partition_key(&home_key));
    assert_eq!(
        snapshot_path_for(&home_key).unwrap(),
        control.join(format!("{home_key}.json"))
    );
    let proj_key = format!("proj-{key}-0123456789abcdef");
    assert!(is_partition_key(&proj_key));
    assert_eq!(
        snapshot_path_for(&proj_key).unwrap(),
        control.join(format!("{proj_key}.json"))
    );
    let upper = key.to_uppercase();
    for bad in [
        "../../x".to_string(),
        "home-../../x".to_string(),
        format!("home-{upper}"),
        format!("home-{key}-0123456789abcdef"),
        format!("home-{key}/x"),
        "home-".to_string(),
        "home".to_string(),
        "proj-".to_string(),
        format!("proj-{key}"),
        format!("proj-{key}-"),
        format!("proj-{key}-0123456789abcde"),
        format!("proj-{key}-0123456789abcdefg"),
        format!("proj-{key}-0123456789ABCDEF"),
        format!("proj-{key}--0123456789abcdef"),
        format!("proj-{upper}-0123456789abcdef"),
        format!("proj-{key}-../..0123456789"),
        key.clone(),
        String::new(),
    ] {
        assert!(!is_partition_key(&bad), "{bad:?}");
        let err = snapshot_path_for(&bad).unwrap_err();
        assert!(
            matches!(err, MemoryFilesError::InvalidInput(_)),
            "{bad:?}: {err}"
        );
        assert!(err.to_string().contains("partition key"), "{bad:?}: {err}");
    }
    assert!(!control.exists(), "a path query creates nothing");
}

#[test]
fn move_snapshots_are_classified_stale_abandoned_or_orphaned() {
    let dir = tempdir().unwrap();
    let exists = dir.path().join("exists");
    std::fs::create_dir(&exists).unwrap();
    let gone = dir.path().join("gone");
    let at = ts(2026, 9, 23, 0, 0, 0);
    let mut new_after_move = ControlSnapshot::new(
        "proj-a-new",
        SnapshotScope::Project,
        "a",
        "u",
        exists.to_string_lossy(),
        at,
    );
    new_after_move.moved_from = Some("proj-a-old".into());
    let old_after_move = ControlSnapshot::new(
        "proj-a-old",
        SnapshotScope::Project,
        "a",
        "u",
        gone.to_string_lossy(),
        at,
    );
    let mut never_moved = ControlSnapshot::new(
        "proj-a-planned",
        SnapshotScope::Project,
        "a",
        "u",
        dir.path().join("planned").to_string_lossy(),
        at,
    );
    never_moved.moved_from = Some("proj-a-new".into());
    let orphan = ControlSnapshot::new(
        "proj-a-lost",
        SnapshotScope::Project,
        "a",
        "u",
        dir.path().join("lost").to_string_lossy(),
        at,
    );
    let all = vec![new_after_move, old_after_move, never_moved, orphan];
    let mut classified = classify_move_snapshots(&all);
    classified.sort();
    assert_eq!(
        classified,
        [
            ("proj-a-lost".to_string(), MoveSnapshotState::Orphaned),
            ("proj-a-old".to_string(), MoveSnapshotState::Stale),
            ("proj-a-planned".to_string(), MoveSnapshotState::Abandoned),
        ]
    );
}

// ============================================================================
// The workspace move's snapshot steps (design §3, five steps; §6 recovery
// table, row *workspace move*; fixture 17)
// ============================================================================

/// The design's step 2: before anything is renamed, the twin snapshot exists
/// under the new key, bound to the directory the partition will have, with
/// `moved_from` naming the old key and the notes copied; the old snapshot is
/// untouched. Until step 5 the twin is what `doctor` calls *abandoned* — the
/// move never happened as far as the disk can tell.
#[test]
fn begin_project_move_writes_the_twin_snapshot_under_the_new_key() {
    let sb = Sandbox::new();
    let p = sb.open_project(ALICE).unwrap();
    let created = p
        .create(
            new_note(
                "Release step",
                "# Release step\n\nTag first.\n",
                NoteKind::Procedure,
            ),
            Actor::UserSurface,
        )
        .unwrap();
    let old_snapshot = p
        .snapshot()
        .expect("the first write laid the partition out");
    let old_key = p.partition_key().to_string();
    let new_project = tempdir().unwrap();
    let new_root = new_project.path().canonicalize().unwrap().join("moved");

    let moves =
        begin_project_move(&sb.project_root(), &new_root, ts(2026, 9, 24, 8, 0, 0)).unwrap();
    assert_eq!(moves.len(), 1, "one owner, one partition: {moves:?}");
    let mv = &moves[0];
    assert_eq!(mv.old_key, old_key);
    assert!(
        mv.new_key
            .starts_with(&format!("proj-{}-", owner_key(ALICE)))
    );
    assert_ne!(mv.new_key, mv.old_key);
    assert_eq!(mv.old_path, snapshot_path_for(&old_key).unwrap());
    assert_eq!(mv.new_path, snapshot_path_for(&mv.new_key).unwrap());

    // The old snapshot is byte for byte what it was.
    assert_eq!(
        ControlSnapshot::load(&mv.old_path).unwrap().unwrap(),
        old_snapshot
    );
    // The twin binds the directory the rename will create, carries the notes,
    // and says where it came from.
    let twin = ControlSnapshot::load(&mv.new_path)
        .unwrap()
        .expect("the twin exists");
    assert_eq!(twin.partition_key, mv.new_key);
    assert_eq!(twin.moved_from.as_deref(), Some(old_key.as_str()));
    assert_eq!(twin.owner_key, old_snapshot.owner_key);
    assert_eq!(twin.owner_id, ALICE);
    assert_eq!(twin.notes, old_snapshot.notes);
    assert!(twin.notes.contains_key(&created.id));
    assert_eq!(
        Path::new(&twin.root),
        new_root
            .join(".openalpaca")
            .join("memory")
            .join("owners")
            .join(owner_key(ALICE))
    );
    assert!(
        !new_root.exists(),
        "step 2 writes a snapshot, never a directory"
    );

    // What doctor says between step 2 and the rename: the twin is abandoned.
    let all: Vec<ControlSnapshot> = list_snapshots()
        .unwrap()
        .into_iter()
        .filter_map(|(_, s)| s.ok())
        .collect();
    assert_eq!(
        classify_move_snapshots(&all),
        [(mv.new_key.clone(), MoveSnapshotState::Abandoned)]
    );
}

/// Once the directory has moved (step 4), the old snapshot is *stale*; step 5
/// deletes it and clears `moved_from`, after which the partition opens at the
/// new root with the same note at the same revision and doctor has nothing
/// to say.
#[test]
fn finish_project_move_deletes_the_old_snapshot_and_clears_moved_from() {
    let sb = Sandbox::new();
    let p = sb.open_project(ALICE).unwrap();
    let created = p
        .create(
            new_note(
                "Release step",
                "# Release step\n\nTag first.\n",
                NoteKind::Procedure,
            ),
            Actor::UserSurface,
        )
        .unwrap();
    let old_key = p.partition_key().to_string();
    let new_project = tempdir().unwrap();
    let new_root = new_project.path().canonicalize().unwrap().join("moved");
    let moves =
        begin_project_move(&sb.project_root(), &new_root, ts(2026, 9, 24, 8, 0, 0)).unwrap();

    // Step 4, by hand: the whole store travels.
    std::fs::create_dir_all(&new_root).unwrap();
    std::fs::rename(
        sb.project_root().join(".openalpaca"),
        new_root.join(".openalpaca"),
    )
    .unwrap();
    let all = || -> Vec<ControlSnapshot> {
        list_snapshots()
            .unwrap()
            .into_iter()
            .filter_map(|(_, s)| s.ok())
            .collect()
    };
    assert_eq!(
        classify_move_snapshots(&all()),
        [(old_key.clone(), MoveSnapshotState::Stale)],
        "after the rename the old snapshot is bound to a directory that is gone"
    );

    finish_project_move(&moves, ts(2026, 9, 24, 8, 0, 1)).unwrap();

    assert!(
        !moves[0].old_path.exists(),
        "step 5 deletes the old-key snapshot"
    );
    let twin = ControlSnapshot::load(&moves[0].new_path).unwrap().unwrap();
    assert_eq!(twin.moved_from, None, "step 5 clears moved_from");
    assert!(classify_move_snapshots(&all()).is_empty());

    let moved = Partition::open(&StoreScope::Project(new_root.clone()), ALICE).unwrap();
    assert_eq!(moved.partition_key(), moves[0].new_key);
    assert_eq!(moved.health(), PartitionHealth::Ready);
    let view = moved.read(&created.id).unwrap();
    assert_eq!(view.revision, created.revision);
    assert_eq!(view.derived.origin, Origin::User);
    assert!(
        view.derived.recorded,
        "the twin recorded the revision, so the note is not `external`"
    );

    // Idempotent: finishing again is a no-op, not an error.
    finish_project_move(&moves, ts(2026, 9, 24, 8, 0, 2)).unwrap();
}

/// A project with no memory partition has nothing to re-key: an empty plan,
/// and no file written. Two owners' partitions are both re-keyed.
#[test]
fn begin_project_move_covers_every_owner_and_nothing_else() {
    let sb = Sandbox::new();
    let new_project = tempdir().unwrap();
    let new_root = new_project.path().canonicalize().unwrap().join("moved");
    let control = sb.home.join("state").join("memory-control");

    let none = begin_project_move(&sb.project_root(), &new_root, ts(2026, 9, 24, 8, 0, 0)).unwrap();
    assert!(none.is_empty());
    assert!(!control.exists(), "nothing to move, nothing written");

    for owner in [ALICE, BOB] {
        sb.open_project(owner)
            .unwrap()
            .create(
                new_note("A note", "# A note\n\nBody.\n", NoteKind::Fact),
                Actor::UserSurface,
            )
            .unwrap();
    }
    // The home partition is not the project's: it stays where it is.
    sb.open_home(ALICE)
        .create(
            new_note("Global", "# Global\n\nBody.\n", NoteKind::Preference),
            Actor::UserSurface,
        )
        .unwrap();
    let before = list_snapshots().unwrap().len();
    assert_eq!(before, 3);

    let moves =
        begin_project_move(&sb.project_root(), &new_root, ts(2026, 9, 24, 8, 0, 0)).unwrap();
    let mut owners: Vec<String> = moves
        .iter()
        .map(|m| {
            ControlSnapshot::load(&m.new_path)
                .unwrap()
                .unwrap()
                .owner_id
        })
        .collect();
    owners.sort();
    assert_eq!(owners, [ALICE, BOB]);
    assert_eq!(
        list_snapshots().unwrap().len(),
        5,
        "two twins, three originals"
    );
    assert!(
        moves
            .iter()
            .all(|m| !m.old_key.starts_with("home-") && !m.new_key.starts_with("home-")),
        "{moves:?}"
    );
}

// ============================================================================
// `doctor`, read-only (design §6, *Management operations*; plan 3.8)
// ============================================================================

/// The read-only scan names, per partition: the owner's own (home and the
/// named project), every other owner's as an orphan, a partition whose
/// snapshot is gone as quarantined, the same-`fact_key` conflict, and a
/// move that never finished as an abandoned snapshot — and it opens nothing
/// for writing: a project `.gitignore` the service would re-assert stays as
/// it was found.
#[test]
fn doctor_lists_partitions_orphans_conflicts_and_move_snapshots_without_writing() {
    let sb = Sandbox::new();
    let home = sb.open_home(ALICE);
    let mut first = new_note("Editor", "# Editor\n\nvim\n", NoteKind::Preference);
    first.fact_key = Some("editor".into());
    home.create(first, Actor::UserSurface).unwrap();
    let mut second = new_note(
        "Editor, again",
        "# Editor\n\nneovim\n",
        NoteKind::Preference,
    );
    second.fact_key = Some("editor".into());
    home.create(second, Actor::UserSurface).unwrap();
    let project = sb.open_project(ALICE).unwrap();
    project
        .create(
            new_note("Release", "# Release\n\nTag first.\n", NoteKind::Procedure),
            Actor::UserSurface,
        )
        .unwrap();
    // Bob's home partition: an orphan when Alice asks.
    sb.open_home(BOB)
        .create(
            new_note("Bob's", "# B\n", NoteKind::Fact),
            Actor::UserSurface,
        )
        .unwrap();
    // Carol's, with its snapshot deleted: an orphan nobody can open by id.
    let carol = "u_example_carol";
    let carol_home = sb.open_home(carol);
    carol_home
        .create(
            new_note("Carol's", "# C\n", NoteKind::Fact),
            Actor::UserSurface,
        )
        .unwrap();
    let carol_key = carol_home.partition_key().to_string();
    std::fs::remove_file(carol_home.snapshot_path()).unwrap();
    // A move begun and never finished: the twin binds a directory that does
    // not exist while the old one still stands — abandoned.
    let new_root = sb.outside.path().join("moved-project");
    let moves =
        begin_project_move(&sb.project_root(), &new_root, ts(2026, 9, 24, 9, 0, 0)).unwrap();
    assert_eq!(moves.len(), 1);
    // A cloned repository's force-added ignore file: `Partition::open` would
    // rewrite it, the doctor must not.
    let gitignore = project.memory_dir().join(".gitignore");
    std::fs::write(&gitignore, "!*\n").unwrap();
    drop((home, project, carol_home));

    let report = doctor(ALICE, Some(&sb.project_root())).unwrap();

    assert_eq!(report.owner_key, owner_key(ALICE));
    let by_key = |key: &str| {
        report
            .partitions
            .iter()
            .find(|p| p.partition_key == key)
            .unwrap_or_else(|| panic!("{key} is reported: {:?}", report.partitions))
    };
    let alice_home = by_key(&home_partition_key(&owner_key(ALICE)));
    assert!(!alice_home.orphan);
    assert_eq!(alice_home.health, PartitionHealth::Ready);
    assert_eq!(alice_home.notes, 2);
    assert_eq!(alice_home.unverified, 0);
    assert_eq!(alice_home.conflicts.len(), 1);
    assert_eq!(alice_home.conflicts[0].kind, ConflictKind::SameFactKey);
    let alice_project = report
        .partitions
        .iter()
        .find(|p| p.scope == SnapshotScope::Project && p.owner_key == owner_key(ALICE))
        .expect("the project partition is reported");
    assert!(!alice_project.orphan);
    assert_eq!(alice_project.notes, 1);
    let bob_home = by_key(&home_partition_key(&owner_key(BOB)));
    assert!(bob_home.orphan);
    assert_eq!(bob_home.notes, 1);
    assert_eq!(bob_home.health, PartitionHealth::Ready);
    let carol_home = by_key(&carol_key);
    assert!(carol_home.orphan);
    assert!(
        matches!(carol_home.health, PartitionHealth::Quarantined(_)),
        "{:?}",
        carol_home.health
    );
    assert_eq!(carol_home.notes, 1);
    assert_eq!(
        carol_home.unverified, 1,
        "nothing is verified without a snapshot"
    );
    assert_eq!(report.move_snapshots.len(), 1);
    assert_eq!(report.move_snapshots[0].state, MoveSnapshotState::Abandoned);
    assert_eq!(report.move_snapshots[0].partition_key, moves[0].new_key);
    assert!(report.refused.is_empty(), "{:?}", report.refused);
    assert!(report.unreadable_snapshots.is_empty());
    assert_eq!(
        std::fs::read_to_string(&gitignore).unwrap(),
        "!*\n",
        "the doctor changes nothing"
    );
}
