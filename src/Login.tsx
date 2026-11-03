import { useEffect, useRef, useState } from "react";
import { api, type Row } from "./api";

const BOOT = ["PAPERTRAIL // COMMAND CENTER", "MOUNTING BOOK OF SECRETS…", "LOADING TEAM PROFILES…", "IDENTIFY YOURSELF"];
const COLORS = ["#a78bfa", "#5b8def", "#aef044", "#c9a227", "#ff4d6d", "#d9a46c", "#7fb7ff", "#ece8fb"];

/** Sign-in gate: boot lines, profile tiles, PIN pad, then an access-granted sweep. Names who does what on the board. */
export default function Login({ onDone }: { onDone: (me: Row | null) => void }) {
  const [boot, setBoot] = useState(0);
  const [team, setTeam] = useState<{ profiles: Row[] } | null>(null);
  const [loadErr, setLoadErr] = useState<string | null>(null);
  const [who, setWho] = useState<Row | null>(null);
  const [creating, setCreating] = useState(false);
  const [pin, setPin] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [shake, setShake] = useState(0);
  const [granted, setGranted] = useState<Row | null>(null);
  const [busy, setBusy] = useState(false);
  const [form, setForm] = useState({ name: "", pin: "", pin2: "", color: COLORS[0] });
  const pinRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    api.team().then((t) => { setTeam(t); if (!t.profiles.length) setCreating(true); }, (e) => setLoadErr(String(e)));
    const id = setInterval(() => setBoot((b) => (b >= BOOT.length ? b : b + 1)), 380);
    return () => clearInterval(id);
  }, []);
  useEffect(() => { if (who) pinRef.current?.focus(); }, [who]);

  const ready = boot >= BOOT.length && (team || loadErr);
  const fail = (e: unknown) => { setErr(String(e)); setShake((n) => n + 1); setPin(""); setBusy(false); };
  const succeed = (me: Row) => { setGranted(me); setTimeout(() => onDone(me), 1100); };

  async function submitPin(p = pin) {
    if (!who || p.length < 4 || busy) return;
    setBusy(true); setErr(null);
    try { succeed(await api.login(who.id, p)); } catch (e) { fail(e); }
  }
  async function create() {
    if (form.pin !== form.pin2) return fail("PINs don't match.");
    setBusy(true); setErr(null);
    try { succeed(await api.teamCreate(form.name, form.pin, form.color)); } catch (e) { fail(e); }
  }
  const press = (d: string) => {
    if (d === "⌫") return setPin((p) => p.slice(0, -1));
    const next = (pin + d).slice(0, 8);
    setPin(next);
  };

  return (
    <div className={`login ${granted ? "granted" : ""}`}>
      <div className="login-grid" aria-hidden />
      <div className="login-scan" aria-hidden />
      <div className="login-panel">
        <div className="login-boot mono">
          {BOOT.slice(0, boot).map((l, i) => <div key={i} className="login-line">{i === BOOT.length - 1 ? <b>{l}</b> : l}<span className="ok">{i < BOOT.length - 1 ? " OK" : ""}</span></div>)}
          {boot < BOOT.length && <span className="login-caret">▌</span>}
        </div>

        {granted ? (
          <div className="login-granted">
            <div className="avatar big" style={{ background: granted.color }}>{granted.name[0]}</div>
            <div className="display">ACCESS GRANTED</div>
            <div className="muted">Welcome back, {granted.name}</div>
          </div>
        ) : ready && loadErr ? (
          <div className="col" style={{ gap: 10 }}>
            <div className="errbox">{loadErr}</div>
            <button className="btn" onClick={() => onDone(null)}>Continue without signing in</button>
          </div>
        ) : ready && creating ? (
          <div className="col login-in" style={{ gap: 10 }}>
            <div className="label">Create your profile</div>
            <input className="input" placeholder="Your name" value={form.name} autoFocus onChange={(e) => setForm({ ...form, name: e.target.value })} aria-label="Name" />
            <div className="row" style={{ gap: 8 }}>
              <input className="input mono" type="password" inputMode="numeric" placeholder="PIN (4–8 digits)" value={form.pin} onChange={(e) => setForm({ ...form, pin: e.target.value.replace(/\D/g, "").slice(0, 8) })} aria-label="PIN" />
              <input className="input mono" type="password" inputMode="numeric" placeholder="Repeat PIN" value={form.pin2} onChange={(e) => setForm({ ...form, pin2: e.target.value.replace(/\D/g, "").slice(0, 8) })} onKeyDown={(e) => e.key === "Enter" && create()} aria-label="Repeat PIN" />
            </div>
            <div className="row" style={{ gap: 6 }} role="radiogroup" aria-label="Colour">
              {COLORS.map((c) => <button key={c} role="radio" aria-checked={form.color === c} aria-label={c} className={`swatch ${form.color === c ? "on" : ""}`} style={{ background: c }} onClick={() => setForm({ ...form, color: c })} />)}
            </div>
            {err && <div className="errbox" key={shake}>{err}</div>}
            <div className="row" style={{ gap: 8 }}>
              <button className="btn primary" disabled={busy || !form.name.trim() || form.pin.length < 4} onClick={create}>Create &amp; sign in</button>
              {team?.profiles.length
                ? <button className="btn ghost" onClick={() => { setCreating(false); setErr(null); }}>Back</button>
                : <button className="btn ghost" onClick={() => onDone(null)}>Skip for now</button>}
            </div>
            <div className="dim small">Saved to the shared vault so teammates see your name on action items. The PIN only marks who did what. Anyone with the vault can work it out, so don't reuse your phone or bank PIN.</div>
          </div>
        ) : ready && who ? (
          <div className={`col login-in ${shake ? "shake" : ""}`} key={shake} style={{ gap: 12, alignItems: "center" }}>
            <div className="avatar big" style={{ background: who.color }}>{who.name[0]}</div>
            <div className="display">{who.name}</div>
            <input ref={pinRef} className="sr-only" type="password" inputMode="numeric" value={pin} aria-label={`PIN for ${who.name}`}
              onChange={(e) => setPin(e.target.value.replace(/\D/g, "").slice(0, 8))} onKeyDown={(e) => { if (e.key === "Enter") submitPin(); if (e.key === "Escape") { setWho(null); setPin(""); } }} />
            <div className="pin-dots" onClick={() => pinRef.current?.focus()}>
              {Array.from({ length: Math.max(4, pin.length) }, (_, i) => <span key={i} className={i < pin.length ? "on" : ""} />)}
            </div>
            <div className="pin-pad">
              {["1", "2", "3", "4", "5", "6", "7", "8", "9", "⌫", "0", "→"].map((d) => (
                <button key={d} className="btn" disabled={busy} onClick={() => (d === "→" ? submitPin() : press(d))} aria-label={d === "→" ? "Sign in" : d === "⌫" ? "Delete" : d}>{d}</button>
              ))}
            </div>
            {err && <div className="errbox">{err}</div>}
            <button className="btn ghost sm" onClick={() => { setWho(null); setPin(""); setErr(null); }}>Not you?</button>
          </div>
        ) : ready ? (
          <div className="col login-in" style={{ gap: 12 }}>
            <div className="login-tiles">
              {team!.profiles.map((p, i) => (
                <button key={p.id} className="login-tile" style={{ animationDelay: `${i * 70}ms` }} onClick={() => setWho(p)}>
                  <div className="avatar" style={{ background: p.color }}>{p.name[0]}</div>
                  <span>{p.name}</span>
                </button>
              ))}
              <button className="login-tile" onClick={() => setCreating(true)}><div className="avatar ghost">+</div><span>New profile</span></button>
            </div>
            <button className="btn ghost sm" onClick={() => onDone(null)}>Continue without signing in</button>
          </div>
        ) : null}
      </div>
    </div>
  );
}
