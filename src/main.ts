import "@xterm/xterm/css/xterm.css";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ask, open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { api, fmtBytes, fmtTokens, on } from "./api";
import { installKeyboard } from "./keyboard";
import { modalForm } from "./modal";
import { openNewProjectModal } from "./newproject";
import { installSidebarResizer } from "./sidebar";
import { installTaskbarResizer } from "./taskbar";
import { Panes } from "./terminals";
import { Voice } from "./voice";
import type { OrchestratorConfig, ProjectContext, SessionInfo, SessionMode, Task } from "./types";

// ---- state -----------------------------------------------------------------

const sessions = new Map<string, SessionInfo>();
const tasks = new Map<string, Task>();
let selectedId: string | null = null;
let filterText = "";
let config: OrchestratorConfig | null = null;
let activeProject: ProjectContext | null = null;
/** Whether voice mode is currently listening. */
let voiceOn = false;
/** When set, `renderSessions` skips work so an in-flight rename input is not
 *  ripped out of the DOM (which would blur the field and commit too early). */
let editingId: string | null = null;
/** The element (sidebar row or pane) the cursor is currently over. */
let hoveredId: string | null = null;
/** Pane most recently clicked (used by mode = "pane"). */
let paneClickedId: string | null = null;
/** Target set via "lane switch to <name>" voice commands (mode = "command"). */
let commandTargetId: string | null = null;

/** Which strategy decides the voice target. Persists across restarts. */
type VoiceMode = "sidebar" | "hover" | "pane" | "command";
let voiceMode: VoiceMode = (localStorage.getItem("agentlane.voiceMode") as VoiceMode | null) ?? "sidebar";
const MODE_HINT: Record<VoiceMode, string> = {
  sidebar: "select a row in the sidebar",
  hover:   "hover a session",
  pane:    "click a pane",
  command: 'say “lane switch to <name>”',
};

/** Resolve the current voice target according to the active mode. */
function voiceTargetSession(): SessionInfo | null {
  let id: string | null = null;
  switch (voiceMode) {
    case "sidebar": id = selectedId; break;
    case "hover":   id = hoveredId ?? selectedId; break;
    case "pane":    id = paneClickedId ?? selectedId; break;
    case "command": id = commandTargetId ?? selectedId; break;
  }
  return id ? sessions.get(id) ?? null : null;
}

const el = <T extends HTMLElement>(sel: string): T => document.querySelector(sel) as T;
const panes = new Panes(el("#panes"));

// ---- selectors / derived ---------------------------------------------------

function visibleSessions(): SessionInfo[] {
  const list = [...sessions.values()].sort((a, b) => a.created_at - b.created_at);
  if (!filterText) return list;
  const f = filterText.toLowerCase();
  return list.filter(
    (s) =>
      s.name.toLowerCase().includes(f) ||
      s.cwd.toLowerCase().includes(f) ||
      s.status.includes(f),
  );
}

// ---- rendering --------------------------------------------------------------

