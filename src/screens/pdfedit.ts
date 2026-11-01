// PDF 편집: 쪽 썸네일 격자(끌어서 순서·고르기·돌리기·지우기·끼워 넣기·되돌리기) → pdf_apply_plan으로 새 파일.
// 도구(쪽 번호·워터마크·서명·글상자·여백·A4·모아 찍기·암호·정보·양식·평평하게)는 큰 쪽 미리보기 + 오른쪽 설정.
// 좌표: 보이는 쪽 기준 pt, 원점 왼쪽 위(= pdf_thumbs 그림 방향). 미리보기 px → pt = px / scale.
import { EditOut, asErr, cancelJob, dirOf, fileName, friendly, human, invoke, isCancelled, needsPassword, newJob, offJobProgress, onJobProgress, pickFiles, stats } from "../api";
import { FT, I } from "../icons";
import { Screen } from "../router";
import {
  $, $$, askPassword, bindResultButtons, bindSegs, checked, chk, confirmDlg, dialog, empty, errorPanel, esc, opt, resultRow, seg, segVal, setProgress, toast, value,
} from "../ui";

interface PageInfo { page: number; width: number; height: number; rotate: number; visual_width: number; visual_height: number }
interface Src { path: string; password: string | null; pages: PageInfo[]; letter: string }
interface PItem { key: string; kind: "page" | "blank"; src: number; index: number; rotate: number }

let root: HTMLElement;
let sources: Src[] = [];
let pages: PItem[] = [];
let sel = new Set<string>();
let anchor: string | null = null;
let focusKey: string | null = null;
let undoStack: PItem[][] = [];
let redoStack: PItem[][] = [];
let keySeq = 0;
let busy = false;
let mode: string = "grid"; // "grid" | 도구 id
let lastOut: { output: string; note: string; before?: number; after?: number } | null = null;
let pendingTool: string | null = null;

const thumbCache = new Map<string, string>(); // `${src}:${index}@${w}` → data url
const THUMB_W = () => Math.round(150 * Math.min(2, window.devicePixelRatio || 1));

const newKey = () => `k${++keySeq}`;
const doc = () => sources[0];
const isDirty = () => undoStack.length > 0 || sources.length > 1;
const pageOf = (p: PItem) => sources[p.src]?.pages[p.index];

// ---------- 열기 ----------

async function loadPages(path: string, password: string | null): Promise<{ pages: PageInfo[]; password: string | null } | null> {
  let pw = password;
  for (let i = 0; i < 6; i++) {
    try {
      const d = await invoke<{ pages: PageInfo[]; encrypted: boolean }>("pdf_pages", { path, password: pw });
      return { pages: d.pages, password: pw };
    } catch (e) {
      if (!needsPassword(e)) throw e;
      const got = await askPassword(fileName(path), asErr(e).code === "wrong_password" && pw != null);
      if (got == null) return null;
      pw = got;
    }
  }
  return null;
}

async function openDoc(path: string) {
  if (isDirty() && !(await confirmDlg("저장하지 않은 쪽 편집이 있어요", "다른 PDF를 열면 지금 편집한 내용은 사라져요. 그래도 열까요?", "열기", true))) return;
  try {
    const got = await loadPages(path, null);
    if (!got) return;
    sources = [{ path, password: got.password, pages: got.pages, letter: "" }];
    pages = got.pages.map((_, i) => ({ key: newKey(), kind: "page", src: 0, index: i, rotate: 0 }));
    sel = new Set(); anchor = null; focusKey = pages[0]?.key ?? null;
    undoStack = []; redoStack = [];
    mode = "grid"; lastOut = null;
    render();
  } catch (e) {
    showError(e);
  }
}

/** 다른 PDF의 쪽을 (고른 쪽 뒤 또는 맨 끝에) 끼워 넣기 */
async function addPdf(path: string, atEnd: boolean) {
  try {
    const got = await loadPages(path, null);
    if (!got) return;
    snapshot();
    const si = sources.length;
    sources.push({ path, password: got.password, pages: got.pages, letter: String.fromCharCode(65 + (si % 26)) });
    if (sources.length === 2) sources[0].letter = "A";
    const add = got.pages.map((_, i) => ({ key: newKey(), kind: "page" as const, src: si, index: i, rotate: 0 }));
    const at = atEnd ? pages.length : insertPos();
    pages.splice(at, 0, ...add);
    sel = new Set(add.map((x) => x.key));
    render();
    toast(`${fileName(path)} ${add.length}쪽을 ${atEnd ? "끝에 더했어요" : "끼워 넣었어요"}`);
  } catch (e) {
    toast(friendly(e).title);
  }
}

/** 고른 쪽 중 마지막 뒤(없으면 맨 끝) */
function insertPos(): number {
  let last = -1;
  pages.forEach((p, i) => { if (sel.has(p.key)) last = i; });
  return last < 0 ? pages.length : last + 1;
}

// ---------- 편집(되돌리기 가능) ----------

function snapshot() {
  undoStack.push(pages.map((p) => ({ ...p })));
  if (undoStack.length > 200) undoStack.shift();
  redoStack = [];
}
function undo() { if (!undoStack.length) return; redoStack.push(pages); pages = undoStack.pop()!; sel = new Set([...sel].filter((k) => pages.some((p) => p.key === k))); renderGrid(); }
function redo() { if (!redoStack.length) return; undoStack.push(pages); pages = redoStack.pop()!; renderGrid(); }
const targets = () => (sel.size ? pages.filter((p) => sel.has(p.key)) : []);

function rotate(deg: number) {
  const t = targets();
  if (!t.length) { toast("돌릴 쪽을 먼저 골라 주세요"); return; }
  snapshot();
  for (const p of t) p.rotate = (((p.rotate + deg) % 360) + 360) % 360;
  renderGrid();
}
function remove() {
  const t = targets();
  if (!t.length) { toast("지울 쪽을 먼저 골라 주세요"); return; }
  if (t.length === pages.length) { toast("쪽을 모두 지울 수는 없어요. 한 쪽은 남겨 주세요"); return; }
  snapshot();
  const idx = pages.findIndex((p) => sel.has(p.key));
  pages = pages.filter((p) => !sel.has(p.key));
  sel = new Set();
  focusKey = pages[Math.min(idx, pages.length - 1)]?.key ?? null;
  renderGrid();
  toast(`${t.length}쪽을 지웠어요 · 되돌리려면 Ctrl+Z`);
}
function insertBlank() {
  snapshot();
  const at = insertPos();
  const b: PItem = { key: newKey(), kind: "blank", src: -1, index: -1, rotate: 0 };
  pages.splice(at, 0, b);
  sel = new Set([b.key]);
  focusKey = b.key;
  renderGrid();
}
/** 고른 쪽들을 to 위치(그 쪽 앞)로 */
function moveSelected(to: number) {
  const moving = pages.filter((p) => sel.has(p.key));
  if (!moving.length) return;
  const before = pages.slice(0, to).filter((p) => !sel.has(p.key)).length;
  const rest = pages.filter((p) => !sel.has(p.key));
  const next = [...rest.slice(0, before), ...moving, ...rest.slice(before)];
  if (next.every((p, i) => p === pages[i])) return;
  snapshot();
  pages = next;
  renderGrid();
}

// ---------- 저장 ----------

function plan(only?: Set<string>) {
  return {
    sources: sources.map((s) => ({ path: s.path, password: s.password })),
    pages: pages.filter((p) => !only || only.has(p.key)).map((p) =>
      p.kind === "blank" ? { kind: "blank", size: "same" } : { kind: "page", src: p.src, index: p.index, rotate: p.rotate }),
  };
}

async function withJob<T>(label: string, f: (job: number) => Promise<T>): Promise<T | null> {
  if (busy) return null;
  busy = true;
  const job = newJob();
  const bar = $(".pe-busy", root);
  if (bar) {
    bar.hidden = false;
    bar.innerHTML = `<div class="panel pg"><div class="pg-h"><b>${esc(label)}…</b><span class="pg-pct">0%</span></div><div class="bar"><i style="width:0"></i></div><div class="pg-f"><span class="pg-step"></span><button type="button" class="btn" data-cancel>취소</button></div></div>`;
    $("[data-cancel]", bar)!.addEventListener("click", () => cancelJob(job));
  }
  onJobProgress(job, (fr) => bar && setProgress(bar, fr));
  try {
    return await f(job);
  } catch (e) {
    if (!isCancelled(e)) showError(e); else toast("멈췄어요");
    return null;
  } finally {
    offJobProgress(job);
    busy = false;
    if (bar) { bar.hidden = true; bar.innerHTML = ""; }
  }
}

