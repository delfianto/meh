//! Incremental JSON parser for streaming tool call arguments.
//!
//! Tool call arguments arrive as partial JSON fragments over the SSE
//! stream. This module tracks multiple in-flight tool calls by ID,
//! accumulates fragments, and attempts parsing after each append
//! for early preview.

use std::collections::HashMap;

/// Tracks the state of a partially-received tool call.
#[derive(Debug)]
pub struct PartialToolCall {
    pub id: String,
    pub name: String,
    accumulated_json: String,
    pub complete: bool,
    pub parsed_args: Option<serde_json::Value>,
}

impl PartialToolCall {
    /// Creates a new partial tool call tracker.
    pub const fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            accumulated_json: String::new(),
            complete: false,
            parsed_args: None,
        }
    }

    /// Appends a JSON delta fragment and optimistically parses.
    pub fn append(&mut self, delta: &str) {
        self.accumulated_json.push_str(delta);
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&self.accumulated_json) {
            self.parsed_args = Some(value);
        }
    }

    /// Marks as complete and performs final parse.
    pub fn finalize(&mut self) -> anyhow::Result<serde_json::Value> {
        self.complete = true;
        serde_json::from_str(&self.accumulated_json)
            .map_err(|e| anyhow::anyhow!("Failed to parse tool arguments: {e}"))
    }

    /// Extracts partial fields from incomplete JSON using regex (best-effort for UI preview).
    pub fn partial_fields(&self) -> HashMap<String, String> {
        extract_partial_json_fields(&self.accumulated_json)
    }

    /// Returns the raw accumulated JSON string.
    pub fn raw_json(&self) -> &str {
        &self.accumulated_json
    }
}

/// Extracts key-value pairs from potentially incomplete JSON.
///
/// Uses `jiter` with `PartialMode::TrailingStrings` for proper incremental
/// parsing. Handles truncated strings, booleans, numbers, nested objects,
/// and escaped quotes natively — no string suffix guessing needed.
pub fn extract_partial_json_fields(partial: &str) -> HashMap<String, String> {
    let Ok(value) = jiter::JsonValue::parse_owned(
        partial.as_bytes(),
        false,
        jiter::PartialMode::TrailingStrings,
    ) else {
        return HashMap::new();
    };

    let jiter::JsonValue::Object(map) = value else {
        return HashMap::new();
    };

    map.iter()
        .map(|(k, v)| (k.to_string(), jiter_value_to_preview(v)))
        .collect()
}

/// Convert a jiter `JsonValue` to a short preview string.
fn jiter_value_to_preview(v: &jiter::JsonValue<'_>) -> String {
    match v {
        jiter::JsonValue::Str(s) => s.to_string(),
        jiter::JsonValue::Int(n) => n.to_string(),
        jiter::JsonValue::Float(n) => n.to_string(),
        jiter::JsonValue::Bool(b) => b.to_string(),
        jiter::JsonValue::Null => "null".to_string(),
        jiter::JsonValue::Array(_) => "[...]".to_string(),
        jiter::JsonValue::Object(_) => "{...}".to_string(),
        jiter::JsonValue::BigInt(n) => n.to_string(),
    }
}

/// Manages all in-flight tool calls during a streaming response.
pub struct ToolCallTracker {
    active: HashMap<String, PartialToolCall>,
}

impl ToolCallTracker {
    /// Creates a new empty tracker.
    pub fn new() -> Self {
        Self {
            active: HashMap::new(),
        }
    }

    /// Starts tracking a new tool call.
    pub fn start_tool_call(&mut self, id: String, name: String) {
        self.active
            .insert(id.clone(), PartialToolCall::new(id, name));
    }

    /// Appends a JSON delta to a tracked tool call.
    pub fn append_delta(&mut self, id: &str, delta: &str) -> Option<&PartialToolCall> {
        let tc = self.active.get_mut(id)?;
        tc.append(delta);
        Some(tc)
    }

    /// Finalizes a tool call, removes it from tracking, and returns parsed result.
    pub fn finalize(
        &mut self,
        id: &str,
    ) -> Option<anyhow::Result<(String, String, serde_json::Value)>> {
        self.active
            .remove(id)
            .map(|mut tc| tc.finalize().map(|args| (tc.id, tc.name, args)))
    }

    /// Gets a reference to a tracked tool call by ID.
    pub fn get(&self, id: &str) -> Option<&PartialToolCall> {
        self.active.get(id)
    }

    /// Clears all tracked tool calls.
    pub fn clear(&mut self) {
        self.active.clear();
    }

    /// Returns the number of active tool calls.
    pub fn len(&self) -> usize {
        self.active.len()
    }

    /// Returns whether there are no active tool calls.
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_tool_call_new() {
        let tc = PartialToolCall::new("tc1".to_string(), "read_file".to_string());
        assert_eq!(tc.id, "tc1");
        assert_eq!(tc.name, "read_file");
        assert!(!tc.complete);
        assert!(tc.parsed_args.is_none());
    }

