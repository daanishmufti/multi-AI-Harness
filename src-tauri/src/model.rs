//! Core domain types shared across the backend and serialized to the UI.
//!
//! Everything here is `serde`-serializable so it can cross the Tauri IPC
//! boundary unchanged. Runtime-only handles (PTY masters, child processes,
//! stdin pipes) live in `session.rs` and never touch this module.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Stable identifier for a session (UUIDv4 string).
pub type SessionId = String;
/// Stable identifier for a task (UUIDv4 string).
pub type TaskId = String;

/// Milliseconds since the Unix epoch. Used for every timestamp.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// How a worker is wired up.
///
/// * `Headless` — plain stdin/stdout pipes speaking the `stream-json`
///   bidirectional protocol. Cheap, structured, scales to many instances.
///   This is the default at scale.
/// * `Interactive` — a real ConPTY-backed agent TUI you can attach to and
///   type into. Heavier; only spun up when you want a live terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionMode {
    /// Headless `stream-json` worker (pipes).
    Headless,
    /// Interactive agent TUI in a ConPTY.
    Interactive,
    /// A plain shell (PowerShell/cmd) in a ConPTY — a normal terminal.
    Shell,
}

/// Lifecycle state of a session. `attached` is tracked separately on
/// [`SessionInfo`] because a session can be attached in any live state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Process spawned, waiting for the `system/init` handshake.
    Starting,
    /// Ready and available to accept a task / message.
    Idle,
    /// Actively processing a turn (between a user message and its `result`).
    Busy,
    /// Finished its work and exited cleanly.
    Completed,
    /// Exited unexpectedly or returned a non-zero code.
    Crashed,
    /// Terminated on request.
    Stopped,
}

impl SessionStatus {
    /// Whether the session occupies a concurrency slot for scheduling.
    pub fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Idle | Self::Busy)
    }
    /// Whether the session process is gone.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Crashed | Self::Stopped)
    }
}

/// Token accounting accumulated from `result` events.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_creation_tokens + self.cache_read_tokens
    }
    /// Fold one turn's usage into the running total.
    pub fn add(&mut self, other: &TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_tokens += other.cache_creation_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
    }
}

/// The serializable snapshot of a session sent to the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: SessionId,
    pub name: String,
    pub mode: SessionMode,
    pub status: SessionStatus,
    pub cwd: String,
    pub model: Option<String>,
    /// The agent CLI's own session id (from `system/init` / `result`), used for resume.
    #[serde(rename = "agent_session_id", alias = "claude_session_id")]
    pub agent_session_id: Option<String>,
    pub attached: bool,
    pub pid: Option<u32>,
    pub created_at: i64,
    pub last_activity: i64,
    pub usage: TokenUsage,
    pub cost_usd: f64,
    pub turns: u32,
    pub current_task: Option<TaskId>,
    /// One-line description of what the session is doing right now.
    pub activity: String,
    // Live resource sample, refreshed by the monitor.
    pub cpu: f32,
    pub mem_bytes: u64,
    pub exit_code: Option<i32>,
}

impl SessionInfo {
    pub fn new(id: SessionId, name: String, mode: SessionMode, cwd: String, model: Option<String>) -> Self {
        let ts = now_ms();
        Self {
            id,
            name,
            mode,
            status: SessionStatus::Starting,
            cwd,
            model,
            agent_session_id: None,
            attached: false,
            pid: None,
            created_at: ts,
            last_activity: ts,
            usage: TokenUsage::default(),
            cost_usd: 0.0,
            turns: 0,
            current_task: None,
            activity: "starting".into(),
            cpu: 0.0,
            mem_bytes: 0,
            exit_code: None,
        }
    }
}

/// Specification used to launch a session (and to recreate it from a snapshot).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSpec {
    pub name: Option<String>,
    pub mode: SessionMode,
    pub cwd: String,
    pub model: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub allowed_tools: Option<String>,
    /// Optional initial prompt to send as the first turn (headless only).
    #[serde(default)]
    pub initial_prompt: Option<String>,
    /// Resume a prior agent conversation by id.
    #[serde(default)]
    pub resume: Option<String>,
}

/// Status of a queued unit of work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Assigned,
    Running,
    Done,
    Failed,
    Canceled,
}

/// A unit of work for the scheduler: a prompt to run in a (cwd, model) context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub prompt: String,
    pub cwd: String,
    pub model: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub allowed_tools: Option<String>,
    pub status: TaskStatus,
    pub session_id: Option<SessionId>,
    /// Higher runs first.
    pub priority: i32,
    pub attempts: u32,
    pub max_attempts: u32,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub result_summary: Option<String>,
    pub error: Option<String>,
}

/// A structured log line attributed to a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: i64,
    pub session_id: SessionId,
    pub ts: i64,
    /// Category: `system`, `assistant`, `tool_use`, `tool_result`, `result`,
    /// `stderr`, `info`, `error`, `user`.
    pub kind: String,
    pub text: String,
    /// Optional raw JSON payload for drill-down.
    pub data: Option<String>,
}

/// System-wide resource sample emitted by the monitor.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub cpu_percent: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub active_sessions: usize,
    pub total_sessions: usize,
    pub queued_tasks: usize,
    pub running_tasks: usize,
    /// Aggregate tokens across all live sessions.
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub ts: i64,
}

/// Tunable orchestrator policy. Editable live from the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorConfig {
    /// Maximum number of concurrently *active* sessions the scheduler will run.
    pub max_concurrent: usize,
    /// Maximum number of simultaneously *rendered* terminals (1–4).
    pub max_attached: usize,
    /// Pause launching new work above this system CPU percentage.
    pub cpu_limit_percent: f32,
    /// Pause launching new work above this system memory percentage.
    pub mem_limit_percent: f32,
    /// Re-queue a task if its session crashes mid-run.
    pub auto_restart: bool,
    /// Reuse idle pooled sessions with a matching (cwd, model) instead of
    /// spawning fresh processes.
    pub session_pooling: bool,
    /// Path to the agent CLI executable. Empty = auto-detect.
    #[serde(rename = "agent_bin", alias = "claude_bin")]
    pub agent_bin: String,
    /// Shell to launch for plain terminal sessions. Empty = auto-detect
    /// (prefers `pwsh.exe`, then `powershell.exe`, then `cmd.exe`).
    #[serde(default)]
    pub shell_program: String,
    pub default_model: Option<String>,
    /// One of: `default`, `acceptEdits`, `dontAsk`, `bypassPermissions`.
    pub default_permission_mode: String,
    /// Pass `--bare` for faster startup (skips hook/skill/MCP discovery).
    pub bare_mode: bool,
    /// Per-session PTY scrollback retained for replay-on-attach, in bytes.
    pub ring_buffer_bytes: usize,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 8,
            max_attached: 8,
            cpu_limit_percent: 85.0,
            mem_limit_percent: 85.0,
            auto_restart: true,
            session_pooling: true,
            agent_bin: String::new(),
            shell_program: String::new(),
            default_model: None,
            default_permission_mode: "acceptEdits".into(),
            bare_mode: false,
            ring_buffer_bytes: 256 * 1024,
        }
    }
}