function renderSessions(): void {
  // Freeze the list while the user is editing a name so the input element
  // stays in the DOM (detaching it would blur and commit prematurely).
  if (editingId) return;
  const list = el<HTMLUListElement>("#session-list");
  const vis = visibleSessions();
  if (selectedId && !sessions.has(selectedId)) selectedId = null;
  if (!selectedId && vis.length) selectedId = vis[0].id;

  list.replaceChildren(
    ...vis.map((s) => {
      const li = document.createElement("li");
      li.className = "session-row";
      li.classList.toggle("selected", s.id === selectedId);
      li.classList.toggle("attached", s.attached);
      li.dataset.id = s.id;
      const tokens = s.usage
        ? s.usage.input_tokens +
          s.usage.output_tokens +
          s.usage.cache_creation_tokens +
          s.usage.cache_read_tokens
        : 0;
      const modeLabel = s.mode === "headless" ? "agent" : s.mode === "interactive" ? "claude" : "term";
      li.innerHTML =
        `<span class="dot dot-${s.status}"></span>` +
        `<span class="name" title="Double-click to rename">${esc(s.name)}</span>` +
        `<span class="row-actions">` +
        `<span class="badge ${s.mode}">${modeLabel}</span>` +
        `<button class="row-rename" title="Rename">✎</button>` +
        `<button class="row-remove" title="Remove session (Del)">✕</button>` +
        `</span>` +
        `<span class="meta">` +
        `<span>${esc(s.activity)}</span>` +
        `<span>cpu ${s.cpu.toFixed(0)}%</span>` +
        `<span>${fmtBytes(s.mem_bytes)}</span>` +
        `<span class="tok">${fmtTokens(tokens)} tok</span>` +
        (s.cost_usd > 0 ? `<span class="cost">$${s.cost_usd.toFixed(3)}</span>` : "") +
        `</span>`;
      li.onclick = () => selectSession(s.id);
      li.ondblclick = () => api.attachSession(s.id);
      const rm = li.querySelector(".row-remove") as HTMLButtonElement | null;
      if (rm) rm.onclick = (e) => { e.stopPropagation(); void removeSession(s); };
      // Double-click the name (or click the ✎ button) to rename in place.
      // stopPropagation so the row's dblclick→attach handler doesn't also fire.
      const nameEl = li.querySelector(".name") as HTMLElement | null;
      if (nameEl) nameEl.ondblclick = (e) => { e.stopPropagation(); inlineRename(s, nameEl); };
      const rn = li.querySelector(".row-rename") as HTMLButtonElement | null;
      if (rn && nameEl) rn.onclick = (e) => { e.stopPropagation(); inlineRename(s, nameEl); };
      return li;
    }),
  );
}

function renderMetrics(): void {
  const box = el("#metrics");
  const m = lastMetrics;
  if (!m) {
    box.textContent = "—";
    return;
  }
  const memPct = m.mem_total ? (m.mem_used / m.mem_total) * 100 : 0;
  const cpuWarn = !!config && m.cpu_percent > config.cpu_limit_percent;
  const memWarn = !!config && memPct > config.mem_limit_percent;
  box.innerHTML =
    metric("CPU", `${m.cpu_percent.toFixed(0)}%`, cpuWarn) +
    metric("RAM", `${memPct.toFixed(0)}%`, memWarn) +
    metric("Sessions", `${m.active_sessions}/${m.total_sessions}`) +
    metric("Queue", `${m.queued_tasks}`) +
    metric("Running", `${m.running_tasks}`) +
    metric("Tokens", fmtTokens(m.total_tokens)) +
    metric("Cost", `$${m.total_cost_usd.toFixed(3)}`);
}

function metric(label: string, value: string, warn = false): string {
  return `<span class="m ${warn ? "warn" : ""}">${label} <b>${value}</b></span>`;
}

function renderTasks(): void {
  const box = el("#task-list");
  const list = [...tasks.values()].sort(
    (a, b) => b.priority - a.priority || a.created_at - b.created_at,
  );
  box.replaceChildren(
    ...list.map((t) => {
      const row = document.createElement("div");
      row.className = "task-row";
      const cancelBtn =
        t.status === "queued"
          ? `<button data-cancel="${t.id}">cancel</button>`
          : `<span></span>`;
      row.innerHTML =
        `<span class="tstatus ${t.status}">${t.status}</span>` +
        `<span class="tprompt">${esc(t.prompt)}</span>` +
        `<span class="tcwd">${esc(shortPath(t.cwd))}</span>` +
        cancelBtn;
      const btn = row.querySelector("button[data-cancel]");
      if (btn) (btn as HTMLButtonElement).onclick = () => api.cancelTask(t.id);
      return row;
    }),
  );
}

let lastMetrics: import("./types").SystemMetrics | null = null;

