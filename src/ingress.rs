use crate::str_utils;
use crate::types::*;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub struct CursorMetadata {
    #[serde(rename = "cursorConversationId")]
    pub conversation_id: Option<String>,
    #[serde(rename = "cursorRequestId")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub enum ModelProvider {
    Gemini(String),
    Anthropic(String),
    OpenAI(String),
    Standard(String),
}

impl ModelProvider {
    pub fn model_name(&self) -> &str {
        match self {
            ModelProvider::Gemini(s) => s,
            ModelProvider::Anthropic(s) => s,
            ModelProvider::OpenAI(s) => s,
            ModelProvider::Standard(s) => s,
        }
    }
}

impl<'de> Deserialize<'de> for ModelProvider {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let lower = s.to_lowercase();
        if lower.contains("google/") || lower.contains("gemini") {
            Ok(ModelProvider::Gemini(s))
        } else if lower.contains("anthropic/") || lower.contains("claude") {
            Ok(ModelProvider::Anthropic(s))
        } else if lower.contains("openai/") || lower.contains("gpt") {
            Ok(ModelProvider::OpenAI(s))
        } else {
            Ok(ModelProvider::Standard(s))
        }
    }
}

#[derive(Deserialize, Serialize, Debug)]
pub struct RawTurn {
    pub model: ModelProvider,
    #[serde(alias = "input")]
    pub messages: Vec<RawTurnRecord>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub metadata: Option<CursorMetadata>,
    #[serde(default, flatten)]
    pub extra: serde_json::Value,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct RawTurnRecord {
    pub role: Option<Role>,
    #[serde(rename = "type")]
    pub type_: Option<String>,
    #[serde(default)]
    pub content: Option<RawContent>,
    #[serde(default)]
    pub tool_calls: Vec<RawToolCall>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    // For OpenAI function_call style
    pub name: Option<String>,
    pub arguments: Option<String>,
    pub call_id: Option<String>,
    pub output: Option<String>,
}

impl RawTurnRecord {
    pub fn validate(&self, index: usize) -> Result<()> {
        let role = match self.role.clone() {
            Some(r) => r,
            None => Role::User,
        };

        if role == Role::Tool {
            let has_id = self.tool_call_id.is_some() || self.call_id.is_some();
            if !has_id {
                return Err(ParallaxError::InvalidIngress(format!(
                    "Message at index {} has role 'tool' but is missing a tool_call_id",
                    index
                ))
                .into());
            }
        }

        for (tc_idx, tc) in self.tool_calls.iter().enumerate() {
            // Try to parse arguments, but don't fail immediately if they're malformed
            // Allow the hardening/repair layer to handle JSON repair
            if let Err(e) = serde_json::from_str::<serde_json::Value>(&tc.function.arguments) {
                tracing::warn!(
                    "Message {} tool call {} ('{}') has malformed JSON arguments: {} - will attempt repair",
                    index, tc_idx, tc.function.name, e
                );
                // Don't return error here - let the hardening layer attempt repair
            }
        }

        if let Some(args) = &self.arguments {
            // Try to parse arguments, but don't fail immediately if they're malformed
            // Allow the hardening/repair layer to handle JSON repair
            if let Err(e) = serde_json::from_str::<serde_json::Value>(args) {
                tracing::warn!(
                    "Message {} legacy function_call has malformed JSON arguments: {} - will attempt repair",
                    index, e
                );
                // Don't return error here - let the hardening layer attempt repair
            }
        }

        if role == Role::User && matches!(self.content, Some(RawContent::Null)) {
            return Err(ParallaxError::InvalidIngress(format!(
                "Message {} (User) cannot have null content",
                index
            ))
            .into());
        }

        Ok(())
    }
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(untagged)]
pub enum RawContent {
    String(String),
    Parts(Vec<RawContentPart>),
    Null,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(tag = "type")]
pub enum RawContentPart {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(flatten)]
        extra: serde_json::Value,
    },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: RawImageUrl },
    #[serde(rename = "image")]
    Image { source: RawImageSource },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: serde_json::Value,
        #[serde(default)]
        is_error: bool,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct RawImageSource {
    #[serde(rename = "type")]
    pub type_: String,
    pub media_type: String,
    pub data: String,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct RawImageUrl {
    pub url: String,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct RawToolCall {
    pub id: String,
    pub function: RawFunction,
    #[serde(default, flatten)]
    pub extra: serde_json::Value,
}

#[derive(Deserialize, Serialize, Debug)]
pub struct RawFunction {
    pub name: String,
    pub arguments: String,
}

impl RawTurn {
    pub fn validate(&self) -> Result<()> {
        if self.messages.is_empty() {
            return Err(ParallaxError::InvalidIngress(
                "Request must contain at least one message".into(),
            )
            .into());
        }

        for (i, msg) in self.messages.iter().enumerate() {
            msg.validate(i)?;
        }

        Ok(())
    }

    pub fn generate_anchor_hash(&self) -> Result<String> {
        let mut hasher = Sha256::new();
        hasher.update(self.model.model_name());
        if let Some(u) = &self.user {
            hasher.update(u);
        }

        let anchor_text = self.extract_anchor_text();
        let cleaned_text = self.clean_anchor_text(anchor_text);

        hasher.update(cleaned_text.trim().as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        let model_name = self.model.model_name();
        tracing::info!(
            "[⚙️  -> ⚙️ ] Identify: [{}...] Model: {}",
            str_utils::prefix_chars(&hash, 8),
            model_name
        );
        Ok(hash)
    }

    fn extract_anchor_text(&self) -> String {
        for msg in &self.messages {
            let content = match &msg.content {
                Some(RawContent::String(s)) => s.as_str(),
                _ => "",
            };

            if content.contains("<user_query>") {
                if let Some(start) = content.find("<user_query>")
                    && let Some(end) = content.find("</user_query>")
                    && let Some(slice) = str_utils::slice_bytes_safe(content, start + 12, end)
                {
                    return slice.to_string();
                }
                return content.to_string();
            }
        }

        let first_user = self.messages.iter().find(|m| m.role == Some(Role::User));

        match first_user {
            Some(msg) => match &msg.content {
                Some(RawContent::String(s)) => s.clone(),
                Some(RawContent::Parts(parts)) => parts
                    .iter()
                    .filter_map(|p| {
                        if let RawContentPart::Text { text, .. } = p {
                            Some(text.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => String::new(),
            },
            None => String::new(),
        }
    }

    fn clean_anchor_text(&self, mut text: String) -> String {
        let tags = [
            "system_reminder",
            "task_management",
            "communication",
            "terminal_files_information",
            "project_layout",
            "user_info",
        ];

        for tag in tags {
            let start_tag = format!("<{}>", tag);
            let end_tag = format!("</{}>", tag);
            while let Some(start_idx) = text.find(&start_tag) {
                if let Some(end_idx) = text.find(&end_tag) {
                    text.replace_range(start_idx..end_idx + end_tag.len(), "");
                } else {
                    text.replace_range(start_idx..start_idx + start_tag.len(), "");
                }
            }
        }
        text
    }

    pub fn extract_request_id(&self) -> String {
        match &self.metadata {
            Some(meta) => match &meta.request_id {
                Some(id) => id.clone(),
                None => uuid::Uuid::new_v4().to_string(),
            },
            None => uuid::Uuid::new_v4().to_string(),
        }
    }
}
