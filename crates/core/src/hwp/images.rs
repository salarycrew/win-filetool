//! 한글 문서 안 그림을 **원본 바이트 그대로** 꺼낸다(이미지만 뽑기, [`crate::officeimages::extract_images`]가 부른다).
//!
//! - 한글 5(`.hwp`): `DocInfo`의 BIN_DATA 레코드 순서대로 `BinData/BINxxxx.ext`를 읽는다. 항목마다 압축 설정
//!   (문서 따름/압축/압축 안 함)이 있어 그대로 따른다. 압축은 raw DEFLATE.
//! - HWPX: `Contents/content.hpf`(manifest) 순서대로 `BinData/…`를 읽고, manifest에 없는 `BinData/` 항목은 뒤에 붙인다.
//! - 그림인지는 파일 앞부분(서명)으로 판단한다: PNG·JPEG·GIF·BMP·TIFF·WebP·WMF·EMF. WMF/EMF는 원본 파일 그대로 둔다.
//!   OLE 개체·외부 연결·그림이 아닌 첨부는 건너뛰고 이유를 남긴다.
//! - 안전: 항목·전체 크기 한도(압축 해제 후 기준), 항목 수 한도, 이름은 꺼낸 순번 + 정리한 원래 이름만 써서
//!   다른 폴더로 빠져나가지 않는다. 암호·배포용·손상·한글 97 문서는 알기 쉬운 오류로 거절한다.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::detect::{self, HwpFormat, HwpInfo};
use crate::error::{io_err, Error, Result};
use crate::officeimages::ImagesOut;
use crate::ooxml::pptx::fnv;
use crate::paths;

/// HWPTAG_BEGIN(0x10) + 2
pub const TAG_BIN_DATA: u16 = 0x12;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// 그림 하나(압축 푼 뒤)
    pub max_item: u64,
    /// 모든 그림 합계(압축 푼 뒤)
    pub max_total: u64,
    /// 항목 수
    pub max_items: usize,
    /// DocInfo(압축 푼 뒤)
    pub max_docinfo: u64,
}

pub const LIMITS: Limits = Limits { max_item: 256 << 20, max_total: 2 << 30, max_items: 20_000, max_docinfo: 64 << 20 };

pub const MSG_ENCRYPTED: &str = "암호가 걸렸거나 보호된 한글 문서라 그림을 꺼낼 수 없어요. 한글에서 암호를 풀어 저장한 뒤 다시 해 주세요.";
pub const MSG_DISTRIBUTION: &str = "배포용 한글 문서라 그림을 꺼낼 수 없어요.";
pub const MSG_DAMAGED: &str = "한글 파일이 손상되어 읽을 수 없어요.";
pub const MSG_HWP97: &str = "한글 97 문서에서는 그림 뽑기를 할 수 없어요. PDF로 바꾼 뒤 PDF에서 그림을 뽑아 주세요.";

/// 파일 앞부분으로 그림 형식(확장자) 알아보기
pub fn sniff(b: &[u8]) -> Option<&'static str> {
    let u16le = |i: usize| b.get(i..i + 2).map(|x| u16::from_le_bytes([x[0], x[1]]));
    let u32le = |i: usize| b.get(i..i + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]));
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("gif")
    } else if b.starts_with(b"BM") && b.len() > 26 && u32le(14).is_some_and(|h| matches!(h, 12 | 40 | 52 | 56 | 64 | 108 | 124)) {
        Some("bmp")
    } else if b.starts_with(b"II*\0") || b.starts_with(b"MM\0*") {
        Some("tif")
    } else if b.len() > 12 && b.starts_with(b"RIFF") && &b[8..12] == b"WEBP" {
        Some("webp")
    } else if b.starts_with(&[0xD7, 0xCD, 0xC6, 0x9A]) {
        // 위치 정보가 붙은(placeable) WMF
        Some("wmf")
    } else if matches!(u16le(0), Some(1 | 2)) && u16le(2) == Some(9) && matches!(u16le(4), Some(0x100 | 0x300)) {
        Some("wmf")
    } else if u32le(0) == Some(1) && b.get(40..44) == Some(b" EMF") {
        Some("emf")
    } else {
        None
    }
}

