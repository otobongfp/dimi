use crate::common::{
    ContextRequest, ConversationId, DimiError, MessageId, ResourceClass, Result, SqlValue,
    TokenStream,
};
use crate::kernel::events::{topics, EventBus};
use crate::services::context::ContextEngine;
use crate::services::inference::InferenceEngine;
use crate::services::scheduler::TokioSchedulerService;
use crate::services::storage::StorageEngine;
use crate::services::telemetry::TelemetryService;
use crate::services::tool::ToolEngine;
use std::sync::Arc;
use tokio::sync::mpsc as tokio_mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::StreamExt;

// Was 6 — too low for real multi-step work (search several documents, read
// each, compute totals, write a report, convert it). 20 still bounds a
// genuinely stuck loop; it stops being a ceiling real tasks bump into.
const MAX_TOOL_ITERATIONS: usize = 20;
const TOOL_CALL_OPEN: &str = "<tool_call>";

/// How much of `text` is safe to forward right now, withholding any trailing
/// substring that could still grow into a complete `pattern` match once more
/// tokens arrive — so a tag split across streamed pieces is never partially
/// leaked to the frontend before we know whether it's really forming.
fn safe_stream_len(text: &str, pattern: &str) -> usize {
    let max_check = pattern.len().saturating_sub(1).min(text.len());
    for k in (1..=max_check).rev() {
        let idx = text.len() - k;
        if text.is_char_boundary(idx) && text.as_bytes()[idx..] == pattern.as_bytes()[..k] {
            return idx;
        }
    }
    text.len()
}

#[derive(Clone)]
pub struct ChatTurnDeps {
    pub context: Arc<dyn ContextEngine>,
    pub inference: Arc<dyn InferenceEngine>,
    pub tools: Arc<dyn ToolEngine>,
    pub telemetry: Arc<dyn TelemetryService>,
    pub storage: Arc<dyn StorageEngine>,
    pub events: EventBus,
    pub scheduler: Arc<TokioSchedulerService>,
}

pub fn run_chat_turn(deps: ChatTurnDeps, conversation_id: ConversationId, query: String) -> TokenStream {
    let (tx, rx) = tokio_mpsc::unbounded_channel();
    tokio::spawn(drive_chat_turn(deps, conversation_id, query, tx));
    Box::pin(UnboundedReceiverStream::new(rx))
}

