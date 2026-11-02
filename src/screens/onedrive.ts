// 0.3.4 "OneDrive가 가져간 폴더 되돌리기"([더 보기] 안). 폴더마다 카드 하나 + 경우마다 큰 단추 하나.
// A 백업 켜짐 → [OneDrive 백업 끄기 열기] 그리고 [다시 확인] · B OneDrive에 남음 → [○○(으)로 가져오기](복사만)
// C 정상 → "할 일 없어요" · D OneDrive 폴더 없음 → 휴지통 안내. OneDrive 안은 지우지도 옮기지도 않는다.
import { asErr, cancelJob, friendly, human, invoke, isCancelled, newJob, num, offJobProgress, onJobProgress, openDir } from "../api";
import { I } from "../icons";
import { Screen } from "../router";
import { $, chk, dialog, errorPanel, esc, progressPanel, setProgress, toast } from "../ui";

export type Case = "A" | "B" | "C" | "D";
export interface Count { files: number; bytes: number; cloud_files: number; cloud_bytes: number; truncated: boolean }
export interface FolderReport {
  key: string; name: string; case: Case; title: string; detail: string;
  current: string; current_exists: boolean; in_onedrive: boolean; here: Count;
  onedrive_copy?: string; onedrive_count?: Count;
  local_default: string; local_default_exists: boolean; local_default_count?: Count;
}
export interface Report {
  folders: FolderReport[]; accounts: { root: string; business: boolean; signed_in: boolean; exists: boolean }[];
  onedrive_running: boolean; onedrive_set_up: boolean; summary: string; fake: boolean;
  backup_off_steps: string; delete_warning: string; recycle_help: string;
}
export interface RestoreOut {
  id: string; from: string; to: string; copied: number; same: number; renamed: number; copied_bytes: number;
  cloud_skipped: number; cloud_skipped_bytes: number; skipped: { path: string; reason: string }[]; cancelled: boolean; log?: string; summary: string;
}

const BADGE: Record<Case, [string, string, string]> = {
  A: ["백업 켜짐", "orange", "down"],
  B: ["OneDrive에 남아 있음", "blue", "copy"],
  C: ["정상", "green", "check"],
  D: ["OneDrive 폴더 없음", "red", "warn"],
};

/** "바탕 화면으로" · "문서로" (받침 없거나 ㄹ이면 로) */
export function toward(name: string): string {
  const c = name.charCodeAt(name.length - 1) - 0xac00;
  const jong = c >= 0 && c <= 11171 ? c % 28 : 0;
  return `${name}${jong === 0 || jong === 8 ? "로" : "으로"}`;
}

function countLine(c: Count | undefined): string {
  if (!c) return "";
  let s = `파일 ${num(c.files)}개${c.truncated ? " 이상" : ""} · ${human(c.bytes)}`;
  if (c.cloud_files) s += ` · 그중 ${num(c.cloud_files)}개(${human(c.cloud_bytes)})는 클라우드에만 있어요`;
  return s;
}

