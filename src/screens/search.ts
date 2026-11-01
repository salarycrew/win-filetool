// 검색: 지금 기능 그대로(가상 스크롤·정렬·열 너비·셸 아이콘·오른쪽 메뉴), 시안 A 모양(32px 줄, 선택 줄 왼쪽 막대, 상태 칩)
import { fmtTime, friendly, human, invoke, num } from "../api";
import { FT, I, extOf } from "../icons";
import { Screen } from "../router";
import { $, $$, esc } from "../ui";

interface Row { id: number; name: string; folder: string; path: string; is_dir: boolean; size: number; mtime: number }
interface SearchResult { total: number; millis: number; generation: number; version: number }
interface RootStatus { path: string; scanning: boolean; scanned_so_far: number; last_scan: number; source: string; source_note: string | null; catchup: string | null }
interface SearchStatus { entries: number; roots: RootStatus[]; loaded_ms: number | null; load_error: string | null; version: number; generation: number; ready: boolean; live_updates: number; skipped: number; excluded: number }
interface SearchSettings { roots: string[]; excludes: string[]; hotkey_enabled: boolean; hotkey: string }
export interface SearchView { status: SearchStatus; settings: SearchSettings; effective_roots: string[]; service_roots: string[] }
type SortKey = "name" | "path" | "size" | "modified";

const ROW_H = 32;
let root: HTMLElement;
let qEl: HTMLInputElement, resEl: HTMLElement, rowsEl: HTMLElement, spacerEl: HTMLElement, ctxEl: HTMLElement, infoEl: HTMLElement, emptyEl: HTMLElement;
let sortKey: SortKey = "name";
let sortDesc = false;
let total = 0;
let sel = -1;
let rowCache = new Map<number, Row>();
let fetching = new Set<number>();
let searchSeq = 0;
let lastQueryAt = 0;
let lastVersion = -1;
let lastGeneration = -1;
let statusTimer: number | undefined;
let visible = false;
let colWidths: number[] | null = null;
let resizedAt = 0;
const HINTS = ["보고서 ext:pdf", "ㅂㄱㅅ", "*.jpg", "size:>10mb", "dm:today"];

async function runSearch(keepScroll = false) {
  const seq = ++searchSeq;
  const q = qEl.value;
  lastQueryAt = Date.now();
  try {
    const r = await invoke<SearchResult>("search_query", { query: q, sort: sortKey, desc: sortDesc });
    if (seq !== searchSeq) return;
    total = r.total; lastVersion = r.version; lastGeneration = r.generation;
    rowCache = new Map(); fetching = new Set();
    if (!keepScroll) { resEl.scrollTop = 0; sel = total > 0 ? 0 : -1; } else if (sel >= total) sel = total - 1;
    infoEl.className = "q-info";
    infoEl.textContent = q.trim() ? `${num(total)}개 · ${r.millis < 10 ? r.millis.toFixed(1) : Math.round(r.millis)} ms` : "";
    showEmpty(q);
    spacerEl.style.height = `${total * ROW_H}px`;
    renderRows();
  } catch (e) {
    if (seq !== searchSeq) return;
    infoEl.className = "q-info err";
    infoEl.textContent = friendly(e).title;
  }
}
function showEmpty(q: string) {
  if (!q.trim()) {
    emptyEl.hidden = false;
    emptyEl.innerHTML = `<span class="big">${I("search")}</span><b>찾을 파일 이름을 넣어 보세요</b><span>이 PC 전체에서 바로 찾아요. 이렇게도 찾을 수 있어요:</span><div class="row center">${HINTS.map((h) => `<button type="button" class="qchip" data-q="${esc(h)}">${esc(h)}</button>`).join("")}</div>`;
  } else if (total === 0) {
    emptyEl.hidden = false;
    emptyEl.innerHTML = `<span class="big">${I("search")}</span><b>찾은 파일이 없어요</b><span>철자를 확인하거나 조건을 줄여 보세요. 이렇게도 찾을 수 있어요:</span><div class="row center">${HINTS.map((h) => `<button type="button" class="qchip" data-q="${esc(h)}">${esc(h)}</button>`).join("")}</div>`;
  } else emptyEl.hidden = true;
}

function hl(name: string, q: string): string {
  const words = q.split(/\s+/).filter((w) => w && !w.includes(":") && !/[*?]/.test(w)).map((w) => w.toLowerCase());
  if (!words.length) return esc(name);
  const low = name.toLowerCase();
  const marks: [number, number][] = [];
  for (const w of words) { const i = low.indexOf(w); if (i >= 0) marks.push([i, i + w.length]); }
  if (!marks.length) return esc(name);
  marks.sort((a, b) => a[0] - b[0]);
  let out = "", at = 0;
  for (const [a, b] of marks) { if (a < at) continue; out += esc(name.slice(at, a)) + `<mark>${esc(name.slice(a, b))}</mark>`; at = b; }
  return out + esc(name.slice(at));
}

