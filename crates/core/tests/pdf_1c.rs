//! 1c: PDF 압축, 이미지 → PDF, PDF → 이미지, 윈도 시작 시 실행 값.
//! PDF → 이미지는 PDFium(`third_party/pdfium/linux-x64`, scripts/fetch-pdfium.sh)이 있어야 돈다.

use std::path::{Path, PathBuf};

use filetool_core::img2pdf::{self, Img2PdfOpts, PageSize};
use filetool_core::pdfcompress::{self, CompressPreset};
use filetool_core::pdfrender::{self, PageImageFormat, RenderOpts};
use filetool_core::{autostart, samples};

fn use_repo_pdfium() -> bool {
    if std::env::var_os("WIN_FILETOOL_PDFIUM").is_none() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/pdfium/linux-x64/lib");
        std::env::set_var("WIN_FILETOOL_PDFIUM", p);
    }
    pdfrender::pdfium_status().is_ok()
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

fn len(p: &Path) -> u64 {
    std::fs::metadata(p).unwrap().len()
}

// ---------- PDF 압축 ----------

#[test]
fn compress_photo_pdf_shrinks_and_keeps_text_and_pages() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("사진 많은.pdf");
    samples::make_photo_pdf(&src, 3, "PHOTO", 2400).unwrap();
    let before_contents = samples::pdf_page_contents(&src).unwrap();
    let r = pdfcompress::compress(&src, None, CompressPreset::Normal).unwrap();
    let out = r.output.clone().expect("작아졌어야 함");
    assert_eq!(out.file_name().unwrap().to_string_lossy(), "사진 많은_압축.pdf");
    assert!(r.after < r.before / 3, "{} -> {}", r.before, r.after);
    assert_eq!(len(&out), r.after);
    assert_eq!(filetool_core::pdf::page_count(&out).unwrap(), 3);
    assert_eq!(samples::pdf_labels(&out).unwrap(), ["PHOTO p1", "PHOTO p2", "PHOTO p3"]);
    // 글자·벡터(콘텐츠 스트림)는 바이트 그대로
    assert_eq!(samples::pdf_page_contents(&out).unwrap(), before_contents);
    // 사진 3장(6인치로 찍힘) → 150 DPI면 가로 900px, Flate 그림(2인치, 600px) → 300px JPEG
    let imgs = samples::pdf_images(&out).unwrap();
    assert_eq!(r.images_total, 4);
    assert_eq!(r.images_recompressed, 4);
    assert_eq!(r.images_downsampled, 4);
    assert_eq!(imgs.iter().filter(|i| i.0 == 900 && i.1 == 675 && i.2 == "DCTDecode").count(), 3, "{imgs:?}");
    assert!(imgs.iter().any(|i| i.0 == 300 && i.1 == 200 && i.2 == "DCTDecode"), "{imgs:?}");
    // 원본은 그대로
    assert_eq!(len(&src), r.before);
    // 같은 이름이 있으면 (1)
    let r2 = pdfcompress::compress(&src, None, CompressPreset::Normal).unwrap();
    assert_eq!(r2.output.unwrap().file_name().unwrap().to_string_lossy(), "사진 많은_압축 (1).pdf");
}

#[test]
fn compress_presets_order_and_dpi() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("p.pdf");
    samples::make_photo_pdf(&src, 1, "P", 2400).unwrap();
    let mut sizes = vec![];
    for (p, w) in [(CompressPreset::Screen, 576), (CompressPreset::Normal, 900), (CompressPreset::Print, 1800)] {
        let r = pdfcompress::compress(&src, Some(t.path()), p).unwrap();
        let out = r.output.unwrap();
        let imgs = samples::pdf_images(&out).unwrap();
        assert!(imgs.iter().any(|i| i.0 == w), "{p:?}: {imgs:?}");
        sizes.push(r.after);
    }
    assert!(sizes[0] < sizes[1] && sizes[1] < sizes[2], "{sizes:?}");
}

