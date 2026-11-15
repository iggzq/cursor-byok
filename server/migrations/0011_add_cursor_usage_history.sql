-- Usage records imported from the Cursor account usage API.
-- A sync replaces the whole table, so rows need no natural key: the API only
-- exposes timestamp + model + conversation, which is not a unique call identity.
CREATE TABLE IF NOT EXISTS cursor_usage_events (
    created_at_ms INTEGER NOT NULL,
    model_key TEXT NOT NULL,
    display_name TEXT NOT NULL,
    conversation_id TEXT NOT NULL,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS cursor_usage_created
ON cursor_usage_events(created_at_ms);

CREATE INDEX IF NOT EXISTS cursor_usage_model
ON cursor_usage_events(model_key, created_at_ms);
