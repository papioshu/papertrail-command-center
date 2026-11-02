import { useMemo, type CSSProperties } from "react";
import { api } from "../api";
import { ago, Barcode, brand, day, DayChart, Empty, ErrBox, rangeFor, Spark, StatusBar, useAsync, useNav } from "../ui";
import { sumCounts, Ticket } from "./Overview";

/** Workspace overview: one brand, all of its projects, combined activity. Every number is a sum of indexed records. */
export default function Workspace() {
  const { params, nav } = useNav();
  const ws: string = params.ws ?? "";
  const projects = useAsync(() => api.projects(), [], ["projects-updated"]);
  const r = useMemo(() => rangeFor("90d"), []);
  const act = useAsync(() => api.activity(null, r.from, r.to, ws), [ws], ["projects-updated"]);
  const sparkRows = useAsync(() => api.sparks(14), [], ["projects-updated"]);
  const ps = useMemo(() => (projects.data ?? []).filter((p) => p.workspace === ws).sort((a, b) => String(b.last_activity ?? "").localeCompare(String(a.last_activity ?? ""))), [projects.data, ws]);
  const days = useMemo(() => { const out: string[] = []; for (let i = 13; i >= 0; i--) { const d = new Date(); d.setDate(d.getDate() - i); out.push(day(d)); } return out; }, []);
  const spark = (id: string) => { const a = days.map(() => 0); for (const x of sparkRows.data ?? []) if (x.project_id === id) { const i = days.indexOf(x.day); if (i >= 0) a[i] += x.n; } return a; };
  const total = days.map((_, i) => ps.reduce((s, p) => s + spark(p.id)[i], 0));
  const b = brand(ws);
  const glow = { ["--glow" as string]: b.glow } as CSSProperties;
  const last = ps.map((p) => p.last_activity).filter(Boolean).sort().pop();
  const sum = (k: string) => ps.reduce((a, p) => a + (p[k] || 0), 0);
  const commits90 = (act.data ?? []).reduce((a, x) => a + (x.commits || 0), 0);

  if (projects.error) return <ErrBox error={projects.error} />;
  return (
    <div>
      <div className="row" style={{ marginBottom: 14 }}>
        <button className="btn sm" onClick={() => nav("overview")}>← Dashboard</button>
      </div>
      <div className="wshero" style={glow}>
        <div className={`acard ${b.img ? "" : "gen"} on`} style={{ ...glow, flex: "0 0 200px", cursor: "default" }}>
          {b.img ? <img src={b.img} alt={ws} /> : <div className="title">{ws}</div>}
          <div className="shade" />
          <div style={{ position: "absolute", left: 14, right: 14, bottom: 14 }}><StatusBar counts={sumCounts(ps)} height={4} /></div>
        </div>
        <div className="col" style={{ gap: 14, minWidth: 0, flex: 1 }}>
          <div>
            <div className="label" style={{ color: b.glow }}>Workspace</div>
            <h1 className="wsname">{ws}</h1>
            <div className="when">{ps.length} project{ps.length === 1 ? "" : "s"} · last activity {ago(last)}</div>
          </div>
          <div className="board" style={{ alignSelf: "flex-start" }}>
            <div><div className="v">{sum("active_tasks")}</div><div className="k">Open now</div></div>
            <div><div className="v" style={{ color: sum("blocked_tasks") ? "var(--gold)" : undefined }}>{sum("blocked_tasks")}</div><div className="k">Blocked</div></div>
            <div><div className="v">{sum("done_tasks")}</div><div className="k">Done</div></div>
            <div><div className="v">{commits90}</div><div className="k">Commits · 90d</div></div>
          </div>
          <div className="row" style={{ gap: 18, alignItems: "flex-end" }}>
            <div style={{ flex: 1, maxWidth: 420 }}><div className="label" style={{ fontSize: 8.5, marginBottom: 5 }}>Tasks by status · all projects</div><StatusBar counts={sumCounts(ps)} legend /></div>
            <div><div className="label" style={{ fontSize: 8.5, marginBottom: 3 }}>Recorded activity · 14 days</div><Spark values={total} width={220} height={30} color={b.glow} /></div>
            <Barcode value={ws} width={90} height={22} />
          </div>
        </div>
      </div>

      <div className="sect"><h2>Activity · 90 days</h2><div className="rule" /><span className="aside">sum across the workspace · click a bar for the records</span></div>
      <section className="card"><div className="card-body">
        {act.data ? (
          <DayChart rows={act.data} from={r.from} to={r.to} height={170} onPick={(d) => nav("timeline", { from: d, to: d })}
            series={[
              { key: "commits", label: "Commits", color: "var(--violet)" },
              { key: "claude_prompts", label: "Claude prompts", color: "var(--claude)" },
              { key: "codex_sessions", label: "Codex threads", color: "var(--codex)" },
              { key: "log_entries", label: "Log entries", color: "var(--violet-soft)" },
              { key: "decisions", label: "Decisions", color: "var(--gold)" },
            ]} />
        ) : <div className="skeleton" />}
      </div></section>

      <div className="sect"><h2>Projects in {ws}</h2><div className="rule" /><span className="aside">by last activity · open one for its dossier</span></div>
      <div className="tickets">{ps.map((p) => <Ticket key={p.id} p={p} spark={spark(p.id)} />)}</div>
      {!ps.length && !projects.loading && <Empty>No projects in this workspace</Empty>}
    </div>
  );
}
