//! 데스크톱 점검·데모용 샘플 파일 생성기 (NFD 폴더, 맥식 zip, N쪽 PDF).
//! `filetool-cli selftest`가 실제 윈도 디스크 위에서 모든 작업을 돌려 볼 때 쓴다.

use std::path::Path;

use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream};
use unicode_normalization::UnicodeNormalization;

use crate::error::{io_err, Error, Result};

pub fn nfd(s: &str) -> String {
    s.nfd().collect()
}

/// `label p1` … `label pN`이 찍힌 A4 PDF(2단 페이지 트리, 상속 MediaBox).
pub fn make_pdf(path: &Path, n: u32, label: &str) -> Result<()> {
    let mut doc = Document::with_version("1.5");
    let root_pages = doc.new_object_id();
    let mid_pages = doc.new_object_id();
    let font = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let resources = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
    let mut kids = Vec::new();
    for i in 1..=n {
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 36.into()]),
                Operation::new("Td", vec![100.into(), 600.into()]),
                Operation::new("Tj", vec![Object::string_literal(format!("{label} p{i}"))]),
                Operation::new("ET", vec![]),
            ],
        };
        let encoded = content
            .encode()
            .map_err(|e| Error::Pdf { path: path.to_path_buf(), message: e.to_string() })?;
        let cid = doc.add_object(Stream::new(dictionary! {}, encoded));
        let pid = doc.add_object(dictionary! { "Type" => "Page", "Parent" => mid_pages, "Contents" => cid });
        kids.push(Object::Reference(pid));
    }
    doc.objects.insert(
        mid_pages,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Parent" => root_pages, "Kids" => kids, "Count" => n as i64,
            "Resources" => resources,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        }),
    );
    doc.objects.insert(
        root_pages,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![Object::Reference(mid_pages)], "Count" => n as i64,
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => root_pages });
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(io_err(path))?;
    crate::paths::write_new(path, &bytes)
}

/// 각 쪽 첫 텍스트 문자열(`label pN`)을 순서대로 읽는다(점검용).
pub fn pdf_labels(path: &Path) -> Result<Vec<String>> {
    let doc = crate::pdf::load(path)?;
    Ok(doc
        .get_pages()
        .values()
        .map(|&pid| {
            let s = String::from_utf8_lossy(&doc.get_page_content(pid)).to_string();
            s.find('(')
                .and_then(|a| s[a + 1..].find(')').map(|b| s[a + 1..a + 1 + b].to_string()))
                .unwrap_or_default()
        })
        .collect())
}

/// 각 쪽의 `/Rotate` 값(점검용).
pub fn pdf_rotations(path: &Path) -> Result<Vec<i64>> {
    let doc = crate::pdf::load(path)?;
    Ok(doc
        .get_pages()
        .values()
        .map(|&pid| {
            doc.get_dictionary(pid)
                .ok()
                .and_then(|d| d.get(b"Rotate").ok())
                .and_then(|o| o.as_i64().ok())
                .unwrap_or(0)
        })
        .collect())
}

/// 맥 Archive Utility처럼 UTF-8 플래그 없이 이름 바이트를 그대로 넣은 무압축 zip.
pub fn write_mac_style_zip(path: &Path, entries: &[(&str, &[u8])]) -> Result<()> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let u16le = |v: u16| v.to_le_bytes();
    let u32le = |v: u32| v.to_le_bytes();
    for (name, data) in entries {
        let nb = name.as_bytes();
        let crc = crc32(data);
        let offset = out.len() as u32;
        out.extend_from_slice(&u32le(0x04034b50));
        for v in [20u16, 0, 0, 0, 0x21] {
            out.extend_from_slice(&u16le(v));
        }
        for v in [crc, data.len() as u32, data.len() as u32] {
            out.extend_from_slice(&u32le(v));
        }
        out.extend_from_slice(&u16le(nb.len() as u16));
        out.extend_from_slice(&u16le(0));
        out.extend_from_slice(nb);
        out.extend_from_slice(data);

        central.extend_from_slice(&u32le(0x02014b50));
        for v in [(3u16 << 8) | 20, 20, 0, 0, 0, 0x21] {
            central.extend_from_slice(&u16le(v));
        }
        for v in [crc, data.len() as u32, data.len() as u32] {
            central.extend_from_slice(&u32le(v));
        }
        for v in [nb.len() as u16, 0, 0, 0, 0] {
            central.extend_from_slice(&u16le(v));
        }
        let attr: u32 = if name.ends_with('/') { (0o040755 << 16) | 0x10 } else { 0o100644 << 16 };
        central.extend_from_slice(&u32le(attr));
        central.extend_from_slice(&u32le(offset));
        central.extend_from_slice(nb);
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&u32le(0x06054b50));
    for v in [0u16, 0, entries.len() as u16, entries.len() as u16] {
        out.extend_from_slice(&u16le(v));
    }
    out.extend_from_slice(&u32le(central.len() as u32));
    out.extend_from_slice(&u32le(cd_offset));
    out.extend_from_slice(&u16le(0));
    crate::paths::write_new(path, &out)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    #[test]
    fn crc32_known_value() {
        assert_eq!(super::crc32(b"123456789"), 0xCBF4_3926);
    }
}
