// Keyboard-first navigation. Shortcuts are inert while typing in a field
// (except Escape, which blurs it), so the dashboard is fully drivable by hand.

export interface KeyHandlers {
  up: () => void;
  down: () => void;
  attach: () => void;
  detach: () => void;
  kill: () => void;
  remove: () => void;
  newHeadless: () => void;
  newInteractive: () => void;
  newShell: () => void;
  newTask: () => void;
  focusBroadcast: () => void;
  focusFilter: () => void;
  focusPane: (n: number) => void;
  toggleZen: () => void;
  toggleFullscreen: () => void;
  escape: () => void;
}

function isTyping(t: EventTarget | null): boolean {
  const el = t as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT");
}

export function installKeyboard(h: KeyHandlers): void {
  window.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      if (isTyping(e.target)) (document.activeElement as HTMLElement | null)?.blur();
      else h.escape();
      return;
    }
    // F11 toggles OS fullscreen even while a terminal is focused.
    if (e.key === "F11") {
      e.preventDefault();
      h.toggleFullscreen();
      return;
    }
    if (isTyping(e.target)) return;
    if (e.ctrlKey || e.metaKey || e.altKey) return;

    switch (e.key) {
      case "j":
      case "ArrowDown": e.preventDefault(); h.down(); break;
      case "k":
      case "ArrowUp": e.preventDefault(); h.up(); break;
      case "Enter": e.preventDefault(); h.attach(); break;
      case "d": h.detach(); break;
      case "x": h.kill(); break;
      case "Delete": e.preventDefault(); h.remove(); break;
      case "n": h.newHeadless(); break;
      case "N": h.newInteractive(); break;
      case "s": h.newShell(); break;
      case "t": h.newTask(); break;
      case "b": e.preventDefault(); h.focusBroadcast(); break;
      case "z": h.toggleZen(); break;
      case "/": e.preventDefault(); h.focusFilter(); break;
      case "1": case "2": case "3": case "4":
        e.preventDefault(); h.focusPane(parseInt(e.key, 10) - 1); break;
    }
  });
}
