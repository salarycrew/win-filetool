// 화면 공용 조각: 분할 단추·체크·토글·알림·대화상자·결과 줄·오류 판
import { FT, I, extOf } from "./icons";
import { Friendly, dirOf, fileName, human, openDir, openFile, openFolderSelect, friendly } from "./api";

export const esc = (s: unknown) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
export const $ = <T extends HTMLElement = HTMLElement>(sel: string, root: ParentNode = document) => root.querySelector(sel) as T | null;
export const $$ = <T extends HTMLElement = HTMLElement>(sel: string, root: ParentNode = document) => [...root.querySelectorAll(sel)] as T[];

export function el(html: string): HTMLElement {
  const t = document.createElement("template");
  t.innerHTML = html.trim();
  return t.content.firstElementChild as HTMLElement;
}

// ---------- 입력 조각 (마크업 + 값 읽기) ----------

/** 분할 단추(라디오 묶음). items: [값, 글] */
export function seg(name: string, items: [string, string][], value: string, label = ""): string {
  return `<div class="seg" role="radiogroup" data-seg="${esc(name)}" ${label ? `aria-label="${esc(label)}"` : ""}>${items
    .map(([v, t]) => `<button type="button" role="radio" aria-checked="${v === value}" class="${v === value ? "on" : ""}" data-v="${esc(v)}">${esc(t)}</button>`)
    .join("")}</div>`;
}
/** 분할 단추 값을 코드로 바꾸기(보이는 선택·aria-checked 함께) */
export function setSeg(root: ParentNode, name: string, v: string) {
  $$<HTMLButtonElement>(`[data-seg="${name}"] > button`, root).forEach((x) => { x.classList.toggle("on", x.dataset.v === v); x.setAttribute("aria-checked", String(x.dataset.v === v)); });
}
export function segVal(root: ParentNode, name: string): string {
  return $(`[data-seg="${name}"] button.on`, root)?.dataset.v ?? "";
}
/** 분할 단추 누름 처리(한 번만 걸면 됨).
 * 캡처 단계에서 선택을 바꾼다: 안쪽 요소(도구 설정 판 등)에 건 click 처리기가 segVal()을 읽을 때
 * 이미 새 값이 보이게(0.3.0 QA-02 — 한 번 늦게 적용되던 문제). */
export function bindSegs(root: HTMLElement, onChange?: (name: string, v: string) => void) {
  root.addEventListener("click", (e) => {
    const b = (e.target as HTMLElement).closest<HTMLButtonElement>("[data-seg] > button");
    if (!b || b.disabled) return;
    const g = b.parentElement!;
    $$("button", g).forEach((x) => { x.classList.toggle("on", x === b); x.setAttribute("aria-checked", String(x === b)); });
    onChange?.(g.dataset.seg!, b.dataset.v!);
  }, true);
  root.addEventListener("keydown", (e) => {
    const b = (e.target as HTMLElement).closest<HTMLButtonElement>("[data-seg] > button");
    if (!b || (e.key !== "ArrowLeft" && e.key !== "ArrowRight")) return;
    const sib = (e.key === "ArrowRight" ? b.nextElementSibling : b.previousElementSibling) as HTMLButtonElement | null;
    if (sib) { sib.focus(); sib.click(); e.preventDefault(); }
  });
}

export function chk(id: string, label: string, on: boolean, extra = ""): string {
  return `<label class="chk"><input type="checkbox" id="${esc(id)}" ${on ? "checked" : ""} ${extra}><span class="box">${I("check")}</span><span>${label}</span></label>`;
}
export function toggle(id: string, on: boolean, label = ""): string {
  return `<label class="tg"><span class="tg-l">${esc(label)}</span><input type="checkbox" role="switch" id="${esc(id)}" ${on ? "checked" : ""}><i></i></label>`;
}
export function opt(label: string, control: string, help = ""): string {
  // 입력 칸에 이름 붙이기(화면 읽기·자동화): 이름이 없는 첫 입력에 aria-label
  const plain = label.replace(/<[^>]*>/g, "").trim();
  if (plain) control = control.replace(/<(input|select|textarea)(?![^>]*aria-label)(?![^>]*type="checkbox")/, `<$1 aria-label="${esc(plain)}"`);
  return `<div class="opt"><span class="opt-l">${label}</span><div class="opt-c">${control}${help ? `<span class="opt-help">${help}</span>` : ""}</div></div>`;
}
export const checked = (root: ParentNode, id: string) => ($<HTMLInputElement>(`#${id}`, root)?.checked ?? false);
export const value = (root: ParentNode, id: string) => ($<HTMLInputElement>(`#${id}`, root)?.value.trim() ?? "");

