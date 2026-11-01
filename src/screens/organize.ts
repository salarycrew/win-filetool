// 파일 정리: 이름 한꺼번에 바꾸기 · 같은 파일 찾기 · zip 풀기·묶기 · 맥 한글 이름 고치기
import { cancelJob, dirOf, fileName, fmtTime, friendly, human, invoke, isCancelled, newJob, num, offJobProgress, onJobProgress, pickFiles, stats } from "../api";
import { FT, I, extOf } from "../icons";
import { Screen } from "../router";
import { $, $$, bindResultButtons, bindSegs, checked, chk, confirmDlg, empty, errorPanel, esc, opt, progressPanel, resultRow, seg, segVal, setProgress, toast, value } from "../ui";

type Tab = "rename" | "dup" | "zip" | "nfc";
const TABS: [Tab, string, string][] = [["rename", "이름 한꺼번에 바꾸기", "rename"], ["dup", "같은 파일 찾기", "copy"], ["zip", "zip 풀기·묶기", "zip"], ["nfc", "맥 한글 이름 고치기", "hangul"]];

let root: HTMLElement;
let tab: Tab = "rename";
const body = () => $(".org-body", root)!;

// ---------- 이름 한꺼번에 바꾸기 ----------

interface Pair { from: string; to: string; conflict?: string | null }
let rnFiles: string[] = [];
let rnPreview: Pair[] = [];
let rnUndo: { id: string; done: number } | null = null;
let rnTimer = 0;
let rnSeq = 0;

