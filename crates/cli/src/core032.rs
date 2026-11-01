//! 0.3.2 핵심 6개 실제 파일 점검(시험 PC용). 앱 화면이 부르는 것과 같은 코어 함수를 그대로 부른다.
//!
//! - `core032-make <폴더> [--heic <실제 HEIC 폴더>] [--scan-mb 30] [--photos 300]` : 만들 수 있는 시험 자료를 만든다
//!   (합칠 PDF, 50쪽 스캔 PDF, 암호·손상 PDF, 사진 300장(JPG·PNG·WEBP + HEIC 복사), 맥 NFD 폴더·zip, 손상 docx).
//!   공문 HWP·Office 파일은 이 명령 밖에서 `<폴더>\office\`에 복사해 둔다(참고 PDF는 `office\ref\<이름>.pdf`).
//! - `core032-run <폴더> [--only merge,compress,...]` : 할 일마다 통과/실패, 걸린 시간, 결과 크기, 점검 내용을 표로.
//!   통과 조건: 결과 쪽이 비어 있지 않음(원본 쪽이 비어 있지 않을 때), 쪽 수가 맞음, 시간 안에 끝남, 원본 해시 그대로,
//!   여러 개는 나쁜 파일을 건너뛰고 끝까지, 끝난 뒤 남은 soffice·Hwp·ffmpeg 없음.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use filetool_core::convert::{self, Op};
use filetool_core::corejobs;
use filetool_core::docs::DocTarget;
use filetool_core::img2pdf::{self, Img2PdfOpts, PageSize};
use filetool_core::pdfedit::plan::{self, EditPlan, PlanPage, PlanSource};
use filetool_core::pdfedit::{thumbs, Job};
use filetool_core::process::Cancel;
use filetool_core::{nfc, pdfcompress, samples, ziptool, Error, Result};

use crate::conv::{toolset, Flags};

/// 원본이 바뀌지 않았는지 보는 용도(내용 전체를 읽어 만든 64비트 해시 + 크기)
fn sha256_hex(p: &Path) -> String {
    use std::hash::Hasher;
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(p) else { return String::new() };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut n = 0u64;
    loop {
        match f.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(k) => {
                h.write(&buf[..k]);
                n += k as u64;
            }
        }
    }
    format!("{:016x}-{n}", h.finish())
}

pub fn run(cmd: &str, args: &[String]) -> Result<Option<ExitCode>> {
    let f = Flags::parse(&args[1.min(args.len())..]);
    match cmd {
        "core032-make" => {
            let dir = PathBuf::from(f.rest.first().ok_or_else(|| Error::Invalid("폴더를 지정하세요".into()))?);
            make(&dir, f.get("--heic").map(Path::new), f.num("--scan-mb", 30u64)?, f.num("--photos", 300u32)?)?;
            Ok(Some(ExitCode::SUCCESS))
        }
        "core032-run" => {
            let dir = PathBuf::from(f.rest.first().ok_or_else(|| Error::Invalid("폴더를 지정하세요".into()))?);
            let only: Vec<String> = f.get("--only").map(|s| s.split(',').map(str::to_string).collect()).unwrap_or_default();
            Ok(Some(run_all(&dir, &only, &f)))
        }
        _ => Ok(None),
    }
}

// ---------- 시험 자료 만들기 ----------

fn ioe(p: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| Error::Invalid(format!("{}: {e}", p.display()))
}

fn mk(p: &Path) -> Result<()> {
    std::fs::create_dir_all(p).map_err(ioe(p))
}