#[test]
fn compress_not_smaller_keeps_original_and_writes_nothing() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("p.pdf");
    samples::make_photo_pdf(&src, 1, "P", 2400).unwrap();
    let once = pdfcompress::compress(&src, None, CompressPreset::Screen).unwrap().output.unwrap();
    // 이미 화면용으로 줄인 것을 인쇄용으로 다시: 사진은 이미 작고(줄일 것 없음) 다시 인코딩하면 커지므로 그대로
    let again = pdfcompress::compress(&once, None, CompressPreset::Print).unwrap();
    assert!(again.output.is_none(), "{again:?}");
    assert!(again.note.contains("새 파일을 만들지 않았습니다"), "{}", again.note);
    assert_eq!(again.images_recompressed, 0);
    assert_eq!(files_in(t.path()), ["p.pdf", "p_압축.pdf"]);
}

#[test]
fn compress_encrypted_pdf_is_refused() {
    let t = tempfile::tempdir().unwrap();
    for (name, pw) in [("열기암호.pdf", "user-pw"), ("권한암호만.pdf", "")] {
        let src = t.path().join(name);
        samples::make_encrypted_pdf(&src, pw).unwrap();
        let e = pdfcompress::compress(&src, None, CompressPreset::Normal).unwrap_err().to_string();
        assert!(e.contains("암호가 걸린 PDF는 압축할 수 없습니다"), "{name}: {e}");
    }
    assert_eq!(files_in(t.path()), ["권한암호만.pdf", "열기암호.pdf"]);
}

#[test]
fn compress_skips_masks_and_unknown_colorspaces() {
    // 1a 샘플(이미지 없음)은 이미지 0개, 결과는 쪽·글자 그대로
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("text.pdf");
    samples::make_pdf(&src, 4, "T").unwrap();
    let r = pdfcompress::compress(&src, None, CompressPreset::Normal).unwrap();
    assert_eq!(r.images_total, 0);
    if let Some(o) = &r.output {
        assert_eq!(samples::pdf_labels(o).unwrap(), ["T p1", "T p2", "T p3", "T p4"]);
    }
}

// ---------- 이미지 → PDF ----------

fn make_inputs(dir: &Path) -> Vec<PathBuf> {
    // 1) EXIF 회전 6 JPEG (저장 120×80, 보이는 방향 80×120)
    let a = dir.join("a_회전6.jpg");
    samples::make_jpeg_with_exif(&a, 120, 80, "2024:05:01 10:20:30", 6).unwrap();
    // 2) 투명 PNG 200×100
    let b = dir.join("b.png");
    let mut rgba = image::RgbaImage::from_pixel(200, 100, image::Rgba([0, 0, 255, 255]));
    for x in 0..100 {
        for y in 0..100 {
            rgba.put_pixel(x, y, image::Rgba([0, 0, 0, 0]));
        }
    }
    rgba.save(&b).unwrap();
    // 3) 흑백 JPEG 50×40
    let c = dir.join("c_흑백.jpg");
    image::GrayImage::from_pixel(50, 40, image::Luma([128])).save(&c).unwrap();
    vec![a, b, c]
}