function renderRows() {
  if (!visible) return;
  const top = resEl.scrollTop;
  const first = Math.max(0, Math.floor(top / ROW_H) - 5);
  const last = Math.min(total, Math.ceil((top + resEl.clientHeight) / ROW_H) + 5);
  const missing: number[] = [];
  const q = qEl.value;
  let html = "";
  for (let i = first; i < last; i++) {
    const r = rowCache.get(i);
    if (!r) { missing.push(i); html += `<div class="tr" style="top:${i * ROW_H}px"></div>`; continue; }
    const k = iconKey(r);
    const url = iconCache.get(k);
    if (url === undefined) iconWant.add(k);
    const ico = url ? `<img class="ico" alt="" src="${url}">` : FT(r.is_dir ? "folder" : extOf(r.name), 20);
    html += `<div class="tr ${i === sel ? "sel" : ""}" style="top:${i * ROW_H}px" data-i="${i}" role="row" aria-selected="${i === sel}">
      <span class="nm" title="${esc(r.path)}">${ico}<em>${hl(r.name, q)}</em></span><span class="fd" title="${esc(r.folder)}">${esc(r.folder)}</span>
      <span class="r">${r.is_dir ? "" : human(r.size)}</span><span class="dt">${fmtTime(r.mtime)}</span></div>`;
  }
  rowsEl.innerHTML = html;
  if (missing.length) fetchRows(missing[0], missing[missing.length - 1] + 1);
  if (iconWant.size && !iconBusy) fetchIcons();
}

const iconCache = new Map<string, string | null>();
const iconWant = new Set<string>();
let iconBusy = false;
function iconKey(r: Row) {
  if (r.is_dir) return "/dir";
  const d = r.name.lastIndexOf(".");
  return d > 0 && d < r.name.length - 1 ? r.name.slice(d + 1).toLowerCase() : "";
}
async function fetchIcons() {
  const keys = [...iconWant];
  iconWant.clear();
  iconBusy = true;
  try {
    const got = await invoke<Record<string, string | null>>("file_icons", { keys });
    for (const k of keys) iconCache.set(k, got[k] ?? null);
  } catch { for (const k of keys) iconCache.set(k, null); } finally { iconBusy = false; }
  renderRows();
}
async function fetchRows(from: number, to: number) {
  const start = Math.floor(from / 100) * 100, end = Math.ceil(to / 100) * 100;
  if (fetching.has(start)) return;
  fetching.add(start);
  const seq = searchSeq;
  try {
    const out = await invoke<{ rows: Row[]; stale: boolean }>("search_rows", { offset: start, limit: end - start });
    if (seq !== searchSeq) return;
    if (out.stale) { runSearch(true); return; }
    out.rows.forEach((r, k) => rowCache.set(start + k, r));
    renderRows();
  } catch { /* 다음 스크롤에 다시 */ } finally { fetching.delete(start); }
}
function selectRow(i: number, scroll = true) {
  if (!total) return;
  sel = Math.max(0, Math.min(total - 1, i));
  if (scroll) {
    const y = sel * ROW_H;
    if (y < resEl.scrollTop) resEl.scrollTop = y;
    else if (y + ROW_H > resEl.scrollTop + resEl.clientHeight) resEl.scrollTop = y + ROW_H - resEl.clientHeight;
  }
  renderRows();
}
async function act(what: string, i = sel) {
  const r = rowCache.get(i);
  ctxEl.hidden = true;
  if (!r) return;
  try {
    if (what === "open") await invoke("open_path", { path: r.path });
    else if (what === "folder") await invoke("open_folder", { path: r.path });
    else if (what === "copy-path") await copyText(r.path);
    else if (what === "copy-name") await copyText(r.name);
  } catch (e) { infoEl.className = "q-info err"; infoEl.textContent = friendly(e).title; }
}
async function copyText(t: string) {
  try { await navigator.clipboard.writeText(t); } catch {
    const ta = document.createElement("textarea"); ta.value = t; document.body.appendChild(ta); ta.select(); document.execCommand("copy"); ta.remove();
  }
  infoEl.className = "q-info"; infoEl.textContent = "복사했어요: " + (t.length > 50 ? "…" + t.slice(-50) : t);
}
function showSort() {
  $$<HTMLButtonElement>(".th button[data-sort]", root).forEach((x) => {
    const on = x.dataset.sort === sortKey;
    x.classList.toggle("on", on);
    x.classList.toggle("desc", on && sortDesc);
    x.setAttribute("aria-sort", on ? (sortDesc ? "descending" : "ascending") : "none");
  });
}
function applyCols() {
  root.style.setProperty("--res-cols", colWidths ? `${colWidths[0]}px minmax(120px, 1fr) ${colWidths[1]}px ${colWidths[2]}px` : "");
}

