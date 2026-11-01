// 0.3.2 핵심 할 일 화면(6개가 이 한 화면을 같이 씀): 파일 목록 → 설정 한두 개 → 실행 → 한 줄 요약 + [폴더 열기].
// 원본은 그대로, 결과는 원본 옆 새 이름. 여러 개는 하나가 실패해도 계속하고 끝에 쉬운 말로 요약한다.
import {
  Item, cancelJob, dirOf, fileName, friendly, human, invoke, isCancelled, needsPassword, newJob, offJobProgress, onJobProgress,
  openDir, pickFiles, stats,
} from "../api";
import { FT, I, extOf } from "../icons";
import { go, Screen } from "../router";
import { $, ResultLine, askPassword, bindResultButtons, bindSegs, errRow, esc, opt, progressPanel, resultRow, seg, segVal, setProgress, toast } from "../ui";
import { FailKind, JobDef, JobId, acceptsItem, byName, isNfd, jobDef, mbToBytes, summary } from "./core";

type PItem = Item & { pw?: string; excluded?: Set<number>; open?: boolean; nfdCount?: number };
interface Res extends ResultLine { err?: unknown }

let root: HTMLElement;
let job: JobDef = jobDef("merge")!;
let items: PItem[] = [];
let phase: "pick" | "run" | "done" = "pick";
let running = false;
let stopFlag = false;
let runJob = 0;
let results: Res[] = [];
let resultTitle = "";
let resultFolder: string | null = null;

// ---------- 설정(할 일마다 한두 개) ----------

function optionsHTML(): string {
  switch (job.id) {
    case "compress":
      return opt("목표 크기", seg("target", [["5", "5MB 이하"], ["10", "10MB 이하"], ["c", "직접 입력"]], "5") +
        `<input class="txt sm" id="tmb" placeholder="MB" inputmode="decimal" hidden aria-label="목표 크기(MB)">`, "글자·선은 그대로 두고 사진만 줄여요. 목표까지 안 되면 가장 작게 만든 것을 남겨요.");
    case "img2pdf":
      return opt("쪽 크기", seg("page", [["a4_portrait", "A4에 맞추기"], ["image", "사진 크기 그대로"]], "a4_portrait"), "목록 순서대로 한 장씩 넣어요. 사진 방향은 바르게 돌려 넣어요.");
    case "photo":
      return opt("크기", seg("side", [["1920", "보통(1920px)"], ["1280", "작게(1280px)"], ["0", "크기 그대로"]], "1920"), "긴 쪽을 이 크기 이하로 줄이고 JPG로 저장해요. 촬영 날짜는 남겨요.");
    case "office2pdf":
      return `<div class="info">${I("info")}<span>한글 파일은 이 PC에 한글이 있으면 한글로, 없으면 앱에 든 도구로 바꿔요. 여러 개도 한 번에 돼요.</span></div>`;
    case "merge":
      return `<div class="info">${I("info")}<span>목록 순서대로 합쳐요. 빼고 싶은 쪽은 [쪽 고르기]에서 눌러 빼세요.</span></div>`;
    case "macname":
      return `<div class="info">${I("info")}<span>폴더 안 이름, 고른 파일 이름을 바로잡아요. zip은 한글 이름을 바르게 풀어 새 폴더에 담아요.</span></div>`;
  }
}

// ---------- 화면 ----------

