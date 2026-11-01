//! 0.3.0 W4 명령: 사진·영상 고치기, zip 풀기·만들기, 이름 한꺼번에 바꾸기, 같은 파일 찾기.
//! 앱(Tauri)과 같은 코어 함수를 부르고 결과를 JSON으로 찍는다(데스크톱 점검·자동화용).
//! JSON 인자는 그대로 주거나 `@파일.json`으로 준다(cmd에서 따옴표 다루기 어려울 때).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use filetool_core::dupes::{self, ScanOpts};
use filetool_core::imgedit::{self, EditOps};
use filetool_core::mediaedit::{self, MediaOps};
use filetool_core::rename::{self, Pair, Rule, SortBy};
use filetool_core::tools::Tool;
use filetool_core::ziptool::{self, NameEnc};
use filetool_core::{samples, Error, Result};
use serde::Serialize;

use crate::conv::{app_settings_dir, toolset, Flags};

pub const USAGE: &str = "0.3.0 사진·영상·파일 정리(결과는 JSON, JSON 인자는 '@파일.json'도 됨):
  filetool-cli image-probe <사진>
  filetool-cli image-edit --ops <JSON> [--out 폴더] <사진...>        ops 예: {\"resize\":{\"width\":1200,\"keep_ratio\":true},\"strip_meta\":true,\"target_kb\":500}
  filetool-cli media-probe <영상>
  filetool-cli media-frame <영상> <초> [--width N] [--save] [--out 폴더]   --save 없으면 JPG 크기만 보여 줌
  filetool-cli media-edit --ops <JSON> [--out 폴더] <영상>          ops 예: {\"trim\":{\"start\":1.5,\"end\":10},\"mute\":true,\"rotate\":90}
  filetool-cli zip-list <zip> [--encoding auto|utf8|cp949]
  filetool-cli zip-unpack <zip> [--encoding auto|utf8|cp949] [--out 폴더]
  filetool-cli zip-create [--output 파일.zip] <파일·폴더...>
  filetool-cli rename-preview --rule <JSON> [--sort name|modified|taken] <파일...>
  filetool-cli rename-apply --pairs <JSON [{from,to}]>               되돌리기 기록은 앱 설정 폴더 rename-undo\\
  filetool-cli rename-undo <undo_id>
  filetool-cli dup-scan [--min-size N] [--hidden] <폴더...>
  filetool-cli dup-trash --root <폴더> [--hidden] <지울 파일...>     폴더를 다시 찾은 뒤 휴지통으로만(모두 지우기는 거절)
  filetool-cli tools-samples <빈 폴더>                               점검용 샘플(GPS·회전 JPEG, 투명 PNG, CP949 zip, 맥 zip, 같은 파일 폴더, 이름 바꾸기 파일)";

fn json_arg(v: Option<&str>, what: &str) -> Result<String> {
    let v = v.ok_or_else(|| Error::Invalid(format!("{what} 값을 주세요")))?;
    match v.strip_prefix('@') {
        // 메모장·PowerShell이 붙이는 BOM은 뺀다
        Some(f) => std::fs::read_to_string(f).map(|s| s.trim_start_matches('\u{feff}').to_string()).map_err(|e| Error::Invalid(format!("{f}을(를) 읽지 못했습니다: {e}"))),
        None => Ok(v.to_string()),
    }
}

fn parse<T: serde::de::DeserializeOwned>(s: &str, what: &str) -> Result<T> {
    serde_json::from_str(s).map_err(|e| Error::Invalid(format!("{what} JSON이 올바르지 않습니다: {e}")))
}

fn print<T: Serialize>(v: &T) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

/// 값 없는 스위치를 뺀 뒤 공용 Flags로
fn split_switches(args: &[String], switches: &[&str]) -> (Vec<String>, Vec<String>) {
    let (on, rest): (Vec<String>, Vec<String>) = args.iter().cloned().partition(|a| switches.contains(&a.as_str()));
    (on, rest)
}

fn enc(f: &Flags) -> Result<NameEnc> {
    Ok(match f.get("--encoding").unwrap_or("auto") {
        "auto" => NameEnc::Auto,
        "utf8" => NameEnc::Utf8,
        "cp949" => NameEnc::Cp949,
        e => return Err(Error::Invalid(format!("--encoding은 auto|utf8|cp949: {e}"))),
    })
}

fn undo_dir() -> Result<PathBuf> {
    app_settings_dir().ok_or_else(|| Error::Invalid("앱 설정 폴더를 찾지 못했습니다(APPDATA)".into()))
}

pub fn run(cmd: &str, args: &[String]) -> Result<Option<ExitCode>> {
    if !matches!(
        cmd,
        "image-probe" | "image-edit" | "media-probe" | "media-frame" | "media-edit" | "zip-list" | "zip-unpack" | "zip-create" | "rename-preview" | "rename-apply" | "rename-undo" | "dup-scan" | "dup-trash" | "tools-samples" | "help-tools"
    ) {
        return Ok(None);
    }
    let (switches, rest) = split_switches(&args[1.min(args.len())..], &["--save", "--hidden"]);
    let has = |s: &str| switches.iter().any(|x| x == s);
    let f = Flags::parse(&rest);
    let out = f.get("--out").map(PathBuf::from);
    let first = || f.rest.first().map(PathBuf::from).ok_or_else(|| Error::Invalid(format!("파일을 지정하세요\n{USAGE}")));
    match cmd {
        "help-tools" => println!("{USAGE}"),
        "image-probe" => print(&imgedit::probe(&first()?)?),
        "image-edit" => {
            let ops: EditOps = parse(&json_arg(f.get("--ops"), "--ops")?, "--ops")?;
            let ts = toolset(&f);
            let mut failed = 0;
            let mut results = Vec::new();
            for p in &f.rest {
                match imgedit::edit(ts.ffmpeg.as_deref(), Path::new(p), &ops, out.as_deref(), None) {
                    Ok(o) => results.push(serde_json::to_value(&o).unwrap_or_default()),
                    Err(e) => {
                        failed += 1;
                        results.push(serde_json::json!({ "input": p, "error": e.to_string() }));
                    }
                }
            }
            print(&results);
            if failed > 0 {
                return Ok(Some(ExitCode::from(1)));
            }
        }
        "media-probe" => {
            let ts = toolset(&f);
            print(&mediaedit::probe(ts.get(Tool::Ffprobe)?, &first()?)?);
        }
        "media-frame" => {
            let src = first()?;
            let at: f64 = f.rest.get(1).and_then(|s| s.parse().ok()).ok_or_else(|| Error::Invalid("시각(초)을 주세요".into()))?;
            let ts = toolset(&f);
            if has("--save") {
                let p = mediaedit::frame_save(ts.get(Tool::Ffmpeg)?, ts.ffprobe.as_deref(), &src, at, out.as_deref(), None)?;
                print(&serde_json::json!({ "saved": p }));
            } else {
                let width = f.get("--width").and_then(|w| w.parse().ok());
                let dur = ts.ffprobe.as_deref().and_then(|fp| mediaedit::probe(fp, &src).ok()).map(|i| i.duration);
                let jpg = mediaedit::frame_jpeg(ts.get(Tool::Ffmpeg)?, &src, at, width, dur, None)?;
                let (w, h) = image::load_from_memory(&jpg).map(|i| (i.width(), i.height())).unwrap_or((0, 0));
                print(&serde_json::json!({ "jpg_bytes": jpg.len(), "base64_len": jpg.len().div_ceil(3) * 4, "width": w, "height": h }));
            }
        }
        "media-edit" => {
            let ops: MediaOps = parse(&json_arg(f.get("--ops"), "--ops")?, "--ops")?;
            let ts = toolset(&f);
            let src = first()?;
            let t0 = std::time::Instant::now();
            let o = mediaedit::edit(ts.get(Tool::Ffmpeg)?, ts.get(Tool::Ffprobe)?, &src, &ops, out.as_deref(), None, &mut |_| {})?;
            let mut v = serde_json::to_value(&o).unwrap_or_default();
            v["seconds"] = serde_json::json!((t0.elapsed().as_secs_f64() * 10.0).round() / 10.0);
            print(&v);
        }
        "zip-list" => print(&ziptool::list(&first()?, enc(&f)?)?),
        "zip-unpack" => print(&ziptool::extract(&first()?, enc(&f)?, out.as_deref(), None, &mut |_, _, _| {})?),
        "zip-create" => {
            let inputs: Vec<PathBuf> = f.rest.iter().map(PathBuf::from).collect();
            print(&ziptool::create(&inputs, f.get("--output").map(Path::new), None, &mut |_, _, _| {})?);
        }
        "rename-preview" => {
            let rule: Rule = parse(&json_arg(f.get("--rule"), "--rule")?, "--rule")?;
            let sort = match f.get("--sort").unwrap_or("name") {
                "modified" => SortBy::Modified,
                "taken" => SortBy::Taken,
                _ => SortBy::Name,
            };
            let files: Vec<PathBuf> = f.rest.iter().map(PathBuf::from).collect();
            print(&rename::preview(&files, &rule, sort)?);
        }
        "rename-apply" => {
            let pairs: Vec<Pair> = parse(&json_arg(f.get("--pairs"), "--pairs")?, "--pairs")?;
            print(&rename::apply(&pairs, &undo_dir()?)?);
        }
        "rename-undo" => {
            let id = f.rest.first().ok_or_else(|| Error::Invalid("되돌리기 번호를 주세요".into()))?;
            print(&rename::undo(id, &undo_dir()?)?);
        }
        "dup-scan" => {
            let roots: Vec<PathBuf> = f.rest.iter().map(PathBuf::from).collect();
            let opts = ScanOpts { min_size: f.num("--min-size", 1u64)?, include_hidden: has("--hidden") };
            let t0 = std::time::Instant::now();
            let s = dupes::scan(&roots, &opts, None, &mut |_, _, _, _| {})?;
            let mut v = serde_json::to_value(&s).unwrap_or_default();
            v["seconds"] = serde_json::json!((t0.elapsed().as_secs_f64() * 100.0).round() / 100.0);
            print(&v);
        }
        "dup-trash" => {
            let root = PathBuf::from(f.get("--root").ok_or_else(|| Error::Invalid("--root 폴더를 주세요".into()))?);
            let s = dupes::scan(&[root], &ScanOpts { min_size: 1, include_hidden: has("--hidden") }, None, &mut |_, _, _, _| {})?;
            let paths: Vec<PathBuf> = f.rest.iter().map(PathBuf::from).collect();
            let r = dupes::trash(&paths, &s.groups, &mut |p| dupes::recycle(p))?;
            print(&r);
            if !r.failed.is_empty() {
                return Ok(Some(ExitCode::from(1)));
            }
        }
        "tools-samples" => {
            let d = first()?;
            make_samples(&d)?;
            println!("샘플을 만들었습니다: {}", d.display());
        }
        _ => unreachable!(),
    }
    Ok(Some(ExitCode::SUCCESS))
}

fn w(p: &Path, b: &[u8]) -> Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| Error::Invalid(e.to_string()))?;
    }
    std::fs::write(p, b).map_err(|e| Error::Invalid(format!("{}: {e}", p.display())))
}

