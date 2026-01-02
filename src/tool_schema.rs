//! Tool Schema Analysis Module
//!
//! Provides utilities to analyze tool definitions and determine which parameters are required
//! vs optional. This helps classify empty tool arguments appropriately during finalization.

use serde_json::Value;
use std::collections::HashMap;

/// Metadata about a tool's parameters
#[derive(Debug, Clone)]
pub struct ToolSchema {
    pub name: String,
    pub required_params: Vec<String>,
    pub optional_params: Vec<String>,
    pub has_required_params: bool,
}

impl ToolSchema {
    /// Analyze a tool definition to extract parameter requirements
    pub fn from_tool_definition(tool: &Value) -> Option<Self> {
        let function = tool.get("function")?;
        let name = function.get("name")?.as_str()?.to_string();

        let parameters = function.get("parameters")?;
        let properties = parameters.get("properties")?.as_object()?;
        let required = parameters
            .get("required")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let all_params: Vec<String> = properties.keys().cloned().collect();
        let optional_params: Vec<String> = all_params
            .iter()
            .filter(|p| !required.contains(p))
            .cloned()
            .collect();

        Some(ToolSchema {
            name,
            required_params: required.clone(),
            optional_params,
            has_required_params: !required.is_empty(),
        })
    }

    /// Check if this tool should have parameters
    pub fn should_have_params(&self) -> bool {
        self.has_required_params || !self.optional_params.is_empty()
    }

    /// Check if empty arguments are acceptable for this tool
    pub fn empty_args_acceptable(&self) -> bool {
        !self.has_required_params
    }
}

/// Schema registry for all available tools
pub struct ToolSchemaRegistry {
    schemas: HashMap<String, ToolSchema>,
}

impl ToolSchemaRegistry {
    pub fn new() -> Self {
        Self {
            schemas: HashMap::new(),
        }
    }

    /// Build registry from tool definitions
    pub fn from_tools(tools: &[Value]) -> Self {
        let mut registry = Self::new();
        for tool in tools {
            if let Some(schema) = ToolSchema::from_tool_definition(tool) {
                registry.schemas.insert(schema.name.clone(), schema);
            }
        }
        registry
    }

    /// Get schema for a tool by name
    pub fn get(&self, name: &str) -> Option<&ToolSchema> {
        self.schemas.get(name)
    }

    /// Check if tool has required parameters
    pub fn has_required_params(&self, name: &str) -> bool {
        self.get(name)
            .map(|s| s.has_required_params)
            .unwrap_or(false)
    }

    /// Check if empty arguments are acceptable
    pub fn empty_args_acceptable(&self, name: &str) -> bool {
        self.get(name)
            .map(|s| s.empty_args_acceptable())
            .unwrap_or(true) // Default to acceptable if unknown
    }
}

impl Default for ToolSchemaRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_tool_schema_from_definition() {
        let tool = json!({
            "type": "function",
            "function": {
                "name": "grep",
                "description": "Search files",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string"},
                        "-A": {"type": "number"}
                    },
                    "required": ["pattern"]
                }
            }
        });

        let schema = ToolSchema::from_tool_definition(&tool).unwrap();
        assert_eq!(schema.name, "grep");
        assert_eq!(schema.required_params, vec!["pattern"]);
        assert_eq!(schema.optional_params, vec!["-A"]);
        assert!(schema.has_required_params);
    }

    #[test]
    fn test_tool_schema_no_required_params() {
        let tool = json!({
            "type": "function",
            "function": {
                "name": "list_mcp_resources",
                "description": "List resources",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "server": {"type": "string"}
                    },
                    "required": []
                }
            }
        });

        let schema = ToolSchema::from_tool_definition(&tool).unwrap();
        assert_eq!(schema.name, "list_mcp_resources");
        assert!(schema.required_params.is_empty());
        assert!(!schema.has_required_params);
        assert!(schema.empty_args_acceptable());
    }

    #[test]
    fn test_registry_from_tools() {
        let tools = vec![
            json!({
                "type": "function",
                "function": {
                    "name": "grep",
                    "parameters": {
                        "type": "object",
                        "properties": {"pattern": {"type": "string"}},
                        "required": ["pattern"]
                    }
                }
            }),
            json!({
                "type": "function",
                "function": {
                    "name": "list_mcp_resources",
                    "parameters": {
                        "type": "object",
                        "properties": {},
                        "required": []
                    }
                }
            }),
        ];

        let registry = ToolSchemaRegistry::from_tools(&tools);
        assert!(registry.has_required_params("grep"));
        assert!(!registry.has_required_params("list_mcp_resources"));
    }
}
