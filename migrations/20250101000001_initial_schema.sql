-- Create conversation_states table
CREATE TABLE IF NOT EXISTS conversation_states (
    id TEXT PRIMARY KEY NOT NULL,
    state_json TEXT NOT NULL,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP NOT NULL
);

-- Create tool_signatures table with reasoning and thought support
CREATE TABLE IF NOT EXISTS tool_signatures (
    id TEXT PRIMARY KEY NOT NULL,
    conversation_id TEXT NOT NULL,
    signature TEXT NOT NULL,
    reasoning_tokens INTEGER,
    thought_signature TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP NOT NULL
);

-- Index for conversation lookups
CREATE INDEX IF NOT EXISTS idx_tool_signatures_conversation_id ON tool_signatures(conversation_id);

