// 0.3.2 첫 화면: 큰 단추 6개 + 끌어다 놓기. 놓은 파일 종류에 맞는 할 일을 골라 보여 준다.
import { Item, friendly, invoke, pickFiles } from "../api";
import { I } from "../icons";
import { go, Screen } from "../router";
import { $, $$, esc, toast } from "../ui";
import { JOBS, JobId, jobDef, suggest } from "./core";

let root: HTMLElement;
let dropped: Item[] = [];
let picks: JobId[] = [];

const KIND_LABEL = (xs: Item[]) => {
  const c = (f: (x: Item) => boolean) => xs.filter(f).length;
  const parts: string[] = [];
  const pdf = c((x) => x.kind === "pdf"), img = c((x) => x.kind === "image" || x.kind === "heic"), doc = c((x) => x.kind === "document");
  const dir = c((x) => x.kind === "dir"), zip = c((x) => x.kind === "zip"), other = xs.length - pdf - img - doc - dir - zip;
  if (pdf) parts.push(`PDF ${pdf}개`);
  if (img) parts.push(`사진 ${img}장`);
  if (doc) parts.push(`문서 ${doc}개`);
  if (dir) parts.push(`폴더 ${dir}개`);
  if (zip) parts.push(`zip ${zip}개`);
  if (other) parts.push(`그 밖의 파일 ${other}개`);
  return parts.join(" · ");
};

function render() {
  root.innerHTML = `<h1 class="title">무엇을 할까요?</h1>
    <p class="sub home-promise">${I("shield")}<span>광고·가입 없음 · 파일은 PC 밖으로 나가지 않아요</span></p>
    <div class="jobs" role="list">${JOBS.map((j) => `<button type="button" role="listitem" class="job-b ${picks.includes(j.id) ? "hint-on" : ""}" data-job="${j.id}">
      <span class="tile tone-${j.tone}">${I(j.icon)}</span><span class="card-t"><b>${esc(j.title)}</b><span class="d">${esc(j.desc)}</span></span></button>`).join("")}</div>
    ${dropped.length ? suggestHTML() : `<div class="dz home-dz" id="hdz" tabindex="0" role="button" aria-label="파일 고르기 — 또는 여기에 끌어다 놓기">
      <span class="dz-ic sm">${I("upload")}</span><span class="dz-t2"><b>파일을 끌어다 놓으면</b> 맞는 일을 골라 드려요</span>
      <button type="button" class="btn" data-act="pick">파일 고르기</button></div>`}
    <p class="home-more"><button type="button" class="link" data-act="more">${I("grid")}다른 일은 [더 보기]에 — PDF 편집·영상·이름 바꾸기·검색 등</button></p>`;
}

function suggestHTML(): string {
  const label = KIND_LABEL(dropped);
  if (!picks.length) {
    return `<div class="suggest">${I("info")}<span><b>${esc(label)}</b> — 여기 6가지로는 할 수 없는 파일이에요.</span>
      <button type="button" class="btn accent" data-act="all">다른 할 일 보기</button><button type="button" class="btn subtle" data-act="reset" aria-label="지우기">${I("x")}</button></div>`;
  }
  return `<div class="suggest">${I("check")}<span><b>${esc(label)}</b> — 이렇게 할 수 있어요</span>
    <span class="sg-b">${picks.map((p, i) => `<button type="button" class="btn ${i === 0 ? "accent" : ""}" data-go="${p}">${esc(jobDef(p)!.title)}</button>`).join("")}
    <button type="button" class="btn subtle" data-act="all">다른 할 일</button><button type="button" class="btn subtle" data-act="reset" aria-label="지우기">${I("x")}</button></span></div>`;
}

async function take(paths: string[]) {
  if (!paths.length) return;
  try {
    dropped = (await invoke<Item[]>("inspect_paths", { paths })).filter((x) => x.kind !== "missing");
  } catch (e) { toast(friendly(e).title); return; }
  picks = suggest(dropped);
  render();
  $<HTMLElement>(".suggest .btn", root)?.focus();
}

export const homeScreen = {
  id: "home", title: "홈", icon: "home",
  init(el: HTMLElement) {
    root = el;
    root.addEventListener("click", async (e) => {
      const t = e.target as HTMLElement;
      const j = t.closest<HTMLElement>("[data-job]");
      if (j) {
        const id = j.dataset.job as JobId;
        // 놓아 둔 파일이 그 일에 맞으면 가지고 간다
        const paths = picks.includes(id) ? dropped.map((x) => x.path) : [];
        dropped = []; picks = [];
        go("job", { job: id, paths });
        return;
      }
      const g = t.closest<HTMLElement>("[data-go]");
      if (g) { const paths = dropped.map((x) => x.path); dropped = []; picks = []; go("job", { job: g.dataset.go as JobId, paths }); return; }
      const b = t.closest<HTMLElement>("[data-act]");
      if (!b) { if (t.closest("#hdz")) take(await pickFiles()); return; }
      switch (b.dataset.act) {
        case "pick": take(await pickFiles()); break;
        case "more": go("more"); break;
        case "all": { const paths = dropped.map((x) => x.path); dropped = []; picks = []; render(); go("convert", { add: paths }); break; }
        case "reset": dropped = []; picks = []; render(); break;
      }
    });
    root.addEventListener("keydown", async (e) => {
      if ((e.target as HTMLElement).id === "hdz" && (e.key === "Enter" || e.key === " ")) { e.preventDefault(); take(await pickFiles()); }
    });
    render();
  },
  show() { render(); $$<HTMLElement>(".job-b", root)[0]?.blur(); },
  drop(paths: string[]) { take(paths); },
} satisfies Screen as Screen;