async function refreshStatus() {
  let v: SearchView;
  try { v = await invoke<SearchView>("search_status"); } catch { return; }
  const st = v.status;
  const scanning = st.roots.filter((r) => r.scanning);
  const svc = st.roots.some((r) => /서비스/.test(r.source));
  const chip = scanning.length ? `<span class="chip warn"><i></i>색인 만드는 중 ${scanning.map((r) => `${esc(r.path)} ${num(r.scanned_so_far)}`).join(", ")}</span>`
    : svc ? `<span class="chip"><i></i>${esc(st.roots.map((r) => `${r.path} ${r.source}`).join(", "))}</span>`
    : `<span class="chip gray"><i></i>${esc(st.roots.map((r) => r.path).join(", ") || "색인")} 앱 색인</span>`;
  const el = $(".status", root)!;
  el.innerHTML = `${chip}<span class="sp"></span><span>전체 ${num(st.entries)}개${st.live_updates ? " · 바뀐 파일 바로 반영 중" : ""}</span>`;
  const notes = st.roots.flatMap((r) => [r.source_note, r.catchup].filter(Boolean).map((m) => `${r.path}: ${m}`));
  el.title = [st.load_error, ...notes].filter(Boolean).join("\n");
  if (visible && qEl.value.trim() && (st.version !== lastVersion || st.generation !== lastGeneration) && Date.now() - lastQueryAt > 900) runSearch(true);
  const kbd = $(".kbd", root)!;
  kbd.hidden = !v.settings.hotkey_enabled || !v.settings.hotkey;
  kbd.textContent = v.settings.hotkey;
}

