//! 0.3.0 W6 한글: 형식 구분 → 할 수 있는 것 → 변환/그림 뽑기 길. 한글 97 샘플은 LibreOffice 시험 자료(MPL-2.0, tests/data/030/hwp/README.md).
//! 한글 프로그램(Windows COM)이 필요한 변환은 Windows 시험 PC에서 CLI로 점검한다. 여기서는 한글이 없는 경우를 본다.

use std::path::{Path, PathBuf};

use filetool_core::convert::Op;
use filetool_core::docs::{self, DocTarget};
use filetool_core::hwp::{self, hancom, HwpFormat, Route};
use filetool_core::tools::{SearchEnv, Tool, ToolPaths, Toolset};
use filetool_core::{docx2pptx, officeimages};

fn hwp97() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/030/hwp/hangul97-3.0.hwp")
}

fn noop(_: u32, _: u32) {}

/// CFB 한글 5 파일(그림 1개) — 코어 단위 시험의 만들기 도우미와 같은 구조를 여기서 간단히
fn small_hwp5(p: &Path) {
    use std::io::Write;
    let mut h = vec![0u8; 256];
    h[..17].copy_from_slice(b"HWP Document File");
    h[32..36].copy_from_slice(&0x0501_0001u32.to_le_bytes());
    h[36..40].copy_from_slice(&0u32.to_le_bytes()); // 압축 안 함
    let mut c = cfb::create(p).unwrap();
    c.create_stream("/FileHeader").unwrap().write_all(&h).unwrap();
    // BIN_DATA(태그 0x12): 묻음, 압축 안 함, id 1, "png"
    let mut d = vec![0x21, 0x00, 0x01, 0x00, 0x03, 0x00];
    for ch in "png".encode_utf16() {
        d.extend_from_slice(&ch.to_le_bytes());
    }
    let mut di = (0x12u32 | ((d.len() as u32) << 20)).to_le_bytes().to_vec();
    di.extend_from_slice(&d);
    c.create_stream("/DocInfo").unwrap().write_all(&di).unwrap();
    c.create_storage("/BinData").unwrap();
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[5; 30]);
    c.create_stream("/BinData/BIN0001.png").unwrap().write_all(&png).unwrap();
    c.flush().unwrap();
}

#[test]
fn hwp97_sample_routes_to_libreoffice() {
    let p = hwp97();
    let info = hwp::detect(&p).unwrap();
    assert_eq!((info.format, info.version.as_deref()), (HwpFormat::Hwp97, Some("3.00")));
    let c = hwp::capabilities_for(&info, false, true);
    assert_eq!((c.can_pdf, c.can_pptx, c.can_images, c.route), (true, true, false, Route::Libreoffice));
    assert!(c.message.as_deref().unwrap().contains("그림 뽑기"));
    // 그림 뽑기는 한글 97을 지원하지 않는다(안내 오류)
    let dir = tempfile::tempdir().unwrap();
    let e = officeimages::extract_images(&p, Some(dir.path())).unwrap_err().to_string();
    assert!(e.contains("한글 97"), "{e}");
    if hancom::available() {
        return;
    }
    assert_eq!(docs::allowed_targets(&p), vec![DocTarget::Pdf, DocTarget::Docx]);
    let ts = Toolset::discover(&ToolPaths::default(), &SearchEnv::from_system());
    let Ok(so) = ts.get(Tool::Soffice) else {
        eprintln!("LibreOffice가 없어 변환은 건너뜀");
        return;
    };
    let pdf = docs::convert(so, &p, Some(dir.path()), DocTarget::Pdf, None, docs::DOC_TIMEOUT).unwrap();
    assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF"));
    let r = docx2pptx::doc_to_pptx(&p, Some(dir.path()), Some(so), None, &mut noop).unwrap();
    assert!(r.via_libreoffice && r.slides >= 1, "{r:?}");
}

#[test]
fn modern_hwp_without_hancom_is_refused_clearly() {
    if hancom::available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("공문.hwp");
    small_hwp5(&p);
    let caps = hwp::capabilities(&p, true).unwrap();
    assert_eq!((caps.format, caps.can_pdf, caps.can_pptx, caps.can_images, caps.route), (HwpFormat::Hwp5, false, false, true, Route::None));
    assert_eq!(caps.message.as_deref(), Some(hwp::MSG_NEED_HANCOM));
    // 문서 → PDF 작업이 받지 않음(화면·감시 폴더), 직접 부르면 같은 안내
    assert!(docs::allowed_targets(&p).is_empty());
    assert!(!Op::Doc { target: DocTarget::Pdf }.accepts(&p));
    let e = docs::convert(Path::new("/no/soffice"), &p, Some(dir.path()), DocTarget::Pdf, None, docs::DOC_TIMEOUT).unwrap_err().to_string();
    assert_eq!(e, hwp::MSG_NEED_HANCOM);
    // Word → PPT도 같은 안내(LibreOffice로 보내지 않음)
    let e = docx2pptx::doc_to_pptx(&p, Some(dir.path()), Some(Path::new("/no/soffice")), None, &mut noop).unwrap_err().to_string();
    assert_eq!(e, hwp::MSG_NEED_HANCOM);
    // 그림 뽑기는 된다(항목별 '압축 안 함', 원본 바이트 그대로)
    let r = officeimages::extract_images(&p, Some(dir.path())).unwrap();
    assert_eq!(r.files.len(), 1);
    assert_eq!(r.files[0].file_name().unwrap(), "01_BIN0001.png");
    assert_eq!(r.folder.as_ref().unwrap().file_name().unwrap(), "공문_이미지");
    // .hwpx는 이제 문서로 받는다(이전에는 거절)
    assert_eq!(docs::family("hwpx"), Some(docs::DocFamily::Text));
    assert!(docx2pptx::accepts(Path::new("a.hwpx")));
}
