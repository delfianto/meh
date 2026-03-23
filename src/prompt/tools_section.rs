//! Tool definitions section for system prompt.
//!
//! For providers that don't support native tool use (function calling),
//! tool definitions are serialized as XML and injected into the system prompt.

use crate::provider::ToolDefinition;
use std::fmt::Write as _;

/// Convert tool definitions to XML format for system prompt injection.
pub fn tools_to_xml(tools: &[ToolDefinition]) -> String {
    let mut xml = String::from("<tools>\n");
    for tool in tools {
        let _ = writeln!(xml, "  <tool name=\"{}\">", tool.name);
        let _ = writeln!(xml, "    <description>{}</description>", tool.description);
        let _ = writeln!(
            xml,
            "    <input_schema>{}</input_schema>",
            serde_json::to_string(&tool.input_schema).unwrap_or_default()
        );
        let _ = writeln!(xml, "  </tool>");
    }
    xml.push_str("</tools>");
    xml
}

/// Build tool use instructions for XML-based tool calling.
pub fn xml_tool_use_instructions() -> String {
    "To use a tool, respond with XML in this format:\n\
     <tool_use>\n  \
       <name>tool_name</name>\n  \
       <parameters>\n    \
         <param_name>value</param_name>\n  \
       </parameters>\n\
     </tool_use>\n\n\
     You may use multiple tools in a single response. Wait for tool results before proceeding."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_to_xml_basic() {
        let tools = vec![ToolDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            input_schema: serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }];
        let xml = tools_to_xml(&tools);
        assert!(xml.contains("<tool name=\"read_file\">"));
        assert!(xml.contains("Read a file"));
        assert!(xml.contains("</tools>"));
    }

    #[test]
    fn tools_to_xml_multiple() {
        let tools = vec![
            ToolDefinition {
                name: "read_file".to_string(),
                description: "Read".to_string(),
                input_schema: serde_json::json!({}),
            },
            ToolDefinition {
                name: "write_file".to_string(),
                description: "Write".to_string(),
                input_schema: serde_json::json!({}),
            },
        ];
        let xml = tools_to_xml(&tools);
        assert!(xml.contains("read_file"));
        assert!(xml.contains("write_file"));
    }

    #[test]
    fn tools_to_xml_empty() {
        let xml = tools_to_xml(&[]);
        assert_eq!(xml, "<tools>\n</tools>");
    }

    #[test]
    fn xml_tool_use_instructions_content() {
        let inst = xml_tool_use_instructions();
        assert!(inst.contains("tool_use"));
        assert!(inst.contains("tool_name"));
    }
}
