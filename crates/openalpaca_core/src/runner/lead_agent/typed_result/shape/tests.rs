//! SA-4's checker, unit by unit (design §5.3.7, the owner's unit layer):
//! every verdict here is written by hand; nothing is compared against
//! another validator. The tests that feed a shape at or near a cap print
//! their own `elapsed_ms` (the build's one-second rule) and run their check
//! on a thread of their own that must finish within a minute.

use super::*;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn admit(schema: &Value) -> ResultShape {
    ResultShape::admit(schema).unwrap_or_else(|e| panic!("{schema} is admitted: {e}"))
}

fn refusal(schema: &Value) -> String {
    match ResultShape::admit(schema) {
        Ok(_) => panic!("{schema} is refused"),
        Err(why) => why.to_string(),
    }
}

/// The lines a failing value gets.
fn lines(shape: &ResultShape, value: &Value) -> Vec<String> {
    match shape.check(value) {
        ShapeVerdict::Violations { lines, more } => {
            assert_eq!(more, 0, "{value}: {lines:?} and {more} more");
            lines
        }
        other => panic!("{value} fails, not {other:?}"),
    }
}

fn passes(shape: &ResultShape, value: &Value) -> bool {
    matches!(shape.check(value), ShapeVerdict::Matches { .. })
}

/// `work` on a thread of its own, which must finish within a minute (a
/// thread cannot be killed: the test fails instead of waiting); its time
/// printed as `elapsed_ms=<n> <name>`.
fn within_a_minute<T: Send + 'static>(name: &str, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    let out = rx
        .recv_timeout(Duration::from_secs(60))
        .unwrap_or_else(|_| panic!("{name}: the check did not finish within 60 s"));
    println!("elapsed_ms={} {name}", started.elapsed().as_millis());
    out
}

/// A string of `n` ASCII `x`s.
fn xs(n: usize) -> String {
    "x".repeat(n)
}

// ── Per keyword ──────────────────────────────────────────────────────

#[test]
fn type_is_checked_and_its_wrong_shapes_refused() {
    let one = admit(&json!({"type": "string"}));
    assert!(passes(&one, &json!("a")));
    assert_eq!(
        lines(&one, &json!(1)),
        ["(root): expected string, got integer"]
    );

    // An array, a duplicate folded.
    let two = admit(&json!({"type": ["string", "null", "string"]}));
    assert!(passes(&two, &json!(null)));
    assert!(passes(&two, &json!("a")));
    assert_eq!(
        lines(&two, &json!(true)),
        ["(root): expected string or null, got boolean"]
    );

    for wrong in [
        json!({"type": "str"}),
        json!({"type": []}),
        json!({"type": [1]}),
        json!({"type": 3}),
    ] {
        assert_eq!(
            refusal(&wrong),
            "\"type\" at (root) must be a type name or a non-empty array of type names",
            "{wrong}"
        );
    }
}

#[test]
fn integer_follows_json_schema() {
    let integer = admit(&json!({"type": "integer"}));
    for text in ["1", "1.0", "1e2", "-0", "1e-400"] {
        let value: Value = serde_json::from_str(text).unwrap();
        assert!(passes(&integer, &value), "{text} is an integer (L4)");
    }
    let value: Value = serde_json::from_str("1.5").unwrap();
    assert_eq!(
        lines(&integer, &value),
        ["(root): expected integer, got number"]
    );
    // A number names both kinds.
    let number = admit(&json!({"type": "number"}));
    assert!(passes(&number, &json!(1)));
    assert!(passes(&number, &json!(1.5)));
}

