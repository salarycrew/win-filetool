//! selftest 1c: PDF 압축(사진 많은 PDF, 암호 PDF), 이미지 → PDF(EXIF 회전 JPEG + HEIC), PDF → PNG(150 DPI),
//! 들어 있는 이미지 꺼내기. 그려 본 결과는 PDFium(exe 옆 pdfium.dll)으로 확인한다.

use std::path::Path;
use std::process::ExitCode;

use filetool_core::img2pdf::{self, Img2PdfOpts, PageSize};
use filetool_core::pdfcompress::{self, CompressPreset};
use filetool_core::pdfrender::{self, PageImageFormat, RenderOpts};
use filetool_core::tools::{SearchEnv, Toolset};
use filetool_core::{pdf, samples, Error, Result};

use crate::conv::{tool_paths, Flags};

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
    fn result<T>(&mut self, name: &str, r: Result<T>, f: impl FnOnce(T) -> (bool, String)) {
        match r {
            Ok(v) => {
                let (ok, d) = f(v);
                self.check(name, ok, d)
            }
            Err(e) => self.check(name, false, format!("오류: {e}")),
        }
    }
}

fn io(e: std::io::Error) -> Error {
    Error::Invalid(e.to_string())
}

fn fname(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn size(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

pub fn run(dir: &Path, args: &[String]) -> Result<ExitCode> {
    std::fs::create_dir_all(dir).map_err(io)?;
    let f = Flags::parse(&args[1.min(args.len())..]);
    let ts = Toolset::discover(&tool_paths(&f), &SearchEnv::from_system());
    let mut c = Checker { failed: 0, total: 0 };
    println!("--- 1c ---");
    let pdfium = pdfrender::pdfium_status();
    c.check("PDFium 불러오기", pdfium.is_ok(), match &pdfium { Ok(p) => p.display().to_string(), Err(e) => e.clone() });

    // 1) PDF 압축: 사진 많은 PDF(쪽마다 2400px JPEG 품질 95 + 1쪽에 무손실 그림)
    let photo = dir.join("사진 많은 문서.pdf");
    samples::make_photo_pdf(&photo, 4, "PHOTO", 2400)?;
    let before_labels = samples::pdf_labels(&photo)?;
    let before_contents = samples::pdf_page_contents(&photo)?;
    for preset in [CompressPreset::Screen, CompressPreset::Normal, CompressPreset::Print] {
        let name = format!("PDF 압축 {} ({} DPI, 품질 {})", preset.label(), preset.max_dpi(), preset.quality());
        c.result(&name, pdfcompress::compress(&photo, Some(&dir.join(preset.label())), preset), |r| {
            let Some(o) = r.output.clone() else { return (false, r.note) };
            let pages = pdf::page_count(&o).unwrap_or(0);
            let labels = samples::pdf_labels(&o).unwrap_or_default();
            let same_content = samples::pdf_page_contents(&o).map(|x| x == before_contents).unwrap_or(false);
            let ok = r.after < r.before && size(&o) == r.after && pages == 4 && labels == before_labels && same_content && size(&photo) == r.before;
            (ok, format!("{} → {} B, {pages}쪽, 글자 {labels:?}, 콘텐츠 스트림 같음 {same_content}, {}", r.before, r.after, r.note))
        });
    }
    let enc = dir.join("암호.pdf");
    samples::make_encrypted_pdf(&enc, "user-pw")?;
    let r = pdfcompress::compress(&enc, None, CompressPreset::Normal);
    let leftover = std::fs::read_dir(dir).map_err(io)?.filter_map(|e| e.ok()).filter(|e| fname(&e.path()).starts_with("암호_")).count();
    c.check(
        "PDF 압축: 암호 PDF는 거절",
        matches!(&r, Err(e) if e.to_string().contains("암호가 걸린 PDF는 압축할 수 없습니다")) && leftover == 0,
        format!("{} / 남은 출력 {leftover}개", match &r { Err(e) => e.to_string(), Ok(o) => format!("성공해 버림 {:?}", o.output) }),
    );
    // 이미 줄인 것을 다시(인쇄용): 더 줄지 않으면 새 파일 없음
    let screen_out = dir.join("화면용").join("사진 많은 문서_압축.pdf");
    let again = pdfcompress::compress(&screen_out, Some(&dir.join("다시")), CompressPreset::Print);
    c.result("PDF 압축: 더 줄지 않으면 원본 유지", again, |r| (r.output.is_none() && !dir.join("다시").exists(), r.note));

    // 2) 이미지 → PDF: EXIF 회전 6 JPEG + HEIC(ffmpeg) + PNG
    let jpg = dir.join("세로 사진.jpg");
    samples::make_jpeg_with_exif(&jpg, 120, 80, "2024:05:01 10:20:30", 6)?;
    let png = dir.join("그림.png");
    samples::quadrant_image(200, 100).save(&png).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut inputs = vec![jpg.clone()];
    match ts.ffmpeg.as_deref() {
        Some(ff) => {
            let heic = dir.join("IMG_0002.HEIC");
            match samples::make_grid_heic(ff, &heic, 512, 1000, 700, "2023:08:15 14:03:22") {
                Ok(()) => inputs.push(heic),
                Err(e) => c.check("HEIC 샘플 만들기", false, e.to_string()),
            }
        }
        None => c.check("HEIC 샘플 만들기", false, "ffmpeg 없음".into()),
    }
    inputs.push(png);
    let out = img2pdf::default_output(&inputs, None);
    let r = img2pdf::images_to_pdf(ts.ffmpeg.as_deref(), &inputs, &out, &Img2PdfOpts { page: PageSize::Image, margin_mm: 0.0 }, None, &mut |_| {});
    c.result("이미지 → PDF (쪽 수·쪽 크기·JPEG 그대로)", r, |r| {
        let n = pdf::page_count(&r.output).unwrap_or(0);
        let sizes = samples::pdf_page_sizes(&r.output).unwrap_or_default();
        let jpeg_same = lopdf::Document::load(&r.output)
            .map(|d| {
                let b = std::fs::read(&jpg).unwrap_or_default();
                d.objects.values().filter_map(|o| o.as_stream().ok()).any(|s| s.content == b)
            })
            .unwrap_or(false);
        let want: Vec<(u32, u32)> = if inputs.len() == 3 { vec![(80, 120), (700, 1000), (200, 100)] } else { vec![(80, 120), (200, 100)] };
        let got: Vec<(u32, u32)> = r.pages.iter().map(|p| (p.px_w, p.px_h)).collect();
        let ok = n as usize == inputs.len() && got == want && jpeg_same && r.pages[0].orientation == 6
            && sizes.iter().zip(&want).all(|(s, w)| (s.0 - w.0 as f64 * 0.75).abs() < 0.01 && (s.1 - w.1 as f64 * 0.75).abs() < 0.01);
        (ok, format!("{} {n}쪽, 픽셀 {got:?}, 쪽(pt) {sizes:?}, 회전 JPEG 바이트 그대로 {jpeg_same}", fname(&r.output)))
    });
    if pdfium.is_ok() && out.is_file() {
        let rr = pdfrender::render_pages(&out, Some(dir), &RenderOpts { format: PageImageFormat::Png, dpi: 96, quality: 90, pages: None }, None, &mut |_, _| {});
        c.result("이미지 → PDF 그려 보기 (보이는 방향)", rr, |rr| {
            let q: Vec<[&str; 4]> = rr.pages.iter().filter_map(|p| image::open(&p.output).ok()).map(|i| samples::quadrant_colors(&i)).collect();
            let mut want = vec![["파랑", "빨강", "흰색", "초록"]];
            if inputs.len() == 3 {
                want.push(["초록", "흰색", "빨강", "파랑"]);
            }
            want.push(["빨강", "초록", "파랑", "흰색"]);
            let dims: Vec<(u32, u32)> = rr.pages.iter().map(|p| (p.width, p.height)).collect();
            (q == want, format!("사분면 {q:?}, 크기 {dims:?}"))
        });
    }
    let a4 = dir.join("A4 가로.pdf");
    c.result("이미지 → PDF (A4 가로, 여백 15mm)", img2pdf::images_to_pdf(ts.ffmpeg.as_deref(), &inputs[..1], &a4, &Img2PdfOpts { page: PageSize::A4Landscape, margin_mm: 15.0 }, None, &mut |_| {}), |r| {
        let p = &r.pages[0];
        let ok = (p.page_w - img2pdf::A4_H).abs() < 0.01 && (p.page_h - img2pdf::A4_W).abs() < 0.01 && (p.draw_h - (img2pdf::A4_W - 2.0 * 15.0 * 72.0 / 25.4)).abs() < 0.01;
        (ok, format!("쪽 {:.1}×{:.1}pt, 그림 {:.1}×{:.1}pt", p.page_w, p.page_h, p.draw_w, p.draw_h))
    });

    // 3) PDF → PNG 150 DPI
    if pdfium.is_ok() {
        let doc = dir.join("보고서.pdf");
        samples::make_pdf(&doc, 3, "R")?;
        let rr = pdfrender::render_pages(&doc, None, &RenderOpts { format: PageImageFormat::Png, dpi: 150, quality: 90, pages: None }, None, &mut |_, _| {});
        c.result("PDF → PNG 150 DPI (A4 3쪽)", rr, |rr| {
            let dims: Vec<(u32, u32)> = rr.pages.iter().filter_map(|p| image::open(&p.output).ok()).map(|i| (i.width(), i.height())).collect();
            let ok = rr.pages.len() == 3 && dims.len() == 3 && dims.iter().all(|&(w, h)| (1239..=1240).contains(&w) && (1754..=1755).contains(&h));
            (ok, format!("{} {}장 {dims:?} (595×842pt × 150/72 = 1239.6×1754.2)", fname(&rr.folder), rr.pages.len()))
        });
        let rj = pdfrender::render_pages(&photo, None, &RenderOpts { format: PageImageFormat::Jpg, dpi: 72, quality: 85, pages: Some("2-3".into()) }, None, &mut |_, _| {});
        c.result("PDF → JPG 72 DPI (2-3쪽만)", rj, |r| {
            let d: Vec<(u32, u32, u32)> = r.pages.iter().map(|p| (p.page, p.width, p.height)).collect();
            (d == [(2, 595, 842), (3, 595, 842)], format!("{} {d:?}", fname(&r.folder)))
        });
    }
    // 4) 들어 있는 이미지 꺼내기
    c.result("PDF 안 이미지 꺼내기 (JPEG 그대로)", pdfrender::extract_images(&photo, None), |r| {
        let jpgs = r.files.iter().filter(|p| p.extension().map(|e| e == "jpg").unwrap_or(false)).count();
        let pngs = r.files.iter().filter(|p| p.extension().map(|e| e == "png").unwrap_or(false)).count();
        let first_same = samples::jpeg_bytes(&samples::photo_like(2400, 1800, 1), 95).map(|b| r.files.iter().any(|f| std::fs::read(f).map(|x| x == b).unwrap_or(false))).unwrap_or(false);
        (jpgs == 4 && pngs == 1 && first_same, format!("{} JPG {jpgs} / PNG {pngs}, 1쪽 사진 바이트 같음 {first_same}", r.note))
    });

    println!("결과(1c): {}/{} 통과", c.total - c.failed, c.total);
    Ok(if c.failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) })
}
