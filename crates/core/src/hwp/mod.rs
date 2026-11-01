//! 0.3.0 W6 한글(HWP/HWPX): 형식을 정확히 구분해서 **되는 것만** 보여 주고, 되는 길로 보낸다.
//!
//! | 형식 | 한글 프로그램 있음 | 없음 |
//! |---|---|---|
//! | 한글 97 이전 | 한글로 PDF·DOCX(→PPT) | LibreOffice로 PDF·DOCX(→PPT) |
//! | 한글 5.x(2002 이후)·HWPX | 한글로 PDF·DOCX(→PPT) | PDF·PPT 안 됨(안내), 그림 뽑기만 |
//! | 암호·보호·배포용·손상 | 안 됨(이유 안내) | 안 됨(이유 안내) |
//!
//! LibreOffice는 한글 97 이전만 읽는다(스파이크: 현대 HWP 4/4, HWPX 2/2 "source file could not be loaded").
//! 그림 뽑기는 한글 5·HWPX만([`images`]), 한글 97은 지원하지 않는다.
//!
//! - [`detect`]: 내용으로 형식 알아보기
//! - [`hancom`]: (Windows) 설치된 한글을 COM 자동화로 PDF·DOCX 저장
//! - [`images`]: 그림 원본 꺼내기

pub mod detect;
pub mod hancom;
pub mod images;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

pub use detect::{detect, is_hwp_path, HwpFormat, HwpInfo};

use crate::docs::DocTarget;
use crate::error::{Error, Result};
use crate::process::Cancel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    /// 설치된 한글 프로그램(COM 자동화)
    Hancom,
    /// 앱에 든 LibreOffice(한글 97 이전만)
    Libreoffice,
    /// 바꿀 수 없음
    None,
}

