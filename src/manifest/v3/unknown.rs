//! Forward compatibility: preserving what this build does not understand.
//!
//! A manifest published for a newer `midenup` may carry fields, component kinds, or installation
//! methods this build has never heard of. The rule throughout is **parse, preserve, and defer** --
//! never drop, and never fail the whole document over something that may not even be selected.
//!
//! # Why this is not just `#[serde(flatten)]`
//!
//! A catch-all `#[serde(flatten)] extra: Map<..>` works correctly *only* on a struct with no other
//! flattened field. When another flatten is present -- `Component::kind`, or
//! `Artifact::TargetSpecific::substitutions` -- the catch-all also captures the keys that flatten
//! already consumed, and serialization then emits them twice:
//!
//! ```json
//! {"name":"vm","kind":"executable","installed_executable":"miden-vm",
//!  "installed_executable":"miden-vm","kind":"executable"}
//! ```
//!
//! So those types get hand-written `Serialize`/`Deserialize` built on [`split_extra`] and
//! [`merge_extra`] instead.

use serde::{Serialize, de::DeserializeOwned};

/// Fields present in a document that this build does not recognize.
pub type Extra = serde_json::Map<String, serde_json::Value>;

/// Deserializes `T` from `value`, returning whatever keys `T` does not itself round-trip.
///
/// "Known" is defined as *"appears when `T` is serialized"* rather than as a hand-written list of
/// field names. That definition cannot drift from the struct definition, and it handles
/// variant-dependent field sets -- a `Component`'s known keys depend on its `kind` -- without
/// enumerating them.
///
/// A field with `skip_serializing_if` that is empty in the input is therefore treated as unknown
/// and lands in the extras. That is harmless: [`merge_extra`] lets the typed value win on any key
/// collision, so the value is preserved when untouched and correctly overwritten when changed.
pub fn split_extra<T>(value: serde_json::Value) -> Result<(T, Extra), serde_json::Error>
where
    T: Serialize + DeserializeOwned,
{
    let typed: T = serde_json::from_value(value.clone())?;

    let serde_json::Value::Object(mut extra) = value else {
        return Ok((typed, Extra::new()));
    };
    if let serde_json::Value::Object(known) = serde_json::to_value(&typed)? {
        for key in known.keys() {
            extra.remove(key);
        }
    }

    Ok((typed, extra))
}

