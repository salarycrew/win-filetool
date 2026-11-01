// 0.3.1 새 버전 알림: 켤 때 하루 한 번까지 GitHub에서 최신 판 번호만 확인하고, 새 판이 있으면 위쪽에 작은 띠를 보여 준다.
// 받기·설치는 하지 않는다(브라우저에서 릴리스 쪽을 연다). 실패·오프라인이면 아무것도 보이지 않는다.
import { invoke } from "./api";
import { I } from "./icons";
import { $, el, esc } from "./ui";

export interface UpdateView { enabled: boolean; current: string; banner: { version: string; url: string } | null }
export type UpdateOutcome = { state: "newer"; version: string; url: string } | { state: "latest" } | { state: "failed" } | { state: "not_checked" };

let closedFor = "";

/** 알림 띠 보이기/숨기기(설정에서 끄거나 넘기면 바로 사라짐) */
export function showBanner(v: UpdateView | null) {
  const pane = $("#pane")!;
  let b = $<HTMLElement>("#upd", pane);
  const ver = v?.banner?.version;
  if (!ver || closedFor === ver) { b?.remove(); return; }
  if (!b) {
    b = el(`<div id="upd" class="note upd" role="status"></div>`);
    pane.prepend(b);
  }
  b.innerHTML = `${I("info")}<span class="grow">새 버전 ${esc(ver)}가 나왔어요</span>
    <button type="button" class="btn accent sm" data-upd="open">받으러 가기</button>
    <button type="button" class="btn subtle sm" data-upd="skip">이 버전 넘기기</button>
    <button type="button" class="ib" data-upd="close" aria-label="닫기" title="닫기">${I("x")}</button>`;
  b.dataset.ver = ver;
}

export async function refreshBanner() {
  showBanner(await invoke<UpdateView>("update_view").catch(() => null));
}

export function initUpdate() {
  document.addEventListener("click", async (e) => {
    const t = (e.target as HTMLElement).closest<HTMLElement>("[data-upd]");
    if (!t) return;
    const ver = $<HTMLElement>("#upd")?.dataset.ver ?? "";
    switch (t.dataset.upd) {
      case "open": await invoke("update_open").catch(() => {}); break;
      case "skip": showBanner(await invoke<UpdateView>("update_skip", { version: ver }).catch(() => null)); document.dispatchEvent(new Event("ft-update")); break;
      case "close": closedFor = ver; showBanner(null); break;
    }
  });
  // 알아 둔 새 판이 있으면 바로, 그다음 (하루 한 번까지) 확인
  refreshBanner().then(() => invoke<UpdateOutcome>("update_check", { force: false }))
    .then((o) => { if (o.state === "newer") refreshBanner(); })
    .catch(() => {});
}
