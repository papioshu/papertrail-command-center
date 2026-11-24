import { describe, expect, it, vi } from "vitest";
vi.mock("@excalidraw/excalidraw", () => ({}));
vi.mock("../api", () => ({ api: {} }));
import { taskTitle } from "./Team";

describe("taskTitle", () => {
  it("drops author/date lead-ins and bold", () => {
    expect(taskTitle("🟦 CLAUDE 2026-11-14 — Ship the bounty board")).toBe("Ship the bounty board");
    expect(taskTitle("🟧 CODEX — Graph polish")).toBe("Graph polish");
    expect(taskTitle("Owner — Send Rich the installer")).toBe("Send Rich the installer");
    expect(taskTitle("[ASTRA/CODEX] - old item")).toBe("old item");
    expect(taskTitle("**Owner:** confirm GoDaddy login")).toBe("Owner: confirm GoDaddy login");
  });
  it("keeps text that only looks similar", () => {
    expect(taskTitle("Owner reviews and merges PR #1.")).toBe("Owner reviews and merges PR #1.");
    expect(taskTitle("Fix A — B")).toBe("Fix A — B");
  });
});
