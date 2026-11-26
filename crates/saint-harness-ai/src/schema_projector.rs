use crate::canonical::canonicalize_json;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

const MAX_SCHEMA_DEPTH: usize = 16;

/// Projects JSON Schema into a flattened provider-safe subset (Gemini-compatible):
/// inlines `$defs`/`definitions`/`$ref`, flattens `allOf`, strips `$schema`, `additionalProperties`, and `default`.
pub fn project_schema(schema: &Value) -> Value {
    let mut defs = BTreeMap::new();
    collect_defs(schema, &mut defs);
    let mut visiting = BTreeSet::new();
    let projected = project_node(schema, &defs, &mut visiting, 0);
    canonicalize_json(&projected)
}

fn collect_defs(root: &Value, defs: &mut BTreeMap<String, Value>) {
    if let Value::Object(map) = root {
        for container_key in ["$defs", "definitions"] {
            if let Some(Value::Object(entries)) = map.get(container_key) {
                for (k, v) in entries {
                    defs.insert(k.clone(), v.clone());
                }
            }
        }
    }
}

fn resolve_ref_name(ref_str: &str) -> Option<&str> {
    ref_str
        .strip_prefix("#/$defs/")
        .or_else(|| ref_str.strip_prefix("#/definitions/"))
}

fn project_node(
    node: &Value,
    defs: &BTreeMap<String, Value>,
    visiting: &mut BTreeSet<String>,
    depth: usize,
) -> Value {
    if depth >= MAX_SCHEMA_DEPTH {
        let mut fallback = Map::new();
        fallback.insert("type".to_string(), Value::String("object".to_string()));
        return Value::Object(fallback);
    }

    match node {
        Value::Object(map) => {
            if let Some(Value::String(ref_path)) = map.get("$ref")
                && let Some(name) = resolve_ref_name(ref_path)
            {
                if visiting.contains(name) {
                    let mut cycle = Map::new();
                    cycle.insert("type".to_string(), Value::String("object".to_string()));
                    return Value::Object(cycle);
                }
                if let Some(target) = defs.get(name) {
                    visiting.insert(name.to_string());
                    let mut merged = project_node(target, defs, visiting, depth + 1);
                    visiting.remove(name);
                    if let Value::Object(ref mut m) = merged
                        && let Some(desc) = map.get("description")
                    {
                        m.entry("description".to_string())
                            .or_insert_with(|| desc.clone());
                    }
                    return merged;
                }
            }

            let mut out = Map::new();
            let mut merged_props = Map::new();
            let mut merged_required = BTreeSet::new();

            if let Some(Value::Array(all_of)) = map.get("allOf") {
                for branch in all_of {
                    let projected_branch = project_node(branch, defs, visiting, depth + 1);
                    if let Value::Object(bm) = projected_branch {
                        if let Some(Value::Object(props)) = bm.get("properties") {
                            for (pk, pv) in props {
                                merged_props.insert(pk.clone(), pv.clone());
                            }
                        }
                        if let Some(Value::Array(req)) = bm.get("required") {
                            for item in req {
                                if let Value::String(s) = item {
                                    merged_required.insert(s.clone());
                                }
                            }
                        }
                        for (bk, bv) in bm {
                            if bk != "properties" && bk != "required" {
                                out.entry(bk).or_insert(bv);
                            }
                        }
                    }
                }
            }

            for (k, v) in map {
                if matches!(
                    k.as_str(),
                    "$schema"
                        | "$defs"
                        | "definitions"
                        | "additionalProperties"
                        | "default"
                        | "allOf"
                        | "$ref"
                ) {
                    continue;
                }
                match k.as_str() {
                    "properties" => {
                        if let Value::Object(props) = v {
                            for (pk, pv) in props {
                                merged_props.insert(
                                    pk.clone(),
                                    project_node(pv, defs, visiting, depth + 1),
                                );
                            }
                        }
                    }
                    "required" => {
                        if let Value::Array(req) = v {
                            for item in req {
                                if let Value::String(s) = item {
                                    merged_required.insert(s.clone());
                                }
                            }
                        }
                    }
                    "items" => {
                        out.insert(k.clone(), project_node(v, defs, visiting, depth + 1));
                    }
                    "anyOf" | "oneOf" => {
                        if let Value::Array(variants) = v {
                            let projected_variants: Vec<Value> = variants
                                .iter()
                                .map(|item| project_node(item, defs, visiting, depth + 1))
                                .collect();
                            out.insert(k.clone(), Value::Array(projected_variants));
                        }
                    }
                    _ => {
                        out.insert(k.clone(), v.clone());
                    }
                }
            }

            if !merged_props.is_empty() {
                out.insert("properties".to_string(), Value::Object(merged_props));
                out.entry("type".to_string())
                    .or_insert_with(|| Value::String("object".to_string()));
            }
            if !merged_required.is_empty() {
                out.insert(
                    "required".to_string(),
                    Value::Array(merged_required.into_iter().map(Value::String).collect()),
                );
            }

            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(
            arr.iter()
                .map(|item| project_node(item, defs, visiting, depth + 1))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flattens_refs_allof_and_strips_unsupported_fields() {
        let schema = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "$defs": {
                "Range": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "start": { "type": "integer", "default": 1 },
                        "end": { "type": "integer" }
                    },
                    "required": ["start"]
                }
            },
            "allOf": [
                { "$ref": "#/$defs/Range" },
                {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"]
                }
            ],
            "additionalProperties": false
        });
        let projected = project_schema(&schema);
        assert!(projected.get("$schema").is_none());
        assert!(projected.get("$defs").is_none());
        assert!(projected.get("additionalProperties").is_none());
        assert!(projected.get("allOf").is_none());
        assert!(projected["properties"]["start"].get("default").is_none());
        assert_eq!(projected["required"], json!(["path", "start"]));
    }

    #[test]
    fn handles_cyclic_refs_without_stack_overflow() {
        let cyclic = json!({
            "$defs": {
                "Node": {
                    "type": "object",
                    "properties": {
                        "next": { "$ref": "#/$defs/Node" }
                    }
                }
            },
            "$ref": "#/$defs/Node"
        });
        let projected = project_schema(&cyclic);
        assert_eq!(projected["type"], json!("object"));
        assert_eq!(projected["properties"]["next"]["type"], json!("object"));
    }
}
