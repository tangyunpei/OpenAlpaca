//! Each route-named sample held to the router's registered method and path
//! table (CONTRACT-SAMPLES-2; *Rev 107*'s route-path item, (b)).
//!
//! A sample's name is its producer's literal, so a path renamed or removed
//! in `router.rs` alone would move no sample. The table is read from
//! `router.rs` itself — every `.route("<path>", <method>(…).<method>(…))`
//! the file registers (axum's `Router` does not list its routes, and no
//! unit test can build the daemon's `AppState`) — and a sample of a kind
//! named after a route must be the name of one of its (method, path) pairs:
//! the method, then the path after `/v1/` with each `/` a `-` and each
//! `{param}` the last `_`-separated word of its name (`{lane_key}` is
//! `key`, `{message_id}` is `id`). A sample that names one outcome or one
//! value of a route, which that rule cannot spell, is listed in
//! [`VARIANTS`] with the route it belongs to, held to the table the same
//! way. The parse counts the file's `.route(` calls, so a form it cannot
//! read fails rather than shrinks the table.

use std::collections::{BTreeMap, BTreeSet};

/// The router's source, as the check reads it.
const ROUTER: &str = include_str!("../router.rs");

/// The methods `router.rs` registers routes with.
const METHODS: [&str; 5] = ["get", "post", "put", "patch", "delete"];

/// The samples named after one outcome or one value of a route, and the
/// (method, path) each belongs to.
pub(super) const VARIANTS: &[(&str, &str, &str)] = &[
    // `POST /v1/tasks/{id}/action`'s three answers, by its `action`.
    (
        "post-tasks-id-action-cancel",
        "post",
        "/v1/tasks/{id}/action",
    ),
    (
        "post-tasks-id-action-resume",
        "post",
        "/v1/tasks/{id}/action",
    ),
    (
        "post-tasks-id-action-start",
        "post",
        "/v1/tasks/{id}/action",
    ),
    // `POST /v1/memories/{id}`'s two answers, by its `action`.
    ("post-memories-id-confirm", "post", "/v1/memories/{id}"),
    ("post-memories-id-edit", "post", "/v1/memories/{id}"),
    // `GET /v1/workspaces?path=`, the one project's answer.
    ("get-workspaces-path", "get", "/v1/workspaces"),
    // `{kind}` as the wrappers send it.
    ("post-extensions-mcp", "post", "/v1/extensions/{kind}"),
    ("post-extensions-plugin", "post", "/v1/extensions/{kind}"),
    (
        "put-extensions-plugin-id",
        "put",
        "/v1/extensions/{kind}/{id}",
    ),
];

/// Every (method, path) `router.rs` registers.
pub(super) fn table(source: &str) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    let calls: Vec<&str> = source.split(".route(").skip(1).collect();
    for call in &calls {
        let call = call.trim_start();
        let path = call
            .strip_prefix('"')
            .and_then(|rest| rest.split_once('"'))
            .map(|(path, _)| path)
            .unwrap_or_else(|| panic!("a .route( call without a path literal: {call:.60}"));
        let args = registration(call);
        let methods: Vec<&str> = METHODS
            .into_iter()
            .filter(|m| calls_method(args, m))
            .collect();
        assert!(
            !methods.is_empty(),
            "the route {path:?} names none of {METHODS:?}"
        );
        for method in methods {
            assert!(
                out.insert((method.to_string(), path.to_string())),
                "{method} {path} registered twice"
            );
        }
    }
    assert!(!out.is_empty(), "router.rs registers no route");
    out
}

/// The text of one `.route(` call's arguments, up to its closing `)`.
fn registration(call: &str) -> &str {
    let mut depth = 1usize;
    let mut in_string = false;
    for (i, c) in call.char_indices() {
        match c {
            '"' => in_string = !in_string,
            '(' if !in_string => depth += 1,
            ')' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return &call[..i];
                }
            }
            _ => {}
        }
    }
    panic!("a .route( call that does not close: {call:.60}")
}

/// Whether `args` registers `method`: `get(` at its start or `.get(` chained.
fn calls_method(args: &str, method: &str) -> bool {
    let after_path = args.split_once(',').map(|(_, rest)| rest).unwrap_or("");
    let head = after_path.trim_start();
    head.starts_with(&format!("{method}(")) || after_path.contains(&format!(".{method}("))
}

/// The name a route's samples take: `delete-lanes-key-followups-id`.
pub(super) fn sample_name(method: &str, path: &str) -> String {
    let rest = path
        .strip_prefix("/v1/")
        .unwrap_or(path.trim_start_matches('/'));
    let segments: Vec<&str> = rest
        .split('/')
        .map(|segment| match segment.strip_prefix('{') {
            Some(param) => {
                let param = param.trim_end_matches('}');
                param.rsplit('_').next().unwrap_or(param)
            }
            None => segment,
        })
        .collect();
    format!("{method}-{}", segments.join("-"))
}