#[test]
fn images_to_pdf_pages_sizes_orientation_passthrough() {
    let t = tempfile::tempdir().unwrap();
    let inputs = make_inputs(t.path());
    let out = img2pdf::default_output(&inputs, None);
    assert_eq!(out.file_name().unwrap().to_string_lossy(), "a_회전6_외2장.pdf");
    let r = img2pdf::images_to_pdf(None, &inputs, &out, &Img2PdfOpts { page: PageSize::Image, margin_mm: 0.0, compact: false }, None, &mut |_| {}).unwrap();
    assert_eq!(filetool_core::pdf::page_count(&out).unwrap(), 3);
    let sizes = samples::pdf_page_sizes(&out).unwrap();
    let want = [(60.0, 90.0), (150.0, 75.0), (37.5, 30.0)];
    for (s, w) in sizes.iter().zip(want) {
        assert!((s.0 - w.0).abs() < 0.01 && (s.1 - w.1).abs() < 0.01, "{sizes:?}");
    }
    assert_eq!(r.pages.iter().map(|p| p.jpeg_passthrough).collect::<Vec<_>>(), [true, false, true]);
    assert_eq!(r.pages[0].orientation, 6);
    // JPEG 두 장은 파일 바이트 그대로 들어 있다
    let doc = lopdf::Document::load(&out).unwrap();
    let a_bytes = std::fs::read(&inputs[0]).unwrap();
    let c_bytes = std::fs::read(&inputs[2]).unwrap();
    let streams: Vec<&Vec<u8>> = doc.objects.values().filter_map(|o| o.as_stream().ok()).map(|s| &s.content).collect();
    assert!(streams.iter().any(|s| **s == a_bytes) && streams.iter().any(|s| **s == c_bytes));
    // PNG 투명 → SMask
    let imgs = samples::pdf_images(&out).unwrap();
    assert!(imgs.iter().filter(|i| i.0 == 200 && i.1 == 100).count() == 2, "{imgs:?}");

    if use_repo_pdfium() {
        // 그려 보면 보이는 방향이 맞는지: 회전 6 → 시계 방향 90°: [파랑, 빨강, 흰색, 초록]
        let rr = pdfrender::render_pages(&out, Some(t.path()), &RenderOpts { format: PageImageFormat::Png, dpi: 96, quality: 90, pages: Some("1".into()) }, None, &mut |_, _| {}).unwrap();
        let img = image::open(&rr.pages[0].output).unwrap();
        assert_eq!((img.width(), img.height()), (80, 120));
        assert_eq!(samples::quadrant_colors(&img), ["파랑", "빨강", "흰색", "초록"]);
    } else {
        eprintln!("SKIP 회전 그려 보기: PDFium 없음");
    }
}

#[test]
fn placement_matrix_all_orientations_render_correctly() {
    if !use_repo_pdfium() {
        eprintln!("SKIP: PDFium 없음");
        return;
    }
    // 회전 값마다 원본(120×80 사분면)을 기대 방향으로 돌린 것과 같은 색 배치인지
    let t = tempfile::tempdir().unwrap();
    let base = image::DynamicImage::ImageRgb8(samples::quadrant_image(120, 80));
    for o in 1..=8u16 {
        let p = t.path().join(format!("o{o}.jpg"));
        samples::make_jpeg_with_exif(&p, 120, 80, "2024:01:01 00:00:00", o).unwrap();
        let mut want = base.clone();
        want.apply_orientation(image::metadata::Orientation::from_exif(o as u8).unwrap());
        let out = t.path().join(format!("o{o}.pdf"));
        img2pdf::images_to_pdf(None, &[p], &out, &Img2PdfOpts { page: PageSize::A4Portrait, margin_mm: 10.0, compact: false }, None, &mut |_| {}).unwrap();
        let sizes = samples::pdf_page_sizes(&out).unwrap();
        assert!((sizes[0].0 - img2pdf::A4_W).abs() < 0.01 && (sizes[0].1 - img2pdf::A4_H).abs() < 0.01);
        let rr = pdfrender::render_pages(&out, Some(t.path()), &RenderOpts { format: PageImageFormat::Png, dpi: 72, quality: 90, pages: None }, None, &mut |_, _| {}).unwrap();
        let page = image::open(&rr.pages[0].output).unwrap();
        // 그림 상자 가운데 부분만 잘라 사분면 비교
        let (pw, ph, x0, y0, dw, dh) = img2pdf::layout(PageSize::A4Portrait, 10.0, want.width(), want.height());
        let sx = page.width() as f64 / pw;
        let sy = page.height() as f64 / ph;
        let crop = page.crop_imm((x0 * sx) as u32, ((ph - y0 - dh) * sy) as u32, (dw * sx) as u32, (dh * sy) as u32);
        assert_eq!(samples::quadrant_colors(&crop), samples::quadrant_colors(&want), "회전 값 {o}");
    }
}

