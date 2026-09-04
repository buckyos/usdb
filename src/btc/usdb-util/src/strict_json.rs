use serde::Deserialize;
use serde::de::{DeserializeOwned, Error as _, MapAccess, SeqAccess, Visitor};
use std::collections::HashSet;
use std::fmt;

struct DuplicateKeyRejectingValue;

struct DuplicateKeyRejectingVisitor;

impl<'de> Deserialize<'de> for DuplicateKeyRejectingValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(DuplicateKeyRejectingVisitor)
    }
}

impl<'de> Visitor<'de> for DuplicateKeyRejectingVisitor {
    type Value = DuplicateKeyRejectingValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(DuplicateKeyRejectingValue)
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(DuplicateKeyRejectingValue)
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(DuplicateKeyRejectingValue)
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(DuplicateKeyRejectingValue)
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(DuplicateKeyRejectingValue)
    }

    fn visit_string<E>(self, _value: String) -> Result<Self::Value, E> {
        Ok(DuplicateKeyRejectingValue)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(DuplicateKeyRejectingValue)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(DuplicateKeyRejectingValue)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence
            .next_element::<DuplicateKeyRejectingValue>()?
            .is_some()
        {}
        Ok(DuplicateKeyRejectingValue)
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = HashSet::new();
        while let Some(key) = object.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(A::Error::custom(format!("duplicate JSON key: {key}")));
            }
            object.next_value::<DuplicateKeyRejectingValue>()?;
        }
        Ok(DuplicateKeyRejectingValue)
    }
}

/// Deserializes JSON only after recursively rejecting duplicate object keys.
///
/// This intentionally performs a small preflight parse before schema deserialization. It is
/// intended for identity-bearing manifests and key catalogs where cross-parser ambiguity is less
/// acceptable than the cost of parsing the input twice.
pub fn parse_json_slice_strict<T>(input: &[u8]) -> Result<T, serde_json::Error>
where
    T: DeserializeOwned,
{
    let mut preflight = serde_json::Deserializer::from_slice(input);
    DuplicateKeyRejectingValue::deserialize(&mut preflight)?;
    preflight.end()?;
    serde_json::from_slice(input)
}

/// String-input variant of [`parse_json_slice_strict`].
pub fn parse_json_strict<T>(input: &str) -> Result<T, serde_json::Error>
where
    T: DeserializeOwned,
{
    parse_json_slice_strict(input.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::parse_json_strict;
    use serde_json::Value;

    #[test]
    fn shared_corpus_matches_expected_duplicate_key_semantics() {
        let corpus: Value = serde_json::from_str(include_str!(
            "../testdata/strict-json-duplicate-key-corpus.json"
        ))
        .unwrap();
        assert_eq!(
            corpus["schema_version"],
            "usdb-strict-json-duplicate-key-corpus:v1"
        );
        for case in corpus["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let input = case["json"].as_str().unwrap();
            let expected_valid = case["valid"].as_bool().unwrap();
            let result = parse_json_strict::<Value>(input);
            assert_eq!(
                result.is_ok(),
                expected_valid,
                "strict JSON corpus case {name} returned {result:?}"
            );
            if !expected_valid {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("duplicate JSON key"),
                    "strict JSON corpus case {name} did not report a duplicate key"
                );
            }
        }
    }
}