async function finish(o: EditOut, label: string) {
  const st = await stats([o.output, doc().path]);
  lastOut = { output: o.output, note: `${label} · ${o.note || `${o.pages}쪽`}`, before: st.get(doc().path)?.size, after: st.get(o.output)?.size };
  renderResult();
  $(".pe-result", root)?.scrollIntoView({ block: "nearest" });
}

async function save() {
  if (!pages.length) return;
  const o = await withJob("저장하는 중", (job) => invoke<EditOut>("pdf_apply_plan", { job, plan: plan(), outDir: null }));
  if (o) await finish(o, "쪽 편집");
}

async function extractSelected() {
  const t = targets();
  if (!t.length) { toast("뽑을 쪽을 먼저 골라 주세요"); return; }
  const simple = sources.length === 1 && t.every((p) => p.kind === "page" && p.src === 0 && p.rotate === 0);
  const o = await withJob("고른 쪽 뽑는 중", (job) =>
    simple
      ? invoke<EditOut>("pdf_extract", { job, path: doc().path, password: doc().password, pages: t.map((p) => p.index + 1).join(","), outDir: null })
      : invoke<EditOut>("pdf_apply_plan", { job, plan: plan(sel), outDir: null }));
  if (o) await finish(o, `고른 ${t.length}쪽 뽑기`);
}

function showError(e: unknown) {
  const f = friendly(e);
  const box = $(".pe-result", root);
  if (box) { box.innerHTML = errorPanel(f); box.hidden = false; }
  else toast(f.title);
}

// ---------- 그리기 ----------

function render() {
  if (!sources.length) {
    root.innerHTML = `<h1 class="title">PDF 편집</h1><p class="sub">쪽 순서를 바꾸고, 돌리고, 지우고, 다른 PDF를 끼워 넣어요. 서명·쪽 번호·워터마크도 여기서 넣어요.</p>
      <div class="dz" id="pe-dz" tabindex="0" role="button" aria-label="PDF 열기">
        <div class="dz-ic">${I("layers")}</div><div class="dz-t">PDF를 여기에 끌어다 놓으세요</div><div class="dz-s">여러 개를 넣으면 한 문서로 이어서 편집해요</div>
        <div class="row center"><button type="button" class="btn accent" data-act="open">PDF 열기</button></div></div>
      <div class="pe-result" hidden></div>`;
    return;
  }
  const d = doc();
  root.innerHTML = `<h1 class="title">PDF 편집</h1>
    <div class="filecard">${FT("pdf", 36)}<div class="fc-t"><b title="${esc(d.path)}">${esc(fileName(d.path))}${sources.length > 1 ? ` 외 ${sources.length - 1}개` : ""}</b>
      <span class="pe-meta"></span></div>
      <button type="button" class="btn subtle" data-act="open">${I("upload")}<span class="hide-n">다른 PDF 열기</span></button></div>
    <div class="pe-result" hidden></div><div class="pe-busy" hidden></div>
    <div class="pe-main"></div>`;
  renderResult();
  if (mode === "grid") renderGridView(); else renderTool(mode);
}

function renderResult() {
  const box = $(".pe-result", root);
  if (!box) return;
  if (!lastOut) { box.hidden = true; box.innerHTML = ""; return; }
  box.hidden = false;
  box.innerHTML = `<div class="panel"><div class="done-h"><span class="okc">${I("check")}</span><div><b>새 파일로 저장했어요</b><span>${esc(lastOut.note)} · 원본은 그대로예요</span></div>
    <button type="button" class="ib close" data-act="close-result" aria-label="닫기">${I("x")}</button></div>
    ${resultRow({ output: lastOut.output, before: lastOut.before, after: lastOut.after, pill: "" })}
    <div class="after-row"><button type="button" class="btn subtle" data-act="continue">${I("pencil")}이 결과로 계속 편집</button></div></div>`;
}

function renderGridView() {
  const main = $(".pe-main", root)!;
  main.innerHTML = `<div class="pe-bar" role="toolbar" aria-label="쪽 편집">
      <button type="button" class="btn" data-act="rotl" title="왼쪽으로 돌리기 (Shift+R)" aria-label="왼쪽으로 돌리기">${I("rotl")}</button>
      <button type="button" class="btn" data-act="rotr" title="오른쪽으로 돌리기 (R)" aria-label="오른쪽으로 돌리기">${I("rotr")}</button>
      <button type="button" class="btn" data-act="del" title="고른 쪽 지우기 (Delete)" aria-label="지우기">${I("trash")}<span>지우기</span></button>
      <span class="sep"></span>
      <button type="button" class="btn" data-act="extract" title="고른 쪽만 새 PDF로" aria-label="뽑기">${I("extract")}<span>뽑기</span></button>
      <button type="button" class="btn" data-act="blank" title="고른 쪽 뒤에 빈 쪽" aria-label="빈 쪽">${I("blank")}<span>빈 쪽</span></button>
      <button type="button" class="btn" data-act="insert" title="고른 쪽 뒤에 다른 PDF 끼워 넣기" aria-label="끼워 넣기">${I("insert")}<span>끼워 넣기</span></button>
      <button type="button" class="btn" data-act="append" title="다른 PDF를 끝에 붙이기(합치기)" aria-label="더하기">${I("merge")}<span>더하기</span></button>
      <span class="sep"></span>
      <button type="button" class="btn subtle" data-act="undo" title="되돌리기 (Ctrl+Z)" aria-label="되돌리기">${I("undo")}</button>
      <button type="button" class="btn subtle" data-act="redo" title="다시 하기 (Ctrl+Y)" aria-label="다시 하기">${I("redo")}</button>
      <span class="grow"></span>
      <button type="button" class="btn accent" data-act="save" title="새 파일로 저장 (Ctrl+S)" aria-label="저장">${I("check")}<span>저장</span></button></div>
    <div class="pe-status"><span class="pe-sel"></span><span class="hint">눌러서 고르기 · Ctrl/Shift로 여러 쪽 · 끌어서 순서 바꾸기</span></div>
    <div class="pgrid" role="listbox" aria-multiselectable="true" aria-label="쪽" tabindex="-1"></div>
    <h2 class="h2">도구</h2>
    <div class="tools">${TOOLS.map((t) => `<button type="button" class="tool" data-tool="${t.id}"><span class="tile tone-${t.tone}">${I(t.ic)}</span><span><b>${esc(t.t)}</b><span class="d">${esc(t.d)}</span></span></button>`).join("")}</div>`;
  renderGrid();
}

function renderGrid() {
  const grid = $(".pgrid", root);
  if (!grid) return;
  const multi = sources.length > 1;
  grid.innerHTML = pages.map((p, i) => {
    const info = pageOf(p);
    const on = sel.has(p.key);
    const ar = info ? (p.rotate % 180 ? info.visual_height / info.visual_width : info.visual_width / info.visual_height) : 0.707;
    return `<div class="pcell ${on ? "sel" : ""}" draggable="false" role="option" aria-selected="${on}" tabindex="${p.key === (focusKey ?? pages[0]?.key) ? 0 : -1}" data-key="${p.key}"
        aria-label="${i + 1}쪽${p.kind === "blank" ? " (빈 쪽)" : ""}${p.rotate ? `, ${p.rotate}도 돌림` : ""}">
      <div class="pthumb" style="--ar:${ar.toFixed(4)}">${p.kind === "blank" ? `<span class="blankpg">빈 쪽</span>` :
        `<img alt="" draggable="false" data-k="${p.src}:${p.index}" style="${imgRot(p)}">`}</div>
      <div class="pfoot"><span class="pnum">${i + 1}</span>${multi && p.kind === "page" ? `<span class="psrc" title="${esc(fileName(sources[p.src].path))}">${sources[p.src].letter}${p.index + 1}</span>` : ""}${p.rotate ? `<span class="prot">${I("rotr")}${p.rotate}°</span>` : ""}</div>
      <span class="pcheck">${I("check")}</span></div>`;
  }).join("");
  // 썸네일: 캐시된 것은 바로, 나머지는 보일 때
  for (const img of $$<HTMLImageElement>("img[data-k]", grid)) {
    const url = thumbCache.get(`${img.dataset.k}@${THUMB_W()}`);
    if (url) img.src = url; else io.observe(img);
  }
  updateStatus();
}
function imgRot(p: PItem) {
  if (!p.rotate) return "";
  const info = pageOf(p);
  if (!info || p.rotate === 180) return `transform: rotate(${p.rotate}deg)`;
  // 90/270: 칸(돌린 비율) 안에 돌린 그림이 꼭 맞게
  const s = info.visual_height / info.visual_width;
  return `transform: rotate(${p.rotate}deg) scale(${Math.min(s, 1 / s).toFixed(4)})`;
}
function updateStatus() {
  const n = pages.length, s = sel.size;
  const meta = $(".pe-meta", root);
  if (meta) meta.textContent = `${n}쪽${sources.some((x) => x.password) ? " · 암호로 열었어요(저장하면 암호 없이)" : ""}${isDirty() ? " · 저장하지 않은 편집이 있어요" : ""}`;
  const st = $(".pe-sel", root);
  if (st) st.textContent = s ? `${s}쪽 골랐어요` : "쪽을 고르세요";
  const undoB = $<HTMLButtonElement>("[data-act=undo]", root), redoB = $<HTMLButtonElement>("[data-act=redo]", root);
  if (undoB) undoB.disabled = !undoStack.length;
  if (redoB) redoB.disabled = !redoStack.length;
  const saveB = $<HTMLButtonElement>("[data-act=save]", root);
  if (saveB) saveB.classList.toggle("pulse", isDirty());
}

