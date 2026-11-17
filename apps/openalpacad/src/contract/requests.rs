//! A request body's sample, read off the very type the route deserializes
//! (CLI-PROTOCOL's amended half; *Rev 107*'s request-fields item, (b)).
//!
//! A request type derives `Deserialize` only, so no value of it can be
//! serialized; what serde's derive does give is the list of the wire names
//! it reads — the `fields` it hands `Deserializer::deserialize_struct`,
//! after `rename` and `rename_all`. [`fields`] asks for that list with a
//! deserializer that answers nothing else, and [`Body`] writes it as a
//! sample: each wire name a key, its value `null` (the kind is not read),
//! or the shape of a nested request type the producer names. A field
//! renamed or removed in the type drops its key path from the sample,
//! which the folder's check refuses unless `PROTOCOL` moved — the bump
//! check the responses already have. An alias (`#[serde(alias)]`) is listed
//! beside the name it stands for, as serde's derive lists it: the type reads
//! both, so dropping either is a removal.

use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};
use serde_json::{Map, Value};

/// The answer [`Introspect`] gives: the struct's wire names, or nothing for
/// a type that is not a struct.
#[derive(Debug)]
struct Stop(Option<&'static [&'static str]>);

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("introspection stop")
    }
}

impl std::error::Error for Stop {}

impl de::Error for Stop {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Stop(None)
    }
}

/// A deserializer that answers only the struct question.
struct Introspect;

impl<'de> Deserializer<'de> for Introspect {
    type Error = Stop;

    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Stop> {
        Err(Stop(None))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Stop> {
        Err(Stop(Some(fields)))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

/// The wire names `T` reads. Panics for a type serde does not describe as
/// a struct (a map, an enum, a `flatten`ed struct): its producer must say
/// what it reads another way.
pub(crate) fn fields<'de, T: Deserialize<'de>>() -> &'static [&'static str] {
    match T::deserialize(Introspect) {
        Err(Stop(Some(fields))) => fields,
        _ => panic!(
            "{} does not describe itself as a struct to serde; name its fields another way",
            std::any::type_name::<T>()
        ),
    }
}

/// A request body's sample: every wire name of its type, and the shape of
/// each nested request type named.
pub(crate) struct Body {
    fields: Map<String, Value>,
}

impl Body {
    /// `T`'s wire names, each `null`.
    pub(crate) fn of<'de, T: Deserialize<'de>>() -> Self {
        Self {
            fields: fields::<T>()
                .iter()
                .map(|name| ((*name).to_string(), Value::Null))
                .collect(),
        }
    }

    /// The field `name` holds an object of `inner`'s shape.
    pub(crate) fn object(self, name: &str, inner: Body) -> Self {
        self.nest(name, inner.into_value())
    }

    /// The field `name` holds an array of `inner`'s shape.
    pub(crate) fn array(self, name: &str, inner: Body) -> Self {
        self.nest(name, Value::Array(vec![inner.into_value()]))
    }

    fn nest(mut self, name: &str, value: Value) -> Self {
        let slot = self
            .fields
            .get_mut(name)
            .unwrap_or_else(|| panic!("the body has no field {name:?} to nest a shape in"));
        *slot = value;
        self
    }

    pub(crate) fn into_value(self) -> Value {
        Value::Object(self.fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    #[allow(dead_code)]
    struct Inner {
        some_key: String,
    }

    #[derive(Deserialize)]
    #[allow(dead_code)]
    struct Outer {
        #[serde(rename = "wire_name")]
        field: u32,
        #[serde(alias = "old")]
        kept: Option<String>,
        #[serde(default)]
        items: Vec<Inner>,
    }

    #[test]
    fn a_body_holds_the_wire_names_after_renames_and_its_nested_shapes() {
        assert_eq!(fields::<Outer>(), ["wire_name", "kept", "old", "items"]);
        assert_eq!(
            Body::of::<Outer>()
                .array("items", Body::of::<Inner>())
                .into_value(),
            serde_json::json!({
                "wire_name": null, "kept": null, "old": null, "items": [{"someKey": null}]
            })
        );
    }

    #[test]
    #[should_panic(expected = "does not describe itself as a struct")]
    fn a_type_that_is_not_a_struct_is_refused() {
        let _ = fields::<std::collections::BTreeMap<String, String>>();
    }
}