function rowsHTML(): string {
  return items.map((it, i) => {
    const pages = it.pages ?? null;
    const kept = pages != null && it.excluded?.size ? `${pages - it.excluded.size}쪽 / ${pages}쪽` : pages != null ? `${pages}쪽` : "";
    const meta = [it.size ? human(it.size) : "", kept, it.locked ? "암호 걸림" : "", it.nfdCount ? `고칠 이름 ${it.nfdCount}개` : ""].filter(Boolean).join(" · ");
    const ordered = job.order && phase === "pick";
    return `<li class="fl-i" data-i="${i}">${FT(it.kind === "dir" ? "folder" : extOf(it.path), 22)}${it.locked ? `<span class="mini-lock" title="암호 걸림">${I("lock")}</span>` : ""}
      <span class="fl-n" title="${esc(it.path)}">${esc(fileName(it.path))}</span><span class="fl-m">${esc(meta)}</span>
      ${phase === "pick" ? `<span class="fl-b">${job.id === "merge" && pages ? `<button type="button" class="btn subtle sm" data-pages="${i}" aria-expanded="${!!it.open}">${I("pages")}쪽 고르기</button>` : ""}
      ${ordered ? `<button type="button" class="ib" data-up="${i}" title="위로" aria-label="${esc(fileName(it.path))} 위로" ${i === 0 ? "disabled" : ""}>${I("up")}</button><button type="button" class="ib" data-down="${i}" title="아래로" aria-label="${esc(fileName(it.path))} 아래로" ${i === items.length - 1 ? "disabled" : ""}>${I("down")}</button>` : ""}
      <button type="button" class="ib" data-rm="${i}" title="빼기" aria-label="${esc(fileName(it.path))} 빼기">${I("x")}</button></span>` : ""}</li>
      ${it.open && phase === "pick" ? `<li class="pg-strip" data-strip="${i}"><span class="hint">불러오는 중…</span></li>` : ""}`;
  }).join("");
}

function render() {
  const head = `<div class="job-top"><button type="button" class="btn subtle" data-act="home">${I("back")}홈</button></div>
    <h1 class="title"><span class="tile tone-${job.tone}">${I(job.icon)}</span>${esc(job.title)}</h1>`;
  if (phase === "run") {
    root.innerHTML = `${head}${progressPanel(`${job.title} 하는 중`)}`;
    return;
  }
  if (phase === "done") {
    root.innerHTML = `${head}${resultsHTML()}`;
    $<HTMLElement>("[data-act=open-out]", root)?.focus();
    return;
  }
  if (!items.length) {
    root.innerHTML = `${head}<p class="sub">${esc(job.desc)}. 원본은 그대로 두고 새 파일로 저장해요.</p>
      <div class="dz job-dz" id="jdz" tabindex="0" role="button" aria-label="파일 고르기 — 또는 여기에 끌어다 놓기">
        <div class="dz-ic">${I("upload")}</div><div class="dz-t">${esc(job.accepts)}을(를) 여기에 끌어다 놓으세요</div>
        <div class="row center"><button type="button" class="btn accent" data-act="pick">${job.id === "macname" ? "폴더 고르기" : "파일 고르기"}</button>${job.id === "macname" ? `<button type="button" class="btn" data-act="pick-files">파일·zip 고르기</button>` : ""}</div></div>`;
    return;
  }
  const need = job.min ?? 1;
  const short = items.length < need;
  root.innerHTML = `${head}<p class="sub">${esc(job.desc)}. 원본은 그대로 두고 새 파일로 저장해요.</p>
    <div class="filecard multi"><div class="fc-head"><div class="fc-t"><b>${items.length}개</b><span>${job.order ? "위에서부터 차례대로" : esc(job.accepts)}</span></div>
      <button type="button" class="btn subtle" data-act="more">${I("plus")}더 넣기</button>${job.order && items.length > 1 ? `<button type="button" class="btn subtle" data-act="sort">${I("sort")}이름순</button>` : ""}<button type="button" class="btn subtle" data-act="clear">${I("x")}모두 빼기</button></div>
      <ul class="fl job-fl">${rowsHTML()}</ul></div>
    <div class="optcard job-opt" id="optcard"><div class="oc-body"><div class="opts">${optionsHTML()}</div><p class="oc-err" role="alert" hidden></p></div>
      <div class="oc-foot"><div class="save">${I("folder")}<b>${job.id === "macname" ? "그 자리에서 이름만 바꿔요" : "원본 옆에 저장"}</b></div>
      ${short ? `<span class="hint">${need}개 이상 넣어 주세요</span>` : ""}<button type="button" class="btn accent lg" data-act="run" ${short ? "disabled" : ""}>${esc(job.run)}</button></div></div>`;
  bindSegs($("#optcard", root)!, () => syncOpts());
  syncOpts();
  for (const it of items) if (it.open) loadStrip(items.indexOf(it));
}

function syncOpts() {
  const t = $<HTMLInputElement>("#tmb", root);
  if (t) t.hidden = segVal(root, "target") !== "c";
}

