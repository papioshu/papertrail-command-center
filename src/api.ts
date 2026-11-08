import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Rows come straight from SQLite as JSON objects. */
export type Row = Record<string, any>;

/** Opened in a phone browser over Tailscale (D027): read-only, data over HTTP with the paired key. */
export const PHONE = !("__TAURI_INTERNALS__" in window);
const KEY = "pt-phone-key";
// The pairing link carries the key in the #fragment, which never reaches a server or its logs.
function takePairKey() {
  const m = location.hash.match(/pair=([0-9a-f]{64})/);
  if (m) { localStorage.setItem(KEY, m[1]); history.replaceState(null, "", location.pathname); }
  return !!m;
}
if (PHONE) {
  takePairKey();
  // Link opened while this page was already showing: same document, so pick the key up and start over.
  window.addEventListener("hashchange", () => { if (takePairKey()) location.reload(); });
  // Pair once: ask Chrome not to evict this site's storage (and with it the key) when space runs low.
  navigator.storage?.persist?.().catch(() => {});
}
export const phonePaired = () => !!localStorage.getItem(KEY);

/** The PC couldn't be reached (Tailscale off, network switch, PC asleep or restarting), as opposed to a real error. */
class Unreachable extends Error {}
/** The PC is up but the dashboard isn't running: its phone standby answered instead (D029). */
let closed = false;
export const dashboardClosed = () => closed;
const auth = () => ({ Authorization: `Bearer ${localStorage.getItem(KEY) ?? ""}` });
/** Ask the PC's phone standby to start the dashboard. Resolves to an error message, or null once it's starting. */
export async function openOnPc(): Promise<string | null> {
  try {
    const r = await fetch("/api/launch", { method: "POST", headers: auth() });
    return r.ok ? null : ((await r.json().catch(() => ({}))).error ?? `Couldn't start it (${r.status}).`);
  } catch { return "Lost the connection to your PC."; }
}

async function once<T>(cmd: string, args?: Row, wait = 30000): Promise<T> {
  let r: Response, body: any;
  try {
    r = await fetch(`/api/${cmd}`, {
      method: "POST",
      headers: { ...auth(), "Content-Type": "application/json" },
      body: JSON.stringify(args ?? {}),
      signal: AbortSignal.timeout(wait),
    });
    body = await r.json();
  } catch { throw new Unreachable(); } // includes a reply cut off mid-way
  closed = r.status === 503 && !!body?.standby;
  if (r.status === 503) throw new Unreachable();
  if (r.status === 401) window.dispatchEvent(new Event("pt-unpaired"));
  if (!r.ok) throw String(body?.error ?? r.statusText); // same shape as Tauri errors: a string
  return body as T;
}

/** Desktop: Tauri IPC. Phone: HTTP; every phone command is read-only, so an unreachable PC is simply waited out and
 *  the call repeated. Callers never see a dropped connection, only a slower answer; the page keeps what it shows. */
async function call<T>(cmd: string, args?: Row): Promise<T> {
  if (!PHONE) return invoke<T>(cmd, args);
  for (;;) {
    try { return await once<T>(cmd, args); }
    catch (e) {
      if (!(e instanceof Unreachable)) throw e;
      setOffline(true);
      await new Promise<void>((r) => waiters.push(r));
    }
  }
}

export type LaunchReq = {
  project_id: string;
  agent: "claude" | "codex";
  mode: "continue" | "fresh" | "targeted" | "custom";
  task?: string;
  preset_id?: string;
  force?: boolean;
  /** Continue mode: resume this chat instead of the project's latest. */
  session_id?: string;
};

import type { ChatEntry } from "./chat/fold";
export type ChatPage = { entries: ChatEntry[]; cursor?: number | null; end: number; reset: boolean; bad_lines: number };

