// 화면 이동(모듈끼리 서로 import하지 않게 한 곳에서)
export type ScreenId = "home" | "job" | "more" | "convert" | "pdfedit" | "organize" | "search" | "watch" | "settings" | "about" | "onedrive";
export interface Screen {
  id: ScreenId;
  title: string;
  icon: string;
  /** 검색처럼 화면 높이를 꽉 채우는 화면 */
  flush?: boolean;
  init(root: HTMLElement): void;
  show?(arg?: unknown): void;
  hide?(): void;
  /** 끌어다 놓은 파일(이 화면이 보일 때) */
  drop?(paths: string[]): void;
}
type GoFn = (id: ScreenId, arg?: unknown) => void;
let goImpl: GoFn = () => {};
export const setGo = (f: GoFn) => { goImpl = f; };
export const go: GoFn = (id, arg) => goImpl(id, arg);