#[test]
fn images_to_pdf_a4_fit_with_margin() {
    let t = tempfile::tempdir().unwrap();
    let inputs = make_inputs(t.path());
    for (page, pw, ph) in [(PageSize::A4Portrait, img2pdf::A4_W, img2pdf::A4_H), (PageSize::A4Landscape, img2pdf::A4_H, img2pdf::A4_W)] {
        let out = t.path().join(format!("{page:?}.pdf"));
        let r = img2pdf::images_to_pdf(None, &inputs, &out, &Img2PdfOpts { page, margin_mm: 20.0, compact: false }, None, &mut |_| {}).unwrap();
        let m = 20.0 * 72.0 / 25.4;
        for p in &r.pages {
            assert!((p.page_w - pw).abs() < 0.01 && (p.page_h - ph).abs() < 0.01);
            // 여백 안에 들어가고, 한 변은 여백에 꼭 맞음, 비율 유지
            assert!(p.draw_w <= pw - 2.0 * m + 0.01 && p.draw_h <= ph - 2.0 * m + 0.01);
            assert!((p.draw_w - (pw - 2.0 * m)).abs() < 0.01 || (p.draw_h - (ph - 2.0 * m)).abs() < 0.01);
            assert!((p.draw_w / p.draw_h - p.px_w as f64 / p.px_h as f64).abs() < 0.001);
        }
    }
    // 원본은 그대로, 같은 이름이면 (1)
    let o1 = img2pdf::default_output(&inputs[..1], None);
    std::fs::write(&o1, b"x").unwrap();
    assert_eq!(img2pdf::default_output(&inputs[..1], None).file_name().unwrap().to_string_lossy(), "a_회전6 (1).pdf");
}

#[test]
fn images_to_pdf_heic_via_ffmpeg() {
    let Some(ff) = which("ffmpeg") else {
        eprintln!("SKIP: ffmpeg 없음");
        return;
    };
    let t = tempfile::tempdir().unwrap();
    let h = t.path().join("아이폰.heic");
    samples::make_grid_heic(&ff, &h, 512, 1000, 700, "2023:08:15 14:03:22").unwrap();
    let out = t.path().join("h.pdf");
    let r = img2pdf::images_to_pdf(Some(&ff), &[h], &out, &Img2PdfOpts { page: PageSize::Image, margin_mm: 0.0, compact: false }, None, &mut |_| {}).unwrap();
    // grid 1000×700, irot 90° → 700×1000
    assert_eq!((r.pages[0].px_w, r.pages[0].px_h), (700, 1000));
    assert!(!r.pages[0].jpeg_passthrough);
    if use_repo_pdfium() {
        let rr = pdfrender::render_pages(&out, Some(t.path()), &RenderOpts { format: PageImageFormat::Png, dpi: 96, quality: 90, pages: None }, None, &mut |_, _| {}).unwrap();
        let img = image::open(&rr.pages[0].output).unwrap();
        assert_eq!((img.width(), img.height()), (700, 1000));
        assert_eq!(samples::quadrant_colors(&img), ["초록", "흰색", "빨강", "파랑"]);
    }
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|f| f.is_file()))
}

// ---------- PDF → 이미지 ----------

