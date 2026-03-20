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
/// Uses regex to find `"key": "value"` patterns, which works even
/// when the JSON is truncated mid-stream. Only extracts string values.
pub fn extract_partial_json_fields(partial: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    let Ok(re) = regex::Regex::new(r#""(\w+)"\s*:\s*"([^"]*)"?"#) else {
        return fields;
    };
    for cap in re.captures_iter(partial) {
        if let (Some(key), Some(val)) = (cap.get(1), cap.get(2)) {
            fields.insert(key.as_str().to_string(), val.as_str().to_string());
        }
    }
    fields
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
    fn extract_partial_fields_truncated() {
        let partial = r#"{"path": "/src/main.rs", "line"#;
        let fields = extract_partial_json_fields(partial);
        assert_eq!(fields.get("path").unwrap(), "/src/main.rs");
        assert!(!fields.contains_key("line"));
    }

    #[test]
    fn extract_partial_fields_empty() {
        let fields = extract_partial_json_fields("");
        assert!(fields.is_empty());
    }

    #[test]
    fn extract_partial_fields_no_strings() {
        let partial = r#"{"count": 42, "flag": true}"#;
        let fields = extract_partial_json_fields(partial);
        assert!(fields.is_empty());
    }

    #[test]
    fn partial_fields_method() {
        let mut tc = PartialToolCall::new("tc1".to_string(), "read_file".to_string());
        tc.append(r#"{"path": "/src/main.rs", "line"#);
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
