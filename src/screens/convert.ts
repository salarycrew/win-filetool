// 변환(첫 화면): 끌어다 놓기 → 넣은 파일 종류별 할 일 카드 → 고른 카드 아래 설정 → 진행 → 결과 한 줄
import {
  EditOut, Item, asErr, cancelJob, dirOf, fileName, friendly, human, invoke, isCancelled, needsPassword, newJob, offJobProgress,
  onJobProgress, pickFiles, stats, stem,
} from "../api";
import { FT, I, extOf } from "../icons";
import { go, Screen } from "../router";
import {
  $, $$, ResultLine, askPassword, bindResultButtons, bindSegs, checked, chk, dialog, errRow, errorPanel, esc, opt,
  progressPanel, resultRow, seg, segVal, setProgress, toast, value,
} from "../ui";
import { mountTrim, Trim, trimRange } from "../videotrim";
import { welcomeHTML, bindWelcome } from "../welcome";

// ---------- 넣은 파일 분류 ----------

type Cls = "pdf" | "word" | "office" | "image" | "video" | "audio" | "zip" | "dir" | "other";
const WORD = ["doc", "docx", "docm", "dotx", "hwp", "hwpx", "odt", "fodt", "pages", "rtf", "wpd", "txt"];
const DOCX_FAMILY = ["docx", "docm", "dotx"];
const OOXML_IMG = ["docx", "docm", "dotx", "pptx", "pptm", "potx", "xlsx", "xlsm", "xltx"];
const APPLE = ["pages", "numbers", "key"];
function clsOf(it: Item): Cls {
  const e = extOf(it.path);
  if (e === "hwpx" && it.kind !== "dir") return "word"; // 한글 HWPX(코어가 아직 문서로 모를 때도)
  switch (it.kind) {
    case "pdf": return "pdf";
    case "image": case "heic": return "image";
    case "video": return "video";
    case "audio": return "audio";
    case "zip": return "zip";
    case "dir": return "dir";
    case "document": return WORD.includes(e) ? "word" : "office";
    default: return "other";
  }
}
const CLS_LABEL: Record<Cls, (n: number) => string> = {
  pdf: (n) => (n > 1 ? `PDF ${n}개` : "PDF"), word: (n) => (n > 1 ? `문서 ${n}개` : "문서"), office: (n) => (n > 1 ? `PPT·엑셀 ${n}개` : "PPT·엑셀"),
  image: (n) => `사진·그림 ${n}장`, video: (n) => (n > 1 ? `영상 ${n}개` : "영상"), audio: (n) => (n > 1 ? `소리 ${n}개` : "소리"),
  zip: (n) => (n > 1 ? `zip ${n}개` : "zip"), dir: (n) => (n > 1 ? `폴더 ${n}개` : "폴더"), other: (n) => `그 밖의 파일 ${n}개`,
};
const CLS_ORDER: Cls[] = ["pdf", "word", "office", "image", "video", "audio", "zip", "dir", "other"];

// ---------- 할 일 ----------

interface Ctx {
  items: Item[];           // 이 할 일에 쓰는 파일
  panel: HTMLElement;      // 설정 판(값 읽기)
  outDir: string | null;
  trim?: Trim;
}
interface Run extends Ctx {
  job(): number;           // 새 작업 번호(진행 이벤트 연결)
  step(i: number, n: number, name: string): void;
  frac(f: number): void;   // 지금 파일 안의 진행
  ok(r: ResultLine): void;
  fail(input: string, e: unknown): void;
  stopped(): boolean;
}
interface Action {
  id: string; t: string; d: string; ic: string; tone: string; run: string;
  info?: string;
  when?: (items: Item[]) => boolean;     // 보이는 조건(묶음 안 파일)
  pickItems?: (items: Item[]) => Item[]; // 실제로 쓸 파일
  opts?: (c: { items: Item[] }) => string;
  setup?: (c: Ctx) => void;
  /** 누르면 바로 다른 화면으로(설정 판 없음) */
  jump?: (items: Item[]) => void;
  exec?: (r: Run) => Promise<void>;
  noOutDir?: boolean;
}

const pdfs = (xs: Item[]) => xs.filter((x) => x.kind === "pdf");
// W6 한글: hwp_capabilities {path} → 되는 카드만. 아직 모르면(명령 없음·확인 전) 막지 않는다
interface HwpCaps { format: string; can_pdf: boolean; can_pptx: boolean; can_images: boolean; route: string; message?: string | null }
type HItem = Item & { hwp?: HwpCaps | null };
const isHwp = (it: Item) => ["hwp", "hwpx"].includes(extOf(it.path));
const hwpOk = (it: Item, k: "can_pdf" | "can_pptx" | "can_images") => !isHwp(it) || !(it as HItem).hwp || (it as HItem).hwp![k];
const canImg = (it: Item) => DOCX_FAMILY.includes(extOf(it.path)) || (isHwp(it) && !!(it as HItem).hwp?.can_images);
const ext = (it: Item) => extOf(it.path);

/** 하나씩: 실패한 파일만 건너뛰고 계속 */
async function each(r: Run, f: (it: Item) => Promise<ResultLine | ResultLine[] | null>) {
  for (let i = 0; i < r.items.length; i++) {
    if (r.stopped()) break;
    const it = r.items[i];
    r.step(i, r.items.length, fileName(it.path));
    try {
      const out = await f(it);
      if (out) (Array.isArray(out) ? out : [out]).forEach((o) => r.ok({ input: it.path, before: it.size, ...o }));
    } catch (e) {
      if (isCancelled(e)) break;
      r.fail(it.path, e);
    }
  }
}

/** PDF 편집 명령(암호면 물어보고 다시) */
async function withPw<T>(it: Item, f: (pw: string | null) => Promise<T>): Promise<T> {
  let pw = (it as Item & { pw?: string }).pw ?? null;
  for (let tries = 0; ; tries++) {
    try {
      const out = await f(pw);
      if (pw) (it as Item & { pw?: string }).pw = pw;
      return out;
    } catch (e) {
      if (!needsPassword(e) || tries > 4) throw e;
      const got = await askPassword(fileName(it.path), asErr(e).code === "wrong_password" && pw != null);
      if (got == null) throw { code: "cancelled", message: "취소했습니다" };
      pw = got;
    }
  }
}

const NUM_POS: [string, string][] = [["bottom-center", "아래 가운데"], ["bottom-right", "아래 오른쪽"], ["top-right", "위 오른쪽"], ["top-center", "위 가운데"]];

