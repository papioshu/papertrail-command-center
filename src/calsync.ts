import { api, type Row } from "./api";

/** Keeps each workspace's Google team calendar in step (D048). Runs only on the PC that connected Google (the owner's);
 *  everyone else just sees the shared calendar in Google. Every task/milestone is sent, done or not, so the Rust side
 *  can remove finished or deleted ones; if any workspace can't be read, nothing is synced (it would look deleted). */
export async function syncTeamCalendars(): Promise<Row | null> {
  const s = await api.gcalStatus();
  const team = (s.team ?? {}) as Record<string, string>;
  if (!s.connected || !Object.keys(team).length) return null;
  const hub = await api.hubStatus();
  const items: Row[] = [];
  for (const w of (hub.workspaces ?? []) as Row[]) {
    if (!team[w.id]) continue;
    const b = await api.hubBoard(w.id);
    for (const r of (b.records ?? []) as Row[]) {
      if (r.kind === "task" || r.kind === "milestone") {
        items.push({ id: r.id, workspace: w.id, title: `${r.kind === "milestone" ? "◆ " : ""}${r.fields?.title ?? ""}`, due: String(r.fields?.due ?? "").slice(0, 10), done: r.fields?.status === "done" });
      }
    }
  }
  return api.gcalSyncTeam(items);
}
