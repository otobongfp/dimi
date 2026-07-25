
CREATE TABLE repositories (
    id              TEXT PRIMARY KEY,
    kind            TEXT NOT NULL,
    root            TEXT NOT NULL,
    owning_plugin   TEXT REFERENCES plugins(id),
    created_at      INTEGER NOT NULL
);

CREATE TABLE documents (
    id              TEXT PRIMARY KEY,
    repository_id   TEXT NOT NULL REFERENCES repositories(id),
    source_path     TEXT NOT NULL,
    mime_type       TEXT NOT NULL,
    hash            TEXT NOT NULL,
    status          TEXT NOT NULL,
    imported_at     INTEGER NOT NULL,
    indexed_at      INTEGER
);

CREATE TABLE chunks (
    id              TEXT PRIMARY KEY,
    document_id     TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    ordinal         INTEGER NOT NULL,
    text            TEXT NOT NULL,
    token_count     INTEGER NOT NULL
);

-- sqlite-vec virtual table. Dimension fixed to the active EmbeddingEngine's
-- dimensions() (384 for the default bge-small-en-v1.5) — a model change
-- that alters dimensionality forces a full reindex() rather than a schema
-- migration.
CREATE VIRTUAL TABLE chunk_embeddings USING vec0(
    chunk_id        TEXT PRIMARY KEY,
    embedding       FLOAT[384]
);

CREATE TABLE models (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    backend         TEXT NOT NULL,
    sha256          TEXT NOT NULL,
    status          TEXT NOT NULL,
    size_bytes      INTEGER NOT NULL,
    installed_at    INTEGER
);

CREATE TABLE plugins (
    id              TEXT PRIMARY KEY,
    manifest_version TEXT NOT NULL,
    state           TEXT NOT NULL,
    installed_at    INTEGER NOT NULL
);

CREATE TABLE workspaces (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    system_prompt   TEXT NOT NULL,
    owning_plugin   TEXT REFERENCES plugins(id),
    created_at      INTEGER NOT NULL
);

CREATE TABLE workspace_repositories (
    workspace_id    TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    repository_id   TEXT NOT NULL REFERENCES repositories(id),
    PRIMARY KEY (workspace_id, repository_id)
);

CREATE TABLE workspace_tools (
    workspace_id    TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    tool_name       TEXT NOT NULL,
    PRIMARY KEY (workspace_id, tool_name)
);

-- A conversation isn't locked to one workspace/library; conversation_workspaces
-- below is the source of truth for which ones it can draw on, chosen
-- explicitly when the chat starts. Deleting a workspace only removes its row
-- there — a conversation with other libraries attached keeps its history.
CREATE TABLE conversations (
    id              TEXT PRIMARY KEY,
    title           TEXT,
    created_at      INTEGER NOT NULL
);

CREATE TABLE conversation_workspaces (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    PRIMARY KEY (conversation_id, workspace_id)
);

CREATE TABLE messages (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role            TEXT NOT NULL,
    content         TEXT NOT NULL,
    visible         INTEGER NOT NULL DEFAULT 1,
    created_at      INTEGER NOT NULL
);

CREATE TABLE tool_calls (
    id              TEXT PRIMARY KEY,
    message_id      TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    tool_name       TEXT NOT NULL,
    args            TEXT NOT NULL,
    result          TEXT,
    status          TEXT NOT NULL
);

CREATE TABLE jobs (
    id              TEXT PRIMARY KEY,
    kind            TEXT NOT NULL,
    status          TEXT NOT NULL,
    priority        INTEGER NOT NULL,
    created_at      INTEGER NOT NULL,
    completed_at    INTEGER,
    error_message   TEXT
);

CREATE TABLE config (
    key             TEXT PRIMARY KEY,
    value           TEXT NOT NULL
);

CREATE TABLE audit_log (
    id              TEXT PRIMARY KEY,
    actor           TEXT NOT NULL,
    action          TEXT NOT NULL,
    subject         TEXT,
    created_at      INTEGER NOT NULL
);