// ---------- 말풍선(아이콘만 보이는 단추의 이름) ----------

/** 글자가 숨은 단추(아이콘만 보이는 도구 막대·좁은 창 메뉴·탭)에 마우스를 올리거나 키보드로 오면 이름을 보여 준다.
 * title은 data-tip으로 옮겨 Windows 기본 말풍선과 겹치지 않게 하고, 화면 읽기용 이름(aria-label)은 남긴다. */
export function installTips() {
  const tip = document.createElement("div");
  tip.className = "tip";
  tip.id = "ft-tip";
  tip.setAttribute("role", "tooltip");
  tip.hidden = true;
  document.body.appendChild(tip);
  let cur: HTMLElement | null = null;
  let timer = 0;
  const SEL = "button, [role=tab], [role=button], a[href], [data-tip]";
  const textOf = (t: HTMLElement): string => {
    if (t.title) {
      if (!t.dataset.tip) t.dataset.tip = t.title;
      if (!t.getAttribute("aria-label") && !(t.innerText || "").trim()) t.setAttribute("aria-label", t.title);
      t.removeAttribute("title");
    }
    const label = t.dataset.tip || t.getAttribute("aria-label") || "";
    const shown = (t.innerText || "").replace(/\s+/g, " ").trim();
    // 이름이 이미 다 보이면 말풍선 없음
    return label && label !== shown ? label : "";
  };
  const hide = () => { window.clearTimeout(timer); if (cur) cur.removeAttribute("aria-describedby"); cur = null; tip.hidden = true; };
  const show = (t: HTMLElement) => {
    const text = textOf(t);
    if (!text || (t as HTMLButtonElement).disabled && !t.matches(":hover")) { hide(); return; }
    cur = t;
    tip.textContent = text;
    tip.hidden = false;
    if (t.getAttribute("aria-label") !== text) t.setAttribute("aria-describedby", tip.id);
    const r = t.getBoundingClientRect(), w = tip.offsetWidth, h = tip.offsetHeight;
    const nav = !!t.closest(".nav");
    let x = nav ? r.right + 6 : r.left + r.width / 2 - w / 2;
    let y = nav ? r.top + r.height / 2 - h / 2 : r.bottom + 6;
    if (!nav && y + h > window.innerHeight - 4) y = r.top - h - 6;
    x = Math.max(4, Math.min(x, window.innerWidth - w - 4));
    y = Math.max(4, y);
    tip.style.left = `${Math.round(x)}px`;
    tip.style.top = `${Math.round(y)}px`;
  };
  document.addEventListener("pointerover", (e) => {
    const t = (e.target as HTMLElement).closest<HTMLElement>(SEL);
    if (t === cur) return;
    hide();
    if (!t) return;
    textOf(t); // title을 먼저 옮겨 기본 말풍선이 뜨지 않게
    timer = window.setTimeout(() => show(t), 350);
  });
  document.addEventListener("pointerout", (e) => {
    const to = (e as PointerEvent).relatedTarget as HTMLElement | null;
    if (cur && to && cur.contains(to)) return;
    if (!to || !to.closest(SEL)) hide();
  });
  document.addEventListener("focusin", (e) => {
    const t = (e.target as HTMLElement).closest<HTMLElement>(SEL);
    hide();
    if (t && t.matches(":focus-visible")) show(t);
  });
  document.addEventListener("focusout", hide);
  document.addEventListener("pointerdown", hide, true);
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") hide(); });
  window.addEventListener("scroll", hide, true);
  window.addEventListener("resize", hide);
}

// ---------- 알림 ----------

let toastTimer = 0;
export function toast(msg: string, ms = 2600) {
  let t = document.getElementById("toast");
  if (!t) { t = el(`<div id="toast" role="status" aria-live="polite"></div>`); document.body.appendChild(t); }
  t.textContent = msg;
  t.classList.add("show");
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => t!.classList.remove("show"), ms);
}

// ---------- 대화상자 ----------

