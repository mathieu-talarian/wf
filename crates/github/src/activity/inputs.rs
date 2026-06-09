//! `workflow_dispatch` input parsing (port of `workflows/inputs.ts`). Fetches the
//! workflow YAML via the Contents API and extracts the dispatchable flag + input
//! specs. Parsing is permissive: a malformed file degrades to "not dispatchable,
//! no inputs" rather than erroring.

use reqwest::Method;
use serde::Deserialize;
use serde_yaml::Value;

use super::types::{GithubWorkflowInput, GithubWorkflowInputType, GithubWorkflowInputs};
use crate::client::{GithubClient, RepoRef};
use crate::errors::GithubError;

fn empty() -> GithubWorkflowInputs {
    GithubWorkflowInputs { dispatchable: false, inputs: vec![] }
}

/// YAML 1.2 keeps `on` as a string key; guard against a 1.1 parser coercing it
/// to the boolean `true`.
fn read_on(doc: &serde_yaml::Mapping) -> Option<&Value> {
    doc.get("on").or_else(|| doc.get(Value::Bool(true)))
}

struct DispatchInfo<'a> {
    dispatchable: bool,
    inputs: Option<&'a serde_yaml::Mapping>,
}

fn inputs_of(workflow_dispatch: &Value) -> Option<&serde_yaml::Mapping> {
    workflow_dispatch.as_mapping().and_then(|wd| wd.get("inputs")).and_then(Value::as_mapping)
}

fn dispatch_info(on: Option<&Value>) -> DispatchInfo<'_> {
    match on {
        Some(Value::String(s)) if s == "workflow_dispatch" => {
            DispatchInfo { dispatchable: true, inputs: None }
        }
        Some(Value::Sequence(seq)) => DispatchInfo {
            dispatchable: seq.iter().any(|v| v.as_str() == Some("workflow_dispatch")),
            inputs: None,
        },
        Some(Value::Mapping(map)) => match map.get("workflow_dispatch") {
            Some(wd) => DispatchInfo { dispatchable: true, inputs: inputs_of(wd) },
            None => DispatchInfo { dispatchable: false, inputs: None },
        },
        _ => DispatchInfo { dispatchable: false, inputs: None },
    }
}

fn to_type(raw: Option<&Value>) -> GithubWorkflowInputType {
    match raw.and_then(Value::as_str) {
        Some("boolean") => GithubWorkflowInputType::Boolean,
        Some("choice") => GithubWorkflowInputType::Choice,
        Some("number") => GithubWorkflowInputType::Number,
        Some("environment") => GithubWorkflowInputType::Environment,
        _ => GithubWorkflowInputType::String,
    }
}

fn to_default(raw: Option<&Value>) -> Option<String> {
    match raw {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

fn to_options(raw: Option<&Value>) -> Vec<String> {
    raw.and_then(Value::as_sequence)
        .map(|seq| seq.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn to_input(name: &str, spec: &Value) -> GithubWorkflowInput {
    let empty_map = serde_yaml::Mapping::new();
    let s = spec.as_mapping().unwrap_or(&empty_map);
    GithubWorkflowInput {
        name: name.to_string(),
        description: s.get("description").and_then(Value::as_str).map(String::from),
        required: s.get("required").and_then(Value::as_bool) == Some(true),
        r#type: to_type(s.get("type")),
        default: to_default(s.get("default")),
        options: to_options(s.get("options")),
    }
}

/// Extract dispatchable + input specs from workflow YAML (port of
/// `parseWorkflowInputs`).
pub fn parse_workflow_inputs(text: &str) -> GithubWorkflowInputs {
    let Ok(Value::Mapping(doc)) = serde_yaml::from_str::<Value>(text) else {
        return empty();
    };
    let info = dispatch_info(read_on(&doc));
    let inputs = match info.inputs {
        None => vec![],
        Some(map) => map
            .iter()
            .filter_map(|(k, v)| k.as_str().map(|name| to_input(name, v)))
            .collect(),
    };
    GithubWorkflowInputs { dispatchable: info.dispatchable, inputs }
}

#[derive(Deserialize)]
struct ApiContent {
    content: Option<String>,
}

/// GitHub returns base64 with embedded newlines; strip whitespace before decode.
fn decode_content(content: &str) -> Option<String> {
    use base64::Engine;
    let cleaned: String = content.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD.decode(cleaned).ok()?;
    String::from_utf8(bytes).ok()
}

/// Fetch + parse a workflow file's dispatch inputs (port of `fetchWorkflowInputs`).
pub async fn fetch_workflow_inputs(
    token: &str,
    r: &RepoRef,
    path: &str,
) -> Result<GithubWorkflowInputs, GithubError> {
    let client = GithubClient::new(token);
    // `path` is the file path under the repo; its slashes are part of the route.
    let url_path = format!("/repos/{}/{}/contents/{}", r.owner, r.repo, path);
    let resp =
        client.request(Method::GET, &url_path).send().await.map_err(|e| GithubError::Api(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(GithubError::Api(format!("contents HTTP {}", resp.status().as_u16())));
    }
    let body: ApiContent = resp.json().await.map_err(|e| GithubError::Api(e.to_string()))?;
    let text = body.content.as_deref().and_then(decode_content);
    Ok(match text {
        None => empty(),
        Some(t) => parse_workflow_inputs(&t),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_dispatchable_without_workflow_dispatch() {
        let yaml = "on:\n  push:\n    branches: [main]\n";
        let out = parse_workflow_inputs(yaml);
        assert!(!out.dispatchable);
        assert!(out.inputs.is_empty());
    }

    #[test]
    fn dispatchable_bare_string() {
        let out = parse_workflow_inputs("on: workflow_dispatch\n");
        assert!(out.dispatchable);
        assert!(out.inputs.is_empty());
    }

    #[test]
    fn dispatchable_in_sequence() {
        let out = parse_workflow_inputs("on: [push, workflow_dispatch]\n");
        assert!(out.dispatchable);
    }

    const INPUT_SPECS_YAML: &str = r#"
on:
  workflow_dispatch:
    inputs:
      env:
        description: Target environment
        required: true
        type: choice
        options: [staging, prod]
        default: staging
      verbose:
        type: boolean
        default: true
      count:
        type: number
        default: 3
"#;

    #[test]
    fn parses_input_specs() {
        let out = parse_workflow_inputs(INPUT_SPECS_YAML);
        assert!(out.dispatchable);
        assert_eq!(out.inputs.len(), 3);

        let env = &out.inputs[0];
        assert_eq!(env.name, "env");
        assert_eq!(env.description.as_deref(), Some("Target environment"));
        assert!(env.required);
        assert_eq!(env.r#type, GithubWorkflowInputType::Choice);
        assert_eq!(env.options, vec!["staging", "prod"]);
        assert_eq!(env.default.as_deref(), Some("staging"));

        let verbose = &out.inputs[1];
        assert_eq!(verbose.r#type, GithubWorkflowInputType::Boolean);
        assert_eq!(verbose.default.as_deref(), Some("true"));
        assert!(!verbose.required);

        let count = &out.inputs[2];
        assert_eq!(count.r#type, GithubWorkflowInputType::Number);
        assert_eq!(count.default.as_deref(), Some("3"));
    }

    #[test]
    fn empty_on_non_mapping_yaml() {
        assert!(!parse_workflow_inputs("- just\n- a\n- list\n").dispatchable);
        assert!(!parse_workflow_inputs("").dispatchable);
    }
}
