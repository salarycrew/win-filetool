// 앱 정보: 버전, "파일은 이 PC 밖으로 나가지 않아요", 오픈소스 고지(app_info), 소스 받는 곳
import { friendly, invoke } from "../api";
import { I } from "../icons";
import { Screen } from "../router";
import { $, esc, toast } from "../ui";

interface AppInfo { name: string; version: string; notices: string; notices_path: string | null; config_dir: string; config_source: "env" | "portable" | "default"; webview2: string | null }
let root: HTMLElement;
let info: AppInfo | null = null;

function render() {
  const a = info;
  const src = a?.notices.match(/소스 코드는 다음 주소에서 받을 수 있습니다:\s*(\S+)/)?.[1] ?? null;
  root.innerHTML = `<h1 class="title">앱 정보</h1>
    <div class="about-head panel"><svg class="app-big" viewBox="0 0 16 16" aria-hidden="true"><rect width="16" height="16" rx="3.5" fill="#0F6CBD"/><path d="M5 3.5h4l2.5 2.5v6a.5.5 0 0 1-.5.5H5a.5.5 0 0 1-.5-.5V4a.5.5 0 0 1 .5-.5z" fill="#fff"/><path d="M6.3 9.2h3.4M8.4 7.8l1.3 1.4-1.3 1.4" stroke="#0F6CBD" stroke-width="1" fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>
      <div><b class="big-t">파일 도구</b><span>버전 ${esc(a?.version ?? "…")}</span></div></div>
    <div class="sc"><span class="sc-ic ok-t">${I("shield")}</span><div class="sc-t"><b>파일은 이 PC 밖으로 나가지 않아요</b><span>모든 변환·편집·검색을 이 PC 안에서 해요. 인터넷으로 파일을 보내지 않아요.</span></div></div>
    <div class="sc"><span class="sc-ic">${I("heart")}</span><div class="sc-t"><b>원본은 그대로 둬요</b><span>결과는 원본 옆에 새 이름(이름_압축.pdf, 같은 이름이 있으면 “(1)”)으로 저장해요.</span></div></div>
    <div class="sc"><span class="sc-ic">${I("folder")}</span><div class="sc-t"><b>설정이 있는 곳</b><span>${esc(a ? `${a.config_dir}${a.config_source === "portable" ? " (휴대용 — exe 옆)" : ""}` : "…")}</span></div>
      ${a ? `<button type="button" class="btn subtle" data-dir="${esc(a.config_dir)}">${I("open")}열기</button>` : ""}</div>
    <h2 class="h2">오픈소스</h2>
    <div class="sc"><span class="sc-ic">${I("info")}</span><div class="sc-t"><b>오픈소스 고지</b><span>이 앱은 ffmpeg(LGPL)·LibreOffice(MPL)·PDFium(BSD) 등 오픈소스 부품을 써요. 저작권과 라이선스 전문을 볼 수 있어요.</span></div>
      <button type="button" class="btn" data-act="notices">${I("eye")}고지 보기</button>${a?.notices_path ? `<button type="button" class="btn subtle" data-file="${esc(a.notices_path)}">${I("open")}파일로 열기</button>` : ""}</div>
    <div class="sc"><span class="sc-ic">${I("open")}</span><div class="sc-t"><b>소스 받는 곳</b><span class="mono">${esc(src ?? "오픈소스 고지 맨 위 “소스 받는 곳”에 적혀 있어요")}</span></div>
      ${src ? `<button type="button" class="btn subtle" data-copy="${esc(src)}">${I("copy")}주소 복사</button>` : ""}</div>
    <div class="notices" hidden><textarea readonly class="ex big" aria-label="오픈소스 고지 전문"></textarea></div>
    <p class="ver">${a?.webview2 ? `WebView2 ${esc(a.webview2)}` : ""}</p>`;
}

export const aboutScreen: Screen = {
  id: "about", title: "앱 정보", icon: "info",
  init(el) {
    root = el;
    root.addEventListener("click", async (e) => {
      const t = e.target as HTMLElement;
      const d = t.closest<HTMLElement>("[data-dir]"), f = t.closest<HTMLElement>("[data-file]"), c = t.closest<HTMLElement>("[data-copy]");
      try {
        if (d) await invoke("open_dir", { path: d.dataset.dir });
        if (f) await invoke("open_path", { path: f.dataset.file });
      } catch (err) { toast(friendly(err).title); }
      if (c) { try { await navigator.clipboard.writeText(c.dataset.copy!); toast("주소를 복사했어요"); } catch { toast("복사하지 못했어요"); } }
      if (t.closest("[data-act=notices]")) {
        const box = $(".notices", root)!;
        box.hidden = !box.hidden;
        const ta = $<HTMLTextAreaElement>("textarea", box)!;
        if (!ta.value && info) ta.value = info.notices;
      }
    });
    render();
  },
  async show() {
    if (info) return;
    try { info = await invoke<AppInfo>("app_info"); } catch (e) { toast(friendly(e).title); }
    render();
  },
};
