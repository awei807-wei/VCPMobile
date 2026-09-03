import type { MessageShell } from "../types/chat";

export function createStreamShellFactory(
  assistantStore: {
    agents: Array<{
      id: string;
      name?: string;
      avatarCalculatedColor?: string;
    }>;
  },
  avatarStore: {
    getDominantColor: (
      ownerType: string,
      ownerId: string,
    ) => string | undefined;
  },
): (message: {
  role: string;
  agentId?: string;
  name?: string;
}) => MessageShell {
  return (message) => {
    const empty = "";
    if (message.role === "user") {
      return {
        avatarColor:
          avatarStore.getDominantColor("user", "user_avatar") ||
          "rgb(226,54,56)",
        bubbleBorderColor: empty,
        bubbleBoxShadow: empty,
        displayName: message.name || "User",
        isUser: true,
      };
    }
    const agent = message.agentId
      ? assistantStore.agents.find((item) => item.id === message.agentId)
      : undefined;
    return {
      avatarColor: agent?.avatarCalculatedColor || empty,
      bubbleBorderColor: empty,
      bubbleBoxShadow: empty,
      displayName: message.name || agent?.name || "AI",
      isUser: false,
    };
  };
}
