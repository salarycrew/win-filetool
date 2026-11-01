// 시작점. 개발 중 브라우저 미리보기(Tauri 없음)일 때만 가짜 백엔드(mock.ts)를 먼저 올린다 — 배포 빌드에는 빠짐.
(async () => {
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) await import("./mock");
  await import("./main");
})();
