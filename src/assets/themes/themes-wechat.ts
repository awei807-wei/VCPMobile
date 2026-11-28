export const meta = {
  fileName: "themes-wechat.css",
  name: "微信摸鱼",
  layoutVariant: "wechat" as const,
  defaultMode: "light" as const,
};

export const variables = {
  light: {
    "--chat-wallpaper-light": "none",
    "--primary-bg": "#EDEDED",
    "--secondary-bg": "#F7F7F7",
    "--tertiary-bg": "#E5E5E5",
    "--accent-bg": "#DEDEDE",
    "--border-color": "#DCDCDC",
    "--input-bg": "#FFFFFF",
    "--panel-bg-light": "#F7F7F7",
    "--primary-text": "#111111",
    "--secondary-text": "#7F7F7F",
    "--highlight-text": "#07C160",
    "--text-on-accent": "#FFFFFF",
    "--placeholder-text": "#B2B2B2",
    "--quoted-text": "#576B95",
    "--user-text": "#111111",
    "--agent-text": "#111111",
    "--user-bubble-bg": "#95EC69",
    "--assistant-bubble-bg": "#FFFFFF",
    "--button-bg": "#07C160",
    "--button-hover-bg": "#06AD56",
    "--danger-color": "#FA5151",
    "--danger-hover-bg": "#E04444",
    "--success-color": "#07C160",
    "--notification-bg": "#FFFFFF",
    "--notification-header-bg": "#F7F7F7",
    "--notification-border": "#E5E5E5",
    "--tool-bubble-bg": "#FFFFFF",
    "--tool-bubble-border": "#E5E5E5",
    "--scrollbar-track": "transparent",
    "--scrollbar-thumb": "rgba(0, 0, 0, 0.16)",
    "--scrollbar-thumb-hover": "rgba(0, 0, 0, 0.28)",
    "--shimmer-color-transparent": "rgba(17, 17, 17, 0.04)",
    "--shimmer-color-highlight": "rgba(17, 17, 17, 0.09)",
    "--panel-bg": "var(--panel-bg-light)",
  },
  dark: {
    "--chat-wallpaper-dark": "none",
    "--primary-bg": "#111111",
    "--secondary-bg": "#191919",
    "--tertiary-bg": "#1E1E1E",
    "--accent-bg": "#2A2A2A",
    "--border-color": "#2C2C2C",
    "--input-bg": "#262626",
    "--panel-bg-dark": "#191919",
    "--primary-text": "#D9D9D9",
    "--secondary-text": "#7D7D7D",
    "--highlight-text": "#07C160",
    "--text-on-accent": "#FFFFFF",
    "--placeholder-text": "#5C5C5C",
    "--quoted-text": "#7D90A9",
    "--user-text": "#0A1F12",
    "--agent-text": "#D9D9D9",
    "--user-bubble-bg": "#3EB575",
    "--assistant-bubble-bg": "#262626",
    "--button-bg": "#07C160",
    "--button-hover-bg": "#06AD56",
    "--danger-color": "#FA5151",
    "--danger-hover-bg": "#E04444",
    "--success-color": "#07C160",
    "--notification-bg": "#262626",
    "--notification-header-bg": "#1F1F1F",
    "--notification-border": "#303030",
    "--tool-bubble-bg": "#262626",
    "--tool-bubble-border": "#333333",
    "--scrollbar-track": "transparent",
    "--scrollbar-thumb": "rgba(255, 255, 255, 0.18)",
    "--scrollbar-thumb-hover": "rgba(255, 255, 255, 0.3)",
    "--shimmer-color-transparent": "rgba(217, 217, 217, 0.06)",
    "--shimmer-color-highlight": "rgba(217, 217, 217, 0.14)",
    "--panel-text-shadow": "none",
    "--panel-bg": "var(--panel-bg-dark)",
  },
};

