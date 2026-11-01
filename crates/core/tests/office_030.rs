//! 0.3.0 W2 문서 변환 — `tests/data/030`(make_corpus.py로 만든 한국어 샘플)으로 구조 숫자를 확인한다(픽셀 비교 아님).
//! PDF 쪽은 PDFium(`third_party/pdfium/linux-x64`, scripts/fetch-pdfium.sh)이 있어야 돈다(없으면 건너뜀).
//! `.odt` → PPT는 LibreOffice(soffice)를 찾을 때만 돈다.

use std::io::Read;
use std::path::{Path, PathBuf};

use filetool_core::pdf2pptx::PptMode;
use filetool_core::tools::{SearchEnv, Tool, ToolPaths, Toolset};
use filetool_core::{docx2pptx, officeimages, pdf2docx, pdf2pptx, pdflayout, pdfrender};

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/030").join(name)
}

fn pdfium_ok() -> bool {
    if std::env::var_os("WIN_FILETOOL_PDFIUM").is_none() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/pdfium/linux-x64/lib");
        std::env::set_var("WIN_FILETOOL_PDFIUM", p);
    }
    let ok = pdfrender::pdfium_status().is_ok();
    if !ok {
        eprintln!("PDFium이 없어 건너뜀");
    }
    ok
}

fn part(zip_path: &Path, name: &str) -> String {
    let mut z = zip::ZipArchive::new(std::fs::File::open(zip_path).unwrap()).unwrap();
    let mut s = String::new();
    z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

fn parts_matching(zip_path: &Path, prefix: &str) -> Vec<String> {
    let z = zip::ZipArchive::new(std::fs::File::open(zip_path).unwrap()).unwrap();
    let mut v: Vec<String> = z.file_names().filter_map(|n| n.ok().map(|n| n.into_owned())).filter(|n| n.starts_with(prefix)).collect();
    v.sort();
    v
}

/// docx 본문 글자(문단마다 한 줄)
fn docx_text(p: &Path) -> String {
    let xml = part(p, "word/document.xml");
    let mut out = String::new();
    for para in xml.split("</w:p>") {
        let mut line = String::new();
        for t in para.split("<w:t xml:space=\"preserve\">").skip(1) {
            line.push_str(t.split("</w:t>").next().unwrap_or(""));
        }
        if !line.is_empty() {
            out.push_str(&line.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">"));
            out.push('\n');
        }
    }
    out
}

fn count(s: &str, pat: &str) -> usize {
    s.matches(pat).count()
}

fn noop(_: u32, _: u32) {}

// ---------- Word → PPT ----------

