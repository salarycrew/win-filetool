// 0.3.2 핵심 6개: 넣은 파일에 맞는 할 일 고르기, 이름순, 결과 요약 문장 — `npm test`
import { test } from "node:test";
import assert from "node:assert/strict";
import { JOBS, acceptsItem, byName, jobDef, mbToBytes, suggest, summary } from "../../src/screens/core";

const it = (path: string, kind: string) => ({ path, kind });

test("PDF 여러 개 → 합치기·용량 줄이기, 한 개 → 용량 줄이기 먼저", () => {
  assert.deepEqual(suggest([it("a.pdf", "pdf"), it("b.pdf", "pdf")]), ["merge", "compress"]);
  assert.deepEqual(suggest([it("a.pdf", "pdf")]), ["compress", "merge"]);
});

test("사진 여러 장 → PDF로 합치기 먼저, 한 장 → 용량 줄이기 먼저, HEIC 포함", () => {
  assert.deepEqual(suggest([it("1.HEIC", "heic"), it("2.jpg", "image")]), ["img2pdf", "photo"]);
  assert.deepEqual(suggest([it("1.png", "image")]), ["photo", "img2pdf"]);
});

test("한글·Office → PDF로, 폴더·zip·맥 이름 → 이름 고치기", () => {
  assert.deepEqual(suggest([it("보고서.hwp", "document"), it("표.xlsx", "document")]), ["office2pdf"]);
  assert.deepEqual(suggest([it("C:\\받은 것", "dir")]), ["macname"]);
  assert.deepEqual(suggest([it("자료.zip", "zip")]), ["macname"]);
  const nfd = "한글.pdf".normalize("NFD");
  assert.ok(suggest([it(nfd, "pdf")]).includes("macname"));
  assert.deepEqual(suggest([it("x.mp4", "video")]), []);
});

test("할 일마다 맞는 파일만 받음", () => {
  assert.ok(acceptsItem(jobDef("merge")!, it("a.PDF", "pdf")));
  assert.ok(!acceptsItem(jobDef("merge")!, it("a.jpg", "image")));
  assert.ok(acceptsItem(jobDef("photo")!, it("a.webp", "image")));
  assert.ok(acceptsItem(jobDef("office2pdf")!, it("a.hwpx", "document")));
  assert.ok(!acceptsItem(jobDef("office2pdf")!, it("폴더", "dir")));
  assert.ok(acceptsItem(jobDef("macname")!, it("폴더", "dir")));
  assert.equal(JOBS.length, 6);
});

test("이름순은 숫자를 숫자로(사진2 < 사진10)", () => {
  const xs = byName([it("C:\\a\\사진10.jpg", "image"), it("C:\\a\\사진2.jpg", "image"), it("C:\\a\\사진1.jpg", "image")]);
  assert.deepEqual(xs.map((x) => x.path.split("\\").pop()), ["사진1.jpg", "사진2.jpg", "사진10.jpg"]);
});

test("결과 요약은 쉬운 한 문장", () => {
  assert.equal(summary(10, 9, ["locked"]), "10개 중 9개 완료, 1개는 암호가 걸려 있어요");
  assert.equal(summary(3, 3, []), "3개 모두 완료");
  assert.equal(summary(1, 1, []), "다 됐어요");
  assert.equal(summary(5, 2, ["locked", "damaged", "locked"]), "5개 중 2개 완료, 2개는 암호가 걸려 있어요, 1개는 파일이 손상됐거나 읽을 수 없어요");
  assert.equal(summary(300, 120, [], "장"), "300장 중 120장 완료, 180장은 하지 않았어요(멈춤)");
  assert.equal(mbToBytes(5), 5_000_000);
});