export const api = {
  status: () => call<Row>("get_status"),
  projects: (includeHidden = false) => call<Row[]>("get_projects", { includeHidden }),
  briefing: () => call<Row>("get_briefing"),
  project: (id: string) => call<Row>("get_project", { id }),
  activity: (project: string | null, from: string, to: string, workspace?: string) => call<Row[]>("get_activity", { project, from, to, workspace }),
  timeline: (f: Row) => call<{ items: Row[] }>("get_timeline", { f }),
  sessions: (agent?: string) => call<Row[]>("get_sessions", { agent }),
  chatProjects: () => call<Row[]>("chat_projects"),
  chatList: (project: string, agent?: string) => call<Row[]>("chat_list", { project, agent }),
  chatPage: (id: string, before?: number) => call<{ session: Row; page: ChatPage }>("chat_page", { id, before }),
  chatTail: (id: string, from: number) => call<ChatPage>("chat_tail", { id, from }),
  chatEntry: (id: string, off: number) => call<ChatEntry[]>("chat_entry", { id, off }),
  sessionFiles: (id: string) => call<Row[]>("get_session_files", { id }),
  graph: () => call<Row>("get_graph"),
  search: (q: string) => call<Row[]>("search", { q }),
  refresh: () => call<void>("refresh_now"),
  discoveryRefresh: () => call<void>("discovery_refresh"),
  discoveries: () => call<Row>("get_discoveries"),
  setDiscovery: (id: string, status?: string, projectLink?: string) =>
    call<void>("set_discovery", { id, status, projectLink }),
  addReviewTask: (projectId: string, text: string, discoveryId?: string) =>
    call<void>("add_review_task", { projectId, text, discoveryId }),
  toggleReviewTask: (id: number) => call<void>("toggle_review_task", { id }),
  bosPreview: (discoveryId: string, projectId: string) => call<Row>("bos_preview", { discoveryId, projectId }),
  bosAppend: (discoveryId: string, projectId: string) => call<string>("bos_append", { discoveryId, projectId }),
  previewHandoff: (req: LaunchReq) => call<string>("preview_handoff", { req }),
  launch: (req: LaunchReq) => call<Row>("launch_agent", { req }),
  cliInfo: () => call<Row>("get_cli_info"),
  openPath: (path: string) => call<void>("open_path", { path }),
  openUrl: (url: string) => call<void>("open_url", { url }),
  settings: () => call<Row>("get_settings"),
  saveSettings: (patch: Row) => call<Row>("save_settings", { patch }),
  setProjectMeta: (id: string, hidden?: boolean, workspace?: string) =>
    call<void>("set_project_meta", { id, hidden, workspace }),
  registerProject: (name: string, path: string, workspace?: string) =>
    call<void>("register_project", { name, path, workspace }),
  diagnostics: () => call<Row>("get_diagnostics"),
  backup: () => call<string>("backup_db"),
  sparks: (days: number) => call<Row[]>("get_sparks", { days }),
  toggleChat: () => call<boolean>("toggle_chat"),
  folders: (id: string) => call<Row[]>("get_folders", { id }),
  taskTogglePreview: (id: number) => call<Row>("task_toggle_preview", { id }),
  taskToggle: (id: number, before: string) => call<void>("task_toggle", { id, before }),
  team: () => call<{ profiles: Row[]; me: Row | null }>("team_profiles"),
  teamCreate: (name: string, pin: string, color: string) => call<Row>("team_create", { name, pin, color }),
  login: (id: string, pin: string) => call<Row>("team_login", { id, pin }),
  logout: () => call<void>("team_logout"),
  actions: (projectId?: string) => call<Row[]>("actions_list", { projectId }),
  actionOp: (projectId: string, op: ActionOp) => call<void>("action_op", { projectId, op }),
  phoneStatus: () => call<Row>("phone_status"),
  phoneSetEnabled: (enabled: boolean) => call<void>("phone_set_enabled", { enabled }),
  phonePair: (name: string) => call<{ url: string; qr_svg: string }>("phone_pair", { name }),
  phoneRevoke: (id: string) => call<void>("phone_revoke", { id }),
  // Workspace hub (D030–D034): shared team tasks. Every edit is checked again by the hub.
  hubStatus: () => call<Row>("hub_status"),
  hubJoin: (link: string, name: string) => call<Row>("hub_join", { link, name }),
  hubLeave: () => call<void>("hub_leave"),
  hubDeviceLink: () => call<Row>("hub_device_link"),
  updateCheck: () => call<Row>("app_update_check"),
  updateInstall: () => call<void>("app_update_install"),
  hubSyncNow: () => call<void>("hub_sync_now"),
  hubBoard: (workspace: string) => call<Row>("hub_board", { workspace }),
  hubEdit: (workspace: string, action: "create" | "update" | "delete", kind: "task" | "milestone", projectId: string, fields: Row | null, recordId?: string) =>
    call<string>("hub_edit", { workspace, action, kind, recordId, projectId, fields }),
  hubResolve: (workspace: string, conflict: number, keepMine: boolean) => call<void>("hub_resolve", { workspace, conflict, keepMine }),
  hubInvite: (workspace: string, role: string, projects?: string[]) => call<string>("hub_invite", { workspace, role, projects }),
  hubNewProject: (workspace: string, name: string) => call<Row>("hub_new_project", { workspace, name }),
  /** Roles + members (D036): "roles" | "role_save" | "role_delete" | "member_set" | "member_remove". */
  hubAdmin: (workspace: string, action: string, body?: Row) => call<Row>("hub_admin", { workspace, action, body }),
  hubLinkPreview: (project: string) => call<Row>("hub_link_preview", { project }),
  hubLink: (project: string, workspace: string, doImport: boolean) => call<Row>("hub_link", { project, workspace, import: doImport }),
  hubUpload: (workspace: string, projectId: string, name: string, caption: string, folder: string, data: string) => call<string>("hub_upload", { workspace, projectId, name, caption, folder, data }),
  hubSetVaultMedia: (on: boolean) => call<void>("hub_set_vault_media", { on }),
  /** Media: pass the file name, get a data: URL. Snapshot: no name, get its JSON. */
  hubFile: (workspace: string, recordId: string, name?: string) => call<unknown>("hub_file", { workspace, recordId, name }),
  hubSnapshotPreview: (project: string) => call<Row>("hub_snapshot_preview", { project }),
  hubSnapshotPublish: (project: string) => call<string>("hub_snapshot_publish", { project }),
  hubHostStatus: () => call<Row>("hub_host_status"),
  hubOpenShareFolder: () => call<void>("hub_open_share_folder"),
  hubHostCreate: (name: string, me: string) => call<Row>("hub_host_create_workspace", { name, me }),
};