#[test]
fn enum_is_checked_with_json_schema_equality() {
    let one = admit(&json!({"enum": [1]}));
    assert!(passes(&one, &serde_json::from_str("1.0").unwrap()));
    let zero = admit(&json!({"enum": [0]}));
    assert!(passes(&zero, &serde_json::from_str("-0").unwrap()));
    let mixed = admit(&json!({"enum": ["a", {"b": [1]}]}));
    assert!(passes(
        &mixed,
        &serde_json::from_str(r#"{"b":[1.0]}"#).unwrap()
    ));
    assert_eq!(
        lines(&mixed, &json!("c")),
        ["(root): is not one of the 2 values result_schema allows (enum)"]
    );
    for wrong in [json!({"enum": []}), json!({"enum": "a"})] {
        assert_eq!(
            refusal(&wrong),
            "\"enum\" at (root) must be a non-empty array",
            "{wrong}"
        );
    }
}

#[test]
fn const_is_checked() {
    let shape = admit(&json!({"const": {"a": [1, 2]}}));
    assert!(passes(
        &shape,
        &serde_json::from_str(r#"{"a":[1,2.0]}"#).unwrap()
    ));
    assert_eq!(
        lines(&shape, &json!({"a": [1]})),
        ["(root): is not the value result_schema requires (const)"]
    );
    assert_eq!(
        lines(&shape, &json!({"a": [1, 2], "b": 1})),
        ["(root): is not the value result_schema requires (const)"]
    );
    let null = admit(&json!({"const": null}));
    assert!(passes(&null, &json!(null)));
    assert!(!passes(&null, &json!(0)));
}

#[test]
fn any_of_passes_on_one_branch_and_fails_in_one_line() {
    let shape = admit(&json!({"anyOf": [{"type": "string"}, {"type": "integer", "minimum": 5}]}));
    assert!(passes(&shape, &json!("x")));
    assert!(passes(&shape, &json!(7)));
    assert_eq!(
        lines(&shape, &json!(3)),
        ["(root): matches none of the 2 shapes result_schema allows (anyOf)"]
    );
    // Every keyword at a node must hold: `type` and `anyOf` together.
    let both = admit(&json!({"type": "integer", "anyOf": [{"minimum": 5}, {"maximum": -5}]}));
    assert_eq!(
        lines(&both, &json!("x")),
        ["(root): expected integer, got string"]
    );
    assert_eq!(
        lines(&both, &json!(0)),
        ["(root): matches none of the 2 shapes result_schema allows (anyOf)"]
    );
    assert!(passes(&both, &json!(9)));
    assert_eq!(
        refusal(&json!({"anyOf": []})),
        "\"anyOf\" at (root) must be a non-empty array of schemas"
    );
    assert_eq!(
        refusal(&json!({"anyOf": [{}, true]})),
        "\"anyOf\" at /anyOf/1 must be a schema object, not true or false"
    );
}

#[test]
fn properties_are_checked_member_by_member() {
    let shape = admit(&json!({"properties": {"a": {"type": "string"}, "b": {"type": "integer"}}}));
    assert_eq!(
        lines(&shape, &json!({"a": 1, "b": "x", "c": true})),
        [
            "/a: expected string, got integer",
            "/b: expected integer, got string"
        ]
    );
    assert!(passes(&shape, &json!({})));
    assert!(
        passes(&shape, &json!("not an object")),
        "a typed keyword on another kind asserts nothing"
    );
    assert_eq!(
        refusal(&json!({"properties": []})),
        "\"properties\" at (root) must be an object of schemas"
    );
    assert_eq!(
        refusal(&json!({"properties": {"a": 5}})),
        "\"properties\" at /properties/a must be a schema object"
    );
}

#[test]
fn required_names_are_checked() {
    let shape = admit(&json!({"required": ["a", "b"]}));
    assert_eq!(
        lines(&shape, &json!({"a": 1})),
        ["(root): \"b\" is a required property"]
    );
    assert!(passes(&shape, &json!({"a": 1, "b": null})));
    assert!(
        passes(&shape, &json!([1])),
        "on another kind it asserts nothing"
    );
    assert!(ResultShape::admit(&json!({"required": []})).is_ok());
    for wrong in [json!({"required": [1]}), json!({"required": "a"})] {
        assert_eq!(
            refusal(&wrong),
            "\"required\" at (root) must be an array of strings",
            "{wrong}"
        );
    }
}

#[test]
fn additional_properties_false_true_and_absent() {
    let closed = admit(&json!({"properties": {"a": {}}, "additionalProperties": false}));
    assert_eq!(
        lines(&closed, &json!({"a": 1, "b": 2})),
        ["/b: is not allowed (additionalProperties is false)"]
    );
    let open = admit(&json!({"properties": {"a": {}}, "additionalProperties": true}));
    assert!(passes(&open, &json!({"a": 1, "b": 2})));
    let absent = admit(&json!({"properties": {"a": {}}}));
    assert!(passes(&absent, &json!({"a": 1, "b": 2})));
    assert_eq!(
        refusal(&json!({"additionalProperties": {}})),
        "\"additionalProperties\" at (root) must be true or false"
    );
}

#[test]
fn items_is_one_schema_and_the_array_form_is_refused() {
    let shape = admit(&json!({"items": {"type": "integer"}}));
    assert_eq!(
        lines(&shape, &json!([1, "x", 2])),
        ["/1: expected integer, got string"]
    );
    // The proposal's own example.
    assert_eq!(
        refusal(&json!({"properties": {"list": {"items": [{}]}}})),
        "\"items\" at /properties/list must be one schema, not an array"
    );
    assert_eq!(
        refusal(&json!({"items": true})),
        "\"items\" at /items must be a schema object, not true or false"
    );
}

#[test]
fn min_items_and_max_items_are_checked() {
    let shape = admit(&json!({"minItems": 2, "maxItems": 3}));
    assert_eq!(
        lines(&shape, &json!([1])),
        ["(root): has 1 items; at least 2"]
    );
    assert_eq!(
        lines(&shape, &json!([1, 2, 3, 4])),
        ["(root): has 4 items; at most 3"]
    );
    assert!(passes(&shape, &json!([1, 2])));
    // L4, the owner's: `3.0` is the count 3, `-0` and `1e-400` the count 0.
    let three: Value = serde_json::from_str(r#"{"minItems": 3.0}"#).unwrap();
    let three = admit(&three);
    assert!(passes(&three, &json!([1, 2, 3])));
    assert_eq!(
        lines(&three, &json!([1, 2])),
        ["(root): has 2 items; at least 3.0"]
    );
    for text in [r#"{"maxItems": -0}"#, r#"{"maxItems": 1e-400}"#] {
        let zero = admit(&serde_json::from_str(text).unwrap());
        assert!(passes(&zero, &json!([])), "{text}");
        assert!(!passes(&zero, &json!([1])), "{text}");
    }
    for wrong in [
        r#"{"minItems": 1.5}"#,
        r#"{"minItems": -1}"#,
        r#"{"maxItems": "2"}"#,
    ] {
        let wrong: Value = serde_json::from_str(wrong).unwrap();
        let keyword = wrong.as_object().unwrap().keys().next().unwrap().clone();
        assert_eq!(
            refusal(&wrong),
            format!("\"{keyword}\" at (root) must be a non-negative integer")
        );
    }
}

#[test]
fn number_bounds_compare_exactly() {
    // 2^53 + 1 against the float 2^53: not equal, above it.
    let max: Value = serde_json::from_str(r#"{"maximum": 9007199254740992.0}"#).unwrap();
    let max = admit(&max);
    assert_eq!(
        lines(&max, &json!(9_007_199_254_740_993_u64)),
        ["(root): is above the maximum 9007199254740992.0"]
    );
    assert!(passes(&max, &json!(9_007_199_254_740_992_u64)));
    // The f64 2^64 is above u64::MAX.
    let two_64 = Number::from_f64(18_446_744_073_709_551_616.0).unwrap();
    let min = admit(&json!({"minimum": two_64.clone()}));
    assert_eq!(
        lines(&min, &json!(u64::MAX)),
        [format!("(root): is below the minimum {two_64}")]
    );
    // -0 is not above 0.
    let exclusive = admit(&json!({"exclusiveMinimum": 0}));
    assert_eq!(
        lines(&exclusive, &serde_json::from_str("-0").unwrap()),
        ["(root): is not above 0 (exclusiveMinimum)"]
    );
    assert!(passes(&exclusive, &json!(0.5)));
    let below = admit(&json!({"exclusiveMaximum": 1}));
    assert_eq!(
        lines(&below, &serde_json::from_str("1.0").unwrap()),
        ["(root): is not below 1 (exclusiveMaximum)"]
    );
    assert!(
        passes(&below, &json!("text")),
        "a bound asserts nothing on a string"
    );
    assert_eq!(
        refusal(&json!({"exclusiveMinimum": true})),
        "\"exclusiveMinimum\" at (root) must be a number"
    );
}

#[test]
fn format_is_not_asserted() {
    let shape = admit(&json!({"type": "string", "format": "email"}));
    assert!(passes(&shape, &json!("not an email address")));
    assert_eq!(
        refusal(&json!({"format": 5})),
        "\"format\" at (root) must be a string"
    );
}

#[test]
fn each_annotation_is_admitted_and_asserts_nothing() {
    let shape = admit(&json!({
        "type": "string",
        "title": "t",
        "description": "d",
        "$comment": "c",
        "default": {"anything": [1]},
        "examples": [1, "x"],
        "deprecated": true,
        "readOnly": false,
        "writeOnly": true
    }));
    assert!(passes(&shape, &json!("x")));
    assert_eq!(
        refusal(&json!({"title": 1, "examples": {}, "deprecated": "yes"})),
        "\"deprecated\" at (root) must be true or false; \"examples\" at (root) must be an \
         array; \"title\" at (root) must be a string"
    );
}

#[test]
fn the_boolean_schemas_are_refused() {
    assert_eq!(refusal(&json!(true)), "it is a boolean, not a JSON object");
    assert_eq!(refusal(&json!("{}")), "it is a string, not a JSON object");
    assert_eq!(refusal(&json!([])), "it is an array, not a JSON object");
    assert_eq!(refusal(&json!(1)), "it is a number, not a JSON object");
    assert_eq!(
        refusal(&json!({"properties": {"a": true}})),
        "\"properties\" at /properties/a must be a schema object, not true or false"
    );
    assert_eq!(
        refusal(&json!({"items": false})),
        "\"items\" at /items must be a schema object, not true or false"
    );
}

#[test]
fn a_braces_schema_below_the_root_admits_any_value() {
    let shape = admit(&json!({"properties": {"a": {}}, "required": ["a"]}));
    assert!(passes(&shape, &json!({"a": [1, {"x": null}]})));
    let items = admit(&json!({"items": {}}));
    assert!(passes(&items, &json!([1, "x", null, {}])));
}

// ── The refusals ─────────────────────────────────────────────────────

#[test]
fn every_off_list_keyword_is_refused_by_name() {
    let schema = json!({
        "allOf": [], "maxLength": 1, "minLength": 1, "multipleOf": 2, "not": {},
        "oneOf": [], "pattern": "x", "patternProperties": {}, "uniqueItems": true,
        "x-ext": 1
    });
    assert_eq!(
        refusal(&schema),
        "it uses what this checker does not support: allOf at (root), maxLength at (root), \
         minLength at (root), multipleOf at (root), not at (root), oneOf at (root), pattern at \
         (root), patternProperties at (root) (and 2 more); supported: type, enum, const, anyOf, \
         properties, required, additionalProperties (true or false), items (one schema), \
         minItems, maxItems, minimum, maximum, exclusiveMinimum, exclusiveMaximum; not \
         checked: format and the annotations; say a pattern in \"description\" instead"
    );
    // `$schema` is refused, not ignored; an off-list word deep in the tree
    // is named at its node, beside a wrong shape.
    assert_eq!(
        refusal(&json!({"$schema": "x", "properties": {"a": {"oneOf": [], "items": [1]}}})),
        format!(
            "it uses what this checker does not support: $schema at (root), oneOf at \
             /properties/a; {SUPPORTED}; \"items\" at /properties/a must be one schema, not an \
             array"
        )
    );
}

#[test]
fn every_reference_keyword_is_refused_by_name() {
    let tail = format!("; {SUPPORTED}; write each sub-shape out where it is used");
    for form in [
        "https://example.com/schema.json",
        "file:///etc/schema.json",
        "other.json",
        "#",
        "#/$defs/x",
    ] {
        assert_eq!(
            refusal(&json!({"$ref": form})),
            format!("it uses what this checker does not support: $ref at (root){tail}"),
            "{form}"
        );
    }
    for keyword in [
        "$defs",
        "definitions",
        "$dynamicRef",
        "$dynamicAnchor",
        "$recursiveRef",
        "$recursiveAnchor",
        "$anchor",
    ] {
        assert_eq!(
            refusal(&json!({keyword: {}})),
            format!("it uses what this checker does not support: {keyword} at (root){tail}"),
        );
    }
}

#[test]
fn an_id_is_refused() {
    assert_eq!(
        refusal(&json!({"$id": "https://example.com/s", "type": "object"})),
        format!(
            "it uses what this checker does not support: $id at (root); {SUPPORTED}; write \
             each sub-shape out where it is used"
        )
    );
}

#[test]
fn a_refused_name_is_escaped() {
    assert_eq!(
        refusal(&json!({"pa\u{202e}t\u{200b}tern": 1})),
        format!(
            "it uses what this checker does not support: pa<U+202E>t<U+200B>tern at (root); \
             {SUPPORTED}"
        )
    );
    assert_eq!(
        refusal(&json!({"properties": {"x\u{202e}y/z": {"pattern": "a"}}})),
        format!(
            "it uses what this checker does not support: pattern at \
             /properties/x<U+202E>y~1z; {SUPPORTED}; say a pattern in \"description\" instead"
        )
    );
}

#[test]
fn the_refusal_reason_stays_within_its_budget_with_the_list_whole() {
    // Ten off-list keywords with 300-byte names, each under a 300-byte
    // property name: a name and a path are each cut at 64 bytes, marked,
    // so every named problem is 138 bytes and the 1,024-byte budget holds
    // seven of the eight named; the eighth and the two past it are counted.
    let long = |c: char| c.to_string().repeat(300);
    let mut properties = Map::new();
    for c in ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j'] {
        properties.insert(long(c), json!({ long(c.to_ascii_uppercase()): 1 }));
    }
    let reason = refusal(&json!({ "properties": properties }));
    let opening = "it uses what this checker does not support: ";
    assert!(reason.starts_with(opening), "{reason}");
    assert!(reason.ends_with(&format!("; {SUPPORTED}")), "{reason}");
    let named = &reason[opening.len()..reason.find(" (and ").expect("the rest counted")];
    assert!(named.len() <= 1_024, "{} bytes named", named.len());
    assert_eq!(named.split(", ").count(), 7, "{named}");
    assert!(reason.contains(" (and 3 more); "), "{reason}");
    // Every name and every path is cut at 64 bytes, marked.
    assert!(
        named.starts_with(&format!(
            "{}… at /properties/{}…, ",
            "A".repeat(64),
            "a".repeat(52)
        )),
        "{named}"
    );
}

#[test]
fn a_first_problem_longer_than_the_budget_is_cut_to_fit() {
    // Thirty 60-byte names deep (no `type`, so no object level), then one
    // wrongly shaped keyword: its path is over 2,000 bytes before the cut.
    let name = "a".repeat(60);
    let mut node = json!({"minItems": -1});
    for _ in 0..30 {
        node = json!({ "properties": { name.clone(): node } });
    }
    let reason = refusal(&node);
    assert!(reason.len() <= 1_024, "{} bytes: {reason}", reason.len());
    // The whole path is cut at 64 bytes of escaped text, marked.
    assert_eq!(
        reason,
        format!(
            "\"minItems\" at /properties/{}… must be a non-negative integer",
            "a".repeat(52)
        )
    );
}

#[test]
fn the_named_problems_are_the_first_in_walk_order() {
    // Six off-list keywords two long names deep, then one under a short
    // name that sorts last: the problems named are the first ones the walk
    // met, the rest counted — never a later one kept past an earlier drop.
    let long = |c: char| c.to_string().repeat(300);
    let mut properties = Map::new();
    for c in ['a', 'b', 'c', 'd', 'e', 'f'] {
        properties.insert(long(c), json!({"properties": {long(c): {"minLength": 1}}}));
    }
    properties.insert("z".to_string(), json!({"minLength": 1}));
    let reason = refusal(&json!({ "properties": properties }));
    let opening = "it uses what this checker does not support: ";
    assert!(reason.starts_with(opening), "{reason}");
    let (list, more) = match reason.find(" (and ") {
        Some(at) => {
            let count = &reason[at + " (and ".len()..];
            let count: usize = count[..count.find(' ').unwrap()].parse().unwrap();
            (&reason[opening.len()..at], count)
        }
        None => (
            &reason[opening.len()..reason.find("; supported: ").unwrap()],
            0,
        ),
    };
    assert!(list.len() <= 1_024, "{} bytes named", list.len());
    let named: Vec<&str> = list.split(", ").collect();
    assert_eq!(named.len() + more, 7, "{reason}");
    for (shown, c) in named.iter().zip(['a', 'b', 'c', 'd', 'e', 'f', 'z']) {
        assert!(
            shown.starts_with(&format!("minLength at /properties/{c}")),
            "{shown} is not the walk's next problem: {reason}"
        );
    }
}

#[test]
fn asks_nothing_reads_only_the_roots_keys() {
    assert!(asks_nothing(&json!({})));
    assert!(asks_nothing(
        &json!({"title": "t", "description": "d", "examples": [1]})
    ));
    assert!(asks_nothing(&json!({"format": "email"})));
    assert!(!asks_nothing(&json!({"pattern": "x"})));
    assert!(!asks_nothing(&json!({"type": "string"})));
    assert!(!asks_nothing(&json!({"title": "t", "required": []})));
    assert!(!asks_nothing(&json!("{}")));
    assert!(!asks_nothing(&json!([])));
}

// ── At each cap and just over it ─────────────────────────────────────

#[test]
fn schema_bytes_16384_are_admitted_and_16385_refused() {
    // `{"description":""}` is 18 bytes.
    let at = json!({"description": xs(16_384 - 18)});
    assert_eq!(serde_json::to_vec(&at).unwrap().len(), 16_384);
    let over = json!({"description": xs(16_385 - 18)});
    let (admitted, refused) = within_a_minute(
        "schema_bytes_16384_are_admitted_and_16385_refused",
        move || {
            (
                ResultShape::admit(&at).is_ok(),
                ResultShape::admit(&over).err(),
            )
        },
    );
    assert!(admitted);
    assert_eq!(
        refused.map(|r| r.to_string()).as_deref(),
        Some("it is more than 16,384 bytes as compact JSON")
    );
}

#[test]
fn the_size_walk_counts_what_serde_json_writes() {
    let samples: Vec<Value> = vec![
        json!(null),
        json!(true),
        json!(false),
        json!(0),
        json!(-1),
        json!(1.5),
        json!(1e300),
        json!(u64::MAX),
        json!(i64::MIN),
        serde_json::from_str("-0").unwrap(),
        serde_json::from_str("1e-400").unwrap(),
        serde_json::from_str("1e2").unwrap(),
        json!(""),
        json!("quote \" backslash \\ newline \n tab \t cr \r bs \u{8} ff \u{c}"),
        json!("control \u{1} \u{1f} del \u{7f} é 日 🙂 \u{202e}"),
        json!([]),
        json!({}),
        json!([1, [2, [3, {}]], {"a": [], "b": {"c": "d"}}]),
        json!({"key \" with \\ escapes \n": [null, true, 1.25, "x"]}),
    ];
    for sample in samples {
        assert_eq!(
            json_size_within(&sample, usize::MAX, None),
            Ok(serde_json::to_vec(&sample).unwrap().len()),
            "{sample}"
        );
    }
}

/// `{"items": … {} }`, `objects` schema objects nested through `items`.
fn items_chain(objects: usize) -> Value {
    let mut node = json!({});
    for _ in 1..objects {
        node = json!({ "items": node });
    }
    node
}

/// An array nested `levels` deep around `0`.
fn nested_array(levels: usize) -> Value {
    let mut value = json!(0);
    for _ in 0..levels {
        value = json!([value]);
    }
    value
}

#[test]
fn nesting_64_is_admitted_and_65_refused() {
    let (at, over, constant, constant_over) =
        within_a_minute("nesting_64_is_admitted_and_65_refused", || {
            (
                ResultShape::admit(&items_chain(64)).map(|_| ()),
                ResultShape::admit(&items_chain(65)).err(),
                // The root object and 63 arrays.
                ResultShape::admit(&json!({"const": nested_array(63)})).map(|_| ()),
                ResultShape::admit(&json!({"const": nested_array(64)})).err(),
            )
        });
    assert_eq!(at, Ok(()));
    assert_eq!(
        over.map(|r| r.to_string()).as_deref(),
        Some("it nests more than 64 levels")
    );
    assert_eq!(constant, Ok(()));
    assert_eq!(
        constant_over.map(|r| r.to_string()).as_deref(),
        Some("it nests more than 64 levels")
    );
}

/// `objects` levels of `{"type": "object", "properties": {"a": …}}`.
fn object_tower(objects: usize) -> Value {
    let mut node = json!({"type": "object"});
    for _ in 1..objects {
        node = json!({"type": "object", "properties": {"a": node}});
    }
    node
}

#[test]
fn a_schema_eleven_objects_deep_is_refused() {
    assert!(ResultShape::admit(&object_tower(10)).is_ok());
    assert_eq!(
        refusal(&object_tower(11)),
        "it nests more than 10 levels of objects"
    );
    // A `type` array naming `object` counts as a level too.
    let mut node = json!({"type": ["object", "null"]});
    for _ in 1..11 {
        node = json!({"type": ["null", "object"], "items": node});
    }
    assert_eq!(refusal(&node), "it nests more than 10 levels of objects");
}

#[test]
fn a_result_of_65536_bytes_is_checked_and_65537_rejected() {
    let shape = Arc::new(admit(&json!({"type": "string"})));
    // A string's compact form is its text and two quotes.
    let at = json!(xs(65_536 - 2));
    let over = json!(xs(65_537 - 2));
    assert!(result_fits(&at));
    assert!(!result_fits(&over));
    let check = Arc::clone(&shape);
    let (at, over) = within_a_minute(
        "a_result_of_65536_bytes_is_checked_and_65537_rejected",
        move || (check.check(&at), check.check(&over)),
    );
    assert!(
        matches!(at, ShapeVerdict::Matches { parsed: None }),
        "{at:?}"
    );
    assert!(matches!(over, ShapeVerdict::TooLarge), "{over:?}");
}

#[test]
fn a_one_megabyte_string_is_stopped_early() {
    let shape = Arc::new(admit(&json!({"type": "string"})));
    let (walked, steps, verdict) =
        within_a_minute("a_one_megabyte_string_is_stopped_early", move || {
            let value = json!(xs(1_048_576));
            probe::reset();
            let walked = json_size_within(&value, RESULT_MAX_BYTES, None);
            let steps = probe::size_steps();
            (walked, steps, shape.check(&value))
        });
    assert_eq!(walked, Err(TooBig::Bytes));
    // The string itself, then its first 65,536 characters: the one that
    // takes the total past 65,536 (the opening quote is the first byte).
    assert_eq!(steps, 65_537);
    assert!(matches!(verdict, ShapeVerdict::TooLarge), "{verdict:?}");
}

/// A JSON text of arrays nested `levels` deep.
fn nested_text(levels: usize) -> String {
    format!("{}{}", "[".repeat(levels), "]".repeat(levels))
}

#[test]
fn serde_parses_127_levels_and_refuses_128() {
    assert!(serde_json::from_str::<Value>(&nested_text(127)).is_ok());
    assert!(serde_json::from_str::<Value>(&nested_text(128)).is_err());
}

#[test]
fn a_string_result_is_validated_raw_first_then_parsed_once() {
    // Raw first: `"42"` stays a string for a string schema.
    let string = admit(&json!({"type": "string"}));
    assert!(matches!(
        string.check(&json!("42")),
        ShapeVerdict::Matches { parsed: None }
    ));
    // Parsed when the raw value fails.
    let integer = admit(&json!({"type": "integer"}));
    match integer.check(&json!("42")) {
        ShapeVerdict::Matches { parsed } => assert_eq!(parsed, Some(json!(42))),
        other => panic!("{other:?}"),
    }
    let array = Arc::new(admit(&json!({"type": "array"})));
    let deep = Arc::clone(&array);
    let (at, over) = within_a_minute(
        "a_string_result_is_validated_raw_first_then_parsed_once",
        move || {
            (
                deep.check(&json!(nested_text(127))),
                deep.check(&json!(nested_text(128))),
            )
        },
    );
    assert!(
        matches!(at, ShapeVerdict::Matches { parsed: Some(_) }),
        "{at:?}"
    );
    // serde refuses 128 levels: the raw value's lines.
    match over {
        ShapeVerdict::Violations { lines, more } => {
            assert_eq!(lines, ["(root): expected array, got string"]);
            assert_eq!(more, 0);
        }
        other => panic!("{other:?}"),
    }
    // A parse longer than the cap (each `1e2` becomes `100.0`): the raw
    // value's lines, not "more than 65,536 bytes".
    let text = format!("[{}]", vec!["1e2"; 12_000].join(","));
    assert!(text.len() + 2 <= RESULT_MAX_BYTES);
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert!(serde_json::to_vec(&parsed).unwrap().len() > RESULT_MAX_BYTES);
    let bounded = Arc::new(admit(&json!({"type": "array", "maxItems": 1})));
    let verdict = within_a_minute(
        "a_string_result_is_validated_raw_first_then_parsed_once (over)",
        move || bounded.check(&json!(text)),
    );
    match verdict {
        ShapeVerdict::Violations { lines, more } => {
            assert_eq!(lines, ["(root): expected array, got string"]);
            assert_eq!(more, 0);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn nineteen_lines_then_the_count() {
    let shape = admit(&json!({"items": {"type": "string"}}));
    match shape.check(&json!(vec![1; 19])) {
        ShapeVerdict::Violations { lines, more } => {
            assert_eq!(lines.len(), 19);
            assert_eq!(more, 0);
            assert_eq!(lines[18], "/18: expected string, got integer");
        }
        other => panic!("{other:?}"),
    }
    let (lines, more) = match shape.check(&json!(vec![1; 20])) {
        ShapeVerdict::Violations { lines, more } => (lines, more),
        other => panic!("{other:?}"),
    };
    assert_eq!((lines.len(), more), (19, 1));
    // What the child reads: the header, 19 lines, the count.
    let answer = super::super::TypedResultSlot::default().reject(&lines, more, None);
    assert!(
        answer.starts_with("The result does not match result_schema:\n/0: expected"),
        "{answer}"
    );
    assert!(
        answer.ends_with("\n/18: expected string, got integer\n… and 1 more violations"),
        "{answer}"
    );
    assert_eq!(answer.lines().count(), 21);
}

// ── The text of a line ───────────────────────────────────────────────

#[test]
fn a_violation_line_is_escaped_cut_and_quotes_no_value() {
    let shape = admit(&json!({
        "properties": {"k\u{202e}\u{200b}": {"type": "integer"}},
        "required": ["r\u{202e}q\u{200b}"]
    }));
    let found = lines(&shape, &json!({"k\u{202e}\u{200b}": "SECRET-VALUE"}));
    assert_eq!(
        found,
        [
            "(root): \"r<U+202E>q<U+200B>\" is a required property",
            "/k<U+202E><U+200B>: expected integer, got string"
        ]
    );
    // A NUL in a key, and a `/`, which cannot forge a deeper path.
    let closed = admit(&json!({"additionalProperties": false}));
    assert_eq!(
        lines(&closed, &json!({"a\u{0}b": "SECRET-VALUE"})),
        ["/a<U+0000>b: is not allowed (additionalProperties is false)"]
    );
    assert_eq!(
        lines(&closed, &json!({"a/b": 1})),
        ["/a~1b: is not allowed (additionalProperties is false)"]
    );
    // A long key is cut at 64 bytes, marked.
    let long = xs(300);
    let shown = lines(&closed, &json!({ long.clone(): 1 }));
    assert_eq!(
        shown,
        [format!(
            "/{}…: is not allowed (additionalProperties is false)",
            xs(64)
        )]
    );
    // A whole line is cut at 200 characters: four 60-character keys deep.
    let key = |c: char| c.to_string().repeat(60);
    let mut schema = json!({"type": "integer"});
    let mut value = json!("SECRET-VALUE");
    for c in ['d', 'c', 'b', 'a'] {
        schema = json!({"properties": { key(c): schema }});
        value = json!({ key(c): value });
    }
    let line = lines(&admit(&schema), &value).remove(0);
    assert_eq!(line.chars().count(), 200, "{line}");
    assert!(
        line.starts_with(&format!("/{}/{}", key('a'), key('b'))),
        "{line}"
    );
    for line in found.iter().chain(std::iter::once(&line)) {
        assert!(!line.contains("SECRET"), "no value is quoted: {line}");
    }
}

// ── The node-step term (§5.3.4) ──────────────────────────────────────

/// The step bound of §5.3.4's rows for `schema` against a value of `n`
/// nodes, summed over the schema's nodes: each node's own call ≤ N;
/// `properties` or `additionalProperties` ≤ (P + 1)·N; `required`
/// ≤ 2·r·N; each `enum` member and a `const` ≤ N. Never a free constant.
pub(super) fn step_bound(schema: &Value, n: u64) -> u64 {
    let Value::Object(node) = schema else {
        return 0;
    };
    let mut bound = n;
    if node.contains_key("properties") || node.contains_key("additionalProperties") {
        let p = node
            .get("properties")
            .and_then(Value::as_object)
            .map_or(0, |p| p.len() as u64);
        bound += (p + 1) * n;
    }
    if let Some(Value::Array(names)) = node.get("required") {
        bound += 2 * names.len() as u64 * n;
    }
    if let Some(Value::Array(members)) = node.get("enum") {
        bound += members.len() as u64 * n;
    }
    if node.contains_key("const") {
        bound += n;
    }
    if let Some(Value::Object(properties)) = node.get("properties") {
        bound += properties.values().map(|s| step_bound(s, n)).sum::<u64>();
    }
    if let Some(items) = node.get("items") {
        bound += step_bound(items, n);
    }
    if let Some(Value::Array(branches)) = node.get("anyOf") {
        bound += branches.iter().map(|b| step_bound(b, n)).sum::<u64>();
    }
    bound
}

/// A value's nodes: itself, and every element and member below it.
pub(super) fn nodes(value: &Value) -> u64 {
    1 + match value {
        Value::Array(a) => a.iter().map(nodes).sum(),
        Value::Object(o) => o.values().map(nodes).sum(),
        _ => 0,
    }
}

/// The check's steps on `value`, on the thread that runs it, and whether
/// it matched.
fn counted(shape: &ResultShape, value: &Value) -> (u64, bool) {
    probe::reset();
    let verdict = shape.check(value);
    (
        probe::steps(),
        matches!(verdict, ShapeVerdict::Matches { .. }),
    )
}

#[test]
fn the_step_count_stays_within_its_formula() {
    let cases: Vec<(&str, Value, Value, bool)> = vec![
        // (i) A schema at full U (8,000 enum members, about 16 KiB)
        // against a one-node value.
        ("full U", json!({"enum": vec![0; 8_000]}), json!(1), false),
        // (ii) A small schema against a value at full N (32,000 elements,
        // about 64 KiB).
        (
            "full N",
            json!({"items": {"type": "integer"}}),
            json!(vec![1; 32_000]),
            true,
        ),
        // (iii) A middle product: 100 members against 10,000 elements,
        // about 10^6 steps.
        (
            "middle",
            json!({"items": {"enum": vec![0; 100]}}),
            json!(vec![1; 10_000]),
            false,
        ),
    ];
    for (name, schema, value, matches) in cases {
        assert!(ResultShape::admit(&schema).is_ok(), "{name}");
        let bound = step_bound(&schema, nodes(&value));
        let label = format!("the_step_count_stays_within_its_formula ({name})");
        let (steps, matched) = within_a_minute(&label, move || {
            let shape = ResultShape::admit(&schema).unwrap();
            counted(&shape, &value)
        });
        println!("{name}: steps={steps} bound={bound}");
        assert_eq!(matched, matches, "{name}");
        assert!(
            steps <= bound,
            "{name}: {steps} steps over the bound {bound}"
        );
    }
}

// ── The hostile shapes, fed to OUR checker only ─────────────────────

/// `{"anyOf":[w,w]}` written out `k` times around `{"const":0}`.
fn written_out_ladder(k: usize) -> Value {
    let mut node = json!({"const": 0});
    for _ in 0..k {
        node = json!({"anyOf": [node.clone(), node]});
    }
    node
}

#[test]
fn the_hostile_shapes_are_refused_or_stay_in_bounds() {
    let reference_refusal = |first: &str| {
        format!(
            "it uses what this checker does not support: {first} at (root), $ref at (root); \
             {SUPPORTED}; write each sub-shape out where it is used"
        )
    };
    // (i) The `$ref` ladder, n = 32.
    let mut defs = Map::new();
    for i in 0..32 {
        let next = format!("#/$defs/a{}", i + 1);
        defs.insert(
            format!("a{i}"),
            json!({"anyOf": [{"$ref": next}, {"$ref": next}]}),
        );
    }
    defs.insert("a32".to_string(), json!({"const": 0}));
    let ladder = json!({"$defs": defs, "$ref": "#/$defs/a0"});
    assert_eq!(json_size_within(&ladder, usize::MAX, None), Ok(2_069));
    // (ii) The 500-link chain.
    let mut chain = Map::new();
    for i in 0..500 {
        chain.insert(
            format!("c{i}"),
            json!({"$ref": format!("#/$defs/c{}", i + 1)}),
        );
    }
    chain.insert("c500".to_string(), json!({"const": 0}));
    let chain = json!({"$defs": chain, "$ref": "#/$defs/c0"});
    assert_eq!(json_size_within(&chain, usize::MAX, None), Ok(15_332));
    // (iii) The ladder written out: nine levels fit, ten do not.
    let nine = written_out_ladder(9);
    let ten = written_out_ladder(10);
    assert_eq!(json_size_within(&nine, usize::MAX, None), Ok(12_275));
    assert_eq!(json_size_within(&ten, usize::MAX, None), Ok(24_563));
    let bound = step_bound(&nine, 1);

    let (ladder, chain, ten, nine) = within_a_minute(
        "the_hostile_shapes_are_refused_or_stay_in_bounds",
        move || {
            let ladder = ResultShape::admit(&ladder).err().map(|r| r.to_string());
            let chain = ResultShape::admit(&chain).err().map(|r| r.to_string());
            let ten = ResultShape::admit(&ten).err().map(|r| r.to_string());
            let shape = ResultShape::admit(&nine).expect("nine levels are admitted");
            probe::reset();
            let verdict = shape.check(&json!(1));
            (ladder, chain, ten, (probe::steps(), verdict))
        },
    );
    assert_eq!(ladder, Some(reference_refusal("$defs")));
    assert_eq!(chain, Some(reference_refusal("$defs")));
    assert_eq!(
        ten.as_deref(),
        Some("it is more than 16,384 bytes as compact JSON")
    );
    let (steps, verdict) = nine;
    match verdict {
        ShapeVerdict::Violations { lines, more } => {
            assert_eq!(
                lines,
                ["(root): matches none of the 2 shapes result_schema allows (anyOf)"]
            );
            assert_eq!(more, 0);
        }
        other => panic!("{other:?}"),
    }
    // Every node walked once: 1,023 nodes and 512 leaf comparisons.
    assert_eq!(steps, 1_023 + 512);
    assert!(steps <= bound, "{steps} over {bound}");
}

// ── The stack ────────────────────────────────────────────────────────

#[test]
fn the_deepest_admitted_shapes_stay_under_512_kib_of_stack() {
    const LIMIT: usize = 512 * 1024;
    let marks = std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let start_marker = 0u8;
            let start = std::hint::black_box(&start_marker) as *const u8 as usize;
            // The deepest admitted schema, walked by the admission.
            probe::reset();
            let chain = items_chain(64);
            let shape = ResultShape::admit(&chain).expect("64 levels are admitted");
            let admission = probe::stack_used_below(start);
            // The deepest admitted `const`, checked.
            let deep_const = nested_array(63);
            let constant = ResultShape::admit(&json!({ "const": deep_const.clone() })).unwrap();
            probe::reset();
            assert!(matches!(
                constant.check(&deep_const),
                ShapeVerdict::Matches { parsed: None }
            ));
            let equal = probe::stack_used_below(start);
            // `check_node`'s own descent at full depth: the items chain
            // against an array nested as deep.
            probe::reset();
            assert!(matches!(
                shape.check(&nested_array(63)),
                ShapeVerdict::Matches { parsed: None }
            ));
            let descent = probe::stack_used_below(start);
            // A 126-deep value, cloned and dropped on this thread.
            let deep: Value = serde_json::from_str(&nested_text(126)).unwrap();
            let copy = deep.clone();
            drop(copy);
            drop(deep);
            (admission, equal, descent)
        })
        .unwrap()
        .join()
        .unwrap();
    println!(
        "stack marks: admission={} const={} descent={} bytes",
        marks.0, marks.1, marks.2
    );
    assert!(marks.0 <= LIMIT, "admission {}", marks.0);
    assert!(marks.1 <= LIMIT, "const {}", marks.1);
    assert!(marks.2 <= LIMIT, "descent {}", marks.2);
}

// ── The tool around the checker ──────────────────────────────────────

/// A panic inside the check is a spent call — the daemon's own failure,
/// said so — never a rejection: the slot is not written.
#[tokio::test]
async fn a_panicking_check_is_a_spent_call_not_a_rejection() {
    let failed = tokio::task::spawn_blocking(|| -> ShapeVerdict { panic!("a checker bug") })
        .await
        .expect_err("the closure panicked");
    assert_eq!(
        super::super::check_failure(failed),
        "submit_result could not check the result: the check failed inside the daemon; this is \
         not a rejection"
    );
    let slot = super::super::TypedResultSlot::default();
    assert_eq!(slot.verdict(), super::super::SlotVerdict::Open);
    assert_eq!(slot.outcome().rejections, 0);
}

/// S-9 (the lead's ruling, as recommended): a NUL a string result's parse
/// yields is accepted as it is, and reaches the lead only as compact JSON,
/// where it is written `\u0000` — no raw NUL reaches any surface.
#[tokio::test]
async fn a_nul_reached_through_the_parse_reaches_the_lead_escaped() {
    use crate::tools::registry::BuiltInTool;
    let shape = Arc::new(admit(&json!({"type": "object"})));
    let slot = Arc::new(super::super::TypedResultSlot::default());
    let tool = super::super::SubmitResultTool {
        shape,
        slot: Arc::clone(&slot),
    };
    let answer = tool
        .execute(&json!({"result": "{\"a\":\"x\\u0000y\"}"}))
        .await;
    assert_eq!(answer.as_deref(), Ok("Result accepted."));
    let accepted = slot.outcome().accepted.expect("accepted");
    assert_eq!(accepted, r#"{"a":"x\u0000y"}"#);
    assert!(!accepted.contains('\u{0}'));
}
