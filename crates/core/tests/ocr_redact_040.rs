//! 0.4 글자 인식(OCR) 층·진짜 가리기. Linux에는 Windows OCR이 없어서 정해 둔 낱말을 돌려주는 [`FixedRecognizer`]로
//! 글자 층 위치·검색·가리기 경로를 시험한다(인식 정확도는 Windows PC에서 따로 잰다 — 보고서).
//! PDFium(`third_party/pdfium/linux-x64`)이 있어야 돈다.

use std::path::{Path, PathBuf};

use filetool_core::ocr::pdf::{self as ocrpdf, OcrPdfOpts};
use filetool_core::ocr::FixedRecognizer;
use filetool_core::pdfedit::redact::{self, Mark, Preset, RedactOpts};
use filetool_core::pdfedit::{testpdf, testpdf040, thumbs, Job};
use filetool_core::{pdf2docx, pdfrender};
use lopdf::{Document, Object};

fn pdfium_ok() -> bool {
    if std::env::var_os("WIN_FILETOOL_PDFIUM").is_none() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/pdfium/linux-x64/lib");
        std::env::set_var("WIN_FILETOOL_PDFIUM", p);
    }
    let ok = pdfrender::pdfium_status().is_ok();
    if !ok {
        eprintln!("PDFium 없음 — 건너뜀");
    }
    ok
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// 그림 비율 좌표 낱말
fn fixed(lines: &[&[(&str, f32, f32, f32, f32)]]) -> FixedRecognizer {
    FixedRecognizer { lines: lines.iter().map(|l| l.iter().map(|(t, x, y, w, h)| (t.to_string(), *x, *y, *w, *h)).collect()).collect(), lang: "ko".into() }
}

fn text_of(path: &Path, page: u32) -> String {
    thumbs::page_text(path, None, page).unwrap()
}

fn secrets(ko: bool) -> Vec<String> {
    let mut v: Vec<String> = testpdf040::SECRETS.iter().map(|s| s.to_string()).collect();
    if ko {
        v.push(testpdf040::SECRET_KO.into());
    }
    v
}

#[test]
fn fixture_really_contains_the_secrets() {
    if !pdfium_ok() {
        return;
    }
    let d = tmp();
    let src = d.path().join("비밀.pdf");
    let ko = testpdf040::make_secrets(&src).unwrap();
    let v = redact::verify(&src, None, &secrets(ko)).unwrap();
    assert!(!v.clean);
    assert!(v.has_info && v.has_xmp);
    // 글자 객체·주석·양식 값·문서 정보 모두에서 찾혀야 시험이 의미가 있다
    let found = |t: &str| v.text_hits.iter().any(|(_, s)| s == t);
    assert!(found("SECRET-ALPHA") && found("900101-1234567"), "{:?}", v.text_hits);
    if ko {
        assert!(found(testpdf040::SECRET_KO), "한글 글자 객체: {:?}", v.text_hits);
    }
    assert!(v.stream_hits >= 4, "주석 모양·양식 모양·첨부·XMP 스트림: {}", v.stream_hits);
    assert!(v.string_hits >= 4, "주석 내용·양식 값·제목·자바스크립트: {}", v.string_hits);
}

