import { describe, expect, it } from "vitest";
import { foldEntries, toolSummary, type ChatEntry } from "./fold";

const e = (kind: ChatEntry["kind"], extra: Partial<ChatEntry> = {}, off = 0): ChatEntry =>
  ({ off, seq: 0, ts: "", kind, text: "", is_error: false, images: [], truncated: false, ...extra });

describe("toolSummary", () => {
  it("orders and pluralises", () => {
    expect(toolSummary({ command: 4, edit: 2, read: 1 })).toBe("4 commands · 2 edits · 1 read");
    expect(toolSummary({ search: 1 })).toBe("1 search");
    expect(toolSummary({ other: 2, search: 3 })).toBe("3 searches · 2 other steps");
  });
});

describe("foldEntries", () => {
  const list = [
    e("you", { text: "hi" }, 1),
    e("tool_use", { tool_id: "t1", name: "Bash" }, 2),
    e("tool_result", { tool_id: "t1", text: "out" }, 3),
    e("tool_use", { tool_id: "t2", name: "Edit" }, 4),
    e("system", { text: "hook" }, 5),
    e("tool_result", { tool_id: "t2" }, 6),
    e("agent", { text: "done" }, 7),
  ];
  it("folds a tool run and hides system entries", () => {
    const b = foldEntries(list, false);
    expect(b.map((x) => x.kind)).toEqual(["you", "tools", "agent"]);
    const t = b[1] as Extract<typeof b[number], { kind: "tools" }>;
    expect(t.steps).toHaveLength(2);
    expect(t.steps.every((s) => s.result)).toBe(true);
    expect(t.counts).toEqual({ command: 1, edit: 1 });
    expect(t.key).toBe("2:0");
  });
  it("shows system entries when asked", () => {
    expect(foldEntries(list, true).map((x) => x.kind)).toEqual(["you", "tools", "system", "agent"]);
  });
  it("keeps an orphan result as its own step", () => {
    const b = foldEntries([e("tool_result", { tool_id: "zz", text: "x" }, 9)], false);
    expect(b).toHaveLength(1);
    const t = b[0] as Extract<typeof b[number], { kind: "tools" }>;
    expect(t.steps).toHaveLength(1);
  });
  it("attaches a result to a use from an earlier block", () => {
    const b = foldEntries([e("tool_use", { tool_id: "a", name: "Read" }, 1), e("agent", { text: "x" }, 2), e("tool_result", { tool_id: "a" }, 3)], false);
    expect(b.map((x) => x.kind)).toEqual(["tools", "agent"]);
    expect((b[0] as any).steps[0].result.off).toBe(3);
  });
});

import { stepHead } from "./fold";
describe("stepHead", () => {
  it("lifts the main field out and keeps the rest", () => {
    expect(stepHead({ command: "ls", description: "list", timeout: 5 })).toEqual({ head: "ls", rest: { description: "list", timeout: 5 } });
    expect(stepHead({ file_path: "a.ts", old_string: "x", new_string: "y" })).toEqual({ head: "a.ts", rest: { old_string: "x", new_string: "y" } });
    expect(stepHead({ query: "q" })).toEqual({ head: null, rest: { query: "q" } });
    expect(stepHead("raw")).toEqual({ head: null, rest: "raw" });
  });
});
