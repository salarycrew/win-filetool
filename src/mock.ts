// 개발용 가짜 백엔드: `npm run dev`를 브라우저로 열 때만 쓴다(boot.ts가 import.meta.env.DEV일 때만 불러옴 — 배포 빌드에 없음).
// W4 명령(image_edit·media_*·zip_*·rename_*·dup_*)의 화면을 계약대로 미리 만들어 보는 데 쓴다.
type Args = Record<string, unknown>;
const listeners = new Map<string, ((e: { payload: unknown }) => void)[]>();
const emit = (ev: string, payload: unknown) => (listeners.get(ev) ?? []).forEach((f) => f({ payload }));
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const q = new URLSearchParams(location.search);
const baseName = (p: string) => p.split(/[\\/]/).pop()!;
const dir = (p: string) => p.replace(/[\\/][^\\/]*$/, "");
const stemOf = (p: string) => baseName(p).replace(/\.[^.]+$/, "");
const extOf = (p: string) => (baseName(p).includes(".") ? baseName(p).split(".").pop()!.toLowerCase() : "");
const D = "C:\\Users\\me\\Documents";

function kindOf(p: string) {
  const e = extOf(p);
  if (!e) return "dir";
  if (e === "pdf") return "pdf";
  if (e === "zip") return "zip";
  if (["heic", "heif"].includes(e)) return "heic";
  if (["jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff"].includes(e)) return "image";
  if (["mov", "mp4", "mkv", "avi", "webm"].includes(e)) return "video";
  if (["mp3", "m4a", "wav", "flac"].includes(e)) return "audio";
  if (["docx", "doc", "hwp", "odt", "pages", "pptx", "ppt", "xlsx", "xls", "key", "numbers", "rtf"].includes(e)) return "document";
  return "other";
}
const SIZES: Record<string, number> = { pdf: 18_200_000, docx: 1_400_000, heic: 2_600_000, jpg: 3_100_000, mov: 412_000_000, zip: 8_400_000, mp4: 120_000_000 };

function pageImg(n: number, w: number, landscape = false): string {
  const h = Math.round(landscape ? w * 0.707 : w * 1.414);
  const c = document.createElement("canvas");
  c.width = w; c.height = h;
  const g = c.getContext("2d")!;
  g.fillStyle = "#fff"; g.fillRect(0, 0, w, h);
  g.fillStyle = "#1F5BAA"; g.fillRect(w * 0.1, h * 0.08, w * 0.8, h * 0.05);
  g.fillStyle = "#c9ccd2";
  for (let i = 0; i < 14; i++) g.fillRect(w * 0.1, h * (0.18 + i * 0.045), w * (i % 4 === 3 ? 0.5 : 0.8), h * 0.018);
  g.fillStyle = "#e9d6c4"; g.fillRect(w * 0.1, h * 0.82, w * 0.35, h * 0.1);
  g.fillStyle = "#333"; g.font = `bold ${Math.round(w * 0.12)}px sans-serif`; g.textAlign = "center"; g.fillText(String(n), w / 2, h * 0.75);
  return c.toDataURL("image/png");
}
function frameImg(t: number, w: number): string {
  const h = Math.round(w * 0.5625);
  const c = document.createElement("canvas");
  c.width = w; c.height = h;
  const g = c.getContext("2d")!;
  const hue = (t * 7) % 360;
  const gr = g.createLinearGradient(0, 0, w, h);
  gr.addColorStop(0, `hsl(${hue},55%,45%)`); gr.addColorStop(1, `hsl(${(hue + 60) % 360},60%,30%)`);
  g.fillStyle = gr; g.fillRect(0, 0, w, h);
  g.fillStyle = "rgba(255,255,255,.85)"; g.beginPath(); g.arc(w * (0.3 + (t % 10) / 20), h * 0.55, h * 0.18, 0, 7); g.fill();
  g.fillStyle = "#fff"; g.font = `${Math.round(h * 0.12)}px sans-serif`; g.fillText(`${Math.floor(t / 60)}:${String(Math.floor(t % 60)).padStart(2, "0")}`, 8, h - 8);
  return c.toDataURL("image/jpeg", 0.8);
}

async function job(a: Args, steps = 6, ms = 120) {
  for (let i = 1; i <= steps; i++) { await sleep(ms); emit("job-progress", { job: a.job, fraction: i / steps }); }
}
const out = (p: string, suf: string, ext = extOf(p)) => `${dir(p)}\\${stemOf(p)}${suf}.${ext}`;

