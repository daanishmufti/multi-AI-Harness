// Voice mode using the Web Speech API.
// On: continuously listens; final transcripts go to the currently-selected
// session — as a message to a headless worker, or as typed input + Enter to
// an interactive agent or shell. Assistant log lines for the active session
// are spoken aloud via TTS.

import { api } from "./api";
import type { LogEntry, SessionInfo, SessionMode } from "./types";

type SR = any;
const SRCtor: { new (): SR } | undefined =
  (window as any).SpeechRecognition || (window as any).webkitSpeechRecognition;

export interface VoiceDeps {
  getActiveSession: () => SessionInfo | null;
  notify: (msg: string) => void;
  button: HTMLButtonElement;
  /** Notified whenever voice mode turns on or off. */
  onChange?: (on: boolean) => void;
  /** Pre-process each final transcript. Return true to swallow it (the caller
   *  handled it as a command), false to send it to the active session. */
  interceptTranscript?: (text: string) => boolean;
}

export class Voice {
  private rec: SR | null = null;
  private on = false;
  private lastUtteredId = -1;

  constructor(private deps: VoiceDeps) {
    deps.button.onclick = () => this.toggle();
    this.applyButton();
  }

  onLog(l: LogEntry): void {
    if (!this.on || l.kind !== "assistant" || l.id <= this.lastUtteredId) return;
    const active = this.deps.getActiveSession();
    if (active && active.id !== l.session_id) return;
    this.lastUtteredId = l.id;
    this.speak(l.text);
  }

  toggle(): void {
    if (this.on) this.stop();
    else this.start();
  }

  private start(): void {
    if (!SRCtor) {
      this.deps.notify("Voice not supported (no SpeechRecognition in this WebView)");
      return;
    }
    try {
      this.rec = new SRCtor();
      this.rec.continuous = true;
      this.rec.interimResults = false;
      this.rec.lang = navigator.language || "en-US";
      this.rec.onresult = (e: any) => this.onResult(e);
      this.rec.onerror = (e: any) => this.deps.notify(`Voice error: ${e.error || "unknown"}`);
      this.rec.onend = () => { if (this.on) try { this.rec?.start(); } catch { /* already running */ } };
      this.rec.start();
      this.on = true;
      this.applyButton();
      this.deps.onChange?.(true);
      this.deps.notify("Voice on — speak to the selected session");
    } catch (e) {
      this.deps.notify(`Voice failed to start: ${e}`);
    }
  }

  private stop(): void {
    this.on = false;
    try { this.rec?.stop(); } catch { /* */ }
    this.rec = null;
    speechSynthesis.cancel();
    this.applyButton();
    this.deps.onChange?.(false);
    this.deps.notify("Voice off");
  }

  private onResult(event: any): void {
    let transcript = "";
    for (let i = event.resultIndex; i < event.results.length; i++) {
      const r = event.results[i];
      if (r.isFinal) transcript += r[0].transcript;
    }
    transcript = transcript.trim();
    if (!transcript) return;
    // Let the caller handle wake-word commands first.
    if (this.deps.interceptTranscript?.(transcript)) return;
    const active = this.deps.getActiveSession();
    if (!active) {
      this.deps.notify("Voice: no session targeted");
      return;
    }
    this.deliver(transcript, active.id, active.mode);
  }

  private deliver(text: string, id: string, mode: SessionMode): void {
    if (mode === "headless") {
      api.sendMessage(id, text).catch((e) => this.deps.notify(String(e)));
    } else {
      api.sendInput(id, text + "\r").catch((e) => this.deps.notify(String(e)));
    }
  }

  private speak(text: string): void {
    if (!("speechSynthesis" in window)) return;
    const clipped = text.length > 600 ? text.slice(0, 600) + " …" : text;
    const u = new SpeechSynthesisUtterance(clipped);
    u.rate = 1.05;
    speechSynthesis.speak(u);
  }

  private applyButton(): void {
    this.deps.button.classList.toggle("active", this.on);
    this.deps.button.textContent = this.on ? "🎙 On" : "🎙 Voice";
  }
}
