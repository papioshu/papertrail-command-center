import { useEffect, useRef, useState } from "react";
import { api, type Row } from "./api";

const COLORS = ["#a78bfa", "#5b8def", "#aef044", "#c9a227", "#ff4d6d", "#d9a46c", "#7fb7ff", "#ece8fb"];
// Long enough that someone still typing a longer PIN isn't interrupted.
const WRONG_PIN_PAUSE_MS = 1500;
const MIN_PIN = 4;
const MAX_PIN = 8;

/** Sign-in gate: a sheet of liquid glass. Type the PIN and a correct one passes at once; Esc steps back. */
export default function Login({ onDone }: { onDone: (me: Row | null) => void }) {
  const [team, setTeam] = useState<{ profiles: Row[] } | null>(null);
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [who, setWho] = useState<Row | null>(null);
  const [creating, setCreating] = useState(false);
  const [pin, setPin] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [shake, setShake] = useState(0);
  const [busy, setBusy] = useState(false);
  const [form, setForm] = useState({ name: "", pin: "", pin2: "", color: COLORS[0] });
  const pinRef = useRef<HTMLInputElement>(null);
  const latestPin = useRef("");
  const trying = useRef(false);
  const wrongTimer = useRef<ReturnType<typeof setTimeout>>(undefined);

  useEffect(() => {
    api.team().then((t) => {
      setTeam(t);
      if (!t.profiles.length) setCreating(true);
      else if (t.profiles.length === 1) setWho(t.profiles[0]);
    }, (e) => setLoadErr(String(e)));
    return () => clearTimeout(wrongTimer.current);
  }, []);
  // The shake remounts the PIN block, so focus has to come back after it too.
  useEffect(() => { if (who) pinRef.current?.focus(); }, [who, shake]);

  useEffect(() => {
    if (!team || who || creating) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onDone(null); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [team, who, creating, onDone]);

  const fail = (e: unknown) => { setErr(String(e)); setShake((n) => n + 1); setPin(""); latestPin.current = ""; setBusy(false); };

  /**
   * Tries the PIN as it's typed. The backend waits 700 ms on a wrong one to slow guessing, so one attempt runs at a time,
   * and when it fails the newest digits go next. A wrong PIN only complains at full length or after a pause in typing.
   */
  async function tryPin(id: string) {
    if (trying.current) return;
    const attempt = latestPin.current;
    if (attempt.length < MIN_PIN) return;
    trying.current = true;
    try {
      const me = await api.login(id, attempt);
      clearTimeout(wrongTimer.current);
      onDone(me);
    } catch (e) {
      trying.current = false;
      if (latestPin.current !== attempt) return void tryPin(id);
      if (attempt.length >= MAX_PIN) return fail(e);
      clearTimeout(wrongTimer.current);
      wrongTimer.current = setTimeout(() => fail(e), WRONG_PIN_PAUSE_MS);
    }
  }

  function typePin(raw: string) {
    if (!who) return;
    const next = raw.replace(/\D/g, "").slice(0, MAX_PIN);
    clearTimeout(wrongTimer.current);
    setErr(null);
    setPin(next);
    latestPin.current = next;
    void tryPin(who.id);
  }

  function leavePin() {
    clearTimeout(wrongTimer.current);
    setWho(null); setPin(""); setErr(null);
    latestPin.current = "";
  }

  async function create() {
    if (form.pin !== form.pin2) return fail("PINs don't match.");
    setBusy(true); setErr(null);
    try { onDone(await api.teamCreate(form.name, form.pin, form.color)); } catch (e) { fail(e); }
  }

  return (
    <div className="login">
      <svg className="sr-only" aria-hidden>
        <filter id="liquid-glass" x="0" y="0" width="100%" height="100%">
          <feTurbulence type="fractalNoise" baseFrequency="0.008 0.012" numOctaves="2" seed="7" />
          <feDisplacementMap in="SourceGraphic" scale="46" xChannelSelector="R" yChannelSelector="G" />
        </filter>
      </svg>
      <div className="login-panel">
        {loadErr ? (
          <div className="col" style={{ gap: 10 }}>
            <div className="errbox">{loadErr}</div>
            <button className="btn" onClick={() => onDone(null)}>Continue without signing in</button>
          </div>
        ) : !team ? null : creating ? (
          <div className="col login-in" style={{ gap: 10 }}>
            <div className="label">Create your profile</div>
            <input className="input" placeholder="Your name" value={form.name} autoFocus onChange={(e) => setForm({ ...form, name: e.target.value })} aria-label="Name" />
            <div className="row" style={{ gap: 8 }}>
              <input className="input mono" type="password" inputMode="numeric" placeholder="PIN (4–8 digits)" value={form.pin} onChange={(e) => setForm({ ...form, pin: e.target.value.replace(/\D/g, "").slice(0, MAX_PIN) })} aria-label="PIN" />
              <input className="input mono" type="password" inputMode="numeric" placeholder="Repeat PIN" value={form.pin2} onChange={(e) => setForm({ ...form, pin2: e.target.value.replace(/\D/g, "").slice(0, MAX_PIN) })} onKeyDown={(e) => e.key === "Enter" && create()} aria-label="Repeat PIN" />
            </div>
            <div className="row" style={{ gap: 6 }} role="radiogroup" aria-label="Colour">
              {COLORS.map((c) => <button key={c} role="radio" aria-checked={form.color === c} aria-label={c} className={`swatch ${form.color === c ? "on" : ""}`} style={{ background: c }} onClick={() => setForm({ ...form, color: c })} />)}
            </div>
            {err && <div className="errbox" key={shake}>{err}</div>}
            <div className="row" style={{ gap: 8 }}>
              <button className="btn primary" disabled={busy || !form.name.trim() || form.pin.length < MIN_PIN} onClick={create}>Create &amp; sign in</button>
              {team.profiles.length
                ? <button className="btn ghost" onClick={() => { setCreating(false); setErr(null); }}>Back</button>
                : <button className="btn ghost" onClick={() => onDone(null)}>Skip for now</button>}
            </div>
            <div className="dim small">Saved to the shared vault so teammates see your name on action items. The PIN only marks who did what. Anyone with the vault can work it out, so don't reuse your phone or bank PIN.</div>
          </div>
        ) : who ? (
          <div className={`col login-in ${shake ? "shake" : ""}`} key={shake} style={{ gap: 16, alignItems: "center" }} onClick={() => pinRef.current?.focus()}>
            <div className="avatar big" style={{ background: who.color }}>{who.name[0]}</div>
            <div className="display">{who.name}</div>
            <input ref={pinRef} className="sr-only" type="password" inputMode="numeric" autoComplete="off" value={pin} aria-label={`PIN for ${who.name}`}
              onChange={(e) => typePin(e.target.value)} onBlur={(e) => e.currentTarget.focus()} onKeyDown={(e) => { if (e.key === "Escape") { e.stopPropagation(); leavePin(); } }} />
            <div className="pin-dots" aria-hidden>
              {Array.from({ length: Math.max(MIN_PIN, pin.length) }, (_, i) => <span key={i} className={i < pin.length ? "on" : ""} />)}
            </div>
            <div className="login-hint" role={err ? "alert" : undefined}>{err ?? "Type your PIN"}</div>
          </div>
        ) : (
          <div className="col login-in" style={{ gap: 12 }}>
            <div className="login-tiles">
              {team.profiles.map((p, i) => (
                <button key={p.id} className="login-tile" style={{ animationDelay: `${i * 70}ms` }} onClick={() => setWho(p)}>
                  <div className="avatar" style={{ background: p.color }}>{p.name[0]}</div>
                  <span>{p.name}</span>
                </button>
              ))}
              <button className="login-tile" onClick={() => setCreating(true)}><div className="avatar ghost">+</div><span>New profile</span></button>
            </div>
            <div className="login-hint">Esc to continue without signing in</div>
          </div>
        )}
      </div>
    </div>
  );
}
