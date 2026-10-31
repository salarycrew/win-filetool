import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";

type Kind = "dir" | "zip" | "pdf" | "image" | "heic" | "video" | "audio" | "document" | "other" | "missing";
interface Item { path: string; kind: Kind; pages: number | null }
interface Rename { from: string; to: string; is_dir: boolean }
interface Collision { path: string; existing: string }
interface FolderPlan { root: string; renames: Rename[]; collisions: Collision[]; unreadable: string[]; scanned: number }
interface ApplyReport { renamed: Rename[]; collisions: Collision[]; failed: [string, string][] }
interface ExtractReport { out_dir: string; files: string[]; dirs: number; normalized: number; cp949_names: number; skipped_mac: string[]; collisions: string[]; unsafe_names: string[] }
interface RezipReport { output: string; entries: number; normalized: number; skipped_mac: string[]; duplicates: string[] }
interface ConvertOut { input: string; output: string; note: string }
interface ToolStatus { tool: "ffmpeg" | "ffprobe" | "soffice"; label: string; found: boolean; path: string | null; source: string | null; version: string | null; guidance: string | null; download_url: string }
type ImgFormat = "jpg" | "png" | "webp" | "bmp" | "tiff" | "gif";
type MediaPreset =
  | { preset: "mp4" } | { preset: "mp3" } | { preset: "m4a" }
  | { preset: "messenger"; max_mb: number; max_height: number }
  | { preset: "gif"; start: number; duration: number; width: number; fps: number };
type Op =
  | { kind: "image"; format: ImgFormat; quality: number; max_side: number | null; keep_exif: boolean }
  | ({ kind: "media" } & MediaPreset)
  | { kind: "doc"; target: "pdf" | "docx" | "xlsx" | "pptx" };
interface WatchRule { id: string; folder: string; op: Op; out_subfolder: string; enabled: boolean }
interface WatchEvent { rule_id: string; input: string; ok: boolean; output: string | null; message: string }
type Tab = "files" | "pdf" | "image" | "media" | "doc" | "watch" | "tools";
interface CompressOut { input: string; output: string | null; before: number; after: number; pages: number; note: string }
interface Img2PdfOut { output: string; note: string }
interface RenderOut { folder: string; pages: { page: number; output: string; width: number; height: number }[]; note: string }
interface ExtractOut { folder: string | null; files: string[]; note: string }
interface AutostartStatus { enabled: boolean; value: string | null; matches_exe: boolean; supported: boolean }
const TABS: Tab[] = ["files", "pdf", "image", "media", "doc", "watch", "tools"];

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const itemsEl = $<HTMLUListElement>("items");
const opEl = $<HTMLSelectElement>("op");
const optsEl = $<HTMLDivElement>("opts");
const logEl = $<HTMLPreElement>("log");
const runBtn = $<HTMLButtonElement>("run");
const cancelBtn = $<HTMLButtonElement>("cancel");
const busyEl = $<HTMLSpanElement>("busy");
const outdirEl = $<HTMLSpanElement>("outdir");
const dropEl = $<HTMLElement>("drop");
const progressEl = $<HTMLDivElement>("progress");
const progressBar = $<HTMLDivElement>("progress-bar");
const progressText = $<HTMLSpanElement>("progress-text");

let tab: Tab = "files";
let items: Item[] = [];
let outDir: string | null = null;
let pendingPlan: FolderPlan | null = null;
let cancelled = false;
let jobSeq = 1;
let currentJob = 0;
let progressLabel = "";

const KIND_LABEL: Record<Kind, string> = {
  dir: "폴더", zip: "zip", pdf: "PDF", image: "이미지", heic: "HEIC", video: "영상", audio: "소리", document: "문서", other: "기타", missing: "없음",
};

function log(text: string, cls?: "ok" | "err", el: HTMLElement = logEl) {
  const line = document.createElement("div");
  if (cls) line.className = cls;
  line.textContent = text;
  el.appendChild(line);
  el.scrollTop = el.scrollHeight;
}

function errText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}

const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;

// ---------- 탭 ----------