/// 문서 안 이름을 파일 이름으로 쓸 수 있게(경로 구분자·예약 문자 제거, 길이 제한)
pub fn safe_name(s: &str) -> String {
    let last = s.rsplit(['/', '\\']).next().unwrap_or("");
    let mut out: String = last.chars().filter(|c| !c.is_control() && !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')).take(80).collect();
    while out.starts_with('.') || out.ends_with('.') || out.ends_with(' ') || out.starts_with(' ') {
        if out.starts_with('.') || out.starts_with(' ') {
            out.remove(0);
        } else {
            out.pop();
        }
    }
    if out.is_empty() {
        "image".into()
    } else {
        out
    }
}

/// 꺼낼 항목 하나
#[derive(Debug)]
pub(crate) enum Item {
    Image { name: String, bytes: Vec<u8> },
    Skip { name: String, reason: String },
}

fn inflate(data: &[u8], cap: u64) -> std::result::Result<Vec<u8>, &'static str> {
    let mut out = Vec::new();
    match flate2::read::DeflateDecoder::new(data).take(cap + 1).read_to_end(&mut out) {
        Ok(_) if out.len() as u64 > cap => Err("너무 커서 건너뜀(압축을 풀면 한도를 넘음)"),
        Ok(_) => Ok(out),
        Err(_) => Err("압축을 풀지 못함(손상)"),
    }
}

/// 레코드 목록(태그, 내용)
pub(crate) fn records(b: &[u8]) -> Vec<(u16, &[u8])> {
    let mut v = Vec::new();
    let mut i = 0usize;
    while i + 4 <= b.len() {
        let h = u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        i += 4;
        let tag = (h & 0x3ff) as u16;
        let mut size = (h >> 20) as usize;
        if size == 0xfff {
            if i + 4 > b.len() {
                break;
            }
            size = u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
            i += 4;
        }
        let Some(end) = i.checked_add(size).filter(|e| *e <= b.len()) else { break };
        v.push((tag, &b[i..end]));
        i = end;
    }
    v
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BinRef {
    Link,
    Embedded { id: u16, ext: String, compress: Option<bool> },
    Storage { id: u16 },
}

pub(crate) fn parse_bin_data(d: &[u8]) -> Option<BinRef> {
    let prop = u16::from_le_bytes([*d.first()?, *d.get(1)?]);
    let kind = prop & 0xf;
    let compress = match (prop >> 4) & 3 {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    };
    match kind {
        0 => Some(BinRef::Link),
        1 => {
            let id = u16::from_le_bytes([*d.get(2)?, *d.get(3)?]);
            let n = u16::from_le_bytes([*d.get(4)?, *d.get(5)?]) as usize;
            let raw = d.get(6..6 + n * 2)?;
            let ext = String::from_utf16_lossy(&raw.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>());
            Some(BinRef::Embedded { id, ext, compress })
        }
        2 => Some(BinRef::Storage { id: u16::from_le_bytes([*d.get(2)?, *d.get(3)?]) }),
        _ => None,
    }
}

fn read_stream_capped<F: Read + std::io::Seek>(c: &mut cfb::CompoundFile<F>, path: &str, cap: u64) -> std::result::Result<Vec<u8>, &'static str> {
    let s = c.open_stream(path).map_err(|_| "문서 안에 데이터가 없음")?;
    if s.len() > cap {
        return Err("너무 커서 건너뜀");
    }
    let mut b = Vec::new();
    s.take(cap).read_to_end(&mut b).map_err(|_| "읽지 못함(손상)")?;
    Ok(b)
}

