-- Token usage imported from local Codex and Claude session logs.
CREATE TABLE IF NOT EXISTS local_agent_usage_events (
    event_key TEXT PRIMARY KEY,
    source TEXT NOT NULL CHECK (source IN ('codex', 'claude_code', 'claude_cli')),
    created_at_ms INTEGER NOT NULL,
    model_key TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    output_tokens INTEGER NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    cache_read_tokens INTEGER NOT NULL DEFAULT 0 CHECK (cache_read_tokens >= 0),
    cache_write_tokens INTEGER NOT NULL DEFAULT 0 CHECK (cache_write_tokens >= 0)
);

CREATE INDEX IF NOT EXISTS local_agent_usage_created
ON local_agent_usage_events(created_at_ms);

CREATE INDEX IF NOT EXISTS local_agent_usage_source_created
ON local_agent_usage_events(source, created_at_ms);
