import { describe, expect, it } from "vitest";
import { splitHighlight } from "../../../features/globalsearch/safeHighlight";

describe("安全高亮拆分", () => {
  it("按字面量高亮标点且不解释为正则表达式", () => {
    const segments = splitHighlight("a.b [c] (d) <safe>", "a.b [c]");
    expect(
      segments
        .filter((segment) => segment.highlighted)
        .map((segment) => segment.text),
    ).toEqual(["a.b", "[c]"]);
    expect(segments.every((segment) => !segment.text.includes("<script"))).toBe(
      true,
    );
  });

  it("保留表情与组合字符的原始文本边界", () => {
    const segments = splitHighlight("前缀 👩‍💻 café 后缀", "👩‍💻 café");
    expect(
      segments
        .filter((segment) => segment.highlighted)
        .map((segment) => segment.text),
    ).toEqual(["👩‍💻", "café"]);
  });

  it("搜索词为空时返回普通文本片段", () => {
    expect(splitHighlight("plain text", " ")).toEqual([
      { text: "plain text", highlighted: false },
    ]);
  });
});
