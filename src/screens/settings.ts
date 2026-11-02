// 설정: 설정 하나 = 카드 하나(Win11). 앞에는 4개(테마·윈도 시작·빠른 검색 서비스·검색 단축키), 나머지는 [고급] 안에.
import { friendly, human, invoke, num, pickFiles } from "../api";
import { I } from "../icons";
import { Screen } from "../router";
import { $, bindSegs, confirmDlg, esc, seg, toast, toggle } from "../ui";
import { themePref, setThemePref, ThemePref } from "../theme";
import { resetWelcome } from "../welcome";
import { go } from "../router";
import { showBanner, UpdateOutcome, UpdateView } from "../update";
import { docToolsStatus, download as dtDownload, installFromFile } from "../doctools";

interface AutostartStatus { enabled: boolean; value: string | null; matches_exe: boolean; supported: boolean }
interface CtxStatus { enabled: boolean; matches_exe: boolean; registered_exe: string | null; supported: boolean }
interface ServiceInfo { installed: boolean; state: string; running: boolean; start_type: string; account: string }
interface SvcPanel { info: ServiceInfo; connected: boolean; entries: number; ready: boolean; roots: string[]; visibility: string; error: string | null; exe: string | null; install_cmd: string | null; uninstall_cmd: string | null; bundled_version: string; installed_version: string | null; update_available: boolean }
interface SearchSettings { roots: string[]; excludes: string[]; hotkey_enabled: boolean; hotkey: string }
interface RootStatus { path: string; scanning: boolean; source: string; last_scan: number; stats: { dirs: number; files: number } }
interface SearchView { status: { entries: number; roots: RootStatus[]; index_file: string; index_bytes: number }; settings: SearchSettings; effective_roots: string[] }
interface ToolStatus { tool: string; label: string; found: boolean; path: string | null; source: string | null; version: string | null }
interface Bundle { present: boolean; tools: { name: string; ready: boolean; bytes: number }[] }

let root: HTMLElement;
let advOpen = false;
let view: SearchView | null = null;
let hotkeyDraft = "";
let svcBusy = false;
let svcWaitSince = 0;
let svcTimer = 0;
let visible = false;

