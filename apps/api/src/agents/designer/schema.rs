//! The strict JSON schema the designer model must answer with: exactly `AgentSpec`. Every property is
//! required and nullable where `AgentSpec` allows a missing value, so a strict `json_schema`
//! provider accepts it; `AgentSpec`'s own `deny_unknown_fields` is the second line of defence.

use serde_json::{Value, json};

use crate::agents::registry::spec::{CATEGORIES, NOTIFY_TARGETS, PRIORITIES};

fn strings() -> Value {
    json!({"type": "array", "items": {"type": "string"}})
}

fn string_enum(values: &[&str]) -> Value {
    json!({"type": "string", "enum": values})
}

fn object(properties: Value) -> Value {
    let required: Vec<&String> = properties.as_object().map(|o| o.keys().collect()).unwrap_or_default();
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

fn trigger() -> Value {
    json!({"anyOf": [
        object(json!({
            "type": string_enum(&["email_received"]),
            "match": object(json!({
                "labels": strings(), "from_domains": strings(), "from_addresses": strings(), "subject_keywords": strings(),
                "needs_reply": {"type": ["boolean", "null"]},
                "priority": {"type": ["string", "null"], "enum": [PRIORITIES[0], PRIORITIES[1], PRIORITIES[2], null]},
                "attachment_types": strings(),
            })),
        })),
        object(json!({"type": string_enum(&["schedule"]), "every_hours": {"type": "integer"}})),
        object(json!({"type": string_enum(&["manual"])})),
    ]})
}

fn step() -> Value {
    let quality = string_enum(&["fast", "balanced", "best"]);
    let field = object(json!({
        "name": {"type": "string"}, "type": string_enum(&["string", "number", "date", "boolean"]), "description": {"type": "string"},
    }));
    json!({"anyOf": [
        object(json!({"type": string_enum(&["extract"]), "fields": {"type": "array", "items": field}})),
        object(json!({"type": string_enum(&["draft_reply"]), "quality": quality})),
        object(json!({"type": string_enum(&["create_review"]), "title": {"type": "string"}})),
        object(json!({"type": string_enum(&["remind_if_no_reply"]), "hours": {"type": "integer"}})),
        object(json!({"type": string_enum(&["save_to_gmail_drafts"])})),
        object(json!({"type": string_enum(&["label"]), "value": {"type": "string"}})),
        object(json!({"type": string_enum(&["start_case"])})),
        object(json!({"type": string_enum(&["record_timesheet"])})),
        object(json!({"type": string_enum(&["notify"]), "to": string_enum(NOTIFY_TARGETS)})),
    ]})
}

/// The schema of one `AgentSpec`.
pub fn spec_schema() -> Value {
    object(json!({
        "name": {"type": "string"},
        "key": {"type": "string"},
        "category": string_enum(CATEGORIES),
        "description": {"type": "string"},
        "goal": {"type": "string"},
        "persona": {"type": "string"},
        "mode": string_enum(&["workflow", "agent"]),
        "triggers": {"type": "array", "items": trigger()},
        "steps": {"type": "array", "items": step()},
        "tools": strings(),
        "scopes": {"type": "array", "items": string_enum(&["read_mail", "draft", "send", "label", "schedule"])},
        "permissions": object(json!({
            "draft": {"type": "boolean"}, "send": string_enum(&["never", "ask", "earned"]),
            "max_autonomy": {"type": "integer"}, "daily_send_cap": {"type": "integer"},
        })),
        "memory": object(json!({
            "scope": string_enum(&["own+shared", "own", "shared"]),
            "learn_from": {"type": "array", "items": string_enum(&["edits", "approvals", "labels", "routes"])},
        })),
        "drafting": object(json!({"rules": strings(), "never_do": strings(), "quality_tier": string_enum(&["fast", "balanced", "best"])})),
        "budget": object(json!({"max_steps": {"type": "integer"}, "usd_per_case": {"type": "number"}})),
        "questions_for_user": strings(),
        "assumptions": strings(),
    }))
}

#[cfg(test)]
pub fn conforms(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        return if options.iter().any(|o| conforms(o, value, path).is_ok()) { Ok(()) } else { Err(format!("{path}: no anyOf branch fits {value}")) };
    }
    let types: Vec<&str> = match schema.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    let type_ok = |t: &str| match t {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    };
    if !types.is_empty() && !types.iter().any(|t| type_ok(t)) {
        return Err(format!("{path}: expected {types:?}, got {value}"));
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        return Err(format!("{path}: {value} is not one of {allowed:?}"));
    }
    if let (Some(items), Some(arr)) = (schema.get("items"), value.as_array()) {
        for (i, v) in arr.iter().enumerate() {
            conforms(items, v, &format!("{path}[{i}]"))?;
        }
    }
    if let (Some(props), Some(obj)) = (schema.get("properties").and_then(Value::as_object), value.as_object()) {
        for key in schema.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            if !obj.contains_key(key) {
                return Err(format!("{path}: missing {key}"));
            }
        }
        for (k, v) in obj {
            let Some(p) = props.get(k) else { return Err(format!("{path}: unexpected {k}")) };
            conforms(p, v, &format!("{path}.{k}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::registry::{spec::AgentSpec, templates::TEMPLATES};

    #[test]
    fn every_template_spec_fits_the_schema_and_round_trips() {
        let schema = spec_schema();
        for t in TEMPLATES {
            let value = serde_json::to_value(t.spec()).unwrap();
            conforms(&schema, &value, t.key).unwrap_or_else(|e| panic!("{e}"));
            let back: AgentSpec = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(back).unwrap(), value, "{}", t.key);
        }
    }

    #[test]
    fn the_schema_is_strict_everywhere() {
        fn walk(v: &Value, path: &str) {
            if let Some(props) = v.get("properties").and_then(Value::as_object) {
                assert_eq!(v["additionalProperties"], false, "{path}");
                let required: Vec<&str> = v["required"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
                assert_eq!(required.len(), props.len(), "{path}: every property is required");
                props.iter().for_each(|(k, p)| walk(p, &format!("{path}.{k}")));
            }
            if let Some(items) = v.get("items") {
                walk(items, &format!("{path}[]"));
            }
            for option in v.get("anyOf").and_then(Value::as_array).into_iter().flatten() {
                walk(option, path);
            }
        }
        walk(&spec_schema(), "spec");
    }

    #[test]
    fn the_schema_rejects_what_a_spec_cannot_hold() {
        let schema = spec_schema();
        let mut v = serde_json::to_value(TEMPLATES[0].spec()).unwrap();
        v["steps"] = json!([{"type": "send_reply"}]);
        assert!(conforms(&schema, &v, "spec").is_err());
        let mut v = serde_json::to_value(TEMPLATES[0].spec()).unwrap();
        v["permissions"]["send"] = json!("always");
        assert!(conforms(&schema, &v, "spec").is_err());
        let mut v = serde_json::to_value(TEMPLATES[0].spec()).unwrap();
        v["secret"] = json!(1);
        assert!(conforms(&schema, &v, "spec").is_err());
    }

    #[test]
    fn a_spec_missing_any_type_is_rejected_by_the_schema() {
        let schema = spec_schema();
        let mut checked = 0;
        for t in TEMPLATES {
            let base = serde_json::to_value(t.spec()).unwrap();
            for key in ["steps", "triggers"] {
                for i in 0..base[key].as_array().unwrap().len() {
                    let mut v = base.clone();
                    v[key][i].as_object_mut().unwrap().remove("type");
                    assert!(conforms(&schema, &v, "spec").is_err(), "{} {key}[{i}] without type must fail", t.key);
                    checked += 1;
                }
            }
            for (i, step) in base["steps"].as_array().unwrap().iter().enumerate() {
                for j in 0..step["fields"].as_array().map_or(0, Vec::len) {
                    let mut v = base.clone();
                    v["steps"][i]["fields"][j].as_object_mut().unwrap().remove("type");
                    assert!(conforms(&schema, &v, "spec").is_err(), "{} extract field without type must fail", t.key);
                    checked += 1;
                }
            }
        }
        assert!(checked > 5);
    }
}