function resultsHTML(): string {
  const oks = results.filter((r) => !r.err);
  const errs = results.filter((r) => r.err);
  const warn = errs.length > 0 || oks.some((r) => /못 줄였|못 했/.test(r.note ?? ""));
  return `<div class="panel" id="results"><div class="done-h"><span class="okc ${warn ? "warn" : ""}">${I(warn ? "warn" : "check")}</span><div><b>${esc(resultTitle)}</b><span>${esc(job.id === "macname" ? "원래 자리에서 이름을 바꿨어요" : "원본은 그대로 두고 원본 옆에 저장했어요")}</span></div>
      ${resultFolder ? `<button type="button" class="btn accent lg" data-act="open-out">${I("folder")}폴더 열기</button>` : ""}</div>
    <div class="rlist">${oks.map(resultRow).join("")}${errs.map((r) => errRow(r.input ?? r.output, r.err)).join("")}</div>
    <div class="after-row"><button type="button" class="btn subtle" data-act="again">${I("plus")}다른 파일로 한 번 더</button><button type="button" class="btn subtle" data-act="home">${I("back")}홈으로</button></div></div>`;
}

// ---------- 파일 넣기 ----------

async function addPaths(paths: string[], sort = true) {
  if (!paths.length) return;
  let found: Item[];
  try { found = await invoke<Item[]>("inspect_paths", { paths }); } catch (e) { toast(friendly(e).title); return; }
  const ok = found.filter((x) => acceptsItem(job, x));
  const skipped = found.length - ok.length;
  const st = await stats(ok.map((x) => x.path));
  let fresh: PItem[] = ok.filter((x) => !items.some((y) => y.path === x.path)).map((x) => ({ ...x, size: st.get(x.path)?.size }));
  if (sort) fresh = byName(fresh);
  items.push(...fresh);
  if (skipped) toast(`이 일에 맞지 않는 ${skipped}개는 뺐어요`);
  if (phase === "done") phase = "pick";
  render();
  // 뒤에서: 암호 걸린 PDF, 폴더 안 고칠 이름 수
  for (const it of fresh) {
    if (it.kind === "pdf") {
      invoke<{ needs_password: boolean }>("pdf_protection", { path: it.path })
        .then((p) => { it.locked = p.needs_password; if (p.needs_password && phase === "pick") render(); })
        .catch((e) => { it.locked = needsPassword(e); if (it.locked && phase === "pick") render(); });
    }
    if (job.id === "macname" && it.kind === "dir") {
      invoke<{ renames: unknown[] }>("nfc_preview", { folder: it.path })
        .then((p) => { it.nfdCount = p.renames.length + (isNfd(it.path) ? 1 : 0); if (phase === "pick") render(); })
        .catch(() => {});
    }
  }
}

async function loadStrip(i: number) {
  const it = items[i];
  const strip = $(`[data-strip="${i}"]`, root);
  if (!it || !strip || !it.pages) return;
  const n = Math.min(it.pages, 200);
  try {
    const pw = it.locked ? await ensurePw(it) : null;
    const th = await invoke<{ page: number; data_url: string }[]>("pdf_thumbs", { path: it.path, password: pw, pages: Array.from({ length: n }, (_, k) => k + 1), width: 72 });
    if (!strip.isConnected) return;
    strip.innerHTML = `<span class="hint">빼고 싶은 쪽을 누르세요</span><div class="thumbs">${th.map((t) =>
      `<button type="button" class="th ${it.excluded?.has(t.page - 1) ? "off" : ""}" data-th="${i}:${t.page - 1}" aria-pressed="${!it.excluded?.has(t.page - 1)}" title="${t.page}쪽"><img src="${t.data_url}" alt="${t.page}쪽"><span>${t.page}</span></button>`).join("")}</div>`;
  } catch (e) {
    strip.innerHTML = `<span class="hint">${esc(friendly(e).title)}</span>`;
  }
}

async function ensurePw(it: PItem): Promise<string | null> {
  if (it.pw) return it.pw;
  const got = await askPassword(fileName(it.path), false);
  if (got == null) throw { code: "cancelled", message: "취소했습니다" };
  it.pw = got;
  return got;
}

// ---------- 실행 ----------

const failKind = (e: unknown): FailKind => friendly(e).kind;

