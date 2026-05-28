// The "New project" builder: author a set of sessions (with shared defaults)
// in one dialog, then either launch them immediately or save them to a
// reusable project file. No per-session naming required.

import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import type { ProjectContext, ProjectDef, SessionMode } from "./types";

const PERMS = ["acceptEdits", "default", "dontAsk", "bypassPermissions"];
const MODES: [SessionMode, string][] = [
  ["headless", "Worker (headless Claude)"],
  ["interactive", "Claude (interactive)"],
  ["shell", "Terminal (shell)"],
];

export function openNewProjectModal(
  notify: (m: string) => void,
  onActivate?: (ctx: ProjectContext) => void,
): void {
  const root = document.getElementById("modal-root")!;
  const backdrop = document.createElement("div");
  backdrop.className = "modal-backdrop";
  const modal = document.createElement("div");
  modal.className = "modal modal-wide";

  const h = document.createElement("h2");
  h.textContent = "New project";
  modal.appendChild(h);

  // shared working directory (with folder picker)
  const cwdLabel = label("Shared working directory");
  const cwdRow = document.createElement("div");
  cwdRow.className = "dir-row";
  const cwdInput = document.createElement("input");
  cwdInput.placeholder = "Browse or type a path…";
  const cwdBrowse = button("Browse…", async () => {
    const p = await openDialog({ directory: true, multiple: false, title: "Working directory" });
    if (typeof p === "string") cwdInput.value = p;
  });
  cwdRow.append(cwdInput, cwdBrowse);

  // shared model + permission
  const row2 = document.createElement("div");
  row2.className = "row2";
  const modelInput = document.createElement("input");
  modelInput.placeholder = "claude-opus-4-7";
  const modelWrap = field("Model (blank = default)", modelInput);
  const permSel = document.createElement("select");
  for (const p of PERMS) permSel.appendChild(option(p, p));
  permSel.value = "acceptEdits";
  const permWrap = field("Permission mode", permSel);
  row2.append(modelWrap, permWrap);

  // session rows
  const sessLabel = label("Sessions");
  const sessBox = document.createElement("div");
  sessBox.className = "np-sessions";

  const addRow = (mode: SessionMode = "headless") => {
    const r = document.createElement("div");
    r.className = "np-row";
    const sel = document.createElement("select");
    for (const [v, l] of MODES) sel.appendChild(option(v, l));
    sel.value = mode;
    const prompt = document.createElement("input");
    prompt.className = "np-prompt";
    const cwd = document.createElement("input");
    cwd.className = "np-cwd";
    cwd.placeholder = "cwd override";
    const rm = document.createElement("button");
    rm.type = "button";
    rm.className = "np-rm";
    rm.textContent = "✕";
    rm.onclick = () => r.remove();
    const sync = () => {
      const m = sel.value;
      prompt.disabled = false;
      prompt.placeholder =
        m === "headless" ? "initial prompt" :
        m === "shell"    ? "initial command (e.g. cd src; ls)" :
                           "initial input";
    };
    sel.onchange = sync;
    r.append(sel, prompt, cwd, rm);
    sessBox.appendChild(r);
    sync();
  };
  addRow("headless");

  // Quick add — one click stacks another session of that type (shared defaults
  // apply; no details needed).
  const adders = document.createElement("div");
  adders.className = "np-adders";
  const aw = button("+ Worker", () => addRow("headless"));
  const ac = button("+ Claude", () => addRow("interactive"));
  const at = button("+ Terminal", () => addRow("shell"));
  adders.append(aw, ac, at);

  // actions
  const actions = document.createElement("div");
  actions.className = "modal-actions";
  const cancel = button("Cancel", () => backdrop.remove());
  const saveBtn = button("Save to File…", () => void onSave());
  const createBtn = button("Create", () => void onCreate());
  actions.append(cancel, saveBtn, createBtn);

  modal.append(cwdLabel, cwdRow, row2, sessLabel, sessBox, adders, actions);
  backdrop.appendChild(modal);
  root.appendChild(backdrop);
  backdrop.onclick = (e) => { if (e.target === backdrop) backdrop.remove(); };
  queueMicrotask(() => cwdInput.focus());

  function gather() {
    const sharedCwd = cwdInput.value.trim();
    const model = modelInput.value.trim() || null;
    const perm = permSel.value;
    const rows = [...sessBox.querySelectorAll<HTMLElement>(".np-row")].map((r) => ({
      mode: (r.querySelector("select") as HTMLSelectElement).value as SessionMode,
      prompt: (r.querySelector(".np-prompt") as HTMLInputElement).value.trim(),
      cwd: (r.querySelector(".np-cwd") as HTMLInputElement).value.trim(),
    }));
    return { sharedCwd, model, perm, rows };
  }

  async function onCreate() {
    const { sharedCwd, model, perm, rows } = gather();
    let n = 0;
    for (const row of rows) {
      const cwd = row.cwd || sharedCwd;
      if (!cwd) continue;
      try {
        await api.createSession({
          mode: row.mode,
          cwd,
          model,
          name: null,
          permission_mode: perm,
          initial_prompt: row.prompt || null,
        });
        n++;
      } catch (e) {
        notify(String(e));
      }
    }
    notify(`Created ${n} session(s)`);
    if (sharedCwd && onActivate) onActivate({ cwd: sharedCwd, model, permission_mode: perm });
    backdrop.remove();
  }

  async function onSave() {
    const { sharedCwd, model, perm, rows } = gather();
    const project: ProjectDef = {
      cwd: sharedCwd || null,
      model,
      permission_mode: perm,
      sessions: rows.map((r) => ({
        mode: r.mode,
        cwd: r.cwd || null,
        prompt: r.prompt || null,
      })),
    };
    const path = await saveDialog({
      title: "Save project file",
      defaultPath: "agentlane.project.json",
      filters: [{ name: "Project (JSON)", extensions: ["json"] }],
    });
    if (typeof path !== "string") return;
    try {
      await api.writeProject(path, project);
      notify("Project saved");
    } catch (e) {
      notify(String(e));
    }
  }
}

// ---- small DOM helpers -----------------------------------------------------

function label(text: string): HTMLLabelElement {
  const l = document.createElement("label");
  l.textContent = text;
  return l;
}
function field(text: string, input: HTMLElement): HTMLDivElement {
  const wrap = document.createElement("div");
  wrap.append(label(text), input);
  return wrap;
}
function option(value: string, text: string): HTMLOptionElement {
  const o = document.createElement("option");
  o.value = value;
  o.textContent = text;
  return o;
}
function button(text: string, onclick: () => void): HTMLButtonElement {
  const b = document.createElement("button");
  b.type = "button";
  b.textContent = text;
  b.onclick = onclick;
  return b;
}
