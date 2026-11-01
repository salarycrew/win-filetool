//! 0.3.1 한글 → PPT: 설치된 한글(2020)이 SaveAs로 만든 docx(`tests/data/031`, 직접 만든 시험 문서, 작성자 정보 지움)에는
//! 제목(Heading) 스타일이 없다. 글자 크기·굵기·번호로 제목을 찾아 여러 장으로 나뉘는지, 제목 스타일이 있는 Word 문서는 그대로인지.

use std::path::{Path, PathBuf};

use filetool_core::docx2pptx::{self, DocBlock};

fn data(dir: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(dir).join(name)
}

fn noop(_: u32, _: u32) {}

#[test]
fn hancom_docx_gets_slides_per_heading() {
    let p = data("031", "한글저장_운영계획.docx");
    let (blocks, notes) = docx2pptx::read_docx(&p).unwrap();
    let heads: Vec<(u32, String)> = blocks.iter().filter_map(|b| if let DocBlock::Heading { level, text } = b { Some((*level, text.clone())) } else { None }).collect();
    assert_eq!(
        heads,
        vec![
            (0, "2026년 사내 도서관 운영 계획".to_string()),
            (1, "Ⅰ. 추진 배경".into()),
            (1, "Ⅱ. 운영 목표".into()),
            (2, "1. 이용 시간 늘리기".into()),
            (2, "2. 대출 관리 바꾸기".into()),
            (2, "3. 책 늘리기".into()),
            (1, "Ⅲ. 일정과 예산".into()),
            (3, "가. 일정".into()),
            (3, "나. 예산".into()),
            (1, "Ⅳ. 기대 효과".into()),
        ]
    );
    assert!(notes.iter().any(|n| n.contains("제목 9개")), "{notes:?}");
    let dir = tempfile::tempdir().unwrap();
    let r = docx2pptx::doc_to_pptx(&p, Some(dir.path()), None, None, &mut noop).unwrap();
    // 표지 1 + Ⅰ 1 + Ⅱ(내용 없음 → 구역 표시) 1 + 1.·2.·3. 3 + Ⅲ(가.·나.는 굵은 줄) 1 + Ⅳ 1 = 8 (0.3.0은 제목 1개·한 덩어리)
    assert_eq!((r.slides, r.sections), (8, 7), "{}", r.note);
}

#[test]
fn styled_word_document_unchanged() {
    // 0.3.0과 같은 결과(제목 스타일이 있는 Word 문서는 짐작하지 않음)
    let dir = tempfile::tempdir().unwrap();
    let r = docx2pptx::doc_to_pptx(&data("030", "보고서.docx"), Some(dir.path()), None, None, &mut noop).unwrap();
    assert!(!r.note.contains("찾았습니다"), "{}", r.note);
    let (blocks, _) = docx2pptx::read_docx(&data("030", "보고서.docx")).unwrap();
    assert!(blocks.iter().filter(|b| matches!(b, DocBlock::Heading { level: 1.., .. })).count() >= 3);
}
