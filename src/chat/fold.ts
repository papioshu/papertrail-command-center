/** Turns flat transcript entries into display blocks: messages stay as they are, tool steps fold into runs. */
export type ChatEntry = {
  off: number; ts: string; kind: "you" | "agent" | "reasoning" | "system" | "tool_use" | "tool_result";
  text: string; tool_id?: string | null; name?: string | null; input?: any; is_error: boolean;
  images: { media_type: string; data?: string | null; omitted: boolean }[]; truncated: boolean;
};
export type Step = { use: ChatEntry; result?: ChatEntry };
export type Block =
  | { kind: "you" | "agent" | "reasoning" | "system"; entry: ChatEntry }
  | { kind: "tools"; key: string; steps: Step[]; counts: Record<string, number> };

const KINDS: Record<string, "command" | "edit" | "read" | "search"> = {
  Bash: "command", exec: "command", shell: "command", PowerShell: "command",
  Edit: "edit", Write: "edit", MultiEdit: "edit", NotebookEdit: "edit", apply_patch: "edit",
  Read: "read", Grep: "search", Glob: "search", WebSearch: "search",
};
export const stepKind = (name: string) => KINDS[name] ?? "other";

const WORDS: [string, string, string][] = [
  ["command", "command", "commands"], ["edit", "edit", "edits"], ["read", "read", "reads"],
  ["search", "search", "searches"], ["other", "other step", "other steps"],
];
export function toolSummary(counts: Record<string, number>): string {
  return WORDS.filter(([k]) => counts[k]).map(([k, one, many]) => `${counts[k]} ${counts[k] === 1 ? one : many}`).join(" · ");
}

export function foldEntries(entries: ChatEntry[], showSystem: boolean): Block[] {
  const out: Block[] = [];
  const byId = new Map<string, Step>();
  let run: Extract<Block, { kind: "tools" }> | null = null;
  for (const e of entries) {
    if (e.kind === "system" && !showSystem) continue;
    if (e.kind === "tool_result" && e.tool_id && byId.has(e.tool_id) && !byId.get(e.tool_id)!.result) {
      byId.get(e.tool_id)!.result = e;
      continue;
    }
    if (e.kind === "tool_use" || e.kind === "tool_result") {
      if (!run) out.push((run = { kind: "tools", key: String(e.off), steps: [], counts: {} }));
      const step: Step = { use: e };
      if (e.kind === "tool_use" && e.tool_id) byId.set(e.tool_id, step);
      run.steps.push(step);
      const k = stepKind(e.name ?? "");
      run.counts[k] = (run.counts[k] ?? 0) + 1;
      continue;
    }
    run = null;
    out.push({ kind: e.kind, entry: e });
  }
  return out;
}
