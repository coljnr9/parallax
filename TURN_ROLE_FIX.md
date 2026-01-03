# Fix: Incorrect Turn Role Labeling in Debug UI

## Problem
The debug UI was labeling turns incorrectly. It was using a simple alternating pattern based on turn index (`idx % 2 === 0`) to determine if a turn was from "Cursor" or "Assistant". This caused turns to be mislabeled, especially when viewing conversations where the actual sender didn't match the alternating pattern.

**Example**: A turn from the Assistant was labeled as "Cursor" because it was at an even index.

## Root Cause
The React component had no actual data about who sent each turn. It was making assumptions based on the turn's position in the list rather than using real turn metadata.

## Solution

### Backend Changes (Rust)

#### 1. Added `role` field to `TurnSummary` (src/debug_bundle.rs)
```rust
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TurnSummary {
    pub turn_id: String,
    pub request_id: String,
    pub model_id: String,
    pub flavor: String,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub issues: IssueCounts,
    #[serde(default)]
    pub role: Option<String>,  // NEW
}
```

#### 2. Added `role` field to `TurnDetail` (src/debug_bundle.rs)
```rust
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TurnDetail {
    // ... existing fields ...
    #[serde(default)]
    pub role: Option<String>,  // NEW
}
```

#### 3. Updated TurnSummary creation (src/debug_bundle.rs)
When creating a TurnSummary from TurnDetail, now includes the role:
```rust
let turn_summary = TurnSummary {
    // ... existing fields ...
    role: detail.role.clone(),  // NEW
};
```

#### 4. Set role in initial turn creation (src/main.rs)
When a user sends a message, the turn is marked as "User":
```rust
let turn_detail = crate::debug_bundle::TurnDetail {
    // ... existing fields ...
    role: Some("User".to_string()),  // NEW
};
```

#### 5. Set role in streaming response (src/streaming.rs)
When the assistant responds, the turn is marked as "Assistant":
```rust
let detail = crate::debug_bundle::TurnDetail {
    // ... existing fields ...
    role: Some("Assistant".to_string()),  // NEW
};

let summary_update = crate::debug_bundle::TurnSummary {
    // ... existing fields ...
    role: Some("Assistant".to_string()),  // NEW
};
```

### Frontend Changes (React)

#### 1. Updated TurnSummary interface (debug_ui/src/App.tsx)
```typescript
interface TurnSummary {
  // ... existing fields ...
  role?: string;  // NEW
}
```

#### 2. Updated turn rendering logic (debug_ui/src/App.tsx)
Changed from index-based alternation to using actual role data:

**Before:**
```typescript
<div className={`px-1.5 py-0.5 rounded border flex items-center gap-1 ${idx % 2 === 0 ? 'bg-blue-950/30 border-blue-500/20 text-blue-400' : 'bg-purple-950/30 border-purple-500/20 text-purple-400'}`}>
  <span className="text-[10px] font-bold font-mono">
    {idx % 2 === 0 ? 'Cursor' : 'Assistant'}
  </span>
</div>
```

**After:**
```typescript
<div className={`px-1.5 py-0.5 rounded border flex items-center gap-1 ${turn.role === 'Assistant' ? 'bg-purple-950/30 border-purple-500/20 text-purple-400' : 'bg-blue-950/30 border-blue-500/20 text-blue-400'}`}>
  <span className="text-[10px] font-bold font-mono">
    {turn.role || (idx % 2 === 0 ? 'Cursor' : 'Assistant')}
  </span>
</div>
```

The fallback to index-based alternation is kept for backward compatibility with older debug bundles that don't have role data.

## Result
Now when you view a conversation in the debug UI:
- ✅ User turns are correctly labeled as "User" (blue badge)
- ✅ Assistant turns are correctly labeled as "Assistant" (purple badge)
- ✅ Labels match the actual sender, not the turn index
- ✅ Backward compatible with existing debug bundles

## Testing
```bash
# Build the project
cargo check
cargo clippy -- -D warnings
cd debug_ui && npm run build

# Run the server
cargo run --release

# Make a test request and verify turn labels in the debug UI
```

## Files Changed
- `src/debug_bundle.rs` - Added role field to TurnSummary and TurnDetail
- `src/main.rs` - Set role to "User" for initial turns
- `src/streaming.rs` - Set role to "Assistant" for response turns
- `debug_ui/src/App.tsx` - Updated UI to use role field instead of index-based alternation

