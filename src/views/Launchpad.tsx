import { useMemo, useState } from "react";
import { api, type Row } from "../api";
import { ago, AgentChip, Card, Empty, useAsync, useNav } from "../ui";

const AGENTS = ["claude", "codex"] as const;

export default function Launchpad() {
  const { launch, params } = useNav();
  const projects = useAsync(() => api.projects(), [], ["projects-updated"]);
  const settings = useAsync(() => api.settings(), []);
  const launches = useAsync(() => api.timeline({ kind: "launch", limit: 30 }), [], ["projects-updated"]);
  const [q, setQ] = useState("");
  const [sel, setSel] = useState<string | null>(params.id ?? null);
  const list = useMemo(
    () => (projects.data ?? []).filter((p) => !q || `${p.name} ${p.workspace}`.toLowerCase().includes(q.toLowerCase())),
    [projects.data, q],
  );
  const p = (projects.data ?? []).find((x) => x.id === sel) ?? list[0];
  const presets: Row[] = (settings.data?.presets ?? []).filter((x: Row) => !x.project || x.project === p?.id);
  return (
    <div className="col" style={{ gap: 14 }}>
      <div className="page-head">
        <div>
          <h1>Launchpad</h1>
          <div className="sub">Real interactive Claude Code / Codex sessions, opened in the project with a generated brief</div>
        </div>
      </div>
      <div className="grid g3">
        <Card title="Project" body={false}>
          <div style={{ padding: "8px 12px" }}>
            <input className="input" style={{ width: "100%" }} placeholder="Filter projects…" value={q} onChange={(e) => setQ(e.target.value)} />
          </div>
          <div style={{ maxHeight: 560, overflow: "auto" }}>
            {list.map((x) => (
              <div key={x.id} className="li click" style={{ padding: "7px 12px", background: x.id === p?.id ? "rgba(139,92,246,.14)" : undefined }} onClick={() => setSel(x.id)}>
                <span className="label" style={{ fontSize: 9, width: 72 }} title={x.workspace}>{String(x.workspace).slice(0, 10)}</span>
                <span className="ellipsis">{x.name}</span>
                <span className="dim small mono">{ago(x.last_activity)}</span>
              </div>
            ))}
          </div>
        </Card>
        <div className="span2 col" style={{ gap: 14 }}>
          {p ? (
            <>
              <Card title={`Launch · ${p.name}`}>
                <div className="grid g2">
                  {AGENTS.map((agent) => {
                    const raw = p[`last_${agent}`];
                    const last = raw ? JSON.parse(raw) : null;
                    return (
                      <div key={agent} className="card pad col">
                        <div className="row">
                          <AgentChip agent={agent} />
                          <span className="dim small">{last ? `last session ${ago(last.updated)}` : "no mapped session"}</span>
                        </div>
                        {last && <div className="small ellipsis" title={last.title}>“{last.title}”</div>}
                        <div className="row wrap">
                          <button className={`btn ${agent}`} disabled={!last} onClick={() => launch(p, agent, "continue")}>A · Continue</button>
                          <button className={`btn ${agent}`} onClick={() => launch(p, agent, "fresh")}>B · Fresh handoff</button>
                          <button className={`btn ${agent}`} onClick={() => launch(p, agent, "targeted")}>C · Targeted</button>
                          <button className={`btn ${agent}`} onClick={() => launch(p, agent, "custom")}>D · Custom</button>
                        </div>
                      </div>
                    );
                  })}
                </div>
              </Card>
              <Card title="Presets for this project" right={<span className="dim small">edit in Settings</span>}>
                {presets.length ? (
                  presets.map((x) => (
                    <div key={x.id} className="li">
                      <span className="chip">{x.agent}</span>
                      <div>
                        <div className="display" style={{ fontSize: 13 }}>{x.name}</div>
                        <div className="muted small">{x.task}</div>
                      </div>
                      <div className="row">
                        {(x.agent === "any" ? AGENTS : AGENTS.filter((a) => a === x.agent)).map((a) => (
                          <button key={a} className={`btn sm ${a}`} onClick={() => launch(p, a, "targeted", x.id)}>{a}</button>
                        ))}
                      </div>
                    </div>
                  ))
                ) : (
                  <Empty>No presets</Empty>
                )}
              </Card>
            </>
          ) : (
            <Empty>No projects</Empty>
          )}
          <Card title="Recent launches">
            {launches.data?.items.length ? (
              launches.data.items.map((e) => (
                <div key={e.id} className="li">
                  <AgentChip agent={e.agent} />
                  <div className="small">
                    <b>{e.project}</b> · {e.title}
                    <div className="mono dim ellipsis" style={{ fontSize: 10.5 }}>{e.detail}</div>
                  </div>
                  <span className="dim small">{ago(e.ts)}</span>
                </div>
              ))
            ) : (
              <Empty>No launches yet</Empty>
            )}
          </Card>
        </div>
      </div>
    </div>
  );
}