// ---- selection + actions ----------------------------------------------------

function selectSession(id: string): void {
  selectedId = id;
  renderSessions();
  renderVoiceTarget();
}

/** Top-bar `🎙 → <name>` pill plus the sidebar row mic mark for the current target. */
function renderVoiceTarget(): void {
  const pill = el("#voice-target");
  const s = voiceOn ? voiceTargetSession() : null;
  if (voiceOn) {
    pill.classList.remove("hidden");
    const tip = MODE_HINT[voiceMode];
    pill.textContent = s ? `🎙 → ${s.name}` : `🎙 → (${tip})`;
  } else {
    pill.classList.add("hidden");
    pill.textContent = "";
  }
  // Mirror the target onto the matching session row so the user can see which
  // one voice is talking to even when the cursor is over a pane.
  document.querySelectorAll<HTMLElement>(".session-row.voice-target")
    .forEach((r) => r.classList.remove("voice-target"));
  if (s) {
    document.querySelector<HTMLElement>(`.session-row[data-id="${s.id}"]`)
      ?.classList.add("voice-target");
  }
}

function moveSelection(delta: number): void {
  const vis = visibleSessions();
  if (!vis.length) return;
  const idx = Math.max(0, vis.findIndex((s) => s.id === selectedId));
  const next = (idx + delta + vis.length) % vis.length;
  selectSession(vis[next].id);
  el(`.session-row[data-id="${vis[next].id}"]`)?.scrollIntoView({ block: "nearest" });
}

function attachSelected(): void {
  if (selectedId) api.attachSession(selectedId).catch(reportErr);
}
function detachSelected(): void {
  if (selectedId) api.detachSession(selectedId);
}
async function killSelected(): Promise<void> {
  if (!selectedId) return;
  const s = sessions.get(selectedId);
  if (!s) return;
  if (s.status === "completed" || s.status === "crashed" || s.status === "stopped") {
    await api.removeSession(selectedId).catch(reportErr);
  } else {
    await api.killSession(selectedId).catch(reportErr);
  }
}

function isTerminal(s: SessionInfo): boolean {
  return s.status === "completed" || s.status === "crashed" || s.status === "stopped";
}

/** Remove a session entirely; confirm first if it's still running. */
async function removeSession(s: SessionInfo): Promise<void> {
  if (!isTerminal(s)) {
    const ok = await ask(`Remove "${s.name}"? It is still running and will be killed.`, {
      title: "Remove session",
      kind: "warning",
    });
    if (!ok) return;
  }
  await api.removeSession(s.id).catch(reportErr);
}

function removeSelected(): void {
  if (selectedId) {
    const s = sessions.get(selectedId);
    if (s) void removeSession(s);
  }
}

// ---- voice-mode helpers -----------------------------------------------------

function setVoiceMode(m: VoiceMode): void {
  voiceMode = m;
  localStorage.setItem("agentlane.voiceMode", m);
  // Fresh start on switch — clear the mode-specific accumulators.
  if (m !== "pane") paneClickedId = null;
  if (m !== "command") commandTargetId = null;
  renderVoiceTarget();
}

/** Cycle the command-mode target through the live sessions. */
function cycleCommandTarget(delta: 1 | -1): void {
  const list = visibleSessions().filter((s) =>
    s.status !== "completed" && s.status !== "crashed" && s.status !== "stopped",
  );
  if (!list.length) return;
  const cur = commandTargetId ?? selectedId;
  const idx = Math.max(0, list.findIndex((s) => s.id === cur));
  commandTargetId = list[(idx + delta + list.length) % list.length].id;
  renderVoiceTarget();
}

function findSessionByName(query: string): SessionInfo | null {
  const q = query.toLowerCase().trim();
  if (!q) return null;
  const list = [...sessions.values()];
  return (
    list.find((s) => s.name.toLowerCase() === q) ||
    list.find((s) => s.name.toLowerCase().startsWith(q)) ||
    list.find((s) => s.name.toLowerCase().includes(q)) ||
    null
  );
}

