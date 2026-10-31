import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";

type Kind = "dir" | "zip" | "pdf" | "other" | "missing";
interface Item { path: string; kind: Kind; pages: number | null }
interface Rename { from: string; to: string; is_dir: boolean }
interface Collision { path: string; existing: string }
interface FolderPlan { root: string; renames: Rename[]; collisions: Collision[]; unreadable: string[]; scanned: number }
interface ApplyReport { renamed: Rename[]; collisions: Collision[]; failed: [string, string][] }
interface ExtractReport { out_dir: string; files: string[]; dirs: number; normalized: number; cp949_names: number; skipped_mac: string[]; collisions: string[]; unsafe_names: string[] }
interface RezipReport { output: string; entries: number; normalized: number; skipped_mac: string[]; duplicates: string[] }

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const itemsEl = $<HTMLUListElement>("items");
const opEl = $<HTMLSelectElement>("op");
const optsEl = $<HTMLDivElement>("opts");
const logEl = $<HTMLPreElement>("log");
const runBtn = $<HTMLButtonElement>("run");
const busyEl = $<HTMLSpanElement>("busy");
const outdirEl = $<HTMLSpanElement>("outdir");
const dropEl = $<HTMLElement>("drop");

let items: Item[] = [];
let outDir: string | null = null;
let pendingPlan: FolderPlan | null = null;

const KIND_LABEL: Record<Kind, string> = { dir: "폴더", zip: "zip", pdf: "PDF", other: "기타", missing: "없음" };

function log(text: string, cls?: "ok" | "err") {
  const line = document.createElement("div");
  if (cls) line.className = cls;
  line.textContent = text;
  logEl.appendChild(line);
  logEl.scrollTop = logEl.scrollHeight;
}

function errText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}

async function addPaths(paths: string[]) {
  if (paths.length === 0) return;
  try {
    const found = await invoke<Item[]>("inspect_paths", { paths });
    for (const it of found) if (!items.some((x) => x.path === it.path)) items.push(it);
    pendingPlan = null;
    renderItems();
    autoPickOp();
  } catch (e) {
    log(`항목을 읽지 못했습니다: ${errText(e)}`, "err");
  }
}

function renderItems() {
  itemsEl.innerHTML = "";
  if (items.length === 0) {
    itemsEl.innerHTML = '<li class="empty">아직 없습니다</li>';
    return;
  }
  items.forEach((it, i) => {
    const li = document.createElement("li");
    const kind = document.createElement("span");
    kind.className = "kind";
    kind.textContent = KIND_LABEL[it.kind] + (it.pages != null ? ` ${it.pages}쪽` : "");
    const path = document.createElement("span");
    path.className = "path";
    path.textContent = it.path;
    path.title = it.path;
    const mk = (label: string, title: string, fn: () => void, disabled = false) => {
      const b = document.createElement("button");
      b.type = "button";
      b.textContent = label;
      b.title = title;
      b.disabled = disabled;
      b.onclick = () => { fn(); pendingPlan = null; renderItems(); };
      return b;
    };
    li.append(
      kind,
      path,
      mk("▲", "위로", () => { [items[i - 1], items[i]] = [items[i], items[i - 1]]; }, i === 0),
      mk("▼", "아래로", () => { [items[i + 1], items[i]] = [items[i], items[i + 1]]; }, i === items.length - 1),
      mk("✕", "빼기", () => { items.splice(i, 1); }),
    );
    itemsEl.appendChild(li);
  });
}

function autoPickOp() {
  const kinds = new Set(items.map((i) => i.kind));
  if (kinds.size !== 1) return;
  const only = [...kinds][0];
  if (only === "dir") opEl.value = "nfc";
  else if (only === "zip") opEl.value = "zip";
  else if (only === "pdf" && !opEl.value.match(/merge|split|rotate|delete|reorder/)) opEl.value = items.length > 1 ? "merge" : "split";
  renderOpts();
}

