//! selftest 1b: 이미지 왕복, HEIC(grid, ffmpeg로 만든 샘플), 영상 프리셋(ffprobe 확인), 취소,
//! 문서(LibreOffice가 있으면; Apple 형식은 `--apple` 폴더의 실파일), 감시 폴더.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use filetool_core::convert::{self, Op};
use filetool_core::docs::DocTarget;
use filetool_core::imageconv::{ImageOpts, ImgFormat};
use filetool_core::media::{self, MediaPreset};
use filetool_core::process::Cancel;
use filetool_core::tools::{self, SearchEnv, Tool, Toolset};
use filetool_core::watch::{WatchRule, Watcher};
use filetool_core::{exif, pdf, samples, Error, Result};

use crate::conv::{tool_paths, Flags};

struct Checker {
    failed: usize,
    total: usize,
    skipped: Vec<String>,
}

impl Checker {
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        self.total += 1;
        if !ok {
            self.failed += 1;
        }
        println!("{} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
    }
    fn result<T>(&mut self, name: &str, r: Result<T>, f: impl FnOnce(T) -> (bool, String)) {
        match r {
            Ok(v) => {
                let (ok, d) = f(v);
                self.check(name, ok, d)
            }
            Err(e) => self.check(name, false, format!("오류: {e}")),
        }
    }
    fn skip(&mut self, name: &str, why: &str) {
        println!("SKIP {name}: {why}");
        self.skipped.push(name.into());
    }
}

fn io(e: std::io::Error) -> Error {
    Error::Invalid(e.to_string())
}

fn img(format: ImgFormat) -> ImageOpts {
    ImageOpts { format, quality: 90, max_side: None, keep_exif: true }
}

