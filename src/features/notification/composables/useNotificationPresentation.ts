import { computed } from "vue";
import {
  Info,
  CheckCircle,
  AlertTriangle,
  X,
  Cpu,
  User,
} from "lucide-vue-next";
import type { VcpNotification } from "../../../core/stores/notification";

export function useNotificationPresentation() {
  const iconMap = computed<Record<string, any>>(() => ({
    success: CheckCircle,
    warning: AlertTriangle,
    error: X,
    tool: Cpu,
    agent: User,
    info: Info,
  }));

  const colorMap = {
    success: "text-[var(--success-color)]",
    warning: "text-[var(--highlight-text)]",
    error: "text-[var(--danger-color)]",
    tool: "text-[var(--notification-border)]",
    agent: "text-[var(--highlight-text)]",
    info: "text-[var(--highlight-text)]",
  } as const;

  const getIcon = (type: VcpNotification["type"]) =>
    (iconMap.value as any)[type] ?? Info;

  const getTypeColor = (type: VcpNotification["type"]) =>
    (colorMap as any)[type] ?? colorMap.info;

  const getActionButtonClass = (action: { label: string; color: string }) => {
    const isGreen =
      action.label === "允许" ||
      action.label === "Approve" ||
      action.color?.includes("green");
    const isRed =
      action.label === "拒绝" ||
      action.label === "Deny" ||
      action.color?.includes("red");
    const toneClass =
      action.color ||
      (isGreen
        ? "bg-[var(--success-color)]"
        : isRed
          ? "bg-[var(--danger-color)]"
          : "bg-[var(--highlight-text)]");

    return [
      toneClass,
      "px-2.5 py-1 text-[9.5px] rounded-md text-white font-medium",
      "hover:opacity-90 active:scale-95",
      "transition-all duration-100",
    ];
  };

  return {
    getIcon,
    getTypeColor,
    getActionButtonClass,
  };
}