#[test]
fn find_presets_and_words_with_boxes() {
    if !pdfium_ok() {
        return;
    }
    let d = tmp();
    let src = d.path().join("비밀.pdf");
    testpdf040::make_secrets(&src).unwrap();
    let out = redact::find(&src, None, &["secret-alpha".into()], &[Preset::Rrn, Preset::Phone, Preset::Email, Preset::Account], None, &mut Job::none()).unwrap();
    let texts: Vec<(&str, &str, u32)> = out.hits.iter().map(|h| (h.kind.as_str(), h.text.as_str(), h.page)).collect();
    for want in [("rrn", "900101-1234567", 1), ("phone", "010-1234-5678", 1), ("email", "hong@example.com", 1), ("account", "110-123-456789", 1), ("phone", "02-123-4567", 3), ("word", "SECRET-ALPHA", 1)] {
        assert!(texts.contains(&want), "{want:?} 없음: {texts:?}");
    }
    // 1쪽 주민번호: 기준선 760(위에서 82pt), 14pt 글자 → 상자가 위에서 65~90pt 안
    let rrn = out.hits.iter().find(|h| h.kind == "rrn").unwrap();
    assert_eq!(rrn.rects.len(), 1);
    let r = rrn.rects[0];
    assert!(r.y > 62.0 && r.y + r.h < 92.0 && r.x > 150.0 && r.x + r.w < 360.0, "{r:?}");
    // 90도 쪽: 보이는 쪽은 842x595. 글은 위→아래로 흐르므로 상자가 세로로 길다
    let rot = out.hits.iter().find(|h| h.page == 3).unwrap();
    let r = rot.rects[0];
    assert!(r.h > r.w * 3.0 && r.x + r.w <= 842.0 && r.y + r.h <= 595.0, "{r:?}");
    // 그 상자 안을 그리면 검은 글자가 있어야 한다(좌표가 화면 미리보기와 같은 방향인지)
    let img = thumbs::render_rgb(&src, None, 3, 1.0).unwrap();
    let dark = (r.y as u32..(r.y + r.h) as u32).flat_map(|y| (r.x as u32..(r.x + r.w) as u32).map(move |x| (x, y))).filter(|&(x, y)| x < img.width() && y < img.height() && img.get_pixel(x, y)[0] < 100).count();
    assert!(dark > 30, "상자 안 검은 점 {dark}");
}

#[test]
fn redaction_removes_secret_from_text_annotation_field_metadata_and_more() {
    if !pdfium_ok() {
        return;
    }
    let d = tmp();
    let src = d.path().join("비밀.pdf");
    let ko = testpdf040::make_secrets(&src).unwrap();
    let mut marks = testpdf040::secret_marks();
    // 3쪽은 찾은 상자로
    let f = redact::find(&src, None, &["ROT-SECRET".into()], &[Preset::Phone], None, &mut Job::none()).unwrap();
    marks.extend(f.hits.iter().filter(|h| h.page == 3).flat_map(|h| h.rects.clone()));
    // 가린 쪽 글자 인식: 가린 곳 안(LEAKWORD)과 밖(VISIBLEWORD) 낱말 — 안의 것은 글자 층에 들어가면 안 된다
    let rec = fixed(&[&[("LEAKWORD", 0.12, 0.09, 0.2, 0.02)], &[("VISIBLEWORD", 0.12, 0.70, 0.3, 0.02)]]);
    let out_path = redact::default_output(&src, None);
    assert!(out_path.file_name().unwrap().to_string_lossy().ends_with("_가림.pdf"));
    let mut terms = secrets(ko);
    terms.push("ROT-SECRET".into());
    let opts = RedactOpts { dpi: 200, ocr: true, check_terms: terms.clone() };
    let out = redact::apply(&src, None, &marks, &opts, &out_path, Some(&rec), &mut Job::none()).unwrap();
    assert_eq!(out.redacted_pages, vec![1, 3]);
    assert!(out.verify.clean, "{:?}", out.verify);
    assert_eq!(out.verify.chars_in_marks, 0);
    assert!(out.removed.metadata && out.removed.attachments >= 1 && out.removed.javascript >= 2 && out.removed.hidden_layers >= 1);
    assert_eq!(out.removed.form_fields, 1);
    assert_eq!((out.removed.annotations, out.removed.links), (1, 1));
    // 다시 처음부터 확인(결과 파일만 보고)
    let v = redact::verify(&out_path, None, &terms).unwrap();
    assert!(v.clean && !v.has_info && !v.has_xmp, "{v:?}");
    let doc = Document::load(&out_path).unwrap();
    assert!(!doc.objects.values().any(|o| matches!(o, Object::Stream(s) if s.dict.get(b"Type").and_then(|t| t.as_name()).ok() == Some(&b"EmbeddedFile"[..]))), "첨부가 남음");
    let cat = doc.catalog().unwrap();
    for k in [&b"OCProperties"[..], b"AcroForm", b"OpenAction", b"Metadata", b"StructTreeRoot"] {
        assert!(!cat.has(k), "{} 남음", String::from_utf8_lossy(k));
    }
    // 1쪽: 그림 한 장 + 글자 층(가린 곳 밖 낱말만), 주석 없음
    let t1 = text_of(&out_path, 1);
    assert!(t1.contains("VISIBLEWORD") && !t1.contains("LEAKWORD") && !t1.contains("PUBLIC-ONE"), "{t1:?}");
    let pages = doc.get_pages();
    let p1 = doc.get_dictionary(pages[&1]).unwrap();
    assert!(!p1.has(b"Annots"));
    // 2쪽: 손대지 않음(글자 그대로, 숨긴 레이어 조각만 빠짐)
    let t2 = text_of(&out_path, 2);
    assert!(t2.contains("PUBLIC-TWO") && !t2.contains("SECRET-OCG"), "{t2:?}");
    let p2 = doc.get_dictionary(pages[&2]).unwrap();
    let res = p2.get(b"Resources").unwrap();
    let res = match res {
        Object::Reference(r) => doc.get_dictionary(*r).unwrap(),
        o => o.as_dict().unwrap(),
    };
    assert!(res.has(b"Font"), "2쪽은 글자(글꼴)가 그대로여야 함");
    // 3쪽: 회전 0 + 보이는 크기(가로로 긴 쪽)
    let p3 = doc.get_dictionary(pages[&3]).unwrap();
    assert_eq!(p3.get(b"Rotate").unwrap().as_i64().unwrap(), 0);
    let mb: Vec<f32> = p3.get(b"MediaBox").unwrap().as_array().unwrap().iter().map(|o| o.as_float().unwrap()).collect();
    assert!((mb[2] - 842.0).abs() < 0.5 && (mb[3] - 595.0).abs() < 0.5, "{mb:?}");
    // 원본은 그대로
    assert!(!redact::verify(&src, None, &terms).unwrap().clean);
}

