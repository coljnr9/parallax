# Empty Tool Call Arguments Fix

## Problem
When Claude (or other LLMs) send tool calls with empty arguments `{}` despite required parameters being defined in the tool schema, Parallax was storing these invalid calls without feedback. This led to confusion and made debugging harder.

## Solution
Implemented automatic detection and synthetic error injection for empty tool call arguments.

## How It Works

### Detection (lines 341-403 in src/streaming.rs)
When a streaming response is finalized, Parallax checks all tool calls for empty arguments on "suspicious" tools - tools that are known to require parameters.

Suspicious tools include:
- `read_file` - requires `target_file`
- `grep` - requires `pattern`
- `list_dir` - requires `target_directory`
- `codebase_search` - requires `query`, `explanation`, `target_directories`
- `run_terminal_cmd` - requires `command`
- And many others...

### Synthetic Error Injection (lines 378-406)
When empty arguments are detected, Parallax automatically injects a synthetic `ToolResult` with:
- `is_error: true`
- A clear error message explaining the issue
- The tool call ID (so it matches the original call)

### Example Flow

**Before fix:**
1. Claude sends: `{"name": "read_file", "arguments": {}}`
2. Parallax stores it as-is
3. User sees empty tool call in debug UI
4. No error feedback to model
5. Conversation may continue with confusion

**After fix:**
1. Claude sends: `{"name": "read_file", "arguments": {}}`
2. Parallax detects empty arguments
3. Parallax logs warning: `Finalized turn has tool calls with empty args: read_file:toolu_xxx`
4. Parallax injects error result:
   ```json
   {
     "tool_call_id": "toolu_xxx",
     "content": "Error: Tool 'read_file' was called with empty arguments. This tool requires parameters. Please review the tool's schema and provide all required parameters.\n\nThis is likely a model error...",
     "is_error": true,
     "name": "read_file"
   }
   ```
5. Model receives error in next turn
6. Model can retry with correct arguments

## Benefits

1. **Better debugging**: Clear error messages in logs and debug UI
2. **Self-correction**: Model sees error and can retry correctly
3. **User visibility**: Errors are visible in Cursor's tool results
4. **Low complexity**: Simple validation, no retry loops or state management
5. **No extra cost**: No additional API calls
6. **Streaming-friendly**: Works with Parallax's streaming-first architecture

## Code Changes

### src/streaming.rs
- Line 323: Changed `finalized_turn` from immutable to mutable
- Lines 378-406: Added synthetic error injection loop

## Testing

The fix handles the exact case found in conversation `434e8780-335d-4a20-9e9b-de808bc8b2e4`:
- Tool: `read_file`
- ID: `toolu_bdrk_01W4Gt4g3quJ4XyAfqB72ZER`
- Arguments: `{}`
- Result: Now gets synthetic error injected

## Alternative Approaches Considered

1. **Full Retry**: Detect error, discard response, re-send request
   - ❌ Too complex (200-400 lines)
   - ❌ Doubles latency and cost
   - ❌ Breaks streaming architecture

2. **Do Nothing**: Just log warnings
   - ❌ Model never learns from mistake
   - ❌ Poor user experience

3. **Error Feedback** (Chosen)
   - ✅ Simple implementation (~28 lines)
   - ✅ Natural conversation flow
   - ✅ Model can self-correct
   - ✅ Maintains streaming performance

## Future Enhancements

Potential improvements if needed:
- Validate against actual tool schemas (check required fields dynamically)
- Add validation for non-empty but invalid arguments
- Track error injection stats in metrics
- Make suspicious tool list configurable

## References

- Original issue conversation: `cfbfdc71` / `434e8780-335d-4a20-9e9b-de808bc8b2e4`
- Tool call with empty args: `toolu_bdrk_01W4Gt4g3quJ4XyAfqB72ZER`
- Debug capture: `1767476638000_final_cfbfdc71_434e8780_anthropic_claude-haiku-4.5.json`

