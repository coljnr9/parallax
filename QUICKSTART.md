# Quick Start Guide - Parallax Conversation Debugger

## Prerequisites
- Rust 1.70+ (for backend)
- Node.js 18+ (for UI development)
- SQLite (included in sqlx)

## Building

### Quick Start (Automatic UI Build)
```bash
cd /home/cole/rust/feat-debug-utils

# Parallax automatically builds the UI on startup
cargo build --release
./target/release/parallax
```

The server will:
1. Automatically check for and install UI dependencies (if needed)
2. Build the React UI to `debug_ui/dist/`
3. Start the server with the UI available at the Parallax port

### Manual Build (Backend + UI)
```bash
cd /home/cole/rust/feat-debug-utils

# Manually build React UI first
cd debug_ui && npm run build && cd ..

# Build Rust backend (includes UI in binary)
cargo build --release
```

The release binary will be at: `target/release/parallax` (16MB)

### Development Build
```bash
# Faster iteration - UI builds automatically on startup
cargo build
./target/debug/parallax
```

## Running

### Start the Server
```bash
# Set required environment variables
export OPENROUTER_API_KEY="your-key-here"
export DATABASE_URL="sqlite:parallax.db"

# Run the server
./target/release/parallax

# Or with debug capture enabled
ENABLE_DEBUG_CAPTURE=true ./target/release/parallax
```

### Access the UI
Open your browser to:
- **Debug UI**: `http://localhost:8080/debug/ui` (same port as Parallax)
- **API**: `http://localhost:8080/debug/conversations`
- **Health**: `http://localhost:8080/health`

Note: Default port is 8080, change with `--port` flag

## Debug UI Features

### Conversations List
- Shows all captured conversations
- Sorted by last update time
- Issue badges (⚠️) show problem count
- Click to view conversation details

### Turn Details
- **Lifecycle Stages**: See all processing stages (ingress_raw, lifted, projected, final)
- **Issues Panel**: Structured issue detection results
  - Tool args empty/repaired
  - Reasoning leaks
  - Unregistered Cursor tags
- **Stage Timeline**: Table with stage names, sizes, and blob viewer

### Blob Viewer
- Click "View Blob" to load large payloads
- Lazy loading for performance
- Supports JSON, text, and binary data

## API Endpoints

### List Conversations
```bash
curl http://localhost:3000/debug/conversations
```

### Get Conversation Summary
```bash
curl http://localhost:3000/debug/conversation/{cid}
```

### Get Turn Details
```bash
curl http://localhost:3000/debug/conversation/{cid}/turn/{tid}
```

### Get Blob (Lazy Load)
```bash
curl http://localhost:3000/debug/blob/{cid}/{tid}/{blob_id}
```

## Configuration

### Environment Variables
- `OPENROUTER_API_KEY` - Required for upstream requests
- `DATABASE_URL` - SQLite connection string (default: sqlite:parallax.db)
- `ENABLE_DEBUG_CAPTURE` - Enable debug bundle capture (default: false)
- `HOST` - Server host (default: 127.0.0.1)
- `PORT` - Server port (default: 3000)

### Debug Capture
Debug bundles are stored in: `debug_capture/conversations/`

Structure:
```
debug_capture/
└── conversations/
    └── {conversation_id}/
        ├── conversation.json
        └── turns/
            └── {turn_id}/
                ├── turn.json
                └── blobs/
                    ├── ingress_raw.zst
                    ├── lifted.zst
                    ├── projected.zst
                    ├── upstream_response.zst
                    └── final.zst
```

### Cleanup
To delete all debug captures:
```bash
rm -rf debug_capture/
```

## Development

### React UI Development (Live Reload)
For UI development with hot reload:
```bash
cd debug_ui
npm run watch
```

This automatically rebuilds the UI whenever you save changes. The rebuilt files are served by Parallax at `http://localhost:8080/debug/ui`.

**Note:** With Tailwind CSS v4, styles are processed automatically via the `@tailwindcss/vite` plugin.

### Building UI Only
```bash
cd debug_ui
npm run build
```

Output goes to `debug_ui/dist/`

### Rust Development
```bash
# Check code
cargo check

# Run tests
cargo test

# Lint
cargo clippy -- -D warnings

# Format
cargo fmt
```

## Troubleshooting

### "Failed to bind to port"
- Port 8080 is already in use
- Change with: `./target/release/parallax --port 3001`

### "Debug UI not loading"
- Ensure `debug_ui/dist/` exists
- Rebuild UI: `cd debug_ui && npm run build`

### "No conversations showing"
- Enable debug capture: `ENABLE_DEBUG_CAPTURE=true`
- Make a request to `/v1/chat/completions` or `/chat/completions`
- Bundles appear in `debug_capture/conversations/`

### "Blob viewer shows 404"
- Blob may not exist yet
- Ensure the turn completed successfully
- Check `debug_capture/conversations/{cid}/turns/{tid}/blobs/`

## Performance Notes

- **Indexes**: Always loaded (small JSON, <1MB per conversation)
- **Blobs**: Lazy loaded on demand (compressed with zstd)
- **Large conversations**: 100k+ tokens handled efficiently
- **Cleanup**: Automatic cleanup keeps disk usage bounded

## Next Steps

1. **Send a request** to the API to generate debug data
2. **Open the UI** at `http://localhost:3000/debug/ui`
3. **Explore** the conversation and turn details
4. **Check issues** for tool args, reasoning leaks, tag problems
5. **View blobs** to inspect raw payloads

## Support

For issues or questions:
- Check `DEBUG_UI_IMPLEMENTATION.md` for architecture details
- Review Rust standards in `.cursor/rules/rust_standards.mdc`
- See specs in `specs/C2OR_SPEC.md` and `specs/TRACE_SPEC.md`