async function run() {
  if (running || !items.length) return;
  const vals = { target: segVal(root, "target"), tmb: $<HTMLInputElement>("#tmb", root)?.value ?? "", page: segVal(root, "page"), side: segVal(root, "side") };
  let targetBytes = 0;
  if (job.id === "compress") {
    const mb = vals.target === "c" ? Number(vals.tmb.replace(",", ".")) : Number(vals.target);
    if (!Number.isFinite(mb) || mb < 0.1 || mb > 2000) {
      const err = $(".oc-err", root)!; err.hidden = false; err.textContent = "목표 크기를 MB 숫자로 적어 주세요 (예: 3)"; return;
    }
    targetBytes = mbToBytes(mb);
  }
  running = true; stopFlag = false; results = []; resultFolder = null;
  phase = "run";
  render();
  const pg = $(".panel.pg", root)!;
  $("[data-cancel]", pg)!.addEventListener("click", async () => {
    stopFlag = true;
    ($("[data-cancel]", pg) as HTMLButtonElement).disabled = true;
    setProgress(pg, null, "멈추는 중…");
    await cancelJob(runJob);
  });
  const newRun = () => {
    if (runJob) offJobProgress(runJob);
    runJob = newJob();
    return runJob;
  };
  const total = items.length;
  const unit = job.id === "photo" || job.id === "img2pdf" ? "장" : "개";
  try {
    switch (job.id) {
      case "merge": {
        setProgress(pg, 0, `PDF ${total}개`);
        const sources: { path: string; password: string | null }[] = [];
        for (const it of items) sources.push({ path: it.path, password: it.locked ? await ensurePw(it) : null });
        const pages: { kind: "page"; src: number; index: number }[] = [];
        items.forEach((it, s) => { for (let k = 0; k < (it.pages ?? 0); k++) if (!it.excluded?.has(k)) pages.push({ kind: "page", src: s, index: k }); });
        const j = newRun();
        onJobProgress(j, (f) => setProgress(pg, f));
        const o = await invoke<{ output: string; pages: number }>("pdf_merge_plan", { job: j, plan: { sources, pages }, outDir: null });
        results.push({ input: items[0].path, output: o.output, pill: `${o.pages}쪽`, before: items.reduce((a, x) => a + (x.size ?? 0), 0) || null });
        resultTitle = `PDF ${total}개를 한 파일로 합쳤어요 (${o.pages}쪽)`;
        break;
      }
      case "img2pdf": {
        setProgress(pg, 0, `사진 ${total}장`);
        const j = newRun();
        onJobProgress(j, (f) => setProgress(pg, f, `${Math.min(total, Math.round(f * total) + 1)} / ${total}장`));
        const o = await invoke<{ output: string; pages?: unknown[] }>("images_to_pdf", { job: j, paths: items.map((x) => x.path), outDir: null, opts: { page: vals.page, margin_mm: vals.page === "image" ? 0 : 10 } });
        results.push({ input: items[0].path, output: o.output, pill: `${total}쪽`, before: items.reduce((a, x) => a + (x.size ?? 0), 0) || null });
        resultTitle = `사진 ${total}장을 PDF 하나로 만들었어요`;
        break;
      }
      case "photo": {
        const j = newRun();
        onJobProgress(j, (f, d) => setProgress(pg, f, d ? `${Math.round(d.done)} / ${d.total}장${d.current ? ` · ${fileName(d.current)}` : ""}` : undefined));
        const side = Number(vals.side);
        const out = await invoke<{ input: string; output: string | null; before: number; after: number; note?: string; error?: string; unchanged: boolean }[]>("photo_shrink", {
          job: j, paths: items.map((x) => x.path), maxSide: side || null, quality: 85, outDir: null,
        });
        for (const o of out) {
          if (o.error) results.push({ input: o.input, output: o.input, err: o.error });
          else if (o.unchanged || !o.output) results.push({ input: o.input, output: o.input, pill: "그대로", note: o.note ?? "이미 작아요" });
          else results.push({ input: o.input, output: o.output, before: o.before, after: o.after });
        }
        break;
      }
      case "compress":
      case "office2pdf":
      case "macname":
        await eachItem(pg, newRun, targetBytes);
        break;
    }
  } catch (e) {
    if (!isCancelled(e)) results.push({ input: items[0].path, output: items[0].path, err: e });
  } finally {
    if (runJob) offJobProgress(runJob);
    running = false;
  }
  // 요약: "10개 중 9개 완료, 1개는 암호가 걸려 있어요"
  const oks = results.filter((r) => !r.err);
  const fails = results.filter((r) => r.err).map((r) => failKind(r.err));
  if (job.id === "merge" || job.id === "img2pdf") {
    if (fails.length) resultTitle = friendly(results.find((r) => r.err)!.err).title;
  } else {
    resultTitle = summary(total, oks.length, fails, unit);
  }
  if (stopFlag && !results.length) resultTitle = "멈췄어요. 만들던 파일은 남기지 않았어요";
  const need = results.filter((x) => !x.err && !x.isFolder && x.after == null && x.output !== x.input).map((x) => x.output);
  const st = await stats(need);
  for (const x of results) if (x.after == null && st.has(x.output)) x.after = st.get(x.output)!.size;
  const firstOut = results.find((r) => !r.err && (r.output !== r.input || r.isFolder));
  resultFolder = firstOut ? (firstOut.isFolder ? firstOut.output : dirOf(firstOut.output)) : results[0] ? dirOf(results[0].input ?? results[0].output) : null;
  phase = "done";
  render();
}

