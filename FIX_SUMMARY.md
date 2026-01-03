# Fix Summary: Missing Initial Conversation Messages in Debug UI

## Problem
You weren't seeing the initial conversation kick-off messages (like "I'm still not seeing...") in the Parallax debug UI. Only the `final` stage was showing up in turn.json, missing `ingress_raw` and `projected` stages.

## Root Cause
Found **two bugs**:

### Bug 1: Stage Overwrite in streaming.rs
**Location**: `src/streaming.rs` lines 91-92

The code was:
```rust
let _ = bundle_manager.merge_and_write_turn(conversation_id, tid, &detail).await;
let _ = bundle_manager.update_summaries(conversation_id, tid, &detail).await;
```

**Problem**: 
1. Line 91 correctly merged existing stages (`ingress_raw`, `projected`) with new stage (`final`)
2. Line 92 then called `update_summaries()` which **overwrote** the turn.json with only the `final` stage

**Flow**:
- `merge_and_write_turn()` → writes `[ingress_raw, projected, final]` ✓
- `update_summaries()` → writes `[final]` ✗ (OVERWRITES!)

### Bug 2: ingress_raw Blob Not Written
**Location**: `src/main.rs` line 284

The code was:
```rust
if state.args.enable_debug_capture {
    let _ = bundle_manager.write_blob(&cid, &tid, "ingress_raw", payload.to_string().as_bytes()).await;
}
```

**Problem**: The `ingress_raw` blob (containing your initial message) was only written when `enable_debug_capture=true`, but the default is `false`. So the blob file never existed even though the stage was referenced.

## Solution

### Fix 1: Prevent Stage Overwrite
**File**: `src/streaming.rs`

Changed line 92 to only update the conversation summary, not rewrite the turn detail:
```rust
let _ = bundle_manager.merge_and_write_turn(conversation_id, tid, &detail).await;

// Update conversation summary (but don't re-write turn.json, merge_and_write_turn already did that)
let summary_update = crate::debug_bundle::TurnSummary { /* ... */ };
let _ = bundle_manager.update_conversation_summary_only(conversation_id, summary_update).await;
```

### Fix 2: Always Write ingress_raw Blob
**File**: `src/main.rs`

Removed the conditional check so the blob is always written:
```rust
// Always write ingress_raw blob so users can see their messages
let _ = bundle_manager.write_blob(&cid, &tid, "ingress_raw", payload.to_string().as_bytes()).await;
```

### Supporting Changes
**File**: `src/debug_bundle.rs`

Added new method `update_conversation_summary_only()` that updates the conversation.json summary without touching turn.json.

## Result
Now when you view a conversation in the debug UI, you'll see all three stages:
1. **ingress_raw**: The raw OpenAI-format API request with your initial message
2. **projected**: The transformed request sent upstream (includes all conversation context)
3. **final**: The assistant's response

## Bonus Fix
Fixed CLI argument conflict where `-h` was used for both `host` and `help` (clap auto-generates `-h` for help). Removed short flags from `--port`, `--host`, and `--database`.

## Testing
To test, start Parallax and make a new conversation request:
```bash
OPENROUTER_API_KEY=your-key ./target/debug/parallax --port 8081
```

Then check the debug UI at `http://localhost:8081/debug/ui` - you should now see all stages including your initial message in the `ingress_raw` blob.

## Files Changed
- `src/streaming.rs` - Fixed stage overwrite bug
- `src/main.rs` - Always write ingress_raw blob
- `src/debug_bundle.rs` - Added update_conversation_summary_only() method
- `src/main_helper.rs` - Fixed CLI argument conflict

