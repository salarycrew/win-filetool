// 처음 켰을 때 짧은 안내(팁 3개, 한 번만). 화면을 막지 않는 띠 모양.
import { I } from "./icons";
import { $ } from "./ui";

const KEY = "ft.welcomed";
export const welcomed = () => localStorage.getItem(KEY) === "1";
export function resetWelcome() { localStorage.removeItem(KEY); }

export function welcomeHTML(): string {
  if (welcomed()) return "";
  return `<aside class="welcome" aria-label="처음 쓰는 분께">
    <div class="wl-h"><b>처음 오셨네요. 이렇게 쓰면 돼요</b><button type="button" class="btn subtle sm" data-welcome-close>${I("x")}알겠어요</button></div>
    <ol class="wl-tips">
      <li>${I("upload")}<span><b>파일을 끌어다 놓으세요.</b> 그 파일로 할 수 있는 일만 카드로 보여 드려요.</span></li>
      <li>${I("shield")}<span><b>원본은 그대로예요.</b> 결과는 원본 옆에 새 이름으로 저장하고, 파일은 이 PC 밖으로 나가지 않아요.</span></li>
      <li>${I("layers")}<span><b>PDF 쪽 정리·서명은 [PDF 편집]</b>, 이름 바꾸기·같은 파일 찾기는 <b>[파일 정리]</b>에 있어요.</span></li>
    </ol></aside>`;
}
export function bindWelcome(root: HTMLElement) {
  const b = $("[data-welcome-close]", root);
  if (!b) return;
  b.addEventListener("click", () => {
    localStorage.setItem(KEY, "1");
    b.closest(".welcome")?.remove();
  });
}
