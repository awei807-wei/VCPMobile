import { clampUnicode } from "./searchValidation";

export interface HighlightSegment {
  text: string;
  highlighted: boolean;
}

interface TextRange {
  start: number;
  end: number;
}

/** 将纯文本摘要拆成普通文本节点，避免把搜索内容重新解释为 HTML。 */
export function splitHighlight(
  text: string,
  query: string,
): HighlightSegment[] {
  const boundedText = clampUnicode(text, 240);
  if (!boundedText || !query.trim())
    return [{ text: boundedText, highlighted: false }];
  const ranges = collectRanges(boundedText, query);
  if (ranges.length === 0) return [{ text: boundedText, highlighted: false }];
  return buildSegments(boundedText, ranges);
}

function collectRanges(text: string, query: string): TextRange[] {
  const terms = Array.from(new Set(query.trim().split(/\s+/).filter(Boolean)));
  const ranges: TextRange[] = [];
  for (const term of terms) {
    collectTermRanges(text, term, ranges);
  }
  return mergeRanges(ranges);
}

function collectTermRanges(
  text: string,
  term: string,
  ranges: TextRange[],
): void {
  const exactTerm = term.normalize("NFC");
  if (collectMatches(text, exactTerm, ranges)) return;
  if (collectNormalizedMatches(text, exactTerm, ranges)) return;
  const foldedText = text.toLocaleLowerCase();
  const foldedTerm = exactTerm.toLocaleLowerCase();
  if (foldedTerm) collectMatches(foldedText, foldedTerm, ranges);
}

function collectNormalizedMatches(
  text: string,
  term: string,
  ranges: TextRange[],
): boolean {
  const normalized = normalizeWithBoundaries(text);
  const matches: TextRange[] = [];
  if (!collectMatches(normalized.value, term, matches)) return false;
  for (const match of matches) {
    const start = normalized.boundaries[match.start] ?? 0;
    const end = normalized.boundaries[match.end] ?? text.length;
    ranges.push({ start, end });
  }
  return true;
}

function normalizeWithBoundaries(text: string): {
  value: string;
  boundaries: number[];
} {
  let value = "";
  const boundaries = [0];
  let index = 0;
  while (index < text.length) {
    const clusterStart = index;
    index += codePointWidth(text, index);
    while (index < text.length && /\p{M}/u.test(text[index] ?? ""))
      index += codePointWidth(text, index);
    value += text.slice(clusterStart, index).normalize("NFC");
    const normalizedLength = value.length;
    while (boundaries.length < normalizedLength + 1) boundaries.push(index);
  }
  return { value, boundaries };
}

function codePointWidth(text: string, index: number): number {
  return (text.codePointAt(index) ?? 0) > 0xffff ? 2 : 1;
}

function collectMatches(
  text: string,
  term: string,
  ranges: TextRange[],
): boolean {
  if (!term) return false;
  let foundAny = false;
  let start = 0;
  while (start < text.length) {
    const found = text.indexOf(term, start);
    if (found < 0) return foundAny;
    ranges.push({ start: found, end: found + term.length });
    foundAny = true;
    start = found + term.length;
  }
  return foundAny;
}

function mergeRanges(ranges: TextRange[]): TextRange[] {
  return ranges
    .sort((left, right) => left.start - right.start || left.end - right.end)
    .reduce<TextRange[]>((merged, range) => {
      const previous = merged[merged.length - 1];
      if (previous && range.start <= previous.end) {
        previous.end = Math.max(previous.end, range.end);
      } else {
        merged.push({ ...range });
      }
      return merged;
    }, []);
}

function buildSegments(text: string, ranges: TextRange[]): HighlightSegment[] {
  const segments: HighlightSegment[] = [];
  let cursor = 0;
  for (const range of ranges) {
    if (range.start > cursor)
      segments.push({
        text: text.slice(cursor, range.start),
        highlighted: false,
      });
    segments.push({
      text: text.slice(range.start, range.end),
      highlighted: true,
    });
    cursor = range.end;
  }
  if (cursor < text.length)
    segments.push({ text: text.slice(cursor), highlighted: false });
  return segments;
}
