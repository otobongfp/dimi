-- JSON array of {"name", "path"} objects for real files surfaced by tool
-- calls during this turn, so the frontend can render clickable file links
-- instead of guessing filenames out of the raw LLM text. NULL when a message
-- has no associated sources (e.g. user messages, general-knowledge answers).
ALTER TABLE messages ADD COLUMN sources TEXT;