const ACTIONS: Record<Cls, Action[]> = {
  pdf: [
    {
      id: "p2merge", t: "하나로 합치기", d: "넣은 순서대로 PDF 한 개로", ic: "merge", tone: "red", run: "합치기",
      when: (xs) => pdfs(xs).length >= 2, info: "위 목록 순서대로 합쳐요. 화살표로 순서를 바꿀 수 있어요.",
      exec: async (r) => {
        r.step(0, 1, `PDF ${r.items.length}개`);
        const o = await invoke<{ output: string; pages: number }>("pdf_merge", { paths: r.items.map((x) => x.path), outDir: r.outDir });
        r.ok({ output: o.output, pill: `${o.pages}쪽`, before: r.items.reduce((a, x) => a + (x.size ?? 0), 0) || null });
      },
    },
    {
      id: "p2word", t: "Word로", d: "글자를 고칠 수 있게", ic: "doc", tone: "blue", run: "Word로 바꾸기",
      info: "제목·문단·표·그림을 살려요. 줄 바꿈·표 모양이 원본과 조금 다를 수 있어요.",
      exec: (r) => each(r, async (it) => {
        const o = await invoke<{ output: string; pages: number; scanned_pages: number }>("pdf_to_docx", { job: r.job(), path: it.path, outDir: r.outDir });
        return { output: o.output, pill: `${o.pages}쪽`, note: o.scanned_pages ? `스캔한 쪽 ${o.scanned_pages}개는 그림으로 넣었어요` : undefined };
      }),
    },
    {
      id: "p2ppt", t: "PPT로", d: "쪽마다 슬라이드 한 장", ic: "slides", tone: "orange", run: "PPT로 바꾸기",
      opts: () => opt("방식", seg("pptmode", [["image", "그대로 보이게"], ["editable", "글자 고칠 수 있게"]], "image"), "그대로 보이게: 쪽을 그림으로 · 글자 고칠 수 있게: 글상자로(모양이 조금 달라질 수 있어요)"),
      exec: (r) => each(r, async (it) => {
        const o = await invoke<{ output: string; slides: number }>("pdf_to_pptx", { job: r.job(), path: it.path, mode: segVal(r.panel, "pptmode"), outDir: r.outDir });
        return { output: o.output, pill: `슬라이드 ${o.slides}장` };
      }),
    },
    {
      id: "p2txt", t: "글자만(TXT)", d: "메모장으로 여는 글자 파일", ic: "text", tone: "gray", run: "글자만 저장",
      info: "쪽 사이에 “— 2쪽 —” 같은 표시를 넣어요. 스캔한 쪽은 글자가 없어요.",
      exec: (r) => each(r, async (it) => {
        const o = await invoke<{ output: string; chars: number }>("pdf_to_txt", { job: r.job(), path: it.path, outDir: r.outDir });
        return { output: o.output, pill: `${o.chars.toLocaleString("ko-KR")}자` };
      }),
    },
    {
      id: "p2img", t: "이미지로", d: "쪽마다 그림 파일로", ic: "pages", tone: "teal", run: "쪽 이미지로 저장",
      opts: () => opt("형식", seg("fmt", [["png", "PNG"], ["jpg", "JPG"]], "png"), "PNG는 선명하고, JPG는 작아요") +
        opt("선명도", seg("dpi", [["72", "화면용"], ["150", "보통"], ["300", "인쇄용"]], "150")) +
        opt("쪽", `<input class="txt" id="pages" placeholder="비우면 전체 · 예: 1-3, 5">`),
      exec: (r) => each(r, async (it) => {
        const o = await invoke<{ folder: string; pages: { output: string }[] }>("pdf_to_images", {
          job: r.job(), path: it.path, outDir: r.outDir,
          opts: { format: segVal(r.panel, "fmt"), dpi: Number(segVal(r.panel, "dpi")), quality: 90, pages: value(r.panel, "pages") || null },
        });
        return o.pages.length === 1 ? { output: o.pages[0].output } : { output: o.folder, isFolder: true, pill: `그림 ${o.pages.length}장` };
      }),
    },
    {
      id: "p2extract", t: "이미지만 뽑기", d: "PDF에 든 사진만 꺼내기", ic: "images", tone: "green", run: "이미지 뽑기",
      info: "들어 있는 사진을 다시 압축하지 않고 새 폴더에 모아요.",
      exec: (r) => each(r, async (it) => {
        const o = await invoke<{ folder: string | null; files: string[]; note: string }>("extract_images", { job: r.job(), path: it.path, outDir: r.outDir });
        if (!o.folder) { r.ok({ output: it.path, note: "이 PDF에는 꺼낼 사진이 없어요", pill: "0장" }); return null; }
        return { output: o.folder, isFolder: true, pill: `이미지 ${o.files.length}장` };
      }),
    },
    {
      id: "p2small", t: "용량 줄이기", d: "메일·메신저로 보내기 좋게", ic: "compress", tone: "purple", run: "용량 줄이기",
      info: "글자·선·표는 그대로 두고 사진만 줄여요. 줄지 않으면 새 파일을 만들지 않아요.",
      opts: () => opt("정도", seg("preset", [["screen", "화면용"], ["normal", "보통"], ["print", "인쇄용"]], "normal"), "보통: 150 DPI · 메일·메신저용"),
      setup: (c) => {
        const help = $(".opt-help", c.panel)!;
        const H: Record<string, string> = { screen: "96 DPI · 가장 작게", normal: "150 DPI · 메일·메신저용", print: "300 DPI · 인쇄해도 깨끗하게" };
        c.panel.addEventListener("click", () => { help.textContent = H[segVal(c.panel, "preset")] ?? ""; });
      },
      exec: (r) => each(r, async (it) => {
        const o = await invoke<{ output: string | null; before: number; after: number; note: string }>("pdf_compress", { path: it.path, preset: segVal(r.panel, "preset"), outDir: r.outDir });
        if (!o.output) { r.ok({ output: it.path, note: "더 줄일 수 없어서 새 파일을 만들지 않았어요", pill: "그대로" }); return null; }
        return { output: o.output, before: o.before, after: o.after };
      }),
    },
    {
      id: "p2edit", t: "PDF 편집 열기", d: "쪽 순서·돌리기·지우기·끼워 넣기", ic: "layers", tone: "gray", run: "",
      jump: (xs) => go("pdfedit", { open: pdfs(xs).map((x) => x.path) }),
    },
    {
      id: "p2lock", t: "암호 걸기·풀기", d: "열 때 암호를 묻게", ic: "lock", tone: "blue", run: "적용하기",
      opts: (c) => {
        const anyLocked = c.items.some((x) => x.locked);
        return opt("할 일", seg("lockmode", [["on", "암호 걸기"], ["off", "암호 풀기"]], anyLocked ? "off" : "on")) +
          `<div data-mode="on" ${anyLocked ? "hidden" : ""}>` +
          opt("새 암호", `<input type="password" class="txt" id="pw1" autocomplete="new-password">`) +
          opt("한 번 더", `<input type="password" class="txt" id="pw2" autocomplete="new-password">`, "잊으면 아무도 열 수 없어요. 꼭 따로 적어 두세요.") +
          opt("막을 것", `${chk("no-print", "인쇄", false)}${chk("no-copy", "글자 복사", false)}${chk("no-edit", "고치기", false)}`) +
          `</div><div data-mode="off" ${anyLocked ? "" : "hidden"}>` +
          opt("지금 암호", `<input type="password" class="txt" id="pw-old" autocomplete="off">`, "알고 있는 암호로만 풀 수 있어요") + `</div>`;
      },
      setup: (c) => c.panel.addEventListener("click", () => {
        const m = segVal(c.panel, "lockmode");
        $$<HTMLElement>("[data-mode]", c.panel).forEach((d) => (d.hidden = d.dataset.mode !== m));
      }),
      exec: async (r) => {
        const mode = segVal(r.panel, "lockmode");
        if (mode === "on") {
          const p1 = ($<HTMLInputElement>("#pw1", r.panel)!).value, p2 = ($<HTMLInputElement>("#pw2", r.panel)!).value;
          const noP = checked(r.panel, "no-print"), noC = checked(r.panel, "no-copy"), noE = checked(r.panel, "no-edit");
          if (!p1 && !noP && !noC && !noE) throw new Error("암호를 넣거나 막을 것을 하나 이상 골라 주세요");
          if (p1 !== p2) throw new Error("두 칸의 암호가 달라요. 다시 넣어 주세요");
          await each(r, async (it) => {
            const o = await withPw(it, (pw) => invoke<EditOut>("pdf_protect", {
              path: it.path, password: pw, outDir: r.outDir,
              opts: { open_password: p1, owner_password: "", allow_print: !noP, allow_copy: !noC, allow_edit: !noE, cipher: "aes256" },
            }));
            return { output: o.output, pill: p1 ? "암호 걸림" : "제한 걸림" };
          });
        } else {
          const pw = ($<HTMLInputElement>("#pw-old", r.panel)!).value;
          if (!pw) throw new Error("지금 걸린 암호를 넣어 주세요");
          await each(r, async (it) => {
            const o = await invoke<EditOut>("pdf_unprotect", { path: it.path, password: pw, outDir: r.outDir });
            return { output: o.output, pill: "암호 풀림" };
          });
        }
      },
    },
    {
      id: "p2num", t: "쪽 번호", d: "쪽마다 번호 넣기", ic: "hash", tone: "teal", run: "쪽 번호 넣기",
      info: "쪽 위에서 미리 보며 넣으려면 [PDF 편집] > 쪽 번호를 쓰세요.",
      opts: () => opt("위치", seg("pos", NUM_POS, "bottom-center")) +
        opt("모양", seg("nfmt", [["plain", "1"], ["of-total", "1 / 10"], ["dash", "- 1 -"]], "plain")) +
        opt("시작 번호", `<input class="txt sm" id="nstart" value="1" inputmode="numeric">`) +
        opt("", chk("skip1", "첫 쪽(표지)은 비우기", false)),
      exec: (r) => {
        const start = Math.max(0, Math.round(Number(value(r.panel, "nstart")) || 1));
        return each(r, async (it) => {
          const o = await withPw(it, (pw) => invoke<EditOut>("pdf_page_numbers", {
            job: r.job(), path: it.path, password: pw, outDir: r.outDir,
            opts: { position: segVal(r.panel, "pos"), start, format: segVal(r.panel, "nfmt"), skip_first: checked(r.panel, "skip1") },
          }));
          return { output: o.output, pill: `${o.pages}쪽` };
        });
      },
    },
    {
      id: "p2mark", t: "워터마크", d: "“대외비” 같은 글자를 흐리게", ic: "drop", tone: "purple", run: "워터마크 넣기",
      info: "쪽 위에서 미리 보며 넣으려면 [PDF 편집] > 워터마크를 쓰세요.",
      opts: () => opt("글자", `<input class="txt" id="wtext" value="대외비" maxlength="60">`) +
        opt("진하기", seg("wop", [["0.15", "흐리게"], ["0.3", "보통"], ["0.5", "진하게"]], "0.3")) +
        opt("놓는 법", seg("wtile", [["center", "가운데 크게"], ["tile", "바둑판으로"]], "center")) +
        opt("기울기", seg("wang", [["45", "비스듬히"], ["0", "가로로"]], "45")),
      exec: (r) => {
        const text = value(r.panel, "wtext");
        if (!text) throw new Error("넣을 글자를 적어 주세요");
        return each(r, async (it) => {
          const o = await withPw(it, (pw) => invoke<EditOut>("pdf_watermark", {
            job: r.job(), path: it.path, password: pw, outDir: r.outDir,
            opts: { kind: "text", text, font_size: segVal(r.panel, "wtile") === "tile" ? 28 : 54, color: "#808080", opacity: Number(segVal(r.panel, "wop")), angle: Number(segVal(r.panel, "wang")), tiling: segVal(r.panel, "wtile"), layer: "over" },
          }));
          return { output: o.output, pill: `${o.pages}쪽` };
        });
      },
    },
    {
      id: "p2sign", t: "서명·도장", d: "쪽 위에 서명 그림 놓기", ic: "sign", tone: "red", run: "",
      jump: (xs) => go("pdfedit", { open: pdfs(xs).slice(0, 1).map((x) => x.path), tool: "sign" }),
    },
    {
      id: "p2split", t: "나누기", d: "한 쪽씩 또는 범위로 따로", ic: "cut", tone: "gray", run: "나누기",
      opts: () => opt("나누는 법", seg("split", [["every", "한 쪽씩 모두"], ["ranges", "범위로"]], "every")) +
        opt("범위", `<input class="txt" id="ranges" placeholder="예: 1-3, 5  → 1~3쪽 파일, 5쪽 파일">`),
      exec: (r) => {
        const every = segVal(r.panel, "split") === "every";
        const ranges = value(r.panel, "ranges");
        if (!every && !ranges) throw new Error("나눌 범위를 적어 주세요 (예: 1-3, 5)");
        return each(r, async (it) => {
          const outs = await invoke<string[]>("pdf_split", { path: it.path, mode: every ? { mode: "every_page" } : { mode: "ranges", spec: ranges }, outDir: r.outDir });
          return outs.length === 1 ? { output: outs[0] } : { output: dirOf(outs[0] ?? it.path), isFolder: true, pill: `PDF ${outs.length}개` };
        });
      },
    },
  ],
  word: [
    {
      id: "d2pdf", t: "PDF로", d: "인쇄·공유용 PDF로 저장", ic: "pdf", tone: "red", run: "PDF로 바꾸기",
      when: (xs) => xs.some((x) => hwpOk(x, "can_pdf")), pickItems: (xs) => xs.filter((x) => hwpOk(x, "can_pdf")),
      info: "글꼴과 모양을 그대로 담아요.",
      exec: (r) => each(r, async (it) => {
        const o = await invoke<{ output: string; note: string }>("convert_one", { job: r.job(), path: it.path, op: { kind: "doc", target: "pdf" }, outDir: r.outDir });
        return { output: o.output };
      }),
    },
    {
      id: "d2ppt", t: "PPT로", d: "제목마다 슬라이드 한 장", ic: "slides", tone: "orange", run: "PPT로 바꾸기",
      when: (xs) => xs.some((x) => hwpOk(x, "can_pptx")), pickItems: (xs) => xs.filter((x) => hwpOk(x, "can_pptx")),
      info: "문서의 제목마다 슬라이드를 한 장씩 만들고, 표·그림도 옮겨요.",
      exec: (r) => each(r, async (it) => {
        const o = await invoke<{ output: string; slides: number }>("doc_to_pptx", { job: r.job(), path: it.path, outDir: r.outDir });
        return { output: o.output, pill: `슬라이드 ${o.slides}장` };
      }),
    },
    {
      id: "d2img", t: "이미지만 뽑기", d: "문서에 든 사진·그림 꺼내기", ic: "images", tone: "green", run: "이미지 뽑기",
      when: (xs) => xs.some(canImg), pickItems: (xs) => xs.filter(canImg),
      info: "원래 형식 그대로, 원본 옆 새 폴더에 모아요.",
      exec: (r) => each(r, async (it) => {
        const o = await invoke<{ folder: string | null; files: string[] }>("extract_images", { job: r.job(), path: it.path, outDir: r.outDir });
        if (!o.folder) { r.ok({ output: it.path, note: "이 문서에는 꺼낼 그림이 없어요", pill: "0장" }); return null; }
        return { output: o.folder, isFolder: true, pill: `이미지 ${o.files.length}장` };
      }),
    },
    {
      id: "d2office", t: "Word 형식으로", d: "Pages 문서를 Word로", ic: "doc", tone: "blue", run: "Word로 바꾸기",
      when: (xs) => xs.some((x) => APPLE.includes(ext(x))), pickItems: (xs) => xs.filter((x) => APPLE.includes(ext(x))),
      info: "복잡한 서식은 조금 달라질 수 있어요.",
      exec: (r) => each(r, async (it) => (await invoke<{ output: string }>("convert_one", { job: r.job(), path: it.path, op: { kind: "doc", target: "docx" }, outDir: r.outDir }))),
    },
  ],
  office: [
    {
      id: "o2pdf", t: "PDF로", d: "인쇄·공유용 PDF로 저장", ic: "pdf", tone: "red", run: "PDF로 바꾸기",
      info: "슬라이드·시트 모양을 그대로 담아요.",
      exec: (r) => each(r, async (it) => (await invoke<{ output: string }>("convert_one", { job: r.job(), path: it.path, op: { kind: "doc", target: "pdf" }, outDir: r.outDir }))),
    },
    {
      id: "o2img", t: "이미지만 뽑기", d: "들어 있는 사진·그림 꺼내기", ic: "images", tone: "green", run: "이미지 뽑기",
      when: (xs) => xs.some((x) => OOXML_IMG.includes(ext(x))), pickItems: (xs) => xs.filter((x) => OOXML_IMG.includes(ext(x))),
      info: "원래 형식 그대로 새 폴더에 모아요. (.pptx · .xlsx 파일)",
      exec: (r) => each(r, async (it) => {
        const o = await invoke<{ folder: string | null; files: string[] }>("extract_images", { job: r.job(), path: it.path, outDir: r.outDir });
        if (!o.folder) { r.ok({ output: it.path, note: "꺼낼 그림이 없어요", pill: "0장" }); return null; }
        return { output: o.folder, isFolder: true, pill: `이미지 ${o.files.length}장` };
      }),
    },
    {
      id: "o2office", t: "Office 형식으로", d: "Keynote→PPT, Numbers→엑셀", ic: "slides", tone: "orange", run: "바꾸기",
      when: (xs) => xs.some((x) => APPLE.includes(ext(x))), pickItems: (xs) => xs.filter((x) => APPLE.includes(ext(x))),
      exec: (r) => each(r, async (it) => (await invoke<{ output: string }>("convert_one", {
        job: r.job(), path: it.path, op: { kind: "doc", target: ext(it) === "key" ? "pptx" : "xlsx" }, outDir: r.outDir,
      }))),
    },
  ],
  image: [
    {
      id: "i2fmt", t: "JPG 등 형식", d: "아이폰 사진(HEIC)도 바로", ic: "image", tone: "green", run: "바꾸기",
      opts: () => opt("형식", seg("fmt", [["jpg", "JPG"], ["png", "PNG"], ["webp", "WebP"], ["bmp", "BMP"], ["tiff", "TIFF"]], "jpg")) +
        opt("품질", seg("q", [["80", "보통"], ["90", "좋게"], ["95", "최고"]], "90"), "JPG·WebP에만 써요") +
        opt("", chk("exif", "촬영 날짜 등 사진 정보 남기기", true)),
      exec: (r) => each(r, async (it) => {
        const o = await invoke<{ output: string }>("convert_one", {
          job: r.job(), path: it.path, outDir: r.outDir,
          op: { kind: "image", format: segVal(r.panel, "fmt"), quality: Number(segVal(r.panel, "q")), max_side: null, keep_exif: checked(r.panel, "exif") },
        });
        return { output: o.output };
      }),
    },
    {
      id: "i2resize", t: "크기 바꾸기", d: "가로 픽셀이나 %로", ic: "resize", tone: "purple", run: "크기 바꾸기",
      opts: () => opt("기준", seg("rmode", [["px", "가로 크기"], ["pct", "비율"]], "px")) +
        `<div data-r="px">${opt("가로", seg("rw", [["800", "800"], ["1280", "1280"], ["1920", "1920"], ["c", "직접"]], "1280") + `<input class="txt sm" id="rwc" placeholder="px" inputmode="numeric" hidden>`, "세로는 비율에 맞춰요")}</div>` +
        `<div data-r="pct" hidden>${opt("비율", seg("rp", [["25", "25%"], ["50", "50%"], ["75", "75%"]], "50"))}</div>`,
      setup: (c) => c.panel.addEventListener("click", () => {
        const m = segVal(c.panel, "rmode");
        $$<HTMLElement>("[data-r]", c.panel).forEach((d) => (d.hidden = d.dataset.r !== m));
        $<HTMLInputElement>("#rwc", c.panel)!.hidden = segVal(c.panel, "rw") !== "c";
      }),
      exec: (r) => {
        let resize: Record<string, unknown>;
        if (segVal(r.panel, "rmode") === "pct") resize = { percent: Number(segVal(r.panel, "rp")), keep_ratio: true };
        else {
          const w = segVal(r.panel, "rw") === "c" ? Math.round(Number(value(r.panel, "rwc"))) : Number(segVal(r.panel, "rw"));
          if (!w || w < 8 || w > 20000) throw new Error("가로 크기를 8~20000 사이 숫자로 적어 주세요");
          resize = { width: w, keep_ratio: true };
        }
        return imageEdit(r, { resize, strip_meta: false, format: "keep" });
      },
    },
    {
      id: "i2fit", t: "용량 맞추기", d: "정한 용량 아래로", ic: "compress", tone: "purple", run: "용량 맞추기",
      info: "화질을 조금씩 낮춰 보고, 그래도 크면 크기를 줄여요.",
      opts: () => opt("목표", seg("kb", [["200", "200KB"], ["500", "500KB"], ["1024", "1MB"], ["2048", "2MB"], ["c", "직접"]], "500") + `<input class="txt sm" id="kbc" placeholder="KB" inputmode="numeric" hidden>`) +
        opt("형식", seg("kfmt", [["jpg", "JPG로"], ["keep", "원래 형식"]], "jpg")),
      setup: (c) => c.panel.addEventListener("click", () => { $<HTMLInputElement>("#kbc", c.panel)!.hidden = segVal(c.panel, "kb") !== "c"; }),
      exec: (r) => {
        const kb = segVal(r.panel, "kb") === "c" ? Math.round(Number(value(r.panel, "kbc"))) : Number(segVal(r.panel, "kb"));
        if (!kb || kb < 10) throw new Error("목표 용량을 10KB 넘는 숫자로 적어 주세요");
        return imageEdit(r, { target_kb: kb, format: segVal(r.panel, "kfmt"), strip_meta: false });
      },
    },
    {
      id: "i2crop", t: "자르기·돌리기", d: "비율 맞추기·돌리기·뒤집기", ic: "crop", tone: "teal", run: "적용하기",
      opts: () => opt("자르기", seg("aspect", [["", "안 함"], ["1:1", "1:1"], ["4:3", "4:3"], ["16:9", "16:9"], ["3:4", "3:4"], ["9:16", "9:16"]], ""), "가운데를 남겨요") +
        opt("돌리기", seg("rot", [["", "안 함"], ["90", "오른쪽 90°"], ["180", "180°"], ["270", "왼쪽 90°"]], "")) +
        opt("뒤집기", seg("flip", [["", "안 함"], ["h", "좌우"], ["v", "위아래"]], "")),
      exec: (r) => {
        const a = segVal(r.panel, "aspect"), rot = segVal(r.panel, "rot"), fl = segVal(r.panel, "flip");
        if (!a && !rot && !fl) throw new Error("자르기·돌리기·뒤집기 중 하나 이상 골라 주세요");
        return imageEdit(r, {
          ...(a ? { crop: { aspect: a, anchor: "center" } } : {}), ...(rot ? { rotate: Number(rot) } : {}), ...(fl ? { flip: fl } : {}),
          strip_meta: false, format: "keep",
        });
      },
    },
    {
      id: "i2strip", t: "위치·촬영 정보 지우기", d: "남에게 보내기 전에", ic: "shield", tone: "blue", run: "정보 지우기",
      info: "사진에 든 위치(GPS)·촬영 기기·날짜 정보를 지워요. 사진 방향은 그대로예요.",
      exec: (r) => imageEdit(r, { strip_meta: true, format: "keep" }, (it) => it.kind === "heic" ? "HEIC은 JPG로 바꾸면서 지워요" : undefined),
    },
    {
      id: "i2pdf", t: "PDF 하나로", d: "넣은 순서대로 한 파일에", ic: "pdf", tone: "red", run: "PDF로 묶기",
      info: "위 목록 순서대로 한 쪽씩 넣어요. 사진 방향은 바르게 돌려 넣어요.",
      opts: () => opt("쪽 크기", seg("page", [["image", "사진 크기 그대로"], ["a4_portrait", "A4 세로"], ["a4_landscape", "A4 가로"]], "image")) +
        opt("여백", seg("margin", [["0", "없음"], ["10", "좁게"], ["20", "넓게"]], "0")),
      exec: async (r) => {
        r.step(0, 1, `사진 ${r.items.length}장`);
        const o = await invoke<{ output: string }>("images_to_pdf", {
          job: r.job(), paths: r.items.map((x) => x.path), outDir: r.outDir,
          opts: { page: segVal(r.panel, "page"), margin_mm: Number(segVal(r.panel, "margin")) },
        });
        r.ok({ output: o.output, pill: `${r.items.length}쪽`, before: r.items.reduce((a, x) => a + (x.size ?? 0), 0) || null });
      },
    },
  ],
  video: [
    { id: "v2mp4", t: "MP4로", d: "어디서나 열리는 영상으로", ic: "video", tone: "purple", run: "MP4로 바꾸기", info: "화질은 거의 그대로 두고 널리 쓰는 형식으로 바꿔요.", exec: (r) => media(r, { preset: "mp4" }) },
    {
      id: "v2msg", t: "메신저용", d: "메신저로 보내기 좋게 줄이기", ic: "chat", tone: "blue", run: "줄이기",
      opts: () => opt("최대 크기", seg("mb", [["25", "25MB"], ["50", "50MB"], ["100", "100MB"]], "25"), "카카오톡은 보통 25MB") + opt("해상도", seg("h", [["480", "480p"], ["720", "720p"], ["1080", "1080p"]], "720")),
      exec: (r) => media(r, { preset: "messenger", max_mb: Number(segVal(r.panel, "mb")), max_height: Number(segVal(r.panel, "h")) }),
    },
    {
      id: "v2trim", t: "앞뒤 자르기", d: "필요한 부분만 남기기", ic: "cut", tone: "orange", run: "자르기",
      when: (xs) => xs.length === 1, opts: () => `<div class="trim-host"></div>` + opt("", chk("mute", "소리도 빼기", false)),
      setup: (c) => { c.trim = mountTrim($(".trim-host", c.panel)!, c.items[0].path); },
      exec: async (r) => {
        const rg = r.trim ? trimRange(r.trim) : null;
        if (!rg) throw new Error("시작·끝 시간을 확인해 주세요");
        const it = r.items[0];
        r.step(0, 1, fileName(it.path));
        const o = await invoke<{ output: string; before: number; after: number; note?: string }>("media_edit", {
          job: r.job(), input: it.path, outDir: r.outDir, ops: { trim: rg, ...(checked(r.panel, "mute") ? { mute: true } : {}) },
        });
        r.ok({ input: it.path, output: o.output, before: o.before ?? it.size, after: o.after, note: o.note });
      },
    },
    { id: "v2mute", t: "소리 빼기", d: "화면만 남기기", ic: "mute", tone: "pink", run: "소리 빼기", exec: (r) => mediaEdit(r, { mute: true }) },
    {
      id: "v2rot", t: "돌리기", d: "옆으로 찍힌 영상 바로", ic: "rotr", tone: "teal", run: "돌리기",
      opts: () => opt("방향", seg("vrot", [["90", "오른쪽 90°"], ["180", "180°"], ["270", "왼쪽 90°"]], "90")),
      exec: (r) => mediaEdit(r, { rotate: Number(segVal(r.panel, "vrot")) }),
    },
    {
      id: "v2scene", t: "장면 저장", d: "한 장면을 JPG로", ic: "scene", tone: "green", run: "장면 저장",
      when: (xs) => xs.length === 1, opts: () => `<div class="trim-host"></div>`,
      setup: (c) => { c.trim = mountTrim($(".trim-host", c.panel)!, c.items[0].path, true); },
      exec: async (r) => {
        const it = r.items[0];
        r.step(0, 1, fileName(it.path));
        const out = await invoke<string | { output: string }>("media_frame_save", { path: it.path, at: r.trim!.at(), outDir: r.outDir });
        r.ok({ output: typeof out === "string" ? out : out.output });
      },
    },
    {
      id: "v2gif", t: "GIF로", d: "몇 초만 움직이는 그림으로", ic: "loop", tone: "orange", run: "GIF 만들기",
      opts: () => opt("시작", `<input class="txt sm" id="gs" value="0:00" inputmode="decimal">`, "분:초") + opt("길이", seg("gd", [["3", "3초"], ["5", "5초"], ["10", "10초"]], "5")) +
        opt("가로", seg("gw", [["320", "작게"], ["480", "보통"], ["640", "크게"]], "480")),
      exec: (r) => {
        const s = value(r.panel, "gs").split(":").map(Number).reduce((a, x) => a * 60 + x, 0);
        if (!Number.isFinite(s) || s < 0) throw new Error("시작 시간을 “분:초”로 적어 주세요 (예: 0:12)");
        return media(r, { preset: "gif", start: s, duration: Number(segVal(r.panel, "gd")), width: Number(segVal(r.panel, "gw")), fps: 12 });
      },
    },
    {
      id: "v2audio", t: "MP3·M4A로", d: "소리만 저장", ic: "music", tone: "pink", run: "소리만 저장",
      opts: () => opt("형식", seg("afmt", [["mp3", "MP3"], ["m4a", "M4A"]], "mp3")),
      exec: (r) => media(r, { preset: segVal(r.panel, "afmt") }),
    },
  ],
  audio: [
    {
      id: "a2fmt", t: "MP3·M4A로", d: "어디서나 열리는 소리 파일로", ic: "music", tone: "pink", run: "바꾸기",
      opts: () => opt("형식", seg("afmt", [["mp3", "MP3"], ["m4a", "M4A"]], "mp3")),
      exec: (r) => media(r, { preset: segVal(r.panel, "afmt") }),
    },
  ],
  zip: [
    {
      id: "z2open", t: "풀기", d: "한글 이름 깨짐 자동 처리", ic: "unzip", tone: "orange", run: "풀기",
      info: "맥·윈도에서 만든 zip 모두 돼요. 같은 이름 파일은 덮어쓰지 않고 건너뛰어요.",
      opts: () => opt("이름 글자", seg("enc", [["auto", "자동"], ["utf8", "UTF-8"], ["cp949", "한국어 윈도"]], "auto"), "이름이 깨져 보이면 다른 것을 골라 보세요") + `<div class="zip-peek hint"></div>`,
      setup: (c) => {
        const peek = $(".zip-peek", c.panel)!;
        const load = async () => {
          const it = c.items[0];
          try {
            const o = await invoke<{ entries: { name: string; is_dir: boolean }[]; detected_encoding: string; garbled_guess: boolean }>("zip_list", { path: it.path, encoding: segVal(c.panel, "enc") });
            const names = o.entries.filter((x) => !x.is_dir).slice(0, 6).map((x) => `<li>${esc(x.name)}</li>`).join("");
            peek.innerHTML = `<div>${c.items.length > 1 ? `${esc(fileName(it.path))} 미리보기 · ` : ""}파일 ${o.entries.filter((x) => !x.is_dir).length}개${o.garbled_guess ? ' · <b class="warn-t">이름이 깨져 보이면 위에서 다른 글자를 골라 보세요</b>' : ""}</div><ul class="peek">${names}</ul>`;
          } catch { peek.textContent = ""; }
        };
        load();
        c.panel.addEventListener("click", (e) => { if ((e.target as HTMLElement).closest('[data-seg="enc"]')) load(); });
      },
      exec: (r) => each(r, async (it) => {
        const o = await invoke<Record<string, unknown>>("zip_extract", { path: it.path, zip: it.path, encoding: segVal(r.panel, "enc"), outDir: r.outDir, rezip: false });
        if (typeof o.folder === "string") {
          const skipped = (o.skipped as unknown[] | undefined)?.length ?? 0;
          return { output: o.folder, isFolder: true, pill: `파일 ${o.count}개`, note: skipped ? `${skipped}개는 건너뛰었어요(이미 있거나 안전하지 않은 이름)` : undefined };
        }
        const ex = (o.extract ?? o) as { out_dir: string; files: string[] };
        return { output: ex.out_dir, isFolder: true, pill: `파일 ${ex.files.length}개` };
      }),
    },
  ],
  dir: [
    {
      id: "f2nfc", t: "한글 이름 고치기", d: "맥에서 온 깨진 이름(ㅎㅏㄴ)", ic: "hangul", tone: "blue", run: "미리 보고 고치기",
      info: "폴더 안 이름을 먼저 살펴보고, 고칠 이름을 보여 드린 뒤 바꿔요.", noOutDir: true,
      exec: (r) => each(r, async (it) => {
        const plan = await invoke<{ renames: { from: string; to: string }[]; collisions: unknown[] }>("nfc_preview", { folder: it.path });
        if (!plan.renames.length) return { output: it.path, isFolder: true, pill: "고칠 이름 없음", note: "이미 모두 바른 이름이에요" };
        const list = plan.renames.slice(0, 8).map((x) => `<li>${esc(fileName(x.from))} → ${esc(fileName(x.to))}</li>`).join("");
        const ok = await confirmDlgHtml(`이름 ${plan.renames.length}개를 고칠까요?`, `<ul class="peek">${list}</ul>${plan.renames.length > 8 ? `<p class="hint">… 외 ${plan.renames.length - 8}개</p>` : ""}${plan.collisions.length ? `<p class="hint">같은 이름이 이미 있는 ${plan.collisions.length}개는 건너뛰어요.</p>` : ""}`, "고치기");
        if (!ok) return { output: it.path, isFolder: true, pill: "바꾸지 않음" };
        const rep = await invoke<{ renamed: unknown[]; failed: unknown[] }>("nfc_apply", { folder: it.path });
        return { output: it.path, isFolder: true, pill: `이름 ${rep.renamed.length}개 고침`, note: rep.failed.length ? `${rep.failed.length}개는 바꾸지 못했어요` : undefined };
      }),
    },
    { id: "f2rename", t: "이름 한꺼번에 바꾸기", d: "번호·날짜 붙이기, 찾아 바꾸기", ic: "rename", tone: "purple", run: "", jump: (xs) => go("organize", { tab: "rename", folders: xs.map((x) => x.path) }) },
    { id: "f2dup", t: "같은 파일 찾기", d: "내용이 똑같은 파일 정리", ic: "copy", tone: "teal", run: "", jump: (xs) => go("organize", { tab: "dup", roots: xs.map((x) => x.path) }) },
    {
      id: "f2zip", t: "zip으로 묶기", d: "맥·윈도 어디서나 풀리게", ic: "zip", tone: "orange", run: "zip 만들기",
      info: "한글 이름이 맥·윈도 모두에서 바르게 보이게 만들어요.",
      exec: async (r) => {
        r.step(0, 1, r.items.length > 1 ? `폴더 ${r.items.length}개` : fileName(r.items[0].path));
        const out = r.outDir ? `${r.outDir}\\${fileName(r.items[0].path)}.zip` : undefined;
        const o = await invoke<{ output: string; count: number; size: number }>("zip_create", { job: r.job(), inputs: r.items.map((x) => x.path), output: out });
        r.ok({ output: o.output, after: o.size, pill: `파일 ${o.count}개` });
      },
    },
  ],
  other: [],
};