/// 화면이 되는 기능만 보여 주도록 알려 주는 값(`hwp_capabilities`)
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HwpCaps {
    /// "hwp97" | "hwp5" | "hwpx" | "damaged"
    pub format: HwpFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub encrypted: bool,
    pub distribution: bool,
    pub can_pdf: bool,
    /// Word → PPT(한글 → DOCX → PPT)
    pub can_pptx: bool,
    /// 원본 그림 뽑기
    pub can_images: bool,
    /// PDF·PPT를 만드는 길
    pub route: Route,
    /// 안 되는 것이 있을 때 사용자에게 보여 줄 짧은 안내
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl HwpCaps {
    /// 한 줄 JSON(CLI 점검용)
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

pub const MSG_NEED_HANCOM: &str = "이 한글 문서(2002년 이후 형식)는 한글 프로그램이 설치된 PC에서만 PDF·PPT로 바꿀 수 있어요. 그림 뽑기는 쓸 수 있어요.";
pub const MSG_PROTECTED: &str = "암호가 걸렸거나 보호된 한글 문서라 열 수 없어요. 한글에서 암호를 풀어 저장한 뒤 다시 해 주세요.";
pub const MSG_DISTRIBUTION: &str = "배포용 한글 문서라 바꾸거나 그림을 꺼낼 수 없어요.";
pub const MSG_DAMAGED: &str = images::MSG_DAMAGED;
pub const MSG_HWP97_NO_IMAGES: &str = "한글 97 문서는 그림 뽑기를 할 수 없어요. PDF로 바꾼 뒤 PDF에서 그림을 뽑아 주세요.";
pub const MSG_NO_TOOL: &str = "한글 97 문서를 바꿀 문서 도구(LibreOffice)를 찾지 못했어요.";

/// 형식 + 도구 유무 → 할 수 있는 것(순수 함수, 시험하기 쉽게 분리)
pub fn capabilities_for(info: &HwpInfo, hancom: bool, soffice: bool) -> HwpCaps {
    let mut c = HwpCaps {
        format: info.format,
        version: info.version.clone(),
        encrypted: info.encrypted,
        distribution: info.distribution,
        can_pdf: false,
        can_pptx: false,
        can_images: false,
        route: Route::None,
        message: None,
    };
    if info.format == HwpFormat::Damaged {
        c.message = Some(MSG_DAMAGED.into());
        return c;
    }
    if info.encrypted {
        c.message = Some(MSG_PROTECTED.into());
        return c;
    }
    if info.distribution {
        c.message = Some(MSG_DISTRIBUTION.into());
        return c;
    }
    match info.format {
        HwpFormat::Hwp97 => {
            c.route = if hancom {
                Route::Hancom
            } else if soffice {
                Route::Libreoffice
            } else {
                Route::None
            };
            c.message = Some(if c.route == Route::None { format!("{MSG_NO_TOOL} {MSG_HWP97_NO_IMAGES}") } else { MSG_HWP97_NO_IMAGES.into() });
        }
        _ => {
            c.can_images = true;
            if hancom {
                c.route = Route::Hancom;
            } else {
                c.message = Some(MSG_NEED_HANCOM.into());
            }
        }
    }
    c.can_pdf = c.route != Route::None;
    c.can_pptx = c.can_pdf;
    c
}

/// 파일 하나의 할 수 있는 것. `soffice`: 앱이 LibreOffice를 찾았는지.
pub fn capabilities(path: &Path, soffice: bool) -> Result<HwpCaps> {
    if !path.is_file() {
        return Err(Error::Invalid(format!("파일이 없습니다: {}", path.display())));
    }
    if !is_hwp_path(path) {
        return Err(Error::Unsupported(format!("한글 문서(.hwp, .hwpx)가 아닙니다: {}", path.display())));
    }
    Ok(capabilities_for(&detect(path)?, hancom::available(), soffice))
}

/// 이 한글 파일에서 고를 수 있는 변환 대상([`crate::docs::allowed_targets`]가 쓴다). LibreOffice는 있다고 본다(앱에 들어 있음).
pub fn doc_targets(path: &Path) -> Vec<DocTarget> {
    match detect(path) {
        Ok(info) if capabilities_for(&info, hancom::available(), true).can_pdf => vec![DocTarget::Pdf, DocTarget::Docx],
        Ok(_) => vec![],
        // 아직 읽을 수 없는 경로(없음 등): 확장자 기준으로 두고, 실제 변환 때 다시 확인한다
        Err(_) => vec![DocTarget::Pdf, DocTarget::Docx],
    }
}

/// 한글 문서 → PDF/DOCX. 형식과 한글 설치 여부에 따라 한글(COM) 또는 LibreOffice로 보낸다.
/// 안 되는 형식은 [`HwpCaps::message`]와 같은 안내로 거절한다.
pub fn convert(soffice: Option<&Path>, input: &Path, out_dir: Option<&Path>, target: DocTarget, cancel: Option<&Cancel>, timeout: Duration) -> Result<PathBuf> {
    if !input.is_file() {
        return Err(Error::Invalid(format!("파일이 없습니다: {}", input.display())));
    }
    if !matches!(target, DocTarget::Pdf | DocTarget::Docx) {
        return Err(Error::Unsupported(format!("{} → {}", input.display(), target.ext())));
    }
    let caps = capabilities_for(&detect(input)?, hancom::available(), soffice.is_some());
    match caps.route {
        Route::Hancom => hancom::convert(input, out_dir, target, cancel, timeout.min(hancom::TIMEOUT)).map(|r| r.output),
        Route::Libreoffice => crate::docs::convert_lo(soffice.expect("route libreoffice → soffice"), input, out_dir, target, cancel, timeout),
        Route::None => Err(Error::Invalid(caps.message.unwrap_or_else(|| MSG_NEED_HANCOM.into()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(format: HwpFormat) -> HwpInfo {
        HwpInfo { format, version: None, encrypted: false, distribution: false, compressed: true, problem: None }
    }

    #[test]
    fn caps_matrix() {
        // 한글 없음
        let c = capabilities_for(&info(HwpFormat::Hwp5), false, true);
        assert_eq!((c.can_pdf, c.can_pptx, c.can_images, c.route), (false, false, true, Route::None));
        assert_eq!(c.message.as_deref(), Some(MSG_NEED_HANCOM));
        let c = capabilities_for(&info(HwpFormat::Hwpx), false, true);
        assert_eq!((c.can_pdf, c.can_images, c.route), (false, true, Route::None));
        let c = capabilities_for(&info(HwpFormat::Hwp97), false, true);
        assert_eq!((c.can_pdf, c.can_pptx, c.can_images, c.route), (true, true, false, Route::Libreoffice));
        let c = capabilities_for(&info(HwpFormat::Hwp97), false, false);
        assert_eq!((c.can_pdf, c.route), (false, Route::None));
        // 한글 있음
        for f in [HwpFormat::Hwp5, HwpFormat::Hwpx] {
            let c = capabilities_for(&info(f), true, false);
            assert_eq!((c.can_pdf, c.can_pptx, c.can_images, c.route), (true, true, true, Route::Hancom));
            assert!(c.message.is_none());
        }
        assert_eq!(capabilities_for(&info(HwpFormat::Hwp97), true, true).route, Route::Hancom);
        // 보호·배포용·손상은 한글이 있어도 아무것도 안 됨
        for (i, msg) in [
            (HwpInfo { encrypted: true, ..info(HwpFormat::Hwp5) }, MSG_PROTECTED),
            (HwpInfo { distribution: true, ..info(HwpFormat::Hwp5) }, MSG_DISTRIBUTION),
            (HwpInfo { encrypted: true, ..info(HwpFormat::Hwpx) }, MSG_PROTECTED),
            (info(HwpFormat::Damaged), MSG_DAMAGED),
        ] {
            let c = capabilities_for(&i, true, true);
            assert_eq!((c.can_pdf, c.can_pptx, c.can_images, c.route), (false, false, false, Route::None));
            assert_eq!(c.message.as_deref(), Some(msg));
        }
        let j = serde_json::to_value(capabilities_for(&info(HwpFormat::Hwp5), false, true)).unwrap();
        assert_eq!(j["format"], "hwp5");
        assert_eq!(j["route"], "none");
        assert_eq!(j["can_images"], true);
    }

    #[test]
    fn convert_refuses_without_hancom() {
        // 이 시험 환경(Linux 또는 한글 숨김)에는 한글이 없다
        if hancom::available() {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.hwp");
        detect::testfiles::write_hwp5(&p, 1, b"", &[]);
        let e = convert(Some(Path::new("/no/soffice")), &p, Some(t.path()), DocTarget::Pdf, None, Duration::from_secs(5)).unwrap_err();
        assert_eq!(e.to_string(), MSG_NEED_HANCOM);
        assert!(doc_targets(&p).is_empty());
        let x = t.path().join("a.hwpx");
        detect::testfiles::write_hwpx(&x, &[], &[]);
        assert!(doc_targets(&x).is_empty());
        let caps = capabilities(&x, true).unwrap();
        assert_eq!((caps.format, caps.can_images, caps.route), (HwpFormat::Hwpx, true, Route::None));
        let old = t.path().join("old.hwp");
        std::fs::write(&old, b"HWP Document File V3.00 \x1a\x01\x02\x03\x04\x05").unwrap();
        assert_eq!(doc_targets(&old), vec![DocTarget::Pdf, DocTarget::Docx]);
        assert!(capabilities(&t.path().join("a.docx"), true).is_err());
    }
}