// 썸네일 지연 읽기
const want = new Map<number, Set<number>>();
let pumping = false;
const io = new IntersectionObserver((ents) => {
  for (const e of ents) {
    if (!e.isIntersecting) continue;
    const img = e.target as HTMLImageElement;
    io.unobserve(img);
    const [s, i] = img.dataset.k!.split(":").map(Number);
    if (!want.has(s)) want.set(s, new Set());
    want.get(s)!.add(i);
  }
  pump();
}, { rootMargin: "300px" });
async function pump() {
  if (pumping) return;
  pumping = true;
  try {
    while (want.size) {
      const [s, set] = want.entries().next().value as [number, Set<number>];
      const batch = [...set].slice(0, 6);
      batch.forEach((i) => set.delete(i));
      if (!set.size) want.delete(s);
      const src = sources[s];
      if (!src) continue;
      const w = THUMB_W();
      try {
        const out = await invoke<{ page: number; data_url: string }[]>("pdf_thumbs", { path: src.path, password: src.password, pages: batch.map((i) => i + 1), width: w });
        for (const t of out) {
          const k = `${s}:${t.page - 1}`;
          thumbCache.set(`${k}@${w}`, t.data_url);
          $$<HTMLImageElement>(`img[data-k="${k}"]`, root).forEach((img) => (img.src = t.data_url));
        }
      } catch { /* 그림 없이도 편집 가능 */ }
    }
  } finally {
    pumping = false;
  }
}

// ---------- 고르기·끌기 ----------

function clickSelect(key: string, e: { ctrlKey: boolean; shiftKey: boolean; metaKey?: boolean }) {
  if (e.shiftKey && anchor) {
    const a = pages.findIndex((p) => p.key === anchor), b = pages.findIndex((p) => p.key === key);
    const [x, y] = a < b ? [a, b] : [b, a];
    if (!e.ctrlKey) sel = new Set();
    for (let i = x; i <= y; i++) sel.add(pages[i].key);
  } else if (e.ctrlKey || e.metaKey) {
    if (sel.has(key)) sel.delete(key); else sel.add(key);
    anchor = key;
  } else {
    sel = new Set([key]);
    anchor = key;
  }
  focusKey = key;
  for (const c of $$<HTMLElement>(".pcell", root)) {
    const on = sel.has(c.dataset.key!);
    c.classList.toggle("sel", on);
    c.setAttribute("aria-selected", String(on));
    c.tabIndex = c.dataset.key === key ? 0 : -1;
  }
  updateStatus();
}

let drag: { key: string; x: number; y: number; started: boolean; ghost?: HTMLElement; target?: number; id: number } | null = null;
function onPointerDown(e: PointerEvent) {
  const cell = (e.target as HTMLElement).closest<HTMLElement>(".pcell");
  if (!cell || e.button !== 0) return;
  e.preventDefault(); // 그림 기본 끌기·글자 고르기 막기
  cell.focus({ preventScroll: true });
  drag = { key: cell.dataset.key!, x: e.clientX, y: e.clientY, started: false, id: e.pointerId };
}
function onPointerMove(e: PointerEvent) {
  if (!drag || e.pointerId !== drag.id) return;
  if (!drag.started) {
    if (Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < 7) return;
    drag.started = true;
    if (!sel.has(drag.key)) clickSelect(drag.key, { ctrlKey: false, shiftKey: false });
    const n = sel.size;
    drag.ghost = document.createElement("div");
    drag.ghost.className = "drag-ghost";
    drag.ghost.textContent = n > 1 ? `${n}쪽 옮기는 중` : "쪽 옮기는 중";
    document.body.appendChild(drag.ghost);
    root.classList.add("dragging");
  }
  drag.ghost!.style.left = `${e.clientX + 12}px`;
  drag.ghost!.style.top = `${e.clientY + 12}px`;
  // 놓을 자리
  $$(".pcell.drop-b, .pcell.drop-a", root).forEach((c) => c.classList.remove("drop-b", "drop-a"));
  const over = (document.elementFromPoint(e.clientX, e.clientY) as HTMLElement | null)?.closest<HTMLElement>(".pcell");
  if (over) {
    const r = over.getBoundingClientRect();
    const after = e.clientX > r.left + r.width / 2;
    over.classList.add(after ? "drop-a" : "drop-b");
    const i = pages.findIndex((p) => p.key === over.dataset.key);
    drag.target = after ? i + 1 : i;
  }
  // 가장자리에서 저절로 내리기
  const pane = document.getElementById("pane")!;
  const pr = pane.getBoundingClientRect();
  if (e.clientY > pr.bottom - 40) pane.scrollTop += 14; else if (e.clientY < pr.top + 40) pane.scrollTop -= 14;
}
function onPointerUp(e: PointerEvent) {
  if (!drag || e.pointerId !== drag.id) return;
  const d = drag;
  drag = null;
  if (d.started) {
    d.ghost?.remove();
    root.classList.remove("dragging");
    $$(".pcell.drop-b, .pcell.drop-a", root).forEach((c) => c.classList.remove("drop-b", "drop-a"));
    if (d.target != null) moveSelected(d.target);
  } else {
    clickSelect(d.key, e);
  }
}

function onGridKey(e: KeyboardEvent) {
  const cell = (e.target as HTMLElement).closest<HTMLElement>(".pcell");
  if (!cell) return;
  const i = pages.findIndex((p) => p.key === cell.dataset.key);
  const cols = Math.max(1, Math.round($(".pgrid", root)!.clientWidth / cell.getBoundingClientRect().width));
  const focusAt = (j: number, extend: boolean) => {
    j = Math.max(0, Math.min(pages.length - 1, j));
    const k = pages[j].key;
    if (extend) clickSelect(k, { ctrlKey: false, shiftKey: true });
    focusKey = k;
    $$<HTMLElement>(".pcell", root).forEach((c) => (c.tabIndex = c.dataset.key === k ? 0 : -1));
    $<HTMLElement>(`.pcell[data-key="${k}"]`, root)?.focus();
  };
  const moveBy = (d: number) => { if (!sel.has(pages[i].key)) clickSelect(pages[i].key, { ctrlKey: false, shiftKey: false }); const first = pages.findIndex((p) => sel.has(p.key)); const last = pages.length - 1 - [...pages].reverse().findIndex((p) => sel.has(p.key)); moveSelected(d < 0 ? Math.max(0, first - 1) : Math.min(pages.length, last + 2)); $<HTMLElement>(`.pcell[data-key="${pages[i]?.key}"]`, root); const k = cell.dataset.key!; $<HTMLElement>(`.pcell[data-key="${k}"]`, root)?.focus(); };
  if (e.ctrlKey && (e.key === "ArrowLeft" || e.key === "ArrowRight")) { moveBy(e.key === "ArrowLeft" ? -1 : 1); e.preventDefault(); return; }
  switch (e.key) {
    case "ArrowRight": focusAt(i + 1, e.shiftKey); break;
    case "ArrowLeft": focusAt(i - 1, e.shiftKey); break;
    case "ArrowDown": focusAt(i + cols, e.shiftKey); break;
    case "ArrowUp": focusAt(i - cols, e.shiftKey); break;
    case "Home": focusAt(0, e.shiftKey); break;
    case "End": focusAt(pages.length - 1, e.shiftKey); break;
    case " ": clickSelect(pages[i].key, { ctrlKey: true, shiftKey: false }); break;
    case "Enter": clickSelect(pages[i].key, { ctrlKey: false, shiftKey: false }); break;
    case "Delete": case "Backspace": remove(); break;
    case "r": case "R": rotate(e.shiftKey ? -90 : 90); break;
    default: return;
  }
  e.preventDefault();
}

