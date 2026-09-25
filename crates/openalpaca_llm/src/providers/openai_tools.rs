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

/// Keywords strict mode does not support, or whose effect on "every key is
/// required, nothing else is allowed" this check cannot verify. OpenAI
/// documents `anyOf` as the only composition it supports, so `oneOf` and
/// `allOf` are here too.
const UNVERIFIABLE: &[&str] = &[
    "oneOf",
    "allOf",
    "not",
    "if",
    "then",
    "else",
    "patternProperties",
    "unevaluatedProperties",
    "unevaluatedItems",
    "propertyNames",
    "dependentRequired",
    "dependentSchemas",
    "prefixItems",
    "additionalItems",
    "contains",
];

const TYPES: &[&str] = &[
    "string", "number", "integer", "boolean", "object", "array", "null",
];

/// Whether OpenAI's strict mode accepts `schema` as a function's parameters
/// (the function-calling guide, *Strict mode*; the structured-outputs guide,
/// *Supported schemas*): the root is an object and not an `anyOf`; every
/// object has `additionalProperties: false`, a `properties` map, and
/// `required` naming exactly its keys; every node declares a type (or is an
/// `enum`, a `const`, an `anyOf` or a local `$ref`); an array has one `items`
/// schema; and the same holds through `properties`, `items`, `anyOf`,
/// `$defs` and `definitions`. Anything this cannot verify — an
/// `UNVERIFIABLE` keyword, a boolean or untyped schema, a `$ref` that is not
/// `#`, `#/$defs/<name>` or `#/definitions/<name>` naming a definition that
/// exists, more than 10 levels of objects or 5000 properties — counts as
/// incompatible: a flag left off costs best-effort adherence, a flag sent
/// wrongly costs the request.
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
        if depth > MAX_DEPTH || UNVERIFIABLE.iter().any(|k| node.contains_key(*k)) {
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
        if let Some(reference) = node.get("$ref")
            && !self.resolves(reference)
        {
            return false;
        }
        for defs in ["$defs", "definitions"] {
            if let Some(defs) = node.get(defs) {
                let Some(defs) = defs.as_object() else {
                    return false;
                };
                if !defs.values().all(|d| self.node(d, depth + 1, objects)) {
                    return false;
                }
            }
        }
        if types.contains(&"object") || node.contains_key("properties") {
            if objects >= MAX_OBJECT_DEPTH || !self.object(node, depth, objects + 1) {
                return false;
            }
        } else if node.contains_key("required") || node.contains_key("additionalProperties") {
            // Object keywords on a node that says it is not an object.
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
        if let Some(branches) = node.get("anyOf") {
            let Some(branches) = branches.as_array() else {
                return false;
            };
            if branches.is_empty() || !branches.iter().all(|b| self.node(b, depth + 1, objects)) {
                return false;
            }
        }
        true
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