function render() {
  root.innerHTML = `<h1 class="title">설정</h1>
    <div class="sc"><span class="sc-ic">${I("contrast")}</span><div class="sc-t"><b>테마</b><span>밝게·어둡게. “시스템”은 Windows 설정을 따라가요</span></div>
      ${seg("theme", [["system", "시스템"], ["light", "밝게"], ["dark", "어둡게"]], themePref(), "테마")}</div>
    <div class="sc ctx-sc"><span class="sc-ic">${I("grid")}</span><div class="sc-t"><b>우클릭 메뉴</b><span class="ctx-state">확인하는 중…</span></div>
      <button type="button" class="btn accent" data-act="ctx-relink" hidden>${I("refresh")}다시 연결</button>${toggle("ctxmenu", false, "", "우클릭 메뉴")}</div>
    <div class="sc"><span class="sc-ic">${I("power")}</span><div class="sc-t"><b>윈도우 시작 시 실행</b><span class="as-state">확인하는 중…</span></div>${toggle("autostart", false, "")}</div>
    <div class="sc leave-sc" hidden><span class="sc-ic">${I("trash")}</span><div class="sc-t"><b>우클릭 메뉴·시작 프로그램 지우기</b><span>휴대용 앱은 제거 프로그램이 없어요. 앱 파일을 지우기 전에 눌러 두면 탐색기 메뉴와 시작 프로그램 값이 남지 않아요</span></div>
      <button type="button" class="btn" data-act="leave">${I("trash")}지우기</button></div>
    <div class="sc svc"><span class="sc-ic">${I("bolt")}</span><div class="sc-t"><b>빠른 검색 서비스</b><span class="svc-state">확인하는 중…</span><span class="svc-note">C:\\Users 밖의 파일은 내가 열 수 없는 파일이라도 이름이 보일 수 있어요</span></div>
      <span class="state off svc-chip"><i></i>…</span><button type="button" class="btn accent svc-up" data-svc="update" aria-label="빠른 검색 서비스 다시 설치(갱신)" title="빠른 검색 서비스 다시 설치(갱신)" hidden>${I("refresh")}다시 설치(갱신)</button><button type="button" class="btn svc-btn" disabled>…</button></div>
    <div class="sc"><span class="sc-ic">${I("keyboard")}</span><div class="sc-t"><b>단축키로 검색 열기</b><span>어디서든 눌러 검색 창을 띄워요. 칸을 누르고 쓸 단축키를 누르세요</span></div>
      <input class="txt sm hk" id="hk" placeholder="예: Ctrl+Alt+F" aria-label="단축키" readonly>${toggle("hk-on", false, "")}</div>
    <div class="sc upd-sc"><span class="sc-ic">${I("refresh")}</span><div class="sc-t"><b>새 버전 나오면 알려 주기</b><span class="upd-state">GitHub에서 최신 버전 번호만 확인해요(하루 한 번까지)</span></div>
      <button type="button" class="btn" data-act="upd-check">지금 확인</button>${toggle("upd-on", true, "")}</div>
    <div class="sc dt-sc"><span class="sc-ic">${I("doc")}</span><div class="sc-t"><b>문서 도구</b><span class="dt-state">확인하는 중…</span></div>
      <button type="button" class="btn" data-act="dt-file" hidden>${I("folder")}파일로 넣기</button><button type="button" class="btn accent" data-act="dt-get" hidden>받기</button><button type="button" class="btn" data-act="dt-del" hidden>${I("trash")}지우기</button></div>
    <div class="adv ${advOpen ? "open" : ""}"><button type="button" class="adv-h" aria-expanded="${advOpen}" data-act="adv">${I("tool")}<span class="sc-t"><b>고급</b><span>검색할 곳·제외할 폴더·변환 도구·서비스 명령</span></span>${I("down", "chev")}</button>
      <div class="adv-b" ${advOpen ? "" : "hidden"}>
        <div class="ar col"><div class="sc-t"><b>검색할 곳</b><span>비워 두면 모든 드라이브를 찾아요</span></div><ul class="roots-l"></ul>
          <div class="row"><button type="button" class="btn" data-act="add-root">${I("plus")}폴더 더하기</button><button type="button" class="btn subtle" data-act="reset-roots">모든 드라이브로</button><button type="button" class="btn subtle" data-act="rescan">${I("refresh")}다시 색인</button></div></div>
        <div class="ar col"><div class="sc-t"><b>제외할 폴더</b><span>한 줄에 하나씩. 이 폴더 안은 찾지 않아요</span></div><textarea class="ex" id="excludes" spellcheck="false"></textarea>
          <div class="row"><button type="button" class="btn" data-act="save-ex">제외 목록 저장</button><span class="hint s-state" aria-live="polite"></span></div></div>
        <div class="ar col"><div class="sc-t"><b>변환 도구</b><span>영상(ffmpeg)·PDF(PDFium) 도구는 앱에 들어 있어요. 문서(LibreOffice)는 위 [문서 도구]로 받아요</span></div><div class="tools-l hint">확인하는 중…</div></div>
        <div class="ar col"><div class="sc-t"><b>빠른 검색 서비스 자세히</b><span>문제를 알릴 때 쓰는 정보예요</span></div><pre class="svc-cmd"></pre>
          <div class="row"><button type="button" class="btn svc-re" data-svc="update" hidden>${I("refresh")}빠른 검색 서비스 다시 설치(갱신)</button></div></div>
        <div class="ar"><div class="sc-t"><b>처음 안내 다시 보기</b><span>변환 화면 위에 짧은 안내를 다시 보여 드려요</span></div><button type="button" class="btn" data-act="welcome">다시 보기</button></div>
        <div class="ar"><div class="sc-t"><b>색인 파일</b><span class="idx-file"></span></div></div>
      </div></div>
    <p class="ver"></p>`;
  bindSegs(root, (name, v) => { if (name === "theme") setThemePref(v as ThemePref); });
  loadAll();
}