fn fname(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

pub fn run(dir: &Path, args: &[String]) -> Result<ExitCode> {
    std::fs::create_dir_all(dir).map_err(io)?;
    let f = Flags::parse(&args[1.min(args.len())..]);
    let ts = Toolset::discover(&tool_paths(&f), &SearchEnv::from_system());
    let mut c = Checker { failed: 0, total: 0, skipped: vec![] };
    println!("--- 1b ---");

    // 0) 도구
    for st in tools::status_all(&tool_paths(&f), &SearchEnv::from_system()) {
        println!(
            "도구  {:?}: {}",
            st.tool,
            match &st.path {
                Some(p) => format!("{} ({}, {})", p.display(), st.source.unwrap_or(""), st.version.as_deref().unwrap_or("버전 확인 실패")),
                None => "없음".into(),
            }
        );
    }

    // 1) 이미지 왕복
    let src = dir.join("사진.png");
    samples::quadrant_image(120, 80).save(&src).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut prev = src.clone();
    let mut chain = Vec::new();
    let mut ok = true;
    for fm in [ImgFormat::Jpg, ImgFormat::Webp, ImgFormat::Bmp, ImgFormat::Tiff, ImgFormat::Gif, ImgFormat::Png] {
        match convert::convert_file(&ts, &prev, None, &Op::Image(img(fm)), None, &mut |_| {}) {
            Ok(o) => {
                let im = image::open(&o.output).map_err(|e| Error::Invalid(e.to_string()))?;
                let q = samples::quadrant_colors(&im);
                ok &= (im.width(), im.height()) == (120, 80) && q == ["빨강", "초록", "파랑", "흰색"];
                chain.push(format!("{}={}x{}", fname(&o.output), im.width(), im.height()));
                prev = o.output;
            }
            Err(e) => {
                ok = false;
                chain.push(format!("오류 {e}"));
                break;
            }
        }
    }
    c.check("이미지 왕복 png→jpg→webp→bmp→tif→gif→png", ok && fname(&prev) == "사진 (1).png", chain.join(" → "));
    c.result("이미지 품질·긴 변 (jpg q40, 최대 60px)", convert::convert_file(&ts, &src, Some(&dir.join("작게")), &Op::Image(ImageOpts { quality: 40, max_side: Some(60), ..img(ImgFormat::Jpg) }), None, &mut |_| {}), |o| {
        (o.note.starts_with("60×40"), format!("{} {}", fname(&o.output), o.note))
    });
    let ej = dir.join("세로사진.jpg");
    samples::make_jpeg_with_exif(&ej, 120, 80, "2024:05:01 10:20:30", 6)?;
    c.result("JPG EXIF 회전 6·촬영 날짜", convert::convert_file(&ts, &ej, Some(&dir.join("exif")), &Op::Image(img(ImgFormat::Jpg)), None, &mut |_| {}), |o| {
        let b = std::fs::read(&o.output).unwrap_or_default();
        let e = exif::from_jpeg(&b);
        let im = image::open(&o.output).ok();
        let dims = im.as_ref().map(|i| (i.width(), i.height()));
        let q = im.as_ref().map(samples::quadrant_colors);
        let ok = dims == Some((80, 120))
            && e.as_deref().and_then(exif::date_taken).as_deref() == Some("2024:05:01 10:20:30")
            && e.as_deref().and_then(exif::orientation) == Some(1)
            && q == Some(["파랑", "빨강", "흰색", "초록"]);
        (ok, format!("{dims:?} 사분면 {q:?} EXIF 날짜 {:?} 회전값 {:?}", e.as_deref().and_then(exif::date_taken), e.as_deref().and_then(exif::orientation)))
    });

    // 2) HEIC (ffmpeg 필요)
    match ts.ffmpeg.as_deref() {
        None => c.skip("HEIC", "ffmpeg 없음"),
        Some(ff) => {
            let heic = dir.join("IMG_0001.HEIC");
            match samples::make_grid_heic(ff, &heic, 512, 1000, 700, "2023:08:15 14:03:22") {
                Err(e) => c.check("HEIC 샘플 만들기(미리 만든 HEVC 타일 2×2 grid)", false, e.to_string()),
                Ok(()) => {
                    c.check("HEIC 샘플 만들기(미리 만든 HEVC 타일 2×2 grid)", true, format!("{} bytes, 타일 512px, grid 1000×700, irot 90°, EXIF 회전 6", std::fs::metadata(&heic).map(|m| m.len()).unwrap_or(0)));
                    c.result("HEIC → JPG (grid 합치기·회전·EXIF 날짜)", convert::convert_file(&ts, &heic, None, &Op::Image(img(ImgFormat::Jpg)), None, &mut |_| {}), |o| {
                        let im = image::open(&o.output).ok();
                        let dims = im.as_ref().map(|i| (i.width(), i.height()));
                        let q = im.as_ref().map(samples::quadrant_colors);
                        let e = exif::from_jpeg(&std::fs::read(&o.output).unwrap_or_default());
                        let date = e.as_deref().and_then(exif::date_taken);
                        let ok = dims == Some((700, 1000)) && q == Some(["초록", "흰색", "빨강", "파랑"]) && date.as_deref() == Some("2023:08:15 14:03:22") && e.as_deref().and_then(exif::orientation) == Some(1);
                        (ok, format!("{} {dims:?} 사분면 {q:?} EXIF 날짜 {date:?}", fname(&o.output)))
                    });
                    c.result("HEIC → PNG (긴 변 500)", convert::convert_file(&ts, &heic, None, &Op::Image(ImageOpts { max_side: Some(500), ..img(ImgFormat::Png) }), None, &mut |_| {}), |o| {
                        (o.note.starts_with("350×500"), format!("{} {}", fname(&o.output), o.note))
                    });
                }
            }
        }
    }

    // 3) 영상·소리
    match (ts.ffmpeg.as_deref(), ts.ffprobe.as_deref()) {
        (Some(ff), Some(fp)) => {
            let probe = |p: &Path| media::probe_streams(fp, p).join(" | ");
            let vdir = dir.join("영상");
            std::fs::create_dir_all(&vdir).map_err(io)?;
            let mov = vdir.join("클립.mov");
            samples::make_test_video(ff, &mov, 3, 640, 360)?;
            let mut ticks: Vec<f64> = Vec::new();
            c.result("MOV → MP4 (H.264/AAC, 진행률)", convert::convert_file(&ts, &mov, None, &Op::Media(MediaPreset::Mp4), None, &mut |x| ticks.push(x)), |o| {
                let i = probe(&o.output);
                let mid = ticks.iter().filter(|x| **x > 0.0 && **x < 1.0).count();
                (i.contains("codec_name=h264") && i.contains("codec_name=aac") && ticks.last() == Some(&1.0), format!("{} [{i}] 진행 보고 {}회(중간값 {mid}회)", fname(&o.output), ticks.len()))
            });
            for ext in ["mkv", "avi", "webm"] {
                let v = vdir.join(format!("클립_{ext}.{ext}"));
                match samples::make_test_video(ff, &v, 2, 320, 240) {
                    Err(e) => c.check(&format!("{} 샘플", ext.to_uppercase()), false, e.to_string()),
                    Ok(()) => c.result(&format!("{} → MP4", ext.to_uppercase()), convert::convert_file(&ts, &v, None, &Op::Media(MediaPreset::Mp4), None, &mut |_| {}), |o| {
                        let i = probe(&o.output);
                        (i.contains("codec_name=h264") && i.contains("codec_name=aac"), format!("{} [{i}]", fname(&o.output)))
                    }),
                }
            }
            c.result("메신저용 줄이기 (1MB, 240p)", convert::convert_file(&ts, &mov, None, &Op::Media(MediaPreset::Messenger { max_mb: 1, max_height: 240 }), None, &mut |_| {}), |o| {
                let i = probe(&o.output);
                let n = std::fs::metadata(&o.output).map(|m| m.len()).unwrap_or(u64::MAX);
                (i.contains("height=240") && n <= 1024 * 1024, format!("{} {n} bytes [{i}]", fname(&o.output)))
            });
            c.result("소리만 MP3", convert::convert_file(&ts, &mov, None, &Op::Media(MediaPreset::Mp3), None, &mut |_| {}), |o| {
                let i = probe(&o.output);
                (i.contains("codec_name=mp3") && !i.contains("codec_type=video"), format!("{} [{i}]", fname(&o.output)))
            });
            c.result("소리만 M4A", convert::convert_file(&ts, &mov, None, &Op::Media(MediaPreset::M4a), None, &mut |_| {}), |o| {
                let i = probe(&o.output);
                (i.contains("codec_name=aac") && !i.contains("codec_type=video"), format!("{} [{i}]", fname(&o.output)))
            });
            c.result("짧은 GIF (0.5초부터 1.5초, 가로 200)", convert::convert_file(&ts, &mov, None, &Op::Media(MediaPreset::Gif { start: 0.5, duration: 1.5, width: 200, fps: 10 }), None, &mut |_| {}), |o| {
                let i = probe(&o.output);
                (i.contains("codec_name=gif") && i.contains("width=200"), format!("{} [{i}]", fname(&o.output)))
            });
            // 취소: 긴 영상 변환을 1.5초 뒤 취소 → 출력 파일 없음
            let long = vdir.join("긴영상.mkv");
            samples::make_test_video(ff, &long, 40, 1280, 720)?;
            let cancel = Cancel::new();
            let c2 = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1500));
                c2.cancel();
            });
            let t0 = Instant::now();
            let before: Vec<String> = std::fs::read_dir(&vdir).map_err(io)?.filter_map(|e| e.ok()).map(|e| fname(&e.path())).collect();
            let r = convert::convert_file(&ts, &long, None, &Op::Media(MediaPreset::Mp4), Some(&cancel), &mut |_| {});
            let after: Vec<String> = std::fs::read_dir(&vdir).map_err(io)?.filter_map(|e| e.ok()).map(|e| fname(&e.path())).collect();
            let el = t0.elapsed().as_secs_f64();
            c.check("변환 중 취소(ffmpeg 종료, 만들던 파일 삭제)", matches!(r, Err(Error::Cancelled)) && before == after && el < 6.0, format!("{:?} {el:.1}초, 남은 새 파일 {:?}", r.as_ref().err().map(|e| e.to_string()), after.iter().filter(|a| !before.contains(a)).collect::<Vec<_>>()));
        }
        _ => c.skip("영상·소리", "ffmpeg/ffprobe 없음"),
    }

    // 4) 문서
    match ts.get(Tool::Soffice).ok() {
        None => c.skip("문서", "LibreOffice 없음 — 문서 변환은 Windows에서 확인"),
        Some(_) => docs_checks(&mut c, &ts, dir, f.get("--apple").map(PathBuf::from))?,
    }

    // 5) 감시 폴더
    watch_checks(&mut c, &ts, dir)?;

    println!("결과(1b): {}/{} 통과{}", c.total - c.failed, c.total, if c.skipped.is_empty() { String::new() } else { format!(", 건너뜀 {:?}", c.skipped) });
    Ok(if c.failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

const FODT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.text">
<office:body><office:text><text:p>한글 문서 변환 시험 HANGUL-TEST</text:p></office:text></office:body></office:document>"#;
const FODS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.spreadsheet">
<office:body><office:spreadsheet><table:table table:name="시트1"><table:table-row><table:table-cell office:value-type="string"><text:p>품목</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="42"><text:p>42</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document>"#;
const FODP: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.presentation">
<office:body><office:presentation>
<draw:page draw:name="p1"><draw:frame svg:x="2cm" svg:y="2cm" svg:width="20cm" svg:height="3cm"><draw:text-box><text:p>슬라이드 하나</text:p></draw:text-box></draw:frame></draw:page>
<draw:page draw:name="p2"><draw:frame svg:x="2cm" svg:y="2cm" svg:width="20cm" svg:height="3cm"><draw:text-box><text:p>슬라이드 둘</text:p></draw:text-box></draw:frame></draw:page>
</office:presentation></office:body></office:document>"#;

fn zip_has(p: &Path, entry: &str, needle: &str) -> bool {
    let Ok(f) = std::fs::File::open(p) else { return false };
    let Ok(mut z) = zip::ZipArchive::new(f) else { return false };
    let Ok(mut e) = z.by_name(entry) else { return false };
    let mut s = String::new();
    use std::io::Read;
    e.read_to_string(&mut s).is_ok() && s.contains(needle)
}

fn docs_checks(c: &mut Checker, ts: &Toolset, dir: &Path, apple: Option<PathBuf>) -> Result<()> {
    let d = dir.join("문서");
    std::fs::create_dir_all(&d).map_err(io)?;
    let user_lo = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("LibreOffice"));
    let user_lo_before = user_lo.as_ref().map(|p| p.exists());
    let (t, s, p) = (d.join("보고서.fodt"), d.join("표.fods"), d.join("발표.fodp"));
    std::fs::write(&t, FODT).map_err(io)?;
    std::fs::write(&s, FODS).map_err(io)?;
    std::fs::write(&p, FODP).map_err(io)?;
    let doc = |input: &Path, target: DocTarget| convert::convert_file(ts, input, None, &Op::Doc { target }, None, &mut |_| {});

    // 두 작업을 동시에(개인 프로필이 겹치지 않는지)
    let t0 = Instant::now();
    let (ts2, t2, s2) = (ts.clone(), t.clone(), s.clone());
    let h = std::thread::spawn(move || convert::convert_file(&ts2, &t2, None, &Op::Doc { target: DocTarget::Docx }, None, &mut |_| {}));
    let xlsx = convert::convert_file(ts, &s2, None, &Op::Doc { target: DocTarget::Xlsx }, None, &mut |_| {});
    let docx = h.join().unwrap_or_else(|_| Err(Error::Invalid("스레드 오류".into())));
    let el = t0.elapsed().as_secs_f64();
    c.result("ODT(fodt) → DOCX (동시 실행 1)", docx.as_ref().map(|o| o.output.clone()).map_err(|e| Error::Invalid(e.to_string())), |o| {
        (zip_has(&o, "word/document.xml", "HANGUL-TEST") && zip_has(&o, "word/document.xml", "한글"), format!("{} (두 작업 {el:.1}초)", fname(&o)))
    });
    c.result("ODS(fods) → XLSX (동시 실행 2)", xlsx.as_ref().map(|o| o.output.clone()).map_err(|e| Error::Invalid(e.to_string())), |o| {
        (zip_has(&o, "xl/workbook.xml", "시트1"), fname(&o))
    });
    c.result("ODP(fodp) → PPTX", doc(&p, DocTarget::Pptx), |o| (zip_has(&o.output, "ppt/presentation.xml", "sldId"), fname(&o.output)));
    for (name, input) in [("DOCX", docx.ok().map(|o| o.output)), ("XLSX", xlsx.ok().map(|o| o.output)), ("PPTX", d.join("발표.pptx").is_file().then(|| d.join("발표.pptx")))] {
        let Some(input) = input else {
            c.check(&format!("{name} → PDF"), false, "입력이 만들어지지 않음".into());
            continue;
        };
        c.result(&format!("{name} → PDF"), doc(&input, DocTarget::Pdf), |o| {
            let n = pdf::page_count(&o.output).unwrap_or(0);
            let want = if name == "PPTX" { 2 } else { 1 };
            (n == want, format!("{} {n}쪽", fname(&o.output)))
        });
    }
    // 같은 이름이 있으면 (1)
    // 보고서.pdf는 위 DOCX → PDF에서 이미 생겼으므로 (1), (2) 순서
    let first = doc(&t, DocTarget::Pdf).map(|o| fname(&o.output));
    let second = doc(&t, DocTarget::Pdf).map(|o| fname(&o.output));
    c.check("같은 이름 → (1), (2) (덮어쓰지 않음)", first.as_deref().ok() == Some("보고서 (1).pdf") && second.as_deref().ok() == Some("보고서 (2).pdf") && d.join("보고서.pdf").is_file(), format!("{first:?}, {second:?}"));
    let user_lo_after = user_lo.as_ref().map(|p| p.exists());
    c.check("사용자 LibreOffice 프로필 건드리지 않음", user_lo_before == user_lo_after, format!("%APPDATA%\\LibreOffice 있음: 전 {user_lo_before:?} / 후 {user_lo_after:?}"));
    let leftovers = std::fs::read_dir(std::env::temp_dir().join("win-filetool")).map(|it| it.filter_map(|e| e.ok()).filter(|e| fname(&e.path()).starts_with(&format!("lo-{}-", std::process::id()))).count()).unwrap_or(0);
    c.check("작업용 임시 프로필 정리", leftovers == 0, format!("남은 임시 폴더 {leftovers}개"));

    // Apple 실파일
    match apple {
        None => c.skip("Pages·Numbers·Keynote", "--apple 폴더 없음 (Apple 형식 실파일 미확인)"),
        Some(a) => {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&a).map_err(io)?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("pages" | "numbers" | "key"))).collect();
            files.sort();
            let out = d.join("apple");
            for f in files {
                for target in filetool_core::docs::allowed_targets(&f) {
                    c.result(&format!("{} → {}", fname(&f), target.ext().to_uppercase()), convert::convert_file(ts, &f, Some(&out), &Op::Doc { target }, None, &mut |_| {}), |o| {
                        let ok = match target {
                            DocTarget::Pdf => pdf::page_count(&o.output).map(|n| n >= 1).unwrap_or(false),
                            DocTarget::Docx => zip_has(&o.output, "word/document.xml", "w:body"),
                            DocTarget::Xlsx => zip_has(&o.output, "xl/workbook.xml", "sheet"),
                            DocTarget::Pptx => zip_has(&o.output, "ppt/presentation.xml", "sldId"),
                        };
                        let pages = if target == DocTarget::Pdf { format!(" {}쪽", pdf::page_count(&o.output).unwrap_or(0)) } else { String::new() };
                        (ok, format!("{} {}{pages}", fname(&o.output), o.note))
                    });
                }
            }
        }
    }
    Ok(())
}

