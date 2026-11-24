//! The result checker of OpenAlpaca's own (SA-4, design §5.3): a lead's
//! `result_schema` is admitted once at spawn ([`ResultShape::admit`]) and a
//! typed child's submitted `result` is checked against it on every
//! `submit_result` call ([`ResultShape::check`]).
//!
//! A small, fixed subset of JSON Schema, decided by the owner on 2026-11-23
//! ("as proposed", design §5.3.1): `type`, `enum`, `const`, `anyOf`,
//! `properties`, `required`, `additionalProperties` (true or false),
//! `items` (one schema), `minItems`, `maxItems` and the four number bounds
//! are checked; `format` and the annotations are let through unchecked;
//! every other keyword — `pattern` and every reference keyword among them —
//! is refused by name at spawn. A schema is at most 16,384 bytes as compact
//! JSON, 64 levels of nesting and 10 object levels; a result at most 65,536
//! bytes. There is no work limit: those caps bound the work (§5.3.4).
//!
//! `serde_json` and `std` only. No regular expression is on this path, and
//! nothing here calls another schema validator.

use serde_json::{Map, Number, Value};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "shape_bench.rs"]
mod shape_bench;

/// The most bytes a schema may take as compact JSON (L3).
pub(super) const SCHEMA_MAX_BYTES: usize = 16_384;
/// The most levels of JSON nesting a schema may have (L3).
pub(super) const SCHEMA_MAX_NESTING: usize = 64;
/// The most object levels on one path of a schema (Y-12 (5), kept by L3).
pub(super) const MAX_OBJECT_LEVELS: usize = 10;
/// The most bytes a submitted result may take as compact JSON (L3).
pub(super) const RESULT_MAX_BYTES: usize = 65_536;
/// The most violation lines a rejection shows; the rest are counted.
pub(super) const MAX_LINES: usize = 19;
/// Each violation line is cut at this many characters of escaped text.
pub(super) const LINE_MAX_CHARS: usize = 200;
/// A name, a violation line's path segment or a refusal's whole path is
/// first cut to this many bytes …
const NAME_PREFIX_BYTES: usize = 256;
/// … and shown in at most this many bytes.
const NAME_SHOWN_BYTES: usize = 64;
/// A refusal names at most this many problems; the rest are counted.
const MAX_NAMED_PROBLEMS: usize = 8;
/// The named problems of one refusal share this many bytes.
const PROBLEM_BUDGET_BYTES: usize = 1_024;

/// The line a submission with no `result` member gets (Y-16).
pub(super) const MISSING_RESULT_LINE: &str = "(root): \"result\" is a required property";
/// The line a result over [`RESULT_MAX_BYTES`] gets (L3).
pub(super) const TOO_LARGE_LINE: &str =
    "(root): the result is more than 65,536 bytes as compact JSON; send a smaller one";

/// The supported list, as the off-list refusal names it (L1).
const SUPPORTED: &str = "supported: type, enum, const, anyOf, properties, required, \
     additionalProperties (true or false), items (one schema), minItems, maxItems, minimum, \
     maximum, exclusiveMinimum, exclusiveMaximum; not checked: format and the annotations";
/// Added when `pattern` is named (L1).
const PATTERN_HINT: &str = "; say a pattern in \"description\" instead";
/// Added when a reference keyword is named (L2 (A)).
const REFERENCE_HINT: &str = "; write each sub-shape out where it is used";
/// The reference keywords L2 (A) refuses, which take [`REFERENCE_HINT`].
const REFERENCE_KEYWORDS: [&str; 9] = [
    "$ref",
    "$defs",
    "definitions",
    "$dynamicRef",
    "$dynamicAnchor",
    "$recursiveRef",
    "$recursiveAnchor",
    "$id",
    "$anchor",
];
/// The keywords that assert nothing (L1): `format` and the annotations.
const UNCHECKED_KEYWORDS: [&str; 9] = [
    "format",
    "title",
    "description",
    "default",
    "examples",
    "$comment",
    "deprecated",
    "readOnly",
    "writeOnly",
];

/// What a schema node is, once admitted: the checked keywords only.
#[derive(Debug, Default)]
struct ShapeNode {
    types: Option<KindSet>,
    constant: Option<Value>,
    enumeration: Option<Vec<Value>>,
    any_of: Option<Vec<ShapeNode>>,
    properties: Option<BTreeMap<String, ShapeNode>>,
    required: Vec<String>,
    /// `Some(false)` makes an unlisted member a violation.
    additional: Option<bool>,
    items: Option<Box<ShapeNode>>,
    min_items: Option<Number>,
    max_items: Option<Number>,
    minimum: Option<Number>,
    maximum: Option<Number>,
    exclusive_minimum: Option<Number>,
    exclusive_maximum: Option<Number>,
}

/// The kinds a `type` names, folded into seven bits (duplicates fold).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KindSet(u8);

