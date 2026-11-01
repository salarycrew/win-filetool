// 감시 폴더: 지금 기능 그대로(규칙 켜기·지우기·추가, 잠시 멈춤, 최근 기록), 카드 모양으로
import { fileName, friendly, invoke, listen, pickFiles } from "../api";
import { FT, I, extOf } from "../icons";
import { Screen } from "../router";
import { $, bindSegs, empty, esc, opt, seg, segVal, toast, toggle, value } from "../ui";

type Op = { kind: "image"; format: string; quality: number; max_side: number | null; keep_exif: boolean }
  | { kind: "media"; preset: string; max_mb?: number; max_height?: number }
  | { kind: "doc"; target: string };
interface WatchRule { id: string; folder: string; op: Op; out_subfolder: string; enabled: boolean }
interface WatchEvent { rule_id: string; input: string; ok: boolean; output: string | null; message: string; at?: number }

let root: HTMLElement;
let rules: WatchRule[] = [];
let log: (WatchEvent & { t: string })[] = [];
let paused = false;
let newFolder: string | null = null;

const OPS: [string, string][] = [["image:jpg", "사진 → JPG (HEIC 포함)"], ["image:png", "사진 → PNG"], ["media:mp4", "영상 → MP4"], ["media:messenger", "영상 → 메신저용"], ["media:mp3", "→ MP3 (소리만)"], ["doc:pdf", "문서 → PDF"]];
function opFrom(spec: string): Op {
  const [k, v] = spec.split(":");
  if (k === "image") return { kind: "image", format: v, quality: 90, max_side: null, keep_exif: true };
  if (k === "media") return v === "messenger" ? { kind: "media", preset: "messenger", max_mb: 25, max_height: 720 } : { kind: "media", preset: v };
  return { kind: "doc", target: "pdf" };
}
function opLabel(op: Op): string {
  if (op.kind === "image") return `사진 → ${op.format.toUpperCase()}`;
  if (op.kind === "doc") return `문서 → ${op.target.toUpperCase()}`;
  return ({ mp4: "영상 → MP4", messenger: "영상 → 메신저용", mp3: "→ MP3", m4a: "→ M4A", gif: "→ GIF" } as Record<string, string>)[op.preset] ?? op.preset;
}
const nowT = () => { const d = new Date(); return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`; };

function render() {
  root.innerHTML = `<h1 class="title">감시 폴더</h1><p class="sub">정해 둔 폴더에 파일이 들어오면 저절로 바꿔요. 창을 닫아도 알림 영역에서 계속 돌아요.</p>
    <div class="sc"><span class="sc-ic">${I("power")}</span><div class="sc-t"><b>감시하기</b><span>${paused ? "잠시 멈췄어요" : rules.some((r) => r.enabled) ? "켜진 규칙대로 지켜보고 있어요" : "켜진 규칙이 없어요"}</span></div>${toggle("w-on", !paused, paused ? "멈춤" : "켬")}</div>
    <h2 class="h2">규칙</h2>
    <div class="rules">${rules.length ? rules.map((r, i) => `<div class="sc rule ${r.enabled ? "" : "off"}">${FT("folder", 28)}<div class="sc-t"><b title="${esc(r.folder)}">${esc(r.folder)}</b><span>${esc(opLabel(r.op))} · 결과는 안쪽 “${esc(r.out_subfolder)}” 폴더에</span></div>
        ${toggle(`r-on-${i}`, r.enabled, r.enabled ? "켬" : "끔")}<button type="button" class="ib" data-del="${i}" title="규칙 지우기" aria-label="규칙 지우기">${I("trash")}</button></div>`).join("")
      : `<div class="panel">${empty("watch", "아직 규칙이 없어요", "아래에서 폴더와 할 일을 골라 규칙을 더하세요.")}</div>`}</div>
    <div class="optcard add-rule"><div class="oc-body"><div class="oc-h">${I("plus")}규칙 더하기</div>
      ${opt("폴더", `<button type="button" class="btn" data-act="w-folder">${I("folder")}폴더 고르기</button><span class="path hint">${esc(newFolder ?? "")}</span>`)}
      ${opt("할 일", `<select class="txt" id="w-op">${OPS.map(([v, l]) => `<option value="${v}">${esc(l)}</option>`).join("")}</select>`)}
      ${opt("결과 폴더", `<input class="txt" id="w-sub" value="변환됨">`, "감시 폴더 안에 만들어요")}
    </div><div class="oc-foot"><span class="grow"></span><button type="button" class="btn accent" data-act="w-add">규칙 더하기</button></div></div>
    <h2 class="h2">최근 기록</h2>
    <div class="log">${log.length ? log.slice(-30).reverse().map((e) => `<div class="${e.ok ? "" : "bad"}"><time>${e.t}</time><span>${e.ok ? `${esc(fileName(e.input))} → ${esc(fileName(e.output ?? ""))}` : `${esc(fileName(e.input))}: ${esc(friendly(e.message).title)}`}</span>
        ${e.ok && e.output ? `<button type="button" class="link" data-reveal="${esc(e.output)}">폴더 열기</button>` : e.ok ? "" : `<details class="more"><summary>자세히</summary><pre>${esc(e.message)}</pre></details>`}</div>`).join("") : `<div class="hint pad">아직 기록이 없어요</div>`}</div>`;
  bindSegs(root);
}

async function load() {
  try {
    const v = await invoke<{ rules: WatchRule[]; log: WatchEvent[]; paused: boolean }>("watch_get");
    rules = v.rules; paused = v.paused;
    log = v.log.map((e) => ({ ...e, t: "" }));
  } catch (e) { toast(friendly(e).title); }
  render();
}
async function save(next: WatchRule[]) {
  try { rules = await invoke<WatchRule[]>("watch_set_rules", { rules: next }); } catch (e) { toast(`저장하지 못했어요 — ${friendly(e).title}`); }
  render();
}

export const watchScreen: Screen = {
  id: "watch", title: "감시 폴더", icon: "watch",
  init(el) {
    root = el;
    listen<WatchEvent>("watch-event", (e) => { log.push({ ...e, t: nowT() }); if (!root.closest("section")?.hidden) render(); });
    listen<boolean>("watch-paused", (p) => { paused = p; if (!root.closest("section")?.hidden) render(); });
    root.addEventListener("change", (e) => {
      const t = e.target as HTMLInputElement;
      if (t.id === "w-on") { paused = !t.checked; invoke("watch_set_paused", { paused }).catch(() => {}); render(); }
      else if (t.id.startsWith("r-on-")) { const i = Number(t.id.slice(5)); save(rules.map((r, j) => (j === i ? { ...r, enabled: t.checked } : r))); }
    });
    root.addEventListener("click", async (e) => {
      const t = e.target as HTMLElement;
      const del = t.closest<HTMLElement>("[data-del]");
      if (del) { save(rules.filter((_, j) => j !== Number(del.dataset.del))); return; }
      const rv = t.closest<HTMLElement>("[data-reveal]");
      if (rv) { invoke("open_folder", { path: rv.dataset.reveal }).catch(() => {}); return; }
      const b = t.closest<HTMLElement>("[data-act]");
      if (b?.dataset.act === "w-folder") { const [d] = await pickFiles({ directory: true, multiple: false }); if (d) { newFolder = d; const p = $(".add-rule .path", root); if (p) p.textContent = d; } }
      if (b?.dataset.act === "w-add") {
        if (!newFolder) { toast("감시할 폴더를 먼저 골라 주세요"); return; }
        const op = ($<HTMLSelectElement>("#w-op", root)!).value;
        await save([...rules, { id: `r${Date.now().toString(36)}`, folder: newFolder, op: opFrom(op), out_subfolder: value(root, "w-sub") || "변환됨", enabled: true }]);
        newFolder = null;
        toast("규칙을 더했어요");
      }
    });
    void segVal; void seg; void extOf;
  },
  show() { load(); },
};
