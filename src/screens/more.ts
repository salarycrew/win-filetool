// 0.3.2 "더 보기": 핵심 6개 밖의 기능 전부(지운 것은 없음). 예전 화면으로 그대로 들어간다.
import { I } from "../icons";
import { go, Screen, ScreenId } from "../router";
import { esc } from "../ui";

const ENTRIES: { id: ScreenId; t: string; d: string; ic: string; tone: string }[] = [
  { id: "convert", t: "모든 할 일(변환)", d: "파일을 넣으면 할 수 있는 일을 모두 보여 줘요 · Word→PPT, PDF→Word, 영상·소리, 워터마크, 암호…", ic: "convert", tone: "blue" },
  { id: "pdfedit", t: "PDF 편집", d: "쪽 순서·돌리기·지우기·끼워 넣기, 서명·도장, 쪽 번호, 양식 채우기", ic: "layers", tone: "red" },
  { id: "organize", t: "파일 정리", d: "이름 한꺼번에 바꾸기, 같은 파일 찾기, zip 풀기·묶기", ic: "rename", tone: "purple" },
  { id: "search", t: "검색", d: "모든 드라이브의 파일 이름을 바로 찾기(초성도 돼요)", ic: "search", tone: "teal" },
  { id: "watch", t: "감시 폴더", d: "정해 둔 폴더에 들어온 파일을 자동으로 바꾸기", ic: "watch", tone: "orange" },
  { id: "about", t: "앱 정보", d: "버전, 오픈소스 고지", ic: "info", tone: "gray" },
];

export const moreScreen = {
  id: "more", title: "더 보기", icon: "more",
  init(root: HTMLElement) {
    root.innerHTML = `<h1 class="title">더 보기</h1><p class="sub">자주 쓰는 6가지 말고도 이런 일을 할 수 있어요.</p>
      <div class="cards two more-l">${ENTRIES.map((e) => `<button type="button" class="card" data-to="${e.id}"><span class="tile tone-${e.tone}">${I(e.ic)}</span>
        <span class="card-t"><b>${esc(e.t)}</b><span class="d">${esc(e.d)}</span></span><span class="go">${I("right")}</span></button>`).join("")}</div>`;
    root.addEventListener("click", (e) => {
      const b = (e.target as HTMLElement).closest<HTMLElement>("[data-to]");
      if (b) go(b.dataset.to as ScreenId);
    });
  },
} satisfies Screen as Screen;

/** 왼쪽 메뉴에서 "더 보기"가 켜져 보일 화면들 */
export const MORE_IDS: ScreenId[] = ENTRIES.map((e) => e.id);