async fn drive_chat_turn(
    deps: ChatTurnDeps,
    conversation_id: ConversationId,
    query: String,
    tx: tokio_mpsc::UnboundedSender<Result<String>>,
) {
    let request = ContextRequest {
        query,
        conversation_id,
    };
    let mut last_ts = now_unix() - 1;
    // Real file paths surfaced by tool results during this turn (e.g. a
    // file_search hit, a read_file target) — accumulated across every
    // tool-call iteration so the final answer can cite exactly the files
    // the model actually touched, not filenames guessed out of its prose.
    let mut turn_sources: Vec<(String, String)> = Vec::new();

    for _ in 0..MAX_TOOL_ITERATIONS {
        let context = match deps.context.build_context(request.clone()).await {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        };

        let inference = deps.inference.clone();
        let telemetry = deps.telemetry.clone();
        let tx_for_stream = tx.clone();
        let generation = deps
            .scheduler
            .run("inference.chat", ResourceClass::Gpu, 0, async move {
                let mut stream = inference.generate(context).await?;
                let mut full = String::new();
                // Forward tokens to the frontend as they arrive, except while
                // a `<tool_call>` block is (or might be) forming — that JSON
                // is an internal step, never meant to reach the visible
                // chat log (see `extract_tool_call` below).
                let mut sent_len = 0usize;
                let mut suppressing = false;
                // Timed from the *second* piece onward, deliberately excluding
                // time-to-first-token (prompt prefill) — this is meant to be a
                // steady-state decode rate for the Settings panel and the
                // thermal/context feedback loop, not a prefill-skewed number.
                let mut decode_started: Option<std::time::Instant> = None;
                let mut decoded_after_first: u64 = 0;
                while let Some(piece) = stream.next().await {
                    match decode_started {
                        None => decode_started = Some(std::time::Instant::now()),
                        Some(_) => decoded_after_first += 1,
                    }
                    full.push_str(&piece?);
                    if suppressing {
                        continue;
                    }
                    let unsent = &full[sent_len..];
                    if unsent.contains(TOOL_CALL_OPEN) {
                        suppressing = true;
                        continue;
                    }
                    let safe = safe_stream_len(unsent, TOOL_CALL_OPEN);
                    if safe > 0 {
                        let _ = tx_for_stream.send(Ok(unsent[..safe].to_string()));
                        sent_len += safe;
                    }
                }
                if let Some(started) = decode_started {
                    let elapsed = started.elapsed().as_secs_f64();
                    if elapsed > 0.0 && decoded_after_first > 0 {
                        telemetry.record_metric(
                            "inference.tokens_per_sec",
                            decoded_after_first as f64 / elapsed,
                            &[],
                        );
                    }
                }
                Ok(full)
            })
            .await;
        let full = match generation {
            Ok(f) => f,
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        };

        match extract_tool_call(&full) {
            Some((name, arguments)) => {
                // Internal loop trace, not a real chat turn — invisible to the frontend.
                persist_message(&deps.storage, conversation_id, "assistant", &full, false, &mut last_ts, None).await;

                deps.events.publish(
                    topics::TOOL_INVOKED,
                    serde_json::json!({ "name": name, "arguments": arguments }),
                );
                let result = deps.tools.invoke(&name, arguments).await;
                let (result_json, ok) = match &result {
                    Ok(r) => {
                        collect_sources(&r.content, &mut turn_sources);
                        (r.content.clone(), true)
                    }
                    Err(e) => (serde_json::json!({ "error": e.to_string() }), false),
                };
                deps.events.publish(
                    topics::TOOL_RESULT,
                    serde_json::json!({ "name": name, "result": result_json, "ok": ok }),
                );

                let tool_message =
                    serde_json::json!({ "tool": name, "result": result_json }).to_string();
                persist_message(&deps.storage, conversation_id, "tool", &tool_message, false, &mut last_ts, None).await;
            }
            None => {
                let answer = strip_think(&full);
                if answer.is_empty() {
                    // Nothing usable came out of this turn — keep the raw
                    // output around for debugging, but there's no real
                    // answer to show, so it stays invisible too.
                    persist_message(&deps.storage, conversation_id, "assistant", &full, false, &mut last_ts, None).await;
                    let _ = tx.send(Err(DimiError::Internal(
                        "the model produced a reasoning block but no answer for this turn — try rephrasing, or retry".into(),
                    )));
                } else {
                    let sources = dedup_sources(turn_sources);
                    let sources_json = (!sources.is_empty())
                        .then(|| serde_json::to_string(&sources).unwrap_or_default());
                    persist_message(&deps.storage, conversation_id, "assistant", &answer, true, &mut last_ts, sources_json.as_deref()).await;
                    // No further send here — the raw text (think tags and
                    // all) already streamed live via `tx_for_stream` above,
                    // token by token, as it was generated. Sending `answer`
                    // (the think-stripped version, used only for persistence)
                    // again here would duplicate it in the frontend.
                }
                return;
            }
        }
    }

    let _ = tx.send(Err(DimiError::Internal(
        "tool-call loop exceeded max iterations without a final answer".into(),
    )));
}