/// What fails: each name that is no registered route's, and each variant
/// whose route is not registered or that no sample uses any more.
pub(super) fn problems(
    names: &BTreeSet<String>,
    table: &BTreeSet<(String, String)>,
) -> Vec<String> {
    let by_name: BTreeMap<String, &(String, String)> = table
        .iter()
        .map(|route| (sample_name(&route.0, &route.1), route))
        .collect();
    let mut out = Vec::new();
    for (variant, method, path) in VARIANTS {
        if !table.contains(&(method.to_string(), path.to_string())) {
            out.push(format!(
                "the variant {variant} belongs to {} {path}, which router.rs no longer registers",
                method.to_uppercase()
            ));
        }
        if !names.contains(*variant) {
            out.push(format!(
                "the variant {variant} is listed, and no sample is named so: drop it from VARIANTS"
            ));
        }
    }
    for name in names {
        if by_name.contains_key(name) || VARIANTS.iter().any(|(v, _, _)| v == name) {
            continue;
        }
        out.push(format!(
            "{name}: names no route router.rs registers; a route renamed or removed \
             bumps openalpaca_app_support::protocol::PROTOCOL (CLAUDE.md, Storage), \
             and its samples follow"
        ));
    }
    out
}

/// `build_router`'s own text: the file's tests register routes of their
/// own, which are not the daemon's.
fn build_router_source() -> &'static str {
    let start = ROUTER
        .find("pub fn build_router(")
        .expect("router.rs defines build_router");
    let rest = &ROUTER[start..];
    let end = rest
        .find("\n}\n")
        .expect("build_router's body closes at the start of a line");
    &rest[..end]
}

/// The problems of this build's router and these sample names.
pub(super) fn check(names: &BTreeSet<String>) -> Vec<String> {
    problems(names, &table(build_router_source()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_reads_every_route_call_and_chained_methods() {
        let source = r#"
            Router::new()
                .route("/v1/a/{lane_key}/b", get(x::h))
                .route(
                    "/v1/memories/{id}",
                    get(crate::routes::g).post(crate::routes::p)
                        .delete(crate::routes::d),
                )
                .route("/v1/files/upload", post(u).route_layer(DefaultBodyLimit::max(1)))
        "#;
        let routes: Vec<(String, String)> = table(source).into_iter().collect();
        let expected: Vec<(String, String)> = [
            ("delete", "/v1/memories/{id}"),
            ("get", "/v1/a/{lane_key}/b"),
            ("get", "/v1/memories/{id}"),
            ("post", "/v1/files/upload"),
            ("post", "/v1/memories/{id}"),
        ]
        .into_iter()
        .map(|(m, p)| (m.to_string(), p.to_string()))
        .collect();
        assert_eq!(routes, expected);
    }

    #[test]
    #[should_panic(expected = "names none of")]
    fn a_route_form_the_parse_cannot_read_fails() {
        let _ = table(r#".route("/v1/x", any(h))"#);
    }

    #[test]
    fn the_real_router_parses_and_counts_every_call() {
        let source = build_router_source();
        let routes = table(source);
        assert!(routes.contains(&("get".to_string(), "/v1/health".to_string())));
        assert!(routes.contains(&("delete".to_string(), "/v1/memories/{id}".to_string())));
        // Each `.route(` call registers at least one pair.
        assert!(routes.len() >= source.matches(".route(").count());
    }

    #[test]
    fn a_route_names_its_samples_by_method_and_path() {
        assert_eq!(
            sample_name("delete", "/v1/lanes/{lane_key}/followups/{id}"),
            "delete-lanes-key-followups-id"
        );
        assert_eq!(
            sample_name("put", "/v1/chat/messages/{message_id}/feedback"),
            "put-chat-messages-id-feedback"
        );
        assert_eq!(sample_name("get", "/v1/health"), "get-health");
    }

    #[test]
    fn a_sample_whose_route_was_renamed_fails_and_a_listed_variant_passes() {
        let table: BTreeSet<(String, String)> =
            [("get", "/v1/tasks"), ("post", "/v1/tasks/{id}/action")]
                .into_iter()
                .map(|(m, p)| (m.to_string(), p.to_string()))
                .collect();
        let names: BTreeSet<String> = ["get-tasks", "get-runs", "post-tasks-id-action-cancel"]
            .into_iter()
            .map(String::from)
            .collect();
        let found = problems(&names, &table);
        assert!(
            found
                .iter()
                .any(|p| p.starts_with("get-runs: names no route")),
            "{found:?}"
        );
        assert!(
            !found
                .iter()
                .any(|p| p.starts_with("get-tasks:")
                    || p.starts_with("post-tasks-id-action-cancel:")),
            "{found:?}"
        );
    }
}