async function confirmDlgHtml(title: string, html: string, ok: string) {
  return !!(await dialog({ title, html, ok, icon: "info" }));
}

async function media(r: Run, preset: Record<string, unknown>) {
  await each(r, async (it) => {
    const job = r.job();
    const o = await invoke<{ output: string; note: string }>("convert_one", { job, path: it.path, op: { kind: "media", ...preset }, outDir: r.outDir });
    return { output: o.output, note: /크게|목표/.test(o.note ?? "") ? o.note : undefined };
  });
}
async function mediaEdit(r: Run, ops: Record<string, unknown>) {
  await each(r, async (it) => {
    const o = await invoke<{ output: string; before?: number; after?: number; note?: string }>("media_edit", { job: r.job(), input: it.path, ops, outDir: r.outDir });
    return { output: o.output, before: o.before ?? it.size, after: o.after, note: o.note };
  });
}
/** W4 image_edit: 여러 장을 한 번에(진행은 tools_progress) */
async function imageEdit(r: Run, ops: Record<string, unknown>, noteFor?: (it: Item) => string | undefined) {
  r.step(0, r.items.length, r.items.length > 1 ? `사진 ${r.items.length}장` : fileName(r.items[0].path));
  const out = await invoke<{ input: string; output: string; before: number; after: number; note?: string; error?: string }[]>("image_edit", {
    job: r.job(), inputs: r.items.map((x) => x.path), ops, outDir: r.outDir,
  });
  for (const o of out) {
    if (o.error || !o.output) r.fail(o.input, o.error ?? "결과가 없어요");
    else r.ok({ input: o.input, output: o.output, before: o.before, after: o.after, note: o.note ?? noteFor?.(r.items.find((x) => x.path === o.input)!) });
  }
}