async function loadAll() {
  invoke<{ version: string; portable?: boolean }>("app_info").then((a) => {
    $(".ver", root)!.innerHTML = `파일 도구 ${esc(a.version)} · <button type="button" class="link" data-act="about">앱 정보</button>`;
    $<HTMLElement>(".leave-sc", root)!.hidden = !a.portable;
  }).catch(() => {});
  invoke<CtxStatus>("ctx_menu_get").then(showCtx).catch((e) => { $(".ctx-state", root)!.textContent = friendly(e).title; });
  invoke<AutostartStatus>("autostart_get").then(showAutostart).catch((e) => { $(".as-state", root)!.textContent = friendly(e).title; });
  refreshSearch();
  refreshSvc();
  refreshUpd();
  refreshDocTools();
  if (advOpen) loadTools();
}

/** 0.3.3 문서 도구 칸: 이 PC의 한글·Office, 받았는지, [받기]·[파일로 넣기]·[지우기] */
async function refreshDocTools() {
  const st = $(".dt-state", root);
  if (!st) return;
  const v = await docToolsStatus().catch((e) => { st.textContent = friendly(e).title; return null; });
  if (!v) return;
  const off = [v.office.word && "Word", v.office.excel && "엑셀", v.office.powerpoint && "PowerPoint"].filter(Boolean).join("·");
  const pc = `이 PC: 한글 ${v.engines.hancom ? "있음" : "없음"} · Office ${off ? `있음(${off})` : "없음"}`;
  st.textContent = v.ready
    ? `받아 둠 — LibreOffice ${v.lo_version}, 약 ${v.unpacked_mb} MB${v.source === "0.3.x" ? "(예전 판에서 쓰던 것)" : ""}. Pages·Keynote·Numbers처럼 한글·Office로 안 되는 문서에 써요. ${pc}`
    : `아직 받지 않음 — 한글·Office로 안 되는 문서(Pages·Keynote·Numbers 등)를 바꿀 때 한 번 받아요(약 ${v.download_mb} MB${v.partial_bytes ? `, ${Math.round(v.partial_bytes / 1e6)} MB 받아 둠` : ""}). ${pc}`;
  ($("[data-act=dt-get]", root) as HTMLElement).hidden = v.ready;
  ($("[data-act=dt-file]", root) as HTMLElement).hidden = v.ready;
  ($("[data-act=dt-del]", root) as HTMLElement).hidden = !v.ready;
}

const UPD_HINT = "GitHub에서 최신 버전 번호만 확인해요(하루 한 번까지)";
async function refreshUpd(msg?: string) {
  const v = await invoke<UpdateView>("update_view").catch(() => null);
  if (!v) return;
  const t = $<HTMLInputElement>("#upd-on", root);
  if (t) { t.checked = v.enabled; t.closest(".tg")!.querySelector(".tg-l")!.textContent = v.enabled ? "켬" : "끔"; }
  const st = $(".upd-state", root);
  if (st) st.textContent = msg ?? (v.enabled ? UPD_HINT : "꺼져 있어요. [지금 확인]을 누를 때만 확인해요");
}
async function checkNow(btn: HTMLButtonElement) {
  btn.disabled = true;
  $(".upd-state", root)!.textContent = "확인하는 중…";
  const o = await invoke<UpdateOutcome>("update_check", { force: true }).catch(() => ({ state: "failed" }) as UpdateOutcome);
  btn.disabled = false;
  const v = await invoke<UpdateView>("update_view").catch(() => null);
  const msg = o.state === "newer" ? `새 버전 ${o.version}가 나왔어요` : o.state === "latest" ? `최신 버전이에요 (${v?.current ?? ""})` : "확인하지 못했어요. 인터넷 연결을 확인하고 나중에 다시 해 보세요";
  refreshUpd(msg);
  if (o.state === "newer" && v) showBanner({ ...v, banner: { version: o.version, url: o.url } });
}