function renameUI() {
  const b = body();
  b.innerHTML = `<p class="sub">번호·날짜를 붙이거나 글자를 찾아 바꿔요. 바뀔 이름을 먼저 보여 드리고, 바꾼 뒤에도 [되돌리기]로 되돌릴 수 있어요.</p>
    <div class="row"><button type="button" class="btn accent" data-act="rn-files">${I("file")}파일 고르기</button><button type="button" class="btn" data-act="rn-folder">${I("folder")}폴더 안 파일 모두</button>
      ${rnFiles.length ? `<button type="button" class="btn subtle" data-act="rn-clear">${I("x")}비우기</button><span class="hint">${num(rnFiles.length)}개</span>` : ""}</div>
    ${rnFiles.length ? `<div class="rn-grid">
      <div class="optcard rn-rule"><div class="oc-body">
        <div class="oc-h">규칙</div>
        ${opt("찾기", `<input class="txt" id="rn-find" placeholder="바꿀 글자">`)}
        ${opt("바꾸기", `<input class="txt" id="rn-rep" placeholder="새 글자(비우면 지움)">`, "")}
        ${opt("", chk("rn-regex", "정규식으로 찾기(고급)", false))}
        ${opt("앞에 붙이기", `<input class="txt" id="rn-pre" placeholder="예: 제주_">`)}
        ${opt("뒤에 붙이기", `<input class="txt" id="rn-suf" placeholder="예: _최종">`)}
        ${opt("번호", seg("rn-num", [["", "안 붙임"], ["prefix", "앞에"], ["suffix", "뒤에"], ["replace", "이름 대신"]], ""))}
        <div class="rn-numopts" hidden>${opt("시작 번호", `<input class="txt sm" id="rn-start" value="1" inputmode="numeric">`)}${opt("간격", `<input class="txt sm" id="rn-step" value="1" inputmode="numeric">`)}${opt("자릿수", seg("rn-digits", [["1", "1"], ["2", "01"], ["3", "001"]], "2"))}</div>
        ${opt("날짜", seg("rn-date", [["", "안 붙임"], ["prefix", "앞에"], ["suffix", "뒤에"]], ""))}
        <div class="rn-dateopts" hidden>${opt("어떤 날짜", seg("rn-dsrc", [["taken", "찍은 날"], ["modified", "고친 날"]], "taken"))}${opt("모양", seg("rn-dfmt", [["YYYY-MM-DD", "2026-11-01"], ["YYYYMMDD", "20261101"], ["YYYY-MM-DD_HHmmss", "2026-11-01_093015"]], "YYYY-MM-DD"))}</div>
        ${opt("대소문자", seg("rn-case", [["keep", "그대로"], ["lower", "소문자"], ["upper", "대문자"]], "keep"))}
        ${opt("", chk("rn-ext", "확장자를 소문자로(.JPG → .jpg)", false))}
        ${opt("순서", seg("rn-sort", [["name", "이름순"], ["modified", "고친 날짜순"], ["taken", "찍은 날짜순"]], "name"), "번호를 붙이는 순서")}
      </div></div>
      <div class="rn-prev"><div class="tbl"><div class="th rn"><span>지금 이름</span><span>바뀔 이름</span></div><div class="tb rn-rows"></div></div>
        <div class="rn-foot"><span class="rn-sum hint"></span><span class="grow"></span>
          ${rnUndo ? `<button type="button" class="btn" data-act="rn-undo">${I("undo")}되돌리기</button>` : ""}
          <button type="button" class="btn accent lg" data-act="rn-apply" disabled>이름 바꾸기</button></div>
        <div class="rn-result"></div></div></div>`
      : empty("rename", "이름을 바꿀 파일을 넣어 주세요", "파일을 끌어다 놓거나 위 단추로 고르세요.", rnUndo ? `<div class="row center"><button type="button" class="btn" data-act="rn-undo">${I("undo")}방금 바꾼 이름 되돌리기</button></div>` : "")}`;
  bindSegs(b, () => { syncRnOpts(); queuePreview(); });
  b.oninput = () => queuePreview();
  b.onchange = () => queuePreview();
  if (rnFiles.length) { syncRnOpts(); queuePreview(0); }
}
function syncRnOpts() {
  const b = body();
  const n = $(".rn-numopts", b), d = $(".rn-dateopts", b);
  if (n) n.hidden = !segVal(b, "rn-num");
  if (d) d.hidden = !segVal(b, "rn-date");
}
function rule() {
  const b = body();
  const n = segVal(b, "rn-num"), d = segVal(b, "rn-date");
  const find = value(b, "rn-find");
  return {
    ...(find ? { find, replace: ($<HTMLInputElement>("#rn-rep", b)?.value ?? "") } : {}),
    regex: checked(b, "rn-regex"),
    ...(value(b, "rn-pre") ? { prefix: ($<HTMLInputElement>("#rn-pre", b)!).value } : {}),
    ...(value(b, "rn-suf") ? { suffix: ($<HTMLInputElement>("#rn-suf", b)!).value } : {}),
    ...(n ? { numbering: { start: Math.max(0, Number(value(b, "rn-start")) || 0), step: Math.max(1, Number(value(b, "rn-step")) || 1), digits: Number(segVal(b, "rn-digits")), position: n } } : {}),
    ...(d ? { date: { source: segVal(b, "rn-dsrc"), format: segVal(b, "rn-dfmt"), position: d } } : {}),
    case: segVal(b, "rn-case") || "keep",
    ext: checked(b, "rn-ext") ? "lower" : "keep",
  };
}
function queuePreview(delay = 180) {
  window.clearTimeout(rnTimer);
  rnTimer = window.setTimeout(preview, delay);
}
async function preview() {
  const b = body();
  const rows = $(".rn-rows", b);
  if (!rows) return;
  const seq = ++rnSeq;
  try {
    const out = await invoke<Pair[]>("rename_preview", { files: rnFiles, rule: rule(), sort: segVal(b, "rn-sort") || "name" });
    if (seq !== rnSeq) return;
    rnPreview = out;
    const changed = out.filter((p) => p.from !== p.to);
    const bad = out.filter((p) => p.conflict);
    rows.innerHTML = out.map((p) => `<div class="tr rn ${p.conflict ? "bad" : p.from === p.to ? "same" : ""}"><span title="${esc(p.from)}">${FT(extOf(p.from), 18)}<em>${esc(fileName(p.from))}</em></span>
      <span title="${esc(p.conflict ?? p.to)}">${p.from === p.to ? `<i class="hint">그대로</i>` : `<em>${esc(fileName(p.to))}</em>`}${p.conflict ? `<b class="cf">${I("warn")}${esc(p.conflict)}</b>` : ""}</span></div>`).join("");
    $(".rn-sum", b)!.textContent = bad.length ? `겹치는 이름 ${bad.length}개를 먼저 고쳐 주세요` : changed.length ? `${num(changed.length)}개 이름이 바뀌어요` : "바뀌는 이름이 없어요 — 왼쪽에서 규칙을 정하세요";
    $(".rn-sum", b)!.classList.toggle("err-t", bad.length > 0);
    const ap = $<HTMLButtonElement>("[data-act=rn-apply]", b)!;
    ap.disabled = !changed.length || bad.length > 0;
    ap.textContent = changed.length ? `이름 ${num(changed.length)}개 바꾸기` : "이름 바꾸기";
  } catch (e) {
    if (seq !== rnSeq) return;
    rows.innerHTML = `<div class="pad">${errorPanel(friendly(e))}</div>`;
  }
}
async function renameApply() {
  const pairs = rnPreview.filter((p) => p.from !== p.to && !p.conflict).map(({ from, to }) => ({ from, to }));
  if (!pairs.length) return;
  try {
    const r = await invoke<{ done: number; failed: { from: string; reason: string }[]; undo_id: string }>("rename_apply", { pairs });
    rnUndo = { id: r.undo_id, done: r.done };
    const map = new Map(pairs.map((p) => [p.from, p.to]));
    const failed = new Set(r.failed.map((f) => f.from));
    rnFiles = rnFiles.map((f) => (failed.has(f) ? f : map.get(f) ?? f));
    renameUI();
    const res = $(".rn-result", body());
    if (res) res.innerHTML = `<div class="note ok">${I("check")}<span>이름 ${num(r.done)}개를 바꿨어요.${r.failed.length ? ` ${r.failed.length}개는 못 바꿨어요(${esc(r.failed[0].reason)}).` : ""} 잘못 바꿨으면 [되돌리기]를 누르세요.</span></div>`;
  } catch (e) {
    toast(friendly(e).title);
  }
}
async function renameUndo() {
  if (!rnUndo) return;
  try {
    const r = await invoke<{ done: number; failed: unknown[] }>("rename_undo", { undoId: rnUndo.id, undo_id: rnUndo.id });
    // 되돌린 이름으로 목록 다시
    const back = new Map(rnPreview.filter((p) => p.from !== p.to).map((p) => [p.to, p.from]));
    rnFiles = rnFiles.map((f) => back.get(f) ?? f);
    rnUndo = null;
    renameUI();
    const res = $(".rn-result", body());
    const msg = `이름 ${num(r.done)}개를 원래대로 되돌렸어요.${r.failed.length ? ` ${r.failed.length}개는 되돌리지 못했어요.` : ""}`;
    if (res) res.innerHTML = `<div class="note ok">${I("undo")}<span>${msg}</span></div>`; else toast(msg);
  } catch (e) {
    toast(friendly(e).title);
  }
}
async function addRenameFiles(paths: string[]) {
  const st = await stats(paths);
  const out: string[] = [];
  for (const p of paths) {
    if (st.get(p)?.is_dir) out.push(...(await invoke<string[]>("list_files", { folder: p }).catch(() => [])));
    else out.push(p);
  }
  for (const f of out) if (!rnFiles.includes(f)) rnFiles.push(f);
  if (!out.length) toast("이름을 바꿀 파일이 없어요");
  rnUndo = rnUndo && rnFiles.length ? rnUndo : rnUndo;
  renameUI();
}