// ---------- 화면 상태 ----------

let root: HTMLElement;
let items: Item[] = [];
let pick: { cls: Cls; id: string } | null = null;
let outDir: string | null = null;
let phase: "pick" | "run" | "done" = "pick";
let running = false;
let stopFlag = false;
let runJob = 0;
let lastResults: ResultLine[] = [];
let ctx: Ctx | null = null;

function groups(): { cls: Cls; items: Item[]; actions: Action[] }[] {
  const by = new Map<Cls, Item[]>();
  for (const it of items) {
    if (it.kind === "missing") continue;
    const c = clsOf(it);
    by.set(c, [...(by.get(c) ?? []), it]);
  }
  return CLS_ORDER.filter((c) => by.has(c)).map((c) => {
    const xs = by.get(c)!;
    return { cls: c, items: xs, actions: ACTIONS[c].filter((a) => !a.when || a.when(xs)) };
  });
}

async function addPaths(paths: string[]) {
  if (!paths.length) return;
  try {
    const found = await invoke<Item[]>("inspect_paths", { paths });
    const st = await stats(found.map((x) => x.path));
    for (const it of found) {
      it.size = st.get(it.path)?.size;
      if (!items.some((x) => x.path === it.path)) items.push(it);
    }
    const missing = found.filter((x) => x.kind === "missing");
    if (missing.length) toast(`찾을 수 없는 항목 ${missing.length}개는 뺐어요`);
    items = items.filter((x) => x.kind !== "missing");
    if (phase !== "run") phase = "pick";
    if (pick && !groups().some((g) => g.cls === pick!.cls && g.actions.some((a) => a.id === pick!.id))) pick = null;
    render();
    // 한글 문서: 이 PC에서 되는 일만(W6)
    for (const it of items.filter((x) => isHwp(x) && (x as HItem).hwp === undefined) as HItem[]) {
      it.hwp = null;
      invoke<HwpCaps>("hwp_capabilities", { path: it.path })
        .then((c) => { it.hwp = c; if (phase === "pick") render(); })
        .catch(() => { /* 명령이 아직 없으면 예전대로 */ });
    }
    // 암호 걸린 PDF 표시(뒤에서)
    for (const it of items.filter((x) => x.kind === "pdf" && x.locked === undefined)) {
      invoke<{ needs_password: boolean }>("pdf_protection", { path: it.path })
        .then((p) => { it.locked = p.needs_password; if (p.needs_password && phase === "pick") render(); })
        .catch((e) => { it.locked = needsPassword(e); if (it.locked && phase === "pick") render(); });
    }
  } catch (e) {
    toast(friendly(e).title);
  }
}

