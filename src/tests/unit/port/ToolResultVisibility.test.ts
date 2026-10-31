import { describe, expect, it } from "vitest";
import {
  collectClosedToolResultRanges,
  fnv1aHashHex,
  isToolResultHidden,
  setToolResultHidden,
  stripHiddenToolResults,
  toolResultBody,
  toolResultBodyHash,
} from "../../../core/utils/toolResultVisibility";

const visibleBlock = () =>
  "[[VCP调用结果信息汇总:\n- 工具名称: WebSearch\n- 执行状态: ✅ 成功\n- 返回内容: 结果\nVCP调用结果结束]]";

describe("T02 插件回调（工具结果）可见性", () => {
  it("可见块默认注入上下文", () => {
    expect(isToolResultHidden(visibleBlock())).toBe(false);
  });

  it("隐藏后写入 [HIDDEN]，再次恢复与原文字节一致", () => {
    const original = visibleBlock();
    const hidden = setToolResultHidden(original, true);
    expect(hidden.startsWith("[[VCP调用结果信息汇总: [HIDDEN]")).toBe(true);
    expect(isToolResultHidden(hidden)).toBe(true);

    const restored = setToolResultHidden(hidden, false);
    expect(restored).toBe(original);
  });

  it("元数据行形式也被识别为隐藏，并在恢复时清除该行", () => {
    const raw =
      "[[VCP调用结果信息汇总:\n- 注入上下文: 隐藏\n- 工具名称: WebSearch\nVCP调用结果结束]]";
    expect(isToolResultHidden(raw)).toBe(true);

    const restored = setToolResultHidden(raw, false);
    expect(isToolResultHidden(restored)).toBe(false);
    expect(restored).toContain("- 工具名称: WebSearch");
    expect(restored).not.toContain("注入上下文");
  });

  it("剔除隐藏块时保留可见块与前后文", () => {
    const hidden = setToolResultHidden(visibleBlock(), true);
    const text = `前文\n\n${hidden}\n\n${visibleBlock()}后文`;
    const stripped = stripHiddenToolResults(text);

    expect(stripped).not.toContain("[HIDDEN]");
    expect(stripped).toContain("- 工具名称: WebSearch");
    expect(stripped.startsWith("前文")).toBe(true);
    expect(stripped.endsWith("后文")).toBe(true);
  });

  it("嵌套的起止标记不会截断外层块", () => {
    const text =
      "[[VCP调用结果信息汇总:\n- 工具名称: Reader\n- 返回内容: 源码里有 [[VCP调用结果信息汇总: 内层 VCP调用结果结束]] 字样\nVCP调用结果结束]]尾部";
    const ranges = collectClosedToolResultRanges(text);

    expect(ranges).toHaveLength(1);
    expect(text.slice(ranges[0].end)).toBe("尾部");
  });

  it("未闭合的块不会被当作可隐藏区间", () => {
    const text = "[[VCP调用结果信息汇总:\n- 工具名称: WebSearch\n";
    expect(collectClosedToolResultRanges(text)).toHaveLength(0);
  });

  it("FNV-1a 哈希与 Rust 端逐字节一致", () => {
    expect(fnv1aHashHex("")).toBe("cbf29ce484222325");
    expect(fnv1aHashHex("a")).toBe("af63dc4c8601ec8c");
    expect(fnv1aHashHex("abc")).toBe("e71fa2190541574b");
    expect(toolResultBodyHash("\n  正文  ")).toBe(fnv1aHashHex("正文"));
  });

  it("正文哈希与起始/结束标记无关，便于前端回指同一块", () => {
    const body = "\n- 工具名称: WebSearch\n- 执行状态: ✅ 成功\n";
    const raw = `[[VCP调用结果信息汇总:${body}VCP调用结果结束]]`;
    expect(toolResultBodyHash(toolResultBody(raw))).toBe(
      toolResultBodyHash(body),
    );
  });
});
