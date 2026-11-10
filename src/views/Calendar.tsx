import { useCallback, useEffect, useMemo, useState } from "react";
import { api, type Row } from "../api";
import { Card, day, Empty, ErrBox, Modal } from "../ui";
import { syncTeamCalendars } from "../calsync";

/** CALENDAR (D047): the owner's Google Calendar (primary), two-way, beside the workspaces' due dates.
 *  Sign-in uses the owner's own Google OAuth client; tokens stay in the app's database, never in this page. */

type Ev = { id?: string; calendar?: string; title: string; date: string; end: string; start?: string; stop?: string; allDay: boolean; notes: string };
const DAYS = ["MON", "TUE", "WED", "THU", "FRI", "SAT", "SUN"];
const monday = (d: Date) => { const x = new Date(d.getFullYear(), d.getMonth(), d.getDate()); x.setDate(x.getDate() - ((x.getDay() + 6) % 7)); return x; };
const plus = (d: Date, n: number) => { const x = new Date(d); x.setDate(x.getDate() + n); return x; };
const hhmm = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
const timeOf = (iso: string) => { const d = new Date(iso); return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`; };
const dayOf = (e: Row) => (e.start?.date ? String(e.start.date) : day(new Date(e.start?.dateTime)));

function Setup({ hasClient, onDone, onErr }: { hasClient: boolean; onDone: () => void; onErr: (e: string) => void }) {
  const [id, setId] = useState("");
  const [secret, setSecret] = useState("");
  const [busy, setBusy] = useState(false);
  const connect = () => { setBusy(true); api.gcalConnect().then(onDone, (e) => onErr(String(e))).finally(() => setBusy(false)); };
  return (
    <Card title="Connect Google Calendar">
      <ol className="small" style={{ lineHeight: 1.7, margin: "0 0 14px", paddingLeft: 18 }}>
        <li>Open <a href="#" onClick={(e) => { e.preventDefault(); api.openUrl("https://console.cloud.google.com/projectcreate"); }}>Google Cloud console</a> and create a project (free), e.g. "Command Center".</li>
        <li>APIs &amp; Services › Library › <b>Google Calendar API</b> › Enable.</li>
        <li>OAuth consent screen: External, app name "Command Center", your email. Under Audience add yourself as a test user, then <b>Publish app</b> (otherwise Google signs you out every 7 days).</li>
        <li>Credentials › Create credentials › OAuth client ID › type <b>Desktop app</b>. Copy the Client ID and Client secret below.</li>
      </ol>
      <div className="col" style={{ gap: 8, maxWidth: 560 }}>
        <input className="input" placeholder="Client ID (…apps.googleusercontent.com)" value={id} onChange={(e) => setId(e.target.value)} aria-label="Client ID" />
        <input className="input" type="password" placeholder="Client secret" value={secret} onChange={(e) => setSecret(e.target.value)} aria-label="Client secret" />
        <div className="row" style={{ gap: 8 }}>
          <button className="btn sm" disabled={!id.trim() || !secret.trim()} onClick={() => api.gcalSetClient(id, secret).then(() => { setId(""); setSecret(""); onDone(); }, (e) => onErr(String(e)))}>Save client</button>
          <button className="btn sm primary" disabled={!hasClient || busy} onClick={connect}>{busy ? "Finish signing in in your browser…" : "Connect with Google"}</button>
        </div>
        <div className="dim small">Your browser opens Google's sign-in; afterwards it comes back to this PC by itself. Only your main calendar is used.</div>
      </div>
    </Card>
  );
}

function Editor({ ev, cals, onClose, onSaved }: { ev: Ev; cals: Row[]; onClose: () => void; onSaved: () => void }) {
  const [f, setF] = useState(ev);
  const [err, setErr] = useState("");
  const onErr = setErr;
  const [confirm, setConfirm] = useState(false);
  const set = (k: keyof Ev, v: string | boolean) => setF({ ...f, [k]: v });
  const save = () => {
    const body = f.allDay
      ? { title: f.title, notes: f.notes, start: { date: f.date }, end: { date: day(plus(new Date(`${f.end < f.date ? f.date : f.end}T00:00`), 1)) } }
      : { title: f.title, notes: f.notes, start: { dateTime: new Date(`${f.date}T${f.start || "09:00"}`).toISOString() }, end: { dateTime: new Date(`${f.end < f.date ? f.date : f.end}T${f.stop || f.start || "10:00"}`).toISOString() } };
    api.gcalSave(f.id ?? null, body, f.calendar).then(onSaved, (e) => onErr(String(e)));
  };
  return (
    <Modal title={f.id ? "Edit event" : "New event"} onClose={onClose} foot={
      <>
        {f.id && (confirm
          ? <button className="btn sm" style={{ color: "var(--signal)" }} onClick={() => api.gcalDelete(f.id!, f.calendar).then(onSaved, (e) => onErr(String(e)))}>Delete from Google Calendar</button>
          : <button className="btn sm ghost" onClick={() => setConfirm(true)}>Delete</button>)}
        <span style={{ flex: 1 }} />
        <button className="btn sm ghost" onClick={onClose}>Cancel</button>
        <button className="btn sm primary" disabled={!f.title.trim()} onClick={save}>Save to Google</button>
      </>
    }>
      <div className="col" style={{ gap: 10 }}>
        {err && <ErrBox error={err} />}
        <input className="input" autoFocus placeholder="Title" value={f.title} onChange={(e) => set("title", e.target.value)} aria-label="Title" />
        {!f.id && cals.length > 1 && (
          <select className="input" value={f.calendar ?? ""} onChange={(e) => set("calendar", e.target.value)} aria-label="Calendar">
            {cals.map((c) => <option key={c.id} value={c.primary ? "" : c.id}>{c.primary ? "My calendar" : c.name}</option>)}
          </select>
        )}
        <label className="row small" style={{ gap: 8 }}><input type="checkbox" checked={f.allDay} onChange={(e) => set("allDay", e.target.checked)} /> All day</label>
        <div className="row wrap" style={{ gap: 8 }}>
          <input className="input" type="date" value={f.date} onChange={(e) => setF({ ...f, date: e.target.value, end: f.end < e.target.value ? e.target.value : f.end })} aria-label="Start date" />
          {!f.allDay && <input className="input" type="time" value={f.start ?? ""} onChange={(e) => set("start", e.target.value)} aria-label="Start time" />}
          <span className="dim small" style={{ alignSelf: "center" }}>to</span>
          <input className="input" type="date" value={f.end} min={f.date} onChange={(e) => set("end", e.target.value)} aria-label="End date" />
          {!f.allDay && <input className="input" type="time" value={f.stop ?? ""} onChange={(e) => set("stop", e.target.value)} aria-label="End time" />}
        </div>
        <textarea className="input" rows={3} placeholder="Notes" value={f.notes} onChange={(e) => set("notes", e.target.value)} aria-label="Notes" />
      </div>
    </Modal>
  );
}

export default function Calendar() {
  const [st, setSt] = useState<Row | null>(null);
  const [week, setWeek] = useState(() => monday(new Date()));
  const [events, setEvents] = useState<Row[]>([]);
  const [dues, setDues] = useState<Row[]>([]);
  const [err, setErr] = useState("");
  const [msg, setMsg] = useState("");
  const [edit, setEdit] = useState<Ev | null>(null);
  const [cals, setCals] = useState<Row[]>([]);
  const [spaces, setSpaces] = useState<Row[]>([]);
  const team = (st?.team ?? {}) as Record<string, string>;
  const teamKey = JSON.stringify(team);
  const days = useMemo(() => DAYS.map((_, i) => plus(week, i)), [week]);
  const status = useCallback(() => api.gcalStatus().then(setSt, (e) => setErr(String(e))), []);
  useEffect(() => { status(); }, [status]);

  // Your calendar plus every team calendar, merged.
  const load = useCallback(() => {
    if (!st?.connected) return;
    const ids = ["", ...new Set(Object.values(JSON.parse(teamKey) as Record<string, string>))];
    Promise.all(ids.map((c) => api.gcalEvents(week.toISOString(), plus(week, 7).toISOString(), c)))
      .then((x) => { setEvents(x.flat()); setErr(""); }, (e) => setErr(String(e)));
  }, [st?.connected, week, teamKey]);
  const sync = useCallback(() => syncTeamCalendars().then((r) => { if (r && r.added + r.updated + r.removed) setMsg(`Team calendars: ${r.added} added, ${r.updated} updated, ${r.removed} removed.`); load(); }, (e) => setErr(String(e))), [load]);
  useEffect(() => { sync(); const t = setInterval(sync, 5 * 60 * 1000); return () => clearInterval(t); }, [sync]);
  useEffect(() => { if (st?.connected) api.gcalCalendars().then(setCals, (e) => setErr(String(e))); }, [st?.connected]);

  // Open tasks and milestones with a due date, from every workspace this PC has joined.
  useEffect(() => {
    (async () => {
      const s = await api.hubStatus().catch(() => null);
      setSpaces((s?.workspaces ?? []) as Row[]);
      const out: Row[] = [];
      for (const w of (s?.workspaces ?? []) as Row[]) {
        const b = await api.hubBoard(w.id).catch(() => null);
        for (const r of (b?.records ?? []) as Row[]) {
          if ((r.kind === "task" || r.kind === "milestone") && r.fields?.due && r.fields?.status !== "done") out.push({ id: r.id, title: `${w.name}: ${r.fields.title}`, due: String(r.fields.due).slice(0, 10), kind: r.kind });
        }
      }
      setDues(out);
    })();
  }, []);

  const open = (e: Row) => {
    const all = !!e.start?.date;
    const endDay = all ? day(plus(new Date(`${e.end.date}T00:00`), -1)) : day(new Date(e.end?.dateTime));
    setEdit({ id: e.id, calendar: e.calendar || undefined, title: String(e.title ?? ""), notes: String(e.notes ?? ""), allDay: all, date: dayOf(e), end: endDay, start: all ? undefined : timeOf(e.start.dateTime), stop: all ? undefined : timeOf(e.end.dateTime) });
  };
  const color = (cal?: string) => cals.find((c) => c.id === cal)?.color;

  if (!st) return null;
  return (
    <div>
      <div className="page-head">
        <div><h1>Calendar</h1><div className="sub">Your Google Calendar, both ways · team calendars keep workspace due dates</div></div>
        {st.connected && (
          <div className="right row" style={{ gap: 8 }}>
            <button className="btn sm ghost" onClick={() => setWeek(plus(week, -7))} aria-label="Previous week">‹</button>
            <button className="btn sm" onClick={() => setWeek(monday(new Date()))}>Today</button>
            <button className="btn sm ghost" onClick={() => setWeek(plus(week, 7))} aria-label="Next week">›</button>
            <button className="btn sm primary" onClick={() => setEdit({ title: "", notes: "", allDay: false, date: day(new Date()), end: day(new Date()), start: "09:00", stop: "10:00" })}>+ Event</button>
            {Object.keys(team).length > 0 && <button className="btn sm" onClick={sync} title="Team calendars also sync by themselves every 5 minutes">Sync team now</button>}
          </div>
        )}
      </div>
      {err && <div onClick={() => setErr("")} style={{ cursor: "pointer" }}><ErrBox error={err} /></div>}
      {msg && <div className="dim small" style={{ marginBottom: 10 }}>{msg}</div>}
      {!st.connected ? <Setup hasClient={!!st.has_client} onDone={status} onErr={setErr} /> : (
        <>
          <div className="cal-week">
            {days.map((d, i) => {
              const k = day(d);
              const evs = events.filter((e) => dayOf(e) === k);
              const ds = dues.filter((x) => x.due === k);
              return (
                <div key={k} className={`cal-day ${k === day(new Date()) ? "today" : ""}`}>
                  <button className="cal-head" onClick={() => setEdit({ title: "", notes: "", allDay: true, date: k, end: k })} title="Add an event on this day">
                    <span>{DAYS[i]}</span><b>{d.getDate()}</b>
                  </button>
                  {evs.map((e) => (
                    <button key={`${e.calendar}:${e.id}`} className={`cal-ev ${e.ptcc ? "due" : ""}`} onClick={() => open(e)} title={String(e.title ?? "")} style={e.calendar && color(e.calendar) ? { borderLeftColor: color(e.calendar) } : undefined}>
                      {e.start?.dateTime && <span className="mono">{hhmm(e.start.dateTime)}</span>} {e.title || "(no title)"}
                    </button>
                  ))}
                  {ds.filter((x) => !evs.some((e) => e.ptcc === x.id)).map((x) => <div key={x.id} className="cal-due" title="Workspace due date (not in Google Calendar yet)">◇ {x.title}</div>)}
                  {!evs.length && !ds.length && <div className="dim small" style={{ padding: "4px 2px" }}>—</div>}
                </div>
              );
            })}
          </div>
          <div className="row" style={{ gap: 8, marginTop: 14 }}>
            <span className="dim small">Connected {st.connected_at ? `since ${String(st.connected_at).slice(0, 10)}` : ""} · refreshes every 5 minutes</span>
            <button className="btn sm ghost" onClick={() => api.gcalDisconnect().then(status, (e) => setErr(String(e)))}>Disconnect</button>
          </div>
          <Card title="Team calendars" className="cal-team">
            <div className="dim small" style={{ marginBottom: 10, lineHeight: 1.6 }}>
              One Google calendar per workspace keeps the team's open due dates, updated by itself. Set up once: in Google Calendar, Other calendars › + › Create new calendar (e.g. "BOMBSQUAD"); pick it below; then in that calendar's settings › Share with specific people, add Rich's and Profit's Google accounts. They tap Accept and it shows on their phones. Nothing to install for them.
            </div>
            {!spaces.length ? <Empty>Join a workspace in TEAM first.</Empty> : spaces.map((w) => (
              <div key={w.id} className="row" style={{ gap: 10, alignItems: "center", marginBottom: 6 }}>
                <b style={{ minWidth: 140 }}>{w.name}</b>
                <select className="input" value={team[w.id] ?? ""} aria-label={`Google calendar for ${w.name}`}
                  onChange={(e) => api.gcalSetTeam(w.id, e.target.value).then(() => status(), (x) => setErr(String(x)))}>
                  <option value="">Not synced</option>
                  {cals.filter((c) => !c.primary).map((c) => <option key={c.id} value={c.id}>{c.name}</option>)}
                </select>
              </div>
            ))}
            {spaces.length > 0 && !cals.some((c) => !c.primary) && <div className="dim small">No other calendars yet: create one in Google Calendar, then reopen this tab.</div>}
          </Card>
          {!events.length && !dues.length && <Empty>Nothing this week.</Empty>}
        </>
      )}
      {edit && <Editor ev={edit} cals={cals} onClose={() => setEdit(null)} onSaved={() => { setEdit(null); load(); }} />}
    </div>
  );
}
