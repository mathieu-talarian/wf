//! Metadata-driven validation/coercion for issue create & edit (port of
//! `issues/fields.ts`). The server NEVER forwards arbitrary client `fields` to
//! Jira: every field must appear in the relevant createmeta/editmeta, is coerced
//! to the shape Jira's schema expects, and the payload is capped. This is the
//! write-path security boundary.

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use super::adf::text_to_adf;

#[derive(Debug, Clone)]
pub struct JiraFieldSchema {
    pub r#type: String,
    pub items: Option<String>,
    pub system: Option<String>,
    pub custom: Option<String>,
}

#[derive(Debug, Clone)]
pub struct JiraAllowedValue {
    pub id: Option<String>,
    pub value: Option<String>,
}

#[derive(Debug, Clone)]
pub struct JiraFieldMeta {
    pub field_id: String,
    pub required: bool,
    pub schema: JiraFieldSchema,
    pub allowed_values: Vec<JiraAllowedValue>,
}

pub type FieldMetaMap = HashMap<String, JiraFieldMeta>;

#[derive(Debug, Clone, Default)]
pub struct BuildFieldsOpts {
    pub enforce_required: bool,
    pub max_fields: Option<usize>,
    pub max_text_length: Option<usize>,
}

const DEFAULT_MAX_FIELDS: usize = 50;
const DEFAULT_MAX_TEXT: usize = 32_768;

const ADF_SYSTEMS: [&str; 2] = ["description", "environment"];
const ID_REF_TYPES: [&str; 8] = [
    "option", "priority", "issuetype", "resolution", "version", "component", "securitylevel",
    "project",
];

pub fn normalize_create_meta_fields(fields: Vec<JiraFieldMeta>) -> FieldMetaMap {
    fields.into_iter().map(|f| (f.field_id.clone(), f)).collect()
}

fn is_adf_field(s: &JiraFieldSchema) -> bool {
    s.r#type == "string"
        && (ADF_SYSTEMS.contains(&s.system.as_deref().unwrap_or(""))
            || s.custom.as_deref().unwrap_or("").contains("textarea"))
}

fn find_allowed<'a>(meta: &'a JiraFieldMeta, key: &str) -> Option<&'a JiraAllowedValue> {
    meta.allowed_values
        .iter()
        .find(|v| v.id.as_deref() == Some(key) || v.value.as_deref() == Some(key))
}

fn coerce_option(meta: &JiraFieldMeta, value: &Value) -> Result<Value, String> {
    let Some(key) = value.as_str() else {
        return Err(format!("{} expects an id", meta.field_id));
    };
    if meta.allowed_values.is_empty() {
        return Ok(json!({ "id": key }));
    }
    let Some(found) = find_allowed(meta, key) else {
        return Err(format!("{}: value not allowed", meta.field_id));
    };
    Ok(match (&found.id, &found.value) {
        (Some(id), _) => json!({ "id": id }),
        (None, Some(v)) => json!({ "value": v }),
        (None, None) => json!({}),
    })
}

fn coerce_text(meta: &JiraFieldMeta, value: &Value, max_text: usize) -> Result<Value, String> {
    let Some(s) = value.as_str() else {
        return Err(format!("{} expects text", meta.field_id));
    };
    if s.chars().count() > max_text {
        return Err(format!("{}: text too long", meta.field_id));
    }
    Ok(if is_adf_field(&meta.schema) { text_to_adf(s) } else { Value::String(s.to_string()) })
}

fn coerce_number(meta: &JiraFieldMeta, value: &Value) -> Result<Value, String> {
    if value.is_number() {
        Ok(value.clone())
    } else {
        Err(format!("{} expects a number", meta.field_id))
    }
}

fn coerce_user(meta: &JiraFieldMeta, value: &Value) -> Result<Value, String> {
    match value.as_str() {
        Some(s) => Ok(json!({ "accountId": s })),
        None => Err(format!("{} expects an accountId", meta.field_id)),
    }
}

fn coerce_array_item(meta: &JiraFieldMeta, items: &str, item: &Value) -> Result<Value, String> {
    if items == "string" {
        return match item.as_str() {
            Some(s) => Ok(Value::String(s.to_string())),
            None => Err(format!("{}: expects strings", meta.field_id)),
        };
    }
    if ID_REF_TYPES.contains(&items) {
        return coerce_option(meta, item);
    }
    if items == "user" {
        return coerce_user(meta, item);
    }
    Err(format!("{}: unsupported array item {items}", meta.field_id))
}

