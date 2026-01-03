use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorTag {
    pub tag: String,
    pub category: TagCategory,
    pub description: String,
    pub is_scaffolding: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TagCategory {
    Policy,
    Env,
    Payload,
    Other,
}

pub struct TagRegistry {
    pub tags: Vec<CursorTag>,
}

impl Default for TagRegistry {
    fn default() -> Self {
        Self {
            tags: vec![
                CursorTag {
                    tag: "system_reminder".to_string(),
                    category: TagCategory::Policy,
                    description: "System instructions and reminders".to_string(),
                    is_scaffolding: true,
                },
                CursorTag {
                    tag: "task_management".to_string(),
                    category: TagCategory::Policy,
                    description: "Task tracking and todo lists".to_string(),
                    is_scaffolding: true,
                },
                CursorTag {
                    tag: "communication".to_string(),
                    category: TagCategory::Policy,
                    description: "Communication style rules".to_string(),
                    is_scaffolding: true,
                },
                CursorTag {
                    tag: "terminal_files_information".to_string(),
                    category: TagCategory::Env,
                    description: "Information about open terminals".to_string(),
                    is_scaffolding: true,
                },
                CursorTag {
                    tag: "project_layout".to_string(),
                    category: TagCategory::Env,
                    description: "Overview of project file structure".to_string(),
                    is_scaffolding: true,
                },
                CursorTag {
                    tag: "user_info".to_string(),
                    category: TagCategory::Env,
                    description: "Basic user and environment context".to_string(),
                    is_scaffolding: true,
                },
                CursorTag {
                    tag: "rules".to_string(),
                    category: TagCategory::Policy,
                    description: "Generic rules container".to_string(),
                    is_scaffolding: true,
                },
                CursorTag {
                    tag: "agent_requestable_workspace_rules".to_string(),
                    category: TagCategory::Policy,
                    description: "Workspace-specific rules".to_string(),
                    is_scaffolding: true,
                },
                CursorTag {
                    tag: "user_query".to_string(),
                    category: TagCategory::Payload,
                    description: "The actual user query".to_string(),
                    is_scaffolding: true,
                },
                CursorTag {
                    tag: "mcp_instructions".to_string(),
                    category: TagCategory::Policy,
                    description: "MCP server instructions".to_string(),
                    is_scaffolding: true,
                },
                CursorTag {
                    tag: "agent_requestable_workspace_rules".to_string(),
                    category: TagCategory::Policy,
                    description: "Agent-requestable workspace rules".to_string(),
                    is_scaffolding: true,
                },
            ],
        }
    }
}

impl TagRegistry {
    pub fn is_registered(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t.tag == tag)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtractedTag {
    pub tag: String,
    pub content: String,
    pub start_offset: usize,
    pub end_offset: usize,
}

pub fn extract_tags(text: &str) -> Vec<ExtractedTag> {
    let mut tags = Vec::new();
    let mut search_idx = 0;

    while let Some(start_bracket) = text[search_idx..].find('<') {
        let abs_start_bracket = search_idx + start_bracket;
        let rest = &text[abs_start_bracket + 1..];

        if let Some(end_bracket) = rest.find('>') {
            let tag_name = rest[..end_bracket].trim();

            // Basic tag name validation: alphanumeric, underscore, hyphen
            if !tag_name.is_empty()
                && tag_name
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                let close_tag = format!("</{}>", tag_name);
                if let Some(close_start) = rest[end_bracket + 1..].find(&close_tag) {
                    let abs_close_start = abs_start_bracket + 1 + end_bracket + 1 + close_start;
                    let content = &text[abs_start_bracket + 1 + end_bracket + 1..abs_close_start];

                    tags.push(ExtractedTag {
                        tag: tag_name.to_string(),
                        content: content.to_string(),
                        start_offset: abs_start_bracket,
                        end_offset: abs_close_start + close_tag.len(),
                    });

                    search_idx = abs_close_start + close_tag.len();
                    continue;
                }
            }
        }
        search_idx = abs_start_bracket + 1;
    }

    tags
}

/// Represents the change status of a tag between two turns
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TagStatus {
    New,
    Modified,
    Unchanged,
    Removed,
}

/// A tag with its delta status compared to a previous version
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TagDelta {
    pub tag: String,
    pub content: String,
    pub status: TagStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_content: Option<String>,
}

/// Compute the delta between current and previous tags
pub fn compute_tag_deltas(
    current_tags: &[ExtractedTag],
    previous_tags: &[ExtractedTag],
) -> Vec<TagDelta> {
    let mut deltas = Vec::new();

    // Build a map of previous tags for quick lookup
    let mut previous_map: std::collections::HashMap<&str, &ExtractedTag> =
        previous_tags.iter().map(|t| (t.tag.as_str(), t)).collect();

    // Check current tags for new or modified
    for current_tag in current_tags {
        if let Some(previous_tag) = previous_map.remove(current_tag.tag.as_str()) {
            if previous_tag.content == current_tag.content {
                deltas.push(TagDelta {
                    tag: current_tag.tag.clone(),
                    content: current_tag.content.clone(),
                    status: TagStatus::Unchanged,
                    previous_content: None,
                });
            } else {
                deltas.push(TagDelta {
                    tag: current_tag.tag.clone(),
                    content: current_tag.content.clone(),
                    status: TagStatus::Modified,
                    previous_content: Some(previous_tag.content.clone()),
                });
            }
        } else {
            deltas.push(TagDelta {
                tag: current_tag.tag.clone(),
                content: current_tag.content.clone(),
                status: TagStatus::New,
                previous_content: None,
            });
        }
    }

    // Remaining tags in previous_map are removed
    for (_, previous_tag) in previous_map {
        deltas.push(TagDelta {
            tag: previous_tag.tag.clone(),
            content: previous_tag.content.clone(),
            status: TagStatus::Removed,
            previous_content: None,
        });
    }

    deltas
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_simple_tags() {
        let text =
            "Hello <user_query>What is up?</user_query> some more text <rules>Rule 1</rules>";
        let tags = extract_tags(text);
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].tag, "user_query");
        assert_eq!(tags[0].content, "What is up?");
        assert_eq!(tags[1].tag, "rules");
        assert_eq!(tags[1].content, "Rule 1");
    }

    #[test]
    fn test_extract_nested_tags() {
        let text = "<outer><inner>hello</inner></outer>";
        let tags = extract_tags(text);
        // Pragmatic scanner finds outer first and skips inner if it moves search_idx
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].tag, "outer");
        assert_eq!(tags[0].content, "<inner>hello</inner>");
    }

    #[test]
    fn test_malformed_tags() {
        let text = "<not-closed>hi <invalid tag> <rules>ok</rules>";
        let tags = extract_tags(text);
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].tag, "rules");
    }

    #[test]
    fn test_compute_tag_deltas_new() {
        let previous = vec![];
        let current = vec![ExtractedTag {
            tag: "user_info".to_string(),
            content: "OS: Linux".to_string(),
            start_offset: 0,
            end_offset: 20,
        }];

        let deltas = compute_tag_deltas(&current, &previous);
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].status, TagStatus::New);
        assert_eq!(deltas[0].tag, "user_info");
    }

    #[test]
    fn test_compute_tag_deltas_modified() {
        let previous = vec![ExtractedTag {
            tag: "user_info".to_string(),
            content: "OS: Linux".to_string(),
            start_offset: 0,
            end_offset: 20,
        }];
        let current = vec![ExtractedTag {
            tag: "user_info".to_string(),
            content: "OS: Windows".to_string(),
            start_offset: 0,
            end_offset: 22,
        }];

        let deltas = compute_tag_deltas(&current, &previous);
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].status, TagStatus::Modified);
        assert_eq!(deltas[0].previous_content, Some("OS: Linux".to_string()));
    }

    #[test]
    fn test_compute_tag_deltas_unchanged() {
        let previous = vec![ExtractedTag {
            tag: "user_info".to_string(),
            content: "OS: Linux".to_string(),
            start_offset: 0,
            end_offset: 20,
        }];
        let current = vec![ExtractedTag {
            tag: "user_info".to_string(),
            content: "OS: Linux".to_string(),
            start_offset: 0,
            end_offset: 20,
        }];

        let deltas = compute_tag_deltas(&current, &previous);
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].status, TagStatus::Unchanged);
    }

    #[test]
    fn test_compute_tag_deltas_removed() {
        let previous = vec![ExtractedTag {
            tag: "user_info".to_string(),
            content: "OS: Linux".to_string(),
            start_offset: 0,
            end_offset: 20,
        }];
        let current = vec![];

        let deltas = compute_tag_deltas(&current, &previous);
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].status, TagStatus::Removed);
    }

    #[test]
    fn test_compute_tag_deltas_complex() {
        let previous = vec![
            ExtractedTag {
                tag: "user_info".to_string(),
                content: "OS: Linux".to_string(),
                start_offset: 0,
                end_offset: 20,
            },
            ExtractedTag {
                tag: "rules".to_string(),
                content: "Rule 1".to_string(),
                start_offset: 21,
                end_offset: 40,
            },
        ];
        let current = vec![
            ExtractedTag {
                tag: "user_info".to_string(),
                content: "OS: Windows".to_string(),
                start_offset: 0,
                end_offset: 22,
            },
            ExtractedTag {
                tag: "project_layout".to_string(),
                content: "Layout here".to_string(),
                start_offset: 23,
                end_offset: 50,
            },
        ];

        let deltas = compute_tag_deltas(&current, &previous);
        assert_eq!(deltas.len(), 3);

        // Find each delta by tag name
        let user_info_delta = deltas.iter().find(|d| d.tag == "user_info");
        let project_delta = deltas.iter().find(|d| d.tag == "project_layout");
        let rules_delta = deltas.iter().find(|d| d.tag == "rules");

        match user_info_delta {
            Some(d) => assert_eq!(d.status, TagStatus::Modified),
            None => panic!("user_info delta not found"),
        }

        match project_delta {
            Some(d) => assert_eq!(d.status, TagStatus::New),
            None => panic!("project_layout delta not found"),
        }

        match rules_delta {
            Some(d) => assert_eq!(d.status, TagStatus::Removed),
            None => panic!("rules delta not found"),
        }
    }
}
