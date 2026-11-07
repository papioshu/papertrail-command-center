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
}
export const phonePaired = () => !!localStorage.getItem(KEY);

async function call<T>(cmd: string, args?: Row): Promise<T> {
  if (!PHONE) return invoke<T>(cmd, args);
  const r = await fetch(`/api/${cmd}`, {
    method: "POST",
    headers: { Authorization: `Bearer ${localStorage.getItem(KEY) ?? ""}`, "Content-Type": "application/json" },
    body: JSON.stringify(args ?? {}),
  });
  const body = await r.json().catch(() => ({ error: `${r.status} ${r.statusText}` }));
  if (r.status === 401) window.dispatchEvent(new Event("pt-unpaired"));
  if (!r.ok) throw String(body.error ?? r.statusText); // same shape as Tauri errors: a string
  return body as T;
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
};

export type ActionOp = { kind: "add" | "edit" | "start" | "complete" | "reopen" | "remove"; id?: string; title?: string; notes?: string; assignee?: string };

export function on(event: string, cb: (payload: any) => void): Promise<UnlistenFn> {
  if (PHONE) return Promise.resolve(phoneOn(event, cb));
  return listen(event, (e) => cb(e.payload));
}

// Phone: no push events, so one shared 10 s status poll stands in for index-status / projects-updated.
const subs = new Set<[string, (p: any) => void]>();
let lastRefresh: string | undefined;
let timer = 0;
function poll() {
  if (document.hidden) return;
  call<Row>("get_status").then((s) => {
    const fire = (ev: string, p: unknown) => subs.forEach(([e, f]) => e === ev && f(p));
    fire("index-status", s.status);
    if (lastRefresh !== undefined && s.last_refresh !== lastRefresh) fire("projects-updated", null);
    lastRefresh = s.last_refresh;
  }, () => {});
}
function phoneOn(event: string, cb: (p: any) => void): UnlistenFn {
  const sub: [string, (p: any) => void] = [event, cb];
  subs.add(sub);
  if (!timer) { timer = window.setInterval(poll, 10000); poll(); }
  return () => { subs.delete(sub); };
}