// [값, 이름, 묶음(optgroup)]
const OPS: Record<"files" | "pdf" | "image" | "media" | "doc", [string, string, string?][]> = {
  files: [["nfc", "파일명 한글 깨짐 고치기 (폴더, NFD→NFC)"], ["zip", "맥에서 만든 zip 풀기 (한글 이름 고침)"]],
  pdf: [
    ["merge", "PDF 합치기 (목록 순서대로)", "페이지"], ["split", "PDF 나누기", "페이지"], ["rotate", "페이지 회전", "페이지"],
    ["delete", "페이지 삭제", "페이지"], ["reorder", "페이지 순서 변경", "페이지"],
    ["compress", "PDF 용량 줄이기 (사진 다시 압축)", "압축"],
    ["img2pdf", "이미지 여러 장 → PDF 하나 (목록 순서대로)", "이미지→PDF"],
    ["pdf2img", "쪽마다 그림으로 (PNG·JPG)", "PDF→이미지"], ["pdfimages", "들어 있는 사진만 꺼내기", "PDF→이미지"],
  ],
  image: [["img", "이미지 형식 바꾸기 · 크기 줄이기 (HEIC 포함)"]],
  media: [
    ["mp4", "MP4로 바꾸기 (H.264/AAC — 어디서나 재생)"], ["messenger", "메신저용으로 줄이기 (크기·해상도 제한)"],
    ["mp3", "소리만 MP3로"], ["m4a", "소리만 M4A로"], ["gif", "짧은 GIF로 만들기"],
  ],
  doc: [["pdf", "PDF로 바꾸기"], ["office", "MS Office 형식으로 (Pages→Word, Numbers→Excel, Keynote→PowerPoint)"]],
};
const DROP_HINT: Record<string, string> = {
  files: "zip · 폴더",
  pdf: "PDF · (이미지→PDF는 JPG · PNG · WebP · BMP · TIFF · HEIC)",
  image: "JPG · PNG · WebP · BMP · TIFF · GIF · HEIC(아이폰 사진)",
  media: "MOV · MKV · AVI · WebM · MP4 · 소리 파일",
  doc: "Word · Excel · PowerPoint · 한글(HWP 97) · ODF · Pages · Numbers · Keynote",
};

function setTab(t: Tab) {
  tab = t;
  document.querySelectorAll<HTMLButtonElement>("#tabs button").forEach((b) => b.classList.toggle("active", b.dataset.tab === t));
  const work = t !== "watch" && t !== "tools";
  $("work").hidden = !work;
  $("watch-panel").hidden = t !== "watch";
  $("tools-panel").hidden = t !== "tools";
  $("result-section").hidden = t === "watch";
  if (work) {
    let html = "", group: string | undefined;
    for (const [v, l, g] of OPS[t as keyof typeof OPS]) {
      if (g !== group) { if (group) html += "</optgroup>"; if (g) html += `<optgroup label="${g}">`; group = g; }
      html += `<option value="${v}">${l}</option>`;
    }
    opEl.innerHTML = html + (group ? "</optgroup>" : "");
    $("op-row").hidden = t === "image";
    $("drop-hint").textContent = DROP_HINT[t];
    renderOpts();
  }
  if (t === "tools") loadTools();
  if (t === "watch") loadWatch();
}
document.querySelectorAll<HTMLButtonElement>("#tabs button").forEach((b) => (b.onclick = () => setTab(b.dataset.tab as Tab)));

// ---------- 항목 ----------

