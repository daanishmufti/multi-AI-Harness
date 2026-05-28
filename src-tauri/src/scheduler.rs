//! The scheduler loop: drains the task queue into sessions, honoring the
//! concurrency cap, resource throttle, and session-pooling policy.
//!
//! Runs on its own thread (spawned in `lib.rs`). Each tick it admits as many
//! queued tasks as the limits allow, preferring to reuse an idle pooled worker
//! over paying the cost of a fresh agent startup.

use crate::core::Core;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const TICK: Duration = Duration::from_millis(400);

pub fn run(core: Arc<Core>) {
    loop {
        tick(&core);
        thread::sleep(TICK);
    }
}

fn tick(core: &Arc<Core>) {
    let cfg = core.get_config();
    let metrics = *core.metrics.lock();

    // Resource throttle: back off entirely while the box is saturated.
    let mem_pct = if metrics.mem_total > 0 {
        (metrics.mem_used as f32 / metrics.mem_total as f32) * 100.0
    } else {
        0.0
    };
    if metrics.cpu_percent > cfg.cpu_limit_percent || mem_pct > cfg.mem_limit_percent {
        return;
    }

    // Admit tasks until we hit the concurrency cap or run dry.
    loop {
        if core.count_active_headless() >= cfg.max_concurrent {
            break;
        }
        let task = match core.take_next_task() {
            Some(t) => t,
            None => break,
        };

        // Reuse an idle pooled worker when one matches (cwd + model).
        if cfg.session_pooling {
            if let Some(sid) = core.find_idle_session(&task) {
                core.assign_task_to_session(&sid, &task);
                continue;
            }
        }

        // Otherwise spin up a fresh headless worker for this task's context.
        let spec = crate::model::SessionSpec {
            name: None,
            mode: crate::model::SessionMode::Headless,
            cwd: task.cwd.clone(),
            model: task.model.clone(),
            permission_mode: task.permission_mode.clone(),
            allowed_tools: task.allowed_tools.clone(),
            initial_prompt: None,
            resume: None,
        };
        match core.spawn_headless(spec) {
            Ok(sid) => core.assign_task_to_session(&sid, &task),
            Err(e) => core.fail_task(&task.id, &format!("spawn failed: {e}")),
        }
    }
}
