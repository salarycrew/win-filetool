// 0.3.0 아이콘: 인라인 SVG 한 벌(24×24, 선 1.6, 둥근 끝, currentColor). 시안 A(design/src/common.js)에서 옮기고 더함.
export const P: Record<string, string> = {
  convert: '<path d="M7 4 3 8l4 4M3 8h14M17 20l4-4-4-4M21 16H7"/>',
  search: '<circle cx="11" cy="11" r="6.5"/><path d="m16 16 4.5 4.5"/>',
  watch: '<path d="M3 7.5A1.5 1.5 0 0 1 4.5 6H9l2 2h8.5A1.5 1.5 0 0 1 21 9.5v8a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5z"/><circle cx="15" cy="13.5" r="3"/><path d="M15 12.2v1.4l.9.7"/>',
  settings: '<path d="M4 7h9M17 7h3M4 17h3M11 17h9"/><circle cx="15" cy="7" r="2"/><circle cx="9" cy="17" r="2"/>',
  folder: '<path d="M3 7.5A1.5 1.5 0 0 1 4.5 6H9l2 2h8.5A1.5 1.5 0 0 1 21 9.5v8a1.5 1.5 0 0 1-1.5 1.5h-15A1.5 1.5 0 0 1 3 17.5z"/>',
  file: '<path d="M7 3h7l5 5v12a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z"/><path d="M14 3v5h5"/>',
  doc: '<path d="M7 3h7l5 5v12a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z"/><path d="M14 3v5h5M9.5 12.5h5M9.5 15.5h5M9.5 18.5h3"/>',
  pdf: '<path d="M7 3h7l5 5v12a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z"/><path d="M14 3v5h5"/><path d="M9 17.5c1.5-1 3-4 3-6.5 0-1-.8-1-1 0-.3 2 2.5 5 5 5.5 1 .2 1-.8 0-1-2-.3-5.5.5-7 2z"/>',
  slides: '<rect x="3" y="4" width="18" height="12.5" rx="1.5"/><path d="M12 16.5V20M8.5 20h7M7 8.5h6M7 11.5h4"/>',
  image: '<rect x="3.5" y="5" width="17" height="14" rx="2"/><circle cx="9" cy="10" r="1.6"/><path d="m20.5 16-5-5-8 8"/>',
  images: '<rect x="6.5" y="3.5" width="14" height="12" rx="1.8"/><path d="M3.5 7.5v10a2 2 0 0 0 2 2h11"/><circle cx="11" cy="7.8" r="1.3"/><path d="m20.5 13-4-4-6.5 6.5"/>',
  pages: '<path d="M9 3.5h8.5a1 1 0 0 1 1 1V17"/><rect x="5" y="6.5" width="10.5" height="14" rx="1"/><path d="M7.8 11h5M7.8 14h5"/>',
  compress: '<path d="M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5"/>',
  layers: '<path d="m12 3.5-8.5 4.5 8.5 4.5 8.5-4.5z"/><path d="m3.5 12.5 8.5 4.5 8.5-4.5"/>',
  resize: '<path d="M4 9V4h5M20 15v5h-5M4 4l6 6M20 20l-6-6"/>',
  video: '<rect x="3" y="6" width="13" height="12" rx="2"/><path d="m16 10.5 5-3v9l-5-3"/>',
  chat: '<path d="M4.5 5h15a1 1 0 0 1 1 1v9.5a1 1 0 0 1-1 1H10l-4.5 3.5v-3.5h-1a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1z"/>',
  music: '<path d="M9 18V5.5l11-2V16"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="17.5" cy="16" r="2.5"/>',
  loop: '<circle cx="12" cy="12" r="8.5"/><path d="m10 8.8 5 3.2-5 3.2z"/>',
  upload: '<path d="M12 15V4M7.5 8.5 12 4l4.5 4.5M4 14v4.5A1.5 1.5 0 0 0 5.5 20h13a1.5 1.5 0 0 0 1.5-1.5V14"/>',
  check: '<path d="m5 12.5 4.5 4.5L19 7.5"/>',
  x: '<path d="M6 6l12 12M18 6 6 18"/>',
  lock: '<rect x="5" y="10.5" width="14" height="10" rx="2"/><path d="M8 10.5V8a4 4 0 0 1 8 0v2.5M12 14.5v2.5"/>',
  sun: '<circle cx="12" cy="12" r="4"/><path d="M12 2.5v2M12 19.5v2M4.6 4.6 6 6M18 18l1.4 1.4M2.5 12h2M19.5 12h2M4.6 19.4 6 18M18 6l1.4-1.4"/>',
  moon: '<path d="M20 14.5A8 8 0 0 1 9.5 4a8 8 0 1 0 10.5 10.5z"/>',
  open: '<path d="M14 4h6v6M20 4l-8 8M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/>',
  down: '<path d="m6 9 6 6 6-6"/>',
  right: '<path d="m9 6 6 6-6 6"/>',
  info: '<circle cx="12" cy="12" r="8.5"/><path d="M12 11v5M12 8h.01"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  refresh: '<path d="M19.5 11a7.5 7.5 0 1 0-2.2 5.3M19.5 4.5V11H13"/>',
  shield: '<path d="M12 3 5 6v5c0 4.5 3 8 7 10 4-2 7-5.5 7-10V6z"/><path d="m9 12 2 2 4-4"/>',
  power: '<path d="M12 3v8M7.2 6.3a7 7 0 1 0 9.6 0"/>',
  contrast: '<circle cx="12" cy="12" r="8.5"/><path d="M12 3.5a8.5 8.5 0 0 1 0 17z" fill="currentColor"/>',
  keyboard: '<rect x="3" y="6.5" width="18" height="11" rx="2"/><path d="M7 10.5h.01M10.5 10.5h.01M14 10.5h.01M17 10.5h.01M8 14h8"/>',
  tool: '<path d="M14.5 5.5a4 4 0 0 0-5 5L4 16v4h4l5.5-5.5a4 4 0 0 0 5-5l-2.5 2.5-2.5-.5-.5-2.5z"/>',
  sort: '<path d="M8 4v16M4.5 7.5 8 4l3.5 3.5M16 20V4M12.5 16.5 16 20l3.5-3.5"/>',
  min: '<path d="M6 12h12"/>', max: '<rect x="6.5" y="6.5" width="11" height="11" rx="1"/>',
  pencil: '<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="m13.5 6.5 4 4"/>',
  trash: '<path d="M4.5 7h15M10 4h4M6.5 7l1 12.5a1 1 0 0 0 1 .9h7a1 1 0 0 0 1-.9l1-12.5M10 11v6M14 11v6"/>',
  undo: '<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11"/>',
  redo: '<path d="m15 14 5-5-5-5"/><path d="M20 9H9.5a5.5 5.5 0 0 0 0 11H13"/>',
  rotl: '<path d="M4 4v5h5"/><path d="M4.6 9A8 8 0 1 1 6 16.5"/>',
  rotr: '<path d="M20 4v5h-5"/><path d="M19.4 9A8 8 0 1 0 18 16.5"/>',
  cut: '<circle cx="6" cy="7" r="2.5"/><circle cx="6" cy="17" r="2.5"/><path d="M8.2 8.3 20 18M8.2 15.7 20 6"/>',
  mute: '<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z"/><path d="m16 9.5 5 5M21 9.5l-5 5"/>',
  sign: '<path d="M3 17c3-1 4-7 6-7s-1 6 1 6 3-4 4-4 1 3 3 3 3-1 4-2"/><path d="M3 21h18"/>',
  text: '<path d="M5 6V4.5h14V6M12 4.5v15M9 19.5h6"/>',
  hash: '<path d="M9 4 7 20M17 4l-2 16M4.5 9h16M3.5 15h16"/>',
  drop: '<path d="M12 3.5s6 6.4 6 10.5a6 6 0 0 1-12 0c0-4.1 6-10.5 6-10.5z"/>',
  unlock: '<rect x="5" y="10.5" width="14" height="10" rx="2"/><path d="M8 10.5V8a4 4 0 0 1 7.7-1.5M12 14.5v2.5"/>',
  crop: '<path d="M6 2.5V16a2 2 0 0 0 2 2h13.5"/><path d="M2.5 6H16a2 2 0 0 1 2 2v13.5"/>',
  a4: '<rect x="5.5" y="3" width="13" height="18" rx="1"/><path d="M8.5 15l2-5 2 5M9.1 13.5h2.8M15.5 10v5M14 13.5h2.5"/>',
  home: '<path d="M4 10.5 12 4l8 6.5V19a1 1 0 0 1-1 1h-4.5v-5.5h-5V20H5a1 1 0 0 1-1-1z"/>',
  more: '<circle cx="6" cy="12" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="18" cy="12" r="1.6"/>',
  grid: '<rect x="3.5" y="3.5" width="7" height="7" rx="1"/><rect x="13.5" y="3.5" width="7" height="7" rx="1"/><rect x="3.5" y="13.5" width="7" height="7" rx="1"/><rect x="13.5" y="13.5" width="7" height="7" rx="1"/>',
  form: '<rect x="4" y="3.5" width="16" height="17" rx="1.5"/><path d="M7.5 8h9M7.5 12h9M7.5 16h5"/>',
  flatten: '<path d="M4 8.5h16M4 12h16M4 15.5h16"/><path d="m9 4 3 2.5L15 4M9 20l3-2.5 3 2.5"/>',
  zip: '<path d="M7 3h7l5 5v12a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z"/><path d="M11 3v2h1.5M11 6.5h1.5v1.5H11M11 9.5h1.5v1.5H11M10.5 13h3v3.5h-3z"/>',
  unzip: '<path d="M4 7.5A1.5 1.5 0 0 1 5.5 6H9l2 2h7.5A1.5 1.5 0 0 1 20 9.5v8a1.5 1.5 0 0 1-1.5 1.5h-13A1.5 1.5 0 0 1 4 17.5z"/><path d="M12 10.5v6M9.5 14l2.5 2.5 2.5-2.5"/>',
  rename: '<path d="M3.5 7.5h11M3.5 12h7M3.5 16.5h6"/><path d="M13 19h3l5-5-3-3-5 5z"/>',
  copy: '<rect x="8.5" y="8.5" width="12" height="12" rx="1.5"/><path d="M15.5 8.5V5a1.5 1.5 0 0 0-1.5-1.5H5A1.5 1.5 0 0 0 3.5 5v9A1.5 1.5 0 0 0 5 15.5h3.5"/>',
  hangul: '<path d="M4 5.5h6M7 5.5v2.5M4.5 11c1.5-.5 3.5-2 3.5-3M14 4.5v8M14 8.5h3M5 15h9v4.5"/><path d="m16.5 15.5 2 2 3-3.5"/>',
  film: '<rect x="3.5" y="4.5" width="17" height="15" rx="1.5"/><path d="M7.5 4.5v15M16.5 4.5v15M3.5 9h4M3.5 15h4M16.5 9h4M16.5 15h4"/>',
  minus: '<path d="M5 12h14"/>',
  up: '<path d="m6 15 6-6 6 6"/>',
  left: '<path d="m15 6-6 6 6 6"/>',
  back: '<path d="M19 12H5M11 6l-6 6 6 6"/>',
  extract: '<path d="M7 3h7l5 5v12a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z"/><path d="M12 10v7M9 14l3 3 3-3"/>',
  insert: '<rect x="4" y="3.5" width="7" height="9.5" rx="1"/><rect x="13" y="3.5" width="7" height="9.5" rx="1"/><path d="M12 15.5v5M9.5 18h5"/>',
  blank: '<path d="M7 3h7l5 5v12a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z"/><path d="M14 3v5h5M12 11v6M9 14h6"/>',
  merge: '<path d="M6 4v5a3 3 0 0 0 3 3h6a3 3 0 0 1 3 3v5M18 4v5a3 3 0 0 1-3 3"/><path d="m15 17 3 3 3-3"/>',
  eye: '<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="3"/>',
  pin: '<path d="M12 21s-6.5-5.6-6.5-11a6.5 6.5 0 0 1 13 0c0 5.4-6.5 11-6.5 11z"/><circle cx="12" cy="10" r="2.3"/>',
  ruler: '<rect x="2.5" y="7.5" width="19" height="9" rx="1.5"/><path d="M6.5 7.5v3M10 7.5v4.5M13.5 7.5v3M17 7.5v4.5"/>',
  flip: '<path d="M12 3v18M9 6 3.5 18H9zM15 6l5.5 12H15z"/>',
  warn: '<path d="M12 4 2.5 20h19z"/><path d="M12 10v4.5M12 17.5h.01"/>',
  heart: '<path d="M12 20s-7.5-4.6-7.5-10A4.3 4.3 0 0 1 12 7.3 4.3 4.3 0 0 1 19.5 10c0 5.4-7.5 10-7.5 10z"/>',
  bolt: '<path d="M13 3 5 13.5h6L10 21l8-10.5h-6z"/>',
  scene: '<rect x="3.5" y="5" width="17" height="14" rx="2"/><circle cx="12" cy="12" r="3.2"/><path d="M8 5l1.2-2h5.6L16 5"/>',
};

