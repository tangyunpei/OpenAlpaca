//! The API contract, checked from both sides (API-CONTRACT; the owner's
//! decision of 2026-10-30).
//!
//! The daemon's route tests write a golden sample of each response the GUI
//! reads through `apps/openalpaca-gui/src/lib/api/`'s wrappers, and of each
//! `ServerEvent` variant as the events socket sends it, into one checked-in
//! folder, [`DIR`]; the GUI's vitest (`src/lib/api/contract.test.ts`) parses
//! each sample against the TypeScript type that mirrors it. The repository's
//! existing pattern, as `hidden-chars.json` is read by `utils::text` and by
//! `lib/hidden-chars.test.ts`. Since CLI-PROTOCOL two more kinds share the
//! folder and its check: the **request bodies** the routes read (`requests/`,
//! each its type's wire names, [`requests`]), against which vitest checks
//! the keys the GUI's wrappers send and the CLI's own tests the keys the CLI
//! sends; and the **responses only the CLI reads** (`cli/`), against which
//! the CLI's tests check the keys it reads — under the owner's rule that the
//! CLI and the app use one daemon, both clients are held to one `PROTOCOL`.
//!
//! **A sample is a shape, not a recording.** Every scalar is written as its
//! kind's neutral value (`""`, `0`, `false`; `null` stays `null`), an array
//! as one element holding every key its elements carry, and an object whose
//! keys are data (a map keyed by ids) as one `"*"` key — so no temp path,
//! pid, id or clock reaches the repository. An event sample keeps its
//! `type`. A sample moves when the wire's shape moves, and also when its
//! producer's fixture does: a fixture that leaves a nested value `null` or
//! absent drops that value's key paths, which reads as a removal.
//!
//! **The checks** ([`the_samples_are_the_folder`]): a sample that differs
//! from its file, a file no producer writes any more and a producer's sample
//! with no file all fail, until the folder is rewritten with
//! `OPENALPACA_CONTRACT_WRITE=1`. A **removal** — a key path in the file the
//! fresh sample lacks (a rename is a removal and an addition), or a whole
//! sample no longer written (its producer gone) — fails even then, unless
//! `openalpaca_app_support::protocol::PROTOCOL` is above the protocol the
//! folder records (`protocol.json`): the PROTOCOL rule (CLAUDE.md,
//! *Storage*). An addition — a new key path or a new sample — needs no
//! bump. The rewrite records the new `PROTOCOL`. The check reads bodies and
//! events only: a sample's name is its producer's literal, never read from
//! the router, so a route path renamed or removed in `router.rs` alone moves
//! no sample.

mod events;
pub(crate) mod requests;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// The folder, beside the TypeScript types it is checked against.
pub(crate) const DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../openalpaca-gui/src/lib/api/contract"
);

/// The variable that lets a run rewrite the folder (never a removal without
/// a `PROTOCOL` bump).
const WRITE_ENV: &str = "OPENALPACA_CONTRACT_WRITE";

/// Where a sample lives: `responses/`, `events/`, `requests/` or `cli/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    Response,
    Event,
    /// A request body a route reads (CLI-PROTOCOL).
    Request,
    /// A response only the CLI reads (CLI-PROTOCOL).
    CliResponse,
}

impl Kind {
    /// Every kind, as the folder is read: a kind missing here would leave
    /// its files unread, and a removal in them unseen.
    const ALL: [Kind; 4] = [
        Kind::Response,
        Kind::Event,
        Kind::Request,
        Kind::CliResponse,
    ];

    fn folder(self) -> &'static str {
        match self {
            Kind::Response => "responses",
            Kind::Event => "events",
            Kind::Request => "requests",
            Kind::CliResponse => "cli",
        }
    }
}

/// The samples one run produces, by kind and name.
#[derive(Default)]
pub(crate) struct Samples {
    items: BTreeMap<(Kind, String), Value>,
}

impl Samples {
    /// A response's sample, named after its route (`get-artifacts-id`).
    pub(crate) fn response(&mut self, name: &str, body: Value) {
        self.response_with_maps(name, body, &[]);
    }

    /// As [`Self::response`], where each path in `maps` (key paths as
    /// [`key_paths`] spells them, `""` the body itself) is an object keyed
    /// by data, written as one `"*"` key.
    pub(crate) fn response_with_maps(&mut self, name: &str, body: Value, maps: &[&str]) {
        self.insert(Kind::Response, name, shape(&body, "", maps, false));
    }

