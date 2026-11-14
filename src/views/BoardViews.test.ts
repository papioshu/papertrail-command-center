import { describe, expect, it, vi } from "vitest";
vi.mock("@excalidraw/excalidraw", () => ({}));
vi.mock("../api", () => ({ api: {} }));
import { shotCsv, storyOrder } from "./BoardViews";

const f = (id: string, x: number, y: number, shot: object = {}, name = id) => ({ id, type: "frame", version: 1, x, y, width: 100, name, customData: { shot } });

describe("storyboard", () => {
  it("orders shots by scene (numeric), then rows, then left to right", () => {
    const shots = [f("c", 0, 0, { scene: "10" }), f("b", 500, 0, { scene: "2" }), f("a2", 300, 10, { scene: "2" }), f("row2", 0, 400, { scene: "2" }), f("first", 0, 0, { scene: "1" })];
    expect(storyOrder(shots).map((s) => s.id)).toEqual(["first", "a2", "b", "row2", "c"]);
  });
  it("writes a quoted CSV shot list with names and task titles", () => {
    const csv = shotCsv([{ ...f("s1", 0, 0, { scene: "1", duration: 2.5, dialogue: 'He says "go", then, runs\nfast', assignee: "u1" }, "Open"), customData: { task: "t1", shot: { scene: "1", duration: 2.5, dialogue: 'He says "go", then, runs\nfast', assignee: "u1" } } }], () => "Rich", () => "Animatic");
    const [head, row] = csv.trim().split("\r\n");
    expect(head).toBe('"#","Scene","Shot","Seconds","Angle","Movement","Action","Dialogue","Status","Assigned","Task"');
    expect(row).toBe('"1","1","Open","2.5","","","","He says ""go"", then, runs\nfast","draft","Rich","Animatic"');
  });
});
