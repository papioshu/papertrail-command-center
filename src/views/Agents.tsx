import { useState } from "react";
import { api, type Row } from "../api";
import { Card, DataTable, ErrBox, useAsync, type Col } from "../ui";
import { SessionModal, sessionCols } from "./Projects";

export default function Agents() {
  const [agent, setAgent] = useState("");
  const cli = useAsync(() => api.cliInfo(), []);
  const sessions = useAsync(() => api.sessions(agent || undefined), [agent], ["projects-updated"]);
  const projects = useAsync(() => api.projects(true), [], ["projects-updated"]);
  const [open, setOpen] = useState<Row | null>(null);
  const cols: Col[] = [
    ...sessionCols.slice(0, 3),
    { key: "project", label: "Project", width: "14%", render: (r) => r.project ?? <span className="dim">unmapped</span> },
    ...sessionCols.slice(3),
  ];
  const all = sessions.data ?? [];
  const live = all.filter((s) => Date.now() - Date.parse(s.updated) < 180e3).length;
  const cliCard = (name: string, info?: Row | null) => (
    <Card title={name}>
      {info ? (
        <div className="col small">
          <div className="mono">{info.version}</div>
          <div className="mono dim ellipsis" title={info.path}>{info.path}</div>
        </div>
      ) : cli.loading ? (
        <div className="dim small">Checking…</div>
      ) : (
        <ErrBox error={name + " not found on PATH. Launches for it fail until it is installed."} />
      )}
    </Card>
  );
  return (
    <div className="col" style={{ gap: 14 }}>
      <div className="page-head">
        <div>
          <h1>Agents</h1>
          <div className="sub">Claude Code transcripts + Codex thread index · read-only</div>
        </div>
        <div className="right">
          <div className="seg">
            {["", "claude", "codex"].map((a) => (
              <button key={a} className={agent === a ? "on" : ""} onClick={() => setAgent(a)}>{a ? a.toUpperCase() : "ALL"}</button>
            ))}
          </div>
        </div>
      </div>
      <div className="grid g4">
        {cliCard("Claude Code CLI", cli.data?.claude)}
        {cliCard("Codex CLI", cli.data?.codex)}
        <Card title="Terminal host">
          <div className="mono small">{cli.data?.wt ? "Windows Terminal" : "console window (wt.exe not found)"}</div>
        </Card>
        <Card title="Live evidence">
          <div className="mono small">{live} session{live === 1 ? "" : "s"} wrote to a transcript in the last 3 min</div>
          <div className="dim small">A transcript file existing does not mean a process is running.</div>
        </Card>
      </div>
      {sessions.error && <ErrBox error={sessions.error} />}
      <Card title={`${all.length} sessions`} body={false}>
        <DataTable rows={all} cols={cols} height={600} initialSort={["updated", -1]}
          onRow={(r) => setOpen({ ...r, project_row: projects.data?.find((p) => p.id === r.project_id) })} />
      </Card>
      {open && <SessionModal s={open} onClose={() => setOpen(null)} />}
    </div>
  );
}
