//! 0.3.0 공용 OOXML 쓰기(외부 라이브러리 없이 zip + XML 문자열).
//!
//! - [`pptx`]: 16:9(또는 지정 크기) 슬라이드, 마스터 1·빈 레이아웃 1·테마 1(맑은 고딕). 자리 표시자 없이
//!   위치를 직접 정한 글상자·그림·표·사각형·선만 쓰므로 PowerPoint와 LibreOffice에서 같게 보인다.
//! - [`docx`]: 스타일 Title / Heading1-3 / 목록(글머리표·번호) / 표 / 캡션, 하이퍼링크, 그림(인라인), 쪽 나누기.
//!
//! 두 모듈 모두 결과를 메모리에서 만든 뒤 [`crate::paths::write_new`]로 새 파일로만 쓴다(덮어쓰지 않음).

pub mod docx;
pub mod extref;
pub mod pptx;

use std::io::{Cursor, Write};
use std::path::Path;

use crate::error::{Error, Result};

/// 기본 글꼴: 라틴 이름과 한국어 이름(한국어판 Office가 쓰는 표기)
pub const FONT_LATIN: &str = "Malgun Gothic";
pub const FONT_EA: &str = "맑은 고딕";

/// XML 텍스트·속성 값 이스케이프(제어 문자는 버림)
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\t' | '\n' => o.push(c),
            c if (c as u32) < 0x20 => {}
            '\u{FFFE}' | '\u{FFFF}' => {}
            c => o.push(c),
        }
    }
    o
}

/// 파일 이름(확장자) → (zip 안 확장자, Content-Type)
pub fn media_type(name: &str) -> (&'static str, &'static str) {
    let e = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match e.as_str() {
        "png" => ("png", "image/png"),
        "jpg" | "jpeg" | "jpe" => ("jpeg", "image/jpeg"),
        "gif" => ("gif", "image/gif"),
        "bmp" => ("bmp", "image/bmp"),
        "tif" | "tiff" => ("tiff", "image/tiff"),
        "emf" => ("emf", "image/x-emf"),
        "wmf" => ("wmf", "image/x-wmf"),
        "svg" => ("svg", "image/svg+xml"),
        "wdp" | "hdp" | "jxr" => ("wdp", "image/vnd.ms-photo"),
        "webp" => ("webp", "image/webp"),
        _ => ("bin", "application/octet-stream"),
    }
}

/// 그림 바이트의 형식을 머리 바이트로 알아낸다(이름이 틀려도 맞는 Content-Type을 쓰도록).
pub fn sniff_ext(bytes: &[u8]) -> Option<&'static str> {
    let b = bytes;
    if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpeg")
    } else if b.starts_with(b"GIF8") {
        Some("gif")
    } else if b.starts_with(b"BM") {
        Some("bmp")
    } else if b.starts_with(&[0x49, 0x49, 0x2A, 0]) || b.starts_with(&[0x4D, 0x4D, 0, 0x2A]) {
        Some("tiff")
    } else if b.len() > 44 && b[0..4] == [1, 0, 0, 0] && &b[40..44] == b" EMF" {
        Some("emf")
    } else if b.starts_with(&[0xD7, 0xCD, 0xC6, 0x9A]) || b.starts_with(&[1, 0, 9, 0]) || b.starts_with(&[2, 0, 9, 0]) {
        Some("wmf")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// zip 패키지를 메모리에 만든다(deflate).
pub(crate) struct Package {
    z: zip::ZipWriter<Cursor<Vec<u8>>>,
}

impl Package {
    pub fn new() -> Self {
        Self { z: zip::ZipWriter::new(Cursor::new(Vec::new())) }
    }
    pub fn put(&mut self, name: &str, data: &[u8]) -> Result<()> {
        // 이미 압축된 그림은 다시 압축하지 않는다(시간만 듦)
        let lower = name.to_ascii_lowercase();
        let stored = [".jpeg", ".jpg", ".png", ".gif", ".wdp", ".webp"].iter().any(|e| lower.ends_with(e));
        let o = zip::write::SimpleFileOptions::default().compression_method(if stored { zip::CompressionMethod::Stored } else { zip::CompressionMethod::Deflated });
        self.z.start_file(name, o).map_err(|e| Error::Invalid(format!("문서 파일을 만들지 못했습니다: {e}")))?;
        self.z.write_all(data).map_err(|e| Error::Invalid(format!("문서 파일을 만들지 못했습니다: {e}")))
    }
    pub fn finish(self) -> Result<Vec<u8>> {
        self.z.finish().map(Cursor::into_inner).map_err(|e| Error::Invalid(format!("문서 파일을 만들지 못했습니다: {e}")))
    }
}

/// 새 파일로 저장(있으면 오류, 덮어쓰지 않음)
pub fn save(path: &Path, bytes: &[u8]) -> Result<()> {
    crate::paths::write_new(path, bytes)
}

/// 문서 속성(core.xml) — 제목과 만든 프로그램만
pub(crate) fn core_xml(title: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>{}</dc:title><dc:creator>파일 도구</dc:creator></cp:coreProperties>"#,
        esc(title)
    )
}

pub(crate) fn app_xml() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>win-filetool</Application></Properties>"#
}

/// 색 → "RRGGBB"
pub fn hex(c: [u8; 3]) -> String {
    format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_and_types() {
        assert_eq!(esc("a<b>&\"c\u{1}"), "a&lt;b&gt;&amp;&quot;c");
        assert_eq!(media_type("x/image1.JPG"), ("jpeg", "image/jpeg"));
        assert_eq!(media_type("a.emf").1, "image/x-emf");
        assert_eq!(sniff_ext(&[0x89, b'P', b'N', b'G', 0, 0]), Some("png"));
        assert_eq!(sniff_ext(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpeg"));
        assert_eq!(hex([255, 0, 16]), "FF0010");
    }
}