/** Parse a transcript as a "lane …" voice command. Returns true if handled. */
function handleVoiceCommand(text: string): boolean {
  if (voiceMode !== "command") return false;
  const cmd = text.toLowerCase().replace(/[.!?]$/g, "").trim();
  if (!cmd.startsWith("lane")) return false;
  const rest = cmd.replace(/^lane[,\s]*/, "").trim();
  if (rest === "next" || rest === "next session") { cycleCommandTarget(1); return true; }
  if (rest === "previous" || rest === "prev" || rest === "previous session") {
    cycleCommandTarget(-1); return true;
  }
  const switchMatch = rest.match(/^(?:switch to|select|target|use)\s+(.+)$/);
  if (switchMatch) {
    const s = findSessionByName(switchMatch[1]);
    if (s) { commandTargetId = s.id; renderVoiceTarget(); reportErr(`Voice target → ${s.name}`); }
    else reportErr(`No session matches "${switchMatch[1]}"`);
    return true;
  }
  const bcastMatch = rest.match(/^broadcast[:\s]+(.+)$/);
  if (bcastMatch) {
    api.broadcast(bcastMatch[1]).catch(reportErr);
    return true;
  }
  if (rest === "stop" || rest === "off") {
    el<HTMLButtonElement>("#voice-btn").click();
    return true;
  }
  reportErr(`Voice: unknown command "${cmd}"`);
  return true; // swallow so we don't accidentally send "lane …" as a message
}

/** Replace the name span with an input. Enter saves, Esc / blank cancels. */
function inlineRename(s: SessionInfo, span: HTMLElement): void {
  if (editingId) return; // already editing something
  editingId = s.id;
  const orig = s.name;
  const input = document.createElement("input");
  input.className = "name-edit";
  input.value = orig;
  let done = false;
  const finish = (save: boolean) => {
    if (done) return;
    done = true;
    editingId = null;
    const v = input.value.trim();
    if (save && v && v !== orig) {
      span.textContent = v; // optimistic — backend confirms via session:update
      api.renameSession(s.id, v).catch((e) => { span.textContent = orig; reportErr(e); });
    }
    if (input.parentElement) input.replaceWith(span);
    // Catch up on any session updates that were skipped during the edit.
    renderSessions();
  };
  input.onkeydown = (e) => {
    if (e.key === "Enter") { e.preventDefault(); finish(true); }
    else if (e.key === "Escape") { e.preventDefault(); finish(false); }
  };
  input.onblur = () => finish(true);
  span.replaceWith(input);
  input.focus();
  input.select();
}

// ---- create flows -----------------------------------------------------------

async function newSession(mode: SessionMode): Promise<void> {
  const fields: any[] = [
    { key: "cwd", label: "Working directory", type: "dir", placeholder: "Browse or type a path…" },
  ];
  // Shells don't take a model; agent sessions do.
  if (mode !== "shell") {
    fields.push({ key: "model", label: "Model (blank = default)", placeholder: "claude-opus-4-7 / claude-sonnet-4-6" });
  }
  fields.push({ key: "name", label: "Name (optional)" });
  if (mode === "headless") {
    fields.push({
      key: "permission_mode",
      label: "Permission mode",
      type: "select",
      value: config?.default_permission_mode ?? "acceptEdits",
      options: [
        { value: "acceptEdits", label: "acceptEdits" },
        { value: "default", label: "default" },
        { value: "dontAsk", label: "dontAsk" },
        { value: "bypassPermissions", label: "bypassPermissions" },
      ],
    });
    fields.push({ key: "initial_prompt", label: "Initial prompt (optional)", type: "textarea" });
  } else {
    // Shell and interactive agent — initial line is typed into the PTY at start.
    fields.push({
      key: "initial_prompt",
      label: mode === "shell" ? "Initial command (optional)" : "Initial Claude input (optional)",
      type: "textarea",
      placeholder: mode === "shell" ? "e.g. cd src; ls" : "",
    });
  }
  const title =
    mode === "headless" ? "New headless worker" :
    mode === "interactive" ? "New interactive Claude" :
    "New terminal";
  const res = await modalForm(title, fields);
  if (!res) return;
  if (!res.cwd.trim()) return reportErr("Working directory is required");
  await api
    .createSession({
      mode,
      cwd: res.cwd.trim(),
      model: res.model?.trim() || null,
      name: res.name?.trim() || null,
      permission_mode: res.permission_mode || null,
      initial_prompt: res.initial_prompt?.trim() || null,
    })
    .catch(reportErr);
}