fn hwp5_items(path: &Path, info: &HwpInfo, lim: &Limits) -> Result<Vec<Item>> {
    let mut c = cfb::open(path).map_err(|_| Error::Invalid(MSG_DAMAGED.into()))?;
    let raw = read_stream_capped(&mut c, "/DocInfo", lim.max_docinfo).map_err(|_| Error::Invalid(MSG_DAMAGED.into()))?;
    let doc_info = if info.compressed { inflate(&raw, lim.max_docinfo).map_err(|_| Error::Invalid(MSG_DAMAGED.into()))? } else { raw };
    // BinData 저장소 안 실제 이름(대소문자 무시로 찾기)
    let streams: Vec<String> = c.read_storage("/BinData").map(|it| it.filter(|e| e.is_stream()).map(|e| e.name().to_string()).collect()).unwrap_or_default();
    let find = |want: &str| streams.iter().find(|s| s.eq_ignore_ascii_case(want)).cloned();
    let mut refs: Vec<BinRef> = records(&doc_info).into_iter().filter(|(t, _)| *t == TAG_BIN_DATA).filter_map(|(_, d)| parse_bin_data(d)).collect();
    if refs.is_empty() && !streams.is_empty() {
        // DocInfo에 목록이 없는데 BinData가 있으면(드묾) 저장된 이름 순서대로, 압축은 문서 설정을 따름
        let mut s = streams.clone();
        s.sort();
        refs = s
            .iter()
            .filter_map(|n| {
                let (stem, ext) = n.split_once('.')?;
                let id = u16::from_str_radix(stem.strip_prefix("BIN").or_else(|| stem.strip_prefix("bin"))?, 16).ok()?;
                Some(BinRef::Embedded { id, ext: ext.to_string(), compress: None })
            })
            .collect();
    }
    let mut items = Vec::new();
    let mut total: u64 = 0;
    for (k, r) in refs.into_iter().enumerate() {
        if k >= lim.max_items {
            items.push(Item::Skip { name: "…".into(), reason: format!("항목이 너무 많아 {}개 뒤는 건너뜀", lim.max_items) });
            break;
        }
        match r {
            BinRef::Link => items.push(Item::Skip { name: format!("연결 {}", k + 1), reason: "문서 밖 파일에 연결된 그림(문서 안에 없음)".into() }),
            BinRef::Storage { id } => items.push(Item::Skip { name: format!("BIN{id:04X}.OLE"), reason: "OLE 개체(그림이 아님)".into() }),
            BinRef::Embedded { id, ext, compress } => {
                let label = format!("BIN{id:04X}.{}", safe_name(&ext));
                let Some(stream) = find(&format!("BIN{id:04X}.{ext}")).or_else(|| streams.iter().find(|s| s.to_ascii_uppercase().starts_with(&format!("BIN{id:04X}."))).cloned()) else {
                    items.push(Item::Skip { name: label, reason: "문서 안에 데이터가 없음".into() });
                    continue;
                };
                let left = lim.max_total.saturating_sub(total);
                if left == 0 {
                    items.push(Item::Skip { name: label, reason: "전체 크기 한도를 넘어 건너뜀".into() });
                    continue;
                }
                let cap = lim.max_item.min(left);
                let data = match read_stream_capped(&mut c, &format!("/BinData/{stream}"), cap) {
                    Ok(d) => d,
                    Err(why) => {
                        items.push(Item::Skip { name: label, reason: why.into() });
                        continue;
                    }
                };
                let bytes = if compress.unwrap_or(info.compressed) {
                    match inflate(&data, cap) {
                        Ok(b) => b,
                        // 압축 표시가 있어도 실제로는 그대로 저장된 그림이 드물게 있다
                        Err(_) if sniff(&data).is_some() => data,
                        Err(why) => {
                            items.push(Item::Skip { name: label, reason: why.into() });
                            continue;
                        }
                    }
                } else {
                    data
                };
                match sniff(&bytes) {
                    Some(e) => {
                        total += bytes.len() as u64;
                        items.push(Item::Image { name: format!("BIN{id:04X}.{e}"), bytes });
                    }
                    None => items.push(Item::Skip { name: label, reason: format!("그림 형식이 아님(.{})", safe_name(&ext)) }),
                }
            }
        }
    }
    Ok(items)
}