interface DlgOpts { title: string; body?: string; html?: string; ok?: string; cancel?: string | null; danger?: boolean; icon?: string; onOpen?: (root: HTMLElement) => void; validate?: (root: HTMLElement) => string | null }
/** 가운데 작은 대화상자. 확인하면 대화상자 요소, 취소하면 null */
export function dialog(o: DlgOpts): Promise<HTMLElement | null> {
  return new Promise((resolve) => {
    const prev = document.activeElement as HTMLElement | null;
    const back = el(`<div class="dlg-back" role="presentation"><div class="dlg" role="dialog" aria-modal="true" aria-labelledby="dlg-t">
      <div class="dlg-b">${o.icon ? `<span class="dlg-ic">${I(o.icon)}</span>` : ""}<div class="dlg-m"><h2 id="dlg-t">${esc(o.title)}</h2>${o.body ? `<p>${esc(o.body)}</p>` : ""}${o.html ?? ""}<p class="dlg-err" hidden></p></div></div>
      <div class="dlg-f"><button type="button" class="btn ${o.danger ? "danger" : "accent"}" data-ok>${esc(o.ok ?? "확인")}</button>${o.cancel === null ? "" : `<button type="button" class="btn" data-cancel>${esc(o.cancel ?? "취소")}</button>`}</div></div></div>`);
    document.body.appendChild(back);
    const box = $(".dlg", back)!;
    const close = (ok: boolean) => {
      if (ok && o.validate) {
        const err = o.validate(box);
        if (err) { const p = $(".dlg-err", box)!; p.hidden = false; p.textContent = err; return; }
      }
      back.remove();
      document.removeEventListener("keydown", key, true);
      prev?.focus?.();
      resolve(ok ? box : null);
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") { e.preventDefault(); close(false); }
      else if (e.key === "Enter" && !(e.target instanceof HTMLTextAreaElement) && !(e.target instanceof HTMLButtonElement)) { e.preventDefault(); close(true); }
      else if (e.key === "Tab") {
        const f = $$<HTMLElement>("button, input, select, textarea, [tabindex='0']", box).filter((x) => !x.hidden && !(x as HTMLButtonElement).disabled);
        if (!f.length) return;
        const i = f.indexOf(document.activeElement as HTMLElement);
        if (e.shiftKey && i <= 0) { f[f.length - 1].focus(); e.preventDefault(); }
        else if (!e.shiftKey && i === f.length - 1) { f[0].focus(); e.preventDefault(); }
      }
    };
    document.addEventListener("keydown", key, true);
    $("[data-ok]", box)!.addEventListener("click", () => close(true));
    $("[data-cancel]", box)?.addEventListener("click", () => close(false));
    bindSegs(box);
    o.onOpen?.(box);
    ($<HTMLInputElement>("input", box) ?? $<HTMLButtonElement>("[data-ok]", box))!.focus();
  });
}

/** 암호 칸. 틀렸으면 wrong=true로 다시 부름. 취소하면 null */
export async function askPassword(name: string, wrong = false): Promise<string | null> {
  const box = await dialog({
    title: wrong ? "암호가 맞지 않아요" : "암호가 걸린 PDF예요",
    icon: "lock",
    html: `<p>${wrong ? "다시 넣어 주세요. 대문자·소문자와 한/영 상태를 확인해 주세요." : `<b>${esc(name)}</b>을(를) 열려면 암호가 필요해요.`}</p>
      <label class="fld"><span>암호</span><input type="password" class="txt wide" id="dlg-pw" autocomplete="off"></label>
      <p class="hint">암호는 이 PC 안에서만 쓰고 저장하지 않아요. 결과 파일은 암호 없이 저장돼요.</p>`,
    ok: "열기",
    validate: (b) => ($<HTMLInputElement>("#dlg-pw", b)!.value ? null : "암호를 넣어 주세요"),
  });
  return box ? $<HTMLInputElement>("#dlg-pw", box)!.value : null;
}

export async function confirmDlg(title: string, body: string, ok: string, danger = false): Promise<boolean> {
  return !!(await dialog({ title, body, ok, danger, icon: danger ? "warn" : "info" }));
}

// ---------- 결과 줄 · 오류 판 ----------