    #[test]
    fn partial_tool_call_accumulation() {
        let mut tc = PartialToolCall::new("tc1".to_string(), "read_file".to_string());
        tc.append(r#"{"pa"#);
        assert!(tc.parsed_args.is_none());
        tc.append(r#"th": "/src/main.rs"}"#);
        assert!(tc.parsed_args.is_some());
        let result = tc.finalize().unwrap();
        assert_eq!(result["path"], "/src/main.rs");
        assert!(tc.complete);
    }

    #[test]
    fn partial_tool_call_complex_json() {
        let mut tc = PartialToolCall::new("tc2".to_string(), "write_file".to_string());
        tc.append(r#"{"path": "/test.rs", "#);
        tc.append(r#""content": "fn main() {}\n", "#);
        tc.append(r#""create_dirs": true}"#);
        let result = tc.finalize().unwrap();
        assert_eq!(result["path"], "/test.rs");
        assert_eq!(result["content"], "fn main() {}\n");
        assert_eq!(result["create_dirs"], true);
    }

    #[test]
    fn partial_tool_call_invalid_json() {
        let mut tc = PartialToolCall::new("tc1".to_string(), "read_file".to_string());
        tc.append(r#"{"broken"#);
        assert!(tc.finalize().is_err());
    }

    #[test]
    fn partial_tool_call_empty() {
        let mut tc = PartialToolCall::new("tc1".to_string(), "test".to_string());
        assert!(tc.finalize().is_err());
    }

    #[test]
    fn extract_partial_fields_complete() {
        let partial = r#"{"path": "/src/main.rs", "content": "hello"}"#;
        let fields = extract_partial_json_fields(partial);
        assert_eq!(fields.get("path").unwrap(), "/src/main.rs");
        assert_eq!(fields.get("content").unwrap(), "hello");
    }

    #[test]
    fn extract_partial_fields_truncated_at_comma() {
        let partial = r#"{"path": "/src/main.rs","#;
        let fields = extract_partial_json_fields(partial);
        assert_eq!(fields.get("path").unwrap(), "/src/main.rs");
    }

    #[test]
    fn extract_partial_fields_empty() {
        let fields = extract_partial_json_fields("");
        assert!(fields.is_empty());
    }

    #[test]
    fn extract_partial_fields_booleans_and_numbers() {
        let partial = r#"{"count": 42, "flag": true, "name": "test"}"#;
        let fields = extract_partial_json_fields(partial);
        assert_eq!(fields.get("count").unwrap(), "42");
        assert_eq!(fields.get("flag").unwrap(), "true");
        assert_eq!(fields.get("name").unwrap(), "test");
    }

    #[test]
    fn extract_partial_fields_escaped_quotes() {
        let partial = r#"{"content": "println!(\"Hello\");"}"#;
        let fields = extract_partial_json_fields(partial);
        assert!(fields.get("content").unwrap().contains("Hello"));
    }

    #[test]
    fn extract_partial_fields_truncated_mid_string() {
        let partial = r#"{"path": "/src/ma"#;
        let fields = extract_partial_json_fields(partial);
        assert_eq!(fields.get("path").unwrap(), "/src/ma");
    }

    #[test]
    fn extract_partial_fields_null_value() {
        let partial = r#"{"path": "/test", "extra": null}"#;
        let fields = extract_partial_json_fields(partial);
        assert_eq!(fields.get("path").unwrap(), "/test");
        assert_eq!(fields.get("extra").unwrap(), "null");
    }

    #[test]
    fn extract_partial_fields_nested_object() {
        let partial = r#"{"path": "/test", "options": {"recursive": true}}"#;
        let fields = extract_partial_json_fields(partial);
        assert_eq!(fields.get("path").unwrap(), "/test");
        assert_eq!(fields.get("options").unwrap(), "{...}");
    }

    #[test]
    fn partial_fields_method() {
        let mut tc = PartialToolCall::new("tc1".to_string(), "read_file".to_string());
        tc.append(r#"{"path": "/src/main.rs","#);
        let fields = tc.partial_fields();
        assert_eq!(fields.get("path").unwrap(), "/src/main.rs");
    }

    #[test]
    fn tracker_lifecycle() {
        let mut tracker = ToolCallTracker::new();
        assert!(tracker.is_empty());

        tracker.start_tool_call("t1".to_string(), "read_file".to_string());
        assert_eq!(tracker.len(), 1);
        assert!(!tracker.is_empty());

        tracker.append_delta("t1", r#"{"path":""#);
        tracker.append_delta("t1", r#"/main.rs"}"#);

        let result = tracker.finalize("t1").unwrap().unwrap();
        assert_eq!(result.0, "t1");
        assert_eq!(result.1, "read_file");
        assert_eq!(result.2["path"], "/main.rs");

        assert!(tracker.is_empty());
    }

    #[test]
    fn tracker_unknown_id() {
        let mut tracker = ToolCallTracker::new();
        assert!(tracker.append_delta("unknown", "data").is_none());
        assert!(tracker.finalize("unknown").is_none());
        assert!(tracker.get("unknown").is_none());
    }

    #[test]
    fn tracker_multiple() {
        let mut tracker = ToolCallTracker::new();
        tracker.start_tool_call("t1".to_string(), "read_file".to_string());
        tracker.start_tool_call("t2".to_string(), "write_file".to_string());
        assert_eq!(tracker.len(), 2);

        tracker.append_delta("t1", r#"{"path": "/a.rs"}"#);
        tracker.append_delta("t2", r#"{"path": "/b.rs", "content": "test"}"#);

        let r1 = tracker.finalize("t1").unwrap().unwrap();
        assert_eq!(r1.2["path"], "/a.rs");

        let r2 = tracker.finalize("t2").unwrap().unwrap();
        assert_eq!(r2.2["path"], "/b.rs");
    }

    #[test]
    fn tracker_clear() {
        let mut tracker = ToolCallTracker::new();
        tracker.start_tool_call("t1".to_string(), "test".to_string());
        tracker.clear();
        assert!(tracker.is_empty());
    }
}
