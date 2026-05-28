//! Resolving the agent CLI executable on Windows and assembling its argument
//! lists for the two transports.
//!
//! The default agent CLI ships a native exe inside its npm package
//! (`.../@anthropic-ai/claude-code/bin/claude.exe`); spawning that directly
//! avoids the `.cmd` shim and all of `cmd.exe`'s argument-quoting hazards,
//! so prompts and tool patterns pass through untouched. A user-configured
//! `agent_bin` overrides this default.

use crate::model::{OrchestratorConfig, SessionSpec};
use std::path::{Path, PathBuf};

/// Locate the agent CLI executable, preferring an explicit config override,
/// then the native npm package's `bin` exe, then anything on `PATH`.
pub fn resolve_agent(config: &OrchestratorConfig) -> String {
    if !config.agent_bin.trim().is_empty() {
        return config.agent_bin.clone();
    }

    // Native exe inside the npm global package (the default agent CLI).
    if let Ok(appdata) = std::env::var("APPDATA") {
        let p = Path::new(&appdata)
            .join("npm")
            .join("node_modules")
            .join("@anthropic-ai")
            .join("claude-code")
            .join("bin")
            .join("claude.exe");
        if p.exists() {
            return p.to_string_lossy().into_owned();
        }
    }

    // Search PATH for the default CLI name.
    if let Some(p) = which_exe("claude.exe").or_else(|| which_exe("claude")) {
        return p.to_string_lossy().into_owned();
    }

    // Last resort: rely on the OS to resolve it.
    "claude.exe".to_string()
}

/// Resolve the shell to launch for a plain terminal session, returning the
/// program and its startup args. Honors a config override, else prefers
/// PowerShell 7 (`pwsh`), then Windows PowerShell, then `cmd`.
pub fn resolve_shell(config: &OrchestratorConfig) -> (String, Vec<String>) {
    let configured = config.shell_program.trim();
    if !configured.is_empty() {
        let lower = configured.to_ascii_lowercase();
        let args = if lower.contains("powershell") || lower.contains("pwsh") {
            vec!["-NoLogo".to_string()]
        } else {
            Vec::new()
        };
        return (configured.to_string(), args);
    }
    if let Some(p) = which_exe("pwsh.exe") {
        return (p.to_string_lossy().into_owned(), vec!["-NoLogo".to_string()]);
    }
    if let Some(p) = which_exe("powershell.exe") {
        return (p.to_string_lossy().into_owned(), vec!["-NoLogo".to_string()]);
    }
    // cmd.exe is always present via ComSpec.
    let cmd = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
    (cmd, Vec::new())
}

/// Minimal `where`-style lookup across `PATH` for an executable name.
fn which_exe(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Build the argument vector for a **headless** worker speaking the
/// bidirectional `stream-json` protocol. The prompt is *not* an argument; it is
/// written to stdin, which sidesteps all shell-quoting concerns.
pub fn headless_args(spec: &SessionSpec, config: &OrchestratorConfig) -> Vec<String> {
    // Note: `--include-partial-messages` is intentionally omitted. We act on
    // whole `assistant`/`result` messages, so streaming token deltas would only
    // add per-line overhead that does not scale to dozens of workers.
    let mut args = vec![
        "--print".to_string(),
        "--input-format".to_string(),
        "stream-json".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
    ];

    if config.bare_mode {
        args.push("--bare".to_string());
    }

    let model = spec.model.clone().or_else(|| config.default_model.clone());
    if let Some(m) = model {
        args.push("--model".to_string());
        args.push(m);
    }

    let perm = spec
        .permission_mode
        .clone()
        .unwrap_or_else(|| config.default_permission_mode.clone());
    if !perm.is_empty() {
        args.push("--permission-mode".to_string());
        args.push(perm);
    }

    if let Some(tools) = &spec.allowed_tools {
        if !tools.is_empty() {
            args.push("--allowedTools".to_string());
            args.push(tools.clone());
        }
    }

    if let Some(resume) = &spec.resume {
        args.push("--resume".to_string());
        args.push(resume.clone());
    }

    args
}

/// Build the argument vector for an **interactive** ConPTY session: the
/// agent's normal TUI, optionally resuming a prior conversation.
pub fn interactive_args(spec: &SessionSpec, config: &OrchestratorConfig) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    let model = spec.model.clone().or_else(|| config.default_model.clone());
    if let Some(m) = model {
        args.push("--model".to_string());
        args.push(m);
    }

    if let Some(resume) = &spec.resume {
        args.push("--resume".to_string());
        args.push(resume.clone());
    }

    args
}