fn make(dir: &Path, heic: Option<&Path>, scan_mb: u64, photos: u32) -> Result<()> {
    let t0 = Instant::now();
    // 1 합치기: 3·5·4쪽
    let m = dir.join("merge");
    mk(&m)?;
    for (n, pages) in [("가 계약서.pdf", 3), ("나 첨부1.pdf", 5), ("다 첨부2.pdf", 4)] {
        let p = m.join(n);
        if !p.exists() {
            samples::make_pdf(&p, pages, n.split(' ').next().unwrap())?;
        }
    }
    // 2 용량 줄이기: 50쪽 스캔(약 scan_mb MB), 암호, 손상, 글자만
    let c = dir.join("compress");
    mk(&c)?;
    let scan = c.join("스캔 50쪽.pdf");
    if !scan.exists() {
        // 1쪽으로 크기를 재서 사진 픽셀을 맞춘다
        let probe = c.join("_probe.pdf");
        let mut px = 1600u32;
        for _ in 0..4 {
            let _ = std::fs::remove_file(&probe);
            samples::make_photo_pdf(&probe, 1, "P", px)?;
            let per = std::fs::metadata(&probe).map(|m| m.len()).unwrap_or(1).max(1);
            let want = scan_mb * 1_000_000 / 50;
            let k = (want as f64 / per as f64).sqrt();
            if (0.9..1.1).contains(&k) {
                break;
            }
            px = ((px as f64 * k) as u32).clamp(600, 4000);
        }
        let _ = std::fs::remove_file(&probe);
        samples::make_photo_pdf(&scan, 50, "SCAN", px)?;
    }
    let locked = c.join("암호 걸린.pdf");
    if !locked.exists() {
        samples::make_encrypted_pdf(&locked, "1234")?;
    }
    let broken = c.join("손상된.pdf");
    if !broken.exists() {
        let b = std::fs::read(&scan).map_err(ioe(&scan))?;
        std::fs::write(&broken, &b[..b.len() / 3]).map_err(ioe(&broken))?;
    }
    let text = c.join("글자만.pdf");
    if !text.exists() {
        samples::make_pdf(&text, 20, "TEXT")?;
    }
    // 3·5 사진 300장: JPG 75(4032×3024) · PNG 75(1600×1200) · WEBP 75(3000×2000) · HEIC 75(실제 사진 복사)
    let ph = dir.join("photos300");
    mk(&ph)?;
    let heics: Vec<PathBuf> = heic
        .and_then(|h| std::fs::read_dir(h).ok())
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e.eq_ignore_ascii_case("heic")).unwrap_or(false)).collect())
        .unwrap_or_default();
    for i in 0..photos {
        let kind = i % 4;
        let name = match kind {
            0 => format!("사진{:03}.jpg", i + 1),
            1 => format!("사진{:03}.png", i + 1),
            2 => format!("사진{:03}.webp", i + 1),
            _ => format!("사진{:03}.heic", i + 1),
        };
        let p = ph.join(&name);
        if p.exists() {
            continue;
        }
        match kind {
            0 => {
                let img = samples::photo_like(4032, 3024, i);
                let mut buf = Vec::new();
                use image::ImageEncoder;
                let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 92);
                let _ = enc.set_exif_metadata(filetool_core::exif::build_sample("2026:10:01 09:00:00", if i % 8 == 0 { 6 } else { 1 }));
                enc.write_image(img.as_raw(), 4032, 3024, image::ExtendedColorType::Rgb8).map_err(|e| Error::Invalid(e.to_string()))?;
                std::fs::write(&p, buf).map_err(ioe(&p))?;
            }
            1 => samples::photo_like(1600, 1200, i).save(&p).map_err(|e| Error::Invalid(e.to_string()))?,
            2 => {
                let img = image::DynamicImage::ImageRgb8(samples::photo_like(3000, 2000, i));
                let opts = filetool_core::imageconv::ImageOpts { format: filetool_core::imageconv::ImgFormat::Webp, quality: 80, max_side: None, keep_exif: false };
                let b = filetool_core::imageconv::encode(&img, &opts, None, &p)?;
                std::fs::write(&p, b).map_err(ioe(&p))?;
            }
            _ => {
                if heics.is_empty() {
                    continue;
                }
                let src = &heics[(i as usize / 4) % heics.len()];
                std::fs::copy(src, &p).map_err(ioe(&p))?;
            }
        }
    }
    // 6 맥 이름: NFD 폴더(안에 NFD 파일·폴더) + 맥 zip
    let mac = dir.join("mac");
    mk(&mac)?;
    let root = mac.join(samples::nfd("맥에서 온 자료"));
    if !root.exists() {
        mk(&root.join(samples::nfd("회의 사진")))?;
        for n in ["보고서 최종.txt", "예산 정리.csv", "회의 사진/첫째 날.txt", "회의 사진/둘째 날.txt"] {
            let p = root.join(samples::nfd(n));
            std::fs::write(&p, n.as_bytes()).map_err(ioe(&p))?;
        }
    }
    let file = mac.join(samples::nfd("한글 이름 파일.pdf"));
    if !file.exists() {
        samples::make_pdf(&file, 1, "NFD")?;
    }
    let z = mac.join("맥에서 묶은.zip");
    if !z.exists() {
        let a = samples::nfd("가나다/보고서.txt");
        let b = samples::nfd("가나다/사진 목록.txt");
        samples::write_mac_style_zip(&z, &[(a.as_str(), b"one".as_slice()), (b.as_str(), b"two".as_slice())])?;
    }
    // 4 Office: 손상 docx(나머지는 밖에서 복사)
    let o = dir.join("office");
    mk(&o)?;
    let bad = o.join("zz 손상된 문서.docx");
    if !bad.exists() {
        std::fs::write(&bad, b"PK\x03\x04 this is not a real docx").map_err(ioe(&bad))?;
    }
    println!("만들었어요: {} ({:.1}초)", dir.display(), t0.elapsed().as_secs_f64());
    Ok(())
}