async fn persist_message(
    storage: &Arc<dyn StorageEngine>,
    conversation_id: ConversationId,
    role: &str,
    content: &str,
    visible: bool,
    last_ts: &mut i64,
    sources: Option<&str>,
) {
    let ts = next_timestamp(last_ts);
    let _ = storage
        .query(
            "INSERT INTO messages (id, conversation_id, role, content, visible, created_at, sources) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            &[
                SqlValue::Text(MessageId::new().to_string()),
                SqlValue::Text(conversation_id.to_string()),
                SqlValue::Text(role.to_string()),
                SqlValue::Text(content.to_string()),
                SqlValue::Integer(visible as i64),
                SqlValue::Integer(ts),
                match sources {
                    Some(s) => SqlValue::Text(s.to_string()),
                    None => SqlValue::Null,
                },
            ],
        )
        .await;
}

/// Recursively walks a tool result looking for `abs_path` fields (every
/// file-touching tool includes one — see `services::tool`), collecting
/// `(display_name, absolute_path)` pairs regardless of how deeply nested
/// they are (a single `path` field, or an array of search/list results).
fn collect_sources(value: &serde_json::Value, out: &mut Vec<(String, String)>) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(abs_path)) = map.get("abs_path") {
                let name = std::path::Path::new(abs_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| abs_path.clone());
                out.push((name, abs_path.clone()));
            }
            for v in map.values() {
                collect_sources(v, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_sources(item, out);
            }
        }
        _ => {}
    }
}

fn dedup_sources(sources: Vec<(String, String)>) -> Vec<serde_json::Value> {
    let mut seen = std::collections::HashSet::new();
    sources
        .into_iter()
        .filter(|(_, path)| seen.insert(path.clone()))
        .map(|(name, path)| serde_json::json!({ "name": name, "path": path }))
        .collect()
}

