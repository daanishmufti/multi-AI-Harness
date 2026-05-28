//! Names of the events the backend emits to the webview. Centralized so the
//! frontend (`src/api.ts`) and backend never drift apart.

/// A session was created or any of its [`crate::model::SessionInfo`] fields
/// changed. Payload: `SessionInfo`.
pub const SESSION_UPDATE: &str = "session:update";
/// A session was removed from the registry. Payload: `{ "id": SessionId }`.
pub const SESSION_REMOVED: &str = "session:removed";
/// A new structured log line. Payload: `LogEntry`.
pub const LOG: &str = "log";
/// Raw terminal bytes for an attached interactive session.
/// Payload: `{ "session_id": SessionId, "data": base64, "replay": bool }`.
pub const TERM_DATA: &str = "term:data";
/// A task's status changed. Payload: `Task`.
pub const TASK_UPDATE: &str = "task:update";
/// Periodic system + aggregate resource sample. Payload: `SystemMetrics`.
pub const METRICS: &str = "metrics";
