PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;

CREATE TABLE IF NOT EXISTS event_sequence (
    aggregate_id TEXT PRIMARY KEY,
    seq          INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS event (
    id           TEXT PRIMARY KEY,
    aggregate_id TEXT NOT NULL,
    seq          INTEGER NOT NULL,
    event_type   TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    UNIQUE (aggregate_id, seq)
);

CREATE TABLE IF NOT EXISTS session (
    id                TEXT PRIMARY KEY,
    parent_id         TEXT,
    fork_source_id    TEXT,
    title             TEXT NOT NULL,
    title_is_default  INTEGER NOT NULL DEFAULT 1,
    agent_id          TEXT NOT NULL,
    model_profile     TEXT NOT NULL,
    cwd               TEXT NOT NULL,
    workspace_root    TEXT NOT NULL,
    active_branch     TEXT,
    claim_owner       TEXT,
    resume_attempts   INTEGER NOT NULL DEFAULT 0,
    revert_staged_seq INTEGER,
    revert_orig_snap  TEXT,
    total_cost_usd    REAL NOT NULL DEFAULT 0.0,
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS session_message (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    parent_msg_id   TEXT,
    seq             INTEGER NOT NULL,
    msg_type        TEXT NOT NULL,
    model_provider  TEXT,
    model_id        TEXT,
    finish_reason   TEXT,
    error_json      TEXT,
    usage_json      TEXT,
    snapshot_start  TEXT,
    changed_files   TEXT,
    data_json       TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    UNIQUE (session_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_msg_session_seq ON session_message(session_id, seq);
CREATE INDEX IF NOT EXISTS idx_msg_parent ON session_message(session_id, parent_msg_id);

CREATE TABLE IF NOT EXISTS session_inbox (
    id           TEXT PRIMARY KEY,
    session_id   TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    item_type    TEXT NOT NULL,
    delivery     TEXT NOT NULL,
    enqueued_seq INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_inbox_session_seq ON session_inbox(session_id, enqueued_seq);

CREATE TABLE IF NOT EXISTS context_view_epoch (
    session_id      TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    epoch_id        INTEGER NOT NULL,
    reason          TEXT NOT NULL,
    pruned_map_json TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (session_id, epoch_id)
);

CREATE TABLE IF NOT EXISTS instruction_blob (
    hash       TEXT PRIMARY KEY,
    value_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS instruction_state (
    session_id          TEXT PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,
    epoch_seq           INTEGER NOT NULL,
    initial_hashes_json TEXT NOT NULL,
    current_hashes_json TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS permission_saved (
    id         TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    action     TEXT NOT NULL,
    resource   TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE (project_id, action, resource)
);

CREATE TABLE IF NOT EXISTS subagent_outbox (
    id                TEXT PRIMARY KEY,
    parent_session_id TEXT NOT NULL,
    child_session_id  TEXT NOT NULL,
    payload_xml       TEXT NOT NULL,
    delivered         INTEGER NOT NULL DEFAULT 0,
    created_at        INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_subagent_outbox_pending ON subagent_outbox(delivered, created_at);

CREATE TABLE IF NOT EXISTS step_metrics (
    id                  TEXT PRIMARY KEY,
    session_id          TEXT NOT NULL,
    step_num            INTEGER NOT NULL,
    model_profile       TEXT NOT NULL,
    edit_strategy       TEXT NOT NULL,
    non_cached_in       INTEGER NOT NULL,
    cache_read_in       INTEGER NOT NULL,
    cache_write_in      INTEGER NOT NULL,
    output_tokens       INTEGER NOT NULL,
    reasoning_tokens    INTEGER NOT NULL,
    cost_usd            REAL NOT NULL,
    ttft_ms             INTEGER NOT NULL,
    wall_ms             INTEGER NOT NULL,
    prefix_preserved    INTEGER NOT NULL,
    edit_attempts       INTEGER NOT NULL DEFAULT 0,
    edit_first_try_ok   INTEGER NOT NULL DEFAULT 0,
    lsp_errors_caught   INTEGER NOT NULL DEFAULT 0,
    raw_tool_bytes      INTEGER NOT NULL DEFAULT 0,
    context_tool_bytes  INTEGER NOT NULL DEFAULT 0,
    created_at          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_step_metrics_session_step ON step_metrics(session_id, step_num);

-- Durable read state for Delta Re-Read (read_diff)
CREATE TABLE IF NOT EXISTS session_file_read_state (
    session_id      TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    canonical_path  TEXT NOT NULL,
    last_blake3     TEXT NOT NULL,
    last_line_count INTEGER NOT NULL,
    snapshot_zst    BLOB,
    read_epoch      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    PRIMARY KEY (session_id, canonical_path)
);

-- Durable background shell & subagent job state
CREATE TABLE IF NOT EXISTS background_job (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL REFERENCES session(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL,
    command_preview TEXT NOT NULL,
    status          TEXT NOT NULL,
    exit_code       INTEGER,
    spill_path      TEXT,
    started_at      INTEGER NOT NULL,
    ended_at        INTEGER
);
CREATE INDEX IF NOT EXISTS idx_bg_job_session_status ON background_job(session_id, status);

CREATE VIRTUAL TABLE IF NOT EXISTS session_fts USING fts5(session_id, message_id, content);
