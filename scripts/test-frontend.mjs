// 화면 코드 단위 시험: tests/frontend/*.test.ts를 rolldown으로 묶어 node:test로 돌린다(브라우저·Tauri 없이).
import { build } from "rolldown";
import { readdirSync, mkdirSync, rmSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join, resolve } from "node:path";

// 화면 코드가 불러올 때 부르는 Tauri 이벤트 대신 가짜 백엔드(mock.ts와 같은 자리)
const BANNER = "globalThis.window ??= { __FT_MOCK__: { invoke: async () => null, listen: () => {}, open: async () => null } };";
const root = resolve(import.meta.dirname, "..");
const dir = join(root, "tests", "frontend");
const out = join(root, "target", "frontend-tests");
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });
const files = readdirSync(dir).filter((f) => f.endsWith(".test.ts"));
const built = [];
for (const f of files) {
  const file = join(out, f.replace(/\.ts$/, ".mjs"));
  await build({ input: join(dir, f), platform: "node", external: [/^node:/], output: { file, format: "esm", banner: BANNER }, logLevel: "warn" });
  built.push(file);
}
const r = spawnSync(process.execPath, ["--test", ...built], { stdio: "inherit" });
process.exit(r.status ?? 1);
