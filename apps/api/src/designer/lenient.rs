//! Reading a model answer that is nearly an `AgentSpec`. A step, trigger or extract field that lost its
//! `type` is given one when its keys point at exactly one variant; every repair is reported so the
//! person sees it as an assumption. Nothing here adds a scope, tool, permission or level.

use serde_json::{Map, Value, json};

use crate::agent_registry::spec::{AgentSpec, Step, Trigger};

type Check = fn(&Value) -> bool;

const STEP_UNIT: [&str; 2] = ["save_to_gmail_drafts", "start_case"];

fn step_type(obj: &Map<String, Value>) -> Option<&'static str> {
    let hits: Vec<&'static str> = [("fields", "extract"), ("quality", "draft_reply"), ("title", "create_review"), ("hours", "remind_if_no_reply")]
        .into_iter()
        .filter(|(k, _)| obj.contains_key(*k))
        .map(|(_, t)| t)
        .collect();
    (hits.len() == 1).then(|| hits[0])
}

fn trigger_type(obj: &Map<String, Value>) -> Option<&'static str> {
    match (obj.contains_key("every_hours"), obj.contains_key("match")) {
        (true, false) => Some("schedule"),
        (false, true) => Some("email_received"),
        _ => None,
    }
}

fn is_blank(v: Option<&Value>) -> bool {
    v.is_none_or(|t| t.as_str().is_some_and(|s| s.trim().is_empty()))
}

fn fix_list(list: Option<&mut Value>, label: &str, infer: fn(&Map<String, Value>) -> Option<&'static str>, units: &[&str], notes: &mut Vec<String>) {
    let Some(items) = list.and_then(Value::as_array_mut) else { return };
    for (i, item) in items.iter_mut().enumerate() {
        if let Some(name) = item.as_str().filter(|s| units.contains(s)) {
            let t = name.to_string();
            notes.push(format!("{label} {} had no type; I read it as \"{t}\".", i + 1));
            *item = json!({"type": t});
            continue;
        }
        let Some(obj) = item.as_object_mut() else { continue };
        if !is_blank(obj.get("type")) {
            continue;
        }
        if let Some(t) = infer(obj) {
            obj.insert("type".into(), json!(t));
            notes.push(format!("{label} {} had no type; its fields point to \"{t}\", so I read it that way.", i + 1));
        }
    }
}

/// Fills in what is safe to fill in and returns one note per repair.
pub fn infer_missing(value: &mut Value) -> Vec<String> {
    let mut notes = Vec::new();
    fix_list(value.get_mut("steps"), "Step", step_type, &STEP_UNIT, &mut notes);
    fix_list(value.get_mut("triggers"), "Trigger", trigger_type, &["manual"], &mut notes);
    if let Some(steps) = value.get_mut("steps").and_then(Value::as_array_mut) {
        for (i, step) in steps.iter_mut().enumerate() {
            let Some(fields) = step.get_mut("fields").and_then(Value::as_array_mut) else { continue };
            for field in fields.iter_mut().filter_map(Value::as_object_mut) {
                if is_blank(field.get("type")) && field.contains_key("name") {
                    field.insert("type".into(), json!("string"));
                    let name = field.get("name").and_then(Value::as_str).unwrap_or("a field").to_string();
                    notes.push(format!("The field \"{name}\" in step {} had no type; I made it text.", i + 1));
                }
            }
        }
    }
    notes
}

/// The first trigger or step that does not fit, as a path and the fragment itself.
pub fn locate(value: &Value) -> Option<(String, String)> {
    let checks: [(&str, Check); 2] = [
        ("triggers", |v| serde_json::from_value::<Trigger>(v.clone()).is_ok()),
        ("steps", |v| serde_json::from_value::<Step>(v.clone()).is_ok()),
    ];
    for (key, ok) in checks {
        for (i, item) in value.get(key).and_then(Value::as_array).into_iter().flatten().enumerate() {
            if !ok(item) {
                return Some((format!("{key}[{i}]"), crate::agents::sentence(&item.to_string(), 400)));
            }
        }
    }
    None
}

/// What went wrong reading an answer.
#[derive(Debug)]
pub struct ReadError {
    pub path: String,
    pub error: String,
    pub fragment: String,
}

/// Reads the answer: drops text around the JSON object, repairs missing types, then parses strictly.
pub fn parse(raw: &str) -> Result<(AgentSpec, Vec<String>), ReadError> {
    let text = raw.trim();
    let text = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if b > a => &text[a..=b],
        _ => text,
    };
    let mut value: Value = serde_json::from_str(text).map_err(|e| ReadError { path: String::new(), error: e.to_string(), fragment: String::new() })?;
    let notes = infer_missing(&mut value);
    serde_json::from_value::<AgentSpec>(value.clone()).map(|spec| (spec, notes)).map_err(|e| {
        let (path, fragment) = locate(&value).unwrap_or_default();
        ReadError { path, error: e.to_string(), fragment }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_pick_the_variant_only_when_they_agree_on_one() {
        let mut v = json!({"steps": [{"hours": 48}, {"title": "x", "hours": 1}, {"fields": [{"name": "a"}]}, "start_case"], "triggers": [{"every_hours": 3}, {}]});
        let notes = infer_missing(&mut v);
        assert_eq!(v["steps"][0]["type"], "remind_if_no_reply");
        assert!(v["steps"][1].get("type").is_none(), "two candidates: leave it for the model");
        assert_eq!(v["steps"][2]["type"], "extract");
        assert_eq!(v["steps"][2]["fields"][0]["type"], "string");
        assert_eq!(v["steps"][3]["type"], "start_case");
        assert_eq!(v["triggers"][0]["type"], "schedule");
        assert!(v["triggers"][1].get("type").is_none());
        assert_eq!(notes.len(), 5);
    }
}
