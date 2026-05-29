// The rendered-pane pool. This is where "lazy rendering" lives: an xterm.js
// instance (or a headless log view) is created only when a session is attached
// and destroyed the moment it detaches, so we never hold more than `max_attached`
// live terminals regardless of how many sessions are running.

import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { api, b64ToBytes } from "./api";
import type { LogEntry, SessionInfo, TermData } from "./types";

interface Pane {
  id: string;
  mode: "headless" | "interactive";
  root: HTMLElement;
  // interactive
  term?: Terminal;
  fit?: FitAddon;
  resizeObs?: ResizeObserver;
  // headless
  logBody?: HTMLElement;
  msgInput?: HTMLInputElement;
}

const THEME = {
  background: "#0b0e14",
  foreground: "#c5c8d3",
  cursor: "#7aa2f7",
  black: "#15161e",
  brightBlack: "#414868",
  red: "#f7768e",
  green: "#9ece6a",
  yellow: "#e0af68",
  blue: "#7aa2f7",
  magenta: "#bb9af7",
  cyan: "#7dcfff",
  white: "#a9b1d6",
};

export class Panes {
  private panes = new Map<string, Pane>();
  private maximizedId: string | null = null;
  constructor(private container: HTMLElement) {}

  /** Refresh the title/cwd/status dot of an already-mounted pane. Called
   *  whenever the session's info changes — e.g. after a rename. */
  update(s: SessionInfo): void {
    const p = this.panes.get(s.id);
    if (!p) return;
    const title = p.root.querySelector(".pane-title");
    if (title) title.textContent = s.name;
    const dot = p.root.querySelector(".dot") as HTMLElement | null;
    if (dot) dot.className = `dot dot-${s.status}`;
    const cwd = p.root.querySelector(".pane-cwd");
    if (cwd) cwd.textContent = s.cwd;
  }

  has(id: string): boolean {
    return this.panes.has(id);
  }

  /** Create a pane for a freshly-attached session (idempotent). */
  ensure(s: SessionInfo): void {
    if (this.panes.has(s.id)) return;
    // Interactive agent and plain shells are real terminals; headless workers
    // render as a structured log stream.
    if (s.mode === "interactive" || s.mode === "shell") this.createTerminal(s);
    else this.createLogView(s);
    this.relayout();
  }

  /** Tear down a pane on detach/removal, freeing the terminal/buffers. */
  remove(id: string): void {
    const p = this.panes.get(id);
    if (!p) return;
    if (this.maximizedId === id) this.maximizedId = null;
    p.resizeObs?.disconnect();
    p.term?.dispose();
    p.root.remove();
    this.panes.delete(id);
    this.relayout();
  }

  /** Expand one pane to fill the whole pane area (and back). */
  toggleMaximize(id: string): void {
    if (!this.panes.has(id)) return;
    this.maximizedId = this.maximizedId === id ? null : id;
    this.relayout();
    if (this.maximizedId) this.focus(this.maximizedId);
  }

  /** Restore from maximized, if any. Used by the Escape key. */
  restore(): boolean {
    if (!this.maximizedId) return false;
    this.maximizedId = null;
    this.relayout();
    return true;
  }

  /** Live terminal bytes for an interactive pane (incl. the replay snapshot). */
  onTermData(t: TermData): void {
    const p = this.panes.get(t.session_id);
    if (p?.term) p.term.write(b64ToBytes(t.data));
  }

  /** Append a structured log line to an attached headless pane. */
  onLog(l: LogEntry): void {
    const p = this.panes.get(l.session_id);
    if (p?.logBody) this.appendLog(p.logBody, l);
  }

  focus(id: string): void {
    const p = this.panes.get(id);
    p?.term?.focus();
    p?.root.scrollIntoView({ block: "nearest" });
    for (const [pid, pane] of this.panes) pane.root.classList.toggle("focused", pid === id);
  }

  ids(): string[] {
    return [...this.panes.keys()];
  }

  // ---- drag-and-drop -------------------------------------------------------
  // Tauri intercepts OS file drops and hands us real disk paths plus a cursor
  // position. We hit-test which pane the cursor is over and drop the path in.

  /** Highlight the pane currently under the dragged files (if any). */
  setDropTarget(clientX: number, clientY: number): void {
    const id = this.paneIdAt(clientX, clientY);
    for (const [pid, p] of this.panes) p.root.classList.toggle("drop-target", pid === id);
  }

