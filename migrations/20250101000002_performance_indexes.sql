-- Drop unused usage_stats table (requested removal)
DROP TABLE IF EXISTS usage_stats;

-- Index for cleanup queries and time-based sorting
CREATE INDEX IF NOT EXISTS idx_tool_signatures_created_at ON tool_signatures(created_at);

