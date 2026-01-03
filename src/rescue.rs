use crate::str_utils;
use serde_json::json;

pub struct RescueResult {
    pub name: String,
    pub tool_call: serde_json::Value,
}

pub fn detect_xml_invoke(text: &str) -> Option<RescueResult> {
    if !text.contains("<invoke") || !text.contains("</invoke>") {
        return None;
    }

    // Very simple parser for <invoke name="...">{...}</invoke>
    let start_idx = text.find("<invoke")?;
    let end_tag = "</invoke>";
    let end_idx = text.find(end_tag)?;

    let tag_content = str_utils::slice_bytes_safe(text, start_idx, end_idx + end_tag.len())?;

    // Extract name
    let name_start = tag_content.find("name=\"")? + 6;
    let name_end = tag_content[name_start..].find("\"")? + name_start;
    let name = str_utils::slice_bytes_safe(tag_content, name_start, name_end)?.to_string();

    // Extract body (JSON)
    let body_start = tag_content.find(">")? + 1;
    let body_end = tag_content.find("</invoke>")?;
    let body_str = str_utils::slice_bytes_safe(tag_content, body_start, body_end)?.trim();

    let arguments = if body_str.is_empty() {
        "{}".to_string()
    } else {
        body_str.to_string()
    };

    Some(RescueResult {
        name: name.clone(),
        tool_call: json!({
            "id": format!("call_{}", str_utils::prefix_chars(&uuid::Uuid::new_v4().to_string(), 8)),
            "type": "function",
            "function": {
                "name": name,
                "arguments": arguments,
            }
        }),
    })
}
