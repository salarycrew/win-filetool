//! filetool-cli — 코어 작업을 명령줄에서 실행한다(데스크톱 점검·자동화용).
//! GUI 앱(`win-filetool.exe`)과 같은 코어를 쓴다.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use filetool_core::pdf::{self, SplitMode};
use filetool_core::samples::{self, nfd};
use filetool_core::{nfc, paths, zipnfc, Result};

mod conv;
mod pdf1c;
mod runtime030;
mod pdfedit030;
mod search2a;
mod selftest1b;
mod selftest1c;
mod svc2c;

const USAGE: &str = "사용법:
  filetool-cli --version                              앱 버전
  filetool-cli nfc-preview <폴더>
  filetool-cli nfc-apply <폴더>
  filetool-cli zip-extract <zip> [출력폴더]
  filetool-cli zip-rezip <zip> [출력.zip]
  filetool-cli pdf-info <pdf>
  filetool-cli pdf-merge <출력.pdf> <입력1.pdf> <입력2.pdf> ...
  filetool-cli pdf-split <pdf> <every|1-3,5> [출력폴더]
  filetool-cli pdf-rotate <pdf> <페이지|all> <90|180|270> [출력.pdf]
  filetool-cli pdf-delete <pdf> <페이지> [출력.pdf]
  filetool-cli pdf-reorder <pdf> <새순서> [출력.pdf]
  filetool-cli tools                                   ffmpeg/LibreOffice 찾은 위치·버전
  filetool-cli image <jpg|png|webp|bmp|tif|gif> [--quality N] [--max-side N] [--no-exif] [--out 폴더] <파일...>
  filetool-cli media <mp4|messenger|mp3|m4a|gif> [--max-mb N] [--height N] [--start 초] [--dur 초] [--width N] [--fps N] [--out 폴더] <파일...>
  filetool-cli doc <pdf|docx|xlsx|pptx> [--out 폴더] <파일...>
  filetool-cli watch <폴더> <image:jpg|media:mp4|doc:pdf …> [--sub 변환됨] [--seconds N]   감시 폴더를 N초 동안 돌림
  filetool-cli pdf-compress <screen|normal|print> [--out 폴더] <pdf...>     PDF 압축(화면용/보통/인쇄용)
  filetool-cli img2pdf [--page image|a4|a4-landscape] [--margin mm] [--output 파일.pdf] <이미지...>
  filetool-cli pdf2img <png|jpg> [--dpi 150] [--pages 1-3] [--quality N] [--out 폴더] <pdf...>
  filetool-cli pdf-images [--out 폴더] <pdf...>                          들어 있는 이미지 꺼내기
  filetool-cli autostart <on|off|status> [--exe win-filetool.exe]       윈도 시작 시 실행(HKCU Run)
  filetool-cli index [루트...] [--source auto|mft|walk] [--index-dir 폴더] [--exclude 패턴]... [--no-default-excludes]   검색 색인 만들기(루트 없으면 모든 고정 드라이브) + 저장.
               --source: auto(기본, 관리자 권한 + NTFS 드라이브 전체면 MFT) / mft(MFT만, 안 되면 오류) / walk(폴더 훑기만)
  filetool-cli usn-catchup [--index-dir 폴더]   저장한 색인의 MFT 루트를 USN 저널로 따라잡기(바뀐 것만 반영) + 저장
  filetool-cli index-compare <색인 폴더 A> <색인 폴더 B> [--samples 20] [--spot 파일.tsv]   두 색인 비교(점검용)
  filetool-cli search <질의> [--limit 20] [--sort name|path|size|modified] [--desc] [--paths] [--index-dir 폴더] [--via local|service] [--repeat N]   저장한 색인(local, 기본) 또는 빠른 검색 서비스(service)에서 찾기
  filetool-cli svc-status                     빠른 검색 서비스(2c) 설치·상태·색인(일반 사용자로 됨). 종료 코드 0 답함 / 3 설치 안 됨 / 4 답하지 않음
  filetool-cli svc-watch <질의> [--seconds 30] [--interval-ms 100]   서비스 결과가 바뀔 때마다 시각과 함께 출력
  filetool-cli search-live [루트...] --query <질의> [--seconds 60] [--source auto|walk] [--index-dir 폴더]   앱과 같은 검색 서비스(실시간 갱신)를 돌리며 결과 변화를 출력
  filetool-cli selftest <빈 폴더> [--apple <Pages·Numbers·Keynote 샘플 폴더>] [--only 2a]   샘플을 만들어 모든 작업을 점검
  (0.3.0 PDF 편집 명령은 `filetool-cli help-pdfedit`)
