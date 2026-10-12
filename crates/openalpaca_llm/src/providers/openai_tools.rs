//! The OpenAI-compatible `tools[]` entry — one function tool as the chat
//! completions API takes it — shared by the OpenAI and Ollama providers
//! (Ollama sends through the OpenAI request builder).
//!
//! Not behind a provider feature: it is pure JSON, and the crate that owns
//! the builtin tools (`openalpaca_core`, which builds this crate with no
//! provider feature) runs every builtin schema through it in its own tests.

use crate::types::ToolDefinition;
use serde_json::{Map, Value};

/// `{"type": "function", "function": {name, description, parameters}}`,
/// with `"strict": true` only when the definition asks for it **and** its
/// schema is one OpenAI's strict mode accepts ([`strict_mode_compatible`]).
/// OpenAI refuses a strict tool whose schema breaks strict mode's rules with
/// a 400 for the whole request, so such a tool goes out without the flag —
/// best-effort adherence, which is what every other provider gives it — and
/// its schema is sent as written. Nothing is logged: this runs for every
/// tool of every request, and the outcome is fixed by the schema.
pub fn openai_function_tool(t: &ToolDefinition) -> Value {
    let mut function = serde_json::json!({
        "name": t.name,
        "description": t.description,
        "parameters": t.parameters,
    });
    if t.strict == Some(true) && strict_mode_compatible(&t.parameters) {
        function["strict"] = serde_json::json!(true);
    }
    serde_json::json!({
        "type": "function",
        "function": function,
    })
}

/// Object nesting OpenAI's strict mode accepts ("up to 10 levels").
const MAX_OBJECT_DEPTH: usize = 10;
/// Object properties it accepts across the whole schema.
const MAX_PROPERTIES: usize = 5000;
/// Any descent at all — the recursion's own bound.
const MAX_DEPTH: usize = 64;

const TYPES: &[&str] = &[
    "string", "number", "integer", "boolean", "object", "array", "null",
];

/// The `format` values the structured-outputs guide lists for strict mode.
const FORMATS: &[&str] = &[
    "date-time",
    "time",
    "date",
    "duration",
    "email",
    "hostname",
    "ipv4",
    "ipv6",
    "uuid",
];

/// Where a keyword may appear: the keywords the structured-outputs guide
/// documents for strict mode, and nothing else. This is an **allowlist** —
/// a keyword not named here (`uniqueItems`, `default`, `minProperties`, an
/// extension, …) makes the schema incompatible, because it cannot be shown
/// to be accepted.
enum Place {
    /// Any node.
    Anywhere,
    /// Only the root (`$defs`, `definitions`).
    Root,
    /// A node whose `type` includes this.
    Typed(&'static [&'static str]),
}

fn place_of(keyword: &str) -> Option<Place> {
    Some(match keyword {
        "type" | "description" | "title" | "enum" | "const" | "anyOf" | "$ref" => Place::Anywhere,
        "$defs" | "definitions" => Place::Root,
        "properties" | "required" | "additionalProperties" => Place::Typed(&["object"]),
        "items" | "minItems" | "maxItems" => Place::Typed(&["array"]),
        "pattern" | "format" => Place::Typed(&["string"]),
        "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" | "multipleOf" => {
            Place::Typed(&["number", "integer"])
        }
        _ => return None,
    })
}

