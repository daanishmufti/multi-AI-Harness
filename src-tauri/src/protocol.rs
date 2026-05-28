//! Parsing of the agent CLI's `stream-json` protocol and construction of the
//! input messages we write back on stdin.
//!
//! The CLI emits one JSON object per line on stdout when launched with
//! `--output-format stream-json --verbose`. Rather than model every field
//! with a rigid struct (the schema evolves), we parse into `serde_json::Value`
//! and pull out the fields we care about. This keeps us resilient to additive
//! changes in the protocol.

use crate::model::TokenUsage;
use serde_json::{json, Value};

/// A parsed, dashboard-relevant view of a single output line.
///
/// `TextDelta` and `Other` carry data we parse for completeness (and future
/// live-token streaming) but don't currently act on; their payloads are
/// intentionally retained.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum Event {
    /// `system`/`init`: the handshake. Carries the model and agent session id.
    Init {
        session_id: Option<String>,
        model: Option<String>,
        cwd: Option<String>,
    },
    /// `system` with some other subtype (e.g. `api_retry`).
    System { subtype: String, text: String },
    /// An assistant turn. `text` is the concatenated visible text; `tools`
    /// lists any tool calls made in the message.
    Assistant { text: String, tools: Vec<String> },
    /// A tool result echoed back as a `user` message.
    ToolResult { text: String },
    /// Incremental token text from `--include-partial-messages`.
    TextDelta { text: String },
    /// End of a turn. Carries cumulative usage / cost / turn count.
    Result {
        is_error: bool,
        text: Option<String>,
        usage: TokenUsage,
        cost_usd: Option<f64>,
        num_turns: Option<u32>,
        session_id: Option<String>,
    },
    /// Anything we do not specifically model; preserved for the raw log.
    Other { kind: String },
}

/// Extract a [`TokenUsage`] from a `usage` object, tolerating absent fields.
fn parse_usage(usage: &Value) -> TokenUsage {
    let g = |k: &str| usage.get(k).and_then(Value::as_u64).unwrap_or(0);
    TokenUsage {
        input_tokens: g("input_tokens"),
        output_tokens: g("output_tokens"),
        cache_creation_tokens: g("cache_creation_input_tokens"),
        cache_read_tokens: g("cache_read_input_tokens"),
    }
}

/// Pull the visible text and tool names out of a message object.
fn parse_message_content(message: &Value) -> (String, Vec<String>) {
    let mut text = String::new();
    let mut tools = Vec::new();
    match message.get("content") {
        Some(Value::String(s)) => text.push_str(s),
        Some(Value::Array(blocks)) => {
            for b in blocks {
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = b.get("text").and_then(Value::as_str) {
                            text.push_str(t);
                        }
                    }
                    Some("tool_use") => {
                        if let Some(name) = b.get("name").and_then(Value::as_str) {
                            tools.push(name.to_string());
                        }
                    }
                    Some("tool_result") => {
                        // Tool results carry their own content array or string.
                        if let Some(c) = b.get("content") {
                            text.push_str(&flatten_text(c));
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    (text, tools)
}

/// Best-effort flatten of arbitrary `content` into a string.
fn flatten_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|i| i.get("text").and_then(Value::as_str).unwrap_or("").to_string())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Parse one line of `stream-json` output into an [`Event`].
///
/// Returns `None` only when the line is not valid JSON (blank lines, etc.).
pub fn parse_line(line: &str) -> Option<Event> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("unknown");

    Some(match kind {
        "system" => {
            let subtype = v.get("subtype").and_then(Value::as_str).unwrap_or("");
            if subtype == "init" {
                Event::Init {
                    session_id: v.get("session_id").and_then(Value::as_str).map(String::from),
                    model: v.get("model").and_then(Value::as_str).map(String::from),
                    cwd: v.get("cwd").and_then(Value::as_str).map(String::from),
                }
            } else {
                Event::System {
                    subtype: subtype.to_string(),
                    text: line.chars().take(400).collect(),
                }
            }
        }
        "assistant" => {
            let (text, tools) = v
                .get("message")
                .map(parse_message_content)
                .unwrap_or_default();
            Event::Assistant { text, tools }
        }
        "user" => {
            let (text, _) = v
                .get("message")
                .map(parse_message_content)
                .unwrap_or_default();
            Event::ToolResult { text }
        }
        "stream_event" => {
            // message_start / content_block_delta / message_stop, etc.
            let delta_text = v
                .get("event")
                .and_then(|e| e.get("delta"))
                .filter(|d| d.get("type").and_then(Value::as_str) == Some("text_delta"))
                .and_then(|d| d.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if delta_text.is_empty() {
                Event::Other { kind: "stream_event".into() }
            } else {
                Event::TextDelta { text: delta_text.to_string() }
            }
        }
        "result" => {
            let usage = v.get("usage").map(parse_usage).unwrap_or_default();
            Event::Result {
                is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                text: v.get("result").and_then(Value::as_str).map(String::from),
                usage,
                cost_usd: v.get("total_cost_usd").and_then(Value::as_f64),
                num_turns: v.get("num_turns").and_then(Value::as_u64).map(|n| n as u32),
                session_id: v.get("session_id").and_then(Value::as_str).map(String::from),
            }
        }
        other => Event::Other { kind: other.to_string() },
    })
}

/// Build a `user` message line to write to the CLI's stdin in stream-json input
/// mode. The trailing newline delimits the message.
pub fn user_message_line(prompt: &str) -> String {
    let msg = json!({
        "type": "user",
        "message": { "role": "user", "content": prompt }
    });
    format!("{msg}\n")
}