// ---------- 같은 파일 찾기 ----------

interface DupGroup { size: number; hash: string; files: { path: string; modified: number }[] }
let dupRoots: string[] = [];
let dupRes: { groups: DupGroup[]; scanned: number; wasted_bytes: number } | null = null;
let dupMarked = new Set<string>();
let dupJob = 0;
let dupBusy = false;

function dupUI() {
  const b = body();
  const marked = dupRes ? dupRes.groups.flatMap((g) => g.files.filter((f) => dupMarked.has(f.path)).map(() => g.size)).reduce((a, x) => a + x, 0) : 0;
  b.innerHTML = `<p class="sub">내용이 똑같은 파일을 찾아요(이름이 달라도). 지운 파일은 <b>휴지통</b>으로 가서 다시 살릴 수 있어요.</p>
    <div class="optcard"><div class="oc-body">
      ${opt("찾을 폴더", `<div class="roots">${dupRoots.map((r, i) => `<span class="chip-r">${I("folder")}<span title="${esc(r)}">${esc(r)}</span><button type="button" class="ib" data-rm-root="${i}" aria-label="${esc(r)} 빼기">${I("x")}</button></span>`).join("") || `<span class="hint">아직 없어요</span>`}
        <button type="button" class="btn" data-act="dup-add">${I("plus")}폴더 더하기</button></div>`)}
      ${opt("크기", seg("dup-min", [["0", "모두"], ["102400", "100KB 넘는 것"], ["1048576", "1MB 넘는 것"], ["10485760", "10MB 넘는 것"]], "102400"), "작은 파일을 빼면 빨라요")}
      ${opt("", chk("dup-hidden", "숨김 파일도 보기", false))}
    </div><div class="oc-foot"><span class="save">${I("info")}<span>사진·동영상이 많은 폴더는 몇 분 걸릴 수 있어요</span></span><button type="button" class="btn accent lg" data-act="dup-scan" ${dupRoots.length && !dupBusy ? "" : "disabled"}>찾기</button></div></div>
    <div class="dup-out">${dupBusy ? progressPanel("같은 파일 찾는 중") : dupRes ? dupResultHTML(marked) : ""}</div>`;
  bindSegs(b);
}
function dupResultHTML(marked: number): string {
  const r = dupRes!;
  if (!r.groups.length) return `<div class="panel">${empty("check", "같은 파일이 없어요", `파일 ${num(r.scanned)}개를 살펴봤어요.`)}</div>`;
  const n = dupMarked.size;
  return `<div class="panel dup"><div class="dup-h"><div><b>같은 파일 ${num(r.groups.length)}묶음</b><span class="hint"> · 파일 ${num(r.scanned)}개 살펴봄 · 지우면 최대 ${human(r.wasted_bytes)} 비워요</span></div>
      <div class="row"><button type="button" class="btn subtle" data-act="dup-auto">${I("check")}오래된 것만 남기고 모두 고르기</button><button type="button" class="btn subtle" data-act="dup-none">고르기 취소</button></div></div>
    <div class="note">${I("info")}<span>묶음마다 하나는 꼭 남겨요. 다 고르면 [휴지통으로]가 눌리지 않아요.</span></div>
    <div class="dup-list">${r.groups.map((g, gi) => {
      const all = g.files.every((f) => dupMarked.has(f.path));
      return `<div class="dg ${all ? "bad" : ""}"><div class="dg-h">${FT(extOf(g.files[0].path), 22)}<b>${esc(fileName(g.files[0].path))}</b><span class="hint">${human(g.size)} × ${g.files.length}개</span>${all ? `<span class="cf">${I("warn")}하나는 남겨 주세요</span>` : ""}</div>
        ${g.files.map((f, fi) => `<label class="dg-f"><input type="checkbox" data-g="${gi}" data-f="${fi}" ${dupMarked.has(f.path) ? "checked" : ""}><span class="box">${I("check")}</span>
          <span class="dg-p" title="${esc(f.path)}">${esc(dirOf(f.path))}\\<b>${esc(fileName(f.path))}</b></span><span class="hint">${fmtTime(f.modified)}</span>
          <button type="button" class="ib" data-reveal="${esc(f.path)}" title="폴더에서 보기" aria-label="폴더에서 보기">${I("open")}</button></label>`).join("")}</div>`;
    }).join("")}</div>
    <div class="dup-foot"><span class="hint">${n ? `${num(n)}개 고름 · ${human(marked)}` : "지울 파일을 고르세요"}</span><span class="grow"></span>
      <button type="button" class="btn danger lg" data-act="dup-trash" ${n && !r.groups.some((g) => g.files.every((f) => dupMarked.has(f.path))) ? "" : "disabled"}>${I("trash")}휴지통으로</button></div></div>`;
}
async function dupScan() {
  if (dupBusy || !dupRoots.length) return;
  dupBusy = true; dupRes = null; dupMarked = new Set();
  dupUI();
  const out = $(".dup-out", body())!;
  dupJob = newJob();
  onJobProgress(dupJob, (f, d) => setProgress(out, d && d.total ? f : null, d ? `${num(d.done)} / ${num(d.total)}${d.current ? ` · ${fileName(d.current)}` : ""}` : undefined));
  setProgress(out, null, "파일 목록을 읽는 중");
  $("[data-cancel]", out)!.addEventListener("click", () => cancelJob(dupJob));
  try {
    dupRes = await invoke("dup_scan", { job: dupJob, roots: dupRoots, minSize: Number(segVal(body(), "dup-min")), min_size: Number(segVal(body(), "dup-min")), includeHidden: checked(body(), "dup-hidden"), include_hidden: checked(body(), "dup-hidden") });
  } catch (e) {
    dupBusy = false;
    dupUI();
    if (!isCancelled(e)) $(".dup-out", body())!.innerHTML = errorPanel(friendly(e)); else toast("멈췄어요");
    return;
  } finally {
    offJobProgress(dupJob);
  }
  dupBusy = false;
  dupUI();
}
function dupAuto() {
  if (!dupRes) return;
  dupMarked = new Set();
  for (const g of dupRes.groups) {
    const keep = [...g.files].sort((a, b) => a.modified - b.modified || a.path.length - b.path.length)[0];
    g.files.forEach((f) => { if (f !== keep) dupMarked.add(f.path); });
  }
  dupUI();
}
async function dupTrash() {
  if (!dupRes || !dupMarked.size) return;
  if (dupRes.groups.some((g) => g.files.every((f) => dupMarked.has(f.path)))) { toast("묶음마다 하나는 남겨 주세요"); return; }
  const n = dupMarked.size;
  if (!(await confirmDlg(`${num(n)}개를 휴지통으로 옮길까요?`, "휴지통에서 언제든 다시 살릴 수 있어요. 묶음마다 하나씩은 그대로 남아요.", "휴지통으로"))) return;
  try {
    const r = await invoke<{ done: number; failed: { path: string; reason: string }[] | string[] }>("dup_trash", { paths: [...dupMarked] });
    const failed = new Set((r.failed as (string | { path: string })[]).map((x) => (typeof x === "string" ? x : x.path)));
    dupRes.groups = dupRes.groups.map((g) => ({ ...g, files: g.files.filter((f) => !dupMarked.has(f.path) || failed.has(f.path)) })).filter((g) => g.files.length > 1);
    dupRes.wasted_bytes = dupRes.groups.reduce((a, g) => a + g.size * (g.files.length - 1), 0);
    dupMarked = new Set();
    dupUI();
    toast(`${num(r.done)}개를 휴지통으로 옮겼어요${failed.size ? ` · ${failed.size}개는 못 옮겼어요` : ""}`, 4000);
  } catch (e) {
    toast(friendly(e).title);
  }
}