#[test]
fn pdf_to_images_dimensions_at_dpi() {
    if !use_repo_pdfium() {
        eprintln!("SKIP: PDFium 없음");
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("문서.pdf");
    samples::make_pdf(&src, 3, "R").unwrap(); // A4 595×842pt
    let r = pdfrender::render_pages(&src, None, &RenderOpts { format: PageImageFormat::Png, dpi: 150, quality: 90, pages: None }, None, &mut |_, _| {}).unwrap();
    assert_eq!(r.folder.file_name().unwrap().to_string_lossy(), "문서_쪽이미지");
    assert_eq!(r.pages.len(), 3);
    // 595pt × 150/72 = 1239.6, 842 × 150/72 = 1754.2
    for p in &r.pages {
        let img = image::open(&p.output).unwrap();
        assert_eq!((img.width(), img.height()), (p.width, p.height));
        assert!((1239..=1240).contains(&p.width) && (1754..=1755).contains(&p.height), "{}×{}", p.width, p.height);
    }
    assert_eq!(files_in(&r.folder), ["문서_p01.png", "문서_p02.png", "문서_p03.png"]);
    // JPG, 일부 쪽, 72 DPI
    let r2 = pdfrender::render_pages(&src, None, &RenderOpts { format: PageImageFormat::Jpg, dpi: 72, quality: 80, pages: Some("2-3".into()) }, None, &mut |_, _| {}).unwrap();
    assert_eq!(r2.folder.file_name().unwrap().to_string_lossy(), "문서_쪽이미지 (1)");
    assert_eq!(r2.pages.iter().map(|p| (p.page, p.width, p.height)).collect::<Vec<_>>(), [(2, 595, 842), (3, 595, 842)]);
    // 회전한 쪽은 가로·세로가 바뀐다
    let rot = t.path().join("rot.pdf");
    filetool_core::pdf::rotate(&src, "1", 90, &rot).unwrap();
    let r3 = pdfrender::render_pages(&rot, None, &RenderOpts { format: PageImageFormat::Png, dpi: 72, quality: 90, pages: Some("1".into()) }, None, &mut |_, _| {}).unwrap();
    assert_eq!((r3.pages[0].width, r3.pages[0].height), (842, 595));
}

#[test]
fn pdf_to_images_encrypted_message() {
    if !use_repo_pdfium() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("e.pdf");
    samples::make_encrypted_pdf(&src, "user-pw").unwrap();
    let e = pdfrender::render_pages(&src, None, &RenderOpts { format: PageImageFormat::Png, dpi: 72, quality: 90, pages: None }, None, &mut |_, _| {}).unwrap_err();
    assert!(e.to_string().contains("암호"), "{e}");
    assert_eq!(files_in(t.path()), ["e.pdf"]);
}

#[test]
fn extract_embedded_images_keeps_jpeg_bytes() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("x.pdf");
    samples::make_photo_pdf(&src, 2, "X", 800).unwrap();
    let r = pdfrender::extract_images(&src, None).unwrap();
    let names: Vec<String> = r.files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
    // 1쪽: 사진(JPEG) + Flate 그림(PNG), 2쪽: 사진
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(names.iter().filter(|n| n.ends_with(".jpg")).count(), 2);
    assert_eq!(names.iter().filter(|n| n.ends_with(".png")).count(), 1);
    let want = samples::jpeg_bytes(&samples::photo_like(800, 600, 1), 95).unwrap();
    assert!(r.files.iter().any(|f| std::fs::read(f).unwrap() == want));
    let png = r.files.iter().find(|f| f.extension().unwrap() == "png").unwrap();
    let im = image::open(png).unwrap();
    assert_eq!((im.width(), im.height()), (600, 400));
    assert_eq!(im.to_rgb8().into_raw(), samples::photo_like(600, 400, 99).into_raw());
}

// ---------- 윈도 시작 시 실행 ----------

#[test]
fn run_key_value_building() {
    let exe = Path::new(r"C:\Users\홍 길동\AppData\Local\win-filetool\win-filetool.exe");
    let v = autostart::run_value(exe);
    assert_eq!(v, "\"C:\\Users\\홍 길동\\AppData\\Local\\win-filetool\\win-filetool.exe\" --autostart");
    assert_eq!(autostart::exe_of_value(&v).as_deref(), Some(r"C:\Users\홍 길동\AppData\Local\win-filetool\win-filetool.exe"));
    assert_eq!(autostart::exe_of_value(r"C:\a\b.exe --x").as_deref(), Some(r"C:\a\b.exe"));
    let st = autostart::status_from(Some(v.clone()), exe);
    assert!(st.enabled && st.matches_exe);
    // 대소문자만 다른 경로는 같은 exe
    let st2 = autostart::status_from(Some(v.to_uppercase()), exe);
    assert!(st2.matches_exe);
    let moved = autostart::status_from(Some(v), Path::new(r"D:\portable\win-filetool.exe"));
    assert!(moved.enabled && !moved.matches_exe);
    assert!(!autostart::status_from(None, exe).enabled);
    assert!(autostart::is_autostart_launch(["win-filetool.exe", "--autostart"]));
    assert!(!autostart::is_autostart_launch(["win-filetool.exe", "--tab=pdf"]));
    assert_eq!(autostart::RUN_KEY, r"Software\Microsoft\Windows\CurrentVersion\Run");
}

// ---------- 0.3.2 목표 크기 ----------

