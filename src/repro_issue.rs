#[cfg(test)]
mod tests {
    use crate::ingress::RawTurn;
    use crate::types::Role;

    #[test]
    fn test_repro_missing_role() {
        let json_data = r#"
        {
            "model": "gpt-4",
            "input": [
                {
                    "content": "system message",
                    "role": "system"
                },
                {
                    "arguments": "{\"target_directory\":\"now\"}",
                    "call_id": "call_123",
                    "name": "list_dir",
                    "type": "function_call"
                }
            ]
        }
        "#;

        let result: Result<RawTurn, _> = serde_json::from_str(json_data);
        assert!(result.is_ok(), "Expected OK, got {:?}", result.err());
        let turn = match result {
            Ok(t) => t,
            Err(e) => panic!("Failed to parse RawTurn: {:?}", e),
        };
        assert_eq!(turn.messages.len(), 2);
        assert_eq!(turn.messages[0].role, Some(Role::System));
        assert_eq!(turn.messages[1].role, None);
    }

    #[test]
    fn test_anthropic_image_variant() {
        let json_data = r#"
        {
            "model": "anthropic/claude-3",
            "input": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "What is in this image?"
                        },
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/jpeg",
                                "data": "SGVsbG8="
                            }
                        }
                    ]
                }
            ]
        }
        "#;

        let result: Result<RawTurn, _> = serde_json::from_str(json_data);
        assert!(result.is_ok(), "Expected OK, got {:?}", result.err());
        let turn = match result {
            Ok(t) => t,
            Err(e) => panic!("Failed to parse RawTurn: {:?}", e),
        };
        assert_eq!(turn.messages.len(), 1);
    }
}