function fileMeta(it: Item): string {
  const kind = { pdf: "PDF", image: "그림", heic: "아이폰 사진", video: "영상", audio: "소리", document: "문서", zip: "zip", dir: "폴더", other: "파일", missing: "없음" }[it.kind];
  return [kind, it.size ? human(it.size) : "", it.pages != null ? `${it.pages}쪽` : "", it.locked ? "암호 걸림" : ""].filter(Boolean).join(" · ");
}

function fileCards(): string {
  if (items.length === 1) {
    const it = items[0];
    return `<div class="filecard"><div class="ftw">${FT(it.kind === "dir" ? "folder" : extOf(it.path), 36)}${it.locked ? `<span class="lockbadge">${I("lock")}</span>` : ""}</div>
      <div class="fc-t"><b title="${esc(it.path)}">${esc(fileName(it.path))}</b><span>${esc(fileMeta(it))} · ${esc(dirOf(it.path))}</span></div>
      ${phase === "run" ? "" : `<button type="button" class="btn subtle" data-act="more">${I("plus")}<span class="hide-n">더 넣기</span></button><button type="button" class="btn subtle" data-act="clear">${I("x")}<span class="hide-n">다른 파일</span></button>`}</div>`;
  }
  const total = items.reduce((a, x) => a + (x.size ?? 0), 0);
  const kinds = groups().map((g) => CLS_LABEL[g.cls](g.items.length)).join(" · ");
  const rows = items.map((it, i) => `<li class="fl-i">${FT(it.kind === "dir" ? "folder" : extOf(it.path), 22)}${it.locked ? `<span class="mini-lock" title="암호 걸림">${I("lock")}</span>` : ""}
      <span class="fl-n" title="${esc(it.path)}">${esc(fileName(it.path))}</span><span class="fl-m">${esc(it.size ? human(it.size) : "")}${it.pages != null ? ` · ${it.pages}쪽` : ""}</span>
      ${phase === "run" ? "" : `<span class="fl-b"><button type="button" class="ib" data-up="${i}" title="위로" aria-label="${esc(fileName(it.path))} 위로" ${i === 0 ? "disabled" : ""}>${I("up")}</button><button type="button" class="ib" data-down="${i}" title="아래로" aria-label="${esc(fileName(it.path))} 아래로" ${i === items.length - 1 ? "disabled" : ""}>${I("down")}</button><button type="button" class="ib" data-rm="${i}" title="빼기" aria-label="${esc(fileName(it.path))} 빼기">${I("x")}</button></span>`}</li>`).join("");
  return `<div class="filecard multi"><div class="fc-head"><div class="stack">${items.slice(0, 3).map((x) => FT(x.kind === "dir" ? "folder" : extOf(x.path), 32)).join("")}</div>
      <div class="fc-t"><b>${items.length}개 넣었어요</b><span>${esc(kinds)}${total ? ` · ${human(total)}` : ""}</span></div>
      ${phase === "run" ? "" : `<button type="button" class="btn subtle" data-act="more">${I("plus")}<span class="hide-n">더 넣기</span></button><button type="button" class="btn subtle" data-act="clear">${I("x")}<span class="hide-n">모두 빼기</span></button>`}</div>
    <ul class="fl">${rows}</ul></div>`;
}

