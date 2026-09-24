// @vitest-environment happy-dom

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type {
  AstMutation,
  InlineNode,
  MarkdownNode,
} from "../../../core/types/chat";
import {
  clearHtmlCache,
  renderMarkdownNodes,
} from "../../../core/utils/astRenderer";
import {
  applyFrame,
  cleanupRegistry,
  rebuildSnapshot,
} from "../../../core/utils/astExecutor";

const activeMessageIds = new Set<string>();

function paragraph(children: InlineNode[]): MarkdownNode[] {
  return [{ type: "paragraph", children }];
}

function quote(children: InlineNode[]): InlineNode {
  return { type: "vcp_custom", kind: "quote", children };
}

function text(value: string): InlineNode {
  return { type: "text", value };
}

function createSandbox(messageId: string, nodes: MarkdownNode[]): HTMLElement {
  const sandbox = document.createElement("div");
  activeMessageIds.add(messageId);
  rebuildSnapshot(nodes, messageId, sandbox);
  return sandbox;
}

beforeEach(() => {
  clearHtmlCache();
});

afterEach(() => {
  for (const messageId of activeMessageIds) {
    cleanupRegistry(messageId);
  }
  activeMessageIds.clear();
});

describe("AST 自定义节点静态渲染", () => {
  it("显示 ASCII 双引号、前后正文、空引号和中文引号", () => {
    expect(
      renderMarkdownNodes(
        paragraph([quote([text('"'), text("hello"), text('"')])]),
        "static-ascii-only",
      ),
    ).toBe('<p><span class="highlighted-quote">&quot;hello&quot;</span></p>');

    expect(
      renderMarkdownNodes(
        paragraph([
          text("前面 "),
          quote([text('"'), text("hello"), text('"')]),
          text(" 后面"),
        ]),
        "static-ascii-surrounded",
      ),
    ).toBe(
      '<p>前面 <span class="highlighted-quote">&quot;hello&quot;</span> 后面</p>',
    );

    expect(
      renderMarkdownNodes(
        paragraph([quote([text('"'), text('"')])]),
        "static-empty-quote",
      ),
    ).toBe('<p><span class="highlighted-quote">&quot;&quot;</span></p>');

    expect(
      renderMarkdownNodes(
        paragraph([quote([text("“"), text("中文引号"), text("”")])]),
        "static-chinese-quote",
      ),
    ).toBe('<p><span class="highlighted-quote">“中文引号”</span></p>');
  });

  it("递归渲染嵌套引号并安全转义正文", () => {
    const nestedQuote = quote([
      text('"'),
      text("a "),
      quote([text('"'), text("quoted"), text('"')]),
      text(" <text> value"),
      text('"'),
    ]);

    expect(
      renderMarkdownNodes(paragraph([nestedQuote]), "static-nested-quote"),
    ).toBe(
      '<p><span class="highlighted-quote">&quot;a <span class="highlighted-quote">&quot;quoted&quot;</span> &lt;text&gt; value&quot;</span></p>',
    );
  });

  it("渲染 highlight、alert，并让未知 kind 保留可见内容", () => {
    const html = renderMarkdownNodes(
      paragraph([
        {
          type: "vcp_custom",
          kind: "highlight",
          value: "@tag <unsafe>",
        },
        text(" "),
        {
          type: "vcp_custom",
          kind: "alert",
          value: "@!alert & warning",
        },
        text(" "),
        {
          type: "vcp_custom",
          kind: "future-kind",
          children: [text("未来 <内容>")],
        },
        text(" "),
        {
          type: "vcp_custom",
          kind: "value-only-future-kind",
          value: "<fallback>",
        },
      ]),
      "static-custom-kinds",
    );

    expect(html).toBe(
      '<p><span class="highlighted-tag">@tag &lt;unsafe&gt;</span> <span class="highlighted-alert-tag">@!alert &amp; warning</span> 未来 &lt;内容&gt; &lt;fallback&gt;</p>',
    );
  });

  it("继续兼容旧版专用节点协议", () => {
    const html = renderMarkdownNodes(
      paragraph([
        {
          type: "quoted_text",
          children: [text('"'), text("旧引号"), text('"')],
        },
        text(" "),
        { type: "highlight_tag", value: "@legacy" },
        text(" "),
        { type: "alert_tag", value: "@!legacy" },
      ]),
      "static-legacy-custom-nodes",
    );

    expect(html).toBe(
      '<p><span class="highlighted-quote">&quot;旧引号&quot;</span> <span class="highlighted-tag">@legacy</span> <span class="highlighted-alert-tag">@!legacy</span></p>',
    );
  });
});

