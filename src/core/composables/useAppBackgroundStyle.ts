import { computed } from "vue";
import type { useThemeStore } from "../stores/theme";

type ThemeStore = ReturnType<typeof useThemeStore>;

export function useAppBackgroundStyle(themeStore: ThemeStore) {
  return computed(() => {
    const themeInfo =
      themeStore.currentThemeInfo ||
      themeStore.availableThemes.find(
        (theme) => theme.fileName === themeStore.currentTheme,
      );
    if (!themeInfo) return {};

    const isLight = !themeStore.isDarkResolved;
    let rawValue = isLight
      ? themeInfo.variables.light?.["--chat-wallpaper-light"]
      : themeInfo.variables.dark?.["--chat-wallpaper-dark"];
    if (!rawValue || rawValue === "none") {
      rawValue = isLight
        ? themeInfo.variables.dark?.["--chat-wallpaper-dark"]
        : themeInfo.variables.light?.["--chat-wallpaper-light"];
    }
    if (!rawValue || rawValue === "none") return {};

    const match = rawValue.match(/url\(['"]?(.*?)['"]?\)/);
    let filename = match ? match[1] : rawValue;
    filename = filename.replace(/^.*[\\/]/, "").replace(/[\"']/g, "");
    filename = filename.split(".")[0] + ".webp";
    return { backgroundImage: `url("/wallpaper/${filename}")` };
  });
}