// ---------- 도구 ----------

interface Tool { id: string; t: string; d: string; ic: string; tone: string; preview?: boolean }
const TOOLS: Tool[] = [
  { id: "num", t: "쪽 번호", d: "아래·위에 번호", ic: "hash", tone: "teal", preview: true },
  { id: "mark", t: "워터마크", d: "글자·그림을 흐리게", ic: "drop", tone: "purple", preview: true },
  { id: "sign", t: "서명·도장", d: "서명 그림 놓기", ic: "sign", tone: "red", preview: true },
  { id: "text", t: "글상자", d: "새 글자 얹기", ic: "text", tone: "blue", preview: true },
  { id: "crop", t: "여백 자르기", d: "가장자리 잘라내기", ic: "crop", tone: "orange", preview: true },
  { id: "a4", t: "A4로 맞추기", d: "쪽 크기를 A4로", ic: "a4", tone: "gray" },
  { id: "nup", t: "모아 찍기", d: "2쪽·4쪽을 한 장에", ic: "grid", tone: "green" },
  { id: "lock", t: "암호", d: "열 때 암호·인쇄 제한", ic: "lock", tone: "blue" },
  { id: "info", t: "문서 정보", d: "제목·작성자 바꾸기", ic: "info", tone: "gray" },
  { id: "form", t: "양식 채우기", d: "빈칸에 글 넣기", ic: "form", tone: "teal" },
  { id: "flat", t: "평평하게", d: "양식·메모를 쪽에 굳히기", ic: "flatten", tone: "gray" },
];

async function openTool(id: string) {
  if (isDirty()) {
    const ok = await confirmDlg("쪽 편집을 먼저 저장할게요", "도구는 저장한 새 파일에 적용해요. 지금 편집한 쪽을 새 파일로 저장한 뒤 이어서 할까요?", "저장하고 계속");
    if (!ok) return;
    const o = await withJob("저장하는 중", (job) => invoke<EditOut>("pdf_apply_plan", { job, plan: plan(), outDir: null }));
    if (!o) return;
    await finish(o, "쪽 편집");
    await reopen(o.output);
  }
  mode = id;
  previewPage = Math.min(previewPage, pages.length - 1);
  if (id !== "grid") {
    const s = [...sel];
    const first = pages.findIndex((p) => p.key === s[0]);
    if (first >= 0) previewPage = first;
  }
  renderMain();
  $(".pe-main", root)?.scrollIntoView({ block: "start" });
}
function renderMain() { if (mode === "grid") renderGridView(); else renderTool(mode); }

/** 결과 파일을 새 문서로(암호 없이 저장되므로 암호 필요 없음) */
async function reopen(path: string) {
  const got = await loadPages(path, null);
  if (!got) return;
  sources = [{ path, password: got.password, pages: got.pages, letter: "" }];
  pages = got.pages.map((_, i) => ({ key: newKey(), kind: "page", src: 0, index: i, rotate: 0 }));
  sel = new Set(); undoStack = []; redoStack = [];
  const meta = $(".fc-t b", root);
  if (meta) { meta.textContent = fileName(path); meta.title = path; }
  updateStatus();
}

let previewPage = 0;
let ov: Overlay | null = null;

function renderTool(id: string) {
  const t = TOOLS.find((x) => x.id === id)!;
  const main = $(".pe-main", root)!;
  main.innerHTML = `<div class="tool-head"><button type="button" class="btn subtle" data-act="back">${I("back")}쪽 목록으로</button><h2 class="h2 inline">${esc(t.t)}</h2></div>
    <div class="tool-wrap ${t.preview ? "" : "nopv"}">
      ${t.preview ? `<div class="pv"><div class="pv-nav"><button type="button" class="ib" data-act="pv-prev" aria-label="앞 쪽">${I("left")}</button><span class="pv-n"></span><button type="button" class="ib" data-act="pv-next" aria-label="다음 쪽">${I("right")}</button></div>
        <div class="pv-stage"><div class="pv-page"><img class="pv-img" alt=""><div class="pv-ov"></div></div></div></div>` : ""}
      <div class="tool-opts optcard"><div class="oc-body"></div><div class="oc-foot"><span class="save">${I("folder")}<b>원본 옆</b><span class="path">${esc(dirOf(doc().path))}</span></span><button type="button" class="btn accent lg" data-act="apply"></button></div></div>
    </div>`;
  const body = $(".tool-opts .oc-body", main)!;
  const applyB = $<HTMLButtonElement>("[data-act=apply]", main)!;
  ov = null;
  TOOL_UI[id](body, applyB);
  bindSegs(main);
  if (t.preview) showPreview();
}

async function showPreview() {
  const img = $<HTMLImageElement>(".pv-img", root);
  if (!img) return;
  const p = pages[previewPage];
  const info = p && pageOf(p);
  $(".pv-n", root)!.textContent = `${previewPage + 1} / ${pages.length}쪽`;
  if (!info) return;
  const stage = $(".pv-stage", root)!;
  const maxW = Math.max(240, Math.min(stage.clientWidth || 520, 720));
  const ar = info.visual_width / info.visual_height;
  const maxH = Math.max(320, Math.min(window.innerHeight - 220, 760));
  const w = Math.round(Math.min(maxW, maxH * ar));
  const pageEl = $<HTMLElement>(".pv-page", root)!;
  pageEl.style.width = `${w}px`;
  pageEl.style.height = `${Math.round(w / ar)}px`;
  const pw = Math.round(w * Math.min(2, window.devicePixelRatio || 1));
  const k = `${p.src}:${p.index}@${pw}`;
  let url = thumbCache.get(k);
  if (!url) {
    // 같은 쪽의 작은 그림이 이미 있으면 그대로 두고(깜빡임 없게), 다른 쪽이면 지움
    const small = thumbCache.get(`${p.src}:${p.index}@${THUMB_W()}`);
    if (img.dataset.k !== `${p.src}:${p.index}`) { if (small) img.src = small; else img.removeAttribute("src"); }
    img.dataset.k = `${p.src}:${p.index}`;
    try {
      const out = await invoke<{ data_url: string }[]>("pdf_thumbs", { path: sources[p.src].path, password: sources[p.src].password, pages: [p.index + 1], width: pw });
      url = out[0]?.data_url;
      if (url) thumbCache.set(k, url);
    } catch { /* 그림 없이 */ }
  }
  if (url && pages[previewPage] === p) img.src = url;
  const scale = w / info.visual_width;
  ov?.draw({ scale, page: previewPage + 1, total: pages.length, w: info.visual_width, h: info.visual_height });
}

interface Geo { scale: number; page: number; total: number; w: number; h: number }
interface Overlay { draw(g: Geo): void }