function render() {
  if (!items.length) {
    root.innerHTML = `<h1 class="title">변환</h1><p class="sub">파일을 넣으면 그 파일로 할 수 있는 일만 보여 드려요. 원본은 그대로 둬요.</p>
      ${welcomeHTML()}
      <div class="dz" id="dz" tabindex="0" role="button" aria-label="파일 고르기 — 또는 여기에 끌어다 놓기">
        <div class="dz-ic">${I("upload")}</div><div class="dz-t">파일이나 폴더를 여기에 끌어다 놓으세요</div>
        <div class="dz-s">PDF · Word · 한글 · PPT · 엑셀 · 사진(HEIC 포함) · 영상 · 소리 · zip · 폴더</div>
        <div class="row center"><button type="button" class="btn accent" data-act="pick-files">파일 고르기</button><button type="button" class="btn" data-act="pick-folder">폴더 고르기</button></div></div>`;
    bindWelcome(root);
    return;
  }
  const sub = phase === "run" ? "원본은 그대로 두고 새 파일로 저장해요." : phase === "done" ? "원본은 그대로 두고 새 파일로 저장했어요." : "무엇을 할까요? 카드를 고르면 설정이 바로 아래에 나와요.";
  let h = `<h1 class="title">변환</h1><p class="sub">${sub}</p>${fileCards()}`;
  if (phase === "pick") {
    const gs = groups();
    if (!gs.length || (gs.every((g) => !g.actions.length) && !items.some((x) => (x as HItem).hwp?.message))) {
      h += errorPanel({ kind: "unsupported", icon: "info", title: "이 파일로 할 수 있는 일이 없어요", body: "PDF·문서·사진·영상·소리·zip·폴더를 넣어 주세요.", detail: "" },
        `<button type="button" class="btn accent" data-act="clear">다른 파일 넣기</button>`);
    }
    const lockedN = items.filter((x) => x.locked).length;
    for (const g of gs) {
      if (!g.actions.length) {
        h += `<p class="grp-h">${esc(CLS_LABEL[g.cls](g.items.length))} — 이 파일로는 할 수 있는 일이 없어요</p>`;
        for (const it of g.items as HItem[]) if (it.hwp?.message) h += `<div class="note warn">${I("info")}<span><b>${esc(fileName(it.path))}</b> — ${esc(it.hwp.message)}</span></div>`;
        continue;
      }
      if (gs.length > 1) h += `<p class="grp-h">${esc(CLS_LABEL[g.cls](g.items.length))}</p>`;
      if (g.cls === "word") for (const it of g.items as HItem[]) {
        if (it.hwp?.message && (!it.hwp.can_pdf || !it.hwp.can_pptx)) h += `<div class="note warn">${I("info")}<span><b>${esc(fileName(it.path))}</b> — ${esc(it.hwp.message)}</span></div>`;
      }
      if (g.cls === "pdf" && lockedN) h += `<div class="note warn">${I("lock")}<span>암호가 걸린 PDF가 ${lockedN}개 있어요. 할 일을 고르면 암호를 물어보거나, [암호 걸기·풀기]로 먼저 풀 수 있어요.</span></div>`;
      h += `<div class="cards" role="list">${g.actions.map((a) => {
        const on = pick?.cls === g.cls && pick.id === a.id;
        return `<button type="button" role="listitem" class="card ${on ? "on" : ""}" data-cls="${g.cls}" data-pick="${a.id}" aria-pressed="${on}">
          <span class="tile tone-${a.tone}">${I(a.ic)}</span><span class="card-t"><b>${esc(a.t)}</b><span class="d">${esc(a.d)}</span></span>${a.jump ? `<span class="go">${I("right")}</span>` : `<span class="tick">${I("check")}</span>`}</button>`;
      }).join("")}</div>`;
      if (pick?.cls === g.cls) h += `<div class="optslot"></div>`;
    }
    if (!pick && gs.some((g) => g.actions.length)) h += `<div class="pickhint">${I("info")}할 일을 고르면 설정과 실행 단추가 여기에 나와요.</div>`;
  } else if (phase === "run") {
    const a = curAction();
    h += progressPanel(a ? `${a.t} 하는 중` : "하는 중");
  } else {
    h += resultsHTML();
  }
  root.innerHTML = h;
  if (phase === "pick" && pick) renderOptions();
}

