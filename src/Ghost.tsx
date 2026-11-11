import { useCallback, useEffect, useRef, useState } from "react";
import { api, type Row } from "./api";
import { useNav, type View } from "./ui";

/** Ghost on the PC (D052): hold the mic button or Ctrl+Space, speak, let go. Whisper (local) turns it into text; things
 *  about this PC (open a project or a page, your calendar) are handled here, team things by the hub's Ghost. Answers
 *  are spoken with the Windows voice and shown under the top bar. */

const squash = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, "");
const PAGES: [RegExp, View][] = [[/calendar/, "calendar"], [/settings/, "settings"], [/team|workspace/, "team"], [/overview|home/, "overview"], [/agents?|sessions?/, "agents"]];

function speak(t: string) {
  try { speechSynthesis.cancel(); speechSynthesis.speak(new SpeechSynthesisUtterance(t)); } catch { /* the text still shows */ }
}

// Microphone → 16 kHz mono samples (the browser resamples) → WAV, base64 for the backend.
type Rec = { ctx: AudioContext; stream: MediaStream; node: ScriptProcessorNode; chunks: Float32Array[] };
async function startRec(): Promise<Rec> {
  const stream = await navigator.mediaDevices.getUserMedia({ audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true } });
  const ctx = new AudioContext({ sampleRate: 16000 });
  const node = ctx.createScriptProcessor(4096, 1, 1);
  const rec: Rec = { ctx, stream, node, chunks: [] };
  node.onaudioprocess = (e) => { rec.chunks.push(new Float32Array(e.inputBuffer.getChannelData(0))); };
  ctx.createMediaStreamSource(stream).connect(node);
  node.connect(ctx.destination); // a ScriptProcessor only runs while connected; it outputs silence
  return rec;
}
export function wav(chunks: Float32Array[], rate = 16000): string {
  const n = Math.min(chunks.reduce((a, c) => a + c.length, 0), rate * 60);
  const buf = new DataView(new ArrayBuffer(44 + n * 2));
  const str = (o: number, s: string) => [...s].forEach((ch, i) => buf.setUint8(o + i, ch.charCodeAt(0)));
  str(0, "RIFF"); buf.setUint32(4, 36 + n * 2, true); str(8, "WAVE"); str(12, "fmt ");
  buf.setUint32(16, 16, true); buf.setUint16(20, 1, true); buf.setUint16(22, 1, true); buf.setUint32(24, rate, true);
  buf.setUint32(28, rate * 2, true); buf.setUint16(32, 2, true); buf.setUint16(34, 16, true); str(36, "data"); buf.setUint32(40, n * 2, true);
  let i = 0;
  for (const c of chunks) for (const v of c) { if (i >= n) break; buf.setInt16(44 + i * 2, Math.max(-1, Math.min(1, v)) * 0x7fff, true); i++; }
  const bytes = new Uint8Array(buf.buffer);
  let bin = "";
  for (let k = 0; k < bytes.length; k += 0x8000) bin += String.fromCharCode(...bytes.subarray(k, k + 0x8000));
  return btoa(bin);
}