// ---------- zip ----------

let zipOut = "";
function zipUI() {
  const b = body();
  b.innerHTML = `<p class="sub">한글 이름이 깨진 zip도 풀고, 맥·윈도 어디서나 바르게 보이는 zip을 만들어요.</p>
    <div class="cards two">
      <button type="button" class="card" data-act="zip-open"><span class="tile tone-orange">${I("unzip")}</span><span class="card-t"><b>zip 풀기</b><span class="d">깨진 한글 이름 자동 처리</span></span></button>
      <button type="button" class="card" data-act="zip-make"><span class="tile tone-green">${I("zip")}</span><span class="card-t"><b>zip으로 묶기</b><span class="d">파일·폴더를 하나로</span></span></button>
    </div><div class="zip-out">${zipOut}</div>`;
}
async function zipOpen(paths?: string[]) {
  const ps = paths ?? (await pickFiles({ filters: [{ name: "zip", extensions: ["zip"] }] }));
  if (!ps.length) return;
  const out = $(".zip-out", body());
  const rows: string[] = [];
  for (const p of ps) {
    try {
      const o = await invoke<Record<string, unknown>>("zip_extract", { path: p, zip: p, encoding: "auto", outDir: null, rezip: false });
      if (typeof o.folder === "string") rows.push(resultRow({ output: o.folder, isFolder: true, pill: `파일 ${o.count}개`, note: (o.skipped as unknown[])?.length ? `${(o.skipped as unknown[]).length}개 건너뜀` : undefined }));
      else { const ex = (o.extract ?? o) as { out_dir: string; files: string[] }; rows.push(resultRow({ output: ex.out_dir, isFolder: true, pill: `파일 ${ex.files.length}개` })); }
    } catch (e) {
      rows.push(`<div class="rr err">${FT("zip", 32)}<div class="rr-n"><b>${esc(fileName(p))}</b><span class="e">${esc(friendly(e).title)}</span></div></div>`);
    }
  }
  zipOut = `<div class="panel"><div class="done-h"><span class="okc">${I("check")}</span><div><b>풀었어요</b><span>원본 옆 새 폴더에 풀었어요. 같은 이름은 덮어쓰지 않았어요.</span></div></div><div class="rlist">${rows.join("")}</div></div>`;
  if (out) out.innerHTML = zipOut;
}
async function zipMake(paths?: string[]) {
  const ps = paths ?? (await pickFiles());
  if (!ps.length) return;
  try {
    const job = newJob();
    const o = await invoke<{ output: string; count: number; size: number }>("zip_create", { job, inputs: ps });
    zipOut = `<div class="panel"><div class="done-h"><span class="okc">${I("check")}</span><div><b>묶었어요</b><span>맥·윈도 모두에서 한글 이름이 바르게 보여요.</span></div></div><div class="rlist">${resultRow({ output: o.output, after: o.size, pill: `파일 ${o.count}개` })}</div></div>`;
  } catch (e) {
    zipOut = errorPanel(friendly(e));
  }
  const out = $(".zip-out", body());
  if (out) out.innerHTML = zipOut;
}