async function newTask(): Promise<void> {
  const res = await modalForm("Queue a task", [
    { key: "prompt", label: "Prompt", type: "textarea", placeholder: "Describe the task…" },
    { key: "cwd", label: "Working directory", type: "dir", placeholder: "Browse or type a path…" },
    { key: "model", label: "Model (blank = default)" },
    { key: "priority", label: "Priority", type: "number", value: 0 },
    { key: "max_attempts", label: "Max attempts", type: "number", value: 1 },
  ], "Queue");
  if (!res) return;
  if (!res.prompt.trim() || !res.cwd.trim()) return reportErr("Prompt and cwd are required");
  await api
    .enqueueTask({
      id: "",
      prompt: res.prompt.trim(),
      cwd: res.cwd.trim(),
      model: res.model?.trim() || null,
      permission_mode: null,
      allowed_tools: null,
      status: "queued",
      session_id: null,
      priority: parseInt(res.priority || "0", 10) || 0,
      attempts: 0,
      max_attempts: parseInt(res.max_attempts || "1", 10) || 1,
      created_at: 0,
      started_at: null,
      finished_at: null,
      result_summary: null,
      error: null,
    })
    .catch(reportErr);
}

async function openSettings(): Promise<void> {
  if (!config) config = await api.getConfig();
  const res = await modalForm("Orchestrator settings", [
    { key: "max_concurrent", label: "Max concurrent sessions", type: "number", value: config.max_concurrent },
    { key: "max_attached", label: "Max rendered terminals (1–16)", type: "number", value: config.max_attached },
    { key: "cpu_limit_percent", label: "CPU throttle %", type: "number", value: config.cpu_limit_percent },
    { key: "mem_limit_percent", label: "Memory throttle %", type: "number", value: config.mem_limit_percent },
    { key: "default_model", label: "Default model", value: config.default_model ?? "" },
    {
      key: "default_permission_mode", label: "Default permission mode", type: "select",
      value: config.default_permission_mode,
      options: [
        { value: "acceptEdits", label: "acceptEdits" },
        { value: "default", label: "default" },
        { value: "dontAsk", label: "dontAsk" },
        { value: "bypassPermissions", label: "bypassPermissions" },
      ],
    },
    { key: "agent_bin", label: "Claude executable (blank = auto-detect)", value: config.agent_bin },
    { key: "shell_program", label: "Terminal shell (blank = auto: pwsh / powershell / cmd)", value: config.shell_program },
    { key: "auto_restart", label: "Auto-restart crashed tasks", type: "checkbox", value: config.auto_restart },
    { key: "session_pooling", label: "Reuse idle pooled workers", type: "checkbox", value: config.session_pooling },
    { key: "bare_mode", label: "Bare mode (faster startup)", type: "checkbox", value: config.bare_mode },
  ], "Save");
  if (!res) return;
  const next: OrchestratorConfig = {
    ...config,
    max_concurrent: clampInt(res.max_concurrent, 1, 256, config.max_concurrent),
    max_attached: clampInt(res.max_attached, 1, 16, config.max_attached),
    cpu_limit_percent: Number(res.cpu_limit_percent) || config.cpu_limit_percent,
    mem_limit_percent: Number(res.mem_limit_percent) || config.mem_limit_percent,
    default_model: res.default_model.trim() || null,
    default_permission_mode: res.default_permission_mode,
    agent_bin: res.agent_bin.trim(),
    shell_program: res.shell_program.trim(),
    auto_restart: res.auto_restart === "true",
    session_pooling: res.session_pooling === "true",
    bare_mode: res.bare_mode === "true",
  };
  await api.setConfig(next);
  config = next;
  renderMetrics();
}

