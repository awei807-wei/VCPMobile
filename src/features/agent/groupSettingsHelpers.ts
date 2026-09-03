import type { Agent } from "./groupSettingsTypes";

export function toggleGroupMember(
  members: string[],
  memberTags: Record<string, string>,
  agentId: string,
  agents: Agent[],
) {
  const index = members.indexOf(agentId);
  if (index !== -1) {
    members.splice(index, 1);
    return;
  }

  members.push(agentId);
  if (!memberTags[agentId]) {
    const agent = agents.find((item) => item.id === agentId);
    memberTags[agentId] = agent?.name || agentId;
  }
}
