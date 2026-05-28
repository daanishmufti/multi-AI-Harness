// Draggable / collapsible task bar — same pattern as the sidebar resizer,
// just on the horizontal axis (drag the divider up/down). Drag below the
// collapse threshold (or double-click the divider) to fully hide it.

const MIN = 80;
const MAX = 600;
const COLLAPSE_AT = 60;
const DEFAULT = 150;
const KEY = "agentlane.taskbarHeight";

export function installTaskbarResizer(): void {
  const app = document.getElementById("app")!;
  const resizer = document.getElementById("taskbar-resizer")!;

  const setH = (px: number) => app.style.setProperty("--taskbar-h", `${px}px`);
  const collapse = () => { app.classList.add("taskbar-collapsed"); localStorage.setItem(KEY, "0"); };
  const expand = (px = readSaved() || DEFAULT) => {
    app.classList.remove("taskbar-collapsed");
    setH(px);
    localStorage.setItem(KEY, String(px));
  };

  const saved = localStorage.getItem(KEY);
  if (saved === "0") app.classList.add("taskbar-collapsed");
  else if (saved) setH(parseInt(saved, 10) || DEFAULT);

  let dragging = false;
  const onMove = (e: MouseEvent) => {
    if (!dragging) return;
    // Height = distance from cursor to bottom of the viewport.
    const h = window.innerHeight - e.clientY;
    if (h < COLLAPSE_AT) {
      app.classList.add("taskbar-collapsed");
      return;
    }
    app.classList.remove("taskbar-collapsed");
    setH(Math.max(MIN, Math.min(MAX, h)));
  };
  const onUp = () => {
    if (!dragging) return;
    dragging = false;
    resizer.classList.remove("dragging");
    document.body.style.userSelect = "";
    window.removeEventListener("mousemove", onMove);
    window.removeEventListener("mouseup", onUp);
    if (app.classList.contains("taskbar-collapsed")) localStorage.setItem(KEY, "0");
    else localStorage.setItem(KEY, String(currentH()));
  };

  resizer.addEventListener("mousedown", (e) => {
    e.preventDefault();
    if (app.classList.contains("taskbar-collapsed")) expand();
    dragging = true;
    resizer.classList.add("dragging");
    document.body.style.userSelect = "none";
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  });

  resizer.addEventListener("dblclick", () => {
    if (app.classList.contains("taskbar-collapsed")) expand();
    else collapse();
  });

  function currentH(): number {
    const v = parseInt(getComputedStyle(app).getPropertyValue("--taskbar-h"), 10);
    return Number.isFinite(v) ? v : DEFAULT;
  }
  function readSaved(): number {
    const v = parseInt(localStorage.getItem(KEY) || "", 10);
    return Number.isFinite(v) && v > 0 ? v : 0;
  }
}
