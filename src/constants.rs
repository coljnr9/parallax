pub const RETRYABLE_STATUS_CODES: &[u16] = &[429, 500, 502, 503, 504, 520];

/// Tools that require arguments - used to detect suspicious empty argument tool calls
pub const TOOLS_REQUIRING_ARGS: &[&str] = &[
    "read_file",
    "grep",
    "glob_file_search",
    "list_dir",
    "codebase_search",
    "run_terminal_cmd",
    "web_search",
    "fetch_mcp_resource",
    "mcp_context7_query-docs",
    "mcp_context7_resolve-library-id",
    "mcp_docfork_docfork_search_docs",
    "mcp_docfork_docfork_read_url",
    "mcp_cursor-ide-browser_browser_click",
    "mcp_cursor-ide-browser_browser_type",
    "mcp_cursor-ide-browser_browser_select_option",
    "mcp_cursor-ide-browser_browser_press_key",
    "mcp_cursor-ide-browser_browser_navigate",
];

/// Fallback model for Gemini when primary model fails
pub const GEMINI_FLASH_FALLBACK: &str = "google/gemini-3-flash-preview-0814";

/// OpenRouter API endpoints
pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const OPENROUTER_CHAT_COMPLETIONS: &str = "https://openrouter.ai/api/v1/chat/completions";

/// Database defaults
pub const DB_CLEANUP_RETENTION_DAYS: i64 = 7;
pub const DB_BUSY_TIMEOUT_MS: u32 = 5000;
pub const DB_PRAGMAS: &[&str] = &[
    "PRAGMA journal_mode = WAL",
    "PRAGMA synchronous = NORMAL",
    "PRAGMA busy_timeout = 5000",
];

/// Diff detection markers
pub const DIFF_MARKERS: &[&str] = &[
    "diff --git ",
    "--- ",
    "+++ ",
    "@@ -",
    "Index: ",
    "Property changes on: ",
];

/// Forbidden terms in plans that could trigger execution failures
pub const FORBIDDEN_PLAN_TERMS: &[(&str, &str)] = &[
    ("npm install", "package manager install"),
    ("npm build", "package manager build"),
    ("cargo build", "rust build"),
    ("cargo check", "rust check"),
    ("grep ", "ripgrep "),
];

/// Intent detection keywords
pub const PLAN_KEYWORDS: &[&str] = &[" PLAN MODE", " PLANNING MODE"];
pub const AGENT_KEYWORDS: &[&str] = &[" AGENT MODE", " COMPOSER MODE", " BUILD MODE"];
pub const DEBUG_KEYWORDS: &[&str] = &[" DEBUG MODE"];
pub const ASK_KEYWORDS: &[&str] = &[" ASK MODE", " CHAT MODE"];
