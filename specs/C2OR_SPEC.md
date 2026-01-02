# Authoritative API Specifications for Parallax Hub

This document serves as the source of truth for the **Hub and Spoke** mapping logic.

## 1. OpenAI (The Baseline Spoke)
Used by: OpenAI, OpenRouter (Aggregator), Cursor (Input).

### Request Shape
```json
{
  "model": "string",
  "messages": [
    {
      "role": "system|user|assistant|tool",
      "content": "string",
      "tool_call_id": "string (mandatory for tool role)",
      "tool_calls": "array (for assistant role)"
    }
  ],
  "stream": "boolean",
  "temperature": "number",
  "max_tokens": "integer",
  "tools": "array",
  "tool_choice": "string|object"
}
```

## 2. Anthropic (The "System Root" Spoke)
Used by: Claude 3.x models.

### Key Deviations
- **System Message**: Is NOT a message. It must be at the root: `"system": "..."`.
- **Max Tokens**: Mandatory field `"max_tokens": 1024`.
- **Multimodal**: Content must be an array of blocks `{"type": "text", "text": "..."}`.

### Request Shape
```json
{
  "model": "string",
  "system": "string (Extracted from history)",
  "messages": [
    {
      "role": "user|assistant",
      "content": "string|array"
    }
  ],
  "max_tokens": "integer (mandatory)",
  "stream": "boolean"
}
```

## 3. Google Gemini (The "Deeply Nested" Spoke)
Used by: Gemini 1.5/2.0/3.0 models.

### Key Deviations
- **Messages**: Renamed to `"contents"`.
- **Role**: Assistant is renamed to `"model"`.
- **Config**: Root fields (temperature, etc.) must be inside `"generationConfig"`.
- **Tools**: Tools must be inside a nested `"function_declarations"` array.

### Request Shape
```json
{
  "contents": [
    {
      "role": "user|model",
      "parts": [{"text": "..."}]
    }
  ],
  "systemInstruction": {
    "parts": [{"text": "..."}]
  },
  "generationConfig": {
    "temperature": "number",
    "maxOutputTokens": "integer",
    "topP": "number"
  },
  "tools": [
    {
      "function_declarations": [...]
    }
  ]
}
```

## 4. Normalization Rules (The Mapper)

| Internal (Hub) | OpenAI Path | Anthropic Path | Gemini Path |
| :--- | :--- | :--- | :--- |
| `history` | `.messages` | `.messages` (minus system) | `.contents` |
| `system_prompt` | `.messages[0]` | `.system` (root) | `.systemInstruction` |
| `temperature` | `.temperature` | `.temperature` | `.generationConfig.temperature` |
| `max_tokens` | `.max_tokens` | `.max_tokens` | `.generationConfig.maxOutputTokens` |
| `tools` | `.tools` | `.tools` | `.tools[0].function_declarations` |