/** 경우마다 카드(단추·안내). 시험하기 쉽게 순수 함수 */
export function cardHtml(f: FolderReport, r: Report, result?: string): string {
  const [label, tone, icon] = BADGE[f.case];
  const where = f.case === "B"
    ? `<dl class="od-dl"><dt>지금 ${esc(f.name)}</dt><dd title="${esc(f.current)}">${esc(f.current)} — ${esc(countLine(f.here))}</dd>
        <dt>OneDrive에 남은 것</dt><dd title="${esc(f.onedrive_copy)}">${esc(f.onedrive_copy)} — ${esc(countLine(f.onedrive_count))}</dd></dl>`
    : `<dl class="od-dl"><dt>지금 위치</dt><dd title="${esc(f.current)}">${esc(f.current)}${f.current_exists ? ` — ${esc(countLine(f.here))}` : " (없음)"}</dd>
        ${f.local_default_count ? `<dt>이 PC 기본 위치</dt><dd>${esc(f.local_default)} — ${esc(countLine(f.local_default_count))}</dd>` : ""}</dl>`;
  let act = "";
  if (f.case === "A") {
    act = `<div class="row"><button type="button" class="btn accent" data-od="backup">${I("open")}OneDrive 백업 끄기 열기</button>
        <button type="button" class="btn" data-od="recheck">${I("refresh")}다시 확인</button></div>
      <details class="more od-steps"><summary>OneDrive에서 직접 끄는 방법</summary><p>${esc(r.backup_off_steps)}</p></details>
      <p class="hint">이 앱은 OneDrive 설정을 바꾸지 않아요. 백업을 끈 뒤 [다시 확인]을 누르면 파일을 가져올 수 있어요.</p>`;
  } else if (f.case === "B") {
    act = `<div class="row"><button type="button" class="btn accent" data-od="restore" data-key="${esc(f.key)}">${I("copy")}${esc(toward(f.name))} 가져오기</button>
        <button type="button" class="btn" data-od="open" data-path="${esc(f.onedrive_copy)}">${I("folder")}OneDrive 쪽 폴더 보기</button></div>
      <p class="hint">복사만 해요 — OneDrive 쪽 파일은 그대로 둬요. 같은 이름이 있으면 덮어쓰지 않고, 내용이 같으면 건너뛰고, 다르면 "(OneDrive에서)"를 붙여 둘 다 남겨요.</p>`;
  } else if (f.case === "C") {
    act = `<p class="od-ok">${I("check")}할 일 없어요</p>`;
  } else {
    act = `<p>${esc(r.recycle_help)}</p><div class="row"><button type="button" class="btn accent" data-od="web">${I("open")}OneDrive 웹 열기</button>
        <button type="button" class="btn" data-od="recycle">${I("trash")}PC 휴지통 열기</button></div>`;
  }
  return `<article class="panel od-card" data-key="${esc(f.key)}" data-case="${f.case}">
    <div class="od-h"><span class="tile tone-${tone}">${I(icon)}</span><div class="od-t"><b>${esc(f.name)}</b><span class="od-badge tone-${tone}">${esc(label)}</span></div></div>
    <p class="od-title">${esc(f.title)}</p>${where}${act}<div class="od-res">${result ?? ""}</div></article>`;
}

/** 가져오기 결과 판 */
export function resultHtml(o: RestoreOut, warning: string): string {
  const skipped = o.skipped.length
    ? `<details class="more"><summary>못 가져온 ${num(o.skipped.length)}개</summary><ul class="od-skip">${o.skipped.slice(0, 200).map((s) => `<li><b>${esc(s.path)}</b> — ${esc(s.reason)}</li>`).join("")}</ul></details>`
    : "";
  const undo = o.log && o.copied + o.renamed > 0 ? `<button type="button" class="btn" data-od="undo" data-id="${esc(o.id)}">${I("undo")}되돌리기</button>` : "";
  return `<div class="od-done" role="status"><p><b>${esc(`${num(o.copied)}개 가져옴, ${num(o.same)}개 같아서 건너뜀, ${num(o.renamed)}개 이름 바꿔 둠`)}</b>
      ${o.cloud_skipped ? `<br>클라우드에만 있는 ${num(o.cloud_skipped)}개(${esc(human(o.cloud_skipped_bytes))})는 받지 않았어요.` : ""}${o.cancelled ? "<br>중간에 멈췄어요." : ""}</p>
    ${skipped}<div class="row"><button type="button" class="btn accent" data-od="open" data-path="${esc(o.to)}">${I("folder")}폴더 열기</button>${undo}</div>
    <p class="hint od-warn">${I("warn")}${esc(warning)}</p></div>`;
}

let root: HTMLElement;
let report: Report | null = null;
const results = new Map<string, string>();
let busy = false;

async function check() {
  const list = $(".od-list", root)!;
  list.innerHTML = `<div class="panel"><span class="spin"></span> 폴더를 살펴보는 중…</div>`;
  try {
    report = await invoke<Report>("onedrive_check");
  } catch (e) {
    list.innerHTML = errorPanel(friendly(e));
    return;
  }
  draw();
}

function draw() {
  if (!report) return;
  const r = report;
  const acc = r.accounts.length
    ? r.accounts.map((a) => `${a.business ? "회사·학교" : "개인"} 계정 폴더 ${a.root}${a.exists ? "" : "(없음)"}`).join(" · ")
    : "OneDrive 계정 없음";
  $(".od-sum", root)!.innerHTML = `<p><b>${esc(r.summary)}</b></p><p class="hint">${esc(acc)} · OneDrive ${r.onedrive_running ? "실행 중" : "꺼져 있음"}${r.fake ? " · 시험용 가짜 환경" : ""}</p>`;
  $(".od-list", root)!.innerHTML = r.folders.map((f) => cardHtml(f, r, results.get(f.key))).join("");
}