/** 끌기·크기 바꾸기 상자. rect는 pt */
interface Box { x: number; y: number; w: number; h: number }
function makeBox(layer: HTMLElement, g: () => Geo, b: Box, o: { cls: string; html?: string; ratio?: number; handles: "corner" | "edges" | "right"; onChange: (b: Box) => void; onFocus?: () => void }) {
  const el = document.createElement("div");
  el.className = `ovbox ${o.cls}`;
  el.tabIndex = 0;
  el.innerHTML = (o.html ?? "") + (o.handles === "corner" ? `<span class="hd br" data-h="br"></span>` : o.handles === "right" ? `<span class="hd r" data-h="r"></span>` : `<span class="hd t" data-h="t"></span><span class="hd r" data-h="r"></span><span class="hd b" data-h="b"></span><span class="hd l" data-h="l"></span>`);
  layer.appendChild(el);
  const place = () => {
    const s = g().scale;
    el.style.left = `${b.x * s}px`; el.style.top = `${b.y * s}px`; el.style.width = `${b.w * s}px`; el.style.height = `${b.h * s}px`;
  };
  const clamp = () => {
    const { w, h } = g();
    b.w = Math.max(8, Math.min(b.w, w)); b.h = Math.max(6, Math.min(b.h, h));
    b.x = Math.max(0, Math.min(b.x, w - b.w)); b.y = Math.max(0, Math.min(b.y, h - b.h));
  };
  el.addEventListener("pointerdown", (e) => {
    e.preventDefault(); e.stopPropagation();
    el.setPointerCapture(e.pointerId);
    el.focus();
    o.onFocus?.();
    const h = (e.target as HTMLElement).dataset.h;
    const s = g().scale, x0 = e.clientX, y0 = e.clientY, s0 = { ...b };
    const move = (ev: PointerEvent) => {
      const dx = (ev.clientX - x0) / s, dy = (ev.clientY - y0) / s;
      if (!h) { b.x = s0.x + dx; b.y = s0.y + dy; }
      else if (h === "br") { b.w = Math.max(8, s0.w + dx); b.h = o.ratio ? b.w / o.ratio : Math.max(6, s0.h + dy); }
      else if (h === "r") b.w = Math.max(8, s0.w + dx);
      else if (h === "l") { b.x = Math.min(s0.x + dx, s0.x + s0.w - 8); b.w = s0.w - (b.x - s0.x); }
      else if (h === "t") { b.y = Math.min(s0.y + dy, s0.y + s0.h - 8); b.h = s0.h - (b.y - s0.y); }
      else if (h === "b") b.h = Math.max(8, s0.h + dy);
      clamp(); place(); o.onChange(b);
    };
    const up = () => { el.removeEventListener("pointermove", move); el.removeEventListener("pointerup", up); };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  });
  el.addEventListener("keydown", (e) => {
    const st = e.shiftKey ? 10 : 1;
    const k = { ArrowLeft: [-st, 0], ArrowRight: [st, 0], ArrowUp: [0, -st], ArrowDown: [0, st] }[e.key];
    if (!k) return;
    if (e.altKey) { b.w += k[0]; b.h = o.ratio ? b.w / o.ratio : b.h + k[1]; } else { b.x += k[0]; b.y += k[1]; }
    clamp(); place(); o.onChange(b); e.preventDefault();
  });
  clamp(); place();
  return { el, place, box: b, setRatio(r: number) { o.ratio = r; b.h = b.w / r; clamp(); place(); } };
}

const toolJob = (label: string, cmd: string, args: Record<string, unknown>) =>
  withJob(label, (job) => invoke<EditOut>(cmd, { job, path: doc().path, password: doc().password, outDir: null, ...args }));