fn coerce_array(meta: &JiraFieldMeta, value: &Value) -> Result<Value, String> {
    let Some(arr) = value.as_array() else {
        return Err(format!("{} expects an array", meta.field_id));
    };
    let items = meta.schema.items.as_deref().unwrap_or("string");
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        out.push(coerce_array_item(meta, items, item)?);
    }
    Ok(Value::Array(out))
}

fn coerce_scalar(meta: &JiraFieldMeta, value: &Value, max_text: usize) -> Result<Value, String> {
    let t = meta.schema.r#type.as_str();
    if is_adf_field(&meta.schema) || matches!(t, "string" | "date" | "datetime") {
        return coerce_text(meta, value, max_text);
    }
    if t == "number" {
        return coerce_number(meta, value);
    }
    if t == "user" {
        return coerce_user(meta, value);
    }
    if ID_REF_TYPES.contains(&t) {
        return coerce_option(meta, value);
    }
    Err(format!("{}: unsupported field type {t}", meta.field_id))
}

fn coerce_field(meta: &JiraFieldMeta, value: &Value, max_text: usize) -> Result<Value, String> {
    if meta.schema.r#type == "array" {
        coerce_array(meta, value)
    } else {
        coerce_scalar(meta, value, max_text)
    }
}

fn missing_required(meta: &FieldMetaMap, input: &Map<String, Value>) -> Option<String> {
    meta.values()
        .find(|f| f.required && !input.contains_key(&f.field_id))
        .map(|f| format!("{} is required", f.field_id))
}

fn coerce_all(
    meta: &FieldMetaMap,
    input: &Map<String, Value>,
    max_text: usize,
) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    for (key, value) in input {
        let Some(m) = meta.get(key) else {
            return Err(format!("Unknown field: {key}"));
        };
        out.insert(key.clone(), coerce_field(m, value, max_text)?);
    }
    Ok(out)
}

