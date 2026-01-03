# Feature: Message Tags Parser and Display

## Overview
Added a new feature to the debug UI that automatically parses XML-like tags from message content and displays them in a collapsible table format. This makes it easy to inspect structured metadata that's embedded in messages (like `<user_info>`, `<rules>`, `<project_layout>`, etc.).

## Problem Solved
When debugging conversations, users need to see what context/metadata was sent with each request. Previously, this information was buried in the raw message text. Now it's extracted and presented in an organized, collapsible format.

## Implementation

### 1. XML Tag Parser (debug_ui/src/App.tsx)

Added a `parseXmlTags()` function that:
- Uses regex to find all `<tagname>...</tagname>` patterns
- Extracts tag name, content, and attributes
- Returns an array of `ParsedTag` objects

```typescript
interface ParsedTag {
  name: string;
  content: string;
  attributes?: Record<string, string>;
}

function parseXmlTags(text: string): ParsedTag[] {
  const tags: ParsedTag[] = [];
  const tagRegex = /<(\w+)([^>]*)>([\s\S]*?)<\/\1>/g;
  // ... parsing logic ...
  return tags;
}
```

**Features:**
- ✅ Handles nested content (including newlines)
- ✅ Extracts attributes from opening tags
- ✅ Supports any tag name (alphanumeric)
- ✅ Robust regex with backreference for matching closing tags

### 2. Tag Display Component (debug_ui/src/App.tsx)

Created `TagsDisplay` component that:
- Shows all parsed tags in a table-like layout
- Each tag is collapsible with expand/collapse button
- Displays tag name, attributes, and content length
- Shows full content when expanded in a scrollable pre-formatted block

**Features:**
- ✅ Collapsible sections for each tag
- ✅ Shows character count for each tag
- ✅ Displays attributes inline
- ✅ Syntax highlighting with monospace font
- ✅ Scrollable content area for large tags
- ✅ Dark theme styling consistent with rest of UI

### 3. Integration into Turn View

Added tag display section after the "User Query" section in the turn details view:

```typescript
{/* Message Tags Section */}
{turnDetail.user_query && (() => {
  const tags = parseXmlTags(turnDetail.user_query);
  return tags.length > 0 ? <TagsDisplay tags={tags} /> : null;
})()}
```

**Placement:**
1. User Query (raw message text)
2. **Message Tags** (parsed and organized) ← NEW
3. Trace Timeline
4. Issues Detected
5. Tool Calls
6. Cursor Tags
7. Lifecycle Stages

## Example Usage

When a message contains:
```xml
<user_info>
OS Version: linux 6.14.0-37-generic
Current Date: Saturday Jan 3, 2026
Shell: /usr/bin/fish
</user_info>

<rules>
The rules section has a number of possible rules/memories/context...
</rules>

<project_layout>
Below is a snapshot of the current workspace's file structure...
</project_layout>
```

The UI will display:
- **Message Tags** section with 3 tags
  - `<user_info>` - 89 chars - [collapsible]
  - `<rules>` - 245 chars - [collapsible]
  - `<project_layout>` - 1,234 chars - [collapsible]

Click any tag to expand and see its full content.

## UI Design

### Tag List Header
- Shows total tag count
- Consistent with other sections (Cursor Tags, Tool Calls, etc.)
- Uses Tag icon from lucide-react

### Tag Row
- Tag name in emerald color (monospace)
- Attributes displayed inline (if present)
- Character count on the right
- Hover effect for better UX
- Expand/collapse arrow that rotates

### Expanded Content
- Full tag content in monospace font
- Scrollable if content is large (max-height: 24rem)
- Dark background to distinguish from collapsed state
- Preserves whitespace and formatting

## Styling

Uses Tailwind CSS classes consistent with existing debug UI:
- Dark theme (slate-900, slate-950)
- Emerald accents for tag names
- Hover states for interactivity
- Responsive layout

## Testing

To test this feature:

1. Start the Parallax server:
   ```bash
   cargo run --release
   ```

2. Make a request with XML-like tags in the message:
   ```bash
   curl -X POST http://localhost:3000/v1/chat/completions \
     -H "Content-Type: application/json" \
     -H "Authorization: Bearer test" \
     -d '{
       "model": "openrouter/auto",
       "messages": [{
         "role": "user",
         "content": "<user_info>\nTest info\n</user_info>\n\n<context>\nSome context\n</context>"
       }]
     }'
   ```

3. Open the debug UI at `http://localhost:3000/debug/ui`

4. Select the conversation and turn

5. Scroll to the "Message Tags" section

6. Click on tags to expand/collapse their content

## Files Changed
- `debug_ui/src/App.tsx` - Added parser, component, and integration

## Future Enhancements

1. **Syntax Highlighting**: Add language-specific highlighting for tag content
2. **Search**: Filter tags by name or content
3. **Export**: Download tag content as separate files
4. **Diff View**: Compare tags across different turns
5. **Tag Statistics**: Show which tags appear most frequently
6. **Custom Parsers**: Support for other markup formats (JSON, YAML, etc.)

## Backward Compatibility

✅ Fully backward compatible - if no tags are found, the section doesn't display
✅ Works with existing debug bundles
✅ No changes to backend or data structures required

