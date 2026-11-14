-- Collected overview aggregates pushed by member machines.
CREATE TABLE IF NOT EXISTS overview_collected_calls (
    source_id TEXT NOT NULL,
    call_id TEXT NOT NULL,
    source_name TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    finished_at_ms INTEGER NOT NULL,
    request_type TEXT NOT NULL,
    model_key TEXT NOT NULL,
    display_name TEXT NOT NULL,
    status TEXT NOT NULL,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cache_read_tokens INTEGER,
    cache_write_tokens INTEGER,
    PRIMARY KEY (source_id, call_id)
);

CREATE INDEX IF NOT EXISTS overview_collected_created
ON overview_collected_calls(created_at_ms);

CREATE INDEX IF NOT EXISTS overview_collected_model
ON overview_collected_calls(model_key, created_at_ms);

CREATE TABLE IF NOT EXISTS overview_sources (
    source_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    last_push_at_ms INTEGER NOT NULL,
    call_count INTEGER NOT NULL DEFAULT 0
);