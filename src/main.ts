// 파일 도구 화면(시안 A — Windows 11 설정 앱 결): 왼쪽 메뉴 + 오른쪽 내용 판.
// 0.3.2: 왼쪽 메뉴는 홈 · 더 보기 · 설정 셋. 홈 = 핵심 6개 큰 단추, 나머지 화면은 모두 [더 보기] 안에.
// 화면 모듈은 src/screens/*. 처음 쓰는 사람도 설명 없이 쓸 수 있게 짧은 존댓말로.
import { invoke, listen } from "./api";
import { I } from "./icons";
import { $, $$, el, esc, installTips } from "./ui";
import { Screen, ScreenId, setGo } from "./router";
import { applyTheme, initTheme } from "./theme";
import { convertScreen } from "./screens/convert";
import { pdfEditScreen } from "./screens/pdfedit";
import { organizeScreen } from "./screens/organize";
import { searchScreen } from "./screens/search";
import { watchScreen } from "./screens/watch";
import { settingsScreen } from "./screens/settings";
import { initUpdate } from "./update";
import { aboutScreen } from "./screens/about";
import { homeScreen } from "./screens/home";
import { jobScreen } from "./screens/job";
import { moreScreen, MORE_IDS } from "./screens/more";
import type { JobId } from "./screens/core";

const NAV: Screen[] = [homeScreen, moreScreen];
const BOTTOM: Screen[] = [settingsScreen];
const ALL: Screen[] = [homeScreen, jobScreen, moreScreen, convertScreen, pdfEditScreen, organizeScreen, searchScreen, watchScreen, settingsScreen, aboutScreen];
const IDS = ALL.map((s) => s.id) as string[];
/** 왼쪽 메뉴에서 켜져 보일 항목: 할 일 화면 → 홈, 더 보기 안 화면 → 더 보기 */
const navOf = (id: ScreenId): ScreenId => (id === "job" ? "home" : MORE_IDS.includes(id) ? "more" : id);

const nav = $("#nav")!;
const pane = $("#pane")!;
let current: Screen | null = null;
const inited = new Set<ScreenId>();

function navBtn(s: Screen) {
  return `<button type="button" class="nav-i" data-id="${s.id}" aria-label="${esc(s.title)}" title="${esc(s.title)}">${I(s.icon)}<span>${esc(s.title)}</span></button>`;
}
nav.innerHTML = `${NAV.map(navBtn).join("")}<div class="sp"></div>${BOTTOM.map(navBtn).join("")}`;
nav.addEventListener("click", (e) => {
  const b = (e.target as HTMLElement).closest<HTMLButtonElement>(".nav-i");
  if (b) go(b.dataset.id as ScreenId);
});
// 위·아래 화살표로 메뉴 이동
nav.addEventListener("keydown", (e) => {
  if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
  const btns = $$<HTMLButtonElement>(".nav-i", nav);
  const i = btns.indexOf(document.activeElement as HTMLButtonElement);
  if (i < 0) return;
  btns[(i + (e.key === "ArrowDown" ? 1 : btns.length - 1)) % btns.length].focus();
  e.preventDefault();
});


function go(id: ScreenId, arg?: unknown, remember = true) {
  const s = ALL.find((x) => x.id === id) ?? homeScreen;
  if (current && current !== s) current.hide?.();
  for (const sec of $$<HTMLElement>("section.screen", pane)) sec.hidden = sec.dataset.id !== s.id;
  let sec = $<HTMLElement>(`section.screen[data-id="${s.id}"]`, pane);
  if (!sec) {
    sec = el(`<section class="screen" data-id="${s.id}" aria-label="${esc(s.title)}"></section>`);
    pane.appendChild(sec);
  }
  if (!inited.has(s.id)) { inited.add(s.id); s.init(sec); }
  pane.classList.toggle("flush", !!s.flush);
  $$<HTMLButtonElement>(".nav-i", nav).forEach((b) => {
    const on = b.dataset.id === navOf(s.id);
    b.classList.toggle("on", on);
    if (on) b.setAttribute("aria-current", "page"); else b.removeAttribute("aria-current");
  });
  const changed = current !== s;
  current = s;
  if (changed) pane.scrollTop = 0;
  s.show?.(arg);
  if (remember) invoke("ui_set", { patch: { tab: s.id } }).catch(() => {});
}
setGo((id, arg) => go(id, arg));
installTips();