/** 하나씩(실패한 파일은 건너뛰고 계속) */
async function eachItem(pg: HTMLElement, newRun: () => number, targetBytes: number) {
  const n = items.length;
  for (let i = 0; i < n; i++) {
    if (stopFlag) break;
    const it = items[i];
    const base = i / n;
    setProgress(pg, base, n > 1 ? `${i + 1} / ${n} · ${fileName(it.path)}` : fileName(it.path));
    const j = newRun();
    onJobProgress(j, (f) => setProgress(pg, base + f / n));
    try {
      if (job.id === "compress") {
        const o = await invoke<{ output: string | null; before: number; after: number; note: string; reached: boolean; already_small: boolean }>("pdf_compress_target", { job: j, path: it.path, target: targetBytes, outDir: null });
        if (o.already_small) results.push({ input: it.path, output: it.path, pill: "그대로", note: o.note, before: o.before });
        else if (!o.output) results.push({ input: it.path, output: it.path, pill: "그대로", note: o.note });
        else results.push({ input: it.path, output: o.output, before: o.before, after: o.after, note: o.reached ? undefined : o.note });
      } else if (job.id === "office2pdf") {
        const o = await invoke<{ output: string }>("convert_one", { job: j, path: it.path, op: { kind: "doc", target: "pdf" }, outDir: null });
        results.push({ input: it.path, output: o.output, before: it.size ?? null });
      } else if (job.id === "macname") {
        await fixName(it);
      }
    } catch (e) {
      if (isCancelled(e)) break;
      results.push({ input: it.path, output: it.path, err: e });
    }
  }
}

async function fixName(it: PItem) {
  const isZip = it.kind === "zip" || extOf(it.path) === "zip";
  if (isZip) {
    const o = await invoke<Record<string, unknown>>("zip_extract", { job: newJob(), path: it.path, encoding: "auto", outDir: null });
    const folder = typeof o.folder === "string" ? o.folder : ((o.extract ?? o) as { out_dir: string }).out_dir;
    const count = typeof o.count === "number" ? o.count : ((o.extract ?? o) as { files?: unknown[] }).files?.length ?? 0;
    results.push({ input: it.path, output: folder, isFolder: true, pill: `파일 ${count}개`, note: "한글 이름을 바르게 풀었어요" });
    return;
  }
  let renamed = 0, failed = 0;
  let path = it.path;
  if (it.kind === "dir") {
    const rep = await invoke<{ renamed: unknown[]; failed: unknown[] }>("nfc_apply", { folder: path });
    renamed += rep.renamed.length; failed += rep.failed.length;
  }
  if (isNfd(path)) {
    const [r] = await invoke<{ from: string; to: string | null; note?: string | null }[]>("nfc_fix_paths", { paths: [path] });
    if (r?.to) { renamed++; path = r.to; } else if (r?.note) failed++;
  }
  results.push({
    input: it.path, output: path, isFolder: it.kind === "dir",
    pill: renamed ? `이름 ${renamed}개 고침` : "고칠 이름 없음", note: failed ? `${failed}개는 바꾸지 못했어요(같은 이름이 있거나 쓰는 중)` : renamed ? undefined : "이미 모두 바른 이름이에요",
  });
}