// ---------- 맥 한글 이름 ----------

let nfcOut = "";
function nfcUI() {
  const b = body();
  b.innerHTML = `<p class="sub">맥에서 받은 파일 이름이 “ㅎㅏㄴㄱㅡㄹ”처럼 풀어져 보이면 폴더째 고쳐요. 바꿀 이름을 먼저 보여 드려요.</p>
    <div class="row"><button type="button" class="btn accent" data-act="nfc-pick">${I("folder")}폴더 고르기</button><span class="hint">맥에서 만든 zip은 [zip 풀기]를 쓰세요</span></div>
    <div class="nfc-out">${nfcOut}</div>`;
}
async function nfcRun(folder: string) {
  const out = $(".nfc-out", body())!;
  out.innerHTML = `<p class="hint">살펴보는 중…</p>`;
  try {
    const plan = await invoke<{ renames: { from: string; to: string }[]; collisions: unknown[]; scanned: number }>("nfc_preview", { folder });
    if (!plan.renames.length) { nfcOut = `<div class="panel">${empty("check", "고칠 이름이 없어요", `${num(plan.scanned)}개 모두 바른 이름이에요.`)}</div>`; out.innerHTML = nfcOut; return; }
    out.innerHTML = `<div class="panel"><b>이름 ${num(plan.renames.length)}개를 고쳐요</b>${plan.collisions.length ? `<span class="hint"> · 같은 이름이 이미 있는 ${plan.collisions.length}개는 건너뛰어요</span>` : ""}
      <ul class="peek">${plan.renames.slice(0, 30).map((r) => `<li>${esc(fileName(r.from))} → <b>${esc(fileName(r.to))}</b></li>`).join("")}</ul>${plan.renames.length > 30 ? `<p class="hint">… 외 ${plan.renames.length - 30}개</p>` : ""}
      <div class="row"><button type="button" class="btn accent" data-nfc-apply="${esc(folder)}">이름 고치기</button></div></div>`;
  } catch (e) {
    out.innerHTML = errorPanel(friendly(e));
  }
}
async function nfcApply(folder: string) {
  try {
    const rep = await invoke<{ renamed: unknown[]; failed: unknown[] }>("nfc_apply", { folder });
    nfcOut = `<div class="panel"><div class="done-h"><span class="okc">${I("check")}</span><div><b>이름 ${num(rep.renamed.length)}개를 고쳤어요</b><span>${rep.failed.length ? `${rep.failed.length}개는 못 바꿨어요` : "폴더 안 이름이 모두 바르게 보여요"}</span></div></div>${resultRow({ output: folder, isFolder: true })}</div>`;
  } catch (e) {
    nfcOut = errorPanel(friendly(e));
  }
  $(".nfc-out", body())!.innerHTML = nfcOut;
}

