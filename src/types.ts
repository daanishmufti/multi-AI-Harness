// Mirrors the serde types in `src-tauri/src/model.rs`. Keep in sync.

export type SessionMode = "headless" | "interactive" | "shell";

export type SessionStatus =
  | "starting"
  | "idle"
  | "busy"
  | "completed"
  | "crashed"
  | "stopped";

export interface TokenUsage {
  input_tokens: number;
  output_tokens: number;
  cache_creation_tokens: number;
  cache_read_tokens: number;
}

export interface SessionInfo {
  id: string;
  name: string;
  mode: SessionMode;
  status: SessionStatus;
  cwd: string;
  model: string | null;
  agent_session_id: string | null;
  attached: boolean;
  pid: number | null;
  created_at: number;
  last_activity: number;
  usage: TokenUsage;
  cost_usd: number;
  turns: number;
  current_task: string | null;
  activity: string;
  cpu: number;
  mem_bytes: number;
  exit_code: number | null;
}

export interface SessionSpec {
  name?: string | null;
  mode: SessionMode;
  cwd: string;
  model?: string | null;
  permission_mode?: string | null;
  allowed_tools?: string | null;
  initial_prompt?: string | null;
  resume?: string | null;
}

export type TaskStatus =
  | "queued"
  | "assigned"
  | "running"
  | "done"
  | "failed"
  | "canceled";

export interface Task {
  id: string;
  prompt: string;
  cwd: string;
  model: string | null;
  permission_mode: string | null;
  allowed_tools: string | null;
  status: TaskStatus;
  session_id: string | null;
  priority: number;
  attempts: number;
  max_attempts: number;
  created_at: number;
  started_at: number | null;
  finished_at: number | null;
  result_summary: string | null;
  error: string | null;
}

export interface LogEntry {
  id: number;
  session_id: string;
  ts: number;
  kind: string;
  text: string;
  data: string | null;
}

export interface SystemMetrics {
  cpu_percent: number;
  mem_used: number;
  mem_total: number;
  active_sessions: number;
  total_sessions: number;
  queued_tasks: number;
  running_tasks: number;
  total_tokens: number;
  total_cost_usd: number;
  ts: number;
}

export interface OrchestratorConfig {
  max_concurrent: number;
  max_attached: number;
  cpu_limit_percent: number;
  mem_limit_percent: number;
  auto_restart: boolean;
  session_pooling: boolean;
  agent_bin: string;
  shell_program: string;
  default_model: string | null;
  default_permission_mode: string;
  bare_mode: boolean;
  ring_buffer_bytes: number;
}

export interface TermData {
  session_id: string;
  data: string; // base64
  replay: boolean;
}

export interface SnapshotMeta {
  id: string;
  name: string;
  created_at: number;
}

export interface ProjectSessionDef {
  mode: SessionMode;
  cwd?: string | null;
  model?: string | null;
  permission_mode?: string | null;
  allowed_tools?: string | null;
  prompt?: string | null;
  name?: string | null;
  resume?: string | null;
}

export interface ProjectDef {
  cwd?: string | null;
  model?: string | null;
  permission_mode?: string | null;
  sessions: ProjectSessionDef[];
}

export interface OpenProjectResult {
  count: number;
  cwd: string | null;
  model: string | null;
  permission_mode: string | null;
}

/** The shared context the UI keeps while "in project mode". */
export interface ProjectContext {
  cwd: string;
  model: string | null;
  permission_mode: string;
}
