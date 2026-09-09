export interface GroupConfig {
  id: string;
  name: string;
  avatar?: string;
  avatarCalculatedColor?: string;
  members: string[];
  mode: string;
  memberTags: Record<string, string>;
  groupPrompt: string;
  invitePrompt: string;
  useUnifiedModel: boolean;
  unifiedModel?: string;
  tagMatchMode: string;
}

export interface Agent {
  id: string;
  name: string;
  avatar?: string;
}
