// 백엔드 부르기 + 공용 형식 + 쉬운 오류 문구.
// 개발 중 브라우저(vite dev)에서는 Tauri가 없으므로 mock.ts가 window.__FT_MOCK__ 을 채운다(배포 빌드에는 들어가지 않음).
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import { open as tauriOpen } from "@tauri-apps/plugin-dialog";

type Mock = {
  invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
  listen: (ev: string, cb: (e: { payload: unknown }) => void) => () => void;
  open: (o: Record<string, unknown>) => Promise<string | string[] | null>;
};
const mock = (): Mock | undefined => (window as unknown as { __FT_MOCK__?: Mock }).__FT_MOCK__;

export function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const m = mock();
  return (m ? m.invoke(cmd, args) : tauriInvoke(cmd, args)) as Promise<T>;
}

export function listen<T>(ev: string, cb: (payload: T) => void): void {
  const m = mock();
  if (m) m.listen(ev, (e) => cb(e.payload as T));
  else tauriListen<T>(ev, (e) => cb(e.payload)).catch(() => {});
}

export async function pickFiles(opts: { multiple?: boolean; directory?: boolean; filters?: { name: string; extensions: string[] }[] } = {}): Promise<string[]> {
  const m = mock();
  const sel = m ? await m.open(opts) : await tauriOpen({ multiple: opts.multiple ?? true, directory: opts.directory ?? false, filters: opts.filters });
  if (!sel) return [];
  return Array.isArray(sel) ? sel : [sel];
}

// ---------- 형식 ----------

export type Kind = "dir" | "zip" | "pdf" | "image" | "heic" | "video" | "audio" | "document" | "other" | "missing";
export interface Item { path: string; kind: Kind; pages: number | null; size?: number; locked?: boolean }
export interface FileStat { path: string; exists: boolean; is_dir: boolean; size: number; modified: number }
export interface EditOut { output: string; pages: number; note: string }

// ---------- 작업 번호·진행 ----------

let jobSeq = Math.floor(Date.now() % 100000) * 10;
export const newJob = () => ++jobSeq;

type ProgressCb = (fraction: number, detail?: { done: number; total: number; current?: string }) => void;
const progressCbs = new Map<number, ProgressCb>();
export function onJobProgress(job: number, cb: ProgressCb) { progressCbs.set(job, cb); }
export function offJobProgress(job: number) { progressCbs.delete(job); }
listen<{ job: number; fraction: number }>("job-progress", (p) => progressCbs.get(p.job)?.(p.fraction));
// W4: 여러 파일·긴 작업 { job, done, total, current }
listen<{ job: number; done: number; total: number; current?: string }>("tools_progress", (p) =>
  progressCbs.get(p.job)?.(p.total > 0 ? p.done / p.total : 0, p));

/** 돌고 있는 작업 멈추기: W4 `cancel_job {job}`이 있으면 그것, 아니면 모든 작업(`cancel_jobs`) */
export async function cancelJob(job?: number) {
  if (job != null) await invoke("cancel_job", { job }).catch(() => {});
  await invoke("cancel_jobs").catch(() => {});
}

// ---------- 파일 정보 ----------

export async function stats(paths: string[]): Promise<Map<string, FileStat>> {
  const m = new Map<string, FileStat>();
  if (!paths.length) return m;
  try {
    for (const s of await invoke<FileStat[]>("file_stats", { paths })) m.set(s.path, s);
  } catch { /* 크기는 없어도 됨 */ }
  return m;
}

export const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;
export const dirOf = (p: string) => p.replace(/[\\/][^\\/]*$/, "");
export const stem = (p: string) => fileName(p).replace(/\.[^.]+$/, "");