// ---------- 화면 ----------

function render() {
  root.innerHTML = `<h1 class="title">파일 정리</h1>
    <div class="pivot" role="tablist">${TABS.map(([t, l, ic]) => `<button type="button" role="tab" aria-selected="${t === tab}" class="${t === tab ? "on" : ""}" data-tab="${t}">${I(ic)}<span>${l}</span></button>`).join("")}</div>
    <div class="org-body" role="tabpanel"></div>`;
  ({ rename: renameUI, dup: dupUI, zip: zipUI, nfc: nfcUI })[tab]();
}
function setTab(t: Tab) { tab = t; render(); }

export const organizeScreen: Screen = {
  id: "organize", title: "파일 정리", icon: "rename",
  init(el) {
    root = el;
    bindResultButtons(root);
    root.addEventListener("click", async (e) => {
      const t = e.target as HTMLElement;
      const tb = t.closest<HTMLElement>("[data-tab]");
      if (tb) { setTab(tb.dataset.tab as Tab); return; }
      const rm = t.closest<HTMLElement>("[data-rm-root]");
      if (rm) { dupRoots.splice(Number(rm.dataset.rmRoot), 1); dupUI(); return; }
      const rv = t.closest<HTMLElement>("[data-reveal]");
      if (rv) { e.preventDefault(); invoke("open_folder", { path: rv.dataset.reveal }).catch(() => {}); return; }
      const na = t.closest<HTMLElement>("[data-nfc-apply]");
      if (na) { nfcApply(na.dataset.nfcApply!); return; }
      const b = t.closest<HTMLElement>("[data-act]");
      if (!b) return;
      switch (b.dataset.act) {
        case "rn-files": addRenameFiles(await pickFiles()); break;
        case "rn-folder": { const [d] = await pickFiles({ directory: true, multiple: false }); if (d) addRenameFiles([d]); break; }
        case "rn-clear": rnFiles = []; rnPreview = []; renameUI(); break;
        case "rn-apply": renameApply(); break;
        case "rn-undo": renameUndo(); break;
        case "dup-add": { const ds = await pickFiles({ directory: true, multiple: true }); for (const d of ds) if (!dupRoots.includes(d)) dupRoots.push(d); dupUI(); break; }
        case "dup-scan": dupScan(); break;
        case "dup-auto": dupAuto(); break;
        case "dup-none": dupMarked = new Set(); dupUI(); break;
        case "dup-trash": dupTrash(); break;
        case "zip-open": zipOpen(); break;
        case "zip-make": zipMake(); break;
        case "nfc-pick": { const [d] = await pickFiles({ directory: true, multiple: false }); if (d) nfcRun(d); break; }
      }
    });
    root.addEventListener("change", (e) => {
      const c = e.target as HTMLInputElement;
      if (c.dataset.g == null || !dupRes) return;
      const f = dupRes.groups[Number(c.dataset.g)].files[Number(c.dataset.f)];
      if (c.checked) dupMarked.add(f.path); else dupMarked.delete(f.path);
      const sc = (document.getElementById("pane") as HTMLElement).scrollTop;
      dupUI();
      (document.getElementById("pane") as HTMLElement).scrollTop = sc;
    });
    render();
  },
  show(arg) {
    const a = arg as { tab?: Tab; folders?: string[]; roots?: string[] } | undefined;
    if (!a?.tab) return;
    tab = a.tab;
    if (a.roots) { dupRoots = [...new Set([...dupRoots, ...a.roots])]; dupRes = null; }
    render();
    if (a.folders) addRenameFiles(a.folders);
  },
  drop(paths) {
    if (tab === "rename") addRenameFiles(paths);
    else if (tab === "dup") { stats(paths).then((st) => { for (const p of paths) if (st.get(p)?.is_dir && !dupRoots.includes(p)) dupRoots.push(p); dupUI(); }); }
    else if (tab === "zip") {
      const zips = paths.filter((p) => p.toLowerCase().endsWith(".zip"));
      if (zips.length === paths.length) zipOpen(zips); else zipMake(paths);
    } else if (tab === "nfc") nfcRun(paths[0]);
  },
};