#[test]
fn redaction_without_ocr_and_box_validation() {
    if !pdfium_ok() {
        return;
    }
    let d = tmp();
    let src = d.path().join("a.pdf");
    testpdf040::make_secrets(&src).unwrap();
    let out = d.path().join("a_가림.pdf");
    let r = redact::apply(&src, None, &[], &RedactOpts::default(), &out, None, &mut Job::none());
    assert!(r.is_err(), "가릴 곳이 없으면 오류");
    let r = redact::apply(&src, None, &[Mark { page: 9, x: 0.0, y: 0.0, w: 10.0, h: 10.0 }], &RedactOpts::default(), &out, None, &mut Job::none());
    assert!(matches!(r, Err(filetool_core::Error::PageSpec(_))));
    let o = redact::apply(&src, None, &testpdf040::secret_marks(), &RedactOpts { dpi: 300, ocr: true, check_terms: vec!["SECRET-ALPHA".into()] }, &out, None, &mut Job::none()).unwrap();
    assert!(o.hint.is_some(), "글자 인식이 없으면 안내");
    assert_eq!(o.ocr_pages, 0);
    assert!(o.verify.clean);
    assert!(text_of(&out, 1).trim().is_empty(), "글자 층 없음");
    // 300dpi 그림
    let doc = Document::load(&out).unwrap();
    let w = doc.objects.values().filter_map(|o| if let Object::Stream(s) = o { s.dict.get(b"Width").ok().and_then(|w| w.as_i64().ok()) } else { None }).max().unwrap();
    assert_eq!(w, (595.0f32 * 300.0 / 72.0).round() as i64);
}