function renderOpts() {
  pendingPlan = null;
  const op = opEl.value;
  const pagesInput = (id: string, label: string, ph: string) =>
    `<label>${label} <input type="text" id="${id}" placeholder="${ph}" /></label>`;
  let html = "";
  switch (op) {
    case "nfc":
      html = `<p class="hint">먼저 <b>미리보기</b>로 바뀔 이름을 확인한 뒤 <b>실행</b>을 누르면 적용합니다. 같은 이름이 이미 있으면 건너뜁니다.</p>
        <div class="row"><button type="button" id="preview">미리보기</button></div>`;
      break;
    case "zip":
      html = `<label><input type="checkbox" id="rezip" /> 한글 이름을 고친 zip도 새로 만들기 (이름_NFC.zip)</label>
        <p class="hint">__MACOSX 폴더와 ._ 로 시작하는 맥 전용 파일은 빼고 풉니다.</p>`;
      break;
    case "merge":
      html = `<p class="hint">목록 순서대로 합칩니다(▲▼로 순서 조정). 결과: 첫파일이름_합침.pdf</p>`;
      break;
    case "split":
      html = `<label><input type="radio" name="split" value="every" checked /> 한 쪽씩 모두 나누기</label>
        <label><input type="radio" name="split" value="ranges" /> 범위로 나누기 ${pagesInput("ranges", "", "예: 1-3,5  → 1~3쪽 파일, 5쪽 파일")}</label>`;
      break;
    case "rotate":
      html = `${pagesInput("pages", "페이지", "예: 1,3-5 (비우면 전체)")}
        <label>각도 <select id="deg"><option value="90">시계 방향 90°</option><option value="180">180°</option><option value="270">반시계 방향 90° (270°)</option></select></label>`;
      break;
    case "delete":
      html = pagesInput("pages", "지울 페이지", "예: 2,4-6");
      break;
    case "reorder":
      html = `${pagesInput("order", "새 순서", "예: 3,1,2 또는 5-1 (모든 쪽을 한 번씩)")}`;
      break;
  }
  optsEl.innerHTML = html;
  const pv = document.getElementById("preview");
  if (pv) pv.onclick = () => run(true);
}

const val = (id: string) => (document.getElementById(id) as HTMLInputElement | null)?.value.trim() ?? "";

function setBusy(b: boolean) {
  busyEl.hidden = !b;
  document.querySelectorAll("button, select, input").forEach((el) => ((el as HTMLButtonElement).disabled = b));
  if (!b) renderItems();
}

function need(kind: Kind, min = 1): Item[] {
  const list = items.filter((i) => i.kind === kind);
  if (list.length < min) {
    const what = kind === "dir" ? "폴더" : kind === "zip" ? "zip 파일" : "PDF";
    throw new Error(min > 1 ? `${what}을(를) ${min}개 이상 넣어 주세요` : `${what}을(를) 먼저 넣어 주세요`);
  }
  return list;
}

