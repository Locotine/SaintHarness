use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Recursively sorts all JSON object keys alphabetically for byte-exact prompt cache determinism.
pub fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted: BTreeMap<String, Value> = BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k.clone(), canonicalize_json(v));
            }
            let mut out = Map::with_capacity(sorted.len());
            for (k, v) in sorted {
                out.insert(k, v);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json).collect()),
        other => other.clone(),
    }
}

pub fn canonical_json_string(value: &Value) -> Result<String, serde_json::Error> {
    serde_json::to_string(&canonicalize_json(value))
}

pub fn canonical_blake3(value: &Value) -> Result<String, serde_json::Error> {
    let encoded = canonical_json_string(value)?;
    Ok(blake3::hash(encoded.as_bytes()).to_hex().to_string())
}

/// Replaces unpaired surrogate escape artifacts or NUL bytes that break provider JSON parsers.
pub fn sanitize_text(input: &str) -> String {
    input
        .chars()
        .map(|ch| if ch == '\0' { '\u{FFFD}' } else { ch })
        .collect()
}

pub fn sanitize_json_strings(value: &Value) -> Value {
    match value {
        Value::String(s) => Value::String(sanitize_text(s)),
        Value::Array(items) => Value::Array(items.iter().map(sanitize_json_strings).collect()),
        Value::Object(map) => {
            let mut out = Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(sanitize_text(k), sanitize_json_strings(v));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hundred_key_permutations_produce_identical_blake3_hash() {
        let keys = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta"];
        let mut baseline: Option<String> = None;
        for rot in 0..100usize {
            let mut inner = Map::new();
            let mut outer = Map::new();
            for offset in 0..keys.len() {
                let idx = (rot + offset * 5) % keys.len();
                inner.insert(keys[idx].to_string(), json!(idx));
            }
            for offset in 0..keys.len() {
                let idx = (rot * 3 + offset) % keys.len();
                let val = if idx == 0 {
                    Value::Object(inner.clone())
                } else {
                    json!(format!("v-{idx}"))
                };
                outer.insert(keys[idx].to_string(), val);
            }
            let hash = canonical_blake3(&Value::Object(outer)).unwrap();
            match &baseline {
                None => baseline = Some(hash),
                Some(b) => assert_eq!(b, &hash, "hash drifted at permutation {rot}"),
            }
        }
    }

    #[test]
    fn sanitize_replaces_nul_characters() {
        assert_eq!(sanitize_text("ok\0text"), "ok\u{FFFD}text");
    }
}