#[test]
fn redaction_cancel_leaves_no_file() {
    if !pdfium_ok() {
        return;
    }
    let d = tmp();
    let src = d.path().join("a.pdf");
    testpdf040::make_secrets(&src).unwrap();
    let out = d.path().join("a_가림.pdf");
    let c = filetool_core::process::Cancel::new();
    c.cancel();
    let mut cb = |_: u32, _: u32| {};
    let r = redact::apply(&src, None, &testpdf040::secret_marks(), &RedactOpts::default(), &out, None, &mut Job::new(Some(&c), &mut cb));
    assert!(matches!(r, Err(filetool_core::Error::Cancelled)));
    assert!(!out.exists());
}

/// 스캔본 만들기: 큰 글자 P1.. 쪽 → 그림만 있는 PDF
fn scan(dir: &Path, specs: &[testpdf::PageSpec]) -> PathBuf {
    let txt = dir.join("글.pdf");
    testpdf::make_pages(&txt, specs, "P").unwrap();
    let sc = dir.join("스캔.pdf");
    testpdf040::make_scan(&txt, &sc, 100, 0).unwrap();
    sc
}

#[test]
fn searchable_layer_is_invisible_and_positioned() {
    if !pdfium_ok() {
        return;
    }
    let d = tmp();
    let sc = scan(d.path(), &[(595.0, 842.0, 0), (595.0, 842.0, 0)]);
    let rec = fixed(&[&[("검색", 0.10, 0.10, 0.20, 0.03), ("HELLO", 0.35, 0.10, 0.25, 0.03)], &[("둘째줄", 0.10, 0.20, 0.3, 0.03)]]);
    let out = d.path().join("스캔_글자인식.pdf");
    let mut seen = Vec::new();
    let mut cb = |i: u32, n: u32| seen.push((i, n));
    let o = ocrpdf::make_searchable(&sc, None, &OcrPdfOpts::default(), &out, &rec, &mut Job::new(None, &mut cb)).unwrap();
    assert_eq!((o.ocr_pages, o.skipped_text_pages, o.words), (2, 0, 6));
    assert_eq!(seen.last(), Some(&(2, 2)));
    let t = text_of(&out, 1);
    assert!(t.contains("검색 HELLO") && t.contains("둘째줄"), "{t:?}");
    // 모양은 그대로(보이지 않는 글자)
    let a = thumbs::render_rgb(&sc, None, 1, 0.5).unwrap();
    let b = thumbs::render_rgb(&out, None, 1, 0.5).unwrap();
    assert_eq!(a.dimensions(), b.dimensions());
    let diff = a.pixels().zip(b.pixels()).filter(|(p, q)| p != q).count();
    assert_eq!(diff, 0, "보이는 모양이 바뀜");
    // 찾기(글자 층의 글자 상자)도 낱말 자리
    let f = redact::find(&out, None, &["HELLO".into()], &[], None, &mut Job::none()).unwrap();
    let r = f.hits[0].rects[0];
    let (vw, vh) = (595.0, 842.0);
    assert!((r.x - 0.35 * vw).abs() < 4.0 && (r.y - 0.10 * vh).abs() < 4.0 && (r.w - 0.25 * vw).abs() < 8.0, "{r:?}");
    // 이미 글자가 있는 쪽은 건너뛴다
    let out2 = d.path().join("again.pdf");
    let e = ocrpdf::make_searchable(&out, None, &OcrPdfOpts::default(), &out2, &rec, &mut Job::none()).unwrap_err();
    assert!(e.to_string().contains("이미 글자가"), "{e}");
    // 다시 인식을 켜면 원래 글자와 겹치는 낱말은 빼고(여기서는 모두 겹침) 아무것도 더하지 않음
    let e = ocrpdf::make_searchable(&out, None, &OcrPdfOpts { redo_text_pages: true, ..Default::default() }, &out2, &rec, &mut Job::none()).unwrap_err();
    assert!(e.to_string().contains("글자를 찾지 못했어요"), "{e}");
}