/// 점검용 샘플(ffmpeg 없이 만들 수 있는 것)
fn make_samples(d: &Path) -> Result<()> {
    if d.exists() && std::fs::read_dir(d).map(|mut x| x.next().is_some()).unwrap_or(true) {
        return Err(Error::Invalid(format!("비어 있는 새 폴더를 지정하세요: {}", d.display())));
    }
    use image::ImageEncoder;
    // 1) GPS + 회전 6 JPEG(저장 4000×3000 → 보이는 3000×4000)
    let img = samples::photo_like(4000, 3000, 11);
    let mut buf = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 92);
    enc.set_exif_metadata(filetool_core::exif::build_sample_gps("2024:05:01 10:20:30", 6, 37, 127)).map_err(|e| Error::Invalid(e.to_string()))?;
    enc.write_image(img.as_raw(), 4000, 3000, image::ExtendedColorType::Rgb8).map_err(|e| Error::Invalid(e.to_string()))?;
    w(&d.join("사진/여행_GPS.jpg"), &buf)?;
    // 2) 투명 PNG
    let png = image::RgbaImage::from_fn(800, 400, |x, y| if x < 400 { image::Rgba([0, 0, 0, 0]) } else { image::Rgba([(x % 256) as u8, (y % 256) as u8, 200, 255]) });
    let mut pb = Vec::new();
    png.write_to(&mut std::io::Cursor::new(&mut pb), image::ImageFormat::Png).map_err(|e| Error::Invalid(e.to_string()))?;
    w(&d.join("사진/로고_투명.png"), &pb)?;
    // 3) 윈도 탐색기식 CP949 zip, 맥 UTF-8 zip
    std::fs::create_dir_all(d.join("zip")).map_err(|e| Error::Invalid(e.to_string()))?;
    samples::write_windows_cp949_zip(&d.join("zip/윈도_자료.zip"), &[("보고서/", b""), ("보고서/1분기 실적.txt", "1분기 실적 내용".as_bytes()), ("사진 모음/가족.jpg", b"jpg"), ("../밖으로.txt", b"x")])?;
    let (a, b, j) = (samples::nfd("맥 폴더/"), samples::nfd("맥 폴더/한글 문서.txt"), samples::nfd("__MACOSX/맥 폴더/._한글 문서.txt"));
    samples::write_mac_style_zip(&d.join("zip/맥_자료.zip"), &[(a.as_str(), b""), (b.as_str(), "맥에서 만든 문서".as_bytes()), (j.as_str(), b"x"), ("맥 폴더/.DS_Store", b"x")])?;
    w(&d.join("zip/묶을 폴더/하위/메모.txt"), "메모".as_bytes())?;
    w(&d.join("zip/묶을 폴더/표.csv"), "a,b\n1,2\n".as_bytes())?;
    // 4) 같은 파일 폴더(같은 크기 다른 내용 포함)
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let mut big2 = big.clone();
    big2[150_000] ^= 0xFF;
    w(&d.join("같은파일/원본/영상.bin"), &big)?;
    w(&d.join("같은파일/백업/영상 복사본.bin"), &big)?;
    w(&d.join("같은파일/백업/다른 영상.bin"), &big2)?;
    w(&d.join("같은파일/원본/메모.txt"), b"same small")?;
    w(&d.join("같은파일/백업/메모 (1).txt"), b"same small")?;
    w(&d.join("같은파일/원본/x.txt"), b"abcdefghij")?;
    // 5) 이름 바꾸기
    for (n, c) in [("a.txt", "A"), ("b.txt", "B"), ("IMG_1.JPG", "1"), ("IMG_2.JPG", "2"), ("IMG_10.JPG", "10")] {
        w(&d.join("이름").join(n), c.as_bytes())?;
    }
    Ok(())
}
