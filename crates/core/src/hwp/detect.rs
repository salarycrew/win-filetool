//! 한글 파일 형식 알아보기. 확장자가 아니라 **내용(앞부분 바이트·구조)**으로 판단한다.
//!
//! - 한글 97 이전(`HWP Document File V3.00` 등): LibreOffice가 읽을 수 있는 옛 형식
//! - 한글 5.x(2002 이후, `.hwp`): CFB(복합 파일) + `FileHeader` 스트림(버전·압축·암호·배포용 표시)
//! - HWPX(OWPML, `.hwpx`): zip + `mimetype` = `application/hwp+zip`
//! - 그 밖(내용이 짧거나 구조가 깨짐): 손상

use std::io::Read;
use std::path::Path;

use serde::Serialize;

use crate::error::{io_err, Result};

/// 한글 5 `FileHeader`의 서명(32바이트 중 앞부분)
pub const HWP5_SIGNATURE: &[u8] = b"HWP Document File";
/// 한글 97 이전 파일 앞부분(뒤에 ` V3.00`, ` V2.10` 등)
pub const HWP3_SIGNATURE: &[u8] = b"HWP Document File V";
pub const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
pub const HWPX_MIME: &str = "application/hwp+zip";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HwpFormat {
    /// 한글 97 이전(V3.00 등)
    Hwp97,
    /// 한글 5.x(2002 이후) `.hwp`
    Hwp5,
    /// HWPX(OWPML)
    Hwpx,
    /// 한글 파일 같지만 구조가 깨졌거나 한글 파일이 아님
    Damaged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HwpInfo {
    pub format: HwpFormat,
    /// `5.1.0.1`(한글 5), `3.00`(한글 97), HWPX는 `version.xml`의 값(있으면)
    pub version: Option<String>,
    /// 문서 암호·DRM·인증서 암호화(한글 5 FileHeader 비트 1·4·8·10, HWPX manifest의 encryption-data)
    pub encrypted: bool,
    /// 배포용 문서(한글 5 FileHeader 비트 2)
    pub distribution: bool,
    /// 본문이 압축되어 저장됨(한글 5 FileHeader 비트 0) — 그림 꺼낼 때 항목별 설정의 기본값
    #[serde(skip)]
    pub compressed: bool,
    /// 손상 이유(사람이 읽을 수 있는 짧은 문장, 점검용)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

impl HwpInfo {
    fn damaged(why: impl Into<String>) -> Self {
        HwpInfo { format: HwpFormat::Damaged, version: None, encrypted: false, distribution: false, compressed: false, problem: Some(why.into()) }
    }
}

/// 확장자가 한글 문서인지(`.hwp`, `.hwpx`)
pub fn is_hwp_path(p: &Path) -> bool {
    matches!(p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("hwp" | "hwpx"))
}

/// 한글 5 `FileHeader` 256바이트 해석
pub fn parse_file_header(h: &[u8]) -> Option<(String, u32)> {
    if h.len() < 40 || !h.starts_with(HWP5_SIGNATURE) {
        return None;
    }
    let v = u32::from_le_bytes([h[32], h[33], h[34], h[35]]);
    let props = u32::from_le_bytes([h[36], h[37], h[38], h[39]]);
    let ver = format!("{}.{}.{}.{}", v >> 24, (v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff);
    Some((ver, props))
}

fn detect_hwp5(path: &Path) -> HwpInfo {
    let mut c = match cfb::open(path) {
        Ok(c) => c,
        Err(e) => return HwpInfo::damaged(format!("복합 파일 구조를 읽지 못함: {e}")),
    };
    let mut head = Vec::new();
    match c.open_stream("/FileHeader") {
        Ok(s) => {
            if s.take(256).read_to_end(&mut head).is_err() {
                return HwpInfo::damaged("FileHeader를 읽지 못함");
            }
        }
        Err(_) => return HwpInfo::damaged("FileHeader가 없음(한글 문서가 아님)"),
    }
    let Some((version, props)) = parse_file_header(&head) else { return HwpInfo::damaged("FileHeader 서명이 다름") };
    HwpInfo {
        format: HwpFormat::Hwp5,
        version: Some(version),
        compressed: props & 1 != 0,
        // 1: 암호, 4: DRM, 8: 공인인증서 암호화, 10: 공인인증서 DRM
        encrypted: props & ((1 << 1) | (1 << 4) | (1 << 8) | (1 << 10)) != 0,
        distribution: props & (1 << 2) != 0,
        problem: None,
    }
}

fn zip_text(z: &mut zip::ZipArchive<std::fs::File>, name: &str, cap: u64) -> Option<String> {
    let f = z.by_name(name).ok()?;
    let mut s = String::new();
    f.take(cap).read_to_string(&mut s).ok()?;
    Some(s)
}

fn detect_hwpx(path: &Path) -> HwpInfo {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return HwpInfo::damaged(format!("열지 못함: {e}")),
    };
    let mut z = match zip::ZipArchive::new(f) {
        Ok(z) => z,
        Err(e) => return HwpInfo::damaged(format!("zip 구조를 읽지 못함: {e}")),
    };
    let mime = zip_text(&mut z, "mimetype", 256).map(|s| s.trim().to_string());
    let has_contents = z.index_for_name("Contents/content.hpf").is_some() || z.index_for_name("Contents/header.xml").is_some();
    if mime.as_deref() != Some(HWPX_MIME) && !has_contents {
        return HwpInfo::damaged("HWPX(mimetype application/hwp+zip)가 아님");
    }
    // 암호를 건 HWPX는 META-INF/manifest.xml에 encryption-data가 있다
    let encrypted = zip_text(&mut z, "META-INF/manifest.xml", 4 << 20).is_some_and(|m| m.contains("encryption-data"));
    let version = zip_text(&mut z, "version.xml", 64 << 10).and_then(|x| {
        let attr = |k: &str| -> Option<String> {
            let i = x.find(&format!(" {k}=\""))? + k.len() + 3;
            Some(x[i..].split('"').next()?.to_string())
        };
        Some(format!("{}.{}.{}.{}", attr("major")?, attr("minor")?, attr("micro")?, attr("buildNumber")?))
    });
    HwpInfo { format: HwpFormat::Hwpx, version, encrypted, distribution: false, compressed: true, problem: None }
}

/// 파일 내용으로 한글 형식을 알아본다. 파일을 읽을 수 없으면(없음·권한) 오류, 구조가 이상하면 `Damaged`.
pub fn detect(path: &Path) -> Result<HwpInfo> {
    let mut f = std::fs::File::open(path).map_err(io_err(path))?;
    let mut head = [0u8; 64];
    let mut n = 0;
    while n < head.len() {
        let k = f.read(&mut head[n..]).map_err(io_err(path))?;
        if k == 0 {
            break;
        }
        n += k;
    }
    let head = &head[..n];
    drop(f);
    if head.starts_with(&CFB_MAGIC) {
        return Ok(detect_hwp5(path));
    }
    if head.starts_with(b"PK\x03\x04") {
        return Ok(detect_hwpx(path));
    }
    if head.starts_with(HWP3_SIGNATURE) {
        let ver: String = head[HWP3_SIGNATURE.len()..].iter().take_while(|b| b.is_ascii_digit() || **b == b'.').map(|b| *b as char).collect();
        return Ok(HwpInfo { format: HwpFormat::Hwp97, version: (!ver.is_empty()).then_some(ver), encrypted: false, distribution: false, compressed: false, problem: None });
    }
    Ok(HwpInfo::damaged(if n == 0 { "빈 파일" } else { "한글 문서 서명이 없음" }))
}

#[cfg(test)]
pub(crate) mod testfiles {
    //! 시험용 작은 한글 파일 만들기(한글 5 CFB, HWPX zip, 한글 97 머리)
    use std::io::Write;
    use std::path::Path;

    use flate2::write::DeflateEncoder;
    use flate2::Compression;

    pub fn deflate(b: &[u8]) -> Vec<u8> {
        let mut e = DeflateEncoder::new(Vec::new(), Compression::default());
        e.write_all(b).unwrap();
        e.finish().unwrap()
    }

    pub fn file_header(version: u32, props: u32) -> Vec<u8> {
        let mut h = vec![0u8; 256];
        h[..super::HWP5_SIGNATURE.len()].copy_from_slice(super::HWP5_SIGNATURE);
        h[32..36].copy_from_slice(&version.to_le_bytes());
        h[36..40].copy_from_slice(&props.to_le_bytes());
        h
    }

    /// 레코드 머리(태그 10비트·수준 10비트·크기 12비트, 크기 4095 이상이면 뒤에 u32)
    pub fn record(tag: u16, level: u16, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let size = data.len() as u32;
        let h = (tag as u32 & 0x3ff) | ((level as u32 & 0x3ff) << 10) | (size.min(0xfff) << 20);
        out.extend_from_slice(&h.to_le_bytes());
        if size >= 0xfff {
            out.extend_from_slice(&size.to_le_bytes());
        }
        out.extend_from_slice(data);
        out
    }

    /// BIN_DATA 레코드: kind 0 링크, 1 묻음, 2 OLE 저장소 / comp 0 문서 따름, 1 압축, 2 압축 안 함
    pub fn bin_data_record(kind: u16, comp: u16, id: u16, ext: &str) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&(kind | (comp << 4)).to_le_bytes());
        if kind == 0 {
            for s in ["C:\\a.png", "a.png"] {
                let u: Vec<u16> = s.encode_utf16().collect();
                d.extend_from_slice(&(u.len() as u16).to_le_bytes());
                for c in u {
                    d.extend_from_slice(&c.to_le_bytes());
                }
            }
        } else {
            d.extend_from_slice(&id.to_le_bytes());
            if kind == 1 {
                let u: Vec<u16> = ext.encode_utf16().collect();
                d.extend_from_slice(&(u.len() as u16).to_le_bytes());
                for c in u {
                    d.extend_from_slice(&c.to_le_bytes());
                }
            }
        }
        record(super::super::images::TAG_BIN_DATA, 0, &d)
    }

    /// 한글 5 파일: FileHeader + DocInfo(문서 압축이면 DEFLATE) + BinData/BINxxxx.ext(바이트 그대로 넣음)
    pub fn write_hwp5(path: &Path, props: u32, doc_info: &[u8], bins: &[(&str, Vec<u8>)]) {
        let mut c = cfb::create(path).unwrap();
        c.create_stream("/FileHeader").unwrap().write_all(&file_header(0x0501_0001, props)).unwrap();
        let di = if props & 1 != 0 { deflate(doc_info) } else { doc_info.to_vec() };
        c.create_stream("/DocInfo").unwrap().write_all(&di).unwrap();
        c.create_storage("/BodyText").unwrap();
        c.create_stream("/BodyText/Section0").unwrap().write_all(&deflate(b"")).unwrap();
        if !bins.is_empty() {
            c.create_storage("/BinData").unwrap();
        }
        for (name, b) in bins {
            c.create_stream(format!("/BinData/{name}")).unwrap().write_all(b).unwrap();
        }
        c.flush().unwrap();
    }

    /// HWPX: mimetype + content.hpf(manifest) + BinData/…
    pub fn write_hwpx(path: &Path, items: &[(&str, &str, &[u8])], extra: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let st = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let df = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file("mimetype", st).unwrap();
        z.write_all(super::HWPX_MIME.as_bytes()).unwrap();
        z.start_file("version.xml", df).unwrap();
        z.write_all(br#"<?xml version="1.0"?><hv:HCFVersion tagetApplication="WORDPROCESSOR" major="5" minor="1" micro="1" buildNumber="0"/>"#).unwrap();
        let mut hpf = String::from(r#"<?xml version="1.0"?><opf:package><opf:manifest><opf:item id="header" href="Contents/header.xml" media-type="application/xml"/>"#);
        for (i, (href, mt, _)) in items.iter().enumerate() {
            hpf.push_str(&format!(r#"<opf:item id="image{}" href="{href}" media-type="{mt}" isEmbeded="1"/>"#, i + 1));
        }
        hpf.push_str("</opf:manifest></opf:package>");
        z.start_file("Contents/content.hpf", df).unwrap();
        z.write_all(hpf.as_bytes()).unwrap();
        z.start_file("Contents/header.xml", df).unwrap();
        z.write_all(b"<hh:head/>").unwrap();
        for (href, _, b) in items {
            z.start_file(*href, df).unwrap();
            z.write_all(b).unwrap();
        }
        for (name, b) in extra {
            z.start_file(*name, df).unwrap();
            z.write_all(b).unwrap();
        }
        z.finish().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::testfiles::*;
    use super::*;

    #[test]
    fn hwp5_flags() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.hwp");
        write_hwp5(&p, 1, b"", &[]);
        let i = detect(&p).unwrap();
        assert_eq!(i.format, HwpFormat::Hwp5);
        assert_eq!(i.version.as_deref(), Some("5.1.0.1"));
        assert!(i.compressed && !i.encrypted && !i.distribution);
        let p = t.path().join("암호.hwp");
        write_hwp5(&p, 1 | 2, b"", &[]);
        assert!(detect(&p).unwrap().encrypted);
        let p = t.path().join("drm.hwp");
        write_hwp5(&p, 1 << 4, b"", &[]);
        assert!(detect(&p).unwrap().encrypted);
        let p = t.path().join("배포용.hwp");
        write_hwp5(&p, 1 | 4, b"", &[]);
        let i = detect(&p).unwrap();
        assert!(i.distribution && !i.encrypted);
    }

    #[test]
    fn hwpx_and_encrypted_hwpx() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.hwpx");
        write_hwpx(&p, &[], &[]);
        let i = detect(&p).unwrap();
        assert_eq!(i.format, HwpFormat::Hwpx);
        assert_eq!(i.version.as_deref(), Some("5.1.1.0"));
        assert!(!i.encrypted);
        let p = t.path().join("b.hwpx");
        write_hwpx(&p, &[], &[("META-INF/manifest.xml", br#"<odf:manifest><odf:file-entry><odf:encryption-data/></odf:file-entry></odf:manifest>"#)]);
        assert!(detect(&p).unwrap().encrypted);
    }

    #[test]
    fn hwp97_and_damaged() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("old.hwp");
        std::fs::write(&p, b"HWP Document File V3.00 \x1a\x01\x02\x03\x04\x05rest").unwrap();
        let i = detect(&p).unwrap();
        assert_eq!(i.format, HwpFormat::Hwp97);
        assert_eq!(i.version.as_deref(), Some("3.00"));
        for (name, bytes) in [
            ("empty.hwp", &b""[..]),
            ("junk.hwp", &b"hello world, not a hwp"[..]),
            ("cut.hwp", &CFB_MAGIC[..]),
            ("zip.hwpx", &b"PK\x03\x04broken"[..]),
        ] {
            let p = t.path().join(name);
            std::fs::write(&p, bytes).unwrap();
            let i = detect(&p).unwrap();
            assert_eq!(i.format, HwpFormat::Damaged, "{name}");
            assert!(i.problem.is_some());
        }
        // CFB지만 FileHeader 없음(예: 이름만 바꾼 .doc)
        let p = t.path().join("doc.hwp");
        let mut c = cfb::create(&p).unwrap();
        std::io::Write::write_all(&mut c.create_stream("/WordDocument").unwrap(), b"x").unwrap();
        c.flush().unwrap();
        drop(c);
        assert_eq!(detect(&p).unwrap().format, HwpFormat::Damaged);
        // 일반 zip(docx 등)은 HWPX가 아님
        let p = t.path().join("plain.hwpx");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
        z.start_file("word/document.xml", zip::write::SimpleFileOptions::default()).unwrap();
        z.finish().unwrap();
        assert_eq!(detect(&p).unwrap().format, HwpFormat::Damaged);
        assert!(detect(&t.path().join("없음.hwp")).is_err());
        assert!(is_hwp_path(Path::new("a.HWPX")) && !is_hwp_path(Path::new("a.docx")));
    }
}
