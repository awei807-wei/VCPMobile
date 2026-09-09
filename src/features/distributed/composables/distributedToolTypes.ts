export interface RawInvocationCommand {
  commandIdentifier?: unknown;
  command_identifier?: unknown;
  description?: unknown;
  example?: unknown;
}

export interface InvocationCommand {
  commandIdentifier: string | null;
  description?: string;
  example?: string;
  missingIdentifier?: boolean;
}

export interface PluginItem {
  id: string;
  name: string;
  englishName: string;
  description: string;
  type: "oneshot" | "interactive" | "streaming";
  placeholder?: string;
  invocationCommands: InvocationCommand[];
  icon: string;
  communication?: {
    mode?: "Ipc" | "Mock";
    payload?: { command: string; args?: unknown };
  };
  enabled: boolean;
  requiresRoot: boolean;
}

export interface PlaceholderItem {
  macro: string;
  name: string;
  description: string;
  example: string;
}

export interface FoldBlock {
  threshold: number;
  desc: string;
  content: string;
}
