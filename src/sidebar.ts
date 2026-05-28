// Draggable sidebar: drag the divider to resize, drag narrow (or double-click)
// to collapse, drag/click the collapsed handle to expand. Width persists.

const MIN = 220; // never render the sidebar narrower than this (keeps it usable)
const MAX = 560;
const COLLAPSE_AT = 150; // dragging narrower than this hides it entirely
const DEFAULT = 290;
const KEY = "agentlane.sidebarWidth";

export function installSidebarResizer(): { toggle: () => void } {
  const body = document.getElementById("body")!;
  const resizer = document.getElementById("sidebar-resizer")!;

  const setWidth = (px: number) => body.style.setProperty("--sidebar-w", `${px}px`);
  const collapse = () => { body.classList.add("sidebar-collapsed"); localStorage.setItem(KEY, "0"); };
  const expand = (px = readSaved() || DEFAULT) => {
    body.classList.remove("sidebar-collapsed");
    setWidth(px);
    localStorage.setItem(KEY, String(px));
  };

  // Restore last state.
  const saved = localStorage.getItem(KEY);
  if (saved === "0") body.classList.add("sidebar-collapsed");
  else if (saved) setWidth(parseInt(saved, 10) || DEFAULT);

  let dragging = false;
  const onMove = (e: MouseEvent) => {
    if (!dragging) return;
    // The sidebar's left edge is the window's left edge, so clientX ≈ width.
    const x = e.clientX;
    if (x < COLLAPSE_AT) {
      body.classList.add("sidebar-collapsed");
      return;
    }
    body.classList.remove("sidebar-collapsed");
    setWidth(Math.max(MIN, Math.min(MAX, x)));
  };
  const onUp = () => {
    if (!dragging) return;
    dragging = false;
    resizer.classList.remove("dragging");
    document.body.style.userSelect = "";
    window.removeEventListener("mousemove", onMove);
    window.removeEventListener("mouseup", onUp);
    // Persist final state.
    if (body.classList.contains("sidebar-collapsed")) localStorage.setItem(KEY, "0");
    else localStorage.setItem(KEY, String(currentWidth()));
  };

  resizer.addEventListener("mousedown", (e) => {
    e.preventDefault();
    if (body.classList.contains("sidebar-collapsed")) expand();
    dragging = true;
    resizer.classList.add("dragging");
    document.body.style.userSelect = "none";
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  });

  const toggle = () => {
    if (body.classList.contains("sidebar-collapsed")) expand();
    else collapse();
  };
  resizer.addEventListener("dblclick", toggle);

  function currentWidth(): number {
    const v = parseInt(getComputedStyle(body).getPropertyValue("--sidebar-w"), 10);
    return Number.isFinite(v) ? v : DEFAULT;
  }
  function readSaved(): number {
    const v = parseInt(localStorage.getItem(KEY) || "", 10);
    return Number.isFinite(v) && v > 0 ? v : 0;
  }

  return { toggle };
}