function showCtx(st: CtxStatus) {
  const t = $<HTMLInputElement>("#ctxmenu", root)!;
  t.checked = st.enabled;
  t.disabled = !st.supported;
  t.closest(".tg")!.querySelector(".tg-l")!.textContent = st.enabled ? "켬" : "끔";
  const moved = st.enabled && !st.matches_exe;
  $<HTMLElement>("[data-act=ctx-relink]", root)!.hidden = !moved;
  $(".ctx-state", root)!.textContent = !st.supported ? "Windows에서만 쓸 수 있어요"
    : moved ? "앱 위치가 바뀌어 메뉴가 예전 위치를 가리켜요. [다시 연결]을 누르세요"
    : st.enabled ? "탐색기에서 파일을 우클릭 → 파일도구(Windows 11은 “더 많은 옵션 표시” 안)"
    : "꺼져 있어요. 켜면 탐색기 우클릭에 “파일도구”가 생겨요";
}

function showAutostart(st: AutostartStatus) {
  const t = $<HTMLInputElement>("#autostart", root)!;
  t.checked = st.enabled;
  t.disabled = !st.supported;
  const lab = t.closest(".tg")!.querySelector(".tg-l")!;
  lab.textContent = st.enabled ? "켬" : "끔";
  $(".as-state", root)!.textContent = !st.supported ? "Windows에서만 쓸 수 있어요"
    : st.enabled ? `로그인하면 창 없이 알림 영역에서 시작해 감시 폴더를 돌려요${st.matches_exe ? "" : " · 앱 위치가 바뀌었어요. 껐다 켜면 고쳐져요"}`
    : "꺼져 있어요. 켜면 로그인할 때 알림 영역에서 조용히 시작해요";
}

async function refreshSearch() {
  try { view = await invoke<SearchView>("search_status"); } catch { return; }
  const s = view.settings;
  const hk = $<HTMLInputElement>("#hk", root);
  if (hk && document.activeElement !== hk) hk.value = hotkeyDraft || s.hotkey;
  const on = $<HTMLInputElement>("#hk-on", root);
  if (on) { on.checked = s.hotkey_enabled; on.closest(".tg")!.querySelector(".tg-l")!.textContent = s.hotkey_enabled ? "켬" : "끔"; }
  const ul = $(".roots-l", root);
  if (ul) {
    ul.innerHTML = view.effective_roots.map((r) => {
      const rs = view!.status.roots.find((x) => x.path === r);
      return `<li>${I("folder")}<span class="grow" title="${esc(r)}">${esc(r)}<span class="hint"> · ${rs ? (rs.scanning ? "색인 만드는 중" : esc(rs.source)) : ""}${rs && rs.last_scan ? ` · 폴더 ${num(rs.stats.dirs)} · 파일 ${num(rs.stats.files)}` : ""}</span></span>
        ${s.roots.length ? `<button type="button" class="ib" data-rm-root="${esc(r)}" aria-label="${esc(r)} 빼기">${I("x")}</button>` : ""}</li>`;
    }).join("");
  }
  const ex = $<HTMLTextAreaElement>("#excludes", root);
  if (ex && document.activeElement !== ex && ex.dataset.dirty !== "1") ex.value = s.excludes.join("\n");
  const f = $(".idx-file", root);
  if (f) f.textContent = `${view.status.index_file} (${human(view.status.index_bytes)}) · 항목 ${num(view.status.entries)}개`;
}

async function saveSearch(next: SearchSettings) {
  try {
    view = await invoke<SearchView>("search_set_settings", { settings: next });
    hotkeyDraft = "";
    const ex = $<HTMLTextAreaElement>("#excludes", root);
    if (ex) ex.dataset.dirty = "";
    toast("저장했어요");
    refreshSearch();
  } catch (e) {
    toast(friendly(e).title);
    refreshSearch();
  }
}