const PROJECT_FILTERS = [{ name: "Project (JSON)", extensions: ["json"] }];

/** Batch-open sessions defined in a shared project file (no naming needed). */
async function openProject(): Promise<void> {
  const path = await openDialog({
    multiple: false,
    directory: false,
    title: "Open project file",
    filters: PROJECT_FILTERS,
  });
  if (typeof path !== "string") return;
  try {
    const res = await api.openProject(path);
    reportErr(`Opened ${res.count} session(s) from project`);
    if (res.cwd) {
      setActiveProject({
        cwd: res.cwd,
        model: res.model,
        permission_mode: res.permission_mode ?? "acceptEdits",
      });
    }
  } catch (e) {
    reportErr(e);
  }
}

// ---- active project mode ----------------------------------------------------
// Once a project is created/opened, keep its shared context so you can add more
// sessions of any type with one click — no re-specifying.

function setActiveProject(ctx: ProjectContext | null): void {
  activeProject = ctx;
  renderProjectBar();
}

function renderProjectBar(): void {
  const bar = el("#project-bar");
  if (!activeProject) {
    bar.classList.add("hidden");
    bar.replaceChildren();
    return;
  }
  bar.classList.remove("hidden");
  bar.innerHTML =
    `<div class="pb-head">` +
    `<span class="pb-name" title="${esc(activeProject.cwd)}">📁 ${esc(shortPath(activeProject.cwd))}</span>` +
    `<button class="pb-clear" title="Exit project mode">✕</button>` +
    `</div>` +
    `<div class="pb-adds">` +
    `<button data-m="headless">+ Worker</button>` +
    `<button data-m="interactive">+ Claude</button>` +
    `<button data-m="shell">+ Terminal</button>` +
    `</div>`;
  (bar.querySelector(".pb-clear") as HTMLButtonElement).onclick = () => setActiveProject(null);
  bar.querySelectorAll<HTMLButtonElement>(".pb-adds button").forEach((b) => {
    b.onclick = () => void addToProject(b.dataset.m as SessionMode);
  });
}

/** One-click spawn into the active project's context (no prompts/naming). */
async function addToProject(mode: SessionMode): Promise<void> {
  if (!activeProject) return;
  await api
    .createSession({
      mode,
      cwd: activeProject.cwd,
      model: activeProject.model,
      name: null,
      permission_mode: activeProject.permission_mode,
      initial_prompt: null,
    })
    .catch(reportErr);
}

/** Export the current live sessions to a reusable project file. */
async function saveProject(): Promise<void> {
  const path = await saveDialog({
    title: "Save project file",
    defaultPath: "agentlane.project.json",
    filters: PROJECT_FILTERS,
  });
  if (typeof path !== "string") return;
  await api.saveProject(path).then(() => reportErr("Project saved")).catch(reportErr);
}

async function saveSnapshot(): Promise<void> {
  const res = await modalForm("Save snapshot", [
    { key: "name", label: "Snapshot name", value: `snapshot ${new Date().toLocaleString()}` },
  ], "Save");
  if (res?.name) await api.saveSnapshot(res.name).catch(reportErr);
}

