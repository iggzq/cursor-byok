-- Give imported history stable export identities and per-event push state.
CREATE TABLE cursor_usage_events_v2 (
    event_key TEXT PRIMARY KEY,
    created_at_ms INTEGER NOT NULL,
    model_key TEXT NOT NULL,
    display_name TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    pushed_at_ms INTEGER
);

INSERT INTO cursor_usage_events_v2(
    event_key, created_at_ms, model_key, display_name, conversation_id,
    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens
)
SELECT printf('cursor-legacy:%d', rowid), created_at_ms, model_key, display_name,
       conversation_id, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens
FROM cursor_usage_events;

DROP TABLE cursor_usage_events;
ALTER TABLE cursor_usage_events_v2 RENAME TO cursor_usage_events;

CREATE INDEX cursor_usage_created
ON cursor_usage_events(created_at_ms);

CREATE INDEX cursor_usage_model
ON cursor_usage_events(model_key, created_at_ms);

CREATE INDEX cursor_usage_pending
ON cursor_usage_events(pushed_at_ms, created_at_ms, event_key);

ALTER TABLE local_agent_usage_events ADD COLUMN pushed_at_ms INTEGER;

CREATE INDEX local_agent_usage_pending
ON local_agent_usage_events(pushed_at_ms, created_at_ms, event_key);