async function refreshSvc() {
  let p: SvcPanel;
  try { p = await invoke<SvcPanel>("svc_view"); } catch (e) { $(".svc-state", root)!.textContent = friendly(e).title; return; }
  const i = p.info;
  const starting = i.installed && (i.running || i.state === "시작 중") && !p.connected;
  if (!starting) svcWaitSince = 0; else if (!svcWaitSince) svcWaitSince = Date.now();
  let text: string, chip: string, cls: string;
  if (!i.installed) { text = "설치하면 C 드라이브 전체를 훨씬 빨리 찾아요(관리자 확인 1번)"; chip = "설치 안 됨"; cls = "off"; }
  else if (p.connected && !p.ready) { text = `첫 색인을 만드는 중이에요 · 항목 ${num(p.entries)}개`; chip = "시작 중"; cls = "warn"; }
  else if (p.connected) { text = `${p.roots.join(", ")} · 항목 ${num(p.entries)}개 · ${p.visibility}`; chip = "실행 중"; cls = ""; }
  else if (starting && Date.now() - svcWaitSince < 30_000) { text = "서비스가 시작하는 중이에요. 응답하면 저절로 연결해요"; chip = "시작 중"; cls = "warn"; }
  else { text = `설치돼 있지만 연결되지 않았어요 — 앱이 스스로 찾아요 (${i.state})`; chip = "연결 안 됨"; cls = "err"; }
  if (p.update_available && !svcBusy) text += ` · 앱에 든 새 서비스(${p.bundled_version})로 갱신할 수 있어요${p.installed_version ? ` (설치된 것 ${p.installed_version})` : ""}`;
  if (!p.exe) text += i.installed ? " · 설치·제거는 설치본으로 깐 파일 도구에서 할 수 있어요" : " · 설치본(setup.exe)으로 깔면 쓸 수 있어요";
  $(".svc-state", root)!.textContent = text;
  const c = $(".svc-chip", root)!;
  c.className = `state svc-chip ${cls}`;
  c.innerHTML = `<i></i>${chip}`;
  const btn = $<HTMLButtonElement>(".svc-btn", root)!;
  btn.textContent = svcBusy ? "기다리는 중…" : i.installed ? "제거" : "설치";
  btn.dataset.install = i.installed ? "0" : "1";
  btn.disabled = svcBusy || !p.exe;
  const up = $<HTMLButtonElement>(".svc-up", root)!, re = $<HTMLButtonElement>(".svc-re", root);
  up.hidden = !(i.installed && p.exe && p.update_available);
  up.disabled = svcBusy;
  if (re) { re.hidden = !(i.installed && p.exe); re.disabled = svcBusy; }
  $(".svc-cmd", root)!.textContent = [`상태: ${i.installed ? `설치됨 · ${i.state} · 시작 ${i.start_type} · 계정 ${i.account || "?"}` : "설치 안 됨"}`, p.error ? `오류: ${p.error}` : "",
    `앱에 든 서비스 ${p.bundled_version}${i.installed ? ` · 설치된 서비스 ${p.installed_version ?? "(응답 없음)"}${p.update_available ? " · 갱신 필요" : ""}` : ""}`,
    p.install_cmd ? `설치(갱신) 명령: ${p.install_cmd}` : "", p.uninstall_cmd && i.installed ? `제거 명령: ${p.uninstall_cmd}` : ""].filter(Boolean).join("\n");
  window.clearTimeout(svcTimer);
  if (visible && (starting || (p.connected && !p.ready))) svcTimer = window.setTimeout(refreshSvc, 1500);
}
async function svcSetup(install: boolean, update = false) {
  if (svcBusy) return;
  const q = update ? "빠른 검색 서비스를 다시 설치(갱신)할까요?" : install ? "빠른 검색 서비스를 설치할까요?" : "빠른 검색 서비스를 제거할까요?";
  const d = update ? "앱에 든 서비스로 바꾸고 다시 시작해요. 만든 색인은 그대로 써요. 관리자 권한 확인 창이 뜨면 [예]를 눌러 주세요." : "관리자 권한 확인 창이 뜨면 [예]를 눌러 주세요.";
  if (!(await confirmDlg(q, d, update ? "다시 설치(갱신)" : install ? "설치" : "제거", !install))) return;
  svcBusy = true;
  refreshSvc();
  try {
    await invoke<string>("svc_setup", { install });
    if (install) svcWaitSince = Date.now();
    toast(update ? "서비스를 갱신했어요" : install ? "설치했어요" : "제거했어요");
  } catch (e) {
    toast(friendly(e).title, 4000);
  } finally {
    svcBusy = false;
    refreshSvc();
  }
}