async function addPaths(paths: string[]) {
  if (paths.length === 0) return;
  try {
    const found = await invoke<Item[]>("inspect_paths", { paths });
    for (const it of found) if (!items.some((x) => x.path === it.path)) items.push(it);
    pendingPlan = null;
    renderItems();
    autoPickTab();
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

/** 넣은 파일 종류에 맞는 탭으로(파일 작업 탭에 있을 때만). */
function autoPickTab() {
  const kinds = new Set(items.map((i) => i.kind));
  if (kinds.size !== 1) return;
  const only = [...kinds][0];
  // PDF 탭의 [이미지→PDF]에서는 사진을 넣어도 탭을 바꾸지 않는다
  if (tab === "pdf" && opEl.value === "img2pdf" && (only === "image" || only === "heic")) return;
  const target: Tab | null =
    only === "image" || only === "heic" ? "image" : only === "video" || only === "audio" ? "media" : only === "document" ? "doc" : only === "pdf" ? "pdf" : only === "dir" || only === "zip" ? "files" : null;
  if (target && target !== tab && tab !== "watch" && tab !== "tools") setTab(target);
  if (tab === "files") {
    if (only === "dir") opEl.value = "nfc";
    else if (only === "zip") opEl.value = "zip";
    renderOpts();
  } else if (tab === "pdf" && only === "pdf") {
    if (opEl.value === "img2pdf") opEl.value = items.length > 1 ? "merge" : "compress";
    renderOpts();
  }
}

// ---------- 작업 옵션 ----------

function renderOpts() {
  pendingPlan = null;
  const op = opEl.value;
  const pagesInput = (id: string, label: string, ph: string) =>
    `<label>${label} <input type="text" id="${id}" placeholder="${ph}" /></label>`;
  let html = "";
  if (tab === "files" || tab === "pdf") {
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
      case "compress":
        html = `<label>정도 <select id="c-preset">
            <option value="screen">화면용 — 가장 작게 (사진 96 DPI, 품질 55)</option>
            <option value="normal" selected>보통 — 메일·메신저용 (150 DPI, 품질 70)</option>
            <option value="print">인쇄용 — 화질 우선 (300 DPI, 품질 85)</option></select></label>
          <p class="hint">들어 있는 사진만 줄입니다. 글자·선·표는 그대로입니다. 결과가 원본보다 작지 않으면 새 파일을 만들지 않습니다.
            암호가 걸린 PDF는 줄일 수 없습니다. 결과: 이름_압축.pdf</p>`;
        break;
      case "img2pdf":
        html = `<label>쪽 크기 <select id="i-page">
            <option value="image">이미지 크기 그대로</option>
            <option value="a4_portrait" selected>A4 세로에 맞춤</option>
            <option value="a4_landscape">A4 가로에 맞춤</option></select></label>
          <label>여백 <select id="i-margin"><option value="0">없음</option><option value="10" selected>10mm</option><option value="20">20mm</option></select></label>
          <p class="hint">넣은 이미지가 목록 순서대로 한 쪽씩 들어갑니다(▲▼로 순서 조정). 사진 방향(EXIF)은 바르게 돌려 넣고,
            JPG는 다시 압축하지 않아 화질이 그대로입니다. HEIC는 ffmpeg가 필요합니다. 결과: 첫이미지_외N장.pdf</p>`;
        break;
      case "pdf2img":
        html = `<label>형식 <select id="r-fmt"><option value="png">PNG (선명, 큼)</option><option value="jpg">JPG (작음)</option></select></label>
          <label>해상도 <select id="r-dpi"><option value="72">72 DPI (화면)</option><option value="150" selected>150 DPI (보통)</option><option value="300">300 DPI (인쇄)</option></select></label>
          ${pagesInput("r-pages", "쪽", "예: 1-3,5 (비우면 전체)")}
          <p class="hint">결과는 원본 옆 “이름_쪽이미지” 폴더에 이름_p01.png … 로 저장합니다.</p>`;
        break;
      case "pdfimages":
        html = `<p class="hint">PDF 안에 들어 있는 사진을 다시 압축하지 않고 꺼냅니다(JPG는 그대로, 그 밖은 PNG). 결과: “이름_추출이미지” 폴더</p>`;
        break;
    }
  } else if (tab === "image") {
    html = `<label>바꿀 형식 <select id="img-fmt">
        <option value="jpg">JPG</option><option value="png">PNG</option><option value="webp">WebP</option>
        <option value="bmp">BMP</option><option value="tiff">TIFF</option><option value="gif">GIF</option></select></label>
      <label>품질 (JPG·WebP) <input type="range" id="img-q" min="40" max="100" value="90" /> <span id="img-q-v">90</span></label>
      <label>긴 변 최대 <select id="img-max"><option value="">원본 크기 그대로</option><option value="3840">3840px (4K)</option>
        <option value="2560">2560px</option><option value="1920" >1920px</option><option value="1280">1280px</option><option value="800">800px</option></select></label>
      <label><input type="checkbox" id="img-exif" checked /> 촬영 날짜 등 사진 정보(EXIF) 유지 (JPG·PNG로 바꿀 때)</label>
      <p class="hint">HEIC(아이폰 사진)는 ffmpeg로 풉니다([설정] 확인). 움직이는 GIF는 첫 장면만 씁니다. 사진 방향은 바르게 돌려 저장합니다.</p>`;
  } else if (tab === "media") {
    switch (op) {
      case "messenger":
        html = `<label>최대 크기 <select id="m-mb"><option value="10">10MB</option><option value="25" selected>25MB (카카오톡 등)</option><option value="50">50MB</option><option value="100">100MB</option></select></label>
          <label>최대 해상도 <select id="m-h"><option value="1080">1080p</option><option value="720" selected>720p</option><option value="480">480p</option></select></label>
          <p class="hint">영상 길이에 맞춰 화질을 정해 지정한 크기 안에 들어오게 합니다. 결과: 이름_메신저.mp4</p>`;
        break;
      case "gif":
        html = `<label>시작 (초) <input type="text" id="g-start" value="0" /></label>
          <label>길이 (초) <input type="text" id="g-dur" value="5" /></label>
          <label>가로 <select id="g-w"><option value="320">320px</option><option value="480" selected>480px</option><option value="640">640px</option></select></label>
          <label>초당 장면 <select id="g-fps"><option value="10">10</option><option value="12" selected>12</option><option value="15">15</option></select></label>`;
        break;
      default:
        html = `<p class="hint">ffmpeg로 변환합니다([설정] 확인). 긴 영상은 시간이 걸리며 [취소]로 멈출 수 있습니다.</p>`;
    }
  } else if (tab === "doc") {
    html = `<p class="hint">LibreOffice로 변환합니다(앱에 들어 있지 않음 — [설정] 확인).
      ${op === "office" ? "Pages→DOCX, Numbers→XLSX, Keynote→PPTX. 복잡한 서식은 달라질 수 있습니다." : "Word·Excel·PowerPoint·ODF·Pages·Numbers·Keynote → PDF. HWP는 오래된 한글 97 형식만 됩니다."}</p>`;
  }
  optsEl.innerHTML = html;
  const pv = document.getElementById("preview");
  if (pv) pv.onclick = () => run(true);
  const q = document.getElementById("img-q") as HTMLInputElement | null;
  if (q) q.oninput = () => ($("img-q-v").textContent = q.value);
}

const val = (id: string) => (document.getElementById(id) as HTMLInputElement | null)?.value.trim() ?? "";
const checked = (id: string) => (document.getElementById(id) as HTMLInputElement | null)?.checked ?? false;

function setBusy(b: boolean) {
  busyEl.hidden = !b;
  document.querySelectorAll("#work button, #work select, #work input, #tabs button").forEach((el) => ((el as HTMLButtonElement).disabled = b));
  cancelBtn.hidden = !b;
  cancelBtn.disabled = !b;
  if (!b) {
    progressEl.hidden = true;
    renderItems();
  }
}

function need(kind: Kind, min = 1): Item[] {
  const list = items.filter((i) => i.kind === kind);
  if (list.length < min) {
    const what = kind === "dir" ? "폴더" : kind === "zip" ? "zip 파일" : kind === "image" ? "이미지" : "PDF";
    throw new Error(min > 1 ? `${what}을(를) ${min}개 이상 넣어 주세요` : `${what}을(를) 먼저 넣어 주세요`);
  }
  return list;
}

function currentOp(): { op: Op; accept: Kind[] } {
  if (tab === "image") {
    const max = val("img-max");
    return {
      op: { kind: "image", format: val("img-fmt") as ImgFormat, quality: Number(val("img-q") || 90), max_side: max ? Number(max) : null, keep_exif: checked("img-exif") },
      accept: ["image", "heic"],
    };
  }
  if (tab === "media") {
    const p = opEl.value;
    const num = (id: string, label: string) => {
      const n = Number(val(id));
      if (!Number.isFinite(n) || n < 0) throw new Error(`${label}을(를) 숫자로 적어 주세요`);
      return n;
    };
    const preset: MediaPreset =
      p === "messenger" ? { preset: "messenger", max_mb: Number(val("m-mb")), max_height: Number(val("m-h")) }
      : p === "gif" ? { preset: "gif", start: num("g-start", "시작"), duration: Math.max(0.5, num("g-dur", "길이")), width: Number(val("g-w")), fps: Number(val("g-fps")) }
      : ({ preset: p } as MediaPreset);
    return { op: { kind: "media", ...preset }, accept: p === "mp3" || p === "m4a" ? ["video", "audio"] : ["video"] };
  }
  return { op: { kind: "doc", target: opEl.value === "pdf" ? "pdf" : "docx" }, accept: ["document"] };
}

/** 문서 → Office: 종류별로 docx/xlsx/pptx를 고른다. */
function docTarget(path: string): "pdf" | "docx" | "xlsx" | "pptx" | null {
  if (opEl.value === "pdf") return "pdf";
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  if (["pages", "doc", "odt", "rtf", "txt", "hwp", "fodt", "wpd"].includes(ext)) return "docx";
  if (["numbers", "xls", "ods", "csv", "fods"].includes(ext)) return "xlsx";
  if (["key", "ppt", "odp", "pps", "fodp"].includes(ext)) return "pptx";
  return null;
}

function showProgress(i: number, n: number, name: string, fraction: number | null) {
  progressEl.hidden = false;
  progressLabel = `[${i}/${n}] ${name}`;
  progressBar.style.width = `${Math.round(((i - 1 + (fraction ?? 0)) / n) * 100)}%`;
  progressText.textContent = fraction == null ? `${progressLabel}` : `${progressLabel} — ${Math.round(fraction * 100)}%`;
}

async function runBatch() {
  const { op, accept } = currentOp();
  const list = items.filter((i) => accept.includes(i.kind));
  const skipped = items.filter((i) => !accept.includes(i.kind) && i.kind !== "missing");
  if (list.length === 0) throw new Error("이 작업에 맞는 파일을 먼저 넣어 주세요");
  skipped.forEach((s) => log(`[건너뜀] 이 작업에 쓸 수 없는 파일: ${fileName(s.path)}`));
  let ok = 0, fail = 0;
  for (let i = 0; i < list.length; i++) {
    if (cancelled) { log(`취소: 남은 ${list.length - i}개는 처리하지 않았습니다`, "err"); break; }
    const it = list[i];
    let thisOp = op;
    if (op.kind === "doc") {
      const t = docTarget(it.path);
      if (!t || t === it.path.split(".").pop()?.toLowerCase()) { log(`✗ ${fileName(it.path)}: 이 문서는 그 형식으로 바꿀 수 없습니다`, "err"); fail++; continue; }
      thisOp = { kind: "doc", target: t };
    }
    currentJob = jobSeq++;
    showProgress(i + 1, list.length, fileName(it.path), op.kind === "media" ? 0 : null);
    const t0 = performance.now();
    try {
      const r = await invoke<ConvertOut>("convert_one", { job: currentJob, path: it.path, op: thisOp, outDir });
      ok++;
      log(`✓ ${fileName(it.path)} → ${r.output}  (${r.note}, ${((performance.now() - t0) / 1000).toFixed(1)}초)`, "ok");
    } catch (e) {
      fail++;
      log(`✗ ${fileName(it.path)}: ${errText(e)}`, "err");
    }
  }
  log(`끝: 성공 ${ok} / 실패 ${fail}${skipped.length ? ` / 건너뜀 ${skipped.length}` : ""}`, fail ? "err" : "ok");
}

async function run(previewOnly = false) {
  setBusy(true);
  cancelled = false;
  try {
    if (tab !== "files" && tab !== "pdf") {
      await runBatch();
      return;
    }
    const op = opEl.value;
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
        const rezip = checked("rezip");
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
      case "compress": {
        const list = need("pdf");
        const preset = val("c-preset");
        let saved = 0;
        for (let i = 0; i < list.length; i++) {
          if (cancelled) { log(`취소: 남은 ${list.length - i}개는 처리하지 않았습니다`, "err"); break; }
          showProgress(i + 1, list.length, fileName(list[i].path), null);
          try {
            const r = await invoke<CompressOut>("pdf_compress", { path: list[i].path, preset, outDir });
            if (r.output) saved += r.before - r.after;
            log(r.output ? `✓ ${fileName(r.input)} → ${r.output}  (${r.note})` : `= ${fileName(r.input)}: ${r.note}`, r.output ? "ok" : undefined);
          } catch (e) {
            log(`✗ ${fileName(list[i].path)}: ${errText(e)}`, "err");
          }
        }
        if (list.length > 1) log(`끝: 모두 ${(saved / 1048576).toFixed(1)} MB 줄임`, "ok");
        break;
      }
      case "img2pdf": {
        const imgs = items.filter((i) => i.kind === "image" || i.kind === "heic");
        if (imgs.length === 0) throw new Error("PDF로 묶을 이미지를 먼저 넣어 주세요");
        const skipped = items.filter((i) => i.kind !== "image" && i.kind !== "heic");
        skipped.forEach((s) => log(`[건너뜀] 이미지가 아님: ${fileName(s.path)}`));
        currentJob = jobSeq++;
        showProgress(1, 1, `이미지 ${imgs.length}장`, 0);
        const r = await invoke<Img2PdfOut>("images_to_pdf", {
          job: currentJob, paths: imgs.map((i) => i.path), outDir,
          opts: { page: val("i-page"), margin_mm: Number(val("i-margin") || 0) },
        });
        log(`✓ 이미지 ${imgs.length}장 → ${r.output}  (${r.note})`, "ok");
        break;
      }
      case "pdf2img": {
        const list = need("pdf");
        const opts = { format: val("r-fmt"), dpi: Number(val("r-dpi")), quality: 90, pages: val("r-pages") || null };
        for (let i = 0; i < list.length; i++) {
          if (cancelled) { log(`취소: 남은 ${list.length - i}개는 처리하지 않았습니다`, "err"); break; }
          currentJob = jobSeq++;
          showProgress(i + 1, list.length, fileName(list[i].path), 0);
          try {
            const r = await invoke<RenderOut>("pdf_to_images", { job: currentJob, path: list[i].path, opts, outDir });
            log(`✓ ${fileName(list[i].path)} → ${r.folder}  (${r.note})`, "ok");
          } catch (e) {
            log(`✗ ${fileName(list[i].path)}: ${errText(e)}`, "err");
          }
        }
        break;
      }
      case "pdfimages": {
        for (const it of need("pdf")) {
          try {
            const r = await invoke<ExtractOut>("pdf_extract_images", { path: it.path, outDir });
            log(r.folder ? `✓ ${fileName(it.path)} → ${r.folder}  (${r.note})` : `= ${fileName(it.path)}: ${r.note}`, r.folder ? "ok" : undefined);
          } catch (e) {
            log(`✗ ${fileName(it.path)}: ${errText(e)}`, "err");
          }
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

listen<{ job: number; fraction: number }>("job-progress", (ev) => {
  if (ev.payload.job !== currentJob || progressEl.hidden) return;
  const m = progressLabel.match(/^\[(\d+)\/(\d+)\]/);
  if (!m) return;
  const i = Number(m[1]), n = Number(m[2]);
  progressBar.style.width = `${Math.round(((i - 1 + ev.payload.fraction) / n) * 100)}%`;
  progressText.textContent = `${progressLabel} — ${Math.round(ev.payload.fraction * 100)}%`;
});

cancelBtn.onclick = async () => {
  cancelled = true;
  cancelBtn.disabled = true;
  try { await invoke<number>("cancel_jobs"); } catch { /* 무시 */ }
};

// ---------- 도구 설정 ----------

const autostartEl = $<HTMLInputElement>("autostart");

function showAutostart(st: AutostartStatus) {
  autostartEl.checked = st.enabled;
  autostartEl.disabled = !st.supported;
  $("autostart-state").textContent = !st.supported
    ? "Windows에서만 쓸 수 있습니다."
    : st.enabled
      ? `켜짐 — 로그인하면 창 없이 알림 영역에서 시작해 켜진 감시 폴더 규칙을 돌립니다.${st.matches_exe ? "" : " ※ 등록된 경로가 지금 앱 위치와 다릅니다. 껐다 다시 켜면 고쳐집니다."} (HKCU\\…\\Run: ${st.value ?? ""})`
      : "꺼짐 (기본). 관리자 권한 없이 현재 사용자에게만 등록합니다.";
}

autostartEl.onchange = async () => {
  try { showAutostart(await invoke<AutostartStatus>("autostart_set", { enabled: autostartEl.checked })); }
  catch (e) { log(errText(e), "err"); autostartEl.checked = !autostartEl.checked; }
};

async function loadTools() {
  invoke<AutostartStatus>("autostart_get").then(showAutostart).catch((e) => ($("autostart-state").textContent = errText(e)));
  invoke<string>("pdfium_status")
    .then((p) => ($("pdfium-state").textContent = `✓ PDF→이미지 렌더러(PDFium): ${p}`))
    .catch((e) => ($("pdfium-state").textContent = `✗ PDF→이미지 렌더러(PDFium): ${errText(e)}`));
  const el = $("tool-list");
  el.innerHTML = '<p class="hint">찾는 중…</p>';
  try {
    renderTools(await invoke<ToolStatus[]>("tools_status"));
  } catch (e) {
    el.innerHTML = "";
    log(`도구를 확인하지 못했습니다: ${errText(e)}`, "err");
  }
}

function renderTools(list: ToolStatus[]) {
  const el = $("tool-list");
  el.innerHTML = "";
  for (const t of list) {
    const box = document.createElement("div");
    box.className = "tool " + (t.found ? "found" : "missing");
    const head = document.createElement("div");
    head.className = "tool-head";
    head.textContent = `${t.found ? "✓ 있음" : "✗ 없음"} — ${t.label}`;
    box.appendChild(head);
    const p = document.createElement("div");
    p.className = "hint";
    if (t.found) {
      p.textContent = `${t.path}  (${t.source ?? ""}${t.version ? `, 버전 ${t.version}` : ", 버전 확인 실패"})`;
    } else {
      p.textContent = `${t.guidance ?? ""}  내려받기: ${t.download_url}`;
    }
    box.appendChild(p);
    if (t.tool !== "ffprobe") {
      const row = document.createElement("div");
      row.className = "row";
      const pick = document.createElement("button");
      pick.type = "button";
      pick.textContent = "경로 지정…";
      pick.onclick = async () => {
        const sel = await open({ multiple: false, directory: false, filters: [{ name: t.tool === "soffice" ? "soffice.exe" : "ffmpeg.exe", extensions: ["exe"] }] });
        if (typeof sel !== "string") return;
        try { renderTools(await invoke<ToolStatus[]>("set_tool_path", { tool: t.tool, path: sel })); log(`${t.tool} 경로 저장: ${sel}`, "ok"); }
        catch (e) { log(errText(e), "err"); }
      };
      const reset = document.createElement("button");
      reset.type = "button";
      reset.className = "ghost";
      reset.textContent = "자동으로 찾기";
      reset.onclick = async () => {
        try { renderTools(await invoke<ToolStatus[]>("set_tool_path", { tool: t.tool, path: null })); } catch (e) { log(errText(e), "err"); }
      };
      row.append(pick, reset);
      box.appendChild(row);
    }
    el.appendChild(box);
  }
}
$("tools-refresh").onclick = loadTools;

// ---------- 감시 폴더 ----------

let rules: WatchRule[] = [];
let wFolder: string | null = null;
const watchLog = $<HTMLPreElement>("watch-log");

function opFromSpec(spec: string): Op {
  const [k, v] = spec.split(":");
  if (k === "image") return { kind: "image", format: v as ImgFormat, quality: 90, max_side: null, keep_exif: true };
  if (k === "media") return v === "messenger" ? { kind: "media", preset: "messenger", max_mb: 25, max_height: 720 } : ({ kind: "media", preset: v } as Op);
  return { kind: "doc", target: "pdf" };
}

function opLabel(op: Op): string {
  if (op.kind === "image") return `사진 → ${op.format.toUpperCase()}`;
  if (op.kind === "doc") return `문서 → ${op.target.toUpperCase()}`;
  return { mp4: "영상 → MP4", messenger: "영상 → 메신저용", mp3: "→ MP3", m4a: "→ M4A", gif: "→ GIF" }[op.preset];
}

function watchLine(ev: WatchEvent) {
  log(ev.ok ? `✓ ${fileName(ev.input)} → ${ev.output}  (${ev.message})` : `✗ ${fileName(ev.input)}: ${ev.message}`, ev.ok ? "ok" : "err", watchLog);
}

const pausedEl = $<HTMLInputElement>("w-paused");
pausedEl.onchange = () => invoke<boolean>("watch_set_paused", { paused: pausedEl.checked }).catch(() => {});
listen<boolean>("watch-paused", (ev) => (pausedEl.checked = ev.payload));

async function loadWatch() {
  try {
    const v = await invoke<{ rules: WatchRule[]; log: WatchEvent[]; paused: boolean }>("watch_get");
    pausedEl.checked = v.paused;
    rules = v.rules;
    watchLog.innerHTML = "";
    v.log.forEach(watchLine);
    renderRules();
  } catch (e) {
    log(errText(e), "err", watchLog);
  }
}

async function saveRules(next: WatchRule[]) {
  try {
    rules = await invoke<WatchRule[]>("watch_set_rules", { rules: next });
  } catch (e) {
    log(`저장하지 못했습니다: ${errText(e)}`, "err", watchLog);
  }
  renderRules();
}

function renderRules() {
  const ul = $<HTMLUListElement>("rules");
  ul.innerHTML = "";
  if (rules.length === 0) {
    ul.innerHTML = '<li class="empty">아직 규칙이 없습니다</li>';
    return;
  }
  rules.forEach((r, i) => {
    const li = document.createElement("li");
    const on = document.createElement("input");
    on.type = "checkbox";
    on.checked = r.enabled;
    on.title = "켜기/끄기";
    on.onchange = () => saveRules(rules.map((x, j) => (j === i ? { ...x, enabled: on.checked } : x)));
    const kind = document.createElement("span");
    kind.className = "kind";
    kind.textContent = r.enabled ? "켜짐" : "꺼짐";
    const path = document.createElement("span");
    path.className = "path";
    path.textContent = `${r.folder}  ·  ${opLabel(r.op)}  →  ${r.out_subfolder}\\`;
    path.title = path.textContent;
    const del = document.createElement("button");
    del.type = "button";
    del.textContent = "✕";
    del.title = "규칙 지우기";
    del.onclick = () => saveRules(rules.filter((_, j) => j !== i));
    li.append(on, kind, path, del);
    ul.appendChild(li);
  });
}

$("w-folder").onclick = async () => {
  const sel = await open({ directory: true, multiple: false });
  if (typeof sel === "string") { wFolder = sel; $("w-folder-path").textContent = sel; }
};
$("w-add").onclick = () => {
  if (!wFolder) { log("감시할 폴더를 먼저 고르세요", "err", watchLog); return; }
  const sub = val("w-sub") || "변환됨";
  const id = `r${Date.now().toString(36)}`;
  saveRules([...rules, { id, folder: wFolder, op: opFromSpec(val("w-op")), out_subfolder: sub, enabled: true }]);
};
listen<WatchEvent>("watch-event", (ev) => watchLine(ev.payload));

// ---------- 공통 ----------

$("pick-files").onclick = async () => {
  const sel = await open({ multiple: true, directory: false });
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
  if (p.type === "drop" && tab !== "watch" && tab !== "tools") addPaths(p.paths);
});

// 시작 탭: 실행 인자 --tab=image 등(화면 점검용), 없으면 파일 탭
setTab("files");
invoke<string | null>("startup_tab").then((t) => {
  if (t && (TABS as string[]).includes(t)) setTab(t as Tab);
}).catch(() => {});