/// content.hpf의 BinData 항목(href) 순서
fn manifest_hrefs(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut r = Reader::from_str(xml);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if e.local_name().as_ref() == "item" => {
                if let Some(h) = e.attributes().flatten().find(|a| a.key.local_name().as_ref() == "href") {
                    let h = h.value.to_string();
                    if h.starts_with("BinData/") && !out.contains(&h) {
                        out.push(h);
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

fn hwpx_items(path: &Path, lim: &Limits) -> Result<Vec<Item>> {
    let f = std::fs::File::open(path).map_err(io_err(path))?;
    let mut z = zip::ZipArchive::new(f).map_err(|_| Error::Invalid(MSG_DAMAGED.into()))?;
    let names: Vec<String> = z.file_names().filter_map(|n| n.ok().map(|n| n.into_owned())).collect();
    let hpf = z.by_name("Contents/content.hpf").ok().and_then(|f| {
        let mut s = String::new();
        f.take(16 << 20).read_to_string(&mut s).ok().map(|_| s)
    });
    let mut order = hpf.as_deref().map(manifest_hrefs).unwrap_or_default();
    let mut rest: Vec<String> = names.iter().filter(|n| n.starts_with("BinData/") && !n.ends_with('/') && !order.contains(n)).cloned().collect();
    rest.sort();
    order.extend(rest);
    let mut items = Vec::new();
    let mut total: u64 = 0;
    for (k, href) in order.into_iter().enumerate() {
        if k >= lim.max_items {
            items.push(Item::Skip { name: "…".into(), reason: format!("항목이 너무 많아 {}개 뒤는 건너뜀", lim.max_items) });
            break;
        }
        let label = safe_name(&href);
        if href.contains("..") || href.contains('\\') || href.contains(':') || href.split('/').any(str::is_empty) {
            items.push(Item::Skip { name: label, reason: "이름이 올바르지 않은 항목".into() });
            continue;
        }
        let Ok(e) = z.by_name(&href) else {
            items.push(Item::Skip { name: label, reason: "문서 안에 데이터가 없음".into() });
            continue;
        };
        let left = lim.max_total.saturating_sub(total);
        let cap = lim.max_item.min(left);
        if e.size() > cap || left == 0 {
            items.push(Item::Skip { name: label, reason: "너무 커서 건너뜀".into() });
            continue;
        }
        let mut b = Vec::new();
        if e.take(cap + 1).read_to_end(&mut b).is_err() {
            items.push(Item::Skip { name: label, reason: "읽지 못함(손상)".into() });
            continue;
        }
        if b.len() as u64 > cap {
            items.push(Item::Skip { name: label, reason: "너무 커서 건너뜀".into() });
            continue;
        }
        match sniff(&b) {
            Some(_) => {
                total += b.len() as u64;
                items.push(Item::Image { name: label, bytes: b });
            }
            None => items.push(Item::Skip { name: label, reason: "그림 형식이 아님".into() }),
        }
    }
    Ok(items)
}

/// 형식·보호 상태 확인 후 꺼낼 항목 목록
pub(crate) fn items(path: &Path, lim: &Limits) -> Result<(HwpInfo, Vec<Item>)> {
    let info = detect::detect(path)?;
    if info.encrypted {
        return Err(Error::Invalid(MSG_ENCRYPTED.into()));
    }
    if info.distribution {
        return Err(Error::Invalid(MSG_DISTRIBUTION.into()));
    }
    let items = match info.format {
        HwpFormat::Hwp5 => hwp5_items(path, &info, lim)?,
        HwpFormat::Hwpx => hwpx_items(path, lim)?,
        HwpFormat::Hwp97 => return Err(Error::Invalid(MSG_HWP97.into())),
        HwpFormat::Damaged => return Err(Error::Invalid(MSG_DAMAGED.into())),
    };
    Ok((info, items))
}

pub fn extract(input: &Path, out_dir: Option<&Path>) -> Result<ImagesOut> {
    extract_with(input, out_dir, &LIMITS)
}

/// 꺼내서 `이름_이미지\NN_원래이름`으로 쓴다. 같은 바이트의 그림은 한 번만.
pub fn extract_with(input: &Path, out_dir: Option<&Path>, lim: &Limits) -> Result<ImagesOut> {
    let t0 = Instant::now();
    let (_info, items) = items(input, lim)?;
    let n_images = items.iter().filter(|i| matches!(i, Item::Image { .. })).count();
    let width = n_images.to_string().len().max(2);
    let mut seen: HashMap<(u64, usize), Vec<usize>> = HashMap::new();
    let mut kept: Vec<Vec<u8>> = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();
    let mut folder: Option<PathBuf> = None;
    let mut dups = 0;
    let mut skipped = Vec::new();
    for it in items {
        match it {
            Item::Skip { name, reason } => skipped.push(format!("{name}: {reason}")),
            Item::Image { name, bytes } => {
                let key = (fnv(&bytes), bytes.len());
                let e = seen.entry(key).or_default();
                if e.iter().any(|i| kept[*i] == bytes) {
                    dups += 1;
                    continue;
                }
                let dir = match &folder {
                    Some(d) => d.clone(),
                    None => {
                        let d = crate::pdfrender::output_folder(input, out_dir, "_이미지");
                        std::fs::create_dir_all(&d).map_err(io_err(&d))?;
                        folder = Some(d.clone());
                        d
                    }
                };
                let path = paths::unique_path(&dir.join(format!("{:0width$}_{name}", files.len() + 1)));
                paths::write_new(&path, &bytes)?;
                files.push(path);
                e.push(kept.len());
                kept.push(bytes);
            }
        }
    }
    let mut note = if files.is_empty() {
        "문서에 꺼낼 그림이 없습니다".to_string()
    } else {
        format!("그림 {}개를 원본 그대로 꺼냈습니다{}", files.len(), if dups > 0 { format!(" (같은 그림 {dups}개는 한 번만)") } else { String::new() })
    };
    if !skipped.is_empty() {
        note.push_str(&format!(" · 그림이 아니거나 읽을 수 없는 항목 {}개는 건너뜀", skipped.len()));
    }
    Ok(ImagesOut { input: input.into(), folder, files, duplicates: dups, skipped: skipped.len(), millis: t0.elapsed().as_millis() as u64, note, skipped_items: skipped })
}

#[cfg(test)]
mod tests {
    use super::super::detect::testfiles::*;
    use super::*;

    fn png(seed: u8) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.extend_from_slice(&[seed; 40]);
        v
    }
    fn jpg(seed: u8) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
        v.extend_from_slice(&[seed; 100]);
        v
    }
    fn wmf() -> Vec<u8> {
        let mut v = vec![0xD7, 0xCD, 0xC6, 0x9A];
        v.extend_from_slice(&[7; 60]);
        v
    }

    #[test]
    fn sniff_and_names() {
        assert_eq!(sniff(&png(1)), Some("png"));
        assert_eq!(sniff(&jpg(1)), Some("jpg"));
        assert_eq!(sniff(&wmf()), Some("wmf"));
        assert_eq!(sniff(&[1, 0, 9, 0, 0, 3, 0, 0]), Some("wmf"));
        let mut emf = vec![0u8; 60];
        emf[0] = 1;
        emf[40..44].copy_from_slice(b" EMF");
        assert_eq!(sniff(&emf), Some("emf"));
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0; 12]);
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&[0; 20]);
        assert_eq!(sniff(&bmp), Some("bmp"));
        assert_eq!(sniff(b"BM hello there this is text, not a bitmap"), None);
        assert_eq!(sniff(b"PK\x03\x04"), None);
        assert_eq!(safe_name("BinData/../../evil.png"), "evil.png");
        assert_eq!(safe_name(r"..\..\a:b?.jpg"), "ab.jpg");
        assert_eq!(safe_name("..."), "image");
        assert_eq!(safe_name("C:\\Windows\\x.exe"), "x.exe");
    }

    #[test]
    fn records_and_bin_data() {
        let mut big = vec![9u8; 5000];
        big[0] = 1;
        let mut b = record(0x10, 0, b"abc");
        b.extend(record(TAG_BIN_DATA, 1, &big));
        b.extend(bin_data_record(1, 2, 0x1a, "jpg"));
        let r = records(&b);
        assert_eq!(r.len(), 3);
        assert_eq!(r[1].1.len(), 5000);
        assert_eq!(parse_bin_data(r[2].1), Some(BinRef::Embedded { id: 0x1a, ext: "jpg".into(), compress: Some(false) }));
        assert_eq!(parse_bin_data(&bin_data_record(2, 0, 3, "")[4..]), Some(BinRef::Storage { id: 3 }));
        assert_eq!(parse_bin_data(&bin_data_record(0, 0, 0, "")[4..]), Some(BinRef::Link));
        // 잘린 레코드는 거기서 멈춤(패닉 없음)
        assert_eq!(records(&b[..b.len() - 3]).len(), 2);
        assert!(parse_bin_data(&[1, 0, 5]).is_none());
    }

    /// 압축 문서 + 항목별 압축 설정(따름/압축 안 함), OLE·연결·그림 아님·중복·없는 데이터
    fn sample_hwp5(dir: &Path) -> PathBuf {
        let p = dir.join("보고서.hwp");
        let mut di = record(0x10, 0, &[0; 26]); // DOCUMENT_PROPERTIES
        di.extend(bin_data_record(1, 0, 1, "jpg")); // 문서 따름(압축)
        di.extend(bin_data_record(1, 2, 2, "png")); // 압축 안 함
        di.extend(bin_data_record(2, 0, 3, "")); // OLE
        di.extend(bin_data_record(1, 1, 4, "bin")); // 그림 아님(압축)
        di.extend(bin_data_record(1, 0, 5, "jpg")); // 1과 같은 그림
        di.extend(bin_data_record(0, 0, 0, "")); // 연결
        di.extend(bin_data_record(1, 0, 6, "wmf")); // WMF 그대로
        di.extend(bin_data_record(1, 0, 7, "png")); // 데이터 없음
        write_hwp5(
            &p,
            1,
            &di,
            &[
                ("BIN0001.jpg", deflate(&jpg(1))),
                ("BIN0002.png", png(2)),
                ("BIN0003.OLE", b"ole".to_vec()),
                ("BIN0004.bin", deflate(b"just some attachment")),
                ("BIN0005.jpg", deflate(&jpg(1))),
                ("BIN0006.wmf", deflate(&wmf())),
            ],
        );
        p
    }

    #[test]
    fn hwp5_extract() {
        let t = tempfile::tempdir().unwrap();
        let p = sample_hwp5(t.path());
        let r = extract(&p, Some(t.path())).unwrap();
        let names: Vec<String> = r.files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["01_BIN0001.jpg", "02_BIN0002.png", "03_BIN0006.wmf"]);
        assert_eq!(r.folder.as_ref().unwrap().file_name().unwrap(), "보고서_이미지");
        assert_eq!(std::fs::read(&r.files[0]).unwrap(), jpg(1));
        assert_eq!(std::fs::read(&r.files[1]).unwrap(), png(2));
        assert_eq!(std::fs::read(&r.files[2]).unwrap(), wmf());
        assert_eq!(r.duplicates, 1);
        assert_eq!(r.skipped, 4, "{:?}", r.skipped_items);
        assert!(r.skipped_items.iter().any(|s| s.starts_with("BIN0003.OLE") && s.contains("OLE")));
        assert!(r.skipped_items.iter().any(|s| s.starts_with("BIN0004.bin") && s.contains("그림 형식이 아님")));
        assert!(r.skipped_items.iter().any(|s| s.contains("연결")));
        assert!(r.skipped_items.iter().any(|s| s.starts_with("BIN0007.png") && s.contains("데이터가 없음")));
        // 다시 하면 새 폴더(덮어쓰지 않음)
        let r2 = extract(&p, Some(t.path())).unwrap();
        assert_ne!(r2.folder, r.folder);
        assert_eq!(r2.files.len(), 3);
    }

    #[test]
    fn hwp5_limits() {
        let t = tempfile::tempdir().unwrap();
        let p = sample_hwp5(t.path());
        // 그림 하나 한도 50바이트: 압축 해제 폭탄처럼 큰 항목은 건너뜀
        let lim = Limits { max_item: 50, ..LIMITS };
        let r = extract_with(&p, Some(t.path()), &lim).unwrap();
        assert_eq!(r.files.len(), 1, "{:?}", r.skipped_items); // png(48바이트)만
        assert!(r.skipped_items.iter().any(|s| s.starts_with("BIN0001.jpg") && s.contains("너무 커서")));
        // 전체 한도
        let lim = Limits { max_total: 110, ..LIMITS };
        let r = extract_with(&p, Some(t.path()), &lim).unwrap();
        assert_eq!(r.files.len(), 1, "{:?}", r.skipped_items); // jpg 104바이트 뒤로는 한도 초과
        // 항목 수 한도
        let lim = Limits { max_items: 2, ..LIMITS };
        let r = extract_with(&p, Some(t.path()), &lim).unwrap();
        assert_eq!(r.files.len(), 2);
        assert!(r.skipped_items.iter().any(|s| s.contains("너무 많아")));
        // 진짜 압축 폭탄: 1 MB 0바이트 → 수 KB
        let bomb = t.path().join("폭탄.hwp");
        write_hwp5(&bomb, 1, &bin_data_record(1, 0, 1, "png"), &[("BIN0001.png", deflate(&vec![0u8; 1 << 20]))]);
        let r = extract_with(&bomb, Some(t.path()), &Limits { max_item: 64 << 10, ..LIMITS }).unwrap();
        assert!(r.files.is_empty() && r.skipped_items[0].contains("너무 커서"));
    }

    #[test]
    fn hwp5_protected_and_others() {
        let t = tempfile::tempdir().unwrap();
        let enc = t.path().join("암호.hwp");
        write_hwp5(&enc, 1 | 2, b"", &[]);
        assert_eq!(extract(&enc, Some(t.path())).unwrap_err().to_string(), MSG_ENCRYPTED);
        let dist = t.path().join("배포용.hwp");
        write_hwp5(&dist, 1 | 4, b"", &[]);
        assert_eq!(extract(&dist, Some(t.path())).unwrap_err().to_string(), MSG_DISTRIBUTION);
        let bad = t.path().join("손상.hwp");
        std::fs::write(&bad, b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1 cut").unwrap();
        assert_eq!(extract(&bad, Some(t.path())).unwrap_err().to_string(), MSG_DAMAGED);
        let old = t.path().join("옛.hwp");
        std::fs::write(&old, b"HWP Document File V3.00 \x1a\x01\x02\x03\x04\x05").unwrap();
        assert!(extract(&old, Some(t.path())).unwrap_err().to_string().contains("한글 97"));
        // DocInfo가 망가진(압축 표시인데 압축이 아님) 문서
        let p = t.path().join("docinfo.hwp");
        let mut c = cfb::create(&p).unwrap();
        std::io::Write::write_all(&mut c.create_stream("/FileHeader").unwrap(), &file_header(0x05000303, 1)).unwrap();
        std::io::Write::write_all(&mut c.create_stream("/DocInfo").unwrap(), b"\xff\xff\xff\xff not deflate").unwrap();
        c.flush().unwrap();
        drop(c);
        assert_eq!(extract(&p, Some(t.path())).unwrap_err().to_string(), MSG_DAMAGED);
        // 그림 없는 문서: 오류 아님, 폴더 만들지 않음
        let empty = t.path().join("글만.hwp");
        write_hwp5(&empty, 1, &record(0x10, 0, &[0; 26]), &[]);
        let r = extract(&empty, Some(t.path())).unwrap();
        assert!(r.files.is_empty() && r.folder.is_none());
    }

    #[test]
    fn hwpx_extract_and_traversal() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("발표.hwpx");
        let (a, b, w) = (png(3), jpg(4), wmf());
        write_hwpx(
            &p,
            &[("BinData/image2.jpg", "image/jpg", &b), ("BinData/image1.png", "image/png", &a), ("BinData/ole1.ole", "application/ole", b"not an image")],
            &[("BinData/image9.wmf", &w), ("BinData/image3.png", &a), ("BinData/../../escape.png", &a), ("Contents/section0.xml", b"<hs:sec/>")],
        );
        let r = extract(&p, Some(t.path())).unwrap();
        let names: Vec<String> = r.files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
        // manifest 순서 → 나머지 BinData 이름순, 같은 그림(image3 = image1)은 한 번
        assert_eq!(names, vec!["01_image2.jpg", "02_image1.png", "03_image9.wmf"]);
        assert_eq!(r.duplicates, 1);
        assert!(r.skipped_items.iter().any(|s| s.starts_with("ole1.ole")));
        assert!(r.skipped_items.iter().any(|s| s.starts_with("escape.png") && s.contains("올바르지 않은")));
        // 폴더 밖으로 나간 파일 없음
        assert!(!t.path().join("escape.png").exists() && !t.path().parent().unwrap().join("escape.png").exists());
        for f in &r.files {
            assert_eq!(f.parent(), r.folder.as_deref());
        }
        // 크기 한도
        let r = extract_with(&p, Some(t.path()), &Limits { max_item: 50, ..LIMITS }).unwrap();
        assert_eq!(r.files.len(), 1);
        // 암호 HWPX
        let enc = t.path().join("암호.hwpx");
        write_hwpx(&enc, &[("BinData/image1.png", "image/png", &a)], &[("META-INF/manifest.xml", b"<m><encryption-data/></m>")]);
        assert_eq!(extract(&enc, Some(t.path())).unwrap_err().to_string(), MSG_ENCRYPTED);
    }

    #[test]
    fn manifest_order() {
        let x = r#"<opf:package><opf:manifest><opf:item id="a" href="Contents/section0.xml"/><opf:item id="i2" href="BinData/b.png"/><opf:item id="i1" href="BinData/a.png"/></opf:manifest></opf:package>"#;
        assert_eq!(manifest_hrefs(x), vec!["BinData/b.png", "BinData/a.png"]);
    }
}