    /// A request body's sample, named after its route (`put-artifacts-id-pin`):
    /// the type's wire names, as [`requests::Body`] reads them.
    pub(crate) fn request(&mut self, name: &str, body: requests::Body) {
        self.insert(
            Kind::Request,
            name,
            shape(&body.into_value(), "", &[], false),
        );
    }

    /// A response only the CLI reads, named after its route (`get-me`),
    /// shaped as [`Self::response`] shapes one.
    pub(crate) fn cli_response(&mut self, name: &str, body: Value) {
        self.insert(Kind::CliResponse, name, shape(&body, "", &[], false));
    }

    /// An event's sample, named after its `type`.
    pub(crate) fn event(&mut self, body: Value) {
        let name = body["type"]
            .as_str()
            .expect("an event frame carries its type")
            .to_string();
        self.insert(Kind::Event, &name, shape(&body, "", &[], true));
    }

    fn insert(&mut self, kind: Kind, name: &str, shaped: Value) {
        assert!(
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'),
            "a sample's name is lowercase ASCII, digits, '-' and '_': {name:?}"
        );
        let previous = self.items.insert((kind, name.to_string()), shaped);
        assert!(previous.is_none(), "sample {name:?} written twice");
    }
}

/// `value` as a shape: scalars neutral, arrays merged to one element, the
/// objects at `maps` keyed by `"*"`. With `keep_type`, the top-level `type`
/// string stays as it is (an event's discriminant).
fn shape(value: &Value, path: &str, maps: &[&str], keep_type: bool) -> Value {
    match value {
        Value::Null => Value::Null,
        Value::Bool(_) => Value::Bool(false),
        Value::Number(_) => Value::from(0),
        Value::String(_) => Value::String(String::new()),
        Value::Array(items) => {
            let inner = format!("{path}[]");
            let merged = items
                .iter()
                .map(|item| shape(item, &inner, maps, false))
                .reduce(merge);
            Value::Array(merged.into_iter().collect())
        }
        Value::Object(fields) if maps.contains(&path) => {
            let inner = join(path, "*");
            let merged = fields
                .values()
                .map(|item| shape(item, &inner, maps, false))
                .reduce(merge);
            let mut out = Map::new();
            if let Some(merged) = merged {
                out.insert("*".to_string(), merged);
            }
            Value::Object(out)
        }
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, field)| {
                    let shaped = if keep_type && path.is_empty() && key == "type" {
                        field.clone()
                    } else {
                        shape(field, &join(path, key), maps, false)
                    };
                    (key.clone(), shaped)
                })
                .collect(),
        ),
    }
}

