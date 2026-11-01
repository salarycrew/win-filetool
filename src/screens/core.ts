// 0.3.2 핵심 6개 할 일 — 정의(제목·받는 파일·분류)와 넣은 파일에 맞는 할 일 고르기.
// 화면(홈·작업)과 시험(node:test)이 같이 쓴다. DOM·Tauri를 부르지 않는다.

export type JobId = "merge" | "compress" | "img2pdf" | "office2pdf" | "photo" | "macname";

export interface JobDef {
  id: JobId;
  title: string;      // 큰 단추 글자
  desc: string;       // 한 줄 설명
  icon: string;
  tone: string;
  run: string;        // 실행 단추
  accepts: string;    // 빈 화면 안내("PDF를 …")
  exts: string[] | null; // 받는 확장자(null = 폴더·zip·이름)
  folders?: boolean;
  min?: number;       // 실행에 필요한 최소 개수
  order?: boolean;    // 순서가 중요(위·아래 단추)
}

export const IMAGE_EXT = ["jpg", "jpeg", "jfif", "png", "heic", "heif", "webp", "bmp", "gif", "tif", "tiff"];
export const OFFICE_EXT = ["hwp", "hwpx", "doc", "docx", "docm", "rtf", "odt", "xls", "xlsx", "xlsm", "ods", "ppt", "pptx", "pptm", "odp", "pages", "numbers", "key"];

export const JOBS: JobDef[] = [
  { id: "merge", title: "PDF 합치기", desc: "여러 PDF를 하나로 · 순서 바꾸기·쪽 빼기", icon: "merge", tone: "red", run: "합치기", accepts: "합칠 PDF", exts: ["pdf"], min: 2, order: true },
  { id: "compress", title: "PDF 용량 줄이기", desc: "5MB 이하처럼 정한 크기로", icon: "compress", tone: "purple", run: "용량 줄이기", accepts: "줄일 PDF", exts: ["pdf"] },
  { id: "img2pdf", title: "사진을 PDF 하나로", desc: "여러 장을 차례대로 · 아이폰 사진도", icon: "images", tone: "orange", run: "PDF 만들기", accepts: "PDF로 만들 사진", exts: IMAGE_EXT, order: true },
  { id: "office2pdf", title: "한글·Word·엑셀·PPT → PDF", desc: "여러 개를 한꺼번에", icon: "pdf", tone: "blue", run: "PDF로 바꾸기", accepts: "PDF로 바꿀 문서", exts: OFFICE_EXT },
  { id: "photo", title: "사진 용량 줄이기·JPG로", desc: "HEIC·WEBP·PNG도 JPG로", icon: "image", tone: "green", run: "줄이기", accepts: "줄일 사진", exts: IMAGE_EXT },
  { id: "macname", title: "맥에서 온 이름 고치기", desc: "ㅎㅏㄴㄱㅡㄹ처럼 갈라진 이름, 깨진 zip", icon: "hangul", tone: "teal", run: "이름 고치기", accepts: "맥에서 받은 폴더·zip·파일", exts: null, folders: true },
];

export const jobDef = (id: string): JobDef | undefined => JOBS.find((j) => j.id === id);

export const extOfPath = (p: string) => { const m = /\.([^.\\/]+)$/.exec(p); return m ? m[1].toLowerCase() : ""; };
/** 맥에서 온 이름(자모가 갈라진 NFD)인지 */
export const isNfd = (p: string) => { const n = p.split(/[\\/]/).pop() ?? p; return n.normalize("NFC") !== n; };

export interface InItem { path: string; kind: string }

/** 이 할 일이 받을 파일만 */
export function acceptsItem(job: JobDef, it: InItem): boolean {
  if (job.id === "macname") return it.kind === "dir" || it.kind === "zip" || extOfPath(it.path) === "zip" || isNfd(it.path);
  if (it.kind === "dir" || it.kind === "missing") return false;
  return !!job.exts?.includes(extOfPath(it.path));
}

/** 넣은 파일에 맞는 할 일(맞는 순서대로). 예: PDF 여러 개 → 합치기, 용량 줄이기 */
export function suggest(items: InItem[]): JobId[] {
  const out: JobId[] = [];
  const n = (j: JobId) => items.filter((x) => acceptsItem(jobDef(j)!, x)).length;
  const pdf = n("merge");
  if (pdf >= 2) out.push("merge", "compress");
  else if (pdf === 1) out.push("compress", "merge");
  if (n("img2pdf")) out.push(n("img2pdf") > 1 ? "img2pdf" : "photo", n("img2pdf") > 1 ? "photo" : "img2pdf");
  if (n("office2pdf")) out.push("office2pdf");
  if (items.some((x) => x.kind === "dir" || x.kind === "zip" || isNfd(x.path))) out.push("macname");
  return [...new Set(out)];
}

/** 이름순(숫자는 숫자대로: 사진2 < 사진10) */
const coll = new Intl.Collator("ko", { numeric: true, sensitivity: "base" });
export function byName<T extends { path: string }>(xs: T[]): T[] {
  const name = (p: string) => p.split(/[\\/]/).pop() ?? p;
  return [...xs].sort((a, b) => coll.compare(name(a.path), name(b.path)));
}

export type FailKind = "locked" | "missing-tool" | "need-doctools" | "damaged" | "unsupported" | "busy" | "cancelled" | "space" | "other";
const REASON: Record<FailKind, string> = {
  locked: "암호가 걸려 있어요", damaged: "파일이 손상됐거나 읽을 수 없어요", unsupported: "이 일로는 바꿀 수 없는 파일이에요",
  "missing-tool": "변환 도구를 쓸 수 없어요", "need-doctools": "문서 도구가 필요해요(받지 않음)", busy: "다른 프로그램이 쓰고 있어요", cancelled: "멈췄어요", space: "디스크 공간이 모자라요", other: "끝내지 못했어요",
};

/** "10개 중 9개 완료, 1개는 암호가 걸려 있어요" */
export function summary(total: number, ok: number, fails: FailKind[], unit = "개"): string {
  if (!fails.length && ok === total) return total === 1 ? "다 됐어요" : `${total}${unit} 모두 완료`;
  const by = new Map<FailKind, number>();
  for (const f of fails) by.set(f, (by.get(f) ?? 0) + 1);
  const nun = unit === "장" ? "은" : "는"; // 받침 있는 단위(장) 뒤엔 "은"
  const parts = [...by].map(([k, c]) => `${c}${unit}${nun} ${REASON[k]}`);
  const skipped = total - ok - fails.length;
  if (skipped > 0) parts.push(`${skipped}${unit}${nun} 하지 않았어요(멈춤)`);
  return `${total}${unit} 중 ${ok}${unit} 완료${parts.length ? `, ${parts.join(", ")}` : ""}`;
}

/** 목표 크기(MB, 10진: 공공기관 "5MB 이하"에 맞게) → 바이트 */
export const mbToBytes = (mb: number) => Math.round(mb * 1_000_000);
