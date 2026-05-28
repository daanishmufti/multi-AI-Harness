//! The resource monitor loop: samples per-process CPU/RAM for every live
//! session and the system as a whole, feeding both the dashboard and the
//! scheduler's throttle. Runs on its own thread (spawned in `lib.rs`).
//!
//! A single long-lived `System` is reused across ticks because `sysinfo`
//! computes CPU usage from the delta between two refreshes.

use crate::core::Core;
use crate::events;
use crate::model::{now_ms, SessionInfo, SessionStatus, SystemMetrics};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use sysinfo::{Pid, ProcessesToUpdate, System};

const TICK: Duration = Duration::from_millis(1500);

pub fn run(core: Arc<Core>) {
    let mut sys = System::new();
    loop {
        sys.refresh_processes(ProcessesToUpdate::All, true);
        sys.refresh_memory();
        sys.refresh_cpu_usage();

        // Apply per-process samples and collect aggregates under the lock.
        let mut changed: Vec<SessionInfo> = Vec::new();
        let (active, total, total_tokens, total_cost) = {
            let mut sessions = core.sessions.lock();
            let mut active = 0usize;
            let mut total_tokens = 0u64;
            let mut total_cost = 0.0f64;
            for s in sessions.values_mut() {
                if let Some(pid) = s.info.pid {
                    if let Some(p) = sys.process(Pid::from_u32(pid)) {
                        s.info.cpu = p.cpu_usage();
                        s.info.mem_bytes = p.memory();
                    } else if !s.info.status.is_terminal() {
                        // Process vanished but we haven't finalized yet.
                        s.info.cpu = 0.0;
                    }
                }
                total_tokens += s.info.usage.total();
                total_cost += s.info.cost_usd;
                if s.info.status.is_active() {
                    active += 1;
                    changed.push(s.info.clone());
                }
            }
            (active, sessions.len(), total_tokens, total_cost)
        };

        let (queued, running) = core.task_counts();
        let metrics = SystemMetrics {
            cpu_percent: sys.global_cpu_usage(),
            mem_used: sys.used_memory(),
            mem_total: sys.total_memory(),
            active_sessions: active,
            total_sessions: total,
            queued_tasks: queued,
            running_tasks: running,
            total_tokens,
            total_cost_usd: total_cost,
            ts: now_ms(),
        };
        *core.metrics.lock() = metrics;
        core.emit_public(events::METRICS, metrics);

        // Refresh live resource numbers for active sessions in the UI.
        for info in changed.into_iter().filter(|i| i.status != SessionStatus::Starting) {
            core.emit_public(events::SESSION_UPDATE, info);
        }

        thread::sleep(TICK);
    }
}