/// The seven type names, in the order a line lists them.
const KIND_NAMES: [&str; 7] = [
    "string", "number", "integer", "boolean", "object", "array", "null",
];

impl KindSet {
    fn bit(name: &str) -> Option<u8> {
        KIND_NAMES.iter().position(|k| *k == name).map(|i| 1u8 << i)
    }

    fn has(self, name: &str) -> bool {
        Self::bit(name).is_some_and(|b| self.0 & b != 0)
    }

    /// Whether `value`'s kind is one this set names; `integer` follows L4.
    fn admits(self, value: &Value) -> bool {
        match value {
            Value::Null => self.has("null"),
            Value::Bool(_) => self.has("boolean"),
            Value::String(_) => self.has("string"),
            Value::Array(_) => self.has("array"),
            Value::Object(_) => self.has("object"),
            Value::Number(n) => self.has("number") || (self.has("integer") && is_integer(n)),
        }
    }

    fn names(self) -> String {
        KIND_NAMES
            .iter()
            .enumerate()
            .filter(|(i, _)| self.0 & (1 << i) != 0)
            .map(|(_, k)| *k)
            .collect::<Vec<_>>()
            .join(" or ")
    }
}

/// L4: JSON Schema's integer — `3.0`, `-0` and an underflowed `1e-400` are
/// integers; `1.5` is not.
fn is_integer(n: &Number) -> bool {
    n.is_u64()
        || n.is_i64()
        || n.as_f64()
            .is_some_and(|f| f.is_finite() && f.fract() == 0.0)
}

/// A count bound (`minItems`, `maxItems`): an integer (L4) not below zero.
fn is_count(n: &Number) -> bool {
    is_integer(n) && number_cmp(n, &Number::from(0u64)) != Ordering::Less
}

/// One number, read exactly: an integer as `i128`, anything else as `f64`.
enum Exact {
    Int(i128),
    Float(f64),
}

fn exact(n: &Number) -> Exact {
    if let Some(u) = n.as_u64() {
        Exact::Int(i128::from(u))
    } else if let Some(i) = n.as_i64() {
        Exact::Int(i128::from(i))
    } else {
        // A `Number` is one of the three; NaN and the infinities cannot
        // arrive (serde refuses an overflowing literal).
        Exact::Float(n.as_f64().unwrap_or(0.0))
    }
}

/// 2^127 as an `f64`: at or past it, a float is outside `i128`.
const I128_EDGE: f64 = 170_141_183_460_469_231_731_687_303_715_884_105_728.0;

/// `f` against the integer `i`, exactly (§5.3.4).
fn float_against_int(f: f64, i: i128) -> Ordering {
    let t = f.trunc();
    if t >= I128_EDGE {
        return Ordering::Greater;
    }
    if t < -I128_EDGE {
        return Ordering::Less;
    }
    // `t` is integral and inside `i128`, so the cast is exact.
    match (t as i128).cmp(&i) {
        Ordering::Equal => (f - t).partial_cmp(&0.0).unwrap_or(Ordering::Equal),
        other => other,
    }
}