async function restoreSnapshot(): Promise<void> {
  const snaps = await api.listSnapshots();
  if (!snaps.length) return reportErr("No snapshots saved yet");
  const res = await modalForm("Restore snapshot", [
    {
      key: "id", label: "Snapshot", type: "select",
      options: snaps.map((s) => ({ value: s.id, label: `${s.name} — ${new Date(s.created_at).toLocaleString()}` })),
    },
  ], "Restore");
  if (res?.id) {
    const n = await api.restoreSnapshot(res.id).catch(reportErr);
    if (typeof n === "number") reportErr(`Restored ${n} session(s)`);
  }
}

// ---- view modes -------------------------------------------------------------
// `zen`   = hide the side + bottom panels (terminal area fills the window).
// `full`  = OS borderless fullscreen + hide ALL chrome (no edge content).

let zen = false;
let full = false;

function applyViewClasses(): void {
  const app = document.getElementById("app")!;
  app.classList.toggle("zen", zen && !full);
  app.classList.toggle("fullscreen", full);
}

function toggleZen(): void {
  zen = !zen;
  applyViewClasses();
}

async function toggleFullscreen(): Promise<void> {
  full = !full;
  applyViewClasses();
  try {
    await getCurrentWindow().setFullscreen(full);
  } catch (e) {
    reportErr(e);
  }
}

/** Escape unwinds the deepest active view state, one level at a time. */
function escapeViews(): void {
  if (panes.restore()) return; // 1: un-maximize a single pane
  if (full) { void toggleFullscreen(); return; } // 2: leave OS fullscreen
  if (zen) toggleZen(); // 3: leave expand mode
}

// ---- pane sync --------------------------------------------------------------

function syncPane(s: SessionInfo): void {
  if (s.attached && !panes.has(s.id)) panes.ensure(s);
  else if (!s.attached && panes.has(s.id)) panes.remove(s.id);
  // Always refresh title / cwd / status dot on any session change — covers
  // rename, status transitions, etc.
  panes.update(s);
}

// ---- live events ------------------------------------------------------------

function wireEvents(): void {
  on.sessionUpdate((s) => {
    sessions.set(s.id, s);
    syncPane(s);
    renderSessions();
    if (s.id === selectedId) renderVoiceTarget();
  });
  on.sessionRemoved((id) => {
    sessions.delete(id);
    panes.remove(id);
    renderSessions();
  });
  on.termData((t) => panes.onTermData(t));
  on.log((l) => panes.onLog(l));
  on.taskUpdate((t) => {
    tasks.set(t.id, t);
    renderTasks();
  });
  on.metrics((m) => {
    lastMetrics = m;
    renderMetrics();
  });
}

// ---- bootstrap --------------------------------------------------------------