/// Two shapes as one holding every key either holds; where one is `null`
/// the other wins, otherwise the first.
fn merge(a: Value, b: Value) -> Value {
    match (a, b) {
        (Value::Object(mut a), Value::Object(b)) => {
            for (key, value) in b {
                let merged = match a.remove(&key) {
                    Some(existing) => merge(existing, value),
                    None => value,
                };
                a.insert(key, merged);
            }
            Value::Object(a)
        }
        (Value::Array(a), Value::Array(b)) => {
            let merged = a.into_iter().chain(b).reduce(merge);
            Value::Array(merged.into_iter().collect())
        }
        (Value::Null, other) => other,
        (first, _) => first,
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

/// Every key path a shape holds: `a`, `a.b`, `a[]`, `a[].c`, `a.*`; the
/// body's own array is `[]`.
pub(crate) fn key_paths(value: &Value) -> BTreeSet<String> {
    fn walk(value: &Value, path: &str, out: &mut BTreeSet<String>) {
        match value {
            Value::Object(fields) => {
                for (key, field) in fields {
                    let here = join(path, key);
                    out.insert(here.clone());
                    walk(field, &here, out);
                }
            }
            Value::Array(items) => {
                let here = format!("{path}[]");
                out.insert(here.clone());
                for item in items {
                    walk(item, &here, out);
                }
            }
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    walk(value, "", &mut out);
    out
}

/// What the folder holds on disk: the protocol it records and each sample.
struct Folder {
    protocol: Option<u32>,
    samples: BTreeMap<(Kind, String), Value>,
}

impl Folder {
    fn read(dir: &Path) -> Self {
        let protocol = std::fs::read(dir.join("protocol.json")).ok().map(|bytes| {
            let value: Value = serde_json::from_slice(&bytes).expect("protocol.json is JSON");
            u32::try_from(
                value["protocol"]
                    .as_u64()
                    .expect("protocol.json names a protocol"),
            )
            .expect("a u32 protocol")
        });
        let mut samples = BTreeMap::new();
        for kind in Kind::ALL {
            let Ok(entries) = std::fs::read_dir(dir.join(kind.folder())) else {
                continue;
            };
            for entry in entries {
                let path = entry.expect("a folder entry").path();
                // A hidden file (Finder's `.DS_Store`) is the desktop's, not
                // a sample; vitest's glob passes it over too.
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'))
                {
                    continue;
                }
                let Some(name) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_suffix(".json"))
                else {
                    panic!("{} is not a .json sample", path.display());
                };
                let value: Value = serde_json::from_slice(&std::fs::read(&path).expect("read"))
                    .unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()));
                samples.insert((kind, name.to_string()), value);
            }
        }
        Self { protocol, samples }
    }
}

/// One file's body as the folder writes it.
fn render(value: &Value) -> String {
    let mut text = serde_json::to_string_pretty(value).expect("a shape serializes");
    text.push('\n');
    text
}

fn sample_path(dir: &Path, kind: Kind, name: &str) -> PathBuf {
    dir.join(kind.folder()).join(format!("{name}.json"))
}

/// The outcome of comparing a run's samples with the folder.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Verdict {
    /// What fails the run, one sentence each.
    pub(crate) problems: Vec<String>,
    /// What a rewrite would change, if allowed: files to write and remove,
    /// and the protocol to record.
    writes: Vec<(Kind, String)>,
    removes: Vec<(Kind, String)>,
    record: Option<u32>,
}

/// The comparison, apart from the disk, so the rules are tested on their
/// own: `protocol` is this build's `PROTOCOL`, `write` whether the run may
/// rewrite.
fn judge(
    fresh: &BTreeMap<(Kind, String), Value>,
    folder: &Folder,
    protocol: u32,
    write: bool,
) -> Verdict {
    let mut verdict = Verdict::default();
    let recorded = folder.protocol;
    let bumped = match recorded {
        Some(recorded) if recorded > protocol => {
            verdict.problems.push(format!(
                "the folder records protocol {recorded}, above this build's PROTOCOL {protocol}: \
                 PROTOCOL never moves back"
            ));
            return verdict;
        }
        Some(recorded) => recorded < protocol,
        // A folder with samples and no recorded protocol lost its record:
        // treating that as a bump would let any removal through.
        None if !folder.samples.is_empty() => {
            verdict.problems.push(format!(
                "protocol.json is missing beside {} samples: restore it; only an empty \
                 folder is written fresh",
                folder.samples.len()
            ));
            return verdict;
        }
        None => true,
    };
    let rewrite = "rewrite the folder with OPENALPACA_CONTRACT_WRITE=1";
    let bump = "a removed or renamed field, route or event variant bumps \
                openalpaca_app_support::protocol::PROTOCOL in the same commit \
                (CLAUDE.md, Storage)";

    for ((kind, name), value) in fresh {
        let label = format!("{}/{name}", kind.folder());
        match folder.samples.get(&(*kind, name.clone())) {
            None => {
                if write {
                    verdict.writes.push((*kind, name.clone()));
                } else {
                    verdict.problems.push(format!(
                        "{label}: written by the tests, absent from the folder; {rewrite}"
                    ));
                }
            }
            Some(on_disk) if on_disk == value => {}
            Some(on_disk) => {
                let fresh_paths = key_paths(value);
                let removed: Vec<String> = key_paths(on_disk)
                    .into_iter()
                    .filter(|p| !fresh_paths.contains(p))
                    .collect();
                if !removed.is_empty() && !bumped {
                    verdict.problems.push(format!(
                        "{label}: no longer carries {}; {bump}",
                        removed.join(", ")
                    ));
                } else if write {
                    verdict.writes.push((*kind, name.clone()));
                } else {
                    verdict
                        .problems
                        .push(format!("{label}: differs from the folder; {rewrite}"));
                }
            }
        }
    }
    for (kind, name) in folder.samples.keys() {
        if fresh.contains_key(&(*kind, name.clone())) {
            continue;
        }
        let label = format!("{}/{name}", kind.folder());
        if !bumped {
            verdict
                .problems
                .push(format!("{label}: no test writes it any more; {bump}"));
        } else if write {
            verdict.removes.push((*kind, name.clone()));
        } else {
            verdict
                .problems
                .push(format!("{label}: no test writes it any more; {rewrite}"));
        }
    }
    if recorded != Some(protocol) {
        if write {
            verdict.record = Some(protocol);
        } else {
            verdict.problems.push(format!(
                "the folder records protocol {recorded:?}, this build's PROTOCOL is {protocol}; \
                 {rewrite}"
            ));
        }
    }
    if !verdict.problems.is_empty() {
        verdict.writes.clear();
        verdict.removes.clear();
        verdict.record = None;
    }
    verdict
}