// ---------- 끌어다 놓기: 보이는 화면으로 ----------

const veil = $("#drop-veil")!;
$(".dv-ic", veil)!.innerHTML = I("upload");
function dropTo(paths: string[]) {
  if (!paths.length || !current) return;
  if (current.drop) current.drop(paths);
  else { go("home"); homeScreen.drop!(paths); }
}
(window as unknown as { __FT_DROP__: typeof dropTo }).__FT_DROP__ = dropTo;
(async () => {
  if ((window as unknown as { __FT_MOCK__?: unknown }).__FT_MOCK__) return;
  try {
    const { getCurrentWebview } = await import("@tauri-apps/api/webview");
    getCurrentWebview().onDragDropEvent((ev) => {
      const p = ev.payload;
      veil.hidden = !(p.type === "enter" || p.type === "over");
      if (p.type === "drop") dropTo(p.paths);
    });
  } catch { /* 브라우저 미리보기 */ }
})();

// ---------- 문서 도구 처음 풀 때(약 3초) ----------

let prepEl: HTMLElement | null = null;
listen<{ tool: string; fraction: number }>("tools_unpack_progress", (p) => {
  if (p.tool !== "lo" && p.tool !== "ffmpeg" && p.tool !== "pdfium") return;
  if (p.fraction >= 1) { prepEl?.remove(); prepEl = null; return; }
  if (!prepEl) {
    prepEl = el(`<div class="prep" role="status" aria-live="polite"><span class="spin"></span><span class="prep-t"></span><span class="bar"><i></i></span></div>`);
    document.body.appendChild(prepEl);
  }
  $(".prep-t", prepEl)!.textContent = p.tool === "lo" ? "문서 도구 준비 중(약 3초) — 처음 한 번만 해요" : "도구 준비 중 — 처음 한 번만 해요";
  $<HTMLElement>(".bar i", prepEl)!.style.width = `${Math.round(p.fraction * 100)}%`;
});

// ---------- 시작 ----------

initTheme();
applyTheme();

const OLD_TABS: Record<string, ScreenId> = { files: "organize", pdf: "convert", image: "convert", media: "convert", doc: "convert", tools: "settings" };
const pick = (t: string | null | undefined): ScreenId | null => (!t ? null : IDS.includes(t) ? (t as ScreenId) : OLD_TABS[t] ?? null);

// ---------- 0.3.2 우클릭 메뉴로 온 파일 ----------

interface CtxBatch { job: JobId; paths: string[] }
async function takeCtx() {
  const b = await invoke<CtxBatch[]>("ctx_take").catch(() => [] as CtxBatch[]);
  if (!b.length) return;
  const last = b[b.length - 1];
  const cur = jobScreen.current();
  // 같은 할 일 화면에 이미 파일이 있고 아직 시작 전이면 이어 붙인다(탐색기가 파일마다 따로 실행해 늦게 온 것)
  const add = current?.id === "job" && cur.job === last.job && !cur.busy;
  go("job", { job: last.job, paths: last.paths, add });
}
listen<null>("ctx-ready", () => { takeCtx(); });

// 첫 화면(홈)은 스크립트가 돌자마자 그린다 — 백엔드 답을 기다리지 않음(0.3.2 빠른 시작).
// 시작 인자(--tab=, --search=, 우클릭 묶음)는 그다음에 묻고, 새 버전 확인은 첫 화면을 그린 뒤에.
go("home", undefined, false);
requestAnimationFrame(() => setTimeout(() => invoke("ui_ready", { what: "home" }).catch(() => {}), 0));
(async () => {
  const [t, q] = await Promise.all([
    invoke<string | null>("startup_tab").catch(() => null),
    invoke<string | null>("startup_query").catch(() => null),
  ]);
  const start = q ? "search" : pick(t);
  if (start && start !== "home") go(start, q ? { query: q } : undefined, false);
  await takeCtx();
  initUpdate();
})();
listen<string>("goto-tab", (t) => { const id = pick(t); if (id) go(id); });

// 단축키: Ctrl+F = 검색, Ctrl+O = 파일 고르기(변환)
document.addEventListener("keydown", (e) => {
  if (document.querySelector(".dlg-back")) return;
  if (e.ctrlKey && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "f" && current?.id !== "pdfedit") { e.preventDefault(); go("search"); }
});
