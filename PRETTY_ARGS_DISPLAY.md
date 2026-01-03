# Improvement: Pretty-Printed Tool Arguments Display

## Overview
Enhanced the tool call arguments display in the debug UI to show arguments in a human-readable format (`arg_name: value`) instead of raw JSON.

## Problem
Previously, tool call arguments were displayed as compact JSON strings like:
```
{"file_path":"/home/cole/rust/feat-debug-utils/deb...","new_string":"...","old_string":"..."}
```

This was hard to read and didn't clearly show which value belonged to which argument.

## Solution

### New Formatting Function: `formatArgsPreview()`

Added a helper function that:
1. Parses JSON arguments
2. Extracts key-value pairs
3. Formats as `key: value, key: value, ...`
4. Truncates long values (30 chars) with ellipsis
5. Truncates overall preview (60 chars) with ellipsis
6. Falls back gracefully if JSON parsing fails

```typescript
function formatArgsPreview(argsSource: string | undefined): string {
  if (!argsSource) return 'N/A';
  
  try {
    const args = JSON.parse(argsSource);
    if (typeof args !== 'object' || args === null) {
      return String(args);
    }
    
    // Format as "key1: value1, key2: value2, ..."
    const pairs = Object.entries(args)
      .map(([key, value]) => {
        let displayValue: string;
        if (typeof value === 'string') {
          displayValue = value.length > 30 ? value.slice(0, 30) + '...' : value;
        } else if (typeof value === 'object') {
          displayValue = JSON.stringify(value).slice(0, 30) + '...';
        } else {
          displayValue = String(value);
        }
        return `${key}: ${displayValue}`;
      })
      .join(', ');
    
    return pairs.length > 60 ? pairs.slice(0, 60) + '...' : pairs;
  } catch {
    return argsSource.length > 60 ? argsSource.slice(0, 60) + '...' : argsSource;
  }
}
```

### Updated Tool Call Table

Changed the args preview in the tool calls table to use the new formatter:

**Before:**
```typescript
const argsPreview = (() => {
  try {
    const argsSource = tc.evidence.raw_arguments_snippet || tc.evidence.snippet;
    if (!argsSource) return 'N/A';
    const args = JSON.parse(argsSource);
    const argsStr = JSON.stringify(args);
    return argsStr.length > 50 ? argsStr.slice(0, 50) + '...' : argsStr;
  } catch {
    return tc.evidence.snippet || 'N/A';
  }
})();
```

**After:**
```typescript
const argsPreview = formatArgsPreview(tc.evidence.raw_arguments_snippet || tc.evidence.snippet);
```

## Result

### Example Transformation

**Before:**
```
{"file_path":"/home/cole/rust/feat-debug-utils/deb...
```

**After:**
```
file_path: /home/cole/rust/feat-debug-utils/deb..., new_string: ..., old_string: ...
```

Much clearer! Now you can immediately see:
- ✅ What arguments the tool call has
- ✅ What values were passed
- ✅ Which argument is which

### Features

- ✅ Human-readable format with clear key-value pairs
- ✅ Intelligent truncation of long values
- ✅ Graceful fallback for non-JSON arguments
- ✅ Consistent with rest of debug UI styling
- ✅ Full arguments still visible in expanded tool call analysis panel

## Testing

To test this improvement:

1. Start the server:
   ```bash
   cargo run --release
   ```

2. Make a request that triggers tool calls with multiple arguments

3. Open the debug UI at `http://localhost:3000/debug/ui`

4. Navigate to a turn with tool calls

5. Look at the "Args" column in the Tool Calls table

6. You should see nicely formatted arguments like:
   ```
   file_path: /path/to/file, new_string: content..., old_string: original...
   ```

## Files Changed
- `debug_ui/src/App.tsx` - Added `formatArgsPreview()` function and updated tool call table

## Backward Compatibility

✅ Fully backward compatible
✅ Works with all existing debug bundles
✅ Gracefully handles edge cases (non-JSON, missing args, etc.)

## Future Enhancements

1. **Syntax Highlighting**: Color-code argument names and values
2. **Type Indicators**: Show argument types (string, number, object, etc.)
3. **Validation**: Highlight invalid or suspicious argument values
4. **Comparison**: Compare arguments across multiple tool calls
5. **Copy to Clipboard**: Quick copy button for individual arguments