async function restore(key: string) {
  if (busy || !report) return;
  const f = report.folders.find((x) => x.key === key);
  if (!f || f.case !== "B") return;
  const c = f.onedrive_count!;
  let allowDownload = false;
  if (c.cloud_files > 0) {
    const box = await dialog({
      title: `${num(c.cloud_files)}개 파일 ${human(c.cloud_bytes)}를 OneDrive에서 내려받아야 해요`,
      icon: "down",
      html: `<p>이 파일들은 클라우드에만 있어요. 가져오려면 인터넷이 필요하고, 받는 동안 [취소]로 멈출 수 있어요.</p>
        ${chk("od-dl", `받아서 같이 가져오기 (끄면 이 PC에 있는 ${num(c.files - c.cloud_files)}개만 가져와요)`, true)}`,
      ok: "가져오기",
    });
    if (!box) return;
    allowDownload = ($<HTMLInputElement>("#od-dl", box)?.checked ?? false);
  }
  busy = true;
  const card = $(`.od-card[data-key="${key}"]`, root)!;
  const res = $(".od-res", card)!;
  res.innerHTML = progressPanel(`${toward(f.name)} 가져오는 중`);
  card.querySelectorAll<HTMLButtonElement>("[data-od]").forEach((b) => (b.disabled = true));
  const job = newJob();
  onJobProgress(job, (fr, d) => setProgress(res, fr, d?.current ? `${num(d.done)} / ${num(d.total)} · ${d.current}` : undefined));
  $("[data-cancel]", res)!.addEventListener("click", () => cancelJob(job));
  let html = "";
  try {
    const o = await invoke<RestoreOut>("onedrive_restore", { folder: key, allowDownload, job });
    html = resultHtml(o, report.delete_warning);
  } catch (e) {
    html = isCancelled(e) ? `<p>멈췄어요.</p>` : errorPanel(friendly(e));
  } finally {
    offJobProgress(job);
    busy = false;
  }
  results.set(key, html);
  await check();
}

async function undo(id: string, key: string) {
  const ok = await dialog({ title: "가져온 파일을 되돌릴까요?", body: "이번에 복사한 파일 중 그 뒤로 바뀌지 않은 것만 지워요. OneDrive 쪽 파일과 원래 있던 파일은 그대로예요.", ok: "되돌리기", icon: "undo" });
  if (!ok) return;
  try {
    const u = await invoke<{ summary: string }>("onedrive_undo", { id });
    results.set(key, `<p class="od-done" role="status"><b>${esc(u.summary)}</b></p>`);
    toast(u.summary, 4000);
  } catch (e) {
    toast(asErr(e).message || friendly(e).title, 5000);
  }
  await check();
}

export const onedriveScreen = {
  id: "onedrive", title: "OneDrive 폴더 되돌리기", icon: "copy",
  init(el: HTMLElement) {
    root = el;
    root.innerHTML = `<h1 class="title">OneDrive가 가져간 폴더 되돌리기</h1>
      <p class="sub">바탕 화면·문서·사진이 비어 보이면, 파일이 어디 있는지 찾아 이 PC 폴더로 복사해 드려요. OneDrive 쪽은 지우거나 옮기지 않아요.</p>
      <div class="panel od-sum"></div><div class="od-list"></div>`;
    root.addEventListener("click", async (e) => {
      const b = (e.target as HTMLElement).closest<HTMLButtonElement>("[data-od]");
      if (!b || b.disabled) return;
      const card = b.closest<HTMLElement>(".od-card");
      try {
        switch (b.dataset.od) {
          case "recheck": results.clear(); await check(); break;
          case "backup": await invoke("onedrive_open", { what: "backup" }); toast("OneDrive 백업을 끈 뒤 [다시 확인]을 눌러 주세요", 5000); break;
          case "web": await invoke("onedrive_open", { what: "web" }); break;
          case "recycle": await invoke("onedrive_open", { what: "recycle" }); break;
          case "open": await openDir(b.dataset.path!); break;
          case "restore": await restore(b.dataset.key!); break;
          case "undo": await undo(b.dataset.id!, card?.dataset.key ?? ""); break;
        }
      } catch (err) {
        toast(friendly(err).title, 4000);
      }
    });
  },
  show() {
    if (!busy) check();
  },
} satisfies Screen as Screen;
