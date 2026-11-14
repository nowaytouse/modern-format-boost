//! Reject contradictory JSON members before a value-based evidence reader can discard them.

use serde::Deserialize;
use std::collections::BTreeSet;

// Serde owns syntax, escaping, numeric parsing and recursion limits. This
// visitor only retains each object's decoded keys to enforce uniqueness.
struct UniqueMembers;

impl<'de> Deserialize<'de> for UniqueMembers {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueMembers;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON with unique object members")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut keys = BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !keys.insert(key.clone()) {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate JSON evidence member {key}"
                        )));
                    }
                    map.next_value::<UniqueMembers>()?;
                }
                Ok(UniqueMembers)
            }
            fn visit_seq<S: serde::de::SeqAccess<'de>>(
                self,
                mut seq: S,
            ) -> std::result::Result<Self::Value, S::Error> {
                while seq.next_element::<UniqueMembers>()?.is_some() {}
                Ok(UniqueMembers)
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                _: bool,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueMembers)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> std::result::Result<Self::Value, E> {
                Ok(UniqueMembers)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> std::result::Result<Self::Value, E> {
                Ok(UniqueMembers)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> std::result::Result<Self::Value, E> {
                Ok(UniqueMembers)
            }
            fn visit_str<E: serde::de::Error>(
                self,
                _: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueMembers)
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(UniqueMembers)
            }
        }
        d.deserialize_any(Visitor)
    }
}

/// Call before deserializing evidence into `Value` or another partial view.
/// Callers retain their own byte limits and schema/identity validation.
pub(super) fn validate_unique_json_members(bytes: &[u8]) -> anyhow::Result<()> {
    let _: UniqueMembers = serde_json::from_slice(bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_and_escaped_duplicate_members_cannot_override_evidence() {
        for bytes in [
            br#"{"state":"failed","state":"committed"}"#.as_slice(),
            br#"{"request":{"version":2,"version":1}}"#.as_slice(),
            br#"{"identities":[{"id":"a","\u0069d":"b"}]}"#.as_slice(),
            br#"{"state":"committed"} {"other":true}"#.as_slice(),
        ] {
            assert!(validate_unique_json_members(bytes).is_err());
        }
        validate_unique_json_members(
            br#"{"request":{"id":1},"identities":[{"id":2}],"other":[true,false,null,-1,1.5]}"#,
        )
        .unwrap();
    }
}