/// Apply a verdict's rewrite to `dir`.
fn apply(dir: &Path, fresh: &BTreeMap<(Kind, String), Value>, verdict: &Verdict) {
    for (kind, name) in &verdict.writes {
        let path = sample_path(dir, *kind, name);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("create the folder");
        std::fs::write(&path, render(&fresh[&(*kind, name.clone())])).expect("write a sample");
    }
    for (kind, name) in &verdict.removes {
        std::fs::remove_file(sample_path(dir, *kind, name)).expect("remove a sample");
    }
    if let Some(protocol) = verdict.record {
        std::fs::create_dir_all(dir).expect("create the folder");
        std::fs::write(
            dir.join("protocol.json"),
            render(&serde_json::json!({ "protocol": protocol })),
        )
        .expect("write protocol.json");
    }
}

/// A route's response as its JSON body, refusing anything but a success:
/// a sample is what the GUI reads when the route answers.
pub(crate) async fn ok_body(response: axum::response::Response) -> Value {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read the body");
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert!(
        status.is_success(),
        "a sample's route answered {status}: {body}"
    );
    body
}

/// Every producer, in one place: a producer dropped here leaves its files
/// unwritten, which fails the check as a removal.
async fn produce() -> Samples {
    let mut samples = Samples::default();
    crate::router::contract_samples(&mut samples);
    events::samples(&mut samples);
    crate::routes::contract_samples(&mut samples).await;
    crate::routes::request_samples::contract_samples(&mut samples);
    samples
}

