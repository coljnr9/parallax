//! JSON Repair and Streaming Buffer Module
//! 
//! Handles incomplete JSON from streaming APIs by detecting incomplete structures
//! and attempting to repair them gracefully.

use serde_json::Value;

/// Detects if a JSON string is incomplete (unbalanced braces/quotes)
pub fn is_json_complete(json_str: &str) -> bool {
    let trimmed = json_str.trim();
    if trimmed.is_empty() {
        return false;
    }

    let mut brace_count = 0;
    let mut bracket_count = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for ch in trimmed.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match ch {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            '{' if !in_string => brace_count += 1,
            '}' if !in_string => brace_count -= 1,
            '[' if !in_string => bracket_count += 1,
            ']' if !in_string => bracket_count -= 1,
            _ => {}
        }

        // Early exit if we have unbalanced closing braces
        if brace_count < 0 || bracket_count < 0 {
            return false;
        }
    }

    // Complete if all braces/brackets are balanced and we're not in a string
    !in_string && brace_count == 0 && bracket_count == 0
}

/// Attempts to repair incomplete JSON by closing unclosed structures
pub fn repair_json(json_str: &str) -> String {
    let trimmed = json_str.trim();
    if trimmed.is_empty() {
        return "{}".to_string();
    }

    let mut result = trimmed.to_string();
    let mut brace_count = 0;
    let mut bracket_count = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for ch in trimmed.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match ch {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            '{' if !in_string => brace_count += 1,
            '}' if !in_string => brace_count -= 1,
            '[' if !in_string => bracket_count += 1,
            ']' if !in_string => bracket_count -= 1,
            _ => {}
        }
    }

    // Close unclosed strings
    if in_string {
        result.push('"');
    }

    // Close unclosed brackets
    for _ in 0..bracket_count {
        result.push(']');
    }

    // Close unclosed braces
    for _ in 0..brace_count {
        result.push('}');
    }

    result
}

/// Attempts to parse JSON, with fallback to repair and retry
pub fn parse_json_with_repair(json_str: &str) -> Result<Value, String> {
    // Try direct parse first
    if let Ok(value) = serde_json::from_str::<Value>(json_str) {
        return Ok(value);
    }

    // Try repair and parse
    let repaired = repair_json(json_str);
    match serde_json::from_str::<Value>(&repaired) {
        Ok(value) => {
            tracing::debug!(
                "[JSON-REPAIR] Successfully repaired JSON: {} -> {} chars",
                json_str.len(),
                repaired.len()
            );
            Ok(value)
        }
        Err(e) => {
            Err(format!(
                "Failed to parse JSON even after repair: {} (original: {} chars, repaired: {} chars)",
                e,
                json_str.len(),
                repaired.len()
            ))
        }
    }
}

/// Attempts to repair tool call arguments with semantic understanding
pub fn repair_tool_call_arguments(name: &str, arguments: &str) -> Result<Value, String> {
    // For create_plan tool, always use semantic repair regardless of JSON validity
    if name == "create_plan" {
        return repair_create_plan_arguments(arguments);
    }

    // First try standard JSON repair for other tools
    if let Ok(value) = parse_json_with_repair(arguments) {
        return Ok(value);
    }

    // For other tools, return the repaired JSON even if imperfect
    let repaired = repair_json(arguments);
    match serde_json::from_str::<Value>(&repaired) {
        Ok(value) => {
            tracing::warn!(
                "[JSON-REPAIR] Tool '{}' arguments repaired with warnings: {} -> {} chars",
                name,
                arguments.len(),
                repaired.len()
            );
            Ok(value)
        }
        Err(e) => {
            Err(format!(
                "Failed to repair tool '{}' arguments: {} (original: {} chars, repaired: {} chars)",
                name, e, arguments.len(), repaired.len()
            ))
        }
    }
}

