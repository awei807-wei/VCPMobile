/** 话题列表中携带所有者复合身份的展示模型。 */
export interface Topic {
  id: string;
  ownerId: string;
  ownerType: "agent" | "group";
  name: string;
  createdAt: number;
  locked?: boolean;
  unread?: boolean;
  unreadCount?: number;
  msgCount?: number;
}
