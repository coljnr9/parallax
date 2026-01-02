# TRACE_SPEC.md

## Overview
This specification defines the causal tracing narrative for Parallax. Every request must be traceable as a single story with nested spans, explicit lineage, and redacted outputs.

## Span Topology

### `shim.request` (Root, INFO)
- **Created at**: Ingress (middleware or early handler).
- **Placeholders**:
  - `request_id`: UUID or stable ID.
  - `model.target`: The requested model ID.
  - `tokens.prompt`: count (recorded at end).
  - `tokens.completion`: count (recorded at end).
  - `http.status`: status code (recorded at end).
  - `shim.outcome`: `success`, `client_error`, `upstream_error`, `internal_error`.

### `shim.lift` (DEBUG)
- **Purpose**: Converting raw JSON ingress to internal context.
- **Fields**: `conversation.id`, `conversation.anchor_hash_prefix`.

### `shim.route_model` (DEBUG)
- **Purpose**: Decision logic for provider flavor.
- **Fields**: `model.provider`, `model.flavor`.

### `shim.project` (DEBUG)
- **Purpose**: Projecting context to upstream payload.
- **Fields**: `shim.project.messages_len`, `shim.project.tools_len`.

### `shim.upstream.http` (INFO)
- **Purpose**: The actual network call to OpenRouter/Upstream.
- **Fields**: `http.method`, `http.url.host`, `http.url.path`, `http.status`, `shim.output.latency_ms`.

### `shim.upstream.stream` (DEBUG)
- **Purpose**: Lifecycle of a streaming response.
- **Fields**: `shim.stream.chunks`, `shim.stream.tokens`, `shim.stream.tool_calls`.

## Field Taxonomy (Dot-Notation)
All structured fields must follow this dot-notation naming convention:

- `identity.*`: `request_id`, `turn_id`, `conversation.id`
- `model.*`: `model.target`, `model.provider`, `model.flavor`
- `http.*`: `http.method`, `http.status`, `http.url.host`, `http.url.path`
- `tokens.*`: `tokens.prompt`, `tokens.completion`, `tokens.total`, `tokens.cached`
- `shim.input.*`: `shim.input.messages_len`, `shim.input.has_tools`, `shim.input.stream`
- `shim.output.*`: `shim.output.latency_ms`
- `shim.outcome`: outcome status

## Redaction Policy
- **Banned Fields**: `Authorization`, `x-api-key`, `cookie`, `set-cookie`.
- **Masking**: Any field or log line matching `sk-[A-Za-z0-9]{20,}` or `Bearer\s+[^\s]+` must be masked.
- **Default**: No PII (emails, names) or secrets should ever be emitted in `INFO` or `WARN` level fields unless explicitly marked as safe.

## Output Formats

### Human Tree View
- Indented hierarchical view of spans.
- ANSI colors for levels.
- Targets `stderr`.

### Agent NDJSON (logs/trace_buffer.json)
- One JSON object per line.
- Schema:
  ```json
  {
    "timestamp": "ISO8601",
    "level": "INFO|DEBUG|...",
    "trace_id": "...",
    "span_list": [
      { "name": "shim.request", "fields": { ... } },
      { "name": "shim.project", "fields": { ... } }
    ],
    "message": "...",
    "fields": { ... }
  }
  ```