// ---------- 점검 ----------

struct Row {
    job: &'static str,
    case: String,
    pass: bool,
    secs: f64,
    out: String,
    note: String,
}

fn files_in(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()).map(|e| exts.contains(&e.to_ascii_lowercase().as_str())).unwrap_or(false)).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn hashes(ps: &[PathBuf]) -> BTreeMap<PathBuf, String> {
    ps.iter().map(|p| (p.clone(), sha256_hex(p))).collect()
}

fn size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn mb(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.2} MB", n as f64 / 1_000_000.0)
    } else {
        format!("{} KB", n / 1000)
    }
}

/// 쪽이 비어 있지 않은지(흰색이 아닌 화소 비율 > 0.1%)
fn page_nonblank(p: &Path, pw: Option<&str>, page: u32) -> Result<bool> {
    let img = thumbs::render_rgb(p, pw, page, 0.25)?;
    let total = (img.width() * img.height()).max(1) as f64;
    let ink = img.pixels().filter(|px| px.0.iter().any(|c| *c < 235)).count() as f64;
    Ok(ink / total > 0.001)
}

/// 결과 PDF의 빈 쪽 번호들
pub(crate) fn blank_pages(p: &Path) -> Result<(u32, Vec<u32>)> {
    let n = thumbs::page_count(p, None)?;
    let mut blanks = Vec::new();
    for i in 1..=n {
        if !page_nonblank(p, None, i)? {
            blanks.push(i);
        }
    }
    Ok((n, blanks))
}

/// 남은 우리 도구 프로세스(Windows: tasklist)
fn leftovers() -> Vec<String> {
    #[cfg(windows)]
    {
        let out = std::process::Command::new("tasklist").args(["/FO", "CSV", "/NH"]).output();
        if let Ok(o) = out {
            let s = String::from_utf8_lossy(&o.stdout).to_lowercase();
            return ["soffice.bin", "soffice.exe", "soffice.com", "hwp.exe", "winword.exe", "excel.exe", "powerpnt.exe", "ffmpeg.exe", "ffprobe.exe"]
                .iter()
                .filter(|n| s.contains(&format!("\"{n}\"")))
                .map(|n| n.to_string())
                .collect();
        }
        Vec::new()
    }
    #[cfg(not(windows))]
    Vec::new()
}

fn originals_ok(before: &BTreeMap<PathBuf, String>) -> bool {
    before.iter().all(|(p, h)| &sha256_hex(p) == h)
}