describe("AST 自定义节点流式 DOM 渲染", () => {
  it("从完整快照重建 vcp_custom 节点且未知 kind 不丢字", () => {
    const sandbox = createSandbox(
      "stream-snapshot-custom-nodes",
      paragraph([
        quote([text('"'), text("hello"), text('"')]),
        text(" "),
        { type: "vcp_custom", kind: "highlight", value: "@tag" },
        text(" "),
        { type: "vcp_custom", kind: "alert", value: "@!alert" },
        text(" "),
        {
          type: "vcp_custom",
          kind: "future-kind",
          children: [text("未来内容")],
        },
      ]),
    );

    expect(sandbox.textContent).toBe('"hello" @tag @!alert 未来内容');
    expect(sandbox.querySelector(".highlighted-quote")?.textContent).toBe(
      '"hello"',
    );
    expect(sandbox.querySelector(".highlighted-tag")?.textContent).toBe("@tag");
    expect(sandbox.querySelector(".highlighted-alert-tag")?.textContent).toBe(
      "@!alert",
    );
  });

  it("引号跨流式 chunk 闭合后通过 replace_inline 保持可见", () => {
    const messageId = "stream-ascii-quote-chunks";
    const sandbox = createSandbox(
      messageId,
      paragraph([text("前面 "), text('"')]),
    );

    const appendResult = applyFrame(
      [{ op: "append", id: "t0.i1", chunk: "hidden" }],
      messageId,
      sandbox,
    );
    expect(appendResult).toMatchObject({ ok: true, applied: 1 });
    expect(sandbox.textContent).toBe('前面 "hidden');

    const closeQuoteMutations: AstMutation[] = [
      {
        op: "replace_inline",
        id: "t0.i1",
        node: quote([text('"'), text("hidden"), text('"')]),
      },
      {
        op: "add_inline",
        id: "t0.i2",
        parent: "t0",
        node: text(" 后面"),
      },
    ];
    const closeResult = applyFrame(closeQuoteMutations, messageId, sandbox);

    expect(closeResult).toMatchObject({ ok: true, applied: 2 });
    expect(sandbox.textContent).toBe('前面 "hidden" 后面');
    expect(sandbox.querySelector(".highlighted-quote")?.textContent).toBe(
      '"hidden"',
    );

    const updateResult = applyFrame(
      [
        {
          op: "replace_inline",
          id: "t0.i1",
          node: quote([text('"'), text("updated"), text('"')]),
        },
      ],
      messageId,
      sandbox,
    );

    expect(updateResult).toMatchObject({ ok: true, applied: 1 });
    expect(sandbox.textContent).toBe('前面 "updated" 后面');
    expect(sandbox.querySelector(".highlighted-quote")?.textContent).toBe(
      '"updated"',
    );
  });

  it("原地更新叶子型 vcp_custom，并允许替换为未知 kind", () => {
    const messageId = "stream-custom-node-mutations";
    const sandbox = createSandbox(
      messageId,
      paragraph([{ type: "vcp_custom", kind: "highlight", value: "@old" }]),
    );
    const originalElement = sandbox.querySelector(".highlighted-tag");

    const updateResult = applyFrame(
      [
        {
          op: "replace_inline",
          id: "t0.i0",
          node: { type: "vcp_custom", kind: "highlight", value: "@new" },
        },
      ],
      messageId,
      sandbox,
    );

    expect(updateResult).toMatchObject({ ok: true, applied: 1 });
    expect(sandbox.querySelector(".highlighted-tag")).toBe(originalElement);
    expect(sandbox.textContent).toBe("@new");

    const unknownKindResult = applyFrame(
      [
        {
          op: "replace_inline",
          id: "t0.i0",
          node: {
            type: "vcp_custom",
            kind: "future-kind",
            children: [text("不可丢失")],
          },
        },
      ],
      messageId,
      sandbox,
    );

    expect(unknownKindResult).toMatchObject({ ok: true, applied: 1 });
    expect(sandbox.textContent).toBe("不可丢失");
  });

  it("流式快照继续兼容旧版专用节点", () => {
    const sandbox = createSandbox(
      "stream-legacy-custom-nodes",
      paragraph([
        {
          type: "quoted_text",
          children: [text('"'), text("legacy"), text('"')],
        },
        text(" "),
        { type: "highlight_tag", value: "@legacy" },
        text(" "),
        { type: "alert_tag", value: "@!legacy" },
      ]),
    );

    expect(sandbox.textContent).toBe('"legacy" @legacy @!legacy');
    expect(sandbox.querySelector(".highlighted-quote")).not.toBeNull();
    expect(sandbox.querySelector(".highlighted-tag")).not.toBeNull();
    expect(sandbox.querySelector(".highlighted-alert-tag")).not.toBeNull();
  });
});
