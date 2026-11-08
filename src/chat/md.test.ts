import { describe, expect, it } from "vitest";
import { inline, mdBlocks } from "./md";

describe("mdBlocks", () => {
  it("splits headings, paragraphs, lists and code", () => {
    expect(mdBlocks("# Title\n\npara one\nline two\n\n- a\n- b\n\n```ts\nx<y\n```")).toEqual([
      { t: "h", level: 1, text: "Title" },
      { t: "p", text: "para one\nline two" },
      { t: "li", ordered: false, text: "a" },
      { t: "li", ordered: false, text: "b" },
      { t: "code", lang: "ts", body: "x<y" },
    ]);
  });
  it("handles ordered items and an unclosed fence", () => {
    expect(mdBlocks("1. one\n2. two\n```\nopen")).toEqual([
      { t: "li", ordered: true, text: "one" },
      { t: "li", ordered: true, text: "two" },
      { t: "code", lang: "", body: "open" },
    ]);
  });
});

describe("inline", () => {
  it("finds code and bold", () => {
    expect(inline("use `npm i` **now**")).toEqual([
      { t: "text", s: "use " }, { t: "code", s: "npm i" }, { t: "text", s: " " }, { t: "bold", s: "now" },
    ]);
  });
});
