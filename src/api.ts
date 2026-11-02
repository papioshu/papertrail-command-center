import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Rows come straight from SQLite as JSON objects. */
export type Row = Record<string, any>;

export type LaunchReq = {
  project_id: string;
  agent: "claude" | "codex";
  mode: "continue" | "fresh" | "targeted" | "custom";
  task?: string;
  preset_id?: string;
  force?: boolean;
};

export const api = {
  status: () => invoke<Row>("get_status"),
  projects: (includeHidden = false) => invoke<Row[]>("get_projects", { includeHidden }),
  briefing: () => invoke<Row>("get_briefing"),
  project: (id: string) => invoke<Row>("get_project", { id }),
  activity: (project: string | null, from: string, to: string) => invoke<Row[]>("get_activity", { project, from, to }),
  timeline: (f: Row) => invoke<{ items: Row[] }>("get_timeline", { f }),
  sessions: (agent?: string) => invoke<Row[]>("get_sessions", { agent }),
  sessionFiles: (id: string) => invoke<Row[]>("get_session_files", { id }),
  graph: () => invoke<Row>("get_graph"),
  search: (q: string) => invoke<Row[]>("search", { q }),
  refresh: () => invoke<void>("refresh_now"),
  discoveryRefresh: () => invoke<void>("discovery_refresh"),
  discoveries: () => invoke<Row>("get_discoveries"),
  setDiscovery: (id: string, status?: string, projectLink?: string) =>
    invoke<void>("set_discovery", { id, status, projectLink }),
  addReviewTask: (projectId: string, text: string, discoveryId?: string) =>
    invoke<void>("add_review_task", { projectId, text, discoveryId }),
  toggleReviewTask: (id: number) => invoke<void>("toggle_review_task", { id }),
  bosPreview: (discoveryId: string, projectId: string) => invoke<Row>("bos_preview", { discoveryId, projectId }),
  bosAppend: (discoveryId: string, projectId: string) => invoke<string>("bos_append", { discoveryId, projectId }),
  previewHandoff: (req: LaunchReq) => invoke<string>("preview_handoff", { req }),
  launch: (req: LaunchReq) => invoke<Row>("launch_agent", { req }),
  cliInfo: () => invoke<Row>("get_cli_info"),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  settings: () => invoke<Row>("get_settings"),
  saveSettings: (patch: Row) => invoke<Row>("save_settings", { patch }),
  setProjectMeta: (id: string, hidden?: boolean, workspace?: string) =>
    invoke<void>("set_project_meta", { id, hidden, workspace }),
  registerProject: (name: string, path: string, workspace?: string) =>
    invoke<void>("register_project", { name, path, workspace }),
  diagnostics: () => invoke<Row>("get_diagnostics"),
  backup: () => invoke<string>("backup_db"),
  sparks: (days: number) => invoke<Row[]>("get_sparks", { days }),
  toggleChat: () => invoke<boolean>("toggle_chat"),
};

export function on(event: string, cb: (payload: any) => void): Promise<UnlistenFn> {
  return listen(event, (e) => cb(e.payload));
}