  clearDropTarget(): void {
    for (const p of this.panes.values()) p.root.classList.remove("drop-target");
  }

  /** Insert dropped file path(s) into the pane under the cursor. Returns false
   *  if the drop didn't land on a pane (so the caller can hint the user). */
  handleFileDrop(clientX: number, clientY: number, paths: string[]): boolean {
    this.clearDropTarget();
    const id = this.paneIdAt(clientX, clientY);
    const p = id ? this.panes.get(id) : undefined;
    if (!p || !paths.length) return false;
    const text = paths.map(quotePath).join(" ");
    if (p.mode === "interactive") {
      // Live PTY (Claude or shell): type the path(s) straight in, trailing space.
      void api.sendInput(p.id, text + " ");
      p.term?.focus();
    } else if (p.msgInput) {
      // Headless worker: append into the message box without sending.
      const inp = p.msgInput;
      const sep = inp.value && !inp.value.endsWith(" ") ? " " : "";
      inp.value = inp.value + sep + text + " ";
      inp.focus();
    }
    return true;
  }

  private paneIdAt(clientX: number, clientY: number): string | null {
    const node = (document.elementFromPoint(clientX, clientY) as Element | null)
      ?.closest(".pane[data-id]") as HTMLElement | null;
    return node?.dataset.id ?? null;
  }

  // ---- interactive (xterm) -------------------------------------------------

  private createTerminal(s: SessionInfo): void {
    const root = this.makeRoot(s, s.mode === "shell" ? "shell" : "terminal");
    const host = document.createElement("div");
    host.className = "term-host";
    root.appendChild(host);

    const term = new Terminal({
      theme: THEME,
      fontFamily: "Cascadia Code, Consolas, monospace",
      fontSize: 13,
      cursorBlink: true,
      scrollback: 5000,
      allowProposedApi: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.loadAddon(new WebLinksAddon());
    term.open(host);
    try { fit.fit(); } catch { /* host not laid out yet */ }

    term.onData((d) => void api.sendInput(s.id, d));

    // Clipboard, terminal-style: Ctrl+Shift+C / Ctrl+Shift+V always copy/paste,
    // and a bare Ctrl+C copies when there's a selection (otherwise it falls
    // through as SIGINT, the usual terminal behavior). Native Ctrl+V still works
    // too — xterm handles the textarea paste event itself.
    term.attachCustomKeyEventHandler((e) => {
      if (e.type !== "keydown" || !e.ctrlKey) return true;
      const key = e.key.toLowerCase();
      if (key === "c") {
        const sel = term.getSelection();
        if (e.shiftKey || sel) {
          if (sel) { void copyText(sel); term.clearSelection(); }
          return false; // consume — don't also send ^C
        }
        return true; // no selection: let ^C through as SIGINT
      }
      if (e.shiftKey && key === "v") {
        void pasteInto(s.id);
        return false;
      }
      return true;
    });
    // Right-click pastes (conhost/PuTTY convention).
    host.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      void pasteInto(s.id);
    });

    const doFit = () => {
      try {
        fit.fit();
        api.resizeTerminal(s.id, term.cols, term.rows).catch(() => {});
      } catch { /* ignore */ }
    };
    const resizeObs = new ResizeObserver(() => doFit());
    resizeObs.observe(host);
    queueMicrotask(doFit);

    this.panes.set(s.id, { id: s.id, mode: "interactive", root, term, fit, resizeObs });
    term.focus();
  }

  // ---- headless (structured log stream) ------------------------------------

