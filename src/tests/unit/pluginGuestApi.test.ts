import { describe, expect, it } from "vitest";
import { startStreamService, stopStreamService } from "../../../src-tauri/plugins/vcp-mobile/guest-js/index";
import { invokeMock, mockInvoke } from "../mocks/tauri";

const identity = {
  ownerType: "group",
  ownerId: "group-1",
  topicId: "topic-1",
  messageId: "message-1",
};

describe("vcp-mobile guest-js 流式服务契约", () => {
  it("start 原样返回 number 或 null", async () => {
    mockInvoke("plugin:vcp-mobile|start_streaming_service", () => 17);
    await expect(startStreamService("agent-label", identity)).resolves.toBe(17);

    mockInvoke("plugin:vcp-mobile|start_streaming_service", () => null);
    await expect(startStreamService("agent-label", identity)).resolves.toBeNull();
  });

  it("stop 原样传递 agentName、identity 和 expectedGeneration", async () => {
    mockInvoke("plugin:vcp-mobile|stop_streaming_service", () => undefined);
    await stopStreamService("agent-label", identity, 17);

    expect(invokeMock).toHaveBeenCalledWith(
      "plugin:vcp-mobile|stop_streaming_service",
      { agentName: "agent-label", identity, expectedGeneration: 17 },
    );
  });
});
