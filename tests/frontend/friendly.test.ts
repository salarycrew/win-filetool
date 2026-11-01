// 화면 오류 안내(friendly) 회귀 시험 — `npm test`(scripts/test-frontend.mjs가 묶어서 node:test로 돌림)
import { test } from "node:test";
import assert from "node:assert/strict";
import { friendly } from "../../src/api";

test("QA-05: 0바이트 파일은 '잠시 뒤 다시'가 아니라 빈 파일 안내", () => {
  const f = friendly({ code: "empty_file", message: "빈 파일입니다(0바이트, 내용 없음): C:\\샘플\\빈파일.pdf" });
  assert.equal(f.title, "빈 파일이에요");
  assert.match(f.body, /다시 받거나/);
  // 코드 없이 문자열 오류로 와도
  assert.equal(friendly("빈 파일입니다(0바이트, 내용 없음): a.jpg").title, "빈 파일이에요");
});

test("QA-05: 쓰기 금지 폴더는 다른 저장 위치를 권함", () => {
  for (const e of [
    { code: "permission_denied", message: "파일 입출력 오류 (C:\\샘플\\잠금\\a_편집.pdf): 액세스가 거부되었습니다. (os error 5)" },
    "파일 입출력 오류 (C:\\샘플\\잠금\\a.jpg): 액세스가 거부되었습니다. (os error 5)",
    "Access is denied. (os error 5)",
  ]) {
    const f = friendly(e);
    assert.equal(f.title, "이 폴더에 저장할 수 없어요", JSON.stringify(e));
    assert.match(f.body, /저장 위치/);
  }
});

test("다른 안내는 그대로", () => {
  assert.equal(friendly({ code: "cancelled", message: "취소했습니다" }).title, "멈췄어요");
  assert.equal(friendly({ code: "error", message: "PDF를 처리할 수 없습니다 (a.pdf): invalid file header" }).title, "파일을 읽지 못했어요");
  assert.equal(friendly("알 수 없는 문제").title, "끝내지 못했어요");
});

test("0.3.1 한글 암호·배포용은 PDF 안내가 아니라 한글 안내", () => {
  const pw = friendly("암호가 걸린 한글 파일이에요. 한글에서 암호를 풀고 다시 넣어 주세요.");
  assert.equal(pw.title, "암호가 걸린 한글 파일이에요");
  assert.equal(pw.kind, "locked");
  const open = friendly({ code: "error", message: "한글: 한글이 이 파일을 열지 못했어요. 손상된 파일이거나 암호가 걸린 한글 파일일 수 있어요." });
  assert.equal(open.title, "한글이 이 파일을 열지 못했어요");
  const dist = friendly("배포용 한글 파일인데 만든 사람이 인쇄를 막아 두어 PDF로 바꿀 수 없어요.");
  assert.equal(dist.title, "배포용 한글 파일이에요");
  assert.match(dist.body, /인쇄를 막아/);
  // 실제 오류 글(앞에 "한글 실행 실패:")도 본문은 안내만
  assert.equal(friendly("한글 실행 실패: 배포용 한글 파일인데 만든 사람이 인쇄를 막아 두어 PDF로 바꿀 수 없어요.").body, "배포용 한글 파일인데 만든 사람이 인쇄를 막아 두어 PDF로 바꿀 수 없어요.");
  // PDF 암호는 그대로
  assert.equal(friendly("암호가 걸린 PDF입니다").title, "암호가 걸린 PDF예요");
});