  private createLogView(s: SessionInfo): void {
    const root = this.makeRoot(s, "agent");

    const body = document.createElement("div");
    body.className = "log-body";
    root.appendChild(body);

    const form = document.createElement("form");
    form.className = "msg-form";
    const input = document.createElement("input");
    input.type = "text";
    input.placeholder = "Send a message to this worker…";
    form.appendChild(input);
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      const v = input.value.trim();
      if (!v) return;
      api.sendMessage(s.id, v).catch((err) => this.appendError(body, String(err)));
      input.value = "";
    });
    root.appendChild(form);

    this.panes.set(s.id, { id: s.id, mode: "headless", root, logBody: body, msgInput: input });

    // Backfill recent history, then live events stream in via onLog().
    api.getLogs(s.id, 300).then((logs) => logs.forEach((l) => this.appendLog(body, l)));
  }

  private appendLog(body: HTMLElement, l: LogEntry): void {
    const atBottom = body.scrollHeight - body.scrollTop - body.clientHeight < 40;
    const line = document.createElement("div");
    line.className = `log log-${l.kind}`;
    const tag = document.createElement("span");
    tag.className = "log-tag";
    tag.textContent = l.kind;
    const text = document.createElement("span");
    text.className = "log-text";
    text.textContent = l.text;
    line.append(tag, text);
    body.appendChild(line);
    // Cap DOM nodes so a chatty worker can't grow the view unbounded.
    while (body.childElementCount > 800) body.firstElementChild?.remove();
    if (atBottom) body.scrollTop = body.scrollHeight;
  }

  private appendError(body: HTMLElement, msg: string): void {
    this.appendLog(body, {
      id: 0,
      session_id: "",
      ts: Date.now(),
      kind: "error",
      text: msg,
      data: null,
    });
  }

  // ---- shared --------------------------------------------------------------

  private makeRoot(s: SessionInfo, kind: string): HTMLElement {
    const root = document.createElement("div");
    root.className = "pane";
    root.dataset.id = s.id;
    const head = document.createElement("div");
    head.className = "pane-head";
    head.innerHTML =
      `<span class="dot dot-${s.status}"></span>` +
      `<span class="pane-title">${escapeHtml(s.name)}</span>` +
      `<span class="pane-badge">${kind}</span>` +
      `<span class="pane-cwd">${escapeHtml(s.cwd)}</span>`;
    const maximize = document.createElement("button");
    maximize.className = "pane-max";
    maximize.textContent = "⤢";
    maximize.title = "Fullscreen this pane (Esc to exit)";
    maximize.onclick = () => this.toggleMaximize(s.id);
    head.appendChild(maximize);

    const detach = document.createElement("button");
    detach.className = "pane-detach";
    detach.textContent = "detach";
    detach.title = "Detach (frees the rendered pane)";
    detach.onclick = () => api.detachSession(s.id);
    head.appendChild(detach);
    // Double-clicking the header also toggles fullscreen.
    head.ondblclick = (e) => {
      if ((e.target as HTMLElement).tagName !== "BUTTON") this.toggleMaximize(s.id);
    };
    root.appendChild(head);
    this.container.appendChild(root);
    return root;
  }

  private relayout(): void {
    const n = this.panes.size;
    const maxActive = !!this.maximizedId && this.panes.has(this.maximizedId);
    // Favor wider panes for small counts (terminals like horizontal space);
    // beyond that, fall back to a roughly-square grid.
    const cols = n <= 1 ? 1 : n <= 3 ? n : Math.ceil(Math.sqrt(n));
    const rows = Math.max(1, Math.ceil(n / cols));
    this.container.style.gridTemplateColumns = `repeat(${cols}, 1fr)`;
    this.container.style.gridTemplateRows = `repeat(${rows}, 1fr)`;
    this.container.classList.toggle("empty", n === 0);
    this.container.classList.toggle("has-max", maxActive);
    for (const [id, p] of this.panes) {
      p.root.classList.toggle("maximized", maxActive && id === this.maximizedId);
      const btn = p.root.querySelector(".pane-max") as HTMLButtonElement | null;
      if (btn) btn.textContent = maxActive && id === this.maximizedId ? "⤡" : "⤢";
    }
    // Terminals need a refit after the box changes.
    for (const p of this.panes.values()) {
      if (p.fit) queueMicrotask(() => { try { p.fit!.fit(); } catch { /* */ } });
    }
  }
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]!));
}

/** Quote a path for a shell/Claude prompt only when it contains whitespace. */
function quotePath(p: string): string {
  return /\s/.test(p) ? `"${p}"` : p;
}

/** Copy text to the system clipboard (best-effort; ignores denial). */
async function copyText(text: string): Promise<void> {
  try { await navigator.clipboard.writeText(text); } catch { /* clipboard blocked */ }
}

/** Read clipboard text and feed it to a live PTY as input. */
async function pasteInto(sessionId: string): Promise<void> {
  let text = "";
  try { text = await navigator.clipboard.readText(); } catch { /* clipboard blocked */ }
  if (text) await api.sendInput(sessionId, text);
}