export type ActionOp = { kind: "add" | "edit" | "start" | "complete" | "reopen" | "remove"; id?: string; title?: string; notes?: string; assignee?: string };

export function on(event: string, cb: (payload: any) => void): Promise<UnlistenFn> {
  if (PHONE) return Promise.resolve(phoneOn(event, cb));
  return listen(event, (e) => cb(e.payload));
}

/* ---------- phone: connection watch ----------
   No push events on the phone, so one status poll (10 s; 3 s while the PC is unreachable) stands in for
   index-status / projects-updated and tells waiting calls when the PC is back. */
const subs = new Set<[string, (p: any) => void]>();
const waiters: (() => void)[] = [];
const watchers = new Set<(offlineSince: number) => void>();
let offlineSince = 0; // 0 = connected
let lastRefresh: string | undefined;
let hiddenAt = 0;
let timer = 0;
const fire = (ev: string, p: unknown) => subs.forEach(([e, f]) => e === ev && f(p));

function setOffline(off: boolean) {
  if (off === offlineSince > 0) return;
  offlineSince = off ? Date.now() : 0;
  watchers.forEach((f) => f(offlineSince));
  if (!off) {
    waiters.splice(0).forEach((r) => r()); // repeat everything that was waiting
    fire("projects-updated", null); // and refresh what was loaded before the gap
  }
  schedule(0);
}

function poll() {
  if (document.hidden) return schedule();
  // Short timeout: a check caught in a PC-side handoff (standby -> dashboard) must not stall reconnecting.
  once<Row>("get_status", undefined, 8000).then((s) => {
    const stale = hiddenAt && Date.now() - hiddenAt > 60000;
    hiddenAt = 0;
    if (offlineSince) setOffline(false);
    else if (stale || (lastRefresh !== undefined && s.last_refresh !== lastRefresh)) fire("projects-updated", null);
    fire("index-status", s.status);
    lastRefresh = s.last_refresh;
    schedule();
  }, (e) => { if (e instanceof Unreachable) setOffline(true); schedule(); });
}
function schedule(ms = offlineSince ? 3000 : 10000) {
  clearTimeout(timer);
  timer = window.setTimeout(poll, ms);
}
if (PHONE) {
  schedule(0);
  // Phone woke up, came back to the tab or regained a network: check at once instead of waiting for the timer.
  document.addEventListener("visibilitychange", () => { if (document.hidden) hiddenAt = Date.now(); else schedule(0); });
  window.addEventListener("online", () => schedule(0));
  window.addEventListener("pageshow", () => schedule(0));
}

/** Phone only: called with the time the PC became unreachable, or 0 when it's back. */
export function onConnection(f: (offlineSince: number) => void) {
  watchers.add(f);
  f(offlineSince);
  return () => { watchers.delete(f); };
}

function phoneOn(event: string, cb: (p: any) => void): UnlistenFn {
  const sub: [string, (p: any) => void] = [event, cb];
  subs.add(sub);
  return () => { subs.delete(sub); };
}