fn next_timestamp(last: &mut i64) -> i64 {
    let now = now_unix();
    *last = if now > *last { now } else { *last + 1 };
    *last
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn extract_tool_call(text: &str) -> Option<(String, serde_json::Value)> {
    let start = text.find("<tool_call>")?;
    let end = text.find("</tool_call>")?;
    if end <= start {
        return None;
    }
    let inner = &text[start + "<tool_call>".len()..end];
    let value: serde_json::Value = serde_json::from_str(inner.trim()).ok()?;
    let name = value.get("name")?.as_str()?.to_string();
    let arguments = value
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    Some((name, arguments))
}

fn strip_think(text: &str) -> String {
    let Some(start) = text.find("<think>") else {
        return text.trim().to_string();
    };
    match text[start..].find("</think>") {
        Some(offset) => {
            let close = start + offset + "</think>".len();
            format!("{}{}", &text[..start], &text[close..])
                .trim()
                .to_string()
        }
        None => text[..start].trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_stream_len_withholds_a_growing_prefix_of_the_pattern() {
        // "<tool" could still become "<tool_call>" with more input — none of
        // it is safe to send yet.
        assert_eq!(safe_stream_len("<tool", "<tool_call>"), 0);
    }

    #[test]
    fn safe_stream_len_sends_unrelated_text_immediately() {
        assert_eq!(safe_stream_len("Hello there", "<tool_call>"), 11);
    }

    #[test]
    fn safe_stream_len_sends_everything_before_a_trailing_partial_match() {
        // "Sure! <tool" — "Sure! " is definitely not part of a tag, only the
        // trailing "<tool" is still ambiguous.
        assert_eq!(safe_stream_len("Sure! <tool", "<tool_call>"), 6);
    }

    #[test]
    fn safe_stream_len_never_withholds_more_than_pattern_len_minus_one() {
        // A string that merely *contains* the pattern early on, with plenty
        // of unrelated trailing text, is entirely safe to send.
        let text = "<tool_call>{...}</tool_call> and then some more prose after it";
        assert_eq!(safe_stream_len(text, "<tool_call>"), text.len());
    }

    #[test]
    fn extracts_well_formed_tool_call() {
        let text = "<tool_call>\n{\"name\": \"read_file\", \"arguments\": {\"repository_id\": \"abc\", \"path\": \"a.txt\"}}\n</tool_call>";
        let (name, args) = extract_tool_call(text).expect("should parse");
        assert_eq!(name, "read_file");
        assert_eq!(args["path"], "a.txt");
    }

    #[test]
    fn plain_answer_is_not_a_tool_call() {
        assert!(extract_tool_call("The answer is 42.").is_none());
    }

    #[test]
    fn malformed_tool_call_fails_open() {
        assert!(extract_tool_call("<tool_call>not json</tool_call>").is_none());
    }

    #[test]
    fn strips_a_closed_think_block() {
        assert_eq!(strip_think("<think>reasoning</think>The answer is 42."), "The answer is 42.");
    }

    #[test]
    fn collect_sources_finds_abs_path_nested_inside_an_array_of_results() {
        let tool_result = serde_json::json!({
            "results": [
                { "source_path": "notes/a.md", "abs_path": "/repo/notes/a.md", "text": "..." },
                { "source_path": "notes/b.md", "abs_path": "/repo/notes/b.md", "text": "..." },
            ]
        });
        let mut sources = Vec::new();
        collect_sources(&tool_result, &mut sources);
        assert_eq!(
            sources,
            vec![
                ("a.md".to_string(), "/repo/notes/a.md".to_string()),
                ("b.md".to_string(), "/repo/notes/b.md".to_string()),
            ]
        );
    }

    #[test]
    fn dedup_sources_keeps_first_occurrence_per_path() {
        let sources = vec![
            ("a.md".to_string(), "/repo/notes/a.md".to_string()),
            ("a.md (again)".to_string(), "/repo/notes/a.md".to_string()),
            ("b.md".to_string(), "/repo/notes/b.md".to_string()),
        ];
        let deduped = dedup_sources(sources);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0]["name"], "a.md");
        assert_eq!(deduped[1]["path"], "/repo/notes/b.md");
    }

    #[tokio::test]
    async fn invisible_messages_are_excluded_by_the_frontend_query_but_not_the_history_query() {
        use crate::services::storage::SqliteStorageEngine;

        let storage: Arc<dyn StorageEngine> =
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap());
        let conversation_id = ConversationId::new();
        // `messages.conversation_id` has a foreign key onto `conversations`.
        storage
            .query(
                "INSERT INTO conversations (id, title, created_at) VALUES (?1, 'test', 0)",
                &[SqlValue::Text(conversation_id.to_string())],
            )
            .await
            .unwrap();
        let mut last_ts = now_unix() - 1;

        persist_message(&storage, conversation_id, "assistant", "<tool_call>...</tool_call>", false, &mut last_ts, None).await;
        persist_message(&storage, conversation_id, "tool", "{\"result\": \"...\"}", false, &mut last_ts, None).await;
        persist_message(&storage, conversation_id, "assistant", "the real answer", true, &mut last_ts, None).await;

        // Mirrors messages_list's frontend-facing query.
        let visible_rows = storage
            .query(
                "SELECT content FROM messages WHERE conversation_id = ?1 AND visible = 1 ORDER BY created_at ASC",
                &[SqlValue::Text(conversation_id.to_string())],
            )
            .await
            .unwrap();
        assert_eq!(visible_rows.len(), 1);
        assert_eq!(
            visible_rows[0].0.get("content"),
            Some(&SqlValue::Text("the real answer".to_string()))
        );

        // Mirrors context.rs's history query — must see everything.
        let all_rows = storage
            .query(
                "SELECT content FROM messages WHERE conversation_id = ?1 ORDER BY created_at ASC",
                &[SqlValue::Text(conversation_id.to_string())],
            )
            .await
            .unwrap();
        assert_eq!(all_rows.len(), 3);
    }

    #[test]
    fn an_unclosed_think_block_leaves_nothing() {
        assert_eq!(strip_think("<think>still reasoning, never finished"), "");
    }

    #[test]
    fn plain_text_is_unaffected() {
        assert_eq!(strip_think("hello there"), "hello there");
    }
}
