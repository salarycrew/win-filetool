// 밝게/어둡게: 시스템(Windows 설정 따라감) · 밝게 · 어둡게. 고른 값은 이 PC의 앱 저장소(localStorage)에.
export type ThemePref = "system" | "light" | "dark";
const KEY = "ft.theme";
const mq = window.matchMedia("(prefers-color-scheme: dark)");

export function themePref(): ThemePref {
  const v = localStorage.getItem(KEY);
  return v === "light" || v === "dark" ? v : "system";
}
export function setThemePref(p: ThemePref) {
  localStorage.setItem(KEY, p);
  applyTheme();
}
export function applyTheme() {
  const p = themePref();
  const dark = p === "dark" || (p === "system" && mq.matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
  // 창 제목 표시줄도 같은 색으로(시스템이면 Windows에 맡김)
  if ("__TAURI_INTERNALS__" in window) {
    import("@tauri-apps/api/window")
      .then(({ getCurrentWindow }) => getCurrentWindow().setTheme(p === "system" ? null : p))
      .catch(() => {});
  }
}
export function initTheme() {
  mq.addEventListener("change", () => { if (themePref() === "system") applyTheme(); });
}