async function run(previewOnly = false) {
  const op = opEl.value;
  setBusy(true);
  try {
    switch (op) {
      case "nfc": {
        for (const it of need("dir")) {
          if (previewOnly || !pendingPlan || pendingPlan.root !== it.path) {
            const plan = await invoke<FolderPlan>("nfc_preview", { folder: it.path });
            log(`[미리보기] ${it.path}: 바꿀 이름 ${plan.renames.length}개, 충돌 ${plan.collisions.length}개 (검사 ${plan.scanned}개)`);
            plan.renames.slice(0, 200).forEach((r) => log(`  ${r.from}  →  ${r.to.split(/[\\/]/).pop()}`));
            if (plan.renames.length > 200) log(`  … 외 ${plan.renames.length - 200}개`);
            plan.collisions.forEach((c) => log(`  [건너뜀] 같은 이름이 이미 있음: ${c.existing}`, "err"));
            pendingPlan = plan;
            if (previewOnly) continue;
            if (plan.renames.length === 0) { log("바꿀 이름이 없습니다.", "ok"); continue; }
            log("확인했으면 [실행]을 한 번 더 누르면 적용합니다.");
            continue;
          }
          const rep = await invoke<ApplyReport>("nfc_apply", { folder: it.path });
          log(`[적용] ${it.path}: ${rep.renamed.length}개 이름 바꿈, 충돌로 건너뜀 ${rep.collisions.length}개, 실패 ${rep.failed.length}개`, rep.failed.length ? "err" : "ok");
          rep.failed.forEach(([p, e]) => log(`  실패: ${p} — ${e}`, "err"));
          pendingPlan = null;
        }
        break;
      }
      case "zip": {
        const rezip = (document.getElementById("rezip") as HTMLInputElement).checked;
        for (const it of need("zip")) {
          const r = await invoke<{ extract: ExtractReport; rezip: RezipReport | null }>("zip_extract", { zip: it.path, outDir, rezip });
          const e = r.extract;
          log(`[zip 풀기] ${it.path} → ${e.out_dir}`, "ok");
          log(`  파일 ${e.files.length}개, 폴더 ${e.dirs}개, 한글 이름 고침 ${e.normalized}개, 맥 전용 파일 제외 ${e.skipped_mac.length}개`);
          if (e.cp949_names) log(`  윈도(CP949) 이름으로 읽은 항목 ${e.cp949_names}개`);
          e.collisions.forEach((c) => log(`  [건너뜀] 이미 있음: ${c}`, "err"));
          e.unsafe_names.forEach((c) => log(`  [건너뜀] 안전하지 않은 경로: ${c}`, "err"));
          if (r.rezip) log(`  새 zip: ${r.rezip.output} (항목 ${r.rezip.entries}개)`, "ok");
        }
        break;
      }
      case "merge": {
        const pdfs = need("pdf", 2);
        const r = await invoke<{ output: string; pages: number }>("pdf_merge", { paths: pdfs.map((p) => p.path), outDir });
        log(`[합치기] ${pdfs.length}개 → ${r.output} (${r.pages}쪽)`, "ok");
        break;
      }
      case "split": {
        const every = (document.querySelector('input[name="split"]:checked') as HTMLInputElement).value === "every";
        const mode = every ? { mode: "every_page" } : { mode: "ranges", spec: val("ranges") };
        if (!every && !val("ranges")) throw new Error("나눌 범위를 적어 주세요 (예: 1-3,5)");
        for (const it of need("pdf")) {
          const outs = await invoke<string[]>("pdf_split", { path: it.path, mode, outDir });
          log(`[나누기] ${it.path} → ${outs.length}개 파일`, "ok");
          outs.forEach((o) => log(`  ${o}`));
        }
        break;
      }
      case "rotate": {
        const degrees = Number((document.getElementById("deg") as HTMLSelectElement).value);
        for (const it of need("pdf")) {
          const out = await invoke<string>("pdf_rotate", { path: it.path, pages: val("pages") || "all", degrees, outDir });
          log(`[회전 ${degrees}°] → ${out}`, "ok");
        }
        break;
      }
      case "delete": {
        if (!val("pages")) throw new Error("지울 페이지를 적어 주세요 (예: 2,4-6)");
        for (const it of need("pdf")) {
          const r = await invoke<{ output: string; pages: number }>("pdf_delete", { path: it.path, pages: val("pages"), outDir });
          log(`[페이지 삭제] → ${r.output} (남은 ${r.pages}쪽)`, "ok");
        }
        break;
      }
      case "reorder": {
        if (!val("order")) throw new Error("새 순서를 적어 주세요 (예: 3,1,2)");
        for (const it of need("pdf")) {
          const out = await invoke<string>("pdf_reorder", { path: it.path, order: val("order"), outDir });
          log(`[순서 변경] → ${out}`, "ok");
        }
        break;
      }
    }
  } catch (e) {
    log(`오류: ${errText(e)}`, "err");
  } finally {
    setBusy(false);
  }
}

$("pick-files").onclick = async () => {
  const sel = await open({ multiple: true, directory: false, filters: [{ name: "PDF·zip", extensions: ["pdf", "zip"] }, { name: "모든 파일", extensions: ["*"] }] });
  if (sel) addPaths(Array.isArray(sel) ? sel : [sel]);
};
$("pick-folder").onclick = async () => {
  const sel = await open({ multiple: true, directory: true });
  if (sel) addPaths(Array.isArray(sel) ? sel : [sel]);
};
$("clear").onclick = () => { items = []; pendingPlan = null; renderItems(); };
$("pick-out").onclick = async () => {
  const sel = await open({ directory: true, multiple: false });
  if (typeof sel === "string") { outDir = sel; outdirEl.textContent = sel; }
};
$("reset-out").onclick = () => { outDir = null; outdirEl.textContent = "원본과 같은 폴더 (기본)"; };
opEl.onchange = renderOpts;
runBtn.onclick = () => run(false);

getCurrentWebview().onDragDropEvent((ev) => {
  const p = ev.payload;
  if (p.type === "enter" || p.type === "over") dropEl.classList.add("over");
  else dropEl.classList.remove("over");
  if (p.type === "drop") addPaths(p.paths);
});

renderOpts();
