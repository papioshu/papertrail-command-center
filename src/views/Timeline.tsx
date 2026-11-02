import { useState } from "react";
import { api, type Row } from "../api";
import { Card, Empty, ErrBox, useAsync, useNav } from "../ui";
import { EventList } from "./Projects";

const KINDS = ["", "commit", "session", "log", "decision", "launch"];
const AGENTS = ["", "claude", "codex", "owner", "none"];
const BLANK = { project: "", agent: "", kind: "", workstream: "", from: "", to: "", q: "" };

export default function Timeline() {
  const { params } = useNav();
  const projects = useAsync(() => api.projects(true), []);
  const [f, setF] = useState<Row>({ ...BLANK, ...params });
  const [limit, setLimit] = useState(200);
  const data = useAsync(() => api.timeline({ ...f, limit }), [JSON.stringify(f), limit], ["projects-updated"]);
  const set = (k: string, v: string) => {
    setF({ ...f, [k]: v });
    setLimit(200);
  };
  const items = data.data?.items ?? [];
  return (
    <div className="col" style={{ gap: 14 }}>
      <div className="page-head">
        <div>
          <h1>Timeline</h1>
          <div className="sub">Commits · agent sessions · PROJECT LOG entries · decisions · launches</div>
        </div>
      </div>
      <Card title="Filters">
        <div className="row wrap">
          <select className="input" value={f.project} onChange={(e) => set("project", e.target.value)}>
            <option value="">All projects</option>
            {(projects.data ?? []).map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
          </select>
          <div className="seg">
            {AGENTS.map((a) => <button key={a} className={f.agent === a ? "on" : ""} onClick={() => set("agent", a)}>{a ? a.toUpperCase() : "ANY AGENT"}</button>)}
          </div>
          <div className="seg">
            {KINDS.map((k) => <button key={k} className={f.kind === k ? "on" : ""} onClick={() => set("kind", k)}>{k ? k.toUpperCase() : "ALL"}</button>)}
          </div>
          <input className="input" placeholder="workstream" value={f.workstream} onChange={(e) => set("workstream", e.target.value)} style={{ width: 120 }} />
          <input className="input mono" type="date" value={f.from} onChange={(e) => set("from", e.target.value)} aria-label="from" />
          <input className="input mono" type="date" value={f.to} onChange={(e) => set("to", e.target.value)} aria-label="to" />
          <input className="input" placeholder="search text" value={f.q} onChange={(e) => set("q", e.target.value)} style={{ width: 180 }} />
          <button className="btn sm ghost" onClick={() => setF(BLANK)}>Clear</button>
        </div>
      </Card>
      {data.error && <ErrBox error={data.error} />}
      <Card title={`${items.length}${items.length === limit ? "+" : ""} events`}>
        <EventList items={items} />
        {!items.length && !data.loading && <Empty>No events match these filters</Empty>}
        {items.length === limit && (
          <div className="row" style={{ justifyContent: "center", marginTop: 10 }}>
            <button className="btn" onClick={() => setLimit(limit + 300)}>Load more</button>
          </div>
        )}
      </Card>
    </div>
  );
}
