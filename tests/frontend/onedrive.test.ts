// 0.3.4 OneDrive 폴더 되돌리기 화면: 경우마다 단추 하나, 조사, 결과 문장 — `npm test`
import { test } from "node:test";
import assert from "node:assert/strict";
import { cardHtml, resultHtml, toward, type FolderReport, type Report } from "../../src/screens/onedrive";

const cnt = (files: number, cf = 0) => ({ files, bytes: files * 1000, cloud_files: cf, cloud_bytes: cf * 1000, truncated: false });
const rep: Report = { folders: [], accounts: [], onedrive_running: true, onedrive_set_up: true, summary: "", fake: true, backup_off_steps: "구름 아이콘 → 백업 관리", delete_warning: "지우면 클라우드에서도 지워져요", recycle_help: "휴지통을 보세요" };
const f = (c: "A" | "B" | "C" | "D"): FolderReport => ({
  key: "desktop", name: "바탕 화면", case: c, title: "t", detail: "", current: "C:\\U\\Desktop", current_exists: true, in_onedrive: c === "A", here: cnt(1),
  onedrive_copy: c === "B" ? "C:\\U\\OneDrive\\바탕 화면" : undefined, onedrive_count: c === "B" ? cnt(5, 2) : undefined,
  local_default: "C:\\U\\Desktop", local_default_exists: true,
});

test("조사: 바탕 화면으로, 문서로, 사진으로, 다운로드로", () => {
  assert.deepEqual(["바탕 화면", "문서", "사진", "다운로드"].map(toward), ["바탕 화면으로", "문서로", "사진으로", "다운로드로"]);
});

test("경우마다 큰 단추 하나", () => {
  assert.match(cardHtml(f("A"), rep), /OneDrive 백업 끄기 열기[\s\S]*다시 확인/);
  assert.match(cardHtml(f("B"), rep), /data-od="restore"[^>]*>[\s\S]*바탕 화면으로 가져오기/);
  assert.match(cardHtml(f("B"), rep), /클라우드에만 있어요/);
  assert.match(cardHtml(f("C"), rep), /할 일 없어요/);
  assert.match(cardHtml(f("D"), rep), /휴지통을 보세요[\s\S]*OneDrive 웹 열기/);
  for (const c of ["A", "C", "D"] as const) assert.doesNotMatch(cardHtml(f(c), rep), /data-od="restore"/);
  // 어려운 말 없음
  for (const c of ["A", "B", "C", "D"] as const) assert.doesNotMatch(cardHtml(f(c), rep), /Known Folder|KFM|레지스트리/);
});

test("결과: N개 가져옴, M개 같아서 건너뜀, K개 이름 바꿔 둠 + 지우기 경고 + 되돌리기", () => {
  const h = resultHtml({ id: "x", from: "a", to: "b", copied: 3, same: 2, renamed: 1, copied_bytes: 1, cloud_skipped: 0, cloud_skipped_bytes: 0, skipped: [], cancelled: false, log: "l", summary: "" }, rep.delete_warning);
  assert.match(h, /3개 가져옴, 2개 같아서 건너뜀, 1개 이름 바꿔 둠/);
  assert.match(h, /클라우드에서도 지워져요/);
  assert.match(h, /data-od="undo"/);
  assert.doesNotMatch(h, /OneDrive.*지우기<\/button>/);
});
