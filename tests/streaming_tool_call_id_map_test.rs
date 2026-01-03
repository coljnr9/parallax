use parallax::types::{InternalPulse, PulsePart, Role, TurnAccumulator};

#[test]
fn tool_call_arguments_are_preserved_when_id_missing_on_followup_chunk() {
    // Simulate a provider that sends tool call name+id on first chunk, but omits id on subsequent chunks.
    // We keep the tool call associated by index->id mapping in streaming.rs; this unit test exercises
    // the accumulator behavior when we feed it consistent ids.

    let mut acc = TurnAccumulator::new();
    acc.role = Some(Role::Assistant);

    // First chunk: tool call begins
    acc.push(InternalPulse {
        content: vec![PulsePart::ToolCall {
            id: Some("call_abc".to_string()),
            name: Some("read_file".to_string()),
            arguments_delta: "{\"target_file\":".to_string(),
            metadata: None,
        }],
        finish_reason: None,
        usage: None,
    });

    // Second chunk: continued arguments. In real streaming, we now map missing id via index->id,
    // so the accumulator should still see "call_abc".
    acc.push(InternalPulse {
        content: vec![PulsePart::ToolCall {
            id: Some("call_abc".to_string()),
            name: None,
            arguments_delta: "\"/tmp/foo\"}".to_string(),
            metadata: None,
        }],
        finish_reason: Some("tool_calls".to_string()),
        usage: None,
    });

    let finalized = acc.finalize();

    let mut tool_calls = finalized
        .content
        .iter()
        .filter_map(|p| match p {
            parallax::types::MessagePart::ToolCall {
                id,
                name,
                arguments,
                ..
            } => Some((id.clone(), name.clone(), arguments.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(tool_calls.len(), 1);
    if let Some((id, name, args)) = tool_calls.pop() {
        assert_eq!(id, "call_abc");
        assert_eq!(name, "read_file");
        assert_eq!(args["target_file"], "/tmp/foo");
    } else {
        unreachable!("Expected one tool call");
    }
}