공통 옵션: --ffmpeg <경로> --soffice <경로>  (주면 그 경로를 먼저, 없으면 앱에 든 도구 → 앱 설정 → PATH → 설치 폴더 순으로 찾음)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("오류: {e}");
            ExitCode::from(1)
        }
    }
}

fn arg(args: &[String], i: usize) -> Result<&str> {
    args.get(i)
        .map(String::as_str)
        .ok_or_else(|| filetool_core::Error::Invalid(format!("인자가 부족합니다\n{USAGE}")))
}

fn opt_path(args: &[String], i: usize) -> Option<PathBuf> {
    args.get(i).map(PathBuf::from)
}

fn run(args: &[String]) -> Result<ExitCode> {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    if matches!(cmd, "--version" | "-V") {
        println!("filetool-cli {}", env!("CARGO_PKG_VERSION"));
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(code) = runtime030::run(cmd, args)? {
        return Ok(code);
    }
    if let Some(code) = conv::run(cmd, args)? {
        return Ok(code);
    }
    if let Some(code) = pdf1c::run(cmd, args)? {
        return Ok(code);
    }
    if let Some(code) = search2a::run(cmd, args)? {
        return Ok(code);
    }
    if let Some(code) = pdfedit030::run(cmd, args)? {
        return Ok(code);
    }
    match cmd {
        "nfc-preview" | "nfc-apply" => {
            let plan = nfc::plan_folder(Path::new(arg(args, 1)?))?;
            for r in &plan.renames {
                println!("바꿈  {} -> {}", r.from.display(), r.to.display());
            }
            for c in &plan.collisions {
                println!("충돌  {} (이미 있음: {})", c.path.display(), c.existing.display());
            }
            if cmd == "nfc-apply" {
                let rep = nfc::apply_plan(&plan);
                println!("적용: 바꿈 {} / 충돌 {} / 실패 {}", rep.renamed.len(), rep.collisions.len(), rep.failed.len());
                for (p, e) in &rep.failed {
                    println!("실패  {}: {e}", p.display());
                }
            } else {
                println!("미리보기: 바꿀 항목 {} / 충돌 {} / 검사 {}", plan.renames.len(), plan.collisions.len(), plan.scanned);
            }
        }
        "zip-extract" => {
            let zip = PathBuf::from(arg(args, 1)?);
            let out = opt_path(args, 2).unwrap_or_else(|| zipnfc::default_extract_dir(&zip));
            let rep = zipnfc::extract_nfc(&zip, &out)?;
            for f in &rep.files {
                println!("풀림  {}", f.display());
            }
            println!(
                "완료: 파일 {} / 폴더 {} / NFC로 바꾼 이름 {} / 맥 부산물 제외 {} / 충돌 {} -> {}",
                rep.files.len(), rep.dirs, rep.normalized, rep.skipped_mac.len(), rep.collisions.len(), rep.out_dir.display()
            );
        }
        "zip-rezip" => {
            let zip = PathBuf::from(arg(args, 1)?);
            let out = opt_path(args, 2).unwrap_or_else(|| zipnfc::default_rezip_path(&zip, None));
            let rep = zipnfc::rezip_nfc(&zip, &out)?;
            println!("완료: 항목 {} / NFC로 바꾼 이름 {} / 맥 부산물 제외 {} -> {}", rep.entries, rep.normalized, rep.skipped_mac.len(), out.display());
        }
        "pdf-info" => {
            let p = PathBuf::from(arg(args, 1)?);
            println!("쪽수 {}", pdf::page_count(&p)?);
            println!("회전 {:?}", samples::pdf_rotations(&p)?);
        }
        "pdf-merge" => {
            let out = PathBuf::from(arg(args, 1)?);
            let inputs: Vec<PathBuf> = args[2..].iter().map(PathBuf::from).collect();
            let n = pdf::merge(&inputs, &out)?;
            println!("완료: {n}쪽 -> {}", out.display());
        }
        "pdf-split" => {
            let p = PathBuf::from(arg(args, 1)?);
            let mode = match arg(args, 2)? {
                "every" => SplitMode::EveryPage,
                spec => SplitMode::Ranges(spec.to_string()),
            };
            for o in pdf::split(&p, &mode, opt_path(args, 3).as_deref())? {
                println!("만듦  {} ({}쪽)", o.display(), pdf::page_count(&o)?);
            }
        }
        "pdf-rotate" => {
            let p = PathBuf::from(arg(args, 1)?);
            let deg: i64 = arg(args, 3)?.parse().map_err(|_| filetool_core::Error::Invalid("각도는 숫자여야 합니다".into()))?;
            let out = opt_path(args, 4).unwrap_or_else(|| paths::output_path(&p, None, &format!("_회전{deg}"), "pdf"));
            pdf::rotate(&p, arg(args, 2)?, deg, &out)?;
            println!("완료: 회전 {:?} -> {}", samples::pdf_rotations(&out)?, out.display());
        }
        "pdf-delete" => {
            let p = PathBuf::from(arg(args, 1)?);
            let out = opt_path(args, 3).unwrap_or_else(|| paths::output_path(&p, None, "_삭제", "pdf"));
            let n = pdf::delete_pages(&p, arg(args, 2)?, &out)?;
            println!("완료: 남은 {n}쪽 -> {}", out.display());
        }
        "pdf-reorder" => {
            let p = PathBuf::from(arg(args, 1)?);
            let out = opt_path(args, 3).unwrap_or_else(|| paths::output_path(&p, None, "_순서", "pdf"));
            pdf::reorder(&p, arg(args, 2)?, &out)?;
            println!("완료 -> {}", out.display());
        }
        "selftest" => {
            let dir = PathBuf::from(arg(args, 1)?);
            // --only 2a: 검색만 점검
            if args.iter().any(|a| a == "--only") && args.iter().any(|a| a == "2a") {
                let code = search2a::selftest(&dir.join("2a"))?;
                println!("전체: 2a {}", ok_word(code));
                return Ok(code);
            }
            let code_1a = selftest(&dir.join("1a"))?;
            let code_1b = selftest1b::run(&dir.join("1b"), args)?;
            let code_1c = selftest1c::run(&dir.join("1c"), args)?;
            let code_2a = search2a::selftest(&dir.join("2a"))?;
            let all = [code_1a, code_1b, code_1c, code_2a].iter().all(|c| *c == ExitCode::SUCCESS);
            println!("전체: 1a {} / 1b {} / 1c {} / 2a {}", ok_word(code_1a), ok_word(code_1b), ok_word(code_1c), ok_word(code_2a));
            return Ok(if all { ExitCode::SUCCESS } else { ExitCode::from(1) });
        }
        _ => {
            println!("{USAGE}\n{}", runtime030::USAGE);
            return Ok(ExitCode::from(2));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn ok_word(c: ExitCode) -> &'static str {
    if c == ExitCode::SUCCESS { "통과" } else { "실패 있음" }
}

struct Checker {
    failed: usize,
    total: usize,
}

impl Checker {
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        self.total += 1;
        if !ok {
            self.failed += 1;
        }
        println!("{} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
    }
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|it| it.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// 빈 폴더에 샘플을 만들고 모든 작업을 돌려 결과 이름·쪽수·회전·순서를 확인한다.
fn selftest(dir: &Path) -> Result<ExitCode> {
    if dir.exists() && std::fs::read_dir(dir).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(filetool_core::Error::Invalid(format!("비어 있는 새 폴더를 지정하세요: {}", dir.display())));
    }
    std::fs::create_dir_all(dir).map_err(|e| filetool_core::Error::Invalid(e.to_string()))?;
    let mut c = Checker { failed: 0, total: 0 };
    let mk = |p: &Path, b: &[u8]| std::fs::write(p, b).map_err(|e| filetool_core::Error::Invalid(e.to_string()));

    // 1) NFD 폴더
    let f = dir.join("nfd_folder");
    let sub = f.join(nfd("사진"));
    std::fs::create_dir_all(&sub).map_err(|e| filetool_core::Error::Invalid(e.to_string()))?;
    mk(&sub.join(nfd("여행.jpg")), b"jpg")?;
    mk(&f.join(nfd("보고서.txt")), b"r")?;
    mk(&f.join("메모.txt"), b"nfc original")?;
    mk(&f.join(nfd("메모.txt")), b"nfd copy")?;
    let plan = nfc::plan_folder(&f)?;
    c.check("nfc 미리보기", plan.renames.len() == 3 && plan.collisions.len() == 1,
        format!("바꿀 {} / 충돌 {}", plan.renames.len(), plan.collisions.len()));
    let rep = nfc::apply_plan(&plan);
    let names = names_in(&f);
    c.check("nfc 적용", rep.renamed.len() == 3 && rep.failed.is_empty()
        && names.contains(&"사진".into()) && names.contains(&"보고서.txt".into()) && names.contains(&nfd("메모.txt")),
        format!("바꿈 {} / 결과 {:?}", rep.renamed.len(), names));
    let memo = std::fs::read(f.join("메모.txt")).unwrap_or_default();
    c.check("nfc 충돌 시 덮어쓰지 않음", memo == b"nfc original", format!("메모.txt = {:?}", String::from_utf8_lossy(&memo)));
    c.check("nfc 하위 파일", names_in(&f.join("사진")) == vec!["여행.jpg".to_string()], format!("{:?}", names_in(&f.join("사진"))));

    // 2) 맥 zip
    let zip = dir.join("mac.zip");
    let (d, doc, img, j1, j2) = (nfd("자료/"), nfd("자료/한글 문서.txt"), nfd("자료/사진.jpg"), nfd("__MACOSX/자료/._한글 문서.txt"), nfd("자료/._사진.jpg"));
    samples::write_mac_style_zip(&zip, &[(d.as_str(), b""), (doc.as_str(), "본문".as_bytes()), (img.as_str(), b"jpg"), (j1.as_str(), b"x"), (j2.as_str(), b"x")])?;
    let out = zipnfc::default_extract_dir(&zip);
    let rep = zipnfc::extract_nfc(&zip, &out)?;
    let got = names_in(&out.join("자료"));
    c.check("zip 풀기(NFC)", rep.files.len() == 2 && got == vec!["사진.jpg".to_string(), "한글 문서.txt".to_string()] && !out.join("__MACOSX").exists(),
        format!("파일 {} / 맥 부산물 제외 {} / {:?}", rep.files.len(), rep.skipped_mac.len(), got));
    let rz = zipnfc::default_rezip_path(&zip, None);
    let rr = zipnfc::rezip_nfc(&zip, &rz)?;
    c.check("zip 다시 압축(NFC)", rr.entries == 3 && rr.skipped_mac.len() == 2, format!("항목 {} -> {}", rr.entries, rz.display()));

    // 3) PDF
    let a = dir.join("a.pdf");
    let b = dir.join("b.pdf");
    samples::make_pdf(&a, 3, "A")?;
    samples::make_pdf(&b, 2, "B")?;
    let m = dir.join("merged.pdf");
    let n = pdf::merge(&[a.clone(), b.clone()], &m)?;
    let labels = samples::pdf_labels(&m)?;
    c.check("pdf 합치기", n == 5 && labels == ["A p1", "A p2", "A p3", "B p1", "B p2"], format!("{n}쪽 {labels:?}"));
    let parts = pdf::split(&m, &SplitMode::Ranges("1-3,5".into()), None)?;
    let desc: Vec<String> = parts.iter().map(|p| format!("{}={}쪽", p.file_name().unwrap().to_string_lossy(), pdf::page_count(p).unwrap_or(0))).collect();
    c.check("pdf 나누기(1-3,5)", desc == ["merged_p1-3.pdf=3쪽", "merged_p5.pdf=1쪽"], format!("{desc:?}"));
    let each = pdf::split(&b, &SplitMode::EveryPage, None)?;
    c.check("pdf 나누기(한 쪽씩)", each.len() == 2, format!("{} 파일", each.len()));
    let r = dir.join("a_rot.pdf");
    pdf::rotate(&a, "2", 90, &r)?;
    let rot = samples::pdf_rotations(&r)?;
    c.check("pdf 회전(2쪽 90도)", rot == [0, 90, 0], format!("{rot:?}"));
    let del = dir.join("merged_del.pdf");
    let left = pdf::delete_pages(&m, "2,4", &del)?;
    let dl = samples::pdf_labels(&del)?;
    c.check("pdf 페이지 삭제(2,4)", left == 3 && dl == ["A p1", "A p3", "B p2"], format!("{left}쪽 {dl:?}"));
    let re = dir.join("a_re.pdf");
    pdf::reorder(&a, "3,1,2", &re)?;
    let rl = samples::pdf_labels(&re)?;
    c.check("pdf 순서 변경(3,1,2)", rl == ["A p3", "A p1", "A p2"], format!("{rl:?}"));

    println!("결과: {}/{} 통과", c.total - c.failed, c.total);
    Ok(if c.failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) })
}