#[test]
fn word_to_ppt_structure() {
    let dir = tempfile::tempdir().unwrap();
    let r = docx2pptx::doc_to_pptx(&data("보고서.docx"), Some(dir.path()), None, None, &mut noop).unwrap();
    assert_eq!(r.output.file_name().unwrap().to_string_lossy(), "보고서_PPT.pptx");
    // 제목 슬라이드 1 + 요약 1 + 지역별 실적(구역) 1 + 세부 숫자(글+표) 1 + 그래프(글+그림) 1 + 다음 분기 계획 2(9줄 → 5+4) + 현장 사진 2(글, 사진) + 부록 1
    assert_eq!((r.sections, r.images, r.tables), (7, 2, 1 + 1), "{r:?}");
    assert_eq!(r.slides, 11, "{r:?}");
    let slides = parts_matching(&r.output, "ppt/slides/slide");
    assert_eq!(slides.iter().filter(|n| n.ends_with(".xml")).count(), 11);
    let all: String = (1..=11).map(|i| part(&r.output, &format!("ppt/slides/slide{i}.xml"))).collect();
    assert!(part(&r.output, "ppt/slides/slide1.xml").contains("2026년 3분기 영업 보고서"));
    assert!(all.contains("다음 분기 계획 (계속)"));
    assert!(all.contains(r#"startAt="6""#) || all.contains(r#"startAt="7""#), "나눠진 번호 목록은 이어 셈");
    // 그림 설명은 글머리표 없는 글(buNone)로 그림 아래에
    let cap = all.find("그림 1. 월별 매출 추이").expect("캡션");
    let before = &all[..cap];
    let ppr = &before[before.rfind("<a:pPr").unwrap()..];
    assert!(ppr.contains("<a:buNone/>"), "캡션이 글머리표가 됨: {ppr}");
    // 머리글·바닥글은 들어가지 않음
    assert!(!all.contains("내부 보고서") && !all.contains("무단 배포"));
    // 짧은 글 + 표가 한 장(세부 숫자)
    let s = (1..=11).map(|i| part(&r.output, &format!("ppt/slides/slide{i}.xml"))).find(|x| x.contains("세부 숫자")).unwrap();
    assert!(s.contains("<a:tbl>") && s.contains("아래 표는"));
    // 표 안 사진도 슬라이드에 그림으로
    assert_eq!(parts_matching(&r.output, "ppt/media/").len(), 2);
    // 원본은 그대로, 두 번째 결과는 (1)
    let r2 = docx2pptx::doc_to_pptx(&data("보고서.docx"), Some(dir.path()), None, None, &mut noop).unwrap();
    assert_eq!(r2.output.file_name().unwrap().to_string_lossy(), "보고서_PPT (1).pptx");
}

#[test]
fn word_to_ppt_odt_via_libreoffice() {
    let ts = Toolset::discover(&ToolPaths::default(), &SearchEnv::from_system());
    let Ok(so) = ts.get(Tool::Soffice) else {
        eprintln!("LibreOffice가 없어 건너뜀");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let r = docx2pptx::doc_to_pptx(&data("보고서.odt"), Some(dir.path()), Some(so), None, &mut noop).unwrap();
    assert!(r.via_libreoffice);
    assert_eq!((r.sections, r.images), (7, 2), "{r:?}");
    // soffice 없이 odt는 쉬운 말 오류
    let e = docx2pptx::doc_to_pptx(&data("보고서.odt"), Some(dir.path()), None, None, &mut noop).unwrap_err().to_string();
    assert!(e.contains("LibreOffice"), "{e}");
}

#[test]
fn word_to_ppt_rejects_other_files() {
    let dir = tempfile::tempdir().unwrap();
    let e = docx2pptx::doc_to_pptx(&data("보고서_LO.pdf"), Some(dir.path()), None, None, &mut noop).unwrap_err().to_string();
    assert!(e.contains("Word"), "{e}");
    let bad = dir.path().join("깨짐.docx");
    std::fs::write(&bad, b"not a zip").unwrap();
    let e = docx2pptx::doc_to_pptx(&bad, Some(dir.path()), None, None, &mut noop).unwrap_err().to_string();
    assert!(e.contains("손상"), "{e}");
}

// ---------- PDF → Word ----------

#[test]
fn pdf_to_word_libreoffice_pdf() {
    if !pdfium_ok() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let r = pdf2docx::pdf_to_docx(&data("보고서_LO.pdf"), Some(dir.path()), None, &mut noop).unwrap();
    assert_eq!(r.output.file_name().unwrap().to_string_lossy(), "보고서_LO_Word.docx");
    // Title 1 + H1 5 + H2 2 + H3 1, 글머리표 4 + 번호 9, 표 1(5줄), 그림 2(그래프, 표 안 사진)
    assert_eq!((r.pages, r.headings, r.list_items, r.tables, r.images, r.scanned_pages), (3, 9, 13, 1, 2, 0), "{r:?}");
    assert!(r.dropped_lines >= 6, "머리글·바닥글(쪽 번호) 3쪽 × 2줄: {r:?}");
    let doc = part(&r.output, "word/document.xml");
    assert_eq!(count(&doc, r#"<w:pStyle w:val="Title"/>"#), 1);
    assert_eq!(count(&doc, r#"<w:pStyle w:val="Heading1"/>"#), 5);
    assert_eq!(count(&doc, r#"<w:pStyle w:val="ListNumber"/>"#), 9);
    assert_eq!(count(&doc, r#"<w:pStyle w:val="Caption"/>"#), 1);
    assert_eq!(count(&doc, "<w:tr>"), 5);
    assert!(doc.contains("<w:drawing>"));
    // 2단계 글머리표(수도권)
    assert!(doc.contains(r#"<w:ilvl w:val="1"/>"#));
    let text = docx_text(&r.output);
    // 한글 줄 잇기: 글자 단위로 끊긴 곳은 붙이고, 낱말 사이에서 끊긴 곳은 띄운다
    for w in ["온라인 주문", "들어온 문의는", "지켜졌습니다. 다음", "지적된 문제를"] {
        assert!(text.contains(w), "'{w}' 없음:\n{text}");
    }
    for bad in ["온 라인", "지적 된", "들어온문의", "습니 다", "주식회사 샘플", "무단 배포"] {
        assert!(!text.contains(bad), "'{bad}' 있음:\n{text}");
    }
    // 쪽을 넘는 문단·쪽 끝 문단이 끊기지 않음(메모 4개가 각각 한 문단)
    assert_eq!(text.lines().filter(|l| l.starts_with("메모 ")).count(), 4, "{text}");
    assert!(text.lines().any(|l| l.starts_with("메모 1.") && l.ends_with("읽어야 합니다.")), "{text}");
}

#[test]
fn pdf_to_word_edge_print() {
    if !pdfium_ok() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let r = pdf2docx::pdf_to_docx(&data("엣지인쇄.pdf"), Some(dir.path()), None, &mut noop).unwrap();
    // Title + H 3, 도형 글머리표 3, 표 1(머리글 + 3줄, 3열), 사진 1
    assert_eq!((r.headings, r.list_items, r.tables, r.images), (4, 3, 1, 1), "{r:?}");
    let doc = part(&r.output, "word/document.xml");
    assert_eq!(count(&doc, "<w:gridCol "), 3, "선 있는 표, 가운데 정렬 머리글도 3열");
    assert_eq!(count(&doc, "<w:tblHeader/>"), 1, "굵은 첫 줄 = 머리글");
    assert!(doc.contains("<w:hyperlink r:id=\"rIdLink1\""));
    assert!(part(&r.output, "word/_rels/document.xml.rels").contains("https://example.com/new"));
    let text = docx_text(&r.output);
    assert!(text.contains("주말과 공휴일에는 누리집"), "줄 끝 빈칸 없는 PDF의 글자 단위 줄바꿈:\n{text}");
    assert!(text.contains("59만 원"));
}

#[test]
fn pdf_to_word_two_columns_reading_order() {
    if !pdfium_ok() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let r = pdf2docx::pdf_to_docx(&data("두단.pdf"), Some(dir.path()), None, &mut noop).unwrap();
    assert_eq!(r.two_column_pages, 1, "{r:?}");
    let text = docx_text(&r.output);
    let pos = |s: &str| text.find(s).unwrap_or_else(|| panic!("'{s}' 없음:\n{text}"));
    assert!(pos("왼쪽 단 시작") < pos("[1]") && pos("[1]") < pos("[2]") && pos("[3]") < pos("[4]") && pos("[4]") < pos("[5]") && pos("[6]") < pos("오른쪽 단 끝"));
    // 왼쪽 단 끝 → 오른쪽 단 첫 줄로 이어지는 문단은 하나
    let p4 = text.lines().find(|l| l.starts_with("[4]")).unwrap();
    assert!(p4.ends_with("읽어야 합니다."), "{p4}");
    assert!(!text.contains("무단 배포"), "쪽 번호가 붙은 바닥글은 한 쪽짜리여도 뺌");
}

#[test]
fn pdf_to_word_borderless_table_bold_italic_link() {
    if !pdfium_ok() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let r = pdf2docx::pdf_to_docx(&data("선없는표.pdf"), Some(dir.path()), None, &mut noop).unwrap();
    assert_eq!((r.headings, r.tables), (1, 1), "{r:?}");
    let doc = part(&r.output, "word/document.xml");
    assert_eq!(count(&doc, "<w:gridCol "), 4, "테두리 없는 표도 4열(가운데 머리글·오른쪽 정렬 숫자)");
    assert_eq!(count(&doc, "<w:tr>"), 4);
    assert_eq!(count(&doc, "<w:tblHeader/>"), 1);
    // 굵게·기울임·링크
    let bold_run = doc.split("<w:r>").find(|r| r.contains("굵은 글씨")).unwrap();
    assert!(bold_run.contains("<w:b/>"), "{bold_run}");
    let it_run = doc.split("<w:r>").find(|r| r.contains("기울인 글씨")).unwrap();
    assert!(it_run.contains("<w:i/>"), "{it_run}");
    assert!(part(&r.output, "word/_rels/document.xml.rels").contains("https://example.org/report"));
    let text = docx_text(&r.output);
    assert!(text.contains("2,180") && text.contains("표 아래 문단입니다."));
}

#[test]
fn word_to_ppt_keeps_emf() {
    use filetool_core::ooxml::docx::{Block, Doc, Run};
    let dir = tempfile::tempdir().unwrap();
    // EMF 머리(레코드 1, 40바이트 위치에 " EMF")를 가진 그림
    let mut emf = vec![1u8, 0, 0, 0, 108, 0, 0, 0];
    emf.resize(40, 0);
    emf.extend_from_slice(b" EMF");
    emf.resize(120, 7);
    let mut d = Doc::new("emf");
    d.blocks = vec![Block::Heading { level: 1, runs: vec![Run::plain("도형")] }, Block::Image { bytes: emf.clone(), name: "chart.emf".into(), w: 914_400, h: 457_200 }];
    let src = dir.path().join("도형.docx");
    d.save(&src).unwrap();
    let r = docx2pptx::doc_to_pptx(&src, Some(dir.path()), None, None, &mut noop).unwrap();
    assert_eq!(parts_matching(&r.output, "ppt/media/"), vec!["ppt/media/image1.emf".to_string()]);
    let mut z = zip::ZipArchive::new(std::fs::File::open(&r.output).unwrap()).unwrap();
    let mut b = Vec::new();
    z.by_name("ppt/media/image1.emf").unwrap().read_to_end(&mut b).unwrap();
    assert_eq!(b, emf, "EMF는 바꾸지 않고 그대로");
    assert!(part(&r.output, "[Content_Types].xml").contains(r#"Extension="emf" ContentType="image/x-emf""#));
}

#[test]
fn pdf_to_word_scanned_rotated_encrypted() {
    if !pdfium_ok() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let r = pdf2docx::pdf_to_docx(&data("스캔본.pdf"), Some(dir.path()), None, &mut noop).unwrap();
    assert_eq!((r.pages, r.scanned_pages, r.headings, r.paragraphs), (2, 2, 0, 0), "{r:?}");
    assert_eq!(parts_matching(&r.output, "word/media/").len(), 2);
    assert!(part(&r.output, "word/document.xml").contains(r#"<w:br w:type="page"/>"#));

    let r = pdf2docx::pdf_to_docx(&data("회전쪽.pdf"), Some(dir.path()), None, &mut noop).unwrap();
    let text = docx_text(&r.output);
    assert!(text.contains("다음 분기 계획") && text.contains("배송 체계를 다시 점검합니다"), "돌린 쪽의 글:\n{text}");
    assert_eq!(r.tables, 1);

    let e = pdf2docx::pdf_to_docx(&data("암호.pdf"), Some(dir.path()), None, &mut noop).unwrap_err().to_string();
    assert!(e.contains("암호가 걸린 PDF"), "{e}");
    assert!(std::fs::read_dir(dir.path()).unwrap().all(|f| !f.unwrap().file_name().to_string_lossy().starts_with("암호")), "실패하면 파일을 남기지 않음");
}

#[test]
fn layout_frame_for_rotated_text() {
    if !pdfium_ok() {
        return;
    }
    let l = pdflayout::analyze(&data("회전쪽.pdf"), &pdflayout::AnalyzeOpts { images: false, render_scanned: false, keep_running: false }, None, &mut noop).unwrap();
    // 2쪽은 /Rotate 90: 글자는 사용자 좌표에서 가로이므로 틀은 그대로, 화면과는 방향이 다름
    assert!(l.pages[0].upright && !l.pages[1].upright);
    assert!(l.pages[1].disp_w > l.pages[1].disp_h);
    assert!(l.trailing_spaces, "LibreOffice PDF는 줄 끝 빈칸을 남김");
}

// ---------- PDF → TXT ----------

#[test]
fn pdf_to_txt_bom_pages_korean() {
    if !pdfium_ok() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let r = pdf2docx::pdf_to_txt(&data("보고서_LO.pdf"), Some(dir.path()), None, &mut noop).unwrap();
    assert_eq!(r.output.file_name().unwrap().to_string_lossy(), "보고서_LO.txt");
    let b = std::fs::read(&r.output).unwrap();
    assert_eq!(&b[..3], &[0xEF, 0xBB, 0xBF], "메모장용 BOM");
    let t = String::from_utf8(b[3..].to_vec()).unwrap();
    assert!(t.contains("\r\n") && !t.replace("\r\n", "").contains('\n'), "CRLF");
    assert!(t.contains("\r\n— 2쪽 —\r\n\r\n") && t.contains("— 3쪽 —"));
    assert!(t.contains("1. 배송 체계를 다시 점검합니다") && t.contains("9. 분기 말에"));
    assert!(t.contains("지역\t매출(억 원)\t증감"));
    assert!(t.contains("지적된 문제") && !t.contains("지적 된"));
    assert!(!t.contains("무단 배포"));
    assert_eq!(r.pages, 3);

    let s = pdf2docx::pdf_to_txt(&data("스캔본.pdf"), Some(dir.path()), None, &mut noop).unwrap();
    assert_eq!(s.scanned_pages, 2);
}

// ---------- PDF → PPT ----------

#[test]
fn pdf_to_ppt_image_and_editable() {
    if !pdfium_ok() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let r = pdf2pptx::pdf_to_pptx(&data("보고서_LO.pdf"), Some(dir.path()), PptMode::Image, None, None, &mut noop).unwrap();
    assert_eq!(r.output.file_name().unwrap().to_string_lossy(), "보고서_LO_PPT.pptx");
    assert_eq!((r.slides, r.pictures, r.text_boxes), (3, 3, 0));
    // 슬라이드 크기 = A4 세로(595.3 × 841.9pt)
    let pres = part(&r.output, "ppt/presentation.xml");
    assert!(pres.contains(r#"<p:sldSz cx="756"#) && pres.contains(r#" cy="1069"#), "{pres}");

    let e = pdf2pptx::pdf_to_pptx(&data("보고서_LO.pdf"), Some(dir.path()), PptMode::Editable, None, None, &mut noop).unwrap();
    assert_eq!(e.output.file_name().unwrap().to_string_lossy(), "보고서_LO_PPT (1).pptx");
    assert_eq!((e.slides, e.image_pages), (3, 0));
    assert!(e.text_boxes >= 20 && e.pictures == 2 && e.shapes > 0, "{e:?}");
    let s1 = part(&e.output, "ppt/slides/slide1.xml");
    // 글상자 글자(run 여러 개로 나뉠 수 있음)
    let paras: Vec<String> = s1.split("</a:p>").map(|p| p.split("<a:t>").skip(1).map(|t| t.split("</a:t>").next().unwrap_or("")).collect()).collect();
    assert!(paras.iter().any(|p| p == "2026년 3분기 영업 보고서"), "{paras:?}");
    assert!(s1.contains(r#"wrap="none""#));
    // 돌아간 쪽·스캔본은 쪽 그림
    let rot = pdf2pptx::pdf_to_pptx(&data("회전쪽.pdf"), Some(dir.path()), PptMode::Editable, None, None, &mut noop).unwrap();
    assert_eq!(rot.image_pages, 1);
    let sc = pdf2pptx::pdf_to_pptx(&data("스캔본.pdf"), Some(dir.path()), PptMode::Editable, None, None, &mut noop).unwrap();
    assert_eq!((sc.slides, sc.image_pages, sc.text_boxes), (2, 2, 0));
    let enc = pdf2pptx::pdf_to_pptx(&data("암호.pdf"), Some(dir.path()), PptMode::Image, None, None, &mut noop).unwrap_err().to_string();
    assert!(enc.contains("암호"));
}

// ---------- 이미지만 뽑기 ----------

#[test]
fn extract_images_office_and_pdf() {
    let dir = tempfile::tempdir().unwrap();
    let r = officeimages::extract_images(&data("보고서.docx"), Some(dir.path())).unwrap();
    let folder = r.folder.clone().unwrap();
    assert_eq!(folder.file_name().unwrap().to_string_lossy(), "보고서_이미지");
    let names: Vec<String> = r.files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names[0].starts_with("01_") && names[0].ends_with(".png") && names[1].starts_with("02_") && (names[1].ends_with(".jpeg") || names[1].ends_with(".jpg")), "문서 순서(그래프 → 표 안 사진): {names:?}");
    // 원본 바이트 그대로
    let orig = names[0].trim_start_matches("01_");
    let mut z = zip::ZipArchive::new(std::fs::File::open(data("보고서.docx")).unwrap()).unwrap();
    let mut b = Vec::new();
    z.by_name(&format!("word/media/{orig}")).unwrap().read_to_end(&mut b).unwrap();
    assert_eq!(std::fs::read(&r.files[0]).unwrap(), b);

    let p = officeimages::extract_images(&data("이미지모음.pptx"), Some(dir.path())).unwrap();
    assert_eq!(p.files.len(), 2, "같은 그림을 두 슬라이드에 써도 한 번: {p:?}");
    let x = officeimages::extract_images(&data("그림표.xlsx"), Some(dir.path())).unwrap();
    assert_eq!(x.files.len(), 2);

    if pdfium_ok() {
        let f = officeimages::extract_images(&data("엣지인쇄.pdf"), Some(dir.path())).unwrap();
        assert_eq!(f.folder.unwrap().file_name().unwrap().to_string_lossy(), "엣지인쇄_이미지");
        assert_eq!(f.files.len(), 1);
        let e = officeimages::extract_images(&data("암호.pdf"), Some(dir.path())).unwrap_err().to_string();
        assert!(e.contains("암호가 걸린 PDF에서는 그림을 꺼낼 수 없습니다"), "{e}");
    }
    let e = officeimages::extract_images(&data("make_corpus.py"), Some(dir.path())).unwrap_err().to_string();
    assert!(e.contains("Word·PowerPoint·Excel·PDF"), "{e}");
    // 다시 하면 새 폴더 (1)
    let again = officeimages::extract_images(&data("보고서.docx"), Some(dir.path())).unwrap();
    assert_eq!(again.folder.unwrap().file_name().unwrap().to_string_lossy(), "보고서_이미지 (1)");
}

#[test]
fn cancel_stops_pdf_jobs() {
    if !pdfium_ok() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let c = filetool_core::process::Cancel::new();
    c.cancel();
    let e = pdf2docx::pdf_to_docx(&data("보고서_LO.pdf"), Some(dir.path()), Some(&c), &mut noop).unwrap_err();
    assert!(matches!(e, filetool_core::Error::Cancelled));
    let e = pdf2pptx::pdf_to_pptx(&data("보고서_LO.pdf"), Some(dir.path()), PptMode::Image, None, Some(&c), &mut noop).unwrap_err();
    assert!(matches!(e, filetool_core::Error::Cancelled));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "취소하면 결과 파일 없음");
}
