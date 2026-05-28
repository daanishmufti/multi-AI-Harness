# AgentLane

A highly-optimized, Windows-first **multi-session orchestrator for Claude Code**. It runs and supervises **50+ concurrent `claude` instances**, headless by default, and lets you attach a live terminal to any one of them on demand. Built for throughput: lean process orchestration, session pooling, lazy terminal rendering, and resource-aware throttling — **not** a custom terminal emulator.

> Inspired by the idea of a fast multi-terminal harness (Ghostty), but deliberately *not* a from-scratch GPU terminal. The hard part here is orchestration, not glyph rendering, so we use a proven terminal widget (xterm.js) and spend our effort on the scheduler, the ConPTY layer, and resource control.

## Architecture

```
┌──────────────────────────────────────────────┐
│  UI Dashboard  (Tauri webview · TS · xterm.js) │  ← sessions, logs, tokens, CPU/RAM, tasks
└───────────────┬──────────────────────────────┘
                │  Tauri IPC (commands + events)
┌───────────────▼──────────────────────────────┐
│  Orchestrator / Scheduler  (Rust)              │  ← task queue, throttle, pooling, crash recovery
├────────────────────────────────────────────── ┤
│  Session Manager                               │  ← registry, attach/detach (LRU), broadcast
├──────────────────┬─────────────────────────── ┤
│  Headless pipe   │  ConPTY (portable-pty)      │  ← two transports
│  (stream-json)   │  interactive TUI            │
└──────────────────┴─────────────────────────── ┘
                │
        Claude Code worker processes (claude.exe)
```

### Two transports, one orchestrator

| | Headless worker (default) | Interactive session |
|---|---|---|
| Wiring | stdin/stdout pipes | Windows **ConPTY** via `portable-pty` |
| Protocol | `--input-format/--output-format stream-json` | raw TTY |
| Strength | structured tokens, tool calls, results, low overhead → **scales to 50+** | full live terminal you can type into |
| Rendered? | only when attached (structured log view) | only when attached (xterm.js) |

Both kinds appear side-by-side in the dashboard with the same status / token / CPU / RAM telemetry.

### Why it stays fast at scale

- **Headless-by-default.** Workers speak `stream-json` over pipes — no PTY, no rendering — until you attach.
- **Lazy rendering.** An xterm.js instance (or log view) is created only on attach and **disposed on detach**. Never more than `max_attached` (1–4) live terminals, regardless of session count. Detached sessions fill a small capped ring buffer that is *replayed once* on attach.
- **Session pooling.** Idle `stream-json` workers stay alive (the protocol is multi-turn) and are **reused** for the next task in the same `cwd`/model instead of paying `claude` startup again.
- **Resource-aware scheduling.** A `sysinfo` monitor samples per-process CPU/RAM and system totals; the scheduler **pauses admitting work** above CPU/RAM thresholds and never exceeds `max_concurrent`.
- **Thread-per-reader, lock-light core.** Writers live behind `Arc<Mutex<_>>` so a command never holds the global sessions lock across blocking I/O.
- **Native `claude.exe`.** Claude Code 2.1+ ships a real executable, spawned directly — no `cmd.exe` shim, no argument-quoting hazards.

## Features

- Dashboard: per-session **status, activity, token usage, cost, CPU, RAM**, plus a system metrics bar.
- **Structured logs** per session (assistant text, tool calls, tool results, results, stderr), persisted to SQLite and streamed live.
- **Task queue** with priority, retries, and automatic **crash recovery** (re-queues an interrupted task).
- **Live attach/detach** to any session; **broadcast** a prompt/keystrokes to every live session at once.
- **Workspace snapshots** — save the current set of sessions and restore them later (resuming Claude conversations by id).
- **Keyboard-first**: `j/k` navigate, `Enter` attach, `d` detach, `x` kill/clear, `n`/`N` new worker/terminal, `t` task, `b` broadcast, `/` filter, `1–4` focus pane.
- Live-editable orchestrator policy (concurrency cap, render cap, throttle thresholds, pooling, bare mode, default model/permission).

## Tech stack

- **Backend:** Rust — `tauri` 2, `portable-pty` (ConPTY), `rusqlite` (bundled SQLite), `sysinfo`, `parking_lot`.
- **UI:** Tauri webview + TypeScript + Vite; **xterm.js** for terminal rendering.
- **Storage:** SQLite (logs, tasks, session history, snapshots, config) under the app-data dir.

## Prerequisites (Windows)

- **Rust** (MSVC toolchain) + **Visual Studio C++ Build Tools** — needed to compile `rusqlite` (bundled) and Tauri.
- **Node.js** 18+ and npm.
- **WebView2 runtime** (preinstalled on Windows 11).
- **Claude Code** installed and authenticated (`npm i -g @anthropic-ai/claude-code`; run `claude` once to log in). The harness auto-detects `claude.exe`; override the path in Settings if needed.

## Run

```powershell
npm install
npm run tauri dev     # dev: hot-reloading UI + debug backend
npm run tauri build   # release: produces an installer + standalone .exe
```

## Usage

1. **+ Worker** launches a headless `stream-json` agent in a chosen directory (optionally with an initial prompt). **+ Terminal** launches an interactive ConPTY session.
2. **+ Task** queues a prompt; the scheduler spins up (or reuses) a worker within the concurrency/resource limits and runs it. Crashed tasks are retried up to `max_attempts`.
3. Click a session (or `j/k`) then **Enter** to attach — an interactive session opens a real terminal; a headless worker opens a live structured log with a message box. Detach frees the pane.
4. Watch the top bar and per-row stats for tokens, cost, CPU, and RAM.

## Opening sessions

Two ways to start work:

1. **Individually** — the **+ Worker / + Claude / + Terminal** buttons open one session at a time, with a folder picker for the directory and per-session settings.
2. **From a project file** — **Open Project** loads a JSON file that defines a whole set of sessions at once. Per-session fields override the shared defaults, and **names are optional** (auto-generated from the directory). **Save Project** exports your current live sessions to such a file. See [`project.example.json`](project.example.json):

```jsonc
{
  "cwd": "C:\\path\\to\\project",   // shared default for sessions that omit their own
  "model": null,
  "permission_mode": "acceptEdits",
  "sessions": [
    { "mode": "headless", "prompt": "Find and summarize all TODOs." },
    { "mode": "headless", "cwd": "C:\\other", "prompt": "Run the tests and fix failures." },
    { "mode": "shell" },
    { "mode": "interactive" }
  ]
}
```

## Notes

- Headless workers use `claude -p` (print mode). Per Anthropic's docs, starting **June 15, 2026**, `claude -p` / Agent SDK usage on subscription plans draws from a separate monthly Agent SDK credit. Plan capacity accordingly when running many workers.
- Default permission mode is `acceptEdits`. Use `bypassPermissions`/`dontAsk` deliberately and only in trusted directories — these workers can edit files and run commands without prompting.

## Project layout

```
src/                     # frontend (TypeScript)
  api.ts                 # typed Tauri command/event wrappers
  terminals.ts           # lazy xterm.js / log-view pane pool
  main.ts                # state, rendering, wiring
  keyboard.ts modal.ts types.ts styles.css
src-tauri/src/           # backend (Rust)
  model.rs               # serde domain types
  protocol.rs            # stream-json parsing + input messages
  launcher.rs            # claude.exe resolution + arg building
  session.rs             # transport/child/ring handles
  core.rs                # orchestrator: lifecycle, events, attach, tasks, snapshots
  scheduler.rs monitor.rs# background loops
  storage.rs             # SQLite
  commands.rs lib.rs     # Tauri command surface + setup
```
