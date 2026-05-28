// Thin typed wrappers over the Tauri command/event surface.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  LogEntry,
  OpenProjectResult,
  OrchestratorConfig,
  ProjectDef,
  SessionInfo,
  SessionSpec,
  SnapshotMeta,
  SystemMetrics,
  Task,
  TermData,
} from "./types";

// ---- commands --------------------------------------------------------------

export const api = {
  createSession: (spec: SessionSpec) => invoke<string>("create_session", { spec }),
  listSessions: () => invoke<SessionInfo[]>("list_sessions"),
  attachSession: (id: string) => invoke<void>("attach_session", { id }),
  detachSession: (id: string) => invoke<void>("detach_session", { id }),
  sendInput: (id: string, data: string) => invoke<void>("send_input", { id, data }),
  sendMessage: (id: string, prompt: string) => invoke<void>("send_message", { id, prompt }),
  resizeTerminal: (id: string, cols: number, rows: number) =>
    invoke<void>("resize_terminal", { id, cols, rows }),
  broadcast: (text: string) => invoke<number>("broadcast", { text }),
  killSession: (id: string) => invoke<void>("kill_session", { id }),
  removeSession: (id: string) => invoke<void>("remove_session", { id }),
  renameSession: (id: string, name: string) => invoke<void>("rename_session", { id, name }),
  getLogs: (id: string, limit = 500) => invoke<LogEntry[]>("get_logs", { id, limit }),

  enqueueTask: (task: Task) => invoke<string>("enqueue_task", { task }),
  listTasks: () => invoke<Task[]>("list_tasks"),
  cancelTask: (id: string) => invoke<void>("cancel_task", { id }),

  getMetrics: () => invoke<SystemMetrics>("get_metrics"),
  getConfig: () => invoke<OrchestratorConfig>("get_config"),
  setConfig: (config: OrchestratorConfig) => invoke<void>("set_config", { config }),

  saveSnapshot: (name: string) => invoke<string>("save_snapshot", { name }),
  listSnapshots: () => invoke<SnapshotMeta[]>("list_snapshots"),
  restoreSnapshot: (id: string) => invoke<number>("restore_snapshot", { id }),

  openProject: (path: string) => invoke<OpenProjectResult>("open_project", { path }),
  saveProject: (path: string) => invoke<void>("save_project", { path }),
  writeProject: (path: string, project: ProjectDef) => invoke<void>("write_project", { path, project }),
};

// ---- events ----------------------------------------------------------------

export const on = {
  sessionUpdate: (cb: (s: SessionInfo) => void): Promise<UnlistenFn> =>
    listen<SessionInfo>("session:update", (e) => cb(e.payload)),
  sessionRemoved: (cb: (id: string) => void): Promise<UnlistenFn> =>
    listen<{ id: string }>("session:removed", (e) => cb(e.payload.id)),
  log: (cb: (l: LogEntry) => void): Promise<UnlistenFn> =>
    listen<LogEntry>("log", (e) => cb(e.payload)),
  termData: (cb: (t: TermData) => void): Promise<UnlistenFn> =>
    listen<TermData>("term:data", (e) => cb(e.payload)),
  taskUpdate: (cb: (t: Task) => void): Promise<UnlistenFn> =>
    listen<Task>("task:update", (e) => cb(e.payload)),
  metrics: (cb: (m: SystemMetrics) => void): Promise<UnlistenFn> =>
    listen<SystemMetrics>("metrics", (e) => cb(e.payload)),
};

// ---- helpers ---------------------------------------------------------------

/** Decode a base64 payload into bytes for `term.write`. */
export function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function fmtBytes(n: number): string {
  if (n <= 0) return "0";
  const units = ["B", "KB", "MB", "GB"];
  const i = Math.min(units.length - 1, Math.floor(Math.log(n) / Math.log(1024)));
  return `${(n / Math.pow(1024, i)).toFixed(i ? 1 : 0)}${units[i]}`;
}

export function fmtTokens(n: number): string {
  if (n < 1000) return `${n}`;
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  return `${(n / 1_000_000).toFixed(2)}M`;
}