const SEARCH = [
  ["계약서_최종.pdf", "C:\\Users\\me\\Downloads", 18_200_000], ["계약서_최종_압축.pdf", "C:\\Users\\me\\Downloads", 2_100_000], ["계약서", "C:\\Users\\me\\Documents\\회사", 0],
  ["계약서 초안 v3.docx", "C:\\Users\\me\\Documents\\회사\\법무", 86_000], ["계약서_검토의견.xlsx", "C:\\Users\\me\\Documents\\회사\\법무", 31_000], ["임대차 계약서.hwp", "C:\\Users\\me\\Documents\\집", 54_000],
  ["계약서 스캔본.jpg", "D:\\스캔\\2025", 3_400_000], ["용역계약서_서명.pdf", "C:\\Users\\me\\OneDrive\\공유", 412_000], ["표준계약서_양식.docx", "D:\\자료\\양식", 44_000], ["근로계약서_2026.pdf", "C:\\Users\\me\\Documents\\회사", 220_000],
] as const;
let lastQ = "";

const H: Record<string, (a: Args) => unknown> = {
  ui_get: () => ({ tab: q.get("s") ?? undefined }),
  ui_set: () => null,
  startup_tab: () => null,
  startup_query: () => null,
  inspect_paths: (a) => (a.paths as string[]).map((p) => ({ path: p, kind: kindOf(p), pages: kindOf(p) === "pdf" ? 12 : null })),
  file_stats: (a) => (a.paths as string[]).map((p) => ({ path: p, exists: true, is_dir: kindOf(p) === "dir", size: kindOf(p) === "dir" ? 0 : /_압축/.test(p) ? 2_100_000 : /_편집/.test(p) ? 24_000_000 : SIZES[extOf(p)] ?? 900_000, modified: 1761900000000 })),
  list_files: (a) => ["IMG_4021.JPG", "IMG_4022.JPG", "IMG_4023.JPG", "IMG_4025.HEIC", "IMG_4030.JPG"].map((n) => `${a.folder}\\${n}`),
  image_data_url: () => { const c = document.createElement("canvas"); c.width = 300; c.height = 120; const g = c.getContext("2d")!; g.strokeStyle = "#1a3a8a"; g.lineWidth = 5; g.beginPath(); g.moveTo(20, 80); g.bezierCurveTo(60, 10, 90, 110, 130, 50); g.bezierCurveTo(170, 0, 200, 110, 280, 40); g.stroke(); return c.toDataURL(); },
  pdf_protection: (a) => ({ needs_password: /암호|급여/.test(String(a.path)), encrypted: false }),
  pdf_pages: (a) => {
    if (/암호|급여/.test(String(a.path)) && !a.password) throw { code: "password_required", message: "암호가 필요합니다" };
    if (/암호|급여/.test(String(a.path)) && a.password !== "1234") throw { code: "wrong_password", message: "암호가 틀렸습니다" };
    return { pages: Array.from({ length: 12 }, (_, i) => ({ page: i + 1, width: 595, height: 842, rotate: 0, visual_width: i === 4 ? 842 : 595, visual_height: i === 4 ? 595 : 842 })), encrypted: !!a.password };
  },
  pdf_thumbs: (a) => (a.pages as number[]).map((p) => ({ page: p, width: a.width, height: 0, data_url: pageImg(p, a.width as number, p === 5), ms: 5 })),
  pdf_apply_plan: async (a) => { await job(a); const pl = a.plan as { sources: { path: string }[]; pages: unknown[] }; return { output: out(pl.sources[0].path, "_쪽편집"), pages: pl.pages.length, note: `${pl.pages.length}쪽` }; },
  pdf_extract: async (a) => { await job(a); return { output: out(String(a.path), "_쪽뽑기"), pages: 2, note: "2쪽" }; },
  pdf_page_numbers: async (a) => { await job(a); return { output: out(String(a.path), "_쪽번호"), pages: 12, note: "12쪽에 번호" }; },
  pdf_watermark: async (a) => { await job(a); return { output: out(String(a.path), "_워터마크"), pages: 12, note: "" }; },
  pdf_place_image: async (a) => { await job(a); return { output: out(String(a.path), "_서명"), pages: 12, note: "" }; },
  pdf_text_boxes: async (a) => { await job(a); return { output: out(String(a.path), "_글상자"), pages: 12, note: "" }; },
  pdf_crop: async (a) => { await job(a); return { output: out(String(a.path), "_여백자름"), pages: 12, note: "" }; },
  pdf_fit_a4: async (a) => { await job(a); return { output: out(String(a.path), "_A4"), pages: 12, note: "" }; },
  pdf_nup: async (a) => { await job(a); return { output: out(String(a.path), "_모아찍기"), pages: 6, note: "" }; },
  pdf_protect: async (a) => { await sleep(300); return { output: out(String(a.path), "_암호"), pages: 12, note: "" }; },
  pdf_unprotect: async (a) => { await sleep(300); if (a.password !== "1234") throw { code: "wrong_password", message: "암호가 틀렸습니다" }; return { output: out(String(a.path), "_암호풀림"), pages: 12, note: "" }; },
  pdf_info_get: () => ({ title: "2026 사업계획", author: "홍길동", subject: null, keywords: null, creator: "Microsoft Word", producer: "Microsoft: Print To PDF", pages: 12 }),
  pdf_info_set: async (a) => ({ output: out(String(a.path), "_정보수정"), pages: 12, note: "" }),
  pdf_info_clear: async (a) => ({ output: out(String(a.path), "_정보삭제"), pages: 12, note: "" }),
  pdf_form_list: () => [{ name: "이름", kind: "text", value: "", options: [], read_only: false, page: 1 }, { name: "생년월일", kind: "text", value: "", options: [], read_only: false, page: 1 }, { name: "동의함", kind: "checkbox", value: "Off", options: ["Yes"], read_only: false, page: 2 }, { name: "지역", kind: "combo", value: "", options: ["서울", "부산", "제주"], read_only: false, page: 2 }],
  pdf_form_fill: async (a) => ({ output: out(String(a.path), "_양식채움"), pages: 2, note: "" }),
  pdf_flatten: async (a) => ({ output: out(String(a.path), "_평평하게"), pages: 12, note: "" }),
  pdf_merge: async (a) => { await sleep(500); const ps = a.paths as string[]; return { output: out(ps[0], "_합침"), pages: 12 * ps.length }; },
  pdf_split: async (a) => { await sleep(400); return Array.from({ length: 12 }, (_, i) => out(String(a.path), `_${i + 1}`)); },
  pdf_compress: async (a) => { await sleep(700); if (/암호|급여/.test(String(a.path))) throw "암호가 걸린 PDF는 열 수 없습니다. PDF 프로그램에서 암호를 풀어 저장한 뒤 다시 시도하세요"; return { input: a.path, output: out(String(a.path), "_압축"), before: 18_200_000, after: 2_100_000, pages: 12, note: "" }; },
  pdf_to_docx: async (a) => { await job(a, 10); return { output: out(String(a.path), "_Word", "docx"), pages: 12, scanned_pages: 0 }; },
  pdf_to_pptx: async (a) => { await job(a, 10); return { output: out(String(a.path), "_PPT", "pptx"), slides: 12 }; },
  pdf_to_txt: async (a) => { await job(a); return { output: out(String(a.path), "", "txt"), chars: 23110 }; },
  pdf_to_images: async (a) => { await job(a); return { folder: `${dir(String(a.path))}\\${stemOf(String(a.path))}_쪽이미지`, pages: Array.from({ length: 12 }, (_, i) => ({ page: i + 1, output: "x" })) }; },
  extract_images: async (a) => { await job(a, 2); return { folder: `${dir(String(a.path))}\\${stemOf(String(a.path))}_이미지`, files: Array(7).fill("x") }; },
  doc_to_pptx: async (a) => { emit("tools_unpack_progress", { tool: "lo", fraction: 0.2 }); await sleep(600); emit("tools_unpack_progress", { tool: "lo", fraction: 1 }); await job(a); return { output: out(String(a.path), "_PPT", "pptx"), slides: 9 }; },
  convert_one: async (a) => {
    await job(a, 8);
    const op = a.op as { kind: string; target?: string; format?: string; preset?: string };
    const e = op.kind === "doc" ? op.target! : op.kind === "image" ? op.format! : ({ mp4: "mp4", messenger: "mp4", mp3: "mp3", m4a: "m4a", gif: "gif" } as Record<string, string>)[op.preset!];
    return { input: a.path, output: out(String(a.path), op.preset === "messenger" ? "_메신저" : "", e), note: "" };
  },
  images_to_pdf: async (a) => { await job(a); const ps = a.paths as string[]; return { output: out(ps[0], `_외${ps.length - 1}장`, "pdf"), note: "" }; },
  image_edit: async (a) => {
    const ins = a.inputs as string[];
    for (let i = 0; i < ins.length; i++) { await sleep(150); emit("tools_progress", { job: a.job, done: i + 1, total: ins.length, current: ins[i] }); }
    return ins.map((p) => ({ input: p, output: out(p, "_편집", (a.ops as { format?: string }).format === "jpg" || extOf(p) === "heic" ? "jpg" : extOf(p)), before: 3_100_000, after: 480_000 }));
  },
  media_probe: () => ({ duration: 151.4, width: 1920, height: 1080, rotation: 0, has_audio: true, video_codec: "hevc", audio_codec: "aac" }),
  media_frame: (a) => frameImg(Number(a.at), Number(a.width) || 320),
  media_frame_save: (a) => `${dir(String(a.path))}\\${stemOf(String(a.path))}_장면_00-12.jpg`,
  media_edit: async (a) => { await job(a, 8); return { output: out(String(a.input), "_편집", "mp4"), before: 412_000_000, after: 96_000_000, note: "키프레임에 맞춰 자르느라 시작이 0.4초 앞당겨졌어요" }; },
  zip_list: (a) => ({ entries: a.encoding === "utf8" ? [{ name: "Ã«Â³Â´ÃªÂ³Â Ã¬â€\u009dÅ“.hwp", size: 1, is_dir: false }] : [{ name: "보고서/", size: 0, is_dir: true }, { name: "보고서/1분기 보고서.hwp", size: 1, is_dir: false }, { name: "보고서/사진 01.jpg", size: 1, is_dir: false }], detected_encoding: "cp949", garbled_guess: a.encoding === "utf8" }),
  zip_extract: async (a) => { await sleep(500); return { folder: `${dir(String(a.path))}\\${stemOf(String(a.path))}`, count: 24, skipped: [] }; },
  zip_create: async (a) => { await sleep(500); const i = (a.inputs as string[])[0]; return { output: `${i}.zip`, count: 31, size: 12_400_000 }; },
  rename_preview: (a) => {
    const r = a.rule as { prefix?: string; suffix?: string; find?: string; replace?: string; numbering?: { start: number; digits: number; position: string; step: number } };
    return (a.files as string[]).map((f, i) => {
      let s = stemOf(f);
      if (r.find) s = s.split(r.find).join(r.replace ?? "");
      if (r.numbering) { const n = String(r.numbering.start + i * r.numbering.step).padStart(r.numbering.digits, "0"); s = r.numbering.position === "prefix" ? `${n}_${s}` : r.numbering.position === "suffix" ? `${s}_${n}` : n; }
      s = (r.prefix ?? "") + s + (r.suffix ?? "");
      const to = `${dir(f)}\\${s}.${extOf(f)}`;
      return { from: f, to, conflict: baseName(to) === "IMG_4022.JPG" && baseName(f) !== "IMG_4022.JPG" ? "같은 이름이 이미 있어요" : null };
    });
  },
  rename_apply: async (a) => ({ done: (a.pairs as unknown[]).length, failed: [], undo_id: "u1" }),
  rename_undo: async () => ({ done: 5, failed: [] }),
  dup_scan: async (a) => {
    for (let i = 1; i <= 10; i++) { await sleep(120); emit("tools_progress", { job: a.job, done: i * 120, total: 1200, current: `${D}\\사진\\IMG_${4000 + i}.JPG` }); }
    return { scanned: 1200, wasted_bytes: 9_800_000, groups: [
      { size: 3_400_000, hash: "a", files: [{ path: `${D}\\사진\\IMG_4021.JPG`, modified: 1761800000000 }, { path: `${D}\\사진\\IMG_4021 (1).JPG`, modified: 1761900000000 }, { path: "C:\\Users\\me\\Desktop\\IMG_4021.JPG", modified: 1761950000000 }] },
      { size: 3_000_000, hash: "b", files: [{ path: `${D}\\계약서_최종.pdf`, modified: 1761000000000 }, { path: "C:\\Users\\me\\Downloads\\계약서_최종 (1).pdf", modified: 1761100000000 }] },
    ] };
  },
  dup_trash: async (a) => ({ done: (a.paths as unknown[]).length, failed: [] }),
  nfc_preview: () => ({ root: "x", renames: [{ from: "보고서.hwp", to: "보고서.hwp" }, { from: "사진1.jpg", to: "사진1.jpg" }], collisions: [], unreadable: [], scanned: 40 }),
  nfc_apply: () => ({ renamed: [1, 2], collisions: [], failed: [] }),
  cancel_jobs: () => 1, cancel_job: () => 1,
  open_path: () => null, open_folder: () => null, open_dir: () => null,
  file_icons: (a) => Object.fromEntries((a.keys as string[]).map((k) => [k, null])),
  search_query: (a) => { lastQ = String(a.query); const n = lastQ.trim() ? (lastQ.includes("없는") ? 0 : SEARCH.length) : 0; return { total: n, millis: 3.8, generation: 1, version: 1 }; },
  search_rows: () => ({ rows: SEARCH.map(([n, f, s], i) => ({ id: i, name: n, folder: f, path: `${f}\\${n}`, is_dir: !s, size: s, mtime: 1761800000 - i * 86400 * 13 })), stale: false }),
  search_status: () => ({ status: { entries: 1814929, roots: [{ path: "C:\\", scanning: false, scanned_so_far: 0, last_scan: 1761900000, stats: { dirs: 312000, files: 1500000, denied: 0, reparse: 0, excluded: 0, millis: 0 }, source: "빠른 색인(서비스)", source_note: null, catchup: null }], loaded_ms: 120, load_error: null, last_save: 0, index_bytes: 88_000_000, index_file: "C:\\Users\\me\\AppData\\Local\\kr.salarycrew.winfiletool\\index.bin", skipped: 0, excluded: 0, version: 1, generation: 1, ready: true, live_updates: 12, memory_bytes: 0 },
    settings: { roots: [], excludes: ["C:\\Windows\\WinSxS"], hotkey_enabled: true, hotkey: "Ctrl+Alt+F" }, effective_roots: ["C:\\"], service_roots: ["C:\\"] }),
  search_set_settings: () => H.search_status({}),
  search_rescan: () => null,
  svc_view: () => ({ info: { installed: true, state: "실행 중", running: true, pid: 1, start_type: "자동", account: "LocalSystem", binary_path: "" }, connected: true, entries: 1814929, ready: true, roots: ["C:\\"], visibility: "내 파일만 보임", error: null, exe: "filetool-svc.exe", install_cmd: "filetool-svc.exe install", uninstall_cmd: "filetool-svc.exe uninstall" }),
  autostart_get: () => ({ enabled: false, value: null, matches_exe: true, supported: true }),
  autostart_set: (a) => ({ enabled: a.enabled, value: null, matches_exe: true, supported: true }),
  tools_status: () => [{ tool: "ffmpeg", label: "영상·소리(ffmpeg)", found: true, path: "…", source: "앱에 든 도구", version: "8.1" }, { tool: "soffice", label: "문서(LibreOffice)", found: true, path: "…", source: "앱에 든 도구", version: "25.8" }],
  tools_bundle_status: () => ({ present: true, tools: [{ name: "ffmpeg", ready: true, bytes: 1 }, { name: "lo", ready: false, bytes: 1 }, { name: "pdfium", ready: true, bytes: 1 }] }),
  pdfium_status: () => "앱에 든 PDFium",
  update_view: () => ({ enabled: true, current: "0.3.1", banner: null }),
  update_check: () => ({ state: "latest" }),
  update_set_enabled: (a) => ({ enabled: !!a.enabled, current: "0.3.1", banner: null }),
  update_skip: () => ({ enabled: true, current: "0.3.1", banner: null }),
  update_open: () => null,
  app_info: () => ({ name: "파일 도구", version: "0.3.1", notices: "파일 도구 0.3.1 — 오픈소스 고지\n\n소스 받는 곳 (LGPL·MPL 부품)\n아래 부품의 소스 코드는 다음 주소에서 받을 수 있습니다: https://github.com/salarycrew/win-filetool-sources\n…", notices_path: "C:\\…\\THIRD-PARTY-NOTICES.txt", config_dir: "C:\\Users\\me\\AppData\\Roaming\\kr.salarycrew.winfiletool", config_source: "default", webview2: "141.0" }),
  watch_get: () => ({ rules: [{ id: "r1", folder: "C:\\Users\\me\\Pictures\\아이폰", op: { kind: "image", format: "jpg", quality: 90, max_side: null, keep_exif: true }, out_subfolder: "변환됨", enabled: true }, { id: "r2", folder: "C:\\Users\\me\\Videos\\받은 영상", op: { kind: "media", preset: "messenger", max_mb: 25, max_height: 720 }, out_subfolder: "변환됨", enabled: false }],
    log: [{ rule_id: "r1", input: "IMG_4101.HEIC", ok: true, output: "C:\\Users\\me\\Pictures\\아이폰\\변환됨\\IMG_4101.jpg", message: "" }, { rule_id: "r1", input: "IMG_4102.HEIC", ok: true, output: "C:\\Users\\me\\Pictures\\아이폰\\변환됨\\IMG_4102.jpg", message: "" }], paused: false }),
  watch_set_rules: (a) => a.rules,
  watch_set_paused: (a) => a.paused,
};