// 所有覆盖都锚定在 `:root[data-vcp-layout="wechat"]` 下：
// 切回任意普通主题时 theme.ts 会把 layoutVariant 写回 "default" 并整体替换
// `#vcp-custom-theme` 的内容，因此不会留下任何微信残留样式。
export const extraCss = `
/* ---------- 1. 底色透传与分割线 ---------- */

/* 无壁纸主题：由背景层直接铺主题底色，避免透出 html/body 的 #000 */
:root[data-vcp-layout="wechat"] .vcp-background-layer {
  background-color: var(--primary-bg) !important;
  background-image: none !important;
}

:root[data-vcp-layout="wechat"] .vcp-background-overlay {
  background-color: transparent !important;
}

:root[data-vcp-layout="wechat"] .vcp-header-fixed {
  background-color: var(--secondary-bg) !important;
  border-bottom: 1px solid var(--border-color) !important;
}

:root[data-vcp-layout="wechat"] .vcp-input-footer {
  background-color: var(--secondary-bg) !important;
  border-top: 1px solid var(--border-color) !important;
}

:root[data-vcp-layout="wechat"] .vcp-drawer {
  background-color: var(--secondary-bg) !important;
}

:root[data-vcp-layout="wechat"] .vcp-drawer-left {
  border-right: 1px solid var(--border-color) !important;
}

:root[data-vcp-layout="wechat"] .vcp-drawer .glass-panel,
:root[data-vcp-layout="wechat"] .vcp-drawer .glass-panel-active {
  box-shadow: none !important;
}

/* ---------- 2. 气泡：去圆角、去阴影、去流光、加小尖角 ---------- */

:root[data-vcp-layout="wechat"] .message-bubble {
  border-radius: 6px !important;
  border: none !important;
  box-shadow: none !important;
}

:root[data-vcp-layout="wechat"] .vcp-bubble-user,
:root[data-vcp-layout="wechat"] .vcp-bubble-agent {
  border-radius: 6px !important;
}

/* 原生阴影伪元素与流式跑马灯边框一起关掉 */
:root[data-vcp-layout="wechat"] .vcp-bubble-container::after,
:root[data-vcp-layout="wechat"] .streaming .vcp-bubble-container::before {
  display: none !important;
}

/* 微信气泡小尖角：由 ChatBubble 内的 .vcp-wechat-tail 元素绘制，
   不复用被禁用的伪元素，也不会被子容器 overflow 裁切 */
:root[data-vcp-layout="wechat"] .vcp-wechat-tail {
  position: absolute;
  top: 13px;
  width: 8px;
  height: 8px;
  border-radius: 1px;
  transform: rotate(45deg);
  pointer-events: none;
}

:root[data-vcp-layout="wechat"] .vcp-wechat-tail.is-user {
  right: -4px;
  background: var(--user-bubble-bg);
}

:root[data-vcp-layout="wechat"] .vcp-wechat-tail.is-agent {
  left: -4px;
  background: var(--assistant-bubble-bg);
}

/* ---------- 3. 工具卡片 / 思考块 / 日记块降噪 ---------- */

:root[data-vcp-layout="wechat"] .vcp-tool-block,
:root[data-vcp-layout="wechat"] .vcp-tool-call-summary-bubble,
:root[data-vcp-layout="wechat"] .vcp-thought-block,
:root[data-vcp-layout="wechat"] .maid-diary-bubble,
:root[data-vcp-layout="wechat"] .maid-diary-update-bubble {
  border: 1px solid var(--border-color) !important;
  border-radius: 6px !important;
  background: var(--tool-bubble-bg) !important;
  box-shadow: none !important;
  /* 左侧强调条收敛成浅灰，卡片看起来像微信里的文件/系统通知条 */
  --vcp-chat-tool-accent: var(--border-color) !important;
}

:root[data-vcp-layout="wechat"] .vcp-tool-block.is-tool-use,
:root[data-vcp-layout="wechat"] .vcp-tool-block.is-tool-result {
  background: var(--tool-bubble-bg) !important;
}

:root[data-vcp-layout="wechat"] .vcp-tool-block.is-tool-use .tool-label,
:root[data-vcp-layout="wechat"] .vcp-tool-block.is-tool-use .tool-name,
:root[data-vcp-layout="wechat"] .vcp-tool-block.is-tool-result .tool-label,
:root[data-vcp-layout="wechat"] .vcp-tool-block.is-tool-result .tool-name {
  color: var(--primary-text) !important;
}

:root[data-vcp-layout="wechat"] .vcp-tool-block.is-tool-use::after {
  content: none !important;
  display: none !important;
}
`;
