import { useEffect, useState } from "react";
import { api, type Row } from "../api";
import { ago, Card, DataTable, ErrBox, useAsync } from "../ui";

export default function Settings() {
  const s = useAsync(() => api.settings(), []);
  const diag = useAsync(() => api.diagnostics(), [], ["projects-updated"]);
  const cli = useAsync(() => api.cliInfo(), []);
  const [form, setForm] = useState<Row | null>(null);
  const [msg, setMsg] = useState<{ ok?: string; err?: string }>({});
  const [reg, setReg] = useState({ name: "", path: "", workspace: "" });

  useEffect(() => {
    if (s.data) setForm({
      ...s.data,
      project_roots: (s.data.project_roots ?? []).join("\n"),
      presets: JSON.stringify(s.data.presets, null, 2),
      workstream_aliases: JSON.stringify(s.data.workstream_aliases, null, 2),
    });
  }, [s.data]);

  async function save(patch?: Row) {
    setMsg({});
    try {
      const p = patch ?? {
        vault_path: form!.vault_path || null,
        project_roots: String(form!.project_roots).split(/\r?\n/).map((x) => x.trim()).filter(Boolean),
        claude_dir: form!.claude_dir,
        codex_dir: form!.codex_dir,
        refresh_minutes: Number(form!.refresh_minutes),
        discovery_refresh_hours: Number(form!.discovery_refresh_hours),
        close_to_tray: !!form!.close_to_tray,
        chat_url: form!.chat_url,
        presets: JSON.parse(form!.presets),
        workstream_aliases: JSON.parse(form!.workstream_aliases),
      };
      if (p.presets && !Array.isArray(p.presets)) throw new Error("presets must be a JSON array");
      await api.saveSettings(p);
      setMsg({ ok: "Saved. Re-indexing in the background." });
      s.reload();
    } catch (e) {
      setMsg({ err: String(e) });
    }
  }
  if (s.error) return <ErrBox error={s.error} />;
  if (!form) return <div className="skeleton" />;
  const field = (k: string, label: string, type = "text") => (
    <label className="col" style={{ gap: 4 }}>
      <span className="label">{label}</span>
      <input className="input mono" type={type} value={form[k] ?? ""} onChange={(e) => setForm({ ...form, [k]: e.target.value })} />
    </label>
  );
  const d = diag.data;
  return (
    <div className="col" style={{ gap: 14 }}>
      <div className="page-head">
        <div><h1>Settings</h1><div className="sub">Indexing · startup · presets · privacy · diagnostics</div></div>
        <div className="right"><button className="btn primary" onClick={() => save()}>Save settings</button></div>
      </div>
      {msg.ok && <div className="notice">{msg.ok}</div>}
      {msg.err && <ErrBox error={msg.err} />}
      <div className="grid g2">
        <Card title="Windows">
          <div className="col">
            <label className="row"><input type="checkbox" checked={!!s.data!.autostart} onChange={(e) => save({ autostart: e.target.checked })} /> Start at Windows login</label>
            <label className="row"><input type="checkbox" checked={!!form.close_to_tray} onChange={(e) => setForm({ ...form, close_to_tray: e.target.checked })} /> Closing the window keeps it in the tray (Quit from the tray menu)</label>
            <div className="dim small">Agents you launch run in their own terminal windows and keep running if this app exits.</div>
          </div>
        </Card>
        <Card title="Sources">
          <div className="col">
            {field("vault_path", "Book of Secrets vault")}
            <label className="col" style={{ gap: 4 }}>
              <span className="label">Project roots (one per line; scanned two levels deep for repos)</span>
              <textarea className="input" rows={3} value={form.project_roots} onChange={(e) => setForm({ ...form, project_roots: e.target.value })} />
            </label>
            <div className="grid g2">{field("claude_dir", "Claude data dir")}{field("codex_dir", "Codex data dir")}</div>
            {field("chat_url", "Chat slide-out URL (https)")}
            <div className="grid g2">{field("refresh_minutes", "Re-index every (minutes)", "number")}{field("discovery_refresh_hours", "Discovery every (hours)", "number")}</div>
            <div className="dim small">Excluded from scans: node_modules, .git internals, dist, build, .next, .cache, .venv, target, backups. .env files and credentials are never read.</div>
          </div>
        </Card>
        <Card title="Launch presets (JSON)">
          <textarea className="input" rows={14} value={form.presets} onChange={(e) => setForm({ ...form, presets: e.target.value })} />
          <div className="dim small" style={{ marginTop: 6 }}>Fields: id, name, agent ("claude" | "codex" | "any"), project (project id or null for all), task.</div>
        </Card>
        <Card title="Workstream aliases (JSON)">
          <textarea className="input" rows={6} value={form.workstream_aliases} onChange={(e) => setForm({ ...form, workstream_aliases: e.target.value })} />
          <div className="dim small" style={{ marginTop: 6 }}>Rename or merge git-derived workstreams, e.g. {"{\"board\": \"Skating\", \"combat\": \"Combat\"}"}.</div>
          <div className="sep" />
          <span className="label">Register a project manually</span>
          <div className="row wrap" style={{ marginTop: 6 }}>
            <input className="input" placeholder="Name" value={reg.name} onChange={(e) => setReg({ ...reg, name: e.target.value })} />
            <input className="input mono" placeholder="C:\\path\\to\\repo" value={reg.path} onChange={(e) => setReg({ ...reg, path: e.target.value })} style={{ flex: 1 }} />
            <input className="input" placeholder="Workspace" value={reg.workspace} onChange={(e) => setReg({ ...reg, workspace: e.target.value })} />
            <button className="btn" onClick={() => api.registerProject(reg.name, reg.path, reg.workspace || undefined).then(() => { setMsg({ ok: `Registered ${reg.name}.` }); setReg({ name: "", path: "", workspace: "" }); }, (e) => setMsg({ err: String(e) }))}>Register</button>
          </div>
        </Card>
        <Card title="CLIs">
          <div className="col small mono">
            <div>claude: {cli.data?.claude ? `${cli.data.claude.version} · ${cli.data.claude.path}` : "not found"}</div>
            <div>codex: {cli.data?.codex ? `${cli.data.codex.version} · ${cli.data.codex.path}` : "not found"}</div>
            <div>terminal: {cli.data?.wt ?? "wt.exe not found, console fallback"}</div>
            <div>git: {cli.data?.git ?? "not found"}</div>
          </div>
          <div className="dim small" style={{ marginTop: 6 }}>Uses your installed, signed-in CLIs. No API keys; no permission-bypass flags.</div>
        </Card>
        <Card title="Diagnostics" right={<><button className="btn sm" onClick={() => api.backup().then((p) => setMsg({ ok: `Backup written: ${p}` }), (e) => setMsg({ err: String(e) }))}>Back up DB</button>{d && <button className="btn sm" onClick={() => api.openPath(d.data_dir)}>Data folder</button>}</>}>
          {d && (
            <div className="col small">
              <div className="mono">{Object.entries(d.counts ?? {}).map(([k, v]) => `${k} ${v}`).join(" · ")}</div>
              <div className="mono dim">db {(d.db_bytes / 1048576).toFixed(1)} MB · pid {d.pid} · {d.data_dir}</div>
            </div>
          )}
        </Card>
      </div>
      <Card title={`Index errors (${d?.errors?.length ?? 0})`} body={false}>
        <DataTable rows={d?.errors ?? []} height={260} cols={[
          { key: "ts", label: "When", width: "14%", render: (r) => ago(r.ts) },
          { key: "scope", label: "Scope", width: "18%" },
          { key: "message", label: "Message", width: "68%" },
        ]} />
      </Card>
    </div>
  );
}