export interface ResultLine {
  output: string;          // 파일 또는 폴더
  isFolder?: boolean;
  before?: number | null;  // 바이트
  after?: number | null;
  pill?: string;           // "88% 작아졌어요" / "슬라이드 9장"
  note?: string;
  input?: string;
}
export function sizePill(before?: number | null, after?: number | null): string {
  if (!before || !after) return "";
  if (after < before) {
    const p = Math.round((1 - after / before) * 100);
    return p >= 1 ? `${p}% 작아졌어요` : "";
  }
  if (after > before * 1.05) return `${(after / before).toFixed(after / before < 10 ? 1 : 0)}배 커졌어요`;
  return "";
}
export function resultRow(r: ResultLine): string {
  const name = fileName(r.output);
  const ext = r.isFolder ? "folder" : extOf(r.output);
  const sz = r.before && r.after ? `${human(r.before)} → <b>${human(r.after)}</b>` : r.after ? `<b>${human(r.after)}</b>` : "";
  const pill = r.pill ?? sizePill(r.before, r.after);
  const pillCls = /커졌|그대로|0장/.test(pill) ? "pill gray" : "pill";
  return `<div class="rr" data-out="${esc(r.output)}" data-folder="${r.isFolder ? 1 : 0}">${FT(ext, 32)}
    <div class="rr-n"><b title="${esc(r.output)}">${esc(name)}</b><span>${esc(r.note ? r.note : dirOf(r.output))}</span></div>
    <div class="sz">${sz}${pill ? `<span class="${pillCls}">${esc(pill)}</span>` : ""}</div>
    <div class="rr-b">${r.isFolder ? "" : `<button type="button" class="btn accent" data-open="file">${I("open")}파일 열기</button>`}
    <button type="button" class="btn ${r.isFolder ? "accent" : ""}" data-open="folder">${I("folder")}폴더 열기</button></div></div>`;
}
export function errRow(input: string, e: unknown): string {
  const f = friendly(e);
  return `<div class="rr err">${FT(extOf(input), 32)}<div class="rr-n"><b>${esc(fileName(input))}</b><span class="e">${esc(f.title)} — ${esc(f.body)}</span></div>
    <details class="more"><summary>자세히</summary><pre>${esc(f.detail)}</pre></details></div>`;
}
/** 결과 줄의 [파일 열기]/[폴더 열기] 처리(한 번만 걸면 됨) */
export function bindResultButtons(root: HTMLElement) {
  root.addEventListener("click", async (e) => {
    const b = (e.target as HTMLElement).closest<HTMLButtonElement>("[data-open]");
    if (!b) return;
    const row = b.closest<HTMLElement>("[data-out]")!;
    const out = row.dataset.out!, folder = row.dataset.folder === "1";
    try {
      if (b.dataset.open === "file") await openFile(out);
      else if (folder) await openDir(out);
      else await openFolderSelect(out);
    } catch (err) {
      toast(friendly(err).title);
    }
  });
}

export function errorPanel(f: Friendly, buttons = ""): string {
  return `<div class="panel errp" role="alert"><div class="err-h"><span class="errc">${I(f.icon)}</span><div class="err-m"><b>${esc(f.title)}</b><p>${esc(f.body)}</p>
    ${f.detail && f.detail !== f.title ? `<details class="more"><summary>자세히</summary><pre>${esc(f.detail)}</pre></details>` : ""}
    ${buttons ? `<div class="row" style="margin-top:12px">${buttons}</div>` : ""}</div></div></div>`;
}

export function progressPanel(title: string): string {
  return `<div class="panel pg" aria-live="polite"><div class="pg-h"><b>${esc(title)}…</b><span class="pg-pct">0%</span></div>
    <div class="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100"><i style="width:0%"></i></div>
    <div class="pg-f"><span class="pg-step">준비하는 중</span><button type="button" class="btn" data-cancel>취소</button></div></div>`;
}
export function setProgress(root: ParentNode, fraction: number | null, step?: string) {
  const bar = $<HTMLElement>(".bar i", root), pct = $(".pg-pct", root), st = $(".pg-step", root);
  if (fraction == null) { bar?.parentElement?.classList.add("indet"); if (pct) pct.textContent = ""; }
  else {
    const p = Math.max(0, Math.min(100, Math.round(fraction * 100)));
    bar?.parentElement?.classList.remove("indet");
    if (bar) bar.style.width = `${p}%`;
    bar?.parentElement?.setAttribute("aria-valuenow", String(p));
    if (pct) pct.textContent = `${p}%`;
  }
  if (st && step != null) st.textContent = step;
}

/** 빈 상태 */
export function empty(icon: string, title: string, body = "", extra = ""): string {
  return `<div class="empty"><span class="big">${I(icon)}</span><b>${esc(title)}</b>${body ? `<span>${esc(body)}</span>` : ""}${extra}</div>`;
}