export const searchScreen: Screen = {
  id: "search", title: "검색", icon: "search", flush: true,
  init(el) {
    root = el;
    root.classList.add("search-screen");
    root.innerHTML = `<div class="sbox">${I("search")}<input id="q" type="search" placeholder="파일 이름으로 찾기 — 예: 보고서 ext:pdf · ㅂㄱㅅ · size:>10mb" autocomplete="off" spellcheck="false" aria-label="파일 이름 검색"><span class="q-info" aria-live="polite"></span><span class="kbd" hidden></span></div>
      <div class="tbl res-tbl"><div class="th" role="row">
        <button type="button" data-sort="name">이름${I("down", "sorti")}</button><button type="button" data-sort="path">폴더${I("down", "sorti")}</button>
        <button type="button" data-sort="size" class="r">크기${I("down", "sorti")}</button><button type="button" data-sort="modified">수정한 날짜${I("down", "sorti")}</button></div>
        <div class="tb res" tabindex="0" role="grid" aria-label="검색 결과"><div class="res-spacer"></div><div class="res-rows"></div></div>
        <div class="empty res-empty" hidden></div></div>
      <div class="status"></div>
      <div class="ctx" hidden role="menu"><button type="button" data-act="open" role="menuitem">열기<span>Enter</span></button><button type="button" data-act="folder" role="menuitem">폴더 열기<span>Ctrl+Enter</span></button>
        <button type="button" data-act="copy-path" role="menuitem">경로 복사</button><button type="button" data-act="copy-name" role="menuitem">이름 복사</button></div>`;
    qEl = $<HTMLInputElement>("#q", root)!; resEl = $(".res", root)!; rowsEl = $(".res-rows", root)!; spacerEl = $(".res-spacer", root)!;
    ctxEl = $(".ctx", root)!; infoEl = $(".q-info", root)!; emptyEl = $(".res-empty", root)!;
    let typing = 0;
    qEl.addEventListener("input", () => { window.clearTimeout(typing); typing = window.setTimeout(() => runSearch(), 60); });
    const keyNav = (e: KeyboardEvent) => {
      const page = Math.max(1, Math.floor(resEl.clientHeight / ROW_H) - 1);
      if (e.key === "ArrowDown") selectRow(sel + 1);
      else if (e.key === "ArrowUp") selectRow(sel - 1);
      else if (e.key === "PageDown") selectRow(sel + page);
      else if (e.key === "PageUp") selectRow(sel - page);
      else if (e.key === "Enter") act(e.ctrlKey ? "folder" : "open");
      else if (e.key === "Escape") { ctxEl.hidden = true; return; }
      else return;
      e.preventDefault();
    };
    qEl.addEventListener("keydown", keyNav);
    resEl.addEventListener("keydown", (e) => {
      if (e.key === "Home") { selectRow(0); e.preventDefault(); return; }
      if (e.key === "End") { selectRow(total - 1); e.preventDefault(); return; }
      keyNav(e);
    });
    resEl.addEventListener("scroll", () => renderRows());
    window.addEventListener("resize", () => renderRows());
    rowsEl.addEventListener("mousedown", (e) => { const r = (e.target as HTMLElement).closest<HTMLElement>(".tr[data-i]"); if (r) selectRow(Number(r.dataset.i), false); });
    rowsEl.addEventListener("dblclick", (e) => { const r = (e.target as HTMLElement).closest<HTMLElement>(".tr[data-i]"); if (r) act("open", Number(r.dataset.i)); });
    rowsEl.addEventListener("contextmenu", (e) => {
      const r = (e.target as HTMLElement).closest<HTMLElement>(".tr[data-i]");
      if (!r) return;
      e.preventDefault();
      selectRow(Number(r.dataset.i), false);
      ctxEl.hidden = false;
      ctxEl.style.left = `${Math.min(e.clientX, window.innerWidth - 210)}px`;
      ctxEl.style.top = `${Math.min(e.clientY, window.innerHeight - 160)}px`;
      ($("button", ctxEl) as HTMLButtonElement).focus();
    });
    ctxEl.addEventListener("click", (e) => { const b = (e.target as HTMLElement).closest<HTMLButtonElement>("button"); if (b) act(b.dataset.act!); });
    ctxEl.addEventListener("keydown", (e) => {
      const bs = $$<HTMLButtonElement>("button", ctxEl), i = bs.indexOf(document.activeElement as HTMLButtonElement);
      if (e.key === "ArrowDown") { bs[(i + 1) % bs.length].focus(); e.preventDefault(); }
      else if (e.key === "ArrowUp") { bs[(i + bs.length - 1) % bs.length].focus(); e.preventDefault(); }
      else if (e.key === "Escape") { ctxEl.hidden = true; resEl.focus(); }
    });
    document.addEventListener("mousedown", (e) => { if (!ctxEl.contains(e.target as Node)) ctxEl.hidden = true; });
    emptyEl.addEventListener("click", (e) => { const b = (e.target as HTMLElement).closest<HTMLElement>("[data-q]"); if (b) { qEl.value = b.dataset.q!; runSearch(); qEl.focus(); } });
    $$<HTMLButtonElement>(".th button", root).forEach((b) => {
      b.onclick = () => {
        if (Date.now() - resizedAt < 300) return;
        const k = b.dataset.sort as SortKey;
        if (k === sortKey) sortDesc = !sortDesc; else { sortKey = k; sortDesc = k === "size" || k === "modified"; }
        showSort();
        invoke("ui_set", { patch: { sort_key: sortKey, sort_desc: sortDesc } }).catch(() => {});
        runSearch();
      };
      const COLS = ["name", "size", "modified"];
      const idx = COLS.indexOf(b.dataset.sort!);
      if (idx < 0) return;
      const grip = document.createElement("span");
      grip.className = "grip";
      grip.title = "끌어서 열 너비 바꾸기";
      b.appendChild(grip);
      grip.addEventListener("mousedown", (e) => {
        e.preventDefault(); e.stopPropagation();
        const heads = $$<HTMLButtonElement>(".th button", root);
        const cur = COLS.map((c) => heads.find((h) => h.dataset.sort === c)!.getBoundingClientRect().width);
        const x0 = e.clientX, w0 = cur[idx];
        const move = (ev: MouseEvent) => { colWidths = cur.map((v, j) => (j === idx ? Math.max(60, Math.min(1200, Math.round(w0 + ev.clientX - x0))) : Math.round(v))); applyCols(); renderRows(); };
        const up = () => { document.removeEventListener("mousemove", move); document.removeEventListener("mouseup", up); resizedAt = Date.now(); if (colWidths) invoke("ui_set", { patch: { col_widths: colWidths } }).catch(() => {}); };
        document.addEventListener("mousemove", move);
        document.addEventListener("mouseup", up);
      });
    });
    invoke<{ sort_key?: string; sort_desc?: boolean; col_widths?: number[] }>("ui_get").then((ui) => {
      if (ui.sort_key && ["name", "path", "size", "modified"].includes(ui.sort_key)) { sortKey = ui.sort_key as SortKey; sortDesc = !!ui.sort_desc; }
      if (ui.col_widths?.length === 3) { colWidths = ui.col_widths; applyCols(); }
      showSort();
    }).catch(() => showSort());
    showSort();
    showEmpty("");
  },
  show(arg) {
    visible = true;
    const q = (arg as { query?: string } | undefined)?.query;
    if (q) { qEl.value = q; runSearch(); }
    qEl.focus();
    qEl.select();
    refreshStatus();
    if (statusTimer === undefined) statusTimer = window.setInterval(() => { if (visible) refreshStatus(); }, 1000);
    renderRows();
  },
  hide() { visible = false; ctxEl.hidden = true; },
};
