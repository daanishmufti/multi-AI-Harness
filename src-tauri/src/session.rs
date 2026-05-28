//! Runtime handles for a live session. These are the non-serializable pieces
//! that `SessionInfo` deliberately omits: OS pipes, the ConPTY master, child
//! processes, and the scrollback ring buffer.
//!
//! Writers are wrapped in `Arc<Mutex<_>>` so a command can grab a clone, drop
//! the sessions-map lock, and then perform a potentially blocking write without
//! stalling the whole orchestrator. The child handle is likewise shared so the
//! reader thread can reap it on EOF while the UI thread retains the ability to
//! kill it.

use crate::model::{SessionInfo, SessionSpec};
use crate::ring::RingBuffer;
use parking_lot::Mutex;
use std::io::Write;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Where a session's input goes.
pub enum Transport {
    /// Headless worker: a JSON line written to the child's stdin per turn.
    Headless {
        stdin: Arc<Mutex<std::process::ChildStdin>>,
    },
    /// Interactive ConPTY: raw keystrokes written to the PTY master.
    Interactive {
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    },
}

/// Uniform wrapper over the two kinds of child process so the rest of the code
/// can kill / reap / identify without caring which transport spawned it.
pub enum ChildHandle {
    Std(std::process::Child),
    Pty(Box<dyn portable_pty::Child + Send + Sync>),
}

impl ChildHandle {
    pub fn kill(&mut self) {
        match self {
            ChildHandle::Std(c) => {
                let _ = c.kill();
            }
            ChildHandle::Pty(c) => {
                let _ = c.kill();
            }
        }
    }

    /// Non-blocking reap. Returns the exit code once the process is gone.
    pub fn try_wait(&mut self) -> Option<i32> {
        match self {
            ChildHandle::Std(c) => c.try_wait().ok().flatten().map(|s| s.code().unwrap_or(-1)),
            ChildHandle::Pty(c) => c.try_wait().ok().flatten().map(|s| s.exit_code() as i32),
        }
    }

}

/// The full live state of one session: its serializable [`SessionInfo`], the
/// spec it was launched from (for restart/snapshot), and the runtime handles.
pub struct Session {
    pub info: SessionInfo,
    pub spec: SessionSpec,
    pub transport: Transport,
    pub child: Arc<Mutex<ChildHandle>>,
    /// Recent PTY output, replayed to the UI on attach (interactive only).
    pub ring: Arc<Mutex<RingBuffer>>,
    /// Whether the UI is currently rendering this session's live stream.
    pub attached: Arc<AtomicBool>,
    /// Set to request the reader threads to stop (used on kill/shutdown).
    pub stop: Arc<AtomicBool>,
}