// ---------- 화면 ----------

export const jobScreen = {
  id: "job", title: "할 일", icon: "convert",
  init(el: HTMLElement) {
    root = el;
    bindResultButtons(root);
    root.addEventListener("click", async (e) => {
      const t = e.target as HTMLElement;
      const th = t.closest<HTMLElement>("[data-th]");
      if (th) {
        const [i, k] = th.dataset.th!.split(":").map(Number);
        const it = items[i];
        it.excluded ??= new Set();
        if (it.excluded.has(k)) it.excluded.delete(k);
        else if (it.excluded.size + 1 >= (it.pages ?? 0)) { toast("쪽을 하나 이상 남겨 주세요"); return; }
        else it.excluded.add(k);
        th.classList.toggle("off", it.excluded.has(k));
        th.setAttribute("aria-pressed", String(!it.excluded.has(k)));
        const meta = $(`.fl-i[data-i="${i}"] .fl-m`, root);
        if (meta) meta.textContent = [it.size ? human(it.size) : "", it.excluded.size ? `${(it.pages ?? 0) - it.excluded.size}쪽 / ${it.pages}쪽` : `${it.pages}쪽`].filter(Boolean).join(" · ");
        return;
      }
      const b = t.closest<HTMLElement>("[data-act], [data-up], [data-down], [data-rm], [data-pages]");
      if (!b) { if (t.closest("#jdz")) addPaths(await pickFiles(job.id === "macname" ? { directory: true } : {})); return; }
      const move = (i: number, d: number) => { [items[i], items[i + d]] = [items[i + d], items[i]]; render(); };
      if (b.dataset.up) return move(Number(b.dataset.up), -1);
      if (b.dataset.down) return move(Number(b.dataset.down), 1);
      if (b.dataset.rm) { items.splice(Number(b.dataset.rm), 1); render(); return; }
      if (b.dataset.pages) { const it = items[Number(b.dataset.pages)]; it.open = !it.open; render(); return; }
      switch (b.dataset.act) {
        case "home": if (!running) go("home"); break;
        case "pick": addPaths(await pickFiles(job.id === "macname" ? { directory: true } : {})); break;
        case "pick-files": case "more": addPaths(await pickFiles()); break;
        case "sort": items = byName(items); render(); break;
        case "clear": items = []; render(); break;
        case "again": items = []; phase = "pick"; render(); break;
        case "open-out": if (resultFolder) openDir(resultFolder).catch((err) => toast(friendly(err).title)); break;
        case "run": run(); break;
      }
    });
    root.addEventListener("keydown", async (e) => {
      if ((e.target as HTMLElement).id === "jdz" && (e.key === "Enter" || e.key === " ")) { e.preventDefault(); addPaths(await pickFiles(job.id === "macname" ? { directory: true } : {})); }
    });
    render();
  },
  /** arg: { job, paths?, add? } — add=true면 지금 목록에 더함(우클릭으로 늦게 온 파일) */
  show(arg?: unknown) {
    const a = arg as { job?: JobId; paths?: string[]; add?: boolean } | undefined;
    const next = a?.job ? jobDef(a.job) : undefined;
    if (running) { if (a?.paths?.length) toast("지금 하는 작업이 끝난 뒤에 넣어 주세요"); render(); return; }
    if (next && (next.id !== job.id || !a?.add)) { job = next; items = []; phase = "pick"; results = []; }
    if (phase === "done" && a?.paths?.length) { items = []; phase = "pick"; }
    render();
    if (a?.paths?.length) addPaths(a.paths);
  },
  drop(paths: string[]) {
    if (running) { toast("지금 하는 작업이 끝난 뒤에 넣어 주세요"); return; }
    if (phase === "done") { items = []; phase = "pick"; }
    addPaths(paths);
  },
  /** 지금 할 일(우클릭 묶음을 이어 붙일지 판단) */
  current(): { job: JobId; busy: boolean } { return { job: job.id, busy: running || phase === "run" }; },
} satisfies Screen & { current(): { job: JobId; busy: boolean } };

