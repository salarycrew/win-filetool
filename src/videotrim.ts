// 영상 앞뒤 자르기·장면 고르기: 장면 띠(media_frame) + 두 손잡이 막대 + 정확한 시간 칸
import { invoke, fmtDur } from "./api";
import { $, esc } from "./ui";

export interface MediaInfo { duration: number; width: number; height: number; rotation: number; has_audio: boolean; video_codec?: string; audio_codec?: string }

export const frameUrl = (s: string) => (s.startsWith("data:") ? s : `data:image/jpeg;base64,${s}`);

/** "1:02.5" / "62.5" / "1:02:03" → 초 */
export function parseTime(t: string): number | null {
  t = t.trim().replace(",", ".");
  if (!t) return null;
  const parts = t.split(":").map((x) => Number(x));
  if (parts.some((x) => !Number.isFinite(x) || x < 0)) return null;
  return parts.reduce((a, x) => a * 60 + x, 0);
}
export function showTime(sec: number): string {
  sec = Math.max(0, sec);
  const h = Math.floor(sec / 3600), m = Math.floor((sec % 3600) / 60), s = sec % 60;
  const ss = s.toFixed(1).padStart(4, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

export interface Trim {
  start: number;
  end: number;
  info: MediaInfo | null;
  at(): number; // 장면 고르기(손잡이 하나)일 때 시각
}

/** root 안에 자르기 판을 그린다. single=true면 손잡이 하나(장면 저장) */
export function mountTrim(root: HTMLElement, path: string, single = false): Trim {
  const st: Trim = { start: 0, end: 0, info: null, at: () => st.start };
  root.innerHTML = `<div class="trim ${single ? "single" : ""}">
    <div class="trim-view"><img alt="" class="trim-big" hidden><span class="trim-load">영상 정보를 읽는 중…</span></div>
    <div class="trim-strip" aria-hidden="true"></div>
    <div class="trim-bar"><div class="trim-sel"></div>
      <button type="button" class="trim-h" data-h="s" role="slider" aria-label="${single ? "장면 위치" : "시작 위치"}"></button>
      ${single ? "" : `<button type="button" class="trim-h" data-h="e" role="slider" aria-label="끝 위치"></button>`}</div>
    <div class="trim-times">
      <label class="fld"><span>${single ? "장면 시각" : "시작"}</span><input class="txt sm" data-t="s" value="0:00.0" inputmode="decimal"></label>
      ${single ? "" : `<label class="fld"><span>끝</span><input class="txt sm" data-t="e" value="" inputmode="decimal"></label>`}
      <span class="trim-len hint"></span>
    </div></div>`;
  const bar = $(".trim-bar", root)!, sel = $(".trim-sel", root)!, big = $<HTMLImageElement>(".trim-big", root)!, load = $(".trim-load", root)!;
  const hs = $<HTMLButtonElement>('[data-h="s"]', root)!, he = $<HTMLButtonElement>('[data-h="e"]', root);
  const ts = $<HTMLInputElement>('[data-t="s"]', root)!, te = $<HTMLInputElement>('[data-t="e"]', root);
  const len = $(".trim-len", root)!;
  let dur = 0;

  const draw = () => {
    if (!dur) return;
    const a = (st.start / dur) * 100, b = (st.end / dur) * 100;
    hs.style.left = `${a}%`;
    hs.setAttribute("aria-valuetext", showTime(st.start));
    if (he) { he.style.left = `${b}%`; he.setAttribute("aria-valuetext", showTime(st.end)); }
    sel.style.left = `${single ? a : a}%`;
    sel.style.width = single ? "0" : `${Math.max(0, b - a)}%`;
    if (document.activeElement !== ts) ts.value = showTime(st.start);
    if (te && document.activeElement !== te) te.value = showTime(st.end);
    len.textContent = single ? `전체 ${fmtDur(dur)}` : `남는 길이 ${fmtDur(st.end - st.start)} / 전체 ${fmtDur(dur)}`;
  };
  let peekTimer = 0, peekSeq = 0;
  const peek = (t: number) => {
    window.clearTimeout(peekTimer);
    peekTimer = window.setTimeout(async () => {
      const seq = ++peekSeq;
      try {
        const f = await invoke<string>("media_frame", { path, at: Math.min(Math.max(0, t), Math.max(0, dur - 0.05)), width: 480 });
        if (seq !== peekSeq) return;
        big.src = frameUrl(f);
        big.hidden = false;
        load.hidden = true;
      } catch { /* 미리보기 없어도 자를 수 있음 */ }
    }, 120);
  };
  const setT = (which: "s" | "e", t: number) => {
    t = Math.max(0, Math.min(dur, t));
    if (single) st.start = t;
    else if (which === "s") st.start = Math.min(t, st.end - 0.1);
    else st.end = Math.max(t, st.start + 0.1);
    st.start = Math.max(0, st.start);
    st.end = Math.min(dur, st.end);
    draw();
    peek(which === "s" ? st.start : st.end);
  };
  const drag = (h: HTMLButtonElement, which: "s" | "e") => {
    h.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      h.setPointerCapture(e.pointerId);
      h.focus();
      const r = bar.getBoundingClientRect();
      const move = (ev: PointerEvent) => setT(which, ((ev.clientX - r.left) / r.width) * dur);
      const up = () => { h.removeEventListener("pointermove", move); h.removeEventListener("pointerup", up); };
      h.addEventListener("pointermove", move);
      h.addEventListener("pointerup", up);
    });
    h.addEventListener("keydown", (e) => {
      const step = e.shiftKey ? 5 : e.ctrlKey ? 0.1 : 1;
      const cur = which === "s" ? st.start : st.end;
      if (e.key === "ArrowRight" || e.key === "ArrowUp") { setT(which, cur + step); e.preventDefault(); }
      else if (e.key === "ArrowLeft" || e.key === "ArrowDown") { setT(which, cur - step); e.preventDefault(); }
      else if (e.key === "Home") { setT(which, 0); e.preventDefault(); }
      else if (e.key === "End") { setT(which, dur); e.preventDefault(); }
    });
  };
  drag(hs, "s");
  if (he) drag(he, "e");
  bar.addEventListener("pointerdown", (e) => {
    if ((e.target as HTMLElement).closest(".trim-h") || !dur) return;
    const r = bar.getBoundingClientRect();
    const t = ((e.clientX - r.left) / r.width) * dur;
    setT(single || Math.abs(t - st.start) < Math.abs(t - st.end) ? "s" : "e", t);
  });
  const onTime = (inp: HTMLInputElement, which: "s" | "e") => inp.addEventListener("change", () => {
    const t = parseTime(inp.value);
    if (t == null) { inp.classList.add("bad"); return; }
    inp.classList.remove("bad");
    setT(which, t);
  });
  onTime(ts, "s");
  if (te) onTime(te, "e");

  (async () => {
    try {
      const info = await invoke<MediaInfo>("media_probe", { path });
      st.info = info;
      dur = info.duration || 0;
      st.end = dur;
      if (!dur) { load.textContent = "영상 길이를 읽지 못했어요. 시간 칸에 직접 넣어 주세요."; return; }
      draw();
      peek(single ? 0 : 0);
      // 장면 띠: 8장
      const strip = $(".trim-strip", root)!;
      const n = 8;
      strip.innerHTML = Array.from({ length: n }, () => `<span></span>`).join("");
      const cells = [...strip.children] as HTMLElement[];
      for (let i = 0; i < n; i++) {
        const t = (dur * (i + 0.5)) / n;
        try {
          const f = await invoke<string>("media_frame", { path, at: t, width: 160 });
          cells[i].innerHTML = `<img alt="" src="${esc(frameUrl(f))}">`;
        } catch { break; }
      }
    } catch (e) {
      load.textContent = "영상 정보를 읽지 못했어요. 시작·끝 시간을 직접 넣어 주세요.";
      dur = 0;
      if (te) te.value = "";
      void e;
    }
  })();

  // 직접 입력만 있을 때(영상 정보를 못 읽었을 때)도 값을 읽을 수 있게
  const origAt = st.at;
  st.at = () => (dur ? origAt() : parseTime(ts.value) ?? 0);
  Object.defineProperty(st, "manual", { value: () => ({ s: parseTime(ts.value), e: te ? parseTime(te.value) : null }) });
  return st;
}

/** 영상 정보 없이 직접 넣은 시간까지 반영한 시작·끝 */
export function trimRange(t: Trim): { start: number; end: number } | null {
  if (t.info && t.info.duration) return { start: t.start, end: t.end };
  const m = (t as unknown as { manual: () => { s: number | null; e: number | null } }).manual();
  if (m.s == null || m.e == null || m.e <= m.s) return null;
  return { start: m.s, end: m.e };
}