#[test]
fn compress_to_target_reaches_target_and_keeps_original() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("스캔.pdf");
    samples::make_photo_pdf(&src, 6, "SCAN", 2400).unwrap();
    let before = len(&src);
    let src_hash = std::fs::read(&src).unwrap();
    // 원본의 1/6을 목표로
    let target = before / 6;
    let mut steps = Vec::new();
    let r = pdfcompress::compress_to_target(&src, None, target, None, &mut |i, n| steps.push((i, n))).unwrap();
    let out = r.output.clone().expect("새 파일");
    assert!(r.reached, "{}", r.note);
    assert!(len(&out) <= target, "{} > {target}", len(&out));
    assert_eq!(r.target, Some(target));
    assert_eq!(filetool_core::pdf::page_count(&out).unwrap(), 6);
    assert_eq!(samples::pdf_labels(&out).unwrap().len(), 6);
    assert_eq!(std::fs::read(&src).unwrap(), src_hash, "원본 그대로");
    assert!(!steps.is_empty());
}

#[test]
fn compress_to_target_already_small_and_impossible() {
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("작은.pdf");
    samples::make_photo_pdf(&src, 1, "S", 800).unwrap();
    let r = pdfcompress::compress_to_target(&src, None, 50_000_000, None, &mut |_, _| {}).unwrap();
    assert!(r.already_small && r.output.is_none() && r.reached);
    assert!(r.note.contains("50MB"), "{}", r.note);
    // 1 KB는 닿을 수 없음 → 가장 작은 결과를 남기고 reached=false, 얼마나 줄었는지 알림
    let big = t.path().join("큰.pdf");
    samples::make_photo_pdf(&big, 2, "B", 2400).unwrap();
    let r = pdfcompress::compress_to_target(&big, None, 1000, None, &mut |_, _| {}).unwrap();
    assert!(!r.reached);
    let out = r.output.expect("가장 작은 결과는 남김");
    assert!(len(&out) < len(&big));
    assert!(r.note.contains("가장 작게 만든 것"), "{}", r.note);
    // 취소
    let c = filetool_core::process::Cancel::new();
    c.cancel();
    let e = pdfcompress::compress_to_target(&big, Some(t.path()), 1000, Some(&c), &mut |_, _| {});
    assert!(matches!(e, Err(filetool_core::Error::Cancelled)));
    assert_eq!(pdfcompress::target_label(5_000_000), "5MB");
}

#[test]
fn img2pdf_compact_keeps_a4_photos_reasonable() {
    // 0.3.2 "사진을 PDF 하나로": 아이폰 크기 JPEG·사진 같은 PNG를 A4에 넣으면 200 DPI로 줄이고 JPEG로
    let t = tempfile::tempdir().unwrap();
    let jpg = t.path().join("1.jpg");
    std::fs::write(&jpg, samples::jpeg_bytes(&samples::photo_like(4032, 3024, 1), 92).unwrap()).unwrap();
    let png = t.path().join("2.png");
    samples::photo_like(2400, 1800, 2).save(&png).unwrap();
    let shot = t.path().join("3 화면.png");
    image::RgbImage::from_fn(1200, 800, |x, _| if x < 600 { image::Rgb([255, 255, 255]) } else { image::Rgb([20, 60, 200]) }).save(&shot).unwrap();
    let inputs = vec![jpg, png, shot];
    let big = t.path().join("big.pdf");
    let small = t.path().join("small.pdf");
    img2pdf::images_to_pdf(None, &inputs, &big, &Img2PdfOpts { page: PageSize::A4Portrait, margin_mm: 10.0, compact: false }, None, &mut |_| {}).unwrap();
    img2pdf::images_to_pdf(None, &inputs, &small, &Img2PdfOpts { page: PageSize::A4Portrait, margin_mm: 10.0, compact: true }, None, &mut |_| {}).unwrap();
    assert!(len(&small) * 4 < len(&big), "{} vs {}", len(&small), len(&big));
    let imgs = samples::pdf_images(&small).unwrap();
    assert_eq!(imgs.len(), 3);
    // A4 세로 여백 10mm → 그림 폭 약 7.5인치 → 200 DPI면 약 1500px
    assert_eq!(imgs.iter().filter(|i| i.0 <= 1560 && i.2 == "DCTDecode").count(), 2, "{imgs:?}");
    // 색이 적은 화면 갈무리는 무손실 그대로(선명하게)
    assert!(imgs.iter().any(|i| (i.0, i.1) == (1200, 800) && i.2 != "DCTDecode"), "{imgs:?}");
    assert_eq!(filetool_core::pdf::page_count(&small).unwrap(), 3);
}
