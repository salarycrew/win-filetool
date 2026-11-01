// 0.3.3 문서 도구(LibreOffice): 앱에 넣지 않고, 필요한 일이 생겼을 때 한 번 묻고 받는다.
// 문서 → PDF는 먼저 이 PC의 한글·Office로 하고, 그것으로 안 될 때만 백엔드가 "문서 도구가 필요해요"로 알린다.
import { asErr, cancelJob, friendly, invoke, listen, newJob, pickFiles } from "./api";
import { I } from "./icons";
import { $, dialog, el, esc, toast } from "./ui";

export interface DocToolsView {
  ready: boolean; soffice: string | null; dir: string; source: string | null; lo_version: string;
  download_mb: number; unpacked_mb: number; partial_bytes: number; url: string; asset: string; sha256: string;
  engines: { hancom: boolean; word: boolean; excel: boolean; powerpoint: boolean };
  office: { word: boolean; excel: boolean; powerpoint: boolean; version: string | null; x64: boolean | null };
}

export const isNeedDocTools = (e: unknown) => /문서 도구가 필요해요/.test(asErr(e).message);

let declined = false;
let pending: Promise<boolean> | null = null;

/** 새 작업을 시작할 때: 지난번에 [취소]했어도 다시 물어볼 수 있게 */
export function resetDocToolsAsk() { declined = false; }

export const docToolsStatus = () => invoke<DocToolsView>("doctools_status");

/** 받아야 하면 한 번 묻고 받는다. 받았으면(또는 이미 있으면) true. 같은 작업 안에서는 [취소] 뒤 다시 묻지 않는다. */
export function ensureDocTools(): Promise<boolean> {
  if (declined) return Promise.resolve(false);
  pending ??= askAndGet().finally(() => { pending = null; });
  return pending;
}

/** `fn`이 "문서 도구가 필요해요"로 실패하면 묻고 받은 뒤 한 번 더 한다 */
export async function withDocTools<T>(fn: () => Promise<T>): Promise<T> {
  try {
    return await fn();
  } catch (e) {
    if (isNeedDocTools(e) && (await ensureDocTools())) return await fn();
    throw e;
  }
}

async function askAndGet(): Promise<boolean> {
  const st = await docToolsStatus().catch(() => null);
  if (st?.ready) return true;
  const mb = st?.download_mb ?? 106;
  const resume = st && st.partial_bytes > 0 ? ` 지난번에 받던 것(${Math.round(st.partial_bytes / 1e6)} MB)부터 이어 받아요.` : "";
  const ok = await dialog({
    title: "문서 도구가 필요해요",
    icon: "doc",
    html: `<p>이 작업에는 문서 도구가 필요해요. 한 번만 받으면 돼요 (약 ${mb} MB). 받을까요?</p>
      <p class="hint">이 앱의 GitHub 공개 저장소에서 받고, 받은 파일이 맞는지 확인한 뒤 이 PC에만 풀어요. 내 파일은 PC 밖으로 나가지 않아요.${esc(resume)}</p>`,
    ok: "받기",
    cancel: "취소",
  });
  if (!ok) { declined = true; return false; }
  return download();
}

/** 받기 진행 창(멈추기 가능). 성공하면 true */
export async function download(): Promise<boolean> {
  for (;;) {
    const job = newJob();
    const back = el(`<div class="dlg-back" role="presentation"><div class="dlg" role="dialog" aria-modal="true" aria-labelledby="dt-t">
      <div class="dlg-b"><span class="dlg-ic">${I("doc")}</span><div class="dlg-m"><h2 id="dt-t">문서 도구 받는 중</h2>
        <p class="dt-step" aria-live="polite">연결하는 중…</p><div class="bar"><i style="width:0%"></i></div></div></div>
      <div class="dlg-f"><button type="button" class="btn" data-stop>멈추기</button></div></div></div>`);
    document.body.appendChild(back);
    $<HTMLButtonElement>("[data-stop]", back)!.focus();
    let stopped = false;
    $("[data-stop]", back)!.addEventListener("click", () => { stopped = true; cancelJob(job); });
    const off = onProgress(job, (done, total) => {
      const f = total ? done / total : 0;
      $<HTMLElement>(".bar i", back)!.style.width = `${Math.round(f * 100)}%`;
      $(".dt-step", back)!.textContent = done >= total && total > 0 ? "받은 파일을 확인하고 푸는 중…(몇 초)" : `${Math.round(done / 1e6)} / ${Math.round(total / 1e6)} MB`;
    });
    try {
      await invoke("doctools_download", { job });
      off(); back.remove();
      toast("문서 도구를 받았어요. 이어서 할게요");
      return true;
    } catch (e) {
      off(); back.remove();
      if (stopped) { toast("멈췄어요. 다음에 받던 곳부터 이어 받아요"); declined = true; return false; }
      const again = await failDialog(e);
      if (again === "retry") continue;
      if (again === "file") return installFromFile();
      declined = true;
      return false;
    }
  }
}

async function failDialog(e: unknown): Promise<"retry" | "file" | "close"> {
  const m = asErr(e).message;
  const hash = /SHA-256/.test(m);
  let choice: "retry" | "file" | "close" = "close";
  const box = await dialog({
    title: hash ? "받은 파일이 원래 파일과 달라요" : "인터넷 연결을 확인하고 다시 해 주세요",
    icon: "warn",
    html: `<p>${hash ? "받은 파일이 이 앱이 아는 문서 도구와 달라서 쓰지 않고 지웠어요. 다시 받아 주세요." : "문서 도구를 받지 못했어요. 인터넷 연결을 확인하고 다시 해 주세요."}</p>
      <p class="hint">인터넷이 없는 PC라면 다른 PC에서 받은 문서 도구 파일을 [파일로 넣기]로 넣을 수 있어요.</p>
      <p><button type="button" class="btn subtle" data-file>${I("folder")}파일로 넣기</button></p>
      <details><summary class="hint">자세히</summary><p class="hint">${esc(m)}</p></details>`,
    ok: "다시 해 보기",
    cancel: "닫기",
    onOpen: (b) => $("[data-file]", b)!.addEventListener("click", () => { choice = "file"; ($("[data-cancel]", b) as HTMLButtonElement).click(); }),
  });
  if (box) choice = "retry";
  return choice;
}

/** [파일로 넣기]: 고른 파일을 확인해서 푼다(고른 파일은 지우지 않음) */
export async function installFromFile(): Promise<boolean> {
  const [p] = await pickFiles({ multiple: false, filters: [{ name: "문서 도구 (win-filetool-doctools-*.zst)", extensions: ["zst"] }] });
  if (!p) return false;
  const back = el(`<div class="prep" role="status" aria-live="polite"><span class="spin"></span><span class="prep-t">문서 도구 파일을 확인하고 푸는 중…</span></div>`);
  document.body.appendChild(back);
  try {
    await invoke("doctools_install_file", { path: p });
    toast("문서 도구를 넣었어요");
    return true;
  } catch (e) {
    await dialog({ title: friendly(e).title, body: asErr(e).message, icon: "warn", ok: "확인", cancel: null });
    return false;
  } finally {
    back.remove();
  }
}

type Cb = (done: number, total: number) => void;
const cbs = new Map<number, Cb>();
listen<{ job: number; done: number; total: number }>("doctools-progress", (p) => cbs.get(p.job)?.(p.done, p.total));
function onProgress(job: number, cb: Cb) { cbs.set(job, cb); return () => cbs.delete(job); }