/// Whether OpenAI's strict mode accepts `schema` as a function's parameters
/// (the function-calling guide, *Strict mode*; the structured-outputs guide,
/// *Supported schemas*). The root is an object and not an `anyOf`. Every
/// node is a JSON object that declares a known type (or is an `enum`, a
/// `const`, an `anyOf` or a local `$ref`) and carries only keywords from
/// the documented list, each where it applies (`place_of`):
/// `type`, `description`, `title`, `enum`, `const`, `anyOf`; `$ref` —
/// alone, with no sibling keyword (OpenAI's own strict-schema helper unravels
/// a `$ref` that has one, a `description` included) — naming `#`, `#/$defs/<name>` or
/// `#/definitions/<name>`, with `$defs`/`definitions` at the root;
/// `properties`, `required` and `additionalProperties` on an object, which
/// must be closed (`additionalProperties: false`) and require exactly its
/// keys; `items` (one schema), `minItems`, `maxItems` on an array; `pattern`
/// and a listed `format` on a string; `minimum`, `maximum`,
/// `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf` on a number. The
/// walk goes through `properties`, `items`, `anyOf`, `$defs` and
/// `definitions`, at most 10 levels of objects and 5000 properties. Anything
/// else counts as incompatible: a flag left off costs best-effort adherence,
/// a flag sent wrongly costs the request.
pub fn strict_mode_compatible(schema: &Value) -> bool {
    let Some(root) = schema.as_object() else {
        return false;
    };
    if root.get("type") != Some(&Value::String("object".to_string())) || root.contains_key("anyOf")
    {
        return false;
    }
    let mut walk = Walk {
        root,
        properties: 0,
    };
    walk.node(schema, 0, 0)
}

struct Walk<'a> {
    root: &'a Map<String, Value>,
    properties: usize,
}