/// Special repair logic for create_plan tool arguments
fn repair_create_plan_arguments(arguments: &str) -> Result<Value, String> {
    let trimmed = arguments.trim();
    
    // If it's empty or just whitespace, return default structure
    if trimmed.is_empty() {
        return Ok(serde_json::json!({
            "plan": "# Implementation Plan\n\nNo plan provided.",
            "name": "Default Plan"
        }));
    }

    // Try to parse as JSON first
    if let Ok(mut value) = serde_json::from_str::<Value>(trimmed) {
        // If it's a string, treat it as the plan content
        if let Some(plan_text) = value.as_str() {
            let plan_text = plan_text.to_string();
            let repaired_value = serde_json::json!({
                "plan": plan_text,
                "name": "Implementation Plan"
            });
            return Ok(repaired_value);
        }
        
        // If it's already an object, ensure it has required fields
        if let Some(obj) = value.as_object_mut() {
            if !obj.contains_key("plan") {
                obj.insert("plan".to_string(), serde_json::Value::String("No plan content provided.".to_string()));
            }
            if !obj.contains_key("name") {
                obj.insert("name".to_string(), serde_json::Value::String("Implementation Plan".to_string()));
            }
            return Ok(value);
        }
        
        return Ok(value);
    }

    // If JSON parsing failed, treat the entire string as plan content
    let plan_content = trimmed.to_string();
    let repaired_value = serde_json::json!({
        "plan": plan_content,
        "name": "Implementation Plan"
    });
    
    tracing::warn!(
        "[JSON-REPAIR] create_plan arguments treated as plain text: {} chars",
        arguments.len()
    );
    
    Ok(repaired_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_json_complete_valid() {
        assert!(is_json_complete("{}"));
        assert!(is_json_complete(r#"{"key": "value"}"#));
        assert!(is_json_complete("[]"));
        assert!(is_json_complete(r#"[1, 2, 3]"#));
    }

    #[test]
    fn test_is_json_complete_incomplete() {
        assert!(!is_json_complete("{"));
        assert!(!is_json_complete(r#"{"key": "value""#));
        assert!(!is_json_complete("["));
        assert!(!is_json_complete(r#"[1, 2, 3"#));
    }

    #[test]
    fn test_is_json_complete_with_escape() {
        assert!(is_json_complete(r#"{"key": "val\"ue"}"#));
        assert!(!is_json_complete(r#"{"key": "val\"ue"#));
    }

    #[test]
    fn test_repair_json_unclosed_braces() {
        let repaired = repair_json(r#"{"key": "value""#);
        assert!(serde_json::from_str::<Value>(&repaired).is_ok());
    }

    #[test]
    fn test_repair_json_unclosed_brackets() {
        let repaired = repair_json("[1, 2, 3");
        assert!(serde_json::from_str::<Value>(&repaired).is_ok());
    }

    #[test]
    fn test_repair_json_unclosed_string() {
        let repaired = repair_json(r#"{"key": "value"#);
        assert!(serde_json::from_str::<Value>(&repaired).is_ok());
    }

    #[test]
    fn test_parse_json_with_repair_valid() {
        let result = parse_json_with_repair(r#"{"key": "value"}"#);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_json_with_repair_incomplete() {
        let result = parse_json_with_repair(r#"{"key": "value""#);
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_json_with_repair_empty() {
        let result = parse_json_with_repair("");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), serde_json::json!({}));
    }

    #[test]
    fn test_repair_tool_call_arguments_create_plan_string() {
        let result = repair_tool_call_arguments("create_plan", "This is a plan without JSON");
        assert!(result.is_ok());
        
        let value = result.unwrap();
        let obj = value.as_object().unwrap();
        assert!(obj.contains_key("plan"));
        assert!(obj.contains_key("name"));
        assert_eq!(obj["plan"].as_str().unwrap(), "This is a plan without JSON");
    }

    #[test]
    fn test_repair_tool_call_arguments_create_plan_empty() {
        let result = repair_tool_call_arguments("create_plan", "");
        assert!(result.is_ok(), "Repair should succeed for empty arguments");
        
        let value = result.unwrap();
        println!("Empty args result: {:?}", value);
        let obj = value.as_object().expect("Result should be an object");
        assert!(obj.contains_key("plan"), "Should contain 'plan' key, got: {:?}", obj.keys().collect::<Vec<_>>());
        assert!(obj.contains_key("name"), "Should contain 'name' key");
        let plan_content = obj["plan"].as_str().unwrap();
        assert!(plan_content.contains("No plan provided") || plan_content.contains("Implementation Plan"), 
               "Plan should mention no plan provided or be default, got: {}", plan_content);
    }

    #[test]
    fn test_repair_tool_call_arguments_create_plan_partial_json() {
        let result = repair_tool_call_arguments("create_plan", r#"{"plan": "Partial plan"#);
        assert!(result.is_ok());
        
        let value = result.unwrap();
        let obj = value.as_object().unwrap();
        assert!(obj.contains_key("plan"), "Should contain 'plan' key");
        // The repair should handle partial JSON gracefully
        if obj.contains_key("name") {
            assert_eq!(obj["name"].as_str().unwrap(), "Implementation Plan");
        }
    }

    #[test]
    fn test_repair_tool_call_arguments_other_tool() {
        let result = repair_tool_call_arguments("grep", r#"{"pattern": "test""#);
        assert!(result.is_ok());
        
        let value = result.unwrap();
        assert!(value.is_object());
    }

    #[test]
    fn test_repair_create_plan_arguments_plain_text() {
        let result = repair_create_plan_arguments("This is just plain text plan");
        assert!(result.is_ok());
        
        let value = result.unwrap();
        let obj = value.as_object().unwrap();
        assert_eq!(obj["plan"].as_str().unwrap(), "This is just plain text plan");
        assert_eq!(obj["name"].as_str().unwrap(), "Implementation Plan");
    }

    #[test]
    fn test_repair_create_plan_arguments_json_string() {
        let result = repair_create_plan_arguments(r#"{"plan": "JSON plan content"}"#);
        assert!(result.is_ok());
        
        let value = result.unwrap();
        let obj = value.as_object().unwrap();
        assert_eq!(obj["plan"].as_str().unwrap(), "JSON plan content");
        assert_eq!(obj["name"].as_str().unwrap(), "Implementation Plan");
    }

    #[test]
    fn test_repair_create_plan_arguments_complete_json() {
        let input = r#"{"plan": "Complete plan", "name": "Custom Plan", "overview": "Test overview"}"#;
        let result = repair_create_plan_arguments(input);
        assert!(result.is_ok());
        
        let value = result.unwrap();
        let obj = value.as_object().unwrap();
        assert_eq!(obj["plan"].as_str().unwrap(), "Complete plan");
        assert_eq!(obj["name"].as_str().unwrap(), "Custom Plan");
        assert_eq!(obj["overview"].as_str().unwrap(), "Test overview");
    }
}