function curAction(): Action | undefined {
  return pick ? ACTIONS[pick.cls].find((a) => a.id === pick!.id) : undefined;
}
function itemsFor(cls: Cls, a: Action): Item[] {
  const xs = items.filter((x) => x.kind !== "missing" && clsOf(x) === cls);
  return a.pickItems ? a.pickItems(xs) : xs;
}

function renderOptions() {
  const a = curAction();
  const slot = $(".optslot", root);
  if (!a || !slot || !pick) return;
  const xs = itemsFor(pick.cls, a);
  const saveTo = outDir ?? (xs.length ? dirOf(xs[0].path) : "");
  slot.outerHTML = `<div class="optcard" id="optcard"><div class="oc-body"><div class="oc-h">${esc(a.t)}${xs.length > 1 ? `<span class="hint">· ${xs.length}개</span>` : ""}</div>
      ${a.info ? `<div class="info">${I("info")}<span>${esc(a.info)}</span></div>` : ""}<div class="opts">${a.opts ? a.opts({ items: xs }) : ""}</div><p class="oc-err" role="alert" hidden></p></div>
    <div class="oc-foot">${a.noOutDir ? `<div class="save">${I("info")}<span>폴더 안에서 바로 바꿔요</span></div>` : `<div class="save">${I("folder")}<b>${outDir ? "저장 위치" : "원본 옆"}</b><span class="path" title="${esc(saveTo)}">${esc(saveTo)}</span>
      <button type="button" class="link" data-act="outdir">바꾸기</button>${outDir ? `<button type="button" class="link" data-act="outdir-reset">원본 옆으로</button>` : ""}</div>`}
      <button type="button" class="btn accent lg" data-act="run">${esc(a.run)}</button></div></div>`;
  const card = $("#optcard", root)!;
  bindSegs(card);
  ctx = { items: xs, panel: card, outDir };
  a.setup?.(ctx);
}

function resultsHTML(): string {
  const oks = lastResults.filter((r) => !(r as ResultLine & { err?: unknown }).err);
  const errs = lastResults.filter((r) => (r as ResultLine & { err?: unknown }).err);
  const a = curAction();
  const after = `<div class="after-row"><button type="button" class="btn subtle" data-act="again">${I("convert")}이 파일로 다른 작업</button>
    ${oks.some((r) => !r.isFolder && r.output !== r.input) ? `<button type="button" class="btn subtle" data-act="use-results">${I("right")}결과 파일로 이어서 하기</button>` : ""}
    <button type="button" class="btn subtle" data-act="clear">${I("upload")}다른 파일 넣기</button></div>`;
  if (!oks.length && errs.length === 1) {
    const e = (errs[0] as ResultLine & { err: unknown }).err;
    const f = friendly(e);
    return errorPanel(f, `<button type="button" class="btn accent" data-act="again">다른 할 일 고르기</button><button type="button" class="btn" data-act="clear">다른 파일 넣기</button>`);
  }
  if (!oks.length && !errs.length) {
    return `<div class="panel"><div class="done-h"><span class="okc gray">${I("x")}</span><div><b>멈췄어요</b><span>만들던 파일은 남기지 않았어요.</span></div></div>${after}</div>`;
  }
  const title = errs.length ? (oks.length ? `${oks.length}개는 됐고, ${errs.length}개는 못 했어요` : "끝내지 못했어요") : "다 됐어요";
  const sub = `${a ? a.t : ""} · ${outDir ? "고른 폴더에 저장했어요" : "원본 옆에 저장했어요"}${stopFlag ? " · 중간에 멈췄어요" : ""}`;
  return `<div class="panel" id="results"><div class="done-h"><span class="okc ${errs.length ? "warn" : ""}">${I(errs.length ? "warn" : "check")}</span><div><b>${esc(title)}</b><span>${esc(sub)}</span></div></div>
    <div class="rlist">${oks.map(resultRow).join("")}${errs.map((r) => errRow(r.input ?? r.output, (r as ResultLine & { err: unknown }).err)).join("")}</div>${after}</div>`;
}