export function I(n: string, cls = ""): string {
  return `<svg class="ic ${cls}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${P[n] ?? ""}</svg>`;
}

// 파일 형식 아이콘: 접힌 모서리 문서 + 색 꼬리표(그림 파일 없이)
const FTC: Record<string, string> = {
  pdf: "#D13438", docx: "#2B579A", doc: "#2B579A", hwp: "#1F86C9", hwpx: "#1F86C9", odt: "#2B579A", pages: "#F09A37", rtf: "#2B579A", txt: "#6B7280",
  pptx: "#C43E1C", ppt: "#C43E1C", key: "#1E7FD8", odp: "#C43E1C", xlsx: "#217346", xls: "#217346", csv: "#217346", numbers: "#2EA44F", ods: "#217346",
  jpg: "#0E8A5F", jpeg: "#0E8A5F", heic: "#0F7B8C", heif: "#0F7B8C", png: "#5865C8", webp: "#3F7FBF", gif: "#A0522D", bmp: "#5865C8", tif: "#5865C8", tiff: "#5865C8",
  mp4: "#7A3EB1", mov: "#7A3EB1", mkv: "#7A3EB1", avi: "#7A3EB1", webm: "#7A3EB1", m4v: "#7A3EB1", "3gp": "#7A3EB1", wmv: "#7A3EB1",
  mp3: "#B4467F", m4a: "#B4467F", wav: "#B4467F", flac: "#B4467F", aac: "#B4467F", ogg: "#B4467F", zip: "#B07B00",
};
export function FT(ext: string, size = 32): string {
  ext = (ext || "").toLowerCase();
  if (ext === "folder" || ext === "dir")
    return `<svg class="ft" width="${size}" height="${size}" viewBox="0 0 32 32" aria-hidden="true"><path d="M3 9a2 2 0 0 1 2-2h7l3 3h12a2 2 0 0 1 2 2v13a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" fill="#E8B53A"/><path d="M3 13h26v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" fill="#F5CB5C"/></svg>`;
  const c = FTC[ext] ?? "#6B7280";
  const label = (ext || "?").toUpperCase().slice(0, 4).replace(/[^A-Z0-9?]/g, "");
  const wide = label.length > 3;
  return `<svg class="ft" width="${size}" height="${size}" viewBox="0 0 32 32" aria-hidden="true"><path d="M7 2h12l7 7v19a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z" fill="var(--ft-page,#fff)" stroke="var(--ft-edge,#C9CDD3)"/><path d="M19 2v5a2 2 0 0 0 2 2h5" fill="var(--ft-fold,#EEF0F3)" stroke="var(--ft-edge,#C9CDD3)"/><rect x="2" y="16" width="${wide ? 22 : 18}" height="10" rx="2" fill="${c}"/><text x="${wide ? 13 : 11}" y="23.6" text-anchor="middle" font-size="${wide ? 6.4 : 7.2}" font-weight="700" fill="#fff" font-family="Segoe UI, Arial, sans-serif">${label}</text></svg>`;
}

/** 경로의 확장자(소문자, 없으면 "") */
export function extOf(p: string): string {
  const n = p.split(/[\\/]/).pop() ?? p;
  const i = n.lastIndexOf(".");
  return i > 0 ? n.slice(i + 1).toLowerCase() : "";
}
