export interface AgentConfig {
  id: string;
  name: string;
  avatar?: string;
  avatarCalculatedColor?: string;
  mobileSystemPrompt?: string;
  model: string;
  temperature: number;
  contextTokenLimit: number;
  maxOutputTokens: number;
  streamOutput: boolean;
  useTemperature: boolean;
}