#[test]
fn searchable_layer_on_rotated_page() {
    if !pdfium_ok() {
        return;
    }
    let d = tmp();
    // 쪽 /Rotate 90: 보이는 쪽은 842x595
    let txt = d.path().join("r.pdf");
    testpdf::make_pages(&txt, &[(595.0, 842.0, 0)], "R").unwrap();
    let sc = d.path().join("rs.pdf");
    testpdf040::make_scan(&txt, &sc, 100, 0).unwrap();
    let mut doc = Document::load(&sc).unwrap();
    let pid = doc.get_pages()[&1];
    doc.get_dictionary_mut(pid).unwrap().set("Rotate", 90);
    let sc2 = d.path().join("rs90.pdf");
    doc.save(&sc2).unwrap();
    let rec = fixed(&[&[("ROTATED", 0.50, 0.40, 0.30, 0.05)]]);
    let out = d.path().join("rs90_ocr.pdf");
    ocrpdf::make_searchable(&sc2, None, &OcrPdfOpts::default(), &out, &rec, &mut Job::none()).unwrap();
    let f = redact::find(&out, None, &["ROTATED".into()], &[], None, &mut Job::none()).unwrap();
    let r = f.hits[0].rects[0];
    let (vw, vh) = (842.0, 595.0);
    assert!((r.x - 0.50 * vw).abs() < 5.0 && (r.y - 0.40 * vh).abs() < 5.0 && (r.w - 0.30 * vw).abs() < 10.0, "{r:?}");
}

#[test]
fn ocr_text_and_docx() {
    if !pdfium_ok() {
        return;
    }
    let d = tmp();
    let sc = scan(d.path(), &[(595.0, 842.0, 0)]);
    let rec = fixed(&[&[("첫", 0.1, 0.1, 0.05, 0.02), ("문단입니다", 0.16, 0.1, 0.3, 0.02)], &[("이어지는", 0.1, 0.125, 0.25, 0.02), ("줄", 0.36, 0.125, 0.05, 0.02)], &[("둘째", 0.1, 0.3, 0.2, 0.02), ("문단", 0.31, 0.3, 0.1, 0.02)]]);
    let t = ocrpdf::file_to_txt(&sc, None, None, None, &rec, &mut Job::none()).unwrap();
    assert!(t.output.file_name().unwrap().to_string_lossy().ends_with("_글자.txt"));
    let bytes = std::fs::read(&t.output).unwrap();
    assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF]);
    let s = String::from_utf8_lossy(&bytes[3..]);
    assert!(s.contains("첫 문단입니다\r\n이어지는 줄\r\n\r\n둘째 문단"), "{s:?}");
    assert_eq!(t.ocr_pages, 1);
    // 그림 파일
    let png = d.path().join("사진.png");
    image::RgbImage::from_pixel(800, 600, image::Rgb([250, 250, 250])).save(&png).unwrap();
    let t = ocrpdf::file_to_txt(&png, None, None, None, &rec, &mut Job::none()).unwrap();
    assert!(t.text.starts_with("첫 문단입니다"));
    // PDF → Word: 기본은 쪽 그림, 글자 인식 켜면 문단
    let a = pdf2docx::pdf_to_docx(&sc, Some(d.path()), None, &mut |_, _| {}).unwrap();
    assert_eq!((a.scanned_pages, a.ocr_pages), (1, 0));
    let sub = d.path().join("ocr");
    std::fs::create_dir(&sub).unwrap();
    let b = pdf2docx::pdf_to_docx_with(&sc, Some(&sub), Some(&rec), None, &mut |_, _| {}).unwrap();
    assert_eq!((b.scanned_pages, b.ocr_pages), (0, 1), "{}", b.note);
    assert!(b.paragraphs >= 2, "{}", b.note);
    let mut z = zip::ZipArchive::new(std::fs::File::open(&b.output).unwrap()).unwrap();
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut z.by_name("word/document.xml").unwrap(), &mut xml).unwrap();
    assert!(xml.contains("문단입니다") && xml.contains("둘째"), "본문에 인식 글");
}