async function run() {
  const a = curAction();
  if (!a || !a.exec || !pick || running) return;
  const xs = itemsFor(pick.cls, a);
  if (!xs.length) return;
  const card = $("#optcard", root)!;
  const errEl = $(".oc-err", card)!;
  errEl.hidden = true;
  const panel = card; // 값은 실행 전에 그대로 두고 읽는다(판은 진행 중에 숨김)
  const holder = document.createElement("div");
  holder.hidden = true;
  holder.appendChild(panel);
  running = true;
  stopFlag = false;
  lastResults = [];
  phase = "run";
  render();
  root.appendChild(holder);
  const pg = $(".panel.pg", root)!;
  $("[data-cancel]", pg)!.addEventListener("click", async () => {
    stopFlag = true;
    ($("[data-cancel]", pg) as HTMLButtonElement).disabled = true;
    setProgress(pg, null, "멈추는 중…");
    await cancelJob(runJob);
  });
  let base = 0, n = 1;
  const r: Run = {
    items: xs, panel, outDir, trim: ctx?.trim,
    job: () => {
      if (runJob) offJobProgress(runJob);
      runJob = newJob();
      onJobProgress(runJob, (f, d) => {
        if (d && d.total > 1 && n <= 1) setProgress(pg, f, `${d.done} / ${d.total}${d.current ? ` · ${fileName(d.current)}` : ""}`);
        else setProgress(pg, (base + f) / n);
      });
      return runJob;
    },
    step: (i, total, name) => { base = i; n = Math.max(1, total); setProgress(pg, i / n, total > 1 ? `${i + 1} / ${total} · ${name}` : name); },
    frac: (f) => setProgress(pg, (base + f) / n),
    ok: (x) => lastResults.push(x),
    fail: (input, e) => lastResults.push({ input, output: input, err: e } as ResultLine),
    stopped: () => stopFlag,
  };
  try {
    await a.exec(r);
  } catch (e) {
    if (lastResults.length === 0 && !isCancelled(e) && e instanceof Error) {
      // 입력 확인 오류(예: 암호가 달라요) → 설정 판으로 돌아가 그 자리에 보여 줌
      running = false;
      phase = "pick";
      render();
      const err = $(".oc-err", root);
      if (err) { err.hidden = false; err.textContent = e.message; }
      restoreValues(panel);
      return;
    }
    if (!isCancelled(e)) lastResults.push({ input: xs[0].path, output: xs[0].path, err: e } as ResultLine);
  } finally {
    if (runJob) offJobProgress(runJob);
    running = false;
  }
  holder.remove();
  // 결과 크기 채우기
  const need = lastResults.filter((x) => !(x as { err?: unknown }).err && !x.isFolder && x.after == null && x.output !== x.input).map((x) => x.output);
  const st = await stats(need);
  for (const x of lastResults) if (x.after == null && st.has(x.output)) x.after = st.get(x.output)!.size;
  phase = "done";
  render();
  $("#results", root)?.querySelector<HTMLButtonElement>(".rr .btn")?.focus();
}

/** 입력 확인 오류로 돌아왔을 때 사용자가 넣은 값 되살리기 */
function restoreValues(old: HTMLElement) {
  const now = $("#optcard", root);
  if (!now) return;
  for (const inp of $$<HTMLInputElement>("input", old)) {
    const t = inp.id ? $<HTMLInputElement>(`#${inp.id}`, now) : null;
    if (t) { if (inp.type === "checkbox") t.checked = inp.checked; else t.value = inp.value; }
  }
  for (const g of $$<HTMLElement>("[data-seg]", old)) {
    const v = $("button.on", g)?.dataset.v;
    const t = $(`[data-seg="${g.dataset.seg}"]`, now);
    if (t && v != null) $$<HTMLButtonElement>("button", t).forEach((b) => b.classList.toggle("on", b.dataset.v === v));
  }
  now.dispatchEvent(new MouseEvent("click", { bubbles: false }));
}

// ---------- 화면 ----------

export const convertScreen = {
  id: "convert", title: "변환", icon: "convert",
  init(el: HTMLElement) {
    root = el;
    bindResultButtons(root);
    root.addEventListener("click", async (e) => {
      const t = e.target as HTMLElement;
      const b = t.closest<HTMLElement>("[data-act], [data-pick], [data-up], [data-down], [data-rm]");
      if (!b) { if (t.closest("#dz")) addPaths(await pickFiles()); return; }
      if (b.dataset.pick) {
        const cls = b.dataset.cls as Cls, a = ACTIONS[cls].find((x) => x.id === b.dataset.pick)!;
        if (a.jump) { a.jump(items.filter((x) => clsOf(x) === cls)); return; }
        pick = pick?.id === a.id && pick.cls === cls ? null : { cls, id: a.id };
        render();
        if (pick) {
          const oc = $("#optcard", root);
          oc?.scrollIntoView({ block: "nearest", behavior: "smooth" });
          (oc?.querySelector<HTMLElement>("input, [data-seg] button.on") ?? oc?.querySelector<HTMLElement>("[data-act=run]"))?.focus({ preventScroll: true });
        }
        return;
      }
      const move = (i: number, d: number) => { [items[i], items[i + d]] = [items[i + d], items[i]]; render(); };
      if (b.dataset.up) return move(Number(b.dataset.up), -1);
      if (b.dataset.down) return move(Number(b.dataset.down), 1);
      if (b.dataset.rm) { items.splice(Number(b.dataset.rm), 1); if (!items.length) { pick = null; phase = "pick"; } render(); return; }
      switch (b.dataset.act) {
        case "pick-files": addPaths(await pickFiles()); break;
        case "pick-folder": addPaths(await pickFiles({ directory: true })); break;
        case "more": addPaths(await pickFiles()); break;
        case "clear": items = []; pick = null; phase = "pick"; render(); break;
        case "again": phase = "pick"; render(); break;
        case "use-results": {
          const outs = lastResults.filter((r) => !(r as { err?: unknown }).err && !r.isFolder && r.output !== r.input).map((r) => r.output);
          items = []; pick = null; phase = "pick";
          await addPaths(outs);
          break;
        }
        case "outdir": {
          const [d] = await pickFiles({ directory: true, multiple: false });
          if (d) { outDir = d; render(); }
          break;
        }
        case "outdir-reset": outDir = null; render(); break;
        case "run": run(); break;
      }
    });
    root.addEventListener("keydown", async (e) => {
      if ((e.target as HTMLElement).id === "dz" && (e.key === "Enter" || e.key === " ")) { e.preventDefault(); addPaths(await pickFiles()); }
    });
    render();
  },
  show(arg?: unknown) {
    const a = arg as { add?: string[] } | undefined;
    if (a?.add) addPaths(a.add);
    else if (!items.length) render(); // 처음 안내 다시 보기 등
  },
  drop(paths: string[]) {
    if (running) { toast("지금 하는 작업이 끝난 뒤에 넣어 주세요"); return; }
    if (phase === "done") { items = []; pick = null; phase = "pick"; }
    addPaths(paths);
  },
} satisfies Screen as Screen;

export const _test = { clsOf, ACTIONS, stem };
