/**
 * 工具结果（插件回调）可见性工具
 *
 * VCP 后端一次性注入的 `[[VCP调用结果信息汇总: ... VCP调用结果结束]]` 块默认注入
 * AI 上下文。用户可以在气泡上手动把某个工具结果标记为“隐藏”（不注入上下文）：
 * 标记直接写回原始消息内容，消息本身完整保留，可以随时恢复。
 *
 * 与 Rust 端 `vcp_modules/chat/tool_result_visibility.rs` 的判定、写入与哈希规则
 * 保持逐字节一致，两端共同保证“UI 状态”和“上下文剔除”永远指向同一个块。
 */

export const TOOL_RESULT_START_MARKER = "[[VCP调用结果信息汇总:";
export const TOOL_RESULT_END_MARKER = "VCP调用结果结束]]";
export const HIDDEN_TOKEN = "[HIDDEN]";

export interface ToolResultRange {
  start: number;
  end: number;
  closed: boolean;
}

const HIDDEN_TOKEN_PREFIX = /^[ \t]*(?:\[HIDDEN\]|<!--HIDDEN-->|\(HIDDEN\))/i;
const HIDDEN_METADATA =
  /(?:^|\r?\n)[ \t]*-[ \t]*(?:注入上下文|上下文注入|上下文状态|Context):\s*(?:隐藏|hidden|false|否)/im;

/**
 * 嵌套感知地查找工具结果的结束位置。
 * 工具结果内部可能包含字面量起止标记（例如 AI 读取了渲染器源码），
 * 采用深度计数：起始标记 +1、结束标记 -1，深度回到 0 时外层块才闭合。
 */
export function findToolResultEnd(text: string, startIndex: number): number {
  let depth = 1;
  let cursor = startIndex + TOOL_RESULT_START_MARKER.length;
  let nextStart = text.indexOf(TOOL_RESULT_START_MARKER, cursor);

  while (cursor <= text.length) {
    const nextEnd = text.indexOf(TOOL_RESULT_END_MARKER, cursor);
    if (nextEnd === -1) return -1;

    if (nextStart !== -1 && nextStart < nextEnd) {
      depth += 1;
      cursor = nextStart + TOOL_RESULT_START_MARKER.length;
      nextStart = text.indexOf(TOOL_RESULT_START_MARKER, cursor);
      continue;
    }

    depth -= 1;
    cursor = nextEnd + TOOL_RESULT_END_MARKER.length;
    if (depth === 0) return cursor;
    if (nextStart !== -1 && nextStart < cursor) {
      nextStart = text.indexOf(TOOL_RESULT_START_MARKER, cursor);
    }
  }

  return -1;
}

/** 收集最外层工具结果区间（按源码顺序、互不重叠；未闭合区间延伸到文末）。 */
export function collectToolResultRanges(text: string): ToolResultRange[] {
  const ranges: ToolResultRange[] = [];
  if (typeof text !== "string" || !text.includes(TOOL_RESULT_START_MARKER)) {
    return ranges;
  }

  let cursor = 0;
  while (cursor < text.length) {
    const start = text.indexOf(TOOL_RESULT_START_MARKER, cursor);
    if (start === -1) break;

    const end = findToolResultEnd(text, start);
    if (end === -1) {
      ranges.push({ start, end: text.length, closed: false });
      break;
    }

    ranges.push({ start, end, closed: true });
    cursor = end;
  }

  return ranges;
}

/** 只保留已闭合的最外层工具结果区间。 */
export function collectClosedToolResultRanges(text: string): ToolResultRange[] {
  return collectToolResultRanges(text).filter((range) => range.closed);
}

/** 去掉可选的起始/结束标记，返回块内部正文。 */
export function toolResultBody(raw: string): string {
  let text = raw.trimStart();
  if (text.startsWith(TOOL_RESULT_START_MARKER)) {
    text = text.slice(TOOL_RESULT_START_MARKER.length);
  }
  const endIndex = text.lastIndexOf(TOOL_RESULT_END_MARKER);
  if (
    endIndex !== -1 &&
    text.slice(endIndex + TOOL_RESULT_END_MARKER.length).trim() === ""
  ) {
    return text.slice(0, endIndex);
  }
  return text;
}

/** 判断一个工具结果块是否被标记为隐藏（不注入上下文）。 */
export function isToolResultHidden(raw: string): boolean {
  if (typeof raw !== "string" || raw.length === 0) return false;
  const body = toolResultBody(raw);
  if (HIDDEN_TOKEN_PREFIX.test(body)) return true;
  // 兼容元数据行形式：只看首部 300 字符
  return HIDDEN_METADATA.test(body.slice(0, 300));
}

/** 去掉正文首部的隐藏标记与元数据行，用于正常解析工具字段。 */
export function stripHiddenMarker(body: string): string {
  const withoutToken = body.replace(HIDDEN_TOKEN_PREFIX, "");
  const trimmed = withoutToken.replace(/^[ \t]+/, "");
  return trimmed.replace(HIDDEN_METADATA, "");
}

/**
 * 设置工具结果块的隐藏状态。
 * 只有首部（前导空白之后）就是起始标记的切片才会被修改，其余原样返回。
 */
export function setToolResultHidden(raw: string, hidden = true): string {
  if (typeof raw !== "string" || raw.length === 0) return raw;
  if (hidden === isToolResultHidden(raw)) return raw;

  const markerPos = raw.indexOf(TOOL_RESULT_START_MARKER);
  if (markerPos === -1) return raw;
  if (raw.slice(0, markerPos).trim() !== "") return raw;

  const bodyStart = markerPos + TOOL_RESULT_START_MARKER.length;
  if (hidden) {
    return raw.slice(0, bodyStart) + " " + HIDDEN_TOKEN + raw.slice(bodyStart);
  }

  return raw.slice(0, bodyStart) + stripHiddenMarker(raw.slice(bodyStart));
}

/** 剥离文本中所有被标记为隐藏的工具结果块，并收拢多余空行。 */
export function stripHiddenToolResults(text: string): string {
  if (typeof text !== "string" || !text.includes(TOOL_RESULT_START_MARKER)) {
    return text;
  }
  const ranges = collectClosedToolResultRanges(text);
  if (ranges.length === 0) return text;

  let out = "";
  let cursor = 0;
  let stripped = false;
  for (const range of ranges) {
    if (isToolResultHidden(text.slice(range.start, range.end))) {
      out += text.slice(cursor, range.start);
      cursor = range.end;
      stripped = true;
    }
  }
  if (!stripped) return text;

  out += text.slice(cursor);
  return out.replace(/\n{3,}/g, "\n\n");
}

const FNV_OFFSET_BASIS = 0xcbf29ce484222325n;
const FNV_PRIME = 0x100000001b3n;
const MASK_64 = 0xffffffffffffffffn;

/** FNV-1a 64 位哈希（十六进制），与 Rust 端 `fnv1a_hash_hex` 逐字节一致。 */
export function fnv1aHashHex(text: string): string {
  let hash = FNV_OFFSET_BASIS;
  const bytes = new TextEncoder().encode(text);
  for (const byte of bytes) {
    hash ^= BigInt(byte);
    hash = (hash * FNV_PRIME) & MASK_64;
  }
  return hash.toString(16).padStart(16, "0");
}

/** 工具结果正文（去首尾空白）的哈希，用于精确定位原始文本中的块。 */
export function toolResultBodyHash(body: string): string {
  return fnv1aHashHex(body.trim());
}