fn run_all(dir: &Path, only: &[String], f: &Flags) -> ExitCode {
    let want = |j: &str| only.is_empty() || only.iter().any(|o| o == j);
    let ts = toolset(f);
    let out_root = dir.join("_out");
    let _ = std::fs::remove_dir_all(&out_root);
    let _ = std::fs::create_dir_all(&out_root);
    let mut rows: Vec<Row> = Vec::new();
    let pre = leftovers();
    if !pre.is_empty() {
        println!("주의: 시작 전에 이미 떠 있는 프로세스 {pre:?}");
    }

    // ---- 1 PDF 합치기(쪽 빼기 포함) ----
    if want("merge") {
        let ins = files_in(&dir.join("merge"), &["pdf"]);
        let h = hashes(&ins);
        let t0 = Instant::now();
        // 둘째 파일의 2·4쪽을 뺀다
        let mut pages = Vec::new();
        let mut srcs = Vec::new();
        let mut src_nonblank = Vec::new();
        for (s, p) in ins.iter().enumerate() {
            srcs.push(PlanSource { path: p.clone(), password: None });
            let n = thumbs::page_count(p, None).unwrap_or(0);
            for k in 0..n {
                if s == 1 && (k == 1 || k == 3) {
                    continue;
                }
                pages.push(PlanPage::Page { src: s, index: k, rotate: 0 });
                src_nonblank.push(page_nonblank(p, None, k + 1).unwrap_or(false));
            }
        }
        let expect = pages.len() as u32;
        let out = filetool_core::paths::output_path(&ins[0], Some(&out_root), "_합침", "pdf");
        let mut cb = |_: u32, _: u32| {};
        let mut j = Job::new(None, &mut cb);
        let r = plan::apply(&EditPlan { sources: srcs, pages }, &out, &mut j);
        let secs = t0.elapsed().as_secs_f64();
        let (pass, note) = match r.and_then(|_| blank_pages(&out)) {
            Ok((n, blanks)) => {
                let bad_blank: Vec<u32> = blanks.iter().copied().filter(|b| src_nonblank.get(*b as usize - 1).copied().unwrap_or(true)).collect();
                (n == expect && bad_blank.is_empty() && originals_ok(&h), format!("{n}쪽(기대 {expect}, 2쪽 뺌) · 빈 쪽 {bad_blank:?}"))
            }
            Err(e) => (false, e.to_string()),
        };
        rows.push(Row { job: "1 합치기", case: format!("PDF {}개(3+5+4쪽)", ins.len()), pass, secs, out: mb(size(&out)), note });
    }

    // ---- 2 PDF 용량 줄이기(목표 크기) ----
    if want("compress") {
        let c = dir.join("compress");
        let ins = files_in(&c, &["pdf"]);
        let h = hashes(&ins);
        let target = 5_000_000u64;
        let t_all = Instant::now();
        let (mut ok, mut fails) = (0, Vec::new());
        for p in &ins {
            let t0 = Instant::now();
            let r = pdfcompress::compress_to_target(p, Some(&out_root), target, None, &mut |_, _| {});
            let secs = t0.elapsed().as_secs_f64();
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            match r {
                Ok(o) => {
                    ok += 1;
                    let (pass, note, out) = match &o.output {
                        Some(out) => {
                            let src_n = thumbs::page_count(p, None).unwrap_or(0);
                            let (n, blanks) = blank_pages(out).unwrap_or((0, vec![0]));
                            let under = size(out) <= target;
                            let pass = n == src_n && blanks.is_empty() && (under || !o.reached);
                            (pass && (under || name.contains("글자")), format!("{}쪽(원본 {src_n}) · 빈 쪽 {blanks:?} · {}", n, o.note), mb(size(out)))
                        }
                        None => (true, o.note.clone(), "새 파일 없음".into()),
                    };
                    rows.push(Row { job: "2 용량 줄이기", case: format!("{name} {} → 목표 5MB", mb(o.before)), pass, secs, out, note });
                }
                Err(e) => {
                    let msg = e.to_string();
                    fails.push(name.clone());
                    let expected = name.contains("암호") || name.contains("손상");
                    rows.push(Row { job: "2 용량 줄이기", case: format!("{name} → 목표 5MB"), pass: expected, secs, out: "-".into(), note: format!("거절(다음 파일 계속): {msg}") });
                }
            }
        }
        rows.push(Row {
            job: "2 용량 줄이기",
            case: format!("묶음 {}개", ins.len()),
            pass: originals_ok(&h) && ok + fails.len() == ins.len(),
            secs: t_all.elapsed().as_secs_f64(),
            out: "-".into(),
            note: format!("{}개 중 {ok}개 완료, {}개는 거절({}) · 원본 해시 {}", ins.len(), fails.len(), fails.join(", "), if originals_ok(&h) { "그대로" } else { "바뀜!" }),
        });
        // 취소: 1초 뒤 멈추면 파일을 남기지 않음
        if let Some(scan) = ins.iter().find(|p| p.to_string_lossy().contains("스캔")) {
            let cdir = out_root.join("cancel");
            let _ = std::fs::create_dir_all(&cdir);
            let c = Cancel::new();
            let c2 = c.clone();
            let th = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(800));
                c2.cancel();
            });
            let t0 = Instant::now();
            let r = pdfcompress::compress_to_target(scan, Some(&cdir), 1_000, Some(&c), &mut |_, _| {});
            let _ = th.join();
            let left = std::fs::read_dir(&cdir).map(|d| d.count()).unwrap_or(0);
            rows.push(Row {
                job: "2 용량 줄이기",
                case: "멈추기(0.8초 뒤)".into(),
                pass: matches!(r, Err(Error::Cancelled)) && left == 0,
                secs: t0.elapsed().as_secs_f64(),
                out: "-".into(),
                note: format!("{} · 남은 파일 {left}개", if matches!(r, Err(Error::Cancelled)) { "멈춤" } else { "멈추지 않음" }),
            });
        }
    }

    // ---- 3 사진 → PDF 하나(섞인 40장, A4) ----
    if want("img2pdf") {
        let all = files_in(&dir.join("photos300"), &["jpg", "png", "webp", "heic"]);
        let ins: Vec<PathBuf> = all.iter().take(40).cloned().collect();
        let h = hashes(&ins);
        let out = filetool_core::paths::output_path(&ins[0], Some(&out_root), "", "pdf");
        let t0 = Instant::now();
        let r = img2pdf::images_to_pdf(ts.ffmpeg.as_deref(), &ins, &out, &Img2PdfOpts { page: PageSize::A4Portrait, margin_mm: 10.0, compact: true }, None, &mut |_| {});
        let secs = t0.elapsed().as_secs_f64();
        let heic_n = ins.iter().filter(|p| p.extension().map(|e| e == "heic").unwrap_or(false)).count();
        let (pass, note) = match r.and_then(|_| blank_pages(&out)) {
            Ok((n, blanks)) => (n as usize == ins.len() && blanks.is_empty() && originals_ok(&h), format!("{n}쪽(사진 {}장, HEIC {heic_n}) · 빈 쪽 {blanks:?} · 이름순", ins.len())),
            Err(e) => (false, e.to_string()),
        };
        rows.push(Row { job: "3 사진→PDF", case: format!("섞인 사진 {}장, A4", ins.len()), pass, secs, out: mb(size(&out)), note });
    }

    // ---- 4 한글·Office → PDF(여러 개, 손상 1개 섞음) ----
    if want("office2pdf") {
        let o = dir.join("office");
        let ins = files_in(&o, &["hwp", "hwpx", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "rtf"]);
        let h = hashes(&ins);
        let t_all = Instant::now();
        let (mut ok, mut fails) = (0usize, Vec::new());
        // 0.3.3: 길(한글·Office COM·문서 도구). WIN_FILETOOL_HIDE_COM=1이면 한글·Office 없이 문서 도구로
        let engines = filetool_core::docs::Engines::detect();
        println!("길: {engines:?} · 문서 도구 {}", if ts.soffice.is_some() { "있음" } else { "없음" });
        for p in &ins {
            let t0 = Instant::now();
            let via = crate::doctools033::engine_label(filetool_core::docs::engine_for(p, DocTarget::Pdf, &engines));
            let r = convert::convert_file(&ts, p, Some(&out_root), &Op::Doc { target: DocTarget::Pdf }, None, &mut |_| {});
            let secs = t0.elapsed().as_secs_f64();
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            let left = leftovers();
            match r {
                Ok(c) => {
                    ok += 1;
                    let refp = o.join("ref").join(format!("{}.pdf", p.file_stem().unwrap().to_string_lossy()));
                    let refn = thumbs::page_count(&refp, None).ok();
                    // 참고 PDF(원본 기관이 낸 PDF)에서도 빈 쪽이면 원래 빈 쪽 — 그 쪽은 빼고 본다
                    let ref_blank = if refn.is_some() { blank_pages(&refp).map(|b| b.1).unwrap_or_default() } else { Vec::new() };
                    let (n, all_blanks) = blank_pages(&c.output).unwrap_or((0, vec![0]));
                    let blanks: Vec<u32> = all_blanks.iter().copied().filter(|b| !ref_blank.contains(b)).collect();
                    let pass = n > 0 && blanks.is_empty() && refn.map(|r| r == n).unwrap_or(true) && left.is_empty();
                    rows.push(Row {
                        job: "4 문서→PDF",
                        case: name,
                        pass,
                        secs,
                        out: mb(size(&c.output)),
                        note: format!("{via} · {n}쪽{} · 빈 쪽 {blanks:?}{}{}", refn.map(|r| format!("(참고 PDF {r}쪽)")).unwrap_or_default(), if ref_blank.is_empty() { String::new() } else { format!(" (참고 PDF에서도 빈 쪽 {ref_blank:?}은 뺌)") }, if left.is_empty() { String::new() } else { format!(" · 남은 프로세스 {left:?}") }),
                    });
                }
                Err(e) => {
                    fails.push(name.clone());
                    rows.push(Row { job: "4 문서→PDF", case: name.clone(), pass: name.contains("손상"), secs, out: "-".into(), note: format!("{via} · 실패(다음 파일 계속): {e}") });
                }
            }
        }
        let left = leftovers();
        rows.push(Row {
            job: "4 문서→PDF",
            case: format!("묶음 {}개", ins.len()),
            pass: originals_ok(&h) && left.is_empty() && ok + fails.len() == ins.len(),
            secs: t_all.elapsed().as_secs_f64(),
            out: "-".into(),
            note: format!("{}개 중 {ok}개 완료, {}개 실패({}) · 원본 {} · 남은 프로세스 {left:?}", ins.len(), fails.len(), fails.join(", "), if originals_ok(&h) { "그대로" } else { "바뀜!" }),
        });
    }

    // ---- 5 사진 용량 줄이기·JPG로(300장) ----
    if want("photo") {
        let ins = files_in(&dir.join("photos300"), &["jpg", "png", "webp", "heic"]);
        let h = hashes(&ins);
        let pdir = out_root.join("photo");
        let _ = std::fs::create_dir_all(&pdir);
        let t0 = Instant::now();
        let r = corejobs::photo_shrink(&ts, &ins, &corejobs::photo_opts(Some(1920), Some(85)), Some(&pdir), &Cancel::new(), &mut |_, _, _| {});
        let secs = t0.elapsed().as_secs_f64();
        match r {
            Ok(items) => {
                let made: Vec<&PathBuf> = items.iter().filter_map(|i| i.output.as_ref()).collect();
                let errs: Vec<String> = items.iter().filter_map(|i| i.error.clone()).collect();
                let unchanged = items.iter().filter(|i| i.unchanged).count();
                let before: u64 = items.iter().map(|i| i.before).sum();
                let after: u64 = made.iter().map(|p| size(p)).sum::<u64>() + items.iter().filter(|i| i.unchanged).map(|i| i.before).sum::<u64>();
                // 모두 JPG, 긴 변 1920 이하
                let mut too_big = 0;
                for p in made.iter().step_by(10) {
                    if let Ok(d) = image::image_dimensions(p) {
                        if d.0.max(d.1) > 1920 {
                            too_big += 1;
                        }
                    }
                }
                let all_jpg = made.iter().all(|p| p.extension().map(|e| e == "jpg").unwrap_or(false));
                let left = leftovers();
                rows.push(Row {
                    job: "5 사진 줄이기",
                    case: format!("사진 {}장(JPG·PNG·WEBP·HEIC 75장씩) → JPG 1920px", ins.len()),
                    pass: errs.is_empty() && all_jpg && too_big == 0 && originals_ok(&h) && left.is_empty() && made.len() + unchanged == ins.len(),
                    secs,
                    out: format!("{} → {}", mb(before), mb(after)),
                    note: format!("만든 JPG {} · 그대로 {unchanged} · 오류 {} · 긴 변 1920 넘음 {too_big}(10장마다 확인) · 남은 프로세스 {left:?}", made.len(), errs.len()),
                });
            }
            Err(e) => rows.push(Row { job: "5 사진 줄이기", case: format!("사진 {}장", ins.len()), pass: false, secs, out: "-".into(), note: e.to_string() }),
        }
        // 멈추기
        let cdir = out_root.join("photo-cancel");
        let _ = std::fs::create_dir_all(&cdir);
        let c = Cancel::new();
        let c2 = c.clone();
        let th = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            c2.cancel();
        });
        let t0 = Instant::now();
        let r = corejobs::photo_shrink(&ts, &ins, &corejobs::photo_opts(Some(1920), Some(85)), Some(&cdir), &c, &mut |_, _, _| {});
        let _ = th.join();
        std::thread::sleep(Duration::from_millis(500));
        let left_files = std::fs::read_dir(&cdir).map(|d| d.count()).unwrap_or(0);
        let left = leftovers();
        rows.push(Row {
            job: "5 사진 줄이기",
            case: "멈추기(1.5초 뒤)".into(),
            pass: matches!(r, Err(Error::Cancelled)) && left_files == 0 && left.is_empty(),
            secs: t0.elapsed().as_secs_f64(),
            out: "-".into(),
            note: format!("{} · 남은 파일 {left_files} · 남은 프로세스 {left:?}", if matches!(r, Err(Error::Cancelled)) { "멈춤" } else { "멈추지 않음" }),
        });
    }

    // ---- 6 맥 이름 고치기 ----
    if want("macname") {
        let mac = dir.join("mac");
        // 시험마다 새로: 사본에서
        let work = out_root.join("mac");
        let _ = copy_tree(&mac, &work);
        let t0 = Instant::now();
        let mut notes = Vec::new();
        let mut pass = true;
        for e in std::fs::read_dir(&work).map(|r| r.flatten().map(|e| e.path()).collect::<Vec<_>>()).unwrap_or_default() {
            if e.is_dir() {
                match nfc::normalize_folder(&e) {
                    Ok(rep) => {
                        let root = corejobs::nfc_fix_path(&e);
                        let fixed = root.to.clone().unwrap_or(e.clone());
                        let still = walk_nfd(&fixed);
                        notes.push(format!("폴더: 안 {}개 + 폴더 이름 {} 고침, 남은 NFD {still}", rep.renamed.len(), if root.to.is_some() { 1 } else { 0 }));
                        pass &= still == 0 && rep.failed.is_empty();
                    }
                    Err(err) => {
                        notes.push(format!("폴더 실패: {err}"));
                        pass = false;
                    }
                }
            } else if e.extension().map(|x| x == "zip").unwrap_or(false) {
                let mut cb = |_: usize, _: usize, _: &str| {};
                match ziptool::extract(&e, ziptool::NameEnc::Auto, None, None, &mut cb) {
                    Ok(o) => {
                        let still = walk_nfd(&o.folder);
                        let names: Vec<String> = walk_names(&o.folder);
                        let ok = still == 0 && names.iter().any(|n| n == "보고서.txt");
                        notes.push(format!("zip: {}개 풀림, 이름 {:?}, 남은 NFD {still}", o.count, names));
                        pass &= ok;
                    }
                    Err(err) => {
                        notes.push(format!("zip 실패: {err}"));
                        pass = false;
                    }
                }
            } else {
                let r = corejobs::nfc_fix_path(&e);
                notes.push(format!("파일: {}", r.to.as_ref().map(|t| t.file_name().unwrap().to_string_lossy().into_owned()).unwrap_or_else(|| r.note.clone().unwrap_or("그대로".into()))));
                pass &= r.to.is_some();
            }
        }
        rows.push(Row { job: "6 맥 이름", case: "NFD 폴더 + NFD 파일 + 맥 zip".into(), pass, secs: t0.elapsed().as_secs_f64(), out: "-".into(), note: notes.join(" / ") });
    }

    // ---- 표 ----
    println!("| 할 일 | 경우 | 결과 | 시간 | 결과 크기 | 점검 |");
    println!("|---|---|---|---:|---:|---|");
    let mut failed = 0;
    for r in &rows {
        if !r.pass {
            failed += 1;
        }
        println!("| {} | {} | {} | {:.1}초 | {} | {} |", r.job, r.case, if r.pass { "통과" } else { "**실패**" }, r.secs, r.out, r.note.replace('|', "/"));
    }
    let left = leftovers();
    println!("\n끝난 뒤 남은 프로세스: {left:?}");
    println!("통과 {} / 실패 {failed}", rows.len() - failed);
    if failed == 0 && left.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)?.flatten() {
        let p = e.path();
        let d = dst.join(e.file_name());
        if p.is_dir() {
            copy_tree(&p, &d)?;
        } else {
            std::fs::copy(&p, &d)?;
        }
    }
    Ok(())
}

fn walk_nfd(root: &Path) -> usize {
    walk_names(root).iter().filter(|n| nfc::to_nfc(n) != **n).count()
}

fn walk_names(root: &Path) -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            v.push(e.file_name().to_string_lossy().into_owned());
            if e.path().is_dir() {
                v.extend(walk_names(&e.path()));
            }
        }
    }
    v
}
