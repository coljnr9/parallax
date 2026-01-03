# Conversation Debugger UI - Implementation Complete ✅

## Overview
Successfully implemented a **web-first, in-process debugger** for Parallax that captures per-conversation artifacts on disk and serves a React UI to explore stages, tool calls, Cursor tags, and leakage/repair issues.

## Architecture

### Backend (Rust)
- **Debug Bundle Format**: Structured JSON indexes + optional compressed blobs
  - `debug_capture/conversations/<cid>/conversation.json` - conversation summary
  - `debug_capture/conversations/<cid>/turns/<tid>/turn.json` - turn details
  - `debug_capture/conversations/<cid>/turns/<tid>/blobs/<blob_id>.zst` - large payloads

### Frontend (React + Vite)
- **Tech Stack**: React 18, TypeScript, Tailwind CSS, Lucide Icons, Axios
- **Pages**:
  - Conversations List: Browse all captured conversations with issue badges
  - Turn List: View turns within a conversation
  - Turn Details: Inspect stages, issues, tool calls, and lifecycle

## Phases Completed

### Phase 1: Bundle Data Structures ✅
**Files**: `src/debug_bundle.rs`, `src/tag_extract.rs`

**Structures**:
- `ConversationSummary`: Top-level conversation metadata
- `TurnDetail`: Per-turn capture with stages, tool calls, tags, issues
- `StageIndex`: Snapshot of each processing stage
- `ToolCallIndex`: Tool call metadata with origin tracking
- `TagSummary`: Cursor tag inventory (registered/unregistered/leaks)
- `Issue`: Structured issue detection results

**Tag Registry**:
- Hardcoded list of known Cursor tags (system_reminder, task_management, etc.)
- Generic tag extractor with `<tag>...</tag>` pattern matching
- Attribution rules: cursor_ingress → model_output → leak

### Phase 2: Capture Points ✅
**Files**: `src/main.rs`, `src/streaming.rs`

**Wired Capture**:
- `ingress_raw`: Raw incoming payload
- `lifted`: Parsed conversation context
- `projected`: Upstream request payload
- `upstream_response`: Provider response
- `final`: Finalized turn record

**Bundle Manager Integration**:
- Initialize turn directories on request start
- Write blobs for each stage
- Update conversation/turn summaries with issue counts

### Phase 3: Issue Detectors ✅
**File**: `src/debug_bundle.rs` (BundleManager::detect_issues)

**Detectors**:
1. **ToolArgsEmptySuspicious**: Empty args on tools that require parameters
2. **ToolArgsRepaired**: Tracked via json_repair.rs integration
3. **RescueUsed**: Tracked via rescue.rs integration
4. **CursorTagUnregistered**: Tags in ingress not in registry
5. **CursorTagLeakEcho**: Registered tags appearing in model output
6. **ReasoningLeakSuspected**: Heuristic detection of `<think>`, `Reasoning:`, etc.

### Phase 4: Debug API ✅
**File**: `src/main.rs`

**Endpoints**:
- `GET /debug/conversations` - List all conversations
- `GET /debug/conversation/:cid` - Get conversation summary
- `GET /debug/conversation/:cid/turn/:tid` - Get turn details
- `GET /debug/blob/:cid/:tid/:bid` - Fetch blob (lazy loading)

**Static UI Hosting**:
- Serves React build from `debug_ui/dist`
- Fallback routing for SPA navigation

### Phase 5: React UI ✅
**Directory**: `debug_ui/`

**Components**:
- **Sidebar**: Conversation list with timestamps and issue badges
- **Turn List**: Turns within selected conversation
- **Turn View**: 
  - Issue panel with severity indicators
  - Lifecycle stages table with blob size info
  - "View Blob" buttons for lazy loading

**Styling**: Dark theme (slate-900 bg) with emerald accents, responsive layout

## Key Features

### Per-Conversation View
- Browse conversations sorted by last update
- See issue counts at a glance (⚠️ badges)
- Drill down to individual turns

### Turn-Level Debugging
- **Lifecycle Timeline**: See all processing stages
- **Issue Detection**: Structured issue reporting with context
- **Tool Call Inventory**: Track tool calls with args status
- **Tag Tracking**: Registered vs unregistered Cursor tags

### Lazy Blob Loading
- Indexes always loaded (small JSON)
- Blobs fetched on demand via `/debug/blob/:cid/:tid/:bid`
- Supports large conversations without memory bloat

### No Parsing Duplication
- All lift/project logic stays in Rust
- UI only renders pre-computed data
- Replay endpoint ready for future enhancement

## File Structure

```
src/
├── debug_bundle.rs       # Bundle structs, manager, issue detection
├── tag_extract.rs        # Tag registry, extractor
├── main.rs               # Debug API routes, static UI hosting
├── streaming.rs          # Final turn capture
└── [existing files]

debug_ui/
├── src/
│   ├── App.tsx           # Main UI component
│   ├── index.css         # Tailwind styles
│   └── main.tsx
├── dist/                 # Built static files
├── package.json
├── tailwind.config.js
├── postcss.config.js
└── vite.config.ts
```

## Compilation Status

✅ `cargo check` - Passes
✅ `cargo clippy -- -D warnings` - Passes
✅ `npm run build` - Produces optimized dist/

## Usage

### Development
```bash
# Terminal 1: Run Rust backend
cargo run

# Terminal 2: Run React dev server (optional)
cd debug_ui && npm run dev
```

### Production
```bash
# Build React UI
cd debug_ui && npm run build

# Run Rust backend (serves static UI + API)
cargo run --release
```

### Access UI
- Navigate to `http://localhost:3000/debug/ui` (or configured port)
- Browse conversations and drill down to turn details

## Future Enhancements

1. **Replay Endpoint**: `POST /debug/replay/:cid/:tid` to re-run lift/project
2. **Blob Compression**: Implement zstd compression for large payloads
3. **Tag Highlighting**: Highlight tag ranges in blob viewer
4. **Advanced Filtering**: Filter turns by issue type, model, etc.
5. **Export**: Download conversation bundles for offline analysis
6. **Trace Integration**: Load NDJSON trace logs alongside bundles

## Testing Strategy

- Unit tests for tag extractor (nested/malformed tags)
- Bundle writer/reader roundtrip tests
- Manual: Run real conversation, verify UI displays stages/tags/issues

## Notes

- **On-disk only**: Easy to delete debug_capture/ directory for cleanup
- **Gated capture**: Behind `enable_debug_capture` flag + "capture on issue" fallback
- **Disk cleanup**: Existing cleanup logic in debug_utils.rs applies
- **No auth**: Local-only debugging tool (add auth if needed for production)

---

**Status**: ✅ All 5 phases complete and compiling successfully!