type ToolUI = (body: HTMLElement, apply: HTMLButtonElement) => void;
const TOOL_UI: Record<string, ToolUI> = {
  num(body, apply) {
    apply.textContent = "쪽 번호 넣기";
    body.innerHTML = `<div class="info">${I("eye")}<span>왼쪽 쪽 위에 넣을 모양을 미리 보여 드려요.</span></div>` +
      opt("위치", seg("pos", [["bottom-center", "아래 가운데"], ["bottom-right", "아래 오른쪽"], ["bottom-left", "아래 왼쪽"], ["top-center", "위 가운데"], ["top-right", "위 오른쪽"], ["top-left", "위 왼쪽"]], "bottom-center")) +
      opt("모양", seg("nfmt", [["plain", "1"], ["of-total", "1 / N"], ["dash", "- 1 -"]], "plain")) +
      opt("글자 크기", seg("nsize", [["9", "작게"], ["11", "보통"], ["14", "크게"]], "11")) +
      opt("시작 번호", `<input class="txt sm" id="nstart" value="1" inputmode="numeric">`) +
      opt("", chk("skip1", "첫 쪽(표지)은 비우기", false));
    const label = document.createElement("div");
    label.className = "ov-num";
    const read = () => ({ position: segVal(body, "pos"), format: segVal(body, "nfmt"), font_size: Number(segVal(body, "nsize")), start: Math.max(0, Math.round(Number(value(body, "nstart")) || 1)), skip_first: checked(body, "skip1") });
    ov = {
      draw(g) {
        const layer = $(".pv-ov", root)!;
        if (!label.isConnected) layer.appendChild(label);
        const o = read();
        const m = 28.35 * g.scale;
        const n = o.skip_first ? g.page - 2 + o.start : g.page - 1 + o.start;
        const last = o.skip_first ? g.total - 2 + o.start : g.total - 1 + o.start;
        const txt = o.skip_first && g.page === 1 ? "" : o.format === "of-total" ? `${n} / ${last}` : o.format === "dash" ? `- ${n} -` : `${n}`;
        label.textContent = txt;
        label.style.fontSize = `${o.font_size * g.scale}px`;
        const [v, hz] = o.position.split("-");
        label.style.top = v === "top" ? `${m}px` : ""; label.style.bottom = v === "bottom" ? `${m}px` : "";
        label.style.left = hz === "left" ? `${m}px` : hz === "center" ? "50%" : ""; label.style.right = hz === "right" ? `${m}px` : "";
        label.style.transform = hz === "center" ? "translateX(-50%)" : "";
      },
    };
    body.addEventListener("click", () => showPreview());
    body.addEventListener("input", () => showPreview());
    apply.onclick = async () => {
      const o = await toolJob("쪽 번호 넣는 중", "pdf_page_numbers", { opts: read() });
      if (o) await finish(o, "쪽 번호");
    };
  },
  mark(body, apply) {
    apply.textContent = "워터마크 넣기";
    let imgPath: string | null = null, imgUrl: string | null = null, imgRatio = 1;
    body.innerHTML = opt("종류", seg("wkind", [["text", "글자"], ["image", "그림"]], "text")) +
      `<div data-k="text">${opt("글자", `<input class="txt" id="wtext" value="대외비" maxlength="60">`)}${opt("크기", seg("wsize", [["36", "작게"], ["54", "보통"], ["80", "크게"]], "54"))}${opt("색", seg("wcolor", [["#808080", "회색"], ["#C42B1C", "빨강"], ["#1F5BAA", "파랑"]], "#808080"))}</div>` +
      `<div data-k="image" hidden>${opt("그림", `<button type="button" class="btn" data-pick-img>${I("image")}그림 고르기</button><span class="wimg-n hint">PNG·JPG</span>`)}${opt("크기", seg("wscale", [["0.3", "작게"], ["0.5", "보통"], ["0.8", "크게"]], "0.5"), "쪽 너비 기준")}</div>` +
      opt("진하기", seg("wop", [["0.15", "흐리게"], ["0.3", "보통"], ["0.5", "진하게"]], "0.3")) +
      opt("기울기", seg("wang", [["45", "비스듬히"], ["0", "가로로"], ["90", "세로로"]], "45")) +
      opt("놓는 법", seg("wtile", [["center", "가운데"], ["tile", "바둑판"]], "center")) +
      opt("", chk("wunder", "글자 아래에 깔기(흰 바탕 그림 쪽에서는 가려질 수 있어요)", false));
    const read = () => {
      const kind = segVal(body, "wkind");
      const common = { opacity: Number(segVal(body, "wop")), angle: Number(segVal(body, "wang")), tiling: segVal(body, "wtile"), layer: checked(body, "wunder") ? "under" : "over" };
      return kind === "image" ? { kind, path: imgPath, scale: Number(segVal(body, "wscale")), ...common }
        : { kind, text: value(body, "wtext"), font_size: Number(segVal(body, "wsize")), color: segVal(body, "wcolor"), ...common };
    };
    ov = {
      draw(g) {
        const layer = $(".pv-ov", root)!;
        layer.innerHTML = "";
        const o = read() as Record<string, unknown>;
        const mk = (cx: number, cy: number) => {
          const d = document.createElement("div");
          d.className = "ov-mark";
          d.style.opacity = String(o.opacity);
          d.style.left = `${cx}px`; d.style.top = `${cy}px`;
          d.style.transform = `translate(-50%,-50%) rotate(${-Number(o.angle)}deg)`;
          if (o.kind === "image") {
            if (!imgUrl) return;
            const w = Number(o.scale) * g.w * g.scale * (o.tiling === "tile" ? 0.5 : 1);
            d.innerHTML = `<img alt="" src="${imgUrl}" style="width:${w}px;height:${w / imgRatio}px">`;
          } else {
            d.textContent = String(o.text || "");
            d.style.fontSize = `${Number(o.font_size) * g.scale * (o.tiling === "tile" ? 0.55 : 1)}px`;
            d.style.color = String(o.color);
          }
          layer.appendChild(d);
        };
        const W = g.w * g.scale, H = g.h * g.scale;
        if (o.tiling === "tile") { for (let y = 0.15; y < 1; y += 0.3) for (let x = 0.2; x < 1; x += 0.33) mk(W * x, H * y); }
        else mk(W / 2, H / 2);
      },
    };
    body.addEventListener("click", async (e) => {
      if ((e.target as HTMLElement).closest("[data-pick-img]")) {
        const [p] = await pickFiles({ multiple: false, filters: [{ name: "그림", extensions: ["png", "jpg", "jpeg"] }] });
        if (p) {
          imgPath = p;
          imgUrl = await invoke<string | null>("image_data_url", { path: p }).catch(() => null);
          if (imgUrl) { const im = new Image(); im.onload = () => { imgRatio = im.naturalWidth / im.naturalHeight || 1; showPreview(); }; im.src = imgUrl; }
          $(".wimg-n", body)!.textContent = fileName(p);
        }
      }
      const k = segVal(body, "wkind");
      $$<HTMLElement>("[data-k]", body).forEach((d) => (d.hidden = d.dataset.k !== k));
      showPreview();
    });
    body.addEventListener("input", () => showPreview());
    apply.onclick = async () => {
      const o = read();
      if (o.kind === "text" && !(o as { text: string }).text) { toast("넣을 글자를 적어 주세요"); return; }
      if (o.kind === "image" && !imgPath) { toast("그림을 먼저 골라 주세요"); return; }
      const out = await toolJob("워터마크 넣는 중", "pdf_watermark", { opts: o });
      if (out) await finish(out, "워터마크");
    };
  },
  sign(body, apply) {
    apply.textContent = "서명 넣기";
    let imgPath: string | null = null, imgUrl: string | null = null, ratio = 2.5;
    let box: Box | null = null;
    let handle: ReturnType<typeof makeBox> | null = null;
    body.innerHTML = opt("서명 그림", `<button type="button" class="btn" data-pick-img>${I("image")}그림 고르기</button><span class="simg-n hint">서명·도장을 찍은 PNG·JPG</span>`) +
      `<div class="info">${I("info")}<span>그림을 고른 뒤 왼쪽 쪽 위에서 끌어 옮기고, 오른쪽 아래 모서리로 크기를 바꾸세요. 화살표 키로도 옮길 수 있어요.</span></div>` +
      opt("", chk("swhite", "흰 바탕을 투명하게", true)) +
      opt("넣을 쪽", seg("sscope", [["this", "이 쪽만"], ["all", "모든 쪽 같은 자리"]], "this")) +
      `<p class="hint s-where"></p>`;
    const where = () => {
      const t = $(".s-where", body);
      if (t) t.textContent = box ? `${previewPage + 1}쪽 · 왼쪽에서 ${Math.round(box.x / 2.835)}mm, 위에서 ${Math.round(box.y / 2.835)}mm, 너비 ${Math.round(box.w / 2.835)}mm` : "";
    };
    let geo: Geo | null = null;
    ov = {
      draw(g) {
        geo = g;
        const layer = $(".pv-ov", root)!;
        layer.innerHTML = "";
        if (!imgUrl) { layer.innerHTML = `<div class="ov-hint">오른쪽에서 서명 그림을 먼저 골라 주세요</div>`; return; }
        if (!box) { const w = g.w * 0.28; box = { x: g.w - w - 40, y: g.h - w / ratio - 60, w, h: w / ratio }; }
        handle = makeBox(layer, () => geo!, box, { cls: "sign", html: `<img alt="" src="${imgUrl}">`, ratio, handles: "corner", onChange: where });
        where();
      },
    };
    body.addEventListener("click", async (e) => {
      if (!(e.target as HTMLElement).closest("[data-pick-img]")) return;
      const [p] = await pickFiles({ multiple: false, filters: [{ name: "그림", extensions: ["png", "jpg", "jpeg"] }] });
      if (!p) return;
      imgPath = p;
      imgUrl = await invoke<string | null>("image_data_url", { path: p }).catch(() => null);
      $(".simg-n", body)!.textContent = fileName(p);
      if (!imgUrl) { toast("이 그림은 미리 볼 수 없어요(PNG·JPG를 써 주세요)"); return; }
      const im = new Image();
      im.onload = () => { ratio = im.naturalWidth / im.naturalHeight || 2.5; if (box) box.h = box.w / ratio; showPreview(); };
      im.src = imgUrl;
    });
    apply.onclick = async () => {
      if (!imgPath || !box) { toast("서명 그림을 먼저 골라 주세요"); return; }
      const all = segVal(body, "sscope") === "all";
      const pl = (pg: number) => ({ page: pg, x: box!.x, y: box!.y, w: box!.w, h: box!.h });
      const placements = all ? pages.map((_, i) => pl(i + 1)) : [pl(previewPage + 1)];
      const out = await toolJob("서명 넣는 중", "pdf_place_image", { opts: { image: imgPath, remove_white: checked(body, "swhite"), opacity: 1, placements } });
      if (out) await finish(out, `서명 · ${all ? "모든 쪽" : `${previewPage + 1}쪽`}`);
      void handle;
    };
  },
  text(body, apply) {
    apply.textContent = "글상자 넣기";
    interface TB { page: number; box: Box; text: string; size: number; color: string; bg: boolean }
    const boxes: TB[] = [];
    let cur = -1;
    let geo: Geo | null = null;
    body.innerHTML = `<div class="info">${I("info")}<span>[글상자 더하기]를 누르면 지금 보는 쪽에 상자가 생겨요. 끌어서 옮기고, 오른쪽 가장자리로 너비를 바꿔요.</span></div>
      <div class="row"><button type="button" class="btn" data-add>${I("plus")}글상자 더하기</button><button type="button" class="btn subtle" data-del disabled>${I("trash")}이 상자 지우기</button></div>
      <div class="tb-edit" hidden>${opt("글자", `<textarea class="txt area" id="tbtext" rows="3"></textarea>`)}${opt("크기", seg("tbsize", [["10", "작게"], ["12", "보통"], ["16", "크게"], ["24", "아주 크게"]], "12"))}${opt("색", seg("tbcolor", [["#000000", "검정"], ["#1F5BAA", "파랑"], ["#C42B1C", "빨강"]], "#000000"))}${opt("", chk("tbbg", "흰 바탕 깔기(아래 글자 가리기)", false))}</div>
      <p class="hint tb-count"></p>`;
    const sync = () => {
      const ed = $(".tb-edit", body)!, b = boxes[cur];
      ed.hidden = !b;
      ($<HTMLButtonElement>("[data-del]", body)!).disabled = !b;
      if (b) {
        ($<HTMLTextAreaElement>("#tbtext", body)!).value = b.text;
        $$<HTMLButtonElement>('[data-seg="tbsize"] button', body).forEach((x) => x.classList.toggle("on", x.dataset.v === String(b.size)));
        $$<HTMLButtonElement>('[data-seg="tbcolor"] button', body).forEach((x) => x.classList.toggle("on", x.dataset.v === b.color));
        ($<HTMLInputElement>("#tbbg", body)!).checked = b.bg;
      }
      $(".tb-count", body)!.textContent = boxes.length ? `글상자 ${boxes.length}개 (${[...new Set(boxes.map((x) => x.page))].sort((a, b) => a - b).join(", ")}쪽)` : "";
    };
    ov = {
      draw(g) {
        geo = g;
        const layer = $(".pv-ov", root)!;
        layer.innerHTML = "";
        boxes.forEach((b, i) => {
          if (b.page !== g.page) return;
          const h = makeBox(layer, () => geo!, b.box, {
            cls: `tbox ${i === cur ? "cur" : ""}`, handles: "right",
            html: `<div class="tb-t" style="font-size:${b.size * g.scale}px;color:${b.color};${b.bg ? "background:#fff;" : ""}">${esc(b.text || " ").replace(/\n/g, "<br>")}</div>`,
            onChange: () => {}, onFocus: () => { if (cur !== i) { cur = i; sync(); showPreview(); } },
          });
          // 높이는 글자에 맞춤
          h.el.style.height = "auto";
        });
      },
    };
    body.addEventListener("click", (e) => {
      const t = e.target as HTMLElement;
      if (t.closest("[data-add]") && geo) {
        boxes.push({ page: previewPage + 1, box: { x: geo.w * 0.15, y: geo.h * 0.15, w: geo.w * 0.4, h: 30 }, text: "여기에 글자를 넣으세요", size: 12, color: "#000000", bg: false });
        cur = boxes.length - 1;
        sync(); showPreview();
        ($<HTMLTextAreaElement>("#tbtext", body))?.select();
        return;
      }
      if (t.closest("[data-del]") && boxes[cur]) { boxes.splice(cur, 1); cur = -1; sync(); showPreview(); return; }
      const b = boxes[cur];
      if (b && t.closest("[data-seg]")) { b.size = Number(segVal(body, "tbsize")); b.color = segVal(body, "tbcolor"); showPreview(); }
    });
    body.addEventListener("input", (e) => {
      const b = boxes[cur];
      if (!b) return;
      if ((e.target as HTMLElement).id === "tbtext") b.text = (e.target as HTMLTextAreaElement).value;
      b.bg = checked(body, "tbbg");
      showPreview();
    });
    body.addEventListener("change", () => { const b = boxes[cur]; if (b) { b.bg = checked(body, "tbbg"); showPreview(); } });
    apply.onclick = async () => {
      const list = boxes.filter((b) => b.text.trim());
      if (!list.length) { toast("글상자를 먼저 더해 주세요"); return; }
      const out = await toolJob("글상자 넣는 중", "pdf_text_boxes", {
        boxes: list.map((b) => ({ page: b.page, x: b.box.x, y: b.box.y + b.size * 0.2, text: b.text, font_size: b.size, color: b.color, background: b.bg ? "#FFFFFF" : null, width: b.box.w })),
      });
      if (out) await finish(out, `글상자 ${list.length}개`);
    };
  },
  crop(body, apply) {
    apply.textContent = "여백 자르기";
    let box: Box | null = null;
    let geo: Geo | null = null;
    body.innerHTML = opt("방법", seg("cmode", [["manual", "직접 고르기"], ["auto", "자동으로"]], "manual")) +
      `<div data-c="manual"><div class="info">${I("info")}<span>왼쪽 쪽 위 파란 상자의 가장자리를 끌어서 남길 부분을 고르세요.</span></div>${opt("자를 쪽", seg("cscope", [["all", "모든 쪽"], ["this", "이 쪽만"]], "all"))}<p class="hint c-mm"></p></div>
      <div data-c="auto" hidden><div class="info">${I("info")}<span>쪽마다 글자·그림이 있는 곳을 찾아 바깥 여백을 잘라요.</span></div>${opt("남길 여유", seg("cpad", [["4", "조금"], ["12", "보통"], ["24", "넉넉히"]], "12"))}</div>`;
    const mm = () => {
      const t = $(".c-mm", body);
      if (!t || !box || !geo) return;
      const m = (v: number) => Math.round(v / 2.835);
      t.textContent = `자르는 폭(mm): 위 ${m(box.y)} · 오른쪽 ${m(geo.w - box.x - box.w)} · 아래 ${m(geo.h - box.y - box.h)} · 왼쪽 ${m(box.x)}`;
    };
    ov = {
      draw(g) {
        geo = g;
        const layer = $(".pv-ov", root)!;
        layer.innerHTML = "";
        if (segVal(body, "cmode") !== "manual") return;
        if (!box || box.w > g.w || box.h > g.h) box = { x: g.w * 0.06, y: g.h * 0.06, w: g.w * 0.88, h: g.h * 0.88 };
        makeBox(layer, () => geo!, box, { cls: "crop", handles: "edges", onChange: mm });
        mm();
      },
    };
    body.addEventListener("click", () => {
      const m = segVal(body, "cmode");
      $$<HTMLElement>("[data-c]", body).forEach((d) => (d.hidden = d.dataset.c !== m));
      showPreview();
    });
    apply.onclick = async () => {
      let opts: Record<string, unknown>;
      if (segVal(body, "cmode") === "auto") opts = { mode: "auto", padding: Number(segVal(body, "cpad")) };
      else {
        if (!box || !geo) return;
        opts = { mode: "margins", top: box.y, left: box.x, right: geo.w - box.x - box.w, bottom: geo.h - box.y - box.h, pages: segVal(body, "cscope") === "this" ? String(previewPage + 1) : null };
      }
      const out = await toolJob("여백 자르는 중", "pdf_crop", { opts });
      if (out) await finish(out, "여백 자르기");
    };
  },
  a4(body, apply) {
    apply.textContent = "A4로 맞추기";
    body.innerHTML = `<div class="info">${I("info")}<span>쪽마다 A4 한 장 가운데에 맞춰요. 인쇄할 때 잘리지 않게 할 때 좋아요.</span></div>` +
      opt("방향", seg("aor", [["auto", "쪽에 맞게"], ["portrait", "세로"], ["landscape", "가로"]], "auto")) +
      opt("여백", seg("amar", [["0", "없음"], ["14", "좁게"], ["28", "넓게"]], "0")) +
      opt("", chk("aen", "작은 쪽은 A4에 맞게 키우기", true));
    apply.onclick = async () => {
      const out = await toolJob("A4로 맞추는 중", "pdf_fit_a4", { opts: { orientation: segVal(body, "aor"), margin: Number(segVal(body, "amar")), enlarge: checked(body, "aen") } });
      if (out) await finish(out, "A4로 맞추기");
    };
  },
  nup(body, apply) {
    apply.textContent = "모아 찍기";
    body.innerHTML = `<div class="info">${I("info")}<span>여러 쪽을 종이 한 장에 작게 모아요. 종이를 아낄 때 좋아요.</span></div>` +
      opt("한 장에", seg("per", [["2", "2쪽"], ["4", "4쪽"]], "2")) +
      opt("순서", seg("ord", [["across", "가로 먼저"], ["down", "세로 먼저"]], "across")) +
      `<div class="nup-pic" aria-hidden="true"></div>`;
    const pic = () => {
      const n = Number(segVal(body, "per")), down = segVal(body, "ord") === "down";
      const order = n === 2 ? [1, 2] : down ? [1, 3, 2, 4] : [1, 2, 3, 4];
      $(".nup-pic", body)!.innerHTML = `<div class="sheet n${n}">${order.map((x) => `<span>${x}</span>`).join("")}</div>`;
    };
    pic();
    body.addEventListener("click", pic);
    apply.onclick = async () => {
      const out = await toolJob("모아 찍는 중", "pdf_nup", { opts: { per_sheet: Number(segVal(body, "per")), orientation: "auto", order: segVal(body, "ord"), gap: 8, margin: 14 } });
      if (out) await finish(out, "모아 찍기");
    };
  },
  lock(body, apply) {
    apply.textContent = "암호 걸기";
    const opened = !!doc().password;
    body.innerHTML = (opened ? `<div class="note">${I("unlock")}<span>이 파일은 암호를 넣어 열었어요. [암호 없이 저장]을 누르면 암호를 푼 새 파일을 만들어요.</span><button type="button" class="btn" data-unlock>암호 없이 저장</button></div>` : "") +
      opt("새 암호", `<input type="password" class="txt" id="pw1" autocomplete="new-password">`, "비워 두면 누구나 열 수 있고, 아래 제한만 걸려요") +
      opt("한 번 더", `<input type="password" class="txt" id="pw2" autocomplete="new-password">`) +
      opt("막을 것", `${chk("no-print", "인쇄", false)}${chk("no-copy", "글자 복사", false)}${chk("no-edit", "고치기", false)}`) +
      `<p class="hint">암호를 잊으면 아무도 열 수 없어요. 꼭 따로 적어 두세요.</p>`;
    $("[data-unlock]", body)?.addEventListener("click", async () => {
      const out = await withJob("암호 푸는 중", async () => invoke<EditOut>("pdf_unprotect", { path: doc().path, password: doc().password, outDir: null }));
      if (out) await finish(out, "암호 풀기");
    });
    apply.onclick = async () => {
      const p1 = ($<HTMLInputElement>("#pw1", body)!).value, p2 = ($<HTMLInputElement>("#pw2", body)!).value;
      const noP = checked(body, "no-print"), noC = checked(body, "no-copy"), noE = checked(body, "no-edit");
      if (!p1 && !noP && !noC && !noE) { toast("암호를 넣거나 막을 것을 골라 주세요"); return; }
      if (p1 !== p2) { toast("두 칸의 암호가 달라요"); return; }
      const out = await withJob("암호 거는 중", async () => invoke<EditOut>("pdf_protect", {
        path: doc().path, password: doc().password, outDir: null,
        opts: { open_password: p1, owner_password: "", allow_print: !noP, allow_copy: !noC, allow_edit: !noE, cipher: "aes256" },
      }));
      if (out) await finish(out, p1 ? "암호 걸기" : "제한 걸기");
    };
  },
  info(body, apply) {
    apply.textContent = "정보 저장";
    body.innerHTML = `<p class="hint">읽는 중…</p>`;
    const F: [string, string][] = [["title", "제목"], ["author", "작성자"], ["subject", "주제"], ["keywords", "키워드"]];
    invoke<Record<string, string | null>>("pdf_info_get", { path: doc().path, password: doc().password }).then((inf) => {
      body.innerHTML = `<div class="info">${I("info")}<span>파일 속성에 보이는 정보예요. 남에게 보내기 전에 작성자 이름을 지울 수 있어요.</span></div>` +
        F.map(([k, l]) => opt(l, `<input class="txt wide" id="i-${k}" value="${esc(inf[k] ?? "")}">`)).join("") +
        `<p class="hint">만든 프로그램: ${esc(inf.creator ?? "-")} · ${esc(inf.producer ?? "-")}</p>
        <div class="row"><button type="button" class="btn" data-clear>${I("trash")}정보 모두 지우기</button></div>`;
      $("[data-clear]", body)!.addEventListener("click", async () => {
        const out = await withJob("정보 지우는 중", async () => invoke<EditOut>("pdf_info_clear", { path: doc().path, password: doc().password, outDir: null }));
        if (out) await finish(out, "문서 정보 지우기");
      });
    }).catch((e) => { body.innerHTML = errorPanel(friendly(e)); });
    apply.onclick = async () => {
      const info: Record<string, string> = {};
      for (const [k] of F) { const i = $<HTMLInputElement>(`#i-${k}`, body); if (i) info[k] = i.value.trim(); }
      const out = await withJob("정보 저장하는 중", async () => invoke<EditOut>("pdf_info_set", { path: doc().path, password: doc().password, info, outDir: null }));
      if (out) await finish(out, "문서 정보");
    };
  },
  form(body, apply) {
    apply.textContent = "채워서 저장";
    apply.disabled = true;
    body.innerHTML = `<p class="hint">빈칸을 찾는 중…</p>`;
    interface Field { name: string; kind: string; value: string | null; options: string[]; read_only: boolean; page?: number }
    let fields: Field[] = [];
    invoke<Field[]>("pdf_form_list", { path: doc().path, password: doc().password }).then((fs) => {
      fields = fs.filter((f) => !["button", "signature"].includes(f.kind));
      if (!fields.length) { body.innerHTML = empty("form", "이 PDF에는 채울 빈칸이 없어요", "글자를 얹으려면 [글상자]를 쓰세요."); return; }
      apply.disabled = false;
      body.innerHTML = `<div class="form-list">${fields.map((f, i) => {
        const id = `f${i}`;
        const label = `${esc(f.name)}${f.page ? ` <span class="hint">${f.page}쪽</span>` : ""}`;
        const dis = f.read_only ? "disabled" : "";
        if (f.kind === "checkbox") return opt(label, chk(id, "체크", f.value != null && !/^(off|false|)$/i.test(f.value), dis));
        if (f.kind === "radio" || f.kind === "combo" || f.kind === "list")
          return opt(label, `<select class="txt" id="${id}" ${dis}>${f.kind === "radio" ? `<option value="Off">고르지 않음</option>` : `<option value=""></option>`}${f.options.map((o) => `<option ${o === f.value ? "selected" : ""}>${esc(o)}</option>`).join("")}</select>`);
        return opt(label, `<input class="txt wide" id="${id}" value="${esc(f.value ?? "")}" ${dis}>`);
      }).join("")}</div>${opt("", chk("fflat", "채운 뒤 고칠 수 없게 굳히기", false))}`;
    }).catch((e) => { body.innerHTML = errorPanel(friendly(e)); });
    apply.onclick = async () => {
      const values: Record<string, string | boolean> = {};
      fields.forEach((f, i) => {
        if (f.read_only) return;
        const el = $<HTMLInputElement>(`#f${i}`, body);
        if (!el) return;
        values[f.name] = f.kind === "checkbox" ? el.checked : el.value;
      });
      const out = await toolJob("양식 채우는 중", "pdf_form_fill", { values, flatten: checked(body, "fflat") });
      if (out) await finish(out, "양식 채우기");
    };
  },
  flat(body, apply) {
    apply.textContent = "평평하게 저장";
    body.innerHTML = `<div class="info">${I("info")}<span>양식 빈칸과 메모·도장 같은 주석을 쪽 내용으로 굳혀요. 어느 프로그램에서 열어도 똑같이 보이고, 더는 고칠 수 없어요.</span></div>`;
    apply.onclick = async () => {
      const out = await toolJob("평평하게 만드는 중", "pdf_flatten", {});
      if (out) await finish(out, "평평하게");
    };
  },
};