const nf = new Intl.NumberFormat("ko-KR");
export const num = (n: number) => nf.format(n);
export function human(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "";
  if (n < 1024) return `${n} B`;
  const u = ["KB", "MB", "GB", "TB"];
  let v = n / 1024, i = 0;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${u[i]}`;
}
export function fmtTime(t: number): string {
  if (!t) return "";
  const d = new Date(t * 1000), p = (x: number) => String(x).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}
export function fmtDur(sec: number): string {
  sec = Math.max(0, sec);
  const m = Math.floor(sec / 60), s = sec - m * 60;
  return m >= 60 ? `${Math.floor(m / 60)}시간 ${m % 60}분` : m ? `${m}분 ${Math.round(s)}초` : `${s < 10 ? s.toFixed(1) : Math.round(s)}초`;
}

export async function openFile(path: string) { await invoke("open_path", { path }); }
export async function openFolderSelect(path: string) { await invoke("open_folder", { path }); }
export async function openDir(path: string) { await invoke("open_dir", { path }); }

// ---------- 오류 ----------

/** 백엔드 오류: 문자열(옛 명령) 또는 { code, message }(PDF 편집·W4) */
export interface CmdErr { code: string; message: string }
export function asErr(e: unknown): CmdErr {
  if (e && typeof e === "object" && "message" in e) {
    const o = e as { code?: unknown; message: unknown };
    return { code: typeof o.code === "string" ? o.code : "error", message: String(o.message) };
  }
  if (typeof e === "string") return { code: /취소했습니다|취소됨|cancel/i.test(e) ? "cancelled" : "error", message: e };
  return { code: "error", message: e instanceof Error ? e.message : JSON.stringify(e) };
}
export const isCancelled = (e: unknown) => asErr(e).code === "cancelled";
export const needsPassword = (e: unknown) => { const c = asErr(e).code; return c === "password_required" || c === "wrong_password"; };

export interface Friendly { title: string; body: string; detail: string; icon: string; kind: "locked" | "missing-tool" | "damaged" | "unsupported" | "busy" | "cancelled" | "space" | "other" }

/** 원래 오류 글은 [자세히]로만 보이고, 화면에는 무엇이 문제인지 + 할 일 한 문장 */
export function friendly(e: unknown): Friendly {
  const { code, message: m } = asErr(e);
  const f = (kind: Friendly["kind"], icon: string, title: string, body: string): Friendly => ({ kind, icon, title, body, detail: m });
  if (code === "cancelled" || /취소했습니다|취소됨/.test(m)) return f("cancelled", "x", "멈췄어요", "만들던 파일은 남기지 않았어요.");
  if (code === "password_required" || /암호가 걸린|암호.*(필요|있어)|encrypted|password/i.test(m))
    return f("locked", "lock", "암호가 걸린 PDF예요", "암호를 알고 있으면 [PDF 편집]에서 열거나 [암호 걸기·풀기]로 먼저 풀어 주세요. 여러 개를 넣었을 때는 이 파일만 건너뛰어요.");
  if (code === "wrong_password") return f("locked", "lock", "암호가 맞지 않아요", "대문자·소문자와 한/영 상태를 확인하고 다시 넣어 주세요.");
  if (/찾지 못했습니다|없습니다.*(ffmpeg|LibreOffice|soffice)|not found.*(ffmpeg|soffice)/i.test(m) && /(ffmpeg|LibreOffice|soffice|PDFium|도구)/i.test(m))
    return f("missing-tool", "tool", "변환 도구를 쓸 수 없어요", "앱에 든 도구를 준비하지 못했어요. 앱을 다시 켜 보고, 그래도 안 되면 [설정] > 고급 > 변환 도구를 확인해 주세요.");
  if (/도구를 준비하지 못했습니다|디스크 공간/.test(m)) return f("space", "warn", "도구를 준비하지 못했어요", "디스크 공간(약 0.5GB)과 폴더 권한을 확인한 뒤 다시 해 주세요.");
  if (/손상|형식이 아닙니다|읽지 못했|corrupt|invalid|unsupported format|could not (open|read|decode)/i.test(m))
    return f("damaged", "warn", "파일을 읽지 못했어요", "파일이 손상됐거나 이 앱이 읽을 수 없는 형식이에요. 원래 프로그램에서 다시 저장해 넣어 보세요.");
  if (/지원하지 않|바꿀 수 없|쓸 수 없는 파일|not supported/i.test(m)) return f("unsupported", "info", "이 파일로는 할 수 없어요", "다른 할 일을 고르거나 다른 파일을 넣어 주세요.");
  if (/기다리다 멈췄|사용 중|being used|in use|잠겨|locked by/i.test(m)) return f("busy", "warn", "파일이 다른 곳에서 쓰이고 있어요", "열려 있는 프로그램을 닫고 잠시 뒤 다시 해 주세요.");
  if (/권한|denied|access/i.test(m)) return f("other", "shield", "이 폴더에 저장할 수 없어요", "저장 위치를 [바꾸기]로 다른 폴더(예: 바탕 화면)로 바꿔 보세요.");
  if (/공간|space|full/i.test(m)) return f("space", "warn", "디스크 공간이 모자라요", "필요 없는 파일을 지우거나 다른 드라이브에 저장해 주세요.");
  return f("other", "warn", "끝내지 못했어요", "잠시 뒤 다시 해 보세요. 계속 안 되면 [자세히]의 내용을 알려 주세요.");
}