/// Two numbers compared exactly across `u64`, `i64` and `f64` (L4): `1`
/// equals `1.0`, `-0.0` equals `0`, and the f64 2^64 is above `u64::MAX`.
fn number_cmp(a: &Number, b: &Number) -> Ordering {
    match (exact(a), exact(b)) {
        (Exact::Int(x), Exact::Int(y)) => x.cmp(&y),
        (Exact::Float(x), Exact::Float(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        (Exact::Float(x), Exact::Int(y)) => float_against_int(x, y),
        (Exact::Int(x), Exact::Float(y)) => float_against_int(y, x).reverse(),
    }
}

/// Deep equality as JSON Schema means it (L4): numbers by [`number_cmp`],
/// strings byte for byte, arrays pairwise, objects member by member. Its
/// own cycle, entered at a leaf of the check; it never calls `check_node`.
fn json_equal(a: &Value, b: &Value) -> bool {
    #[cfg(test)]
    probe::step();
    #[cfg(test)]
    probe::touch_stack();
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => number_cmp(x, y) == Ordering::Equal,
        (Value::String(x), Value::String(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| json_equal(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((k1, v1), (k2, v2))| k1 == k2 && json_equal(v1, v2))
        }
        _ => false,
    }
}

// ── The size walk ────────────────────────────────────────────────────

/// Why [`json_size_within`] stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TooBig {
    Bytes,
    Nesting,
}

/// A sink that counts what `serde_json` writes.
struct Count(usize);

impl std::io::Write for Count {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The compact length `serde_json` gives a number.
fn number_len(n: &Number) -> usize {
    let mut count = Count(0);
    // Writing into a counter cannot fail.
    let _ = serde_json::to_writer(&mut count, n);
    count.0
}

/// One character's length inside a JSON string, as `serde_json` escapes it.
fn escaped_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        c if u32::from(c) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// Add a string's compact length (quotes included) to `total`, one
/// character at a time, stopping inside it once `total` passes `max_bytes`.
fn add_string(text: &str, total: &mut usize, max_bytes: usize) -> Result<(), TooBig> {
    *total += 1;
    for c in text.chars() {
        #[cfg(test)]
        probe::size_step();
        *total += escaped_len(c);
        if *total > max_bytes {
            return Err(TooBig::Bytes);
        }
    }
    *total += 1;
    if *total > max_bytes {
        return Err(TooBig::Bytes);
    }
    Ok(())
}

/// One open container of the size walk.
enum Open<'a> {
    Array(std::slice::Iter<'a, Value>, bool),
    Object(serde_json::map::Iter<'a>, bool),
}

/// `value`'s length as compact JSON, as `serde_json::to_vec` would write
/// it, or where it passed `max_bytes` or `max_depth` levels of nesting —
/// in one walk that holds an iterator per open container on the heap and
/// does not recurse, and stops at the first byte or level past its cap.
fn json_size_within(
    value: &Value,
    max_bytes: usize,
    max_depth: Option<usize>,
) -> Result<usize, TooBig> {
    let mut total = 0usize;
    let mut stack: Vec<Open<'_>> = Vec::new();
    let mut next = Some(value);
    loop {
        if let Some(v) = next.take() {
            #[cfg(test)]
            probe::size_step();
            match v {
                Value::Null | Value::Bool(true) => total += 4,
                Value::Bool(false) => total += 5,
                Value::Number(n) => total += number_len(n),
                Value::String(s) => add_string(s, &mut total, max_bytes)?,
                Value::Array(a) => {
                    if max_depth.is_some_and(|max| stack.len() >= max) {
                        return Err(TooBig::Nesting);
                    }
                    total += 1;
                    stack.push(Open::Array(a.iter(), true));
                }
                Value::Object(o) => {
                    if max_depth.is_some_and(|max| stack.len() >= max) {
                        return Err(TooBig::Nesting);
                    }
                    total += 1;
                    stack.push(Open::Object(o.iter(), true));
                }
            }
            if total > max_bytes {
                return Err(TooBig::Bytes);
            }
        }
        match stack.last_mut() {
            None => return Ok(total),
            Some(Open::Array(iter, first)) => match iter.next() {
                Some(element) => {
                    if !*first {
                        total += 1;
                    }
                    *first = false;
                    next = Some(element);
                }
                None => {
                    total += 1;
                    stack.pop();
                }
            },
            Some(Open::Object(iter, first)) => match iter.next() {
                Some((key, member)) => {
                    #[cfg(test)]
                    probe::size_step();
                    if !*first {
                        total += 1;
                    }
                    *first = false;
                    add_string(key, &mut total, max_bytes)?;
                    total += 1;
                    next = Some(member);
                }
                None => {
                    total += 1;
                    stack.pop();
                }
            },
        }
        if total > max_bytes {
            return Err(TooBig::Bytes);
        }
    }
}

/// Whether a submitted result is within [`RESULT_MAX_BYTES`] as compact
/// JSON — the size check `submit_result` runs inline before it clones the
/// result for the check (§5.3.4 step 2).
pub(super) fn result_fits(value: &Value) -> bool {
    json_size_within(value, RESULT_MAX_BYTES, None).is_ok()
}

// ── Names, paths and lines ───────────────────────────────────────────

/// One segment of a path: a member's name or an element's index.
#[derive(Debug, Clone, Copy)]
enum Seg<'a> {
    Key(&'a str),
    Index(usize),
}

/// RFC 6901's escaping of a pointer segment: `~` as `~0`, `/` as `~1`.
fn pointer_escaped(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

/// `text` cut to at most `max` bytes on a character boundary, with `…`
/// after it when anything was cut.
fn cut_marked(text: &str, max: usize) -> String {
    let kept = crate::utils::text::byte_prefix(text, max);
    if kept.len() < text.len() {
        format!("{kept}…")
    } else {
        kept.to_string()
    }
}

/// A keyword name, or a whole schema path, for a refusal (§5.3.2): its
/// first 256 bytes, escaped one-line, and at most 64 bytes of that, never
/// cut inside an escape, `…` after a cut.
fn refusal_name(text: &str) -> String {
    use std::fmt::Write as _;
    let raw = crate::utils::text::byte_prefix(text, NAME_PREFIX_BYTES);
    let mut shown = String::new();
    let mut cut = raw.len() < text.len();
    for c in raw.chars() {
        let before = shown.len();
        if crate::utils::text::needs_escape_in_line(c) {
            // Writing into a `String` cannot fail.
            let _ = write!(shown, "<U+{:04X}>", u32::from(c));
        } else {
            shown.push(c);
        }
        if shown.len() > NAME_SHOWN_BYTES {
            shown.truncate(before);
            cut = true;
            break;
        }
    }
    if cut {
        shown.push('…');
    }
    shown
}

/// A schema path for a refusal: `(root)`, or a JSON Pointer shown whole as
/// [`refusal_name`] shows a name — its first 256 bytes, escaped one-line,
/// cut at 64 bytes of escaped text — so one named problem stays far inside
/// the refusal's 1,024-byte budget however deep the path runs.
fn refusal_path(path: &[Seg<'_>]) -> String {
    if path.is_empty() {
        return "(root)".to_string();
    }
    let mut pointer = String::new();
    for seg in path {
        pointer.push('/');
        match seg {
            Seg::Key(key) => pointer.push_str(&pointer_escaped(crate::utils::text::byte_prefix(
                key,
                NAME_PREFIX_BYTES,
            ))),
            Seg::Index(i) => pointer.push_str(&i.to_string()),
        }
        if pointer.len() > NAME_PREFIX_BYTES {
            break;
        }
    }
    refusal_name(&pointer)
}

/// A value path for a violation line: `(root)`, or a JSON Pointer whose
/// every key is its first 256 bytes, RFC 6901-escaped, cut at 64 bytes
/// (the line's own escape and cut follow, [`line`]).
fn line_path(path: &[Seg<'_>]) -> String {
    if path.is_empty() {
        return "(root)".to_string();
    }
    path.iter()
        .map(|seg| match seg {
            Seg::Key(key) => format!(
                "/{}",
                cut_marked(
                    &pointer_escaped(crate::utils::text::byte_prefix(key, NAME_PREFIX_BYTES)),
                    NAME_SHOWN_BYTES
                )
            ),
            Seg::Index(i) => format!("/{i}"),
        })
        .collect()
}

/// One violation line as the child reads it: escaped one-line and cut at
/// [`LINE_MAX_CHARS`] characters of escaped text (§5.3.6).
fn line(path: &[Seg<'_>], message: &str) -> String {
    crate::utils::text::escape_hidden_line_within(
        &format!("{}: {message}", line_path(path)),
        LINE_MAX_CHARS,
    )
    .text
}

// ── The admission ────────────────────────────────────────────────────

/// Why a `result_schema` was refused; its `Display` is the reason the
/// lead reads after `result_schema is not usable: `.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::runner::lead_agent) struct ShapeRefusal(String);

impl fmt::Display for ShapeRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One problem the keyword walk found.
enum Problem {
    /// A keyword off the list, at the schema node holding it.
    OffList { keyword: String, shown: String },
    /// A listed keyword of the wrong shape.
    WrongShape { shown: String },
}

impl Problem {
    fn shown(&self) -> &str {
        match self {
            Problem::OffList { shown, .. } | Problem::WrongShape { shown } => shown,
        }
    }
}

/// The problems of one walk: at most [`MAX_NAMED_PROBLEMS`] named, the
/// rest counted.
#[derive(Default)]
struct Problems {
    named: Vec<Problem>,
    more: usize,
}

impl Problems {
    fn off_list(&mut self, keyword: &str, path: &[Seg<'_>]) {
        if self.named.len() >= MAX_NAMED_PROBLEMS {
            self.more += 1;
            return;
        }
        let shown = format!("{} at {}", refusal_name(keyword), refusal_path(path));
        self.named.push(Problem::OffList {
            keyword: keyword.to_string(),
            shown,
        });
    }

    fn wrong_shape(&mut self, keyword: &str, path: &[Seg<'_>], shape: &str) {
        if self.named.len() >= MAX_NAMED_PROBLEMS {
            self.more += 1;
            return;
        }
        let shown = format!(
            "\"{}\" at {} must be {shape}",
            refusal_name(keyword),
            refusal_path(path)
        );
        self.named.push(Problem::WrongShape { shown });
    }

    /// The reason, its named problems within [`PROBLEM_BUDGET_BYTES`]
    /// (dropped whole, last first, and counted), its fixed parts whole.
    /// The first problem that does not fit ends the list: it and every
    /// later one are counted, never a later one kept past it. A problem
    /// is at most about 200 bytes (a name and a path of 64 bytes each,
    /// marked, and a shape phrase), so the first always fits.
    fn reason(self) -> String {
        let mut more = self.more;
        let mut kept: Vec<Problem> = Vec::new();
        let mut used = 0usize;
        let mut dropping = false;
        for problem in self.named {
            let cost = problem.shown().len() + 2;
            dropping |= used + cost > PROBLEM_BUDGET_BYTES;
            if dropping {
                more += 1;
            } else {
                used += cost;
                kept.push(problem);
            }
        }
        let and_more = if more > 0 {
            format!(" (and {more} more)")
        } else {
            String::new()
        };
        let mut off_list: Vec<&str> = Vec::new();
        let mut wrong: Vec<&str> = Vec::new();
        let mut pattern = false;
        let mut reference = false;
        for problem in &kept {
            match problem {
                Problem::OffList { keyword, shown } => {
                    pattern |= keyword == "pattern";
                    reference |= REFERENCE_KEYWORDS.contains(&keyword.as_str());
                    off_list.push(shown);
                }
                Problem::WrongShape { shown } => wrong.push(shown),
            }
        }
        if off_list.is_empty() {
            return format!("{}{and_more}", wrong.join("; "));
        }
        let mut reason = format!(
            "it uses what this checker does not support: {}{and_more}; {SUPPORTED}",
            off_list.join(", ")
        );
        if pattern {
            reason.push_str(PATTERN_HINT);
        }
        if reference {
            reason.push_str(REFERENCE_HINT);
        }
        for shown in wrong {
            reason.push_str("; ");
            reason.push_str(shown);
        }
        reason
    }
}

/// `a boolean`, `a number`, `a string`, `an array` or `null`: what a
/// schema that is not an object is.
fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Whether a schema node names `object` in its `type`, as strict mode's
/// walker counts an object level.
fn names_object(node: &Map<String, Value>) -> bool {
    match node.get("type") {
        Some(Value::String(t)) => t == "object",
        Some(Value::Array(types)) => types.iter().any(|t| t.as_str() == Some("object")),
        _ => false,
    }
}

/// The most object levels on one path below and including `node`,
/// following only well-shaped edges (Y-12 (5), §5.3.2 step 3): run before
/// the keyword walk, on nodes not yet checked; its recursion is bounded by
/// the 64-level cap step 2 has enforced.
fn object_levels(node: &Value) -> usize {
    let Value::Object(map) = node else {
        return 0;
    };
    let own = usize::from(names_object(map));
    let mut deepest = 0;
    if let Some(Value::Object(properties)) = map.get("properties") {
        for member in properties.values() {
            deepest = deepest.max(object_levels(member));
        }
    }
    if let Some(items @ Value::Object(_)) = map.get("items") {
        deepest = deepest.max(object_levels(items));
    }
    if let Some(Value::Array(branches)) = map.get("anyOf") {
        for branch in branches {
            deepest = deepest.max(object_levels(branch));
        }
    }
    own + deepest
}

/// A schema position (`items`, an `anyOf` member, a `properties` member)
/// compiled, or the problem it holds recorded at its own path.
fn compile_position<'a>(
    keyword: &str,
    value: &'a Value,
    path: &mut Vec<Seg<'a>>,
    problems: &mut Problems,
) -> Option<ShapeNode> {
    match value {
        Value::Object(map) => Some(compile(map, path, problems)),
        Value::Bool(_) => {
            problems.wrong_shape(keyword, path, "a schema object, not true or false");
            None
        }
        _ => {
            problems.wrong_shape(keyword, path, "a schema object");
            None
        }
    }
}

/// The keyword walk (§5.3.2 step 4): every key of `node` against the
/// table, every known keyword's value against its shape, the compiled node
/// built as it goes. Never reads inside an `enum`, `const` or annotation
/// value as a schema; a key of `properties` is a name.
fn compile<'a>(
    node: &'a Map<String, Value>,
    path: &mut Vec<Seg<'a>>,
    problems: &mut Problems,
) -> ShapeNode {
    #[cfg(test)]
    probe::touch_stack();
    let mut shape = ShapeNode::default();
    for (key, value) in node {
        match key.as_str() {
            "type" => {
                let folded = match value {
                    Value::String(t) => KindSet::bit(t),
                    Value::Array(types) if !types.is_empty() => {
                        types.iter().try_fold(0u8, |set, t| {
                            t.as_str().and_then(KindSet::bit).map(|b| set | b)
                        })
                    }
                    _ => None,
                };
                match folded {
                    Some(bits) => shape.types = Some(KindSet(bits)),
                    None => problems.wrong_shape(
                        key,
                        path,
                        "a type name or a non-empty array of type names",
                    ),
                }
            }
            "enum" => match value {
                Value::Array(members) if !members.is_empty() => {
                    shape.enumeration = Some(members.clone());
                }
                _ => problems.wrong_shape(key, path, "a non-empty array"),
            },
            "const" => shape.constant = Some(value.clone()),
            "anyOf" => match value {
                Value::Array(branches) if !branches.is_empty() => {
                    let mut compiled = Vec::with_capacity(branches.len());
                    for (i, branch) in branches.iter().enumerate() {
                        path.push(Seg::Key("anyOf"));
                        path.push(Seg::Index(i));
                        if let Some(node) = compile_position(key, branch, path, problems) {
                            compiled.push(node);
                        }
                        path.pop();
                        path.pop();
                    }
                    shape.any_of = Some(compiled);
                }
                _ => problems.wrong_shape(key, path, "a non-empty array of schemas"),
            },
            "properties" => match value {
                Value::Object(members) => {
                    let mut compiled = BTreeMap::new();
                    for (name, member) in members {
                        path.push(Seg::Key("properties"));
                        path.push(Seg::Key(name));
                        if let Some(node) = compile_position(key, member, path, problems) {
                            compiled.insert(name.clone(), node);
                        }
                        path.pop();
                        path.pop();
                    }
                    shape.properties = Some(compiled);
                }
                _ => problems.wrong_shape(key, path, "an object of schemas"),
            },
            "required" => match value {
                Value::Array(names) if names.iter().all(Value::is_string) => {
                    shape.required = names
                        .iter()
                        .filter_map(|n| n.as_str().map(str::to_string))
                        .collect();
                }
                _ => problems.wrong_shape(key, path, "an array of strings"),
            },
            "additionalProperties" => match value {
                Value::Bool(allowed) => shape.additional = Some(*allowed),
                _ => problems.wrong_shape(key, path, "true or false"),
            },
            "items" => match value {
                Value::Array(_) => problems.wrong_shape(key, path, "one schema, not an array"),
                _ => {
                    path.push(Seg::Key("items"));
                    if let Some(node) = compile_position(key, value, path, problems) {
                        shape.items = Some(Box::new(node));
                    }
                    path.pop();
                }
            },
            "minItems" | "maxItems" => match value {
                Value::Number(n) if is_count(n) => {
                    if key == "minItems" {
                        shape.min_items = Some(n.clone());
                    } else {
                        shape.max_items = Some(n.clone());
                    }
                }
                _ => problems.wrong_shape(key, path, "a non-negative integer"),
            },
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" => match value {
                Value::Number(n) => {
                    let slot = match key.as_str() {
                        "minimum" => &mut shape.minimum,
                        "maximum" => &mut shape.maximum,
                        "exclusiveMinimum" => &mut shape.exclusive_minimum,
                        _ => &mut shape.exclusive_maximum,
                    };
                    *slot = Some(n.clone());
                }
                _ => problems.wrong_shape(key, path, "a number"),
            },
            "format" | "title" | "description" | "$comment" => {
                if !value.is_string() {
                    problems.wrong_shape(key, path, "a string");
                }
            }
            "default" => {}
            "examples" => {
                if !value.is_array() {
                    problems.wrong_shape(key, path, "an array");
                }
            }
            "deprecated" | "readOnly" | "writeOnly" => {
                if !value.is_boolean() {
                    problems.wrong_shape(key, path, "true or false");
                }
            }
            _ => problems.off_list(key, path),
        }
    }
    shape
}

/// L5 (i): whether a root `result_schema` asks for nothing — `{}`, or a
/// root whose every key is `format` or an annotation — and so is treated
/// as absent. Reads the root's key names only; a non-object is not
/// "nothing" (the admission refuses it).
pub(in crate::runner::lead_agent) fn asks_nothing(schema: &Value) -> bool {
    schema.as_object().is_some_and(|root| {
        root.keys()
            .all(|k| UNCHECKED_KEYWORDS.contains(&k.as_str()))
    })
}

/// An admitted `result_schema`: the compiled tree a typed child's results
/// are checked against.
#[derive(Debug)]
pub(in crate::runner::lead_agent) struct ResultShape {
    root: ShapeNode,
}

/// What one check of a submitted result found.
#[derive(Debug)]
pub(in crate::runner::lead_agent) enum ShapeVerdict {
    /// The result matches; `parsed` is the value a string result parsed to,
    /// when that is what matched (Y-13), else `None` (the raw value).
    Matches { parsed: Option<Value> },
    /// At most [`MAX_LINES`] lines, then how many more violations there were.
    Violations { lines: Vec<String>, more: usize },
    /// Over [`RESULT_MAX_BYTES`] as compact JSON.
    TooLarge,
}

/// The two ways the check walks a node: recording lines, or deciding an
/// `anyOf` branch, which records nothing and stops at the first failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Collect,
    Decide,
}

/// The lines a check keeps and the path it is at.
struct Lines<'v> {
    kept: Vec<String>,
    count: usize,
    path: Vec<Seg<'v>>,
}

impl Lines<'_> {
    fn record(&mut self, message: impl FnOnce() -> String) {
        self.count += 1;
        if self.kept.len() < MAX_LINES {
            let text = message();
            self.kept.push(line(&self.path, &text));
        }
    }
}

impl ResultShape {
    /// Admit `schema` once, at spawn (§5.3.2), or say why not: not an
    /// object; over 16,384 bytes or 64 levels; 11 or more object levels;
    /// a keyword off the list or of the wrong shape.
    pub(in crate::runner::lead_agent) fn admit(schema: &Value) -> Result<Self, ShapeRefusal> {
        let Value::Object(root) = schema else {
            return Err(ShapeRefusal(format!(
                "it is {}, not a JSON object",
                json_kind(schema)
            )));
        };
        match json_size_within(schema, SCHEMA_MAX_BYTES, Some(SCHEMA_MAX_NESTING)) {
            Ok(_) => {}
            Err(TooBig::Bytes) => {
                return Err(ShapeRefusal(
                    "it is more than 16,384 bytes as compact JSON".to_string(),
                ));
            }
            Err(TooBig::Nesting) => {
                return Err(ShapeRefusal("it nests more than 64 levels".to_string()));
            }
        }
        if object_levels(schema) > MAX_OBJECT_LEVELS {
            return Err(ShapeRefusal(
                "it nests more than 10 levels of objects".to_string(),
            ));
        }
        let mut problems = Problems::default();
        let mut path = Vec::with_capacity(SCHEMA_MAX_NESTING + 1);
        let root = compile(root, &mut path, &mut problems);
        if problems.named.is_empty() && problems.more == 0 {
            Ok(Self { root })
        } else {
            Err(ShapeRefusal(problems.reason()))
        }
    }

    /// Check one submitted `result` (§5.3.4): its size first; then the raw
    /// value; then, if that fails and it is a string, the value it parses
    /// to, once — unless that is over the cap, when the raw value's lines
    /// stand. Runs on the blocking pool (`submit_result`).
    pub(in crate::runner::lead_agent) fn check(&self, value: &Value) -> ShapeVerdict {
        if json_size_within(value, RESULT_MAX_BYTES, None).is_err() {
            return ShapeVerdict::TooLarge;
        }
        let Some((lines, more)) = self.violations(value) else {
            return ShapeVerdict::Matches { parsed: None };
        };
        if let Value::String(text) = value
            && let Ok(parsed) = serde_json::from_str::<Value>(text)
            && json_size_within(&parsed, RESULT_MAX_BYTES, None).is_ok()
        {
            return match self.violations(&parsed) {
                None => ShapeVerdict::Matches {
                    parsed: Some(parsed),
                },
                Some((lines, more)) => ShapeVerdict::Violations { lines, more },
            };
        }
        ShapeVerdict::Violations { lines, more }
    }

    /// The kept lines and how many more, or `None` when `value` matches.
    fn violations(&self, value: &Value) -> Option<(Vec<String>, usize)> {
        let mut lines = Lines {
            kept: Vec::new(),
            count: 0,
            path: Vec::with_capacity(SCHEMA_MAX_NESTING + 1),
        };
        if check_node(&self.root, value, 1, Mode::Collect, &mut lines) {
            return None;
        }
        let more = lines.count.saturating_sub(lines.kept.len());
        Some((lines.kept, more))
    }
}

/// The kind word a type line names for `value` (`integer` for a whole
/// number, L4).
fn kind_word(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if is_integer(n) => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// The check of one node against one value (§5.3.4) — the only function
/// on the descent cycle, so each descent adds one frame. In `Collect` mode
/// every violation is recorded and the walk goes on (a `type` mismatch
/// ends this node); in `Decide` mode nothing is recorded and the first
/// failure returns. Every keyword at a node must hold.
fn check_node<'v>(
    node: &ShapeNode,
    value: &'v Value,
    depth: usize,
    mode: Mode,
    out: &mut Lines<'v>,
) -> bool {
    debug_assert!(depth <= SCHEMA_MAX_NESTING, "the admission caps nesting");
    #[cfg(test)]
    probe::step();
    #[cfg(test)]
    probe::touch_stack();
    let collect = mode == Mode::Collect;

    if let Some(types) = node.types
        && !types.admits(value)
    {
        if collect {
            out.record(|| format!("expected {}, got {}", types.names(), kind_word(value)));
        }
        return false;
    }
    let mut ok = true;
    if let Some(constant) = &node.constant
        && !json_equal(constant, value)
    {
        if !collect {
            return false;
        }
        ok = false;
        out.record(|| "is not the value result_schema requires (const)".to_string());
    }
    if let Some(members) = &node.enumeration
        && !members.iter().any(|m| json_equal(m, value))
    {
        if !collect {
            return false;
        }
        ok = false;
        let n = members.len();
        out.record(|| format!("is not one of the {n} values result_schema allows (enum)"));
    }
    if let Some(branches) = &node.any_of
        && !branches
            .iter()
            .any(|b| check_node(b, value, depth + 1, Mode::Decide, out))
    {
        if !collect {
            return false;
        }
        ok = false;
        let n = branches.len();
        out.record(|| format!("matches none of the {n} shapes result_schema allows (anyOf)"));
    }
    match value {
        Value::Object(map) => {
            for name in &node.required {
                #[cfg(test)]
                probe::step();
                if !map.contains_key(name) {
                    if !collect {
                        return false;
                    }
                    ok = false;
                    out.record(|| {
                        format!(
                            "\"{}\" is a required property",
                            cut_marked(
                                crate::utils::text::byte_prefix(name, NAME_PREFIX_BYTES),
                                NAME_SHOWN_BYTES
                            )
                        )
                    });
                }
            }
            if node.properties.is_some() || node.additional == Some(false) {
                for (key, member) in map {
                    #[cfg(test)]
                    probe::step();
                    match node.properties.as_ref().and_then(|p| p.get(key)) {
                        Some(child) => {
                            out.path.push(Seg::Key(key));
                            let pass = check_node(child, member, depth + 1, mode, out);
                            out.path.pop();
                            if !pass {
                                if !collect {
                                    return false;
                                }
                                ok = false;
                            }
                        }
                        None if node.additional == Some(false) => {
                            if !collect {
                                return false;
                            }
                            ok = false;
                            out.path.push(Seg::Key(key));
                            out.record(|| {
                                "is not allowed (additionalProperties is false)".to_string()
                            });
                            out.path.pop();
                        }
                        None => {}
                    }
                }
            }
        }
        Value::Array(elements) => {
            let len = Number::from(elements.len() as u64);
            if let Some(min) = &node.min_items
                && number_cmp(&len, min) == Ordering::Less
            {
                if !collect {
                    return false;
                }
                ok = false;
                let n = elements.len();
                out.record(|| format!("has {n} items; at least {min}"));
            }
            if let Some(max) = &node.max_items
                && number_cmp(&len, max) == Ordering::Greater
            {
                if !collect {
                    return false;
                }
                ok = false;
                let n = elements.len();
                out.record(|| format!("has {n} items; at most {max}"));
            }
            if let Some(item) = &node.items {
                for (i, element) in elements.iter().enumerate() {
                    out.path.push(Seg::Index(i));
                    let pass = check_node(item, element, depth + 1, mode, out);
                    out.path.pop();
                    if !pass {
                        if !collect {
                            return false;
                        }
                        ok = false;
                    }
                }
            }
        }
        Value::Number(n) => {
            let bounds: [(&Option<Number>, Ordering, &str); 4] = [
                (&node.minimum, Ordering::Less, "is below the minimum"),
                (&node.maximum, Ordering::Greater, "is above the maximum"),
                (&node.exclusive_minimum, Ordering::Less, "is not above"),
                (&node.exclusive_maximum, Ordering::Greater, "is not below"),
            ];
            for (i, (bound, outside, words)) in bounds.into_iter().enumerate() {
                let Some(bound) = bound else { continue };
                let order = number_cmp(n, bound);
                // The exclusive bounds also refuse the bound itself.
                let fails = order == outside || (i >= 2 && order == Ordering::Equal);
                if fails {
                    if !collect {
                        return false;
                    }
                    ok = false;
                    out.record(|| match i {
                        2 => format!("{words} {bound} (exclusiveMinimum)"),
                        3 => format!("{words} {bound} (exclusiveMaximum)"),
                        _ => format!("{words} {bound}"),
                    });
                }
            }
        }
        _ => {}
    }
    ok
}

/// The test-only counters (§5.3.4, §5.3.7): the check's node steps, the
/// size walk's steps and the deepest stack address a walk reached. Each is
/// a thread-local, so tests running on parallel threads never add to one
/// another's counts; a test resets and reads them on the thread that runs
/// the check.
#[cfg(test)]
mod probe {
    use std::cell::Cell;

    thread_local! {
        static STEPS: Cell<u64> = const { Cell::new(0) };
        static SIZE_STEPS: Cell<u64> = const { Cell::new(0) };
        static STACK_LOW: Cell<usize> = const { Cell::new(usize::MAX) };
    }

    /// One `check_node` call, `json_equal` node pair, member iteration or
    /// `required` lookup.
    pub(super) fn step() {
        STEPS.with(|s| s.set(s.get() + 1));
    }

    /// One character scanned, or one value or member opened, by the size
    /// walk.
    pub(super) fn size_step() {
        SIZE_STEPS.with(|s| s.set(s.get() + 1));
    }

    /// Note how deep this frame sits on the thread's stack.
    pub(super) fn touch_stack() {
        let marker = 0u8;
        let at = std::hint::black_box(&marker) as *const u8 as usize;
        STACK_LOW.with(|low| {
            if at < low.get() {
                low.set(at);
            }
        });
    }

    /// Zero every counter on this thread.
    pub(super) fn reset() {
        STEPS.with(|s| s.set(0));
        SIZE_STEPS.with(|s| s.set(0));
        STACK_LOW.with(|s| s.set(usize::MAX));
    }

    pub(super) fn steps() -> u64 {
        STEPS.with(Cell::get)
    }

    pub(super) fn size_steps() -> u64 {
        SIZE_STEPS.with(Cell::get)
    }

    /// How far below `start` (an address taken at the thread's start) the
    /// deepest touched frame reached, in bytes.
    pub(super) fn stack_used_below(start: usize) -> usize {
        STACK_LOW.with(|low| start.saturating_sub(low.get()))
    }
}