async function loadTools() {
  const box = $(".tools-l", root);
  if (!box) return;
  try {
    const [list, bundle] = await Promise.all([invoke<ToolStatus[]>("tools_status"), invoke<Bundle>("tools_bundle_status").catch(() => null)]);
    const inApp = (t: ToolStatus) => /앱|bundle|내장/i.test(t.source ?? "");
    box.innerHTML = list.filter((t) => t.tool !== "ffprobe").map((t) => `<div class="tool-row">${I(t.found ? "check" : "warn", t.found ? "ok-t" : "err-t")}
        <span class="grow"><b>${esc(t.label)}</b> <span class="hint">${t.found ? (inApp(t) ? "앱에 들어 있어요" : esc(t.path ?? "")) : "찾지 못했어요"}${t.version ? ` · ${esc(t.version)}` : ""}</span></span>
        <button type="button" class="btn subtle sm" data-tool="${t.tool}">다른 위치 쓰기…</button>${t.found && !inApp(t) ? `<button type="button" class="btn subtle sm" data-tool-reset="${t.tool}">기본으로</button>` : ""}</div>`).join("") +
      (bundle ? `<p class="hint">앱 묶음: ${bundle.present ? bundle.tools.map((x) => `${x.name} ${x.ready ? "준비됨" : "처음 쓸 때 풀어요"}`).join(" · ") : "없음(직접 설치한 도구를 써요)"}</p>` : "");
    const pdfium = await invoke<string>("pdfium_status").then((p) => `PDF 렌더러(PDFium): ${p}`).catch((e) => `PDF 렌더러(PDFium): ${friendly(e).title}`);
    box.insertAdjacentHTML("beforeend", `<p class="hint">${esc(pdfium)}</p>`);
  } catch (e) {
    box.textContent = friendly(e).title;
  }
}