#[tokio::test]
async fn the_samples_are_the_folder() {
    let samples = produce().await;
    let dir = Path::new(DIR);
    let folder = Folder::read(dir);
    let write = std::env::var_os(WRITE_ENV).is_some_and(|v| v == "1");
    let verdict = judge(
        &samples.items,
        &folder,
        openalpaca_app_support::protocol::PROTOCOL,
        write,
    );
    assert!(
        verdict.problems.is_empty(),
        "the API contract's samples ({}):\n  {}",
        dir.display(),
        verdict.problems.join("\n  ")
    );
    if write {
        apply(dir, &samples.items, &verdict);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn one(kind: Kind, name: &str, value: Value) -> BTreeMap<(Kind, String), Value> {
        BTreeMap::from([((kind, name.to_string()), value)])
    }

    fn folder(protocol: Option<u32>, samples: BTreeMap<(Kind, String), Value>) -> Folder {
        Folder { protocol, samples }
    }

    #[test]
    fn a_shape_keeps_keys_and_kinds_and_drops_values() {
        let body = json!({
            "id": "6f1c", "n": 7, "ok": true, "none": null,
            "rows": [{"a": 1}, {"b": "x"}, {"a": 2, "c": null}],
            "tags": ["x", "y"], "empty": [],
            "by_id": {"k1": {"v": 1}, "k2": {"w": 2}}
        });
        assert_eq!(
            shape(&body, "", &["by_id"], false),
            json!({
                "id": "", "n": 0, "ok": false, "none": null,
                "rows": [{"a": 0, "b": "", "c": null}],
                "tags": [""], "empty": [],
                "by_id": {"*": {"v": 0, "w": 0}}
            })
        );
        let event = json!({"type": "heartbeat", "ts": "2026-11-16T00:00:00Z"});
        assert_eq!(
            shape(&event, "", &[], true),
            json!({"type": "heartbeat", "ts": ""})
        );
    }

    #[test]
    fn key_paths_name_every_key_and_element() {
        let paths = key_paths(&json!([{"a": {"b": 0}, "c": [{"d": 0}], "m": {"*": 0}}]));
        let expected: BTreeSet<String> = [
            "[]", "[].a", "[].a.b", "[].c", "[].c[]", "[].c[].d", "[].m", "[].m.*",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(paths, expected);
    }

    #[test]
    fn a_matching_folder_passes() {
        let fresh = one(Kind::Response, "get-x", json!({"a": ""}));
        let verdict = judge(&fresh, &folder(Some(1), fresh.clone()), 1, false);
        assert_eq!(verdict, Verdict::default());
    }

    #[test]
    fn a_renamed_field_fails_without_a_bump_even_when_writing() {
        let fresh = one(Kind::Response, "get-x", json!({"b": ""}));
        let on_disk = folder(Some(1), one(Kind::Response, "get-x", json!({"a": ""})));
        for write in [false, true] {
            let verdict = judge(&fresh, &on_disk, 1, write);
            assert_eq!(verdict.problems.len(), 1, "{verdict:?}");
            assert!(
                verdict.problems[0].contains("responses/get-x: no longer carries a;")
                    && verdict.problems[0]
                        .contains("bumps openalpaca_app_support::protocol::PROTOCOL"),
                "{verdict:?}"
            );
            assert!(verdict.writes.is_empty() && verdict.record.is_none());
        }
    }

    #[test]
    fn a_renamed_field_passes_with_a_bump_and_records_it() {
        let fresh = one(Kind::Response, "get-x", json!({"b": ""}));
        let on_disk = folder(Some(1), one(Kind::Response, "get-x", json!({"a": ""})));
        let verdict = judge(&fresh, &on_disk, 2, true);
        assert!(verdict.problems.is_empty(), "{verdict:?}");
        assert_eq!(verdict.writes, vec![(Kind::Response, "get-x".to_string())]);
        assert_eq!(verdict.record, Some(2));
        // Without the rewrite, the moved PROTOCOL is itself a stale folder.
        let unwritten = judge(&fresh, &on_disk, 2, false);
        assert!(!unwritten.problems.is_empty());
    }

    #[test]
    fn an_added_field_needs_a_rewrite_and_no_bump() {
        let fresh = one(Kind::Response, "get-x", json!({"a": "", "b": 0}));
        let on_disk = folder(Some(1), one(Kind::Response, "get-x", json!({"a": ""})));
        let stale = judge(&fresh, &on_disk, 1, false);
        assert_eq!(stale.problems.len(), 1);
        assert!(
            stale.problems[0].contains("differs from the folder"),
            "{stale:?}"
        );
        let written = judge(&fresh, &on_disk, 1, true);
        assert!(written.problems.is_empty(), "{written:?}");
        assert_eq!(written.writes.len(), 1);
        assert_eq!(written.record, None);
    }

    #[test]
    fn a_sample_no_test_writes_is_a_removal() {
        let on_disk = folder(Some(1), one(Kind::Event, "gone", json!({"type": "gone"})));
        let fresh = BTreeMap::new();
        let verdict = judge(&fresh, &on_disk, 1, true);
        assert_eq!(verdict.problems.len(), 1);
        assert!(
            verdict.problems[0].contains("events/gone: no test writes it any more")
                && verdict.problems[0].contains("bumps"),
            "{verdict:?}"
        );
        let bumped = judge(&fresh, &on_disk, 2, true);
        assert!(bumped.problems.is_empty(), "{bumped:?}");
        assert_eq!(bumped.removes, vec![(Kind::Event, "gone".to_string())]);
    }

    #[test]
    fn a_new_sample_is_an_addition() {
        let fresh = one(Kind::Response, "get-new", json!({"a": ""}));
        let on_disk = folder(Some(1), BTreeMap::new());
        assert_eq!(judge(&fresh, &on_disk, 1, false).problems.len(), 1);
        let written = judge(&fresh, &on_disk, 1, true);
        assert!(written.problems.is_empty());
        assert_eq!(written.writes.len(), 1);
    }

    #[test]
    fn samples_without_a_recorded_protocol_are_refused() {
        let fresh = one(Kind::Response, "get-x", json!({"b": ""}));
        let on_disk = folder(None, one(Kind::Response, "get-x", json!({"a": ""})));
        let verdict = judge(&fresh, &on_disk, 1, true);
        assert_eq!(verdict.problems.len(), 1, "{verdict:?}");
        assert!(verdict.problems[0].contains("protocol.json is missing"));
        assert!(verdict.writes.is_empty() && verdict.record.is_none());
    }

    #[test]
    fn protocol_never_moves_back() {
        let fresh = one(Kind::Response, "get-x", json!({"a": ""}));
        let verdict = judge(&fresh, &folder(Some(3), fresh.clone()), 2, true);
        assert_eq!(verdict.problems.len(), 1);
        assert!(verdict.problems[0].contains("never moves back"));
    }

    /// CLI-PROTOCOL: a request body's field and a field of a response only
    /// the CLI reads, renamed, fail as a response's field does, even when
    /// writing, until `PROTOCOL` moves.
    #[test]
    fn a_renamed_request_or_cli_read_field_fails_without_a_bump() {
        for kind in [Kind::Request, Kind::CliResponse] {
            let fresh = one(kind, "post-x", json!({"b": null}));
            let on_disk = folder(Some(1), one(kind, "post-x", json!({"a": null})));
            for write in [false, true] {
                let verdict = judge(&fresh, &on_disk, 1, write);
                assert_eq!(verdict.problems.len(), 1, "{verdict:?}");
                assert!(
                    verdict.problems[0]
                        .contains(&format!("{}/post-x: no longer carries a;", kind.folder()))
                        && verdict.problems[0]
                            .contains("bumps openalpaca_app_support::protocol::PROTOCOL"),
                    "{verdict:?}"
                );
            }
            assert!(judge(&fresh, &on_disk, 2, true).problems.is_empty());
        }
    }

    /// Every kind is read from its own folder: a kind the read skipped
    /// would hide its removals.
    #[test]
    fn every_kind_is_read_from_its_folder() {
        let tmp = tempfile::tempdir().expect("a temp folder");
        for kind in Kind::ALL {
            let dir = tmp.path().join(kind.folder());
            std::fs::create_dir_all(&dir).expect("create the folder");
            std::fs::write(dir.join("x.json"), b"{\"a\": null}\n").expect("a sample");
        }
        let read = Folder::read(tmp.path());
        let kinds: Vec<Kind> = read.samples.keys().map(|(kind, _)| *kind).collect();
        assert_eq!(kinds, Kind::ALL.to_vec());
    }

    /// A hidden file a desktop leaves in the folder (Finder's `.DS_Store`)
    /// is not a sample and is passed over.
    #[test]
    fn a_hidden_file_in_the_folder_is_passed_over() {
        let tmp = tempfile::tempdir().expect("a temp folder");
        let responses = tmp.path().join("responses");
        std::fs::create_dir_all(&responses).expect("create the folder");
        std::fs::write(responses.join(".DS_Store"), b"\0\0\0\x01Bud1").expect("plant it");
        std::fs::write(responses.join("get-x.json"), b"{\"a\": \"\"}\n").expect("a sample");
        let folder = Folder::read(tmp.path());
        assert_eq!(
            folder.samples.keys().cloned().collect::<Vec<_>>(),
            vec![(Kind::Response, "get-x".to_string())]
        );
    }

    /// A visible file that is not a `.json` sample is still refused.
    #[test]
    #[should_panic(expected = "is not a .json sample")]
    fn a_visible_file_not_a_sample_is_refused() {
        let tmp = tempfile::tempdir().expect("a temp folder");
        let events = tmp.path().join("events");
        std::fs::create_dir_all(&events).expect("create the folder");
        std::fs::write(events.join("notes.txt"), b"x").expect("plant it");
        let _ = Folder::read(tmp.path());
    }

    /// The folder on disk is read the way the check reads it, and a rewrite
    /// lands where the next read finds it.
    #[test]
    fn a_rewrite_round_trips_through_the_disk() {
        let tmp = tempfile::tempdir().expect("a temp folder");
        let fresh = one(
            Kind::Event,
            "heartbeat",
            json!({"type": "heartbeat", "ts": ""}),
        );
        let verdict = judge(&fresh, &Folder::read(tmp.path()), 1, true);
        assert!(verdict.problems.is_empty(), "{verdict:?}");
        apply(tmp.path(), &fresh, &verdict);
        let again = Folder::read(tmp.path());
        assert_eq!(again.protocol, Some(1));
        assert_eq!(judge(&fresh, &again, 1, false), Verdict::default());
    }
}