impl Walk<'_> {
    fn node(&mut self, schema: &Value, depth: usize, objects: usize) -> bool {
        let Some(node) = schema.as_object() else {
            return false;
        };
        if depth > MAX_DEPTH {
            return false;
        }
        let types: Vec<&str> = match node.get("type") {
            None => Vec::new(),
            Some(Value::String(t)) => vec![t.as_str()],
            Some(Value::Array(ts)) => {
                let names: Vec<&str> = ts.iter().filter_map(Value::as_str).collect();
                if names.len() != ts.len() || names.is_empty() {
                    return false;
                }
                names
            }
            Some(_) => return false,
        };
        if types.iter().any(|t| !TYPES.contains(t)) {
            return false;
        }
        let described = !types.is_empty()
            || ["enum", "const", "anyOf", "$ref"]
                .iter()
                .any(|k| node.contains_key(*k));
        if !described {
            return false;
        }
        // The allowlist: every keyword known, in its place, of the right shape.
        for (keyword, value) in node {
            let placed = match place_of(keyword) {
                None => false,
                Some(Place::Anywhere) => true,
                Some(Place::Root) => depth == 0,
                Some(Place::Typed(wanted)) => wanted.iter().any(|t| types.contains(t)),
            };
            if !placed || !Self::shaped(keyword, value) {
                return false;
            }
        }
        if let Some(reference) = node.get("$ref")
            && (node.len() != 1 || !self.resolves(reference))
        {
            return false;
        }
        for defs in ["$defs", "definitions"] {
            if let Some(defs) = node.get(defs).and_then(Value::as_object)
                && !defs.values().all(|d| self.node(d, depth + 1, objects))
            {
                return false;
            }
        }
        if types.contains(&"object")
            && (objects >= MAX_OBJECT_DEPTH || !self.object(node, depth, objects + 1))
        {
            return false;
        }
        if types.contains(&"array") && !node.contains_key("items") {
            return false;
        }
        if let Some(items) = node.get("items")
            && !self.node(items, depth + 1, objects)
        {
            return false;
        }
        if let Some(branches) = node.get("anyOf").and_then(Value::as_array)
            && !branches.iter().all(|b| self.node(b, depth + 1, objects))
        {
            return false;
        }
        true
    }

    /// The value a keyword on the allowlist must hold; the recursive ones
    /// (`items`, `anyOf`'s branches, a definition) are checked by the walk.
    fn shaped(keyword: &str, value: &Value) -> bool {
        match keyword {
            "description" | "title" | "pattern" | "$ref" => value.is_string(),
            "format" => value.as_str().is_some_and(|f| FORMATS.contains(&f)),
            "enum" => value.as_array().is_some_and(|v| !v.is_empty()),
            "anyOf" => value.as_array().is_some_and(|v| !v.is_empty()),
            "$defs" | "definitions" | "properties" => value.is_object(),
            "required" => value.is_array(),
            "additionalProperties" => value.is_boolean(),
            "minItems" | "maxItems" => value.is_u64(),
            "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum" => value.is_number(),
            "multipleOf" => value.as_f64().is_some_and(|m| m > 0.0),
            // `type` is checked above; `const` may hold any value.
            _ => true,
        }
    }

    /// Closed, every key required, every property compliant.
    fn object(&mut self, node: &Map<String, Value>, depth: usize, objects: usize) -> bool {
        if node.get("additionalProperties") != Some(&Value::Bool(false)) {
            return false;
        }
        let Some(properties) = node.get("properties").and_then(Value::as_object) else {
            return false;
        };
        let Some(required) = node.get("required").and_then(Value::as_array) else {
            return false;
        };
        self.properties += properties.len();
        if self.properties > MAX_PROPERTIES {
            return false;
        }
        let mut named: Vec<&str> = Vec::with_capacity(required.len());
        for key in required {
            let Some(key) = key.as_str() else {
                return false;
            };
            if named.contains(&key) || !properties.contains_key(key) {
                return false;
            }
            named.push(key);
        }
        if named.len() != properties.len() {
            return false;
        }
        properties
            .values()
            .all(|p| self.node(p, depth + 1, objects))
    }

    /// `#`, or `#/$defs/<name>` / `#/definitions/<name>` naming a definition
    /// the root holds (those are walked where they are declared).
    fn resolves(&self, reference: &Value) -> bool {
        let Some(reference) = reference.as_str() else {
            return false;
        };
        if reference == "#" {
            return true;
        }
        ["$defs", "definitions"].iter().any(|defs| {
            reference
                .strip_prefix(&format!("#/{defs}/"))
                .is_some_and(|name| {
                    !name.contains('/')
                        && self
                            .root
                            .get(*defs)
                            .and_then(Value::as_object)
                            .is_some_and(|d| d.contains_key(name))
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// An object schema closed and fully required, over `properties`.
    fn closed(properties: Value) -> Value {
        let keys: Vec<String> = properties.as_object().unwrap().keys().cloned().collect();
        json!({
            "type": "object",
            "properties": properties,
            "required": keys,
            "additionalProperties": false,
        })
    }

    #[test]
    fn a_compliant_schema_is_compatible() {
        let schema = json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "What to find" },
                "limit": { "type": ["integer", "null"] },
                "mode": { "type": "string", "enum": ["fast", "full"] },
                "filters": {
                    "type": "array",
                    "items": closed(json!({ "field": { "type": "string" } })),
                },
                "range": closed(json!({ "from": { "type": "string" } })),
                "target": {
                    "anyOf": [
                        closed(json!({ "id": { "type": "string" } })),
                        { "type": "null" },
                    ],
                },
                "node": { "$ref": "#/$defs/node" },
                "tree": { "$ref": "#" },
            },
            "required": ["query", "limit", "mode", "filters", "range", "target", "node", "tree"],
            "additionalProperties": false,
            "$defs": { "node": closed(json!({ "label": { "type": "string" } })) },
        });
        assert!(strict_mode_compatible(&schema));
        // No properties at all is still a closed, fully required object.
        assert!(strict_mode_compatible(&closed(json!({}))));
        // Every keyword on the allowlist, where it belongs.
        let mut schema = closed(json!({
            "when": { "type": ["string", "null"], "format": "date-time", "title": "When" },
            "code": { "type": "string", "pattern": "^[a-z]+$" },
            "count": {
                "type": "integer", "minimum": 0, "maximum": 10,
                "exclusiveMinimum": -1, "exclusiveMaximum": 11, "multipleOf": 1,
            },
            "tags": { "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": 5 },
            "fixed": { "const": "v1" },
            "node": { "$ref": "#/definitions/node" },
        }));
        schema["definitions"] = json!({ "node": closed(json!({})) });
        assert!(strict_mode_compatible(&schema), "{schema}");
        for format in [
            "date-time",
            "time",
            "date",
            "duration",
            "email",
            "hostname",
            "ipv4",
            "ipv6",
            "uuid",
        ] {
            let schema = closed(json!({ "v": { "type": "string", "format": format } }));
            assert!(strict_mode_compatible(&schema), "{format}");
        }
    }

    #[test]
    fn a_missing_or_open_additional_properties_is_incompatible() {
        let mut schema = closed(json!({ "q": { "type": "string" } }));
        schema
            .as_object_mut()
            .unwrap()
            .remove("additionalProperties");
        assert!(!strict_mode_compatible(&schema));
        schema["additionalProperties"] = json!(true);
        assert!(!strict_mode_compatible(&schema));
        schema["additionalProperties"] = json!({ "type": "string" });
        assert!(!strict_mode_compatible(&schema));
    }

    #[test]
    fn an_optional_key_is_incompatible() {
        let mut schema = closed(json!({ "q": { "type": "string" }, "n": { "type": "integer" } }));
        schema["required"] = json!(["q"]);
        assert!(!strict_mode_compatible(&schema));
        schema.as_object_mut().unwrap().remove("required");
        assert!(!strict_mode_compatible(&schema));
        // A name `required` lists that is not a property, or twice.
        schema["required"] = json!(["q", "n", "extra"]);
        assert!(!strict_mode_compatible(&schema));
        schema["required"] = json!(["q", "q"]);
        assert!(!strict_mode_compatible(&schema));
    }

    #[test]
    fn a_nested_object_that_violates_is_incompatible() {
        let schema = closed(json!({
            "patch": {
                "type": "object",
                "properties": { "title": { "type": "string" } },
                "additionalProperties": false,
            },
        }));
        assert!(!strict_mode_compatible(&schema));
        // An object with no `properties` map is not verifiable either.
        let schema = closed(json!({ "metadata": { "type": "object" } }));
        assert!(!strict_mode_compatible(&schema));
    }

    #[test]
    fn an_array_of_violating_objects_is_incompatible() {
        let schema = closed(json!({
            "sources": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": { "id": { "type": "string" }, "at": { "type": "string" } },
                    "required": ["id"],
                    "additionalProperties": false,
                },
            },
        }));
        assert!(!strict_mode_compatible(&schema));
        // An array with no `items`, or a tuple of them.
        let schema = closed(json!({ "tags": { "type": "array" } }));
        assert!(!strict_mode_compatible(&schema));
        let schema =
            closed(json!({ "pair": { "type": "array", "items": [{ "type": "string" }] } }));
        assert!(!strict_mode_compatible(&schema));
    }

    #[test]
    fn a_violating_any_of_branch_or_definition_is_incompatible() {
        let open = json!({ "type": "object", "properties": {}, "required": [] });
        let schema = closed(json!({ "target": { "anyOf": [open.clone(), { "type": "null" }] } }));
        assert!(!strict_mode_compatible(&schema));
        let mut schema = closed(json!({ "node": { "$ref": "#/$defs/node" } }));
        schema["$defs"] = json!({ "node": open });
        assert!(!strict_mode_compatible(&schema));
    }

    #[test]
    fn what_cannot_be_verified_is_incompatible() {
        let cases = [
            // The root: not an object, or a union.
            json!({ "type": "string" }),
            json!({ "anyOf": [closed(json!({}))] }),
            json!(true),
            // A property with no type, a boolean schema, an unknown type.
            closed(json!({ "anything": {} })),
            closed(json!({ "anything": { "description": "free" } })),
            closed(json!({ "anything": true })),
            closed(json!({ "when": { "type": "date" } })),
            // Composition beyond `anyOf`, and conditional keywords.
            closed(json!({ "v": { "oneOf": [{ "type": "string" }, { "type": "null" }] } })),
            closed(json!({ "v": { "allOf": [{ "type": "string" }] } })),
            closed(json!({ "v": { "type": "string", "not": { "const": "x" } } })),
            // A `$ref` that does not resolve to a local definition.
            closed(json!({ "v": { "$ref": "#/$defs/missing" } })),
            closed(json!({ "v": { "$ref": "https://example.com/schema.json" } })),
            closed(json!({ "v": { "$ref": "#/properties/other" } })),
            // Object keywords on a string.
            closed(json!({ "v": { "type": "string", "required": [] } })),
            // Keywords strict mode does not accept, on a closed, fully
            // required schema (the review's probe): an allowlist, not a
            // denylist, decides.
            closed(
                json!({ "v": { "type": "array", "items": { "type": "string" }, "uniqueItems": true } }),
            ),
            closed(
                json!({ "v": { "type": "array", "items": { "type": "string" }, "minContains": 1 } }),
            ),
            json!({
                "type": "object", "properties": {}, "required": [],
                "additionalProperties": false, "minProperties": 0,
            }),
            json!({
                "type": "object", "properties": {}, "required": [],
                "additionalProperties": false, "maxProperties": 4,
            }),
            closed(json!({ "v": { "type": "string", "default": "x" } })),
            closed(json!({ "v": { "type": "string", "x-internal": true } })),
            // A `$ref` with any sibling keyword, a `description` included.
            {
                let mut schema = closed(json!({ "v": { "$ref": "#/$defs/n", "title": "N" } }));
                schema["$defs"] = json!({ "n": closed(json!({})) });
                schema
            },
            {
                let mut schema =
                    closed(json!({ "v": { "$ref": "#/$defs/n", "description": "N" } }));
                schema["$defs"] = json!({ "n": closed(json!({})) });
                schema
            },
            // A `format` strict mode does not list, and one on a non-string.
            closed(json!({ "v": { "type": "string", "format": "uri" } })),
            closed(json!({ "v": { "type": "integer", "format": "date" } })),
            // Number, array and string keywords where they do not belong.
            closed(json!({ "v": { "type": "string", "minimum": 1 } })),
            closed(json!({ "v": { "type": "integer", "pattern": "^[0-9]+$" } })),
            closed(json!({ "v": { "type": "string", "maxItems": 2 } })),
        ];
        for schema in cases {
            assert!(!strict_mode_compatible(&schema), "{schema}");
        }
        let mut schema = closed(json!({ "tags": { "type": "object" } }));
        schema["properties"]["tags"] = json!({
            "type": "object",
            "properties": {},
            "patternProperties": { "^x-": { "type": "string" } },
            "required": [],
            "additionalProperties": false,
        });
        assert!(!strict_mode_compatible(&schema));
    }

    #[test]
    fn ten_levels_of_objects_are_compatible_and_eleven_are_not() {
        let nest = |levels: usize| {
            let mut schema = closed(json!({ "leaf": { "type": "string" } }));
            for _ in 1..levels {
                schema = closed(json!({ "child": schema }));
            }
            schema
        };
        assert!(strict_mode_compatible(&nest(10)));
        assert!(!strict_mode_compatible(&nest(11)));
    }

    fn definition(parameters: Value, strict: Option<bool>) -> ToolDefinition {
        ToolDefinition {
            name: "calc".to_string(),
            description: "Calculate".to_string(),
            parameters,
            strict,
            input_examples: None,
        }
    }

    #[test]
    fn the_tool_entry_carries_strict_only_when_asked_and_compatible() {
        let compliant = closed(json!({ "expression": { "type": "string" } }));
        let open =
            json!({ "type": "object", "properties": { "expression": { "type": "string" } } });
        for (parameters, strict, sent) in [
            (compliant.clone(), Some(true), true),
            (compliant.clone(), None, false),
            (compliant.clone(), Some(false), false),
            (open.clone(), Some(true), false),
            (open.clone(), None, false),
        ] {
            let entry = openai_function_tool(&definition(parameters.clone(), strict));
            assert_eq!(entry["type"], "function");
            let function = &entry["function"];
            assert_eq!(function["name"], "calc");
            assert_eq!(function["description"], "Calculate");
            assert_eq!(function["parameters"], parameters, "sent as written");
            assert_eq!(
                function.get("strict") == Some(&json!(true)),
                sent,
                "{strict:?} {parameters}"
            );
            if !sent {
                assert!(function.get("strict").is_none(), "{function}");
            }
        }
    }
}
