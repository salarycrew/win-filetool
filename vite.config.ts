import { defineConfig } from "vite";

// Tauri 개발 서버 설정: 고정 포트, Rust 쪽 출력은 화면에서 지우지 않는다.
export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2021", outDir: "dist", emptyOutDir: true },
});