export const settingsScreen: Screen = {
  id: "settings", title: "설정", icon: "settings",
  init(el) {
    root = el;
    root.addEventListener("change", async (e) => {
      const t = e.target as HTMLInputElement;
      if (t.id === "ctxmenu") {
        try { showCtx(await invoke<CtxStatus>("ctx_menu_set", { enabled: t.checked })); } catch (err) { toast(friendly(err).title); t.checked = !t.checked; }
      } else if (t.id === "autostart") {
        try { showAutostart(await invoke<AutostartStatus>("autostart_set", { enabled: t.checked })); } catch (err) { toast(friendly(err).title); t.checked = !t.checked; }
      } else if (t.id === "hk-on" && view) {
        const hk = hotkeyDraft || view.settings.hotkey;
        if (t.checked && !hk) { toast("왼쪽 칸을 누르고 쓸 단축키를 먼저 누르세요"); t.checked = false; return; }
        saveSearch({ ...view.settings, hotkey: hk, hotkey_enabled: t.checked });
      } else if (t.id === "upd-on") {
        try { showBanner(await invoke<UpdateView>("update_set_enabled", { enabled: t.checked })); refreshUpd(); } catch (err) { toast(friendly(err).title); t.checked = !t.checked; }
      }
    });
    root.addEventListener("input", (e) => { if ((e.target as HTMLElement).id === "excludes") (e.target as HTMLElement).dataset.dirty = "1"; });
    root.addEventListener("keydown", (e) => {
      const t = e.target as HTMLInputElement;
      if (t.id !== "hk" || e.key === "Tab") return;
      e.preventDefault();
      if (["Control", "Alt", "Shift", "Meta"].includes(e.key)) return;
      const mods = [e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift", e.metaKey && "Super"].filter(Boolean) as string[];
      if (!mods.length) { toast("Ctrl·Alt·Shift 중 하나와 함께 누르세요"); return; }
      const key = e.code.startsWith("Key") ? e.code.slice(3) : e.code.startsWith("Digit") ? e.code.slice(5) : e.code;
      hotkeyDraft = [...mods, key].join("+");
      t.value = hotkeyDraft;
      if (view) saveSearch({ ...view.settings, hotkey: hotkeyDraft, hotkey_enabled: true });
    });
    root.addEventListener("click", async (e) => {
      const t = e.target as HTMLElement;
      const svc = t.closest<HTMLButtonElement>(".svc-btn");
      if (svc) { svcSetup(svc.dataset.install === "1"); return; }
      if (t.closest("[data-svc=update]")) { svcSetup(true, true); return; }
      const rm = t.closest<HTMLElement>("[data-rm-root]");
      if (rm && view) { saveSearch({ ...view.settings, roots: view.settings.roots.filter((x) => x !== rm.dataset.rmRoot) }); return; }
      const tool = t.closest<HTMLElement>("[data-tool]");
      if (tool) {
        const [p] = await pickFiles({ multiple: false, filters: [{ name: "실행 파일", extensions: ["exe"] }] });
        if (p) { try { await invoke("set_tool_path", { tool: tool.dataset.tool, path: p }); toast("저장했어요"); loadTools(); } catch (err) { toast(friendly(err).title); } }
        return;
      }
      const tr = t.closest<HTMLElement>("[data-tool-reset]");
      if (tr) { await invoke("set_tool_path", { tool: tr.dataset.toolReset, path: null }).catch(() => {}); loadTools(); return; }
      const b = t.closest<HTMLElement>("[data-act]");
      switch (b?.dataset.act) {
        case "adv": advOpen = !advOpen; { const a = $(".adv", root)!; a.classList.toggle("open", advOpen); $(".adv-b", a)!.hidden = !advOpen; b.setAttribute("aria-expanded", String(advOpen)); } if (advOpen) { loadTools(); refreshSearch(); } break;
        case "add-root": { const ds = await pickFiles({ directory: true, multiple: true }); if (ds.length && view) { const base = view.settings.roots; saveSearch({ ...view.settings, roots: [...base, ...ds.filter((d) => !base.includes(d))] }); } break; }
        case "reset-roots": if (view) saveSearch({ ...view.settings, roots: [] }); break;
        case "rescan": await invoke("search_rescan").catch(() => {}); toast("다시 색인을 시작했어요"); break;
        case "save-ex": if (view) saveSearch({ ...view.settings, excludes: ($<HTMLTextAreaElement>("#excludes", root)!).value.split("\n").map((x) => x.trim()).filter(Boolean) }); break;
        case "leave":
          try {
            showCtx(await invoke<CtxStatus>("ctx_menu_set", { enabled: false }));
            showAutostart(await invoke<AutostartStatus>("autostart_set", { enabled: false }));
            toast("우클릭 메뉴와 시작 프로그램 값을 지웠어요. 이제 앱 파일을 지워도 돼요", 4000);
          } catch (err) { toast(friendly(err).title); }
          break;
        case "ctx-relink": try { showCtx(await invoke<CtxStatus>("ctx_menu_set", { enabled: true })); toast("우클릭 메뉴를 이 위치의 앱에 다시 연결했어요"); } catch (err) { toast(friendly(err).title); } break;
        case "welcome": resetWelcome(); toast("변환 화면에 안내를 다시 보여 드려요"); go("convert"); break;
        case "about": go("about"); break;
        case "upd-check": checkNow(b as HTMLButtonElement); break;
        case "dt-get": if (await dtDownload()) toast("문서 도구를 받았어요"); refreshDocTools(); break;
        case "dt-file": await installFromFile(); refreshDocTools(); break;
        case "dt-del":
          if (await confirmDlg("문서 도구를 지울까요?", "약 445 MB를 비워요. 나중에 필요하면 다시 받을 수 있어요.", "지우기", true)) {
            try { await invoke("doctools_remove"); toast("문서 도구를 지웠어요"); } catch (err) { toast(friendly(err).title); }
            refreshDocTools();
          }
          break;
      }
    });
    document.addEventListener("ft-update", () => refreshUpd());
    render();
  },
  show() { visible = true; loadAll(); },
  hide() { visible = false; window.clearTimeout(svcTimer); },
};
