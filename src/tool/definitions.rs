//! Tool schema definitions for system prompt injection.

/// Build the tools section of the system prompt as XML.
/// Used for providers that do not support native tool calling (e.g., some
/// local models via text completion).
pub fn tools_as_xml(tools: &[crate::provider::ToolDefinition]) -> String {
    use std::fmt::Write;
    let mut xml = String::from("<tools>\n");
    for tool in tools {
        let _ = write!(
            xml,
            "<tool name=\"{}\">\n<description>{}</description>\n<parameters>{}</parameters>\n</tool>\n",
            tool.name,
            tool.description,
            serde_json::to_string_pretty(&tool.input_schema).unwrap_or_default(),
        );
    }
    xml.push_str("</tools>");
    xml
}

/// Build a compact one-line-per-tool summary for logging/debugging.
pub fn tools_summary(tools: &[crate::provider::ToolDefinition]) -> String {
    tools
        .iter()
        .map(|t| format!("  - {}: {}", t.name, t.description))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ToolDefinition;

    #[test]
    fn test_tools_as_xml_single() {
        let tools = vec![ToolDefinition {
            name: "test".to_string(),
            description: "A test tool".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        }];
        let xml = tools_as_xml(&tools);
        assert!(xml.starts_with("<tools>"));
        assert!(xml.ends_with("</tools>"));
        assert!(xml.contains("<tool name=\"test\">"));
        assert!(xml.contains("<description>A test tool</description>"));
        assert!(xml.contains("<parameters>"));
    }

    #[test]
    fn test_tools_as_xml_multiple() {
        let tools = vec![
            ToolDefinition {
                name: "read".to_string(),
                description: "Read".to_string(),
                input_schema: serde_json::json!({}),
            },
            ToolDefinition {
                name: "write".to_string(),
                description: "Write".to_string(),
                input_schema: serde_json::json!({}),
            },
        ];
        let xml = tools_as_xml(&tools);
        assert!(xml.contains("<tool name=\"read\">"));
        assert!(xml.contains("<tool name=\"write\">"));
    }

    #[test]
    fn test_tools_as_xml_empty() {
        let xml = tools_as_xml(&[]);
        assert_eq!(xml, "<tools>\n</tools>");
    }

    #[test]
    fn test_tools_summary() {
        let tools = vec![
            ToolDefinition {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                input_schema: serde_json::json!({}),
            },
            ToolDefinition {
                name: "write_file".to_string(),
                description: "Write a file".to_string(),
                input_schema: serde_json::json!({}),
            },
        ];
        let summary = tools_summary(&tools);
        assert!(summary.contains("read_file: Read a file"));
        assert!(summary.contains("write_file: Write a file"));
    }

    #[test]
    fn test_tools_summary_empty() {
        let summary = tools_summary(&[]);
        assert!(summary.is_empty());
    }
}
