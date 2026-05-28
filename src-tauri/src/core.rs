//! The orchestrator core: the single shared state object behind the Tauri
//! `State`, plus every operation that mutates a session, task, or log.
//!
//! Concurrency model (deliberately thread-based, not async):
//!   * Each session owns 1–2 OS reader threads that block on the child's
//!     stdout/stderr (or PTY master) and call back into `Core`.
//!   * Writers live behind `Arc<Mutex<_>>` so a command can clone the handle,
//!     release the sessions-map lock, and write without stalling others.
//!   * Background `scheduler` and `monitor` loops (spawned in `lib.rs`) call
//!     `Core` methods on a fixed cadence.
//! Lock discipline: never hold the `sessions` lock across a blocking I/O call
//! or an event emit — snapshot what you need, drop the guard, then act.

use crate::model::*;
use crate::protocol::{self, Event};
use crate::ring::RingBuffer;
use crate::session::{ChildHandle, Session, Transport};
use crate::{events, launcher, storage};
use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use base64::Engine as _;
const B64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;

pub struct Core {
    pub app: AppHandle,
    pub config: Mutex<OrchestratorConfig>,
    pub sessions: Mutex<HashMap<SessionId, Session>>,
    pub tasks: Mutex<HashMap<TaskId, Task>>,
    pub db: Mutex<Connection>,
    pub metrics: Mutex<SystemMetrics>,
    /// Attach order, oldest first; the tail is the most recently attached.
    pub attach_lru: Mutex<Vec<SessionId>>,
}

impl Core {
    pub fn new(app: AppHandle, db: Connection) -> Arc<Self> {
        let mut config = storage::load_config(&db);
        // One-time migration: the previous cap was 4 and there was no UI for
        // anything higher, so existing users were stuck. Bump anyone at/below
        // the old cap up to the new default so they immediately benefit.
        if config.max_attached <= 4 {
            config.max_attached = OrchestratorConfig::default().max_attached;
            let _ = storage::save_config(&db, &config);
        }
        Arc::new(Self {
            app,
            config: Mutex::new(config),
            sessions: Mutex::new(HashMap::new()),
            tasks: Mutex::new(HashMap::new()),
            db: Mutex::new(db),
            metrics: Mutex::new(SystemMetrics::default()),
            attach_lru: Mutex::new(Vec::new()),
        })
    }

    // ---- small helpers ----------------------------------------------------

    fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) {
        let _ = self.app.emit(event, payload);
    }

    /// Emit an event from outside this module (used by the monitor loop).
    pub fn emit_public<S: Serialize + Clone>(&self, event: &str, payload: S) {
        self.emit(event, payload);
    }

    /// Persist + broadcast a session's current info.
    fn broadcast_info(&self, info: &SessionInfo) {
        {
            let db = self.db.lock();
            storage::upsert_session(&db, info);
        }
        self.emit(events::SESSION_UPDATE, info.clone());
    }

    /// Append a structured log line, persist it, and stream it to the UI.
    fn log(&self, sid: &str, kind: &str, text: &str, data: Option<&str>) {
        let id = {
            let db = self.db.lock();
            storage::insert_log(&db, sid, kind, text, data)
        };
        self.emit(
            events::LOG,
            LogEntry {
                id,
                session_id: sid.to_string(),
                ts: now_ms(),
                kind: kind.to_string(),
                text: text.to_string(),
                data: data.map(str::to_string),
            },
        );
    }

    /// Apply a mutation to a session's info under the lock, then broadcast it.
    fn update_info<F: FnOnce(&mut SessionInfo)>(&self, sid: &str, f: F) {
        let info = {
            let mut sessions = self.sessions.lock();
            match sessions.get_mut(sid) {
                Some(s) => {
                    f(&mut s.info);
                    s.info.last_activity = now_ms();
                    s.info.clone()
                }
                None => return,
            }
        };
        self.broadcast_info(&info);
    }

    fn touch_activity(&self, sid: &str) {
        let mut sessions = self.sessions.lock();
        if let Some(s) = sessions.get_mut(sid) {
            s.info.last_activity = now_ms();
        }
    }

    // ---- spawning ---------------------------------------------------------

    /// Launch a headless `stream-json` worker. Returns its session id.
    pub fn spawn_headless(self: &Arc<Self>, mut spec: SessionSpec) -> Result<SessionId> {
        let cfg = self.config.lock().clone();
        let bin = launcher::resolve_agent(&cfg);
        let args = launcher::headless_args(&spec, &cfg);
        let sid = Uuid::new_v4().to_string();
        let name = spec
            .name
            .clone()
            .unwrap_or_else(|| default_name(&spec.cwd, &sid));

        let mut cmd = Command::new(&bin);
        cmd.args(&args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn agent at {bin}"))?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
        let pid = Some(child.id());

        let effective_model = spec.model.clone().or_else(|| cfg.default_model.clone());
        let mut info = SessionInfo::new(
            sid.clone(),
            name,
            SessionMode::Headless,
            spec.cwd.clone(),
            effective_model,
        );
        info.pid = pid;
        info.activity = "initializing".into();

        let child_arc = Arc::new(Mutex::new(ChildHandle::Std(child)));
        let stop = Arc::new(AtomicBool::new(false));
        let attached = Arc::new(AtomicBool::new(false));
        let ring = Arc::new(Mutex::new(RingBuffer::new(cfg.ring_buffer_bytes)));

        let session = Session {
            info: info.clone(),
            spec: spec.clone(),
            transport: Transport::Headless {
                stdin: Arc::new(Mutex::new(stdin)),
            },
            child: child_arc.clone(),
            ring,
            attached,
            stop: stop.clone(),
        };
        self.sessions.lock().insert(sid.clone(), session);
        self.broadcast_info(&info);
        self.log(&sid, "system", &format!("spawned headless worker (pid {pid:?})"), None);

        // stdout: the protocol stream.
        {
            let core = self.clone();
            let sid = sid.clone();
            let child = child_arc.clone();
            let stop = stop.clone();
            thread::spawn(move || core.run_headless_stdout(sid, stdout, child, stop));
        }
        // stderr: diagnostics.
        {
            let core = self.clone();
            let sid = sid.clone();
            let stop = stop.clone();
            thread::spawn(move || core.run_stderr(sid, stderr, stop));
        }

        if let Some(prompt) = spec.initial_prompt.take() {
            self.send_message(&sid, &prompt)?;
        }
        Ok(sid)
    }

    /// Launch an interactive ConPTY-backed agent TUI. Returns its session id.
    pub fn spawn_interactive(self: &Arc<Self>, spec: SessionSpec) -> Result<SessionId> {
        let cfg = self.config.lock().clone();
        let bin = launcher::resolve_agent(&cfg);
        let args = launcher::interactive_args(&spec, &cfg);
        self.spawn_pty_session(bin, args, spec, SessionMode::Interactive)
    }

    /// Launch a plain shell (PowerShell/cmd) in a ConPTY — a normal terminal.
    pub fn spawn_shell(self: &Arc<Self>, spec: SessionSpec) -> Result<SessionId> {
        let cfg = self.config.lock().clone();
        let (program, args) = launcher::resolve_shell(&cfg);
        self.spawn_pty_session(program, args, spec, SessionMode::Shell)
    }

    /// Shared ConPTY spawn used by both interactive agent and shell sessions.
    fn spawn_pty_session(
        self: &Arc<Self>,
        program: String,
        args: Vec<String>,
        spec: SessionSpec,
        mode: SessionMode,
    ) -> Result<SessionId> {
        let cfg = self.config.lock().clone();
        let sid = Uuid::new_v4().to_string();
        let name = spec
            .name
            .clone()
            .unwrap_or_else(|| default_name(&spec.cwd, &sid));

        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize { rows: 30, cols: 120, pixel_width: 0, pixel_height: 0 })
            .context("openpty (ConPTY) failed")?;

        let mut cmd = CommandBuilder::new(&program);
        for a in &args {
            cmd.arg(a);
        }
        cmd.cwd(&spec.cwd);
        cmd.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawning {program} in ConPTY failed"))?;
        let pid = child.process_id();
        // Drop the slave in the parent so EOF propagates when the child exits.
        drop(pair.slave);

        let reader = pair.master.try_clone_reader().context("clone pty reader")?;
        let writer = pair.master.take_writer().context("take pty writer")?;

        let model = if mode == SessionMode::Interactive {
            spec.model.clone().or_else(|| cfg.default_model.clone())
        } else {
            None
        };
        let mut info = SessionInfo::new(sid.clone(), name, mode, spec.cwd.clone(), model);
        info.pid = pid;
        info.status = SessionStatus::Idle;
        info.activity = if mode == SessionMode::Shell { "terminal".into() } else { "interactive".into() };

        let child_arc = Arc::new(Mutex::new(ChildHandle::Pty(child)));
        let stop = Arc::new(AtomicBool::new(false));
        let attached = Arc::new(AtomicBool::new(false));
        let ring = Arc::new(Mutex::new(RingBuffer::new(cfg.ring_buffer_bytes)));

        let session = Session {
            info: info.clone(),
            spec: spec.clone(),
            transport: Transport::Interactive {
                writer: Arc::new(Mutex::new(writer)),
                master: Arc::new(Mutex::new(pair.master)),
            },
            child: child_arc.clone(),
            ring: ring.clone(),
            attached: attached.clone(),
            stop: stop.clone(),
        };
        self.sessions.lock().insert(sid.clone(), session);
        self.broadcast_info(&info);
        self.log(&sid, "system", &format!("spawned {program} (pid {pid:?})"), None);

        {
            let core = self.clone();
            let sid = sid.clone();
            let child = child_arc.clone();
            thread::spawn(move || core.run_pty_reader(sid, reader, ring, attached, stop, child));
        }

        // If the caller provided an initial command/input, type it into the PTY
        // after a short delay so the shell (PowerShell, cmd) or agent TUI has
        // had a moment to print its prompt and start reading.
        if let Some(line) = spec.initial_prompt.as_deref() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let payload = format!("{trimmed}\r");
                let core = self.clone();
                let sid = sid.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(300));
                    let _ = core.send_input(&sid, &payload);
                });
            }
        }
        Ok(sid)
    }

    // ---- reader threads ---------------------------------------------------

    fn run_headless_stdout(
        self: Arc<Self>,
        sid: SessionId,
        stdout: std::process::ChildStdout,
        child: Arc<Mutex<ChildHandle>>,
        stop: Arc<AtomicBool>,
    ) {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match line {
                Ok(l) => {
                    if let Some(ev) = protocol::parse_line(&l) {
                        self.on_headless_event(&sid, ev);
                    }
                }
                Err(_) => break,
            }
        }
        self.finalize(&sid, &child, stop.load(Ordering::Relaxed));
    }

    fn run_stderr(self: Arc<Self>, sid: SessionId, stderr: std::process::ChildStderr, stop: Arc<AtomicBool>) {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match line {
                Ok(l) if !l.trim().is_empty() => self.log(&sid, "stderr", &l, None),
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }

    fn run_pty_reader(
        self: Arc<Self>,
        sid: SessionId,
        mut reader: Box<dyn Read + Send>,
        ring: Arc<Mutex<RingBuffer>>,
        attached: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        child: Arc<Mutex<ChildHandle>>,
    ) {
        let mut buf = [0u8; 8192];
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = &buf[..n];
                    ring.lock().push(chunk);
                    if attached.load(Ordering::Relaxed) {
                        self.emit_term_data(&sid, chunk, false);
                    }
                    self.touch_activity(&sid);
                }
                Err(_) => break,
            }
        }
        self.finalize(&sid, &child, stop.load(Ordering::Relaxed));
    }

    fn emit_term_data(&self, sid: &str, data: &[u8], replay: bool) {
        self.emit(
            events::TERM_DATA,
            serde_json::json!({
                "session_id": sid,
                "data": B64.encode(data),
                "replay": replay,
            }),
        );
    }

    // ---- stream-json event handling --------------------------------------

    fn on_headless_event(self: &Arc<Self>, sid: &str, ev: Event) {
        match ev {
            Event::Init { session_id, model, cwd } => {
                self.update_info(sid, |i| {
                    if let Some(s) = session_id.clone() {
                        i.agent_session_id = Some(s);
                    }
                    if model.is_some() {
                        i.model = model.clone();
                    }
                    if let Some(c) = cwd.clone() {
                        i.cwd = c;
                    }
                    if i.status == SessionStatus::Starting {
                        i.status = SessionStatus::Idle;
                    }
                    i.activity = "ready".into();
                });
                self.log(sid, "system", "session initialized", None);
            }
            Event::System { subtype, text } => {
                if subtype == "api_retry" {
                    self.update_info(sid, |i| i.activity = "retrying request".into());
                }
                self.log(sid, "system", &format!("{subtype}: {}", truncate(&text, 200)), None);
            }
            Event::Assistant { text, tools } => {
                self.update_info(sid, |i| {
                    i.status = SessionStatus::Busy;
                    i.activity = if let Some(t) = tools.first() {
                        format!("using {t}")
                    } else {
                        "responding".into()
                    };
                });
                for t in &tools {
                    self.log(sid, "tool_use", t, None);
                }
                if !text.trim().is_empty() {
                    self.log(sid, "assistant", &text, None);
                }
            }
            Event::ToolResult { text } => {
                if !text.trim().is_empty() {
                    self.log(sid, "tool_result", &truncate(&text, 400), None);
                }
            }
            // Partial deltas are intentionally not enabled at scale; the full
            // `assistant` message carries the complete text.
            Event::TextDelta { .. } | Event::Other { .. } => {}
            Event::Result {
                is_error,
                text,
                usage,
                cost_usd,
                num_turns,
                session_id,
            } => {
                self.update_info(sid, |i| {
                    i.usage.add(&usage);
                    if let Some(c) = cost_usd {
                        i.cost_usd = c; // CLI reports cumulative conversation cost.
                    }
                    if let Some(n) = num_turns {
                        i.turns = n;
                    } else {
                        i.turns += 1;
                    }
                    if let Some(s) = session_id.clone() {
                        i.agent_session_id = Some(s);
                    }
                    i.status = SessionStatus::Idle;
                    i.activity = if is_error { "turn failed".into() } else { "idle".into() };
                });
                let summary = text.clone().unwrap_or_else(|| "(turn complete)".into());
                self.log(sid, "result", &truncate(&summary, 600), None);
                self.on_turn_complete(sid, is_error, summary);
            }
        }
    }

    /// A headless turn finished — complete (or fail) the session's current task.
    fn on_turn_complete(self: &Arc<Self>, sid: &str, is_error: bool, summary: String) {
        let task_id = {
            let mut sessions = self.sessions.lock();
            match sessions.get_mut(sid) {
                Some(s) => s.info.current_task.take(),
                None => None,
            }
        };
        if let Some(tid) = task_id {
            let task = {
                let mut tasks = self.tasks.lock();
                if let Some(t) = tasks.get_mut(&tid) {
                    t.status = if is_error { TaskStatus::Failed } else { TaskStatus::Done };
                    t.finished_at = Some(now_ms());
                    if is_error {
                        t.error = Some(truncate(&summary, 300));
                    } else {
                        t.result_summary = Some(truncate(&summary, 300));
                    }
                    Some(t.clone())
                } else {
                    None
                }
            };
            if let Some(t) = task {
                let db = self.db.lock();
                storage::upsert_task(&db, &t);
                drop(db);
                self.emit(events::TASK_UPDATE, t);
            }
        }
    }

    // ---- termination + crash recovery ------------------------------------

    fn finalize(self: &Arc<Self>, sid: &str, child: &Arc<Mutex<ChildHandle>>, requested_stop: bool) {
        // Reap without holding the lock across a blocking wait.
        let mut code = None;
        for _ in 0..120 {
            if let Some(c) = child.lock().try_wait() {
                code = Some(c);
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }

        let (status, current_task, name) = {
            let mut sessions = self.sessions.lock();
            match sessions.get_mut(sid) {
                Some(s) => {
                    let status = if requested_stop {
                        SessionStatus::Stopped
                    } else if code == Some(0) {
                        SessionStatus::Completed
                    } else {
                        SessionStatus::Crashed
                    };
                    s.info.status = status;
                    s.info.exit_code = code;
                    s.info.cpu = 0.0;
                    s.info.mem_bytes = 0;
                    s.info.activity = match status {
                        SessionStatus::Stopped => "stopped".into(),
                        SessionStatus::Completed => "completed".into(),
                        _ => format!("crashed (exit {})", code.unwrap_or(-1)),
                    };
                    s.info.last_activity = now_ms();
                    (status, s.info.current_task.clone(), s.info.clone())
                }
                None => return,
            }
        };
        self.broadcast_info(&name);
        self.log(sid, "system", &name.activity.clone(), None);

        // Crash recovery: requeue the interrupted task if policy allows.
        if status == SessionStatus::Crashed {
            if let Some(tid) = current_task {
                self.recover_task(&tid);
            }
        }
    }

    fn recover_task(&self, tid: &str) {
        let cfg = self.config.lock().clone();
        let updated = {
            let mut tasks = self.tasks.lock();
            match tasks.get_mut(tid) {
                Some(t) => {
                    if cfg.auto_restart && t.attempts < t.max_attempts {
                        t.status = TaskStatus::Queued; // scheduler will pick it up again
                        t.session_id = None;
                        t.started_at = None;
                    } else {
                        t.status = TaskStatus::Failed;
                        t.finished_at = Some(now_ms());
                        t.error = Some("session crashed".into());
                    }
                    Some(t.clone())
                }
                None => None,
            }
        };
        if let Some(t) = updated {
            let db = self.db.lock();
            storage::upsert_task(&db, &t);
            drop(db);
            self.emit(events::TASK_UPDATE, t);
        }
    }

    // ---- input ------------------------------------------------------------

    /// Send a turn to a headless worker (writes a JSON line to stdin).
    pub fn send_message(&self, sid: &str, prompt: &str) -> Result<()> {
        let stdin = {
            let sessions = self.sessions.lock();
            let s = sessions.get(sid).ok_or_else(|| anyhow!("unknown session {sid}"))?;
            match &s.transport {
                Transport::Headless { stdin } => stdin.clone(),
                Transport::Interactive { .. } => {
                    return Err(anyhow!("session is interactive; use send_input"))
                }
            }
        };
        {
            let mut g = stdin.lock();
            g.write_all(protocol::user_message_line(prompt).as_bytes())?;
            g.flush()?;
        }
        self.update_info(sid, |i| {
            i.status = SessionStatus::Busy;
            i.activity = "working".into();
        });
        self.log(sid, "user", prompt, None);
        Ok(())
    }

    /// Write raw bytes to an interactive PTY (keystrokes from xterm.js).
    pub fn send_input(&self, sid: &str, data: &str) -> Result<()> {
        let writer = {
            let sessions = self.sessions.lock();
            let s = sessions.get(sid).ok_or_else(|| anyhow!("unknown session {sid}"))?;
            match &s.transport {
                Transport::Interactive { writer, .. } => writer.clone(),
                Transport::Headless { .. } => {
                    return Err(anyhow!("session is headless; use send_message"))
                }
            }
        };
        let mut g = writer.lock();
        g.write_all(data.as_bytes())?;
        g.flush()?;
        Ok(())
    }

    pub fn resize(&self, sid: &str, cols: u16, rows: u16) -> Result<()> {
        let master = {
            let sessions = self.sessions.lock();
            let s = sessions.get(sid).ok_or_else(|| anyhow!("unknown session {sid}"))?;
            match &s.transport {
                Transport::Interactive { master, .. } => master.clone(),
                Transport::Headless { .. } => return Ok(()),
            }
        };
        master.lock().resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    /// Send the same prompt/keys to every live session of the matching kind.
    pub fn broadcast(&self, text: &str) -> usize {
        let targets: Vec<(SessionId, SessionMode)> = {
            let sessions = self.sessions.lock();
            sessions
                .values()
                .filter(|s| !s.info.status.is_terminal())
                .map(|s| (s.info.id.clone(), s.info.mode))
                .collect()
        };
        let mut n = 0;
        for (sid, mode) in targets {
            let ok = match mode {
                SessionMode::Headless => self.send_message(&sid, text).is_ok(),
                // Interactive agent and shell sessions both take raw keystrokes.
                SessionMode::Interactive | SessionMode::Shell => {
                    self.send_input(&sid, &format!("{text}\r")).is_ok()
                }
            };
            if ok {
                n += 1;
            }
        }
        n
    }

    // ---- attach / detach (lazy rendering, max N panes) -------------------

    pub fn attach(&self, sid: &str) -> Result<()> {
        let max = self.config.lock().max_attached.clamp(1, 16);
        let to_detach: Vec<SessionId> = {
            let mut lru = self.attach_lru.lock();
            lru.retain(|x| x != sid);
            lru.push(sid.to_string());
            let mut evicted = Vec::new();
            while lru.len() > max {
                evicted.push(lru.remove(0));
            }
            evicted
        };
        for d in to_detach {
            self.set_attached(&d, false);
        }
        self.set_attached(sid, true);
        self.replay(sid);
        Ok(())
    }

    pub fn detach(&self, sid: &str) {
        self.attach_lru.lock().retain(|x| x != sid);
        self.set_attached(sid, false);
    }

    fn set_attached(&self, sid: &str, attached: bool) {
        let info = {
            let mut sessions = self.sessions.lock();
            match sessions.get_mut(sid) {
                Some(s) => {
                    s.attached.store(attached, Ordering::Relaxed);
                    s.info.attached = attached;
                    s.info.clone()
                }
                None => return,
            }
        };
        self.emit(events::SESSION_UPDATE, info);
    }

    /// Replay the scrollback ring to the UI so an attached terminal repaints.
    fn replay(&self, sid: &str) {
        let snapshot = {
            let sessions = self.sessions.lock();
            match sessions.get(sid) {
                Some(s) if matches!(s.transport, Transport::Interactive { .. }) => {
                    let r = s.ring.lock();
                    if r.len() == 0 {
                        return;
                    }
                    r.snapshot()
                }
                _ => return,
            }
        };
        self.emit_term_data(sid, &snapshot, true);
    }

    // ---- lifecycle commands ----------------------------------------------

    /// Change a session's display name in-place.
    pub fn rename(&self, sid: &str, name: &str) -> Result<()> {
        let info = {
            let mut sessions = self.sessions.lock();
            match sessions.get_mut(sid) {
                Some(s) => {
                    s.info.name = name.to_string();
                    s.info.last_activity = now_ms();
                    s.info.clone()
                }
                None => return Err(anyhow!("unknown session {sid}")),
            }
        };
        self.broadcast_info(&info);
        Ok(())
    }

    pub fn kill(&self, sid: &str) -> Result<()> {
        let child = {
            let sessions = self.sessions.lock();
            let s = sessions.get(sid).ok_or_else(|| anyhow!("unknown session {sid}"))?;
            s.stop.store(true, Ordering::Relaxed);
            s.child.clone()
        };
        child.lock().kill();
        Ok(())
    }

    /// Remove a session from the registry entirely. If it is still running it
    /// is force-stopped first; any task it was mid-flight on is failed.
    pub fn remove(&self, sid: &str) -> Result<()> {
        let (removed, current_task) = {
            let mut sessions = self.sessions.lock();
            match sessions.remove(sid) {
                Some(s) => {
                    if !s.info.status.is_terminal() {
                        s.stop.store(true, Ordering::Relaxed);
                        s.child.lock().kill();
                    }
                    (true, s.info.current_task.clone())
                    // `s` (and its handles) drop here; the reader thread keeps
                    // its own Arc to the child until it sees EOF, then exits.
                }
                None => (false, None),
            }
        };
        if removed {
            self.attach_lru.lock().retain(|x| x != sid);
            if let Some(tid) = current_task {
                self.fail_task(&tid, "session removed");
            }
            self.emit(events::SESSION_REMOVED, serde_json::json!({ "id": sid }));
        }
        Ok(())
    }

    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        let mut v: Vec<SessionInfo> = self.sessions.lock().values().map(|s| s.info.clone()).collect();
        v.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        v
    }

    pub fn get_logs(&self, sid: &str, limit: i64) -> Vec<LogEntry> {
        let db = self.db.lock();
        storage::recent_logs(&db, sid, limit)
    }

    // ---- tasks ------------------------------------------------------------

    pub fn enqueue_task(&self, mut task: Task) -> TaskId {
        if task.id.is_empty() {
            task.id = Uuid::new_v4().to_string();
        }
        task.status = TaskStatus::Queued;
        task.created_at = now_ms();
        let t = task.clone();
        self.tasks.lock().insert(task.id.clone(), task);
        {
            let db = self.db.lock();
            storage::upsert_task(&db, &t);
        }
        self.emit(events::TASK_UPDATE, t.clone());
        t.id
    }

    pub fn list_tasks(&self) -> Vec<Task> {
        let mut v: Vec<Task> = self.tasks.lock().values().cloned().collect();
        v.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.created_at.cmp(&b.created_at)));
        v
    }

    pub fn cancel_task(&self, tid: &str) {
        let updated = {
            let mut tasks = self.tasks.lock();
            tasks.get_mut(tid).filter(|t| t.status == TaskStatus::Queued).map(|t| {
                t.status = TaskStatus::Canceled;
                t.finished_at = Some(now_ms());
                t.clone()
            })
        };
        if let Some(t) = updated {
            let db = self.db.lock();
            storage::upsert_task(&db, &t);
            drop(db);
            self.emit(events::TASK_UPDATE, t);
        }
    }

    pub fn task_counts(&self) -> (usize, usize) {
        let tasks = self.tasks.lock();
        let queued = tasks.values().filter(|t| t.status == TaskStatus::Queued).count();
        let running = tasks
            .values()
            .filter(|t| matches!(t.status, TaskStatus::Running | TaskStatus::Assigned))
            .count();
        (queued, running)
    }

    pub fn count_active_headless(&self) -> usize {
        self.sessions
            .lock()
            .values()
            .filter(|s| s.info.mode == SessionMode::Headless && s.info.status.is_active())
            .count()
    }

    /// Pop the highest-priority queued task, marking it `Assigned`.
    pub fn take_next_task(&self) -> Option<Task> {
        let mut tasks = self.tasks.lock();
        let next_id = tasks
            .values()
            .filter(|t| t.status == TaskStatus::Queued)
            .max_by(|a, b| a.priority.cmp(&b.priority).then(b.created_at.cmp(&a.created_at)))
            .map(|t| t.id.clone())?;
        let t = tasks.get_mut(&next_id)?;
        t.status = TaskStatus::Assigned;
        Some(t.clone())
    }

    /// Find an idle pooled headless session matching a task's context.
    pub fn find_idle_session(&self, task: &Task) -> Option<SessionId> {
        let sessions = self.sessions.lock();
        sessions
            .values()
            .find(|s| {
                s.info.mode == SessionMode::Headless
                    && s.info.status == SessionStatus::Idle
                    && s.info.current_task.is_none()
                    && s.info.cwd == task.cwd
                    && s.spec.model == task.model
            })
            .map(|s| s.info.id.clone())
    }

    /// Bind a task to a session and start the turn.
    pub fn assign_task_to_session(self: &Arc<Self>, sid: &str, task: &Task) {
        let updated = {
            let mut tasks = self.tasks.lock();
            tasks.get_mut(&task.id).map(|t| {
                t.status = TaskStatus::Running;
                t.session_id = Some(sid.to_string());
                t.started_at = Some(now_ms());
                t.attempts += 1;
                t.clone()
            })
        };
        if let Some(t) = &updated {
            let db = self.db.lock();
            storage::upsert_task(&db, t);
            drop(db);
            self.emit(events::TASK_UPDATE, t.clone());
        }
        self.update_info(sid, |i| i.current_task = Some(task.id.clone()));
        if let Err(e) = self.send_message(sid, &task.prompt) {
            self.fail_task(&task.id, &format!("send failed: {e}"));
        }
    }

    pub fn fail_task(&self, tid: &str, err: &str) {
        let updated = {
            let mut tasks = self.tasks.lock();
            tasks.get_mut(tid).map(|t| {
                t.status = TaskStatus::Failed;
                t.finished_at = Some(now_ms());
                t.error = Some(err.to_string());
                t.clone()
            })
        };
        if let Some(t) = updated {
            let db = self.db.lock();
            storage::upsert_task(&db, &t);
            drop(db);
            self.emit(events::TASK_UPDATE, t);
        }
    }

    // ---- config -----------------------------------------------------------

    pub fn get_config(&self) -> OrchestratorConfig {
        self.config.lock().clone()
    }

    pub fn set_config(&self, cfg: OrchestratorConfig) {
        {
            let db = self.db.lock();
            let _ = storage::save_config(&db, &cfg);
        }
        *self.config.lock() = cfg;
    }

    // ---- snapshots --------------------------------------------------------

    pub fn save_snapshot(&self, name: &str) -> Result<String> {
        let sessions: Vec<SnapSession> = self
            .sessions
            .lock()
            .values()
            .filter(|s| !s.info.status.is_terminal())
            .map(|s| SnapSession {
                name: s.info.name.clone(),
                mode: s.info.mode,
                cwd: s.info.cwd.clone(),
                model: s.info.model.clone(),
                agent_session_id: s.info.agent_session_id.clone(),
            })
            .collect();
        let snap = SnapshotData {
            sessions,
            config: self.config.lock().clone(),
        };
        let id = Uuid::new_v4().to_string();
        let json = serde_json::to_string(&snap)?;
        let db = self.db.lock();
        storage::save_snapshot(&db, &id, name, &json)?;
        Ok(id)
    }

    pub fn list_snapshots(&self) -> Vec<(String, String, i64)> {
        let db = self.db.lock();
        storage::list_snapshots(&db)
    }

    /// Recreate the sessions captured in a snapshot, resuming agent
    /// conversations where an id was recorded.
    pub fn restore_snapshot(self: &Arc<Self>, id: &str) -> Result<usize> {
        let json = {
            let db = self.db.lock();
            storage::load_snapshot(&db, id).ok_or_else(|| anyhow!("snapshot not found"))?
        };
        let snap: SnapshotData = serde_json::from_str(&json)?;
        let mut n = 0;
        for s in snap.sessions {
            let spec = SessionSpec {
                name: Some(s.name),
                mode: s.mode,
                cwd: s.cwd,
                model: s.model,
                permission_mode: None,
                allowed_tools: None,
                initial_prompt: None,
                resume: s.agent_session_id,
            };
            let res = match s.mode {
                SessionMode::Headless => self.spawn_headless(spec),
                SessionMode::Interactive => self.spawn_interactive(spec),
                SessionMode::Shell => self.spawn_shell(spec),
            };
            if res.is_ok() {
                n += 1;
            }
        }
        Ok(n)
    }

    // ---- project files ----------------------------------------------------

    /// Open every session described in a JSON project file. Per-session fields
    /// override the shared defaults; sessions need no name (auto-generated).
    /// Returns the number of sessions launched.
    pub fn open_project(self: &Arc<Self>, path: &str) -> Result<OpenProjectResult> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
        let proj: ProjectFile =
            serde_json::from_str(&text).context("parsing project file (expected JSON)")?;
        let shared_cwd = proj.cwd.clone();
        let shared_model = proj.model.clone();
        let shared_perm = proj.permission_mode.clone();
        let mut n = 0;
        for s in proj.sessions {
            let cwd = match s.cwd.or_else(|| shared_cwd.clone()) {
                Some(c) if !c.trim().is_empty() => c,
                _ => continue, // a session with no cwd anywhere is skipped
            };
            let spec = SessionSpec {
                name: s.name,
                mode: s.mode,
                cwd,
                model: s.model.or_else(|| shared_model.clone()),
                permission_mode: s.permission_mode.or_else(|| shared_perm.clone()),
                allowed_tools: s.allowed_tools,
                initial_prompt: s.prompt,
                resume: s.resume,
            };
            let res = match spec.mode {
                SessionMode::Headless => self.spawn_headless(spec),
                SessionMode::Interactive => self.spawn_interactive(spec),
                SessionMode::Shell => self.spawn_shell(spec),
            };
            if res.is_ok() {
                n += 1;
            }
        }
        Ok(OpenProjectResult {
            count: n,
            cwd: shared_cwd,
            model: shared_model,
            permission_mode: shared_perm,
        })
    }

    /// Write a project definition (authored in the UI) to a JSON file.
    pub fn write_project(&self, path: &str, proj: &ProjectFile) -> Result<()> {
        let json = serde_json::to_string_pretty(proj)?;
        std::fs::write(path, json).with_context(|| format!("writing {path}"))?;
        Ok(())
    }

    /// Export the current live sessions to a JSON project file.
    pub fn save_project(&self, path: &str) -> Result<()> {
        let sessions: Vec<ProjectSession> = self
            .sessions
            .lock()
            .values()
            .filter(|s| !s.info.status.is_terminal())
            .map(|s| ProjectSession {
                mode: s.info.mode,
                cwd: Some(s.info.cwd.clone()),
                model: s.info.model.clone(),
                permission_mode: s.spec.permission_mode.clone(),
                allowed_tools: s.spec.allowed_tools.clone(),
                prompt: None,
                name: Some(s.info.name.clone()),
                resume: s.info.agent_session_id.clone(),
            })
            .collect();
        let proj = ProjectFile {
            cwd: None,
            model: None,
            permission_mode: None,
            sessions,
        };
        let json = serde_json::to_string_pretty(&proj)?;
        std::fs::write(path, json).with_context(|| format!("writing {path}"))?;
        Ok(())
    }
}

/// Result of opening a project: how many sessions launched, plus the shared
/// context so the UI can enter "project mode" and add more like them.
#[derive(serde::Serialize)]
pub struct OpenProjectResult {
    pub count: usize,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
}

/// A shareable, hand-editable description of a set of sessions to open at once.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ProjectFile {
    /// Shared default working directory for sessions that omit their own.
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    permission_mode: Option<String>,
    sessions: Vec<ProjectSession>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ProjectSession {
    mode: SessionMode,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    permission_mode: Option<String>,
    #[serde(default)]
    allowed_tools: Option<String>,
    /// Initial prompt for headless workers.
    #[serde(default)]
    prompt: Option<String>,
    /// Optional; auto-generated from the cwd when omitted.
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    resume: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SnapshotData {
    sessions: Vec<SnapSession>,
    config: OrchestratorConfig,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SnapSession {
    name: String,
    mode: SessionMode,
    cwd: String,
    model: Option<String>,
    agent_session_id: Option<String>,
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

fn default_name(cwd: &str, sid: &str) -> String {
    let base = std::path::Path::new(cwd)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "session".into());
    format!("{base}-{}", &sid[..sid.len().min(4)])
}