export default function GhostButton({ big = false }: { big?: boolean }) {
  const { nav } = useNav();
  const [state, setState] = useState<"idle" | "rec" | "think">("idle");
  const [card, setCard] = useState<{ heard: string; answer: string } | null>(null);
  const rec = useRef<Rec | null>(null);
  const hide = useRef<number | undefined>(undefined);
  const show = (heard: string, answer: string) => { setCard({ heard, answer }); window.clearTimeout(hide.current); hide.current = window.setTimeout(() => setCard(null), 12000); };

  const answer = useCallback(async (text: string) => {
    const t = text.toLowerCase().replace(/^(hey |ok |okay )?ghost[,.!]?\s*/, "");
    const open = t.match(/^(?:open|show|go to)\s+(?:the\s+)?(.+?)[.!?]*$/);
    if (open) {
      const page = PAGES.find(([re]) => re.test(open[1]));
      const ps = (await api.projects().catch(() => [])) as Row[];
      const p = ps.find((x) => squash(x.name).includes(squash(open[1]))) ?? ps.find((x) => squash(open[1]).includes(squash(String(x.name).match(/\((.+)\)/)?.[1] ?? x.name)));
      if (p) { nav("projects", { id: p.id }); return `Opening ${p.name}.`; }
      if (page) { nav(page[1]); return `Opening ${page[1]}.`; }
    }
    if (/calendar|schedule|meetings?|appointments?|events?/.test(t)) {
      const st = await api.gcalStatus().catch(() => null);
      if (st?.connected) {
        const from = new Date(); from.setHours(0, 0, 0, 0);
        const to = new Date(from); to.setDate(to.getDate() + 7);
        const ids = ["", ...new Set([...((st.accounts ?? []) as string[]), ...Object.values((st.team ?? {}) as Record<string, string>)])];
        const evs = (await Promise.all(ids.map((c) => api.gcalEvents(from.toISOString(), to.toISOString(), c).catch(() => [] as Row[])))).flat();
        const when = (e: Row) => new Date(e.start?.dateTime ?? `${e.start?.date}T12:00`);
        evs.sort((a, b) => when(a).getTime() - when(b).getTime());
        if (!evs.length) return "Your calendar is clear for the next 7 days.";
        return `${evs.length} on your calendar this week: ${evs.slice(0, 5).map((e) => `${e.title || "an event"} ${when(e).toLocaleDateString(undefined, { weekday: "long" })}`).join(", ")}.`;
      }
    }
    const hub = await api.hubStatus().catch(() => null);
    if (!hub?.joined) return "Join a team hub to ask Ghost about tasks.";
    return String((await api.hubVoice(text, hub.workspaces?.[0]?.id)).say);
  }, [nav]);

  const begin = useCallback(async () => {
    if (rec.current || state !== "idle") return;
    const st = await api.voiceStatus().catch(() => null);
    if (!st?.installed) { show("Voice isn't set up yet.", "Settings › Ghost voice › Download (one time, about 160 MB)."); nav("settings"); return; }
    try { rec.current = await startRec(); setState("rec"); } catch (e) { show("Microphone unavailable.", String(e)); }
  }, [state, nav]);

  const end = useCallback(async () => {
    const r = rec.current;
    if (!r) return;
    rec.current = null;
    r.node.disconnect(); r.stream.getTracks().forEach((t) => t.stop()); r.ctx.close().catch(() => {});
    if (r.chunks.reduce((a, c) => a + c.length, 0) < 16000 * 0.4) { setState("idle"); show("Too short.", "Hold the button (or Ctrl+Space) while you speak."); return; }
    setState("think");
    try {
      const hub = await api.hubStatus().catch(() => null);
      const ps = ((await api.projects().catch(() => [])) as Row[]).slice(0, 30).map((p) => String(p.name).match(/\((.+)\)/)?.[1] ?? p.name);
      const hint = [...((hub?.workspaces ?? []) as Row[]).map((w) => w.name), ...ps].join(", ");
      const text = (await api.voiceTranscribe(wav(r.chunks), hint)).trim();
      if (!text) { show("I didn't catch that.", ""); return; }
      show(`"${text}"`, "…");
      const a = await answer(text);
      show(`"${text}"`, a); speak(a);
    } catch (e) { show("Ghost", String(e)); } finally { setState("idle"); }
  }, [answer]);

  // Hold Ctrl+Space anywhere in the app (only the top-bar button listens, so one press records once).
  useEffect(() => {
    if (big) return;
    const down = (e: KeyboardEvent) => { if (e.ctrlKey && e.code === "Space" && !e.repeat) { e.preventDefault(); begin(); } };
    const up = (e: KeyboardEvent) => { if (e.code === "Space" || e.key === "Control") end(); };
    window.addEventListener("keydown", down); window.addEventListener("keyup", up);
    return () => { window.removeEventListener("keydown", down); window.removeEventListener("keyup", up); };
  }, [begin, end, big]);

  return (
    <>
      <button className={`btn ${big ? "" : "sm"} ghost-btn ${state}`} onPointerDown={(e) => { e.currentTarget.setPointerCapture(e.pointerId); begin(); }} onPointerUp={end} onPointerCancel={end}
        title="Hold to talk to Ghost (or hold Ctrl+Space)" aria-label="Hold to talk to Ghost">
        {state === "rec" ? "● Listening" : state === "think" ? "Ghost…" : "🎙 Ghost"}
      </button>
      {card && <div className="ghost-card" onClick={() => setCard(null)}><div className="dim small">{card.heard}</div>{card.answer && <div>{card.answer}</div>}</div>}
    </>
  );
}