const PICK: Record<string, string[]> = {
  pdf: [`${D}\\계약서_최종.pdf`], pdfs: [`${D}\\계약서_최종.pdf`, `${D}\\부록.pdf`], docx: [`${D}\\2026 상반기 사업계획.docx`],
  photos: ["C:\\Users\\me\\Pictures\\IMG_4021.HEIC", "C:\\Users\\me\\Pictures\\IMG_4022.HEIC", "C:\\Users\\me\\Pictures\\IMG_4023.JPG"],
  mov: ["C:\\Users\\me\\Videos\\아이 생일.mov"], zip: ["C:\\Users\\me\\Downloads\\맥에서 보낸 자료.zip"], dir: [`${D}\\제주 여행`],
  mixed: [`${D}\\계약서_최종.pdf`, `${D}\\2026 상반기 사업계획.docx`, "C:\\Users\\me\\Pictures\\IMG_4021.HEIC", "C:\\Users\\me\\Videos\\아이 생일.mov"],
  locked: ["C:\\Users\\me\\Downloads\\급여명세서_10월.pdf"],
};
(window as unknown as { __FT_MOCK__: unknown }).__FT_MOCK__ = {
  async invoke(cmd: string, args: Args = {}) {
    const f = H[cmd];
    if (!f) { console.warn("mock: no", cmd, args); throw `명령이 없습니다: ${cmd}`; }
    await sleep(30);
    return f(args);
  },
  listen(ev: string, cb: (e: { payload: unknown }) => void) { listeners.set(ev, [...(listeners.get(ev) ?? []), cb]); return () => {}; },
  async open(o: { directory?: boolean; filters?: { extensions: string[] }[] }) {
    const k = (window as unknown as { __PICK__?: string }).__PICK__ ?? (o.directory ? "dir" : o.filters?.[0]?.extensions.includes("pdf") ? "pdf" : o.filters?.[0]?.extensions.includes("png") ? "sig" : "photos");
    if (k === "sig") return "C:\\Users\\me\\Pictures\\서명.png";
    return PICK[k] ?? null;
  },
  PICK,
};
// 화면 점검: ?drop=pdf&s=pdfedit&theme=dark
if (q.get("theme")) localStorage.setItem("ft.theme", q.get("theme")!);
if (q.has("welcomed")) localStorage.setItem("ft.welcomed", "1");
export {};

// 화면 점검용 순서 실행: ?do=nav:convert;drop:pdf;click:[data-pick=p2small];wait:300
(async () => {
  const steps = q.get("do");
  if (!steps) return;
  await sleep(400);
  for (const s of steps.split(";")) {
    const i = s.indexOf(":");
    const k = s.slice(0, i), v = s.slice(i + 1);
    if (k === "wait") await sleep(Number(v));
    else if (k === "drop") (window as unknown as { __FT_DROP__: (p: string[]) => void }).__FT_DROP__(PICK[v]);
    else if (k === "nav") (document.querySelector(`.nav-i[data-id="${v}"]`) as HTMLElement)?.click();
    else if (k === "click") (document.querySelector(v) as HTMLElement)?.click();
    else if (k === "type") { const [sel, val] = v.split("="); const el = document.querySelector(sel) as HTMLInputElement; el.value = val; el.dispatchEvent(new Event("input", { bubbles: true })); }
    else if (k === "pick") (window as unknown as { __PICK__: string }).__PICK__ = v;
    else if (k === "scroll") { const p = document.getElementById("pane")!; p.scrollTop = v === "end" ? p.scrollHeight : Number(v); }
    await sleep(120);
  }
})();