fn watch_checks(c: &mut Checker, ts: &Toolset, dir: &Path) -> Result<()> {
    let inbox = dir.join("감시");
    std::fs::create_dir_all(&inbox).map_err(io)?;
    let state = dir.join("watch-state.json");
    let rule = WatchRule { id: "selftest".into(), folder: inbox.clone(), op: Op::Image(img(ImgFormat::Jpg)), out_subfolder: "변환됨".into(), enabled: true };
    let rules = [rule.clone()];
    let mut w = Watcher::new(state.clone(), Duration::from_secs(2));
    // 복사 중인 파일 흉내: 앞부분만 쓰고, 1초 뒤 나머지(안정 대기 2초보다 짧은 간격으로 계속 바뀜)
    let mut bytes = Vec::new();
    samples::quadrant_image(300, 200).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).map_err(|e| Error::Invalid(e.to_string()))?;
    let a = inbox.join("새 사진.png");
    std::fs::write(&a, &bytes[..bytes.len() / 3]).map_err(io)?;
    let mut extra = vec![];
    if let Some(ff) = ts.ffmpeg.as_deref() {
        let h = inbox.join("아이폰.heic");
        if samples::make_grid_heic(ff, &h, 256, 500, 400, "2023:08:15 14:03:22").is_ok() {
            extra.push(h);
        }
    }
    std::fs::write(inbox.join("메모.txt"), b"not an image").map_err(io)?;
    let mut events = Vec::new();
    let mut early = 0;
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(12) {
        let ev = w.run_once(&rules, ts, None, Instant::now());
        if t0.elapsed() < Duration::from_secs(2) {
            early += ev.iter().filter(|e| e.input == a).count();
        }
        events.extend(ev);
        if t0.elapsed() >= Duration::from_secs(1) && std::fs::metadata(&a).map(|m| m.len()).unwrap_or(0) < bytes.len() as u64 {
            std::fs::write(&a, &bytes).map_err(io)?; // 복사 끝
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let outs = |p: &Path| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(p).map(|it| it.filter_map(|e| e.ok()).map(|e| fname(&e.path())).collect()).unwrap_or_default();
        v.sort();
        v
    };
    let out_dir = rule.out_dir();
    let want = 1 + extra.len();
    c.check(
        "감시 폴더: 복사 중엔 기다렸다가 변환",
        early == 0 && events.len() == want && events.iter().all(|e| e.ok),
        format!("복사 중 처리 {early}회, 처리 {}건 {:?} → {:?}", events.len(), events.iter().map(|e| format!("{}:{}", fname(&e.input), if e.ok { "성공" } else { "실패" })).collect::<Vec<_>>(), outs(&out_dir)),
    );
    // 다시 처리하지 않음(같은 감시기 / 다시 시작한 감시기)
    let mut again = 0;
    for _ in 0..4 {
        again += w.run_once(&rules, ts, None, Instant::now()).len();
        std::thread::sleep(Duration::from_millis(1000));
    }
    let mut w2 = Watcher::new(state.clone(), Duration::from_secs(2));
    for _ in 0..4 {
        again += w2.run_once(&rules, ts, None, Instant::now()).len();
        std::thread::sleep(Duration::from_millis(1000));
    }
    let o = outs(&out_dir);
    c.check("감시 폴더: 한 번만 처리(앱 다시 시작 포함)", again == 0 && o.len() == want, format!("추가 처리 {again}건, 출력 {o:?}"));
    Ok(())
}