async function init(): Promise<void> {
  config = await api.getConfig();
  for (const s of await api.listSessions()) sessions.set(s.id, s);
  for (const t of await api.listTasks()) tasks.set(t.id, t);
  lastMetrics = await api.getMetrics();

  renderSessions();
  renderTasks();
  renderMetrics();
  for (const s of sessions.values()) if (s.attached) panes.ensure(s);

  wireEvents();
  wireButtons();
  const sidebar = installSidebarResizer();
  el("#sidebar-toggle").onclick = () => sidebar.toggle();
  installTaskbarResizer();

  // Voice mode — sends transcripts to the currently selected session and
  // speaks assistant log lines for it.
  const voice = new Voice({
    getActiveSession: voiceTargetSession,
    notify: reportErr,
    button: el<HTMLButtonElement>("#voice-btn"),
    onChange: (on) => { voiceOn = on; renderVoiceTarget(); },
    interceptTranscript: handleVoiceCommand,
  });
  on.log((l) => voice.onLog(l));

  // Voice mode selector — restore the saved mode and react to changes.
  const modeSel = el<HTMLSelectElement>("#voice-mode");
  modeSel.value = voiceMode;
  modeSel.onchange = () => setVoiceMode(modeSel.value as VoiceMode);

  // Track cursor over any element bearing data-id (sidebar rows AND panes).
  // This drives the "hover" mode and the row mic mark.
  document.addEventListener("mouseover", (e) => {
    const node = (e.target as Element | null)?.closest("[data-id]") as HTMLElement | null;
    const id = node?.dataset.id ?? null;
    if (id !== hoveredId) {
      hoveredId = id;
      if (voiceOn && voiceMode === "hover") renderVoiceTarget();
    }
  });

  // Pane clicks set the target in "pane click" mode. The click can be on any
  // descendant of the pane; we walk up to the pane root via its data-id.
  el("#panes").addEventListener("click", (e) => {
    const node = (e.target as Element | null)?.closest(".pane[data-id]") as HTMLElement | null;
    if (!node) return;
    if (voiceMode === "pane") {
      paneClickedId = node.dataset.id ?? null;
      if (voiceOn) renderVoiceTarget();
    }
  });
  installKeyboard({
    up: () => moveSelection(-1),
    down: () => moveSelection(1),
    attach: attachSelected,
    detach: detachSelected,
    kill: () => void killSelected(),
    remove: removeSelected,
    newHeadless: () => void newSession("headless"),
    newInteractive: () => void newSession("interactive"),
    newShell: () => void newSession("shell"),
    newTask: () => void newTask(),
    focusBroadcast: () => el<HTMLInputElement>("#broadcast-input").focus(),
    focusFilter: () => el<HTMLInputElement>("#filter").focus(),
    focusPane: (n) => { const id = panes.ids()[n]; if (id) panes.focus(id); },
    toggleZen,
    toggleFullscreen: () => void toggleFullscreen(),
    escape: escapeViews,
  });
}

function wireButtons(): void {
  el("#new-headless").onclick = () => void newSession("headless");
  el("#new-interactive").onclick = () => void newSession("interactive");
  el("#new-shell").onclick = () => void newSession("shell");
  el("#new-task").onclick = () => void newTask();
  el("#settings-btn").onclick = () => void openSettings();
  el("#snapshot-save").onclick = () => void saveSnapshot();
  el("#snapshot-restore").onclick = () => void restoreSnapshot();
  el("#new-project").onclick = () => openNewProjectModal(reportErr, setActiveProject);
  el("#open-project").onclick = () => void openProject();
  el("#save-project").onclick = () => void saveProject();
  el("#zen-btn").onclick = () => toggleZen();
  el("#full-btn").onclick = () => void toggleFullscreen();

  el<HTMLInputElement>("#filter").addEventListener("input", (e) => {
    filterText = (e.target as HTMLInputElement).value;
    renderSessions();
  });

  el("#broadcast-form").addEventListener("submit", (e) => {
    e.preventDefault();
    const input = el<HTMLInputElement>("#broadcast-input");
    const v = input.value.trim();
    if (!v) return;
    api.broadcast(v).then((n) => reportErr(`Broadcast to ${n} session(s)`)).catch(reportErr);
    input.value = "";
    input.blur();
  });
}

// ---- utils ------------------------------------------------------------------

function esc(s: string): string {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]!));
}
function shortPath(p: string): string {
  const parts = p.replace(/\\/g, "/").split("/").filter(Boolean);
  return parts.length <= 2 ? p : `…/${parts.slice(-2).join("/")}`;
}
function clampInt(v: string, lo: number, hi: number, fallback: number): number {
  const n = parseInt(v, 10);
  return Number.isFinite(n) ? Math.min(hi, Math.max(lo, n)) : fallback;
}
let toastTimer: number | undefined;
function reportErr(msg: unknown): void {
  // Lightweight transient toast reusing the broadcast input's title bar.
  const box = el("#metrics");
  const note = document.createElement("span");
  note.className = "m warn";
  note.innerHTML = `<b>${esc(String(msg))}</b>`;
  box.prepend(note);
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => note.remove(), 4000);
}

window.addEventListener("DOMContentLoaded", () => void init());
