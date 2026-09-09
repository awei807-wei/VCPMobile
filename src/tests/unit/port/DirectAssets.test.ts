import { describe, expect, it } from "vitest";
import { extractMentionedMemberIds, scanMentionHits, splitMentionSegments } from "../../../core/utils/mention";
import { nameHue, nameInitial } from "../../../core/utils/nameHue";
import {
  levelOf,
  splitByKeyword,
  splitLogChunk,
  stripAnsi,
} from "../../../features/logcenter/logText";

describe("T01 原样工具资产", () => {
  it("nameInitial 取第一个非空白字符并支持 Unicode 代理对", () => {
    expect(nameInitial("  张三")).toBe("张");
    expect(nameInitial("alice")).toBe("A");
    expect(nameInitial("  ")).toBe("?");
    expect(nameInitial("𠮷野")).toBe("𠮷");
  });

  it("nameHue 对同一个名字稳定且落在色相范围内", () => {
    const first = nameHue("Alice");
    expect(nameHue("Alice")).toBe(first);
    expect(first).toBeGreaterThanOrEqual(0);
    expect(first).toBeLessThanOrEqual(359);
  });

  it("提取成员提及时按首次出现顺序并去重", () => {
    expect(
      extractMentionedMemberIds("@Alice ＠Bob @Alice", [
        { id: "alice-id", name: "Alice" },
        { id: "bob-id", name: "Bob" },
      ]),
    ).toEqual(["alice-id", "bob-id"]);
  });

  it("同一位置的重叠名字优先匹配最长名字", () => {
    expect(scanMentionHits("@Abc", ["Ab", "Abc"])).toEqual([
      { start: 0, end: 4, name: "Abc" },
    ]);
  });

  it("提及分段重拼后逐字符保留原文", () => {
    const content = "前 @Alice 和＠Bob，尾";
    const segments = splitMentionSegments(content, ["Alice", "Bob"]);
    expect(segments.map((segment) => segment.text).join("")).toBe(content);
    expect(segments.filter((segment) => segment.mention).map((segment) => segment.text)).toEqual([
      "@Alice",
      "＠Bob",
    ]);
  });

  it("日志分块拼接 carry 并保留末尾半行", () => {
    expect(splitLogChunk("行\n尾", "半")).toEqual({
      lines: ["半行", "尾"],
      trailing: "尾",
    });
  });

  it("日志清理会移除 ANSI 并识别 ERROR 级别", () => {
    const line = "\u001b[31m[ERROR] x\u001b[0m";
    expect(stripAnsi(line)).toBe("[ERROR] x");
    expect(levelOf(stripAnsi(line))).toBe("error");
  });

  it("日志关键词命中段保留原始大小写且可重拼", () => {
    const segments = splitByKeyword("aBc", "b");
    expect(segments.find((segment) => segment.hit)?.text).toBe("B");
    expect(segments.map((segment) => segment.text).join("")).toBe("aBc");
  });
});