// ---------- 화면 ----------

export const pdfEditScreen: Screen = {
  id: "pdfedit", title: "PDF 편집", icon: "layers",
  init(el) {
    root = el;
    bindResultButtons(root);
    root.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", onPointerUp);
    window.addEventListener("pointercancel", (e) => { if (drag) { drag.started = false; drag.ghost?.remove(); root.classList.remove("dragging"); $$(".pcell.drop-b, .pcell.drop-a", root).forEach((c) => c.classList.remove("drop-b", "drop-a")); drag = null; } void e; });
    root.addEventListener("dragstart", (e) => { if ((e.target as HTMLElement).closest?.(".pgrid")) e.preventDefault(); });
    root.addEventListener("keydown", (e) => {
      if ((e.target as HTMLElement).closest(".pgrid")) onGridKey(e);
      if ((e.target as HTMLElement).id === "pe-dz" && (e.key === "Enter" || e.key === " ")) { e.preventDefault(); pickAndOpen(); }
    });
    root.addEventListener("click", async (e) => {
      const t = e.target as HTMLElement;
      const tool = t.closest<HTMLElement>("[data-tool]");
      if (tool) { openTool(tool.dataset.tool!); return; }
      const b = t.closest<HTMLElement>("[data-act]");
      if (!b) { if (t.closest("#pe-dz")) pickAndOpen(); return; }
      switch (b.dataset.act) {
        case "open": pickAndOpen(); break;
        case "rotl": rotate(-90); break;
        case "rotr": rotate(90); break;
        case "del": remove(); break;
        case "extract": extractSelected(); break;
        case "blank": insertBlank(); break;
        case "insert": case "append": {
          const [p] = await pickFiles({ multiple: false, filters: [{ name: "PDF", extensions: ["pdf"] }] });
          if (p) addPdf(p, b.dataset.act === "append");
          break;
        }
        case "undo": undo(); break;
        case "redo": redo(); break;
        case "save": save(); break;
        case "back": mode = "grid"; renderMain(); break;
        case "pv-prev": if (previewPage > 0) { previewPage--; showPreview(); } break;
        case "pv-next": if (previewPage < pages.length - 1) { previewPage++; showPreview(); } break;
        case "close-result": lastOut = null; renderResult(); break;
        case "continue": if (lastOut) { const p = lastOut.output; lastOut = null; await reopen(p); mode = "grid"; render(); } break;
      }
    });
    document.addEventListener("keydown", (e) => {
      if (root.hidden || root.closest("section")?.hidden || !sources.length || document.querySelector(".dlg-back")) return;
      if ((e.target as HTMLElement).matches?.("input, textarea, select")) return;
      const k = e.key.toLowerCase();
      if (e.ctrlKey && k === "z") { e.preventDefault(); undo(); }
      else if (e.ctrlKey && (k === "y" || (e.shiftKey && k === "z"))) { e.preventDefault(); redo(); }
      else if (e.ctrlKey && k === "s") { e.preventDefault(); if (mode === "grid") save(); }
      else if (e.ctrlKey && k === "a" && mode === "grid") { e.preventDefault(); sel = new Set(pages.map((p) => p.key)); renderGrid(); }
    });
    window.addEventListener("resize", () => { if (mode !== "grid" && !root.hidden) showPreview(); });
    render();
  },
  async show(arg) {
    const a = arg as { open?: string[]; tool?: string } | undefined;
    if (!a?.open?.length) return;
    await openDoc(a.open[0]);
    for (const p of a.open.slice(1)) await addPdf(p, true);
    if (a.tool && sources.length) { pendingTool = a.tool; await openTool(pendingTool); pendingTool = null; }
  },
  drop(paths) {
    const pdfList = paths.filter((p) => p.toLowerCase().endsWith(".pdf"));
    if (!pdfList.length) { toast("PDF만 열 수 있어요"); return; }
    (async () => {
      if (!sources.length) { await openDoc(pdfList[0]); for (const p of pdfList.slice(1)) await addPdf(p, true); }
      else if (mode === "grid") for (const p of pdfList) await addPdf(p, true);
      else toast("쪽 목록으로 돌아가서 넣어 주세요");
    })();
  },
};

async function pickAndOpen() {
  const ps = await pickFiles({ multiple: true, filters: [{ name: "PDF", extensions: ["pdf"] }] });
  if (!ps.length) return;
  await openDoc(ps[0]);
  for (const p of ps.slice(1)) await addPdf(p, true);
}

void human; void dialog;