/// Validate + coerce client-supplied fields against the field metadata (port of
/// `buildIssueFields`). Returns the Jira-shaped field map or a rejection reason.
pub fn build_issue_fields(
    meta: &FieldMetaMap,
    input: &Map<String, Value>,
    opts: &BuildFieldsOpts,
) -> Result<Map<String, Value>, String> {
    if input.len() > opts.max_fields.unwrap_or(DEFAULT_MAX_FIELDS) {
        return Err("Too many fields.".to_string());
    }
    if opts.enforce_required {
        if let Some(missing) = missing_required(meta, input) {
            return Err(missing);
        }
    }
    coerce_all(meta, input, opts.max_text_length.unwrap_or(DEFAULT_MAX_TEXT))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(t: &str) -> JiraFieldSchema {
        JiraFieldSchema { r#type: t.to_string(), items: None, system: None, custom: None }
    }

    fn meta(field_id: &str, schema: JiraFieldSchema) -> JiraFieldMeta {
        JiraFieldMeta { field_id: field_id.to_string(), required: false, schema, allowed_values: vec![] }
    }

    fn map(entries: Vec<JiraFieldMeta>) -> FieldMetaMap {
        normalize_create_meta_fields(entries)
    }

    fn input(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn opts() -> BuildFieldsOpts {
        BuildFieldsOpts::default()
    }

    fn adf(text: &str) -> Value {
        json!({ "type": "doc", "version": 1, "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": text }] }] })
    }

    #[test]
    fn rejects_unknown_field() {
        assert!(build_issue_fields(&map(vec![]), &input(&[("summary", json!("x"))]), &opts()).is_err());
    }

    #[test]
    fn keeps_summary_as_plain_string() {
        let m = map(vec![meta("summary", JiraFieldSchema { system: Some("summary".into()), ..schema("string") })]);
        let out = build_issue_fields(&m, &input(&[("summary", json!("Fix bug"))]), &opts()).unwrap();
        assert_eq!(out.get("summary"), Some(&json!("Fix bug")));
    }

    #[test]
    fn coerces_description_to_adf() {
        let m = map(vec![meta("description", JiraFieldSchema { system: Some("description".into()), ..schema("string") })]);
        let out = build_issue_fields(&m, &input(&[("description", json!("hello"))]), &opts()).unwrap();
        assert_eq!(out.get("description"), Some(&adf("hello")));
    }

    #[test]
    fn coerces_textarea_custom_to_adf() {
        let m = map(vec![meta("customfield_1", JiraFieldSchema { custom: Some("com.x:textarea".into()), ..schema("string") })]);
        let out = build_issue_fields(&m, &input(&[("customfield_1", json!("note"))]), &opts()).unwrap();
        assert_eq!(out.get("customfield_1"), Some(&adf("note")));
    }

    #[test]
    fn maps_allowed_option_to_id_ref() {
        let mut pri = meta("priority", schema("priority"));
        pri.allowed_values = vec![JiraAllowedValue { id: Some("3".into()), value: None }];
        let out = build_issue_fields(&map(vec![pri]), &input(&[("priority", json!("3"))]), &opts()).unwrap();
        assert_eq!(out.get("priority"), Some(&json!({ "id": "3" })));
    }

    #[test]
    fn rejects_option_not_in_allowed() {
        let mut pri = meta("priority", schema("priority"));
        pri.allowed_values = vec![JiraAllowedValue { id: Some("3".into()), value: None }];
        assert!(build_issue_fields(&map(vec![pri]), &input(&[("priority", json!("9"))]), &opts()).is_err());
    }

    #[test]
    fn coerces_array_of_options() {
        let mut comp = meta("components", JiraFieldSchema { items: Some("component".into()), ..schema("array") });
        comp.allowed_values = vec![JiraAllowedValue { id: Some("100".into()), value: None }];
        let out = build_issue_fields(&map(vec![comp]), &input(&[("components", json!(["100"]))]), &opts()).unwrap();
        assert_eq!(out.get("components"), Some(&json!([{ "id": "100" }])));
    }

    #[test]
    fn passes_labels_string_array_through() {
        let m = map(vec![meta("labels", JiraFieldSchema { items: Some("string".into()), ..schema("array") })]);
        let out = build_issue_fields(&m, &input(&[("labels", json!(["a", "b"]))]), &opts()).unwrap();
        assert_eq!(out.get("labels"), Some(&json!(["a", "b"])));
    }

    #[test]
    fn coerces_user_to_account_id() {
        let m = map(vec![meta("assignee", JiraFieldSchema { system: Some("assignee".into()), ..schema("user") })]);
        let out = build_issue_fields(&m, &input(&[("assignee", json!("5b10"))]), &opts()).unwrap();
        assert_eq!(out.get("assignee"), Some(&json!({ "accountId": "5b10" })));
    }

    #[test]
    fn passes_number_through() {
        let m = map(vec![meta("customfield_2", schema("number"))]);
        let out = build_issue_fields(&m, &input(&[("customfield_2", json!(5))]), &opts()).unwrap();
        assert_eq!(out.get("customfield_2"), Some(&json!(5)));
    }

    #[test]
    fn rejects_unsupported_schema_type() {
        let m = map(vec![meta("weird", schema("mystery-widget"))]);
        assert!(build_issue_fields(&m, &input(&[("weird", json!("x"))]), &opts()).is_err());
    }

    #[test]
    fn rejects_when_field_cap_exceeded() {
        let m = map(vec![
            meta("summary", JiraFieldSchema { system: Some("summary".into()), ..schema("string") }),
            meta("environment", JiraFieldSchema { system: Some("environment".into()), ..schema("string") }),
        ]);
        let o = BuildFieldsOpts { max_fields: Some(1), ..Default::default() };
        assert!(build_issue_fields(&m, &input(&[("summary", json!("a")), ("environment", json!("b"))]), &o).is_err());
    }

    #[test]
    fn rejects_when_text_exceeds_cap() {
        let m = map(vec![meta("summary", JiraFieldSchema { system: Some("summary".into()), ..schema("string") })]);
        let o = BuildFieldsOpts { max_text_length: Some(3), ..Default::default() };
        assert!(build_issue_fields(&m, &input(&[("summary", json!("hello"))]), &o).is_err());
    }

    #[test]
    fn rejects_missing_required_when_enforced() {
        let mut s = meta("summary", JiraFieldSchema { system: Some("summary".into()), ..schema("string") });
        s.required = true;
        let o = BuildFieldsOpts { enforce_required: true, ..Default::default() };
        assert!(build_issue_fields(&map(vec![s]), &input(&[]), &o).is_err());
    }

    #[test]
    fn accepts_present_required_when_enforced() {
        let mut s = meta("summary", JiraFieldSchema { system: Some("summary".into()), ..schema("string") });
        s.required = true;
        let o = BuildFieldsOpts { enforce_required: true, ..Default::default() };
        assert!(build_issue_fields(&map(vec![s]), &input(&[("summary", json!("x"))]), &o).is_ok());
    }

    #[test]
    fn normalize_create_keys_by_field_id() {
        let out = normalize_create_meta_fields(vec![meta(
            "summary",
            JiraFieldSchema { system: Some("summary".into()), ..schema("string") },
        )]);
        assert_eq!(out.get("summary").unwrap().field_id, "summary");
    }
}