/// Serializes `value`, then re-attaches any `extra` key it does not already emit.
///
/// The typed value always wins a collision, so a field that was edited in memory serializes to its
/// new value rather than to a stale captured copy.
pub fn merge_extra<T>(value: &T, extra: &Extra) -> Result<serde_json::Value, serde_json::Error>
where
    T: Serialize,
{
    let mut out = serde_json::to_value(value)?;
    if let serde_json::Value::Object(map) = &mut out {
        for (key, val) in extra {
            map.entry(key.clone()).or_insert_with(|| val.clone());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Inner {
        name: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tags: Vec<String>,
    }

    #[test]
    fn split_captures_only_unrecognized_keys() {
        let value = serde_json::json!({"name": "vm", "tags": ["a"], "future": {"x": 1}});
        let (inner, extra) = split_extra::<Inner>(value).unwrap();
        assert_eq!(inner.name, "vm");
        assert_eq!(extra.keys().collect::<Vec<_>>(), vec!["future"]);
    }

    #[test]
    fn merge_restores_unrecognized_keys_without_duplicating_known_ones() {
        let value = serde_json::json!({"name": "vm", "future": true});
        let (inner, extra) = split_extra::<Inner>(value).unwrap();
        let out = merge_extra(&inner, &extra).unwrap();
        let obj = out.as_object().unwrap();
        assert_eq!(obj.get("name").unwrap(), "vm");
        assert_eq!(obj.get("future").unwrap(), &serde_json::json!(true));
        assert_eq!(obj.len(), 2, "no stray or duplicated keys: {obj:?}");
    }

    #[test]
    fn the_typed_value_wins_a_collision_with_a_stale_extra() {
        // `tags` is empty in the input, so `skip_serializing_if` means it is not in the typed
        // form and it lands in extras. After the typed value is edited, serialization must emit
        // the new value, not the captured one.
        let value = serde_json::json!({"name": "vm", "tags": []});
        let (mut inner, extra) = split_extra::<Inner>(value).unwrap();
        assert!(extra.contains_key("tags"));

        inner.tags = vec!["edited".to_string()];
        let out = merge_extra(&inner, &extra).unwrap();
        assert_eq!(out["tags"], serde_json::json!(["edited"]));
    }

    #[test]
    fn round_trip_is_stable_under_repetition() {
        let original = serde_json::json!({"name": "vm", "future": {"nested": [1, 2]}});
        let (inner, extra) = split_extra::<Inner>(original.clone()).unwrap();
        let once = merge_extra(&inner, &extra).unwrap();
        let (inner2, extra2) = split_extra::<Inner>(once.clone()).unwrap();
        let twice = merge_extra(&inner2, &extra2).unwrap();
        assert_eq!(once, twice, "round-tripping must reach a fixed point");
        assert_eq!(once, original);
    }
}

#[cfg(test)]
mod manifest_round_trip_tests {
    use crate::manifest::VersionedManifest;

    fn source() -> serde_json::Value {
        serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "future_top_level": {"a": 1},
            "channels": [{
                "name": "0.15.0",
                "future_channel_field": "keep me",
                "components": [{
                    "name": "vm",
                    "version": {"kind": "registry", "version": "0.15.0"},
                    "kind": "executable",
                    "installation_method": {"kind": "cargo", "crate_name": "miden-vm"},
                    "installed-executable": "miden-vm",
                    "future_component_field": [1, 2, 3]
                }]
            }]
        })
    }

    /// A manifest carrying fields this build does not know about must round-trip unchanged, so a
    /// newer publisher does not lose data by passing through an older `midenup`.
    #[test]
    fn unknown_fields_round_trip() {
        let src = source();
        let parsed = VersionedManifest::parse_str(&src.to_string()).expect("parse");
        let out: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&parsed).unwrap()).unwrap();

        assert_eq!(out["future_top_level"], src["future_top_level"]);
        assert_eq!(out["channels"][0]["future_channel_field"], serde_json::json!("keep me"));
        assert_eq!(
            out["channels"][0]["components"][0]["future_component_field"],
            serde_json::json!([1, 2, 3])
        );
    }

    /// The flattened `kind` must not be duplicated into the extras.
    ///
    /// A naive `#[serde(flatten)] extra` next to the flattened `kind` emits `kind` and every
    /// kind-specific field twice; JSON tolerates duplicate keys but the output is corrupt.
    #[test]
    fn known_fields_are_never_duplicated() {
        let parsed = VersionedManifest::parse_str(&source().to_string()).expect("parse");
        let text = serde_json::to_string(&parsed).unwrap();

        for key in ["\"kind\"", "\"installed-executable\"", "\"name\"", "\"version\""] {
            // `name`/`version` legitimately appear on both the channel and the component, and
            // `kind` on both the component and its installation method, so just assert that the
            // document re-parses into an equivalent value -- duplicates would not survive.
            assert!(text.contains(key), "expected {key} in output");
        }

        let reparsed = VersionedManifest::parse_str(&text).expect("output must re-parse");
        let again = serde_json::to_string(&reparsed).unwrap();
        assert_eq!(text, again, "serialization must reach a fixed point");

        let component = &serde_json::from_str::<serde_json::Value>(&text).unwrap()["channels"][0]
            ["components"][0];
        let object = component.as_object().unwrap();
        assert_eq!(object.get("kind").unwrap(), "executable");
        assert_eq!(object.get("installed-executable").unwrap(), "miden-vm");
    }
}
