//! Minimal Atlassian Document Format (ADF) helpers (port of `issues/adf.ts`).
//! We only (a) wrap plain user text into a valid ADF doc for writes, and (b)
//! flatten arbitrary ADF read from Jira into plain text for cards/previews. We
//! never render Jira ADF/HTML as trusted markup.

use serde_json::{json, Value};

fn paragraph(line: &str) -> Value {
    if line.is_empty() {
        json!({ "type": "paragraph", "content": [] })
    } else {
        json!({ "type": "paragraph", "content": [{ "type": "text", "text": line }] })
    }
}

/// Wrap plain text into an ADF doc (one paragraph per line).
pub fn text_to_adf(text: &str) -> Value {
    let content: Vec<Value> = text.split('\n').map(paragraph).collect();
    json!({ "type": "doc", "version": 1, "content": content })
}

const BLOCK_TYPES: [&str; 2] = ["paragraph", "heading"];

fn walk(node: &Value, acc: &mut String) {
    let node_type = node.get("type").and_then(Value::as_str);
    if node_type == Some("text") {
        if let Some(text) = node.get("text").and_then(Value::as_str) {
            acc.push_str(text);
        }
    }
    if let Some(children) = node.get("content").and_then(Value::as_array) {
        for child in children {
            walk(child, acc);
        }
    }
    if node_type.map(|t| BLOCK_TYPES.contains(&t)).unwrap_or(false) {
        acc.push('\n');
    }
}

/// Flatten arbitrary ADF to plain text (port of `adfToText`). Null/absent → "".
pub fn adf_to_text(node: Option<&Value>) -> String {
    let Some(node) = node.filter(|v| !v.is_null()) else {
        return String::new();
    };
    let mut acc = String::new();
    walk(node, &mut acc);
    if acc.ends_with('\n') {
        acc.pop();
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_to_adf_wraps_single_line() {
        assert_eq!(
            text_to_adf("hello"),
            json!({ "type": "doc", "version": 1, "content": [
                { "type": "paragraph", "content": [{ "type": "text", "text": "hello" }] }
            ] })
        );
    }

    #[test]
    fn text_to_adf_one_paragraph_per_line() {
        let doc = text_to_adf("a\nb");
        let content = doc["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[1], json!({ "type": "paragraph", "content": [{ "type": "text", "text": "b" }] }));
    }

    #[test]
    fn text_to_adf_empty_is_empty_paragraph() {
        assert_eq!(
            text_to_adf(""),
            json!({ "type": "doc", "version": 1, "content": [{ "type": "paragraph", "content": [] }] })
        );
    }

    #[test]
    fn adf_round_trips_single_paragraph() {
        assert_eq!(adf_to_text(Some(&text_to_adf("hello"))), "hello");
    }

    #[test]
    fn adf_joins_paragraphs_with_newlines() {
        assert_eq!(adf_to_text(Some(&text_to_adf("a\nb"))), "a\nb");
    }

    #[test]
    fn adf_null_is_empty() {
        assert_eq!(adf_to_text(None), "");
        assert_eq!(adf_to_text(Some(&Value::Null)), "");
    }

    #[test]
    fn adf_flattens_nested_list() {
        let list_doc = json!({
            "type": "doc", "version": 1,
            "content": [{ "type": "bulletList", "content": [
                { "type": "listItem", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "x" }] }] },
                { "type": "listItem", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "y" }] }] }
            ] }]
        });
        assert_eq!(adf_to_text(Some(&list_doc)), "x\ny");
    }

    #[test]
    fn adf_concatenates_sibling_text() {
        let sibling_doc = json!({
            "type": "doc", "version": 1,
            "content": [{ "type": "paragraph", "content": [
                { "type": "text", "text": "foo" }, { "type": "text", "text": "bar" }
            ] }]
        });
        assert_eq!(adf_to_text(Some(&sibling_doc)), "foobar");
    }
}
