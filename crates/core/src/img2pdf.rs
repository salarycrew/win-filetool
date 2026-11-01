//! 여러 이미지 → PDF 하나(한 쪽에 한 장, 목록 순서대로).
//!
//! - JPEG(RGB·흑백, baseline/progressive)은 **다시 인코딩하지 않고** 그대로 넣는다(DCTDecode).
//!   EXIF 회전 값은 픽셀을 돌리지 않고 쪽에 그리는 행렬로 반영한다(화질 손실 없음).
//! - CMYK JPEG 등 그대로 넣을 수 없는 JPEG, PNG/WebP/BMP/TIFF/GIF는 풀어서(EXIF 회전 적용)
//!   무손실(Flate)로 넣고, 투명도가 있으면 SMask로 넣는다.
//! - HEIC/HEIF는 1b 경로(ffmpeg, grid·irot 처리)로 푼 뒤 JPEG(품질 92)로 넣는다(무손실로 넣으면 원본보다 훨씬 커짐).
//!
//! 쪽 크기: 이미지 크기 그대로(96 DPI 기준 px × 0.75 pt + 여백) 또는 A4 세로·가로(여백 안에 비율 유지, 가운데).

use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageEncoder};
use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream};
use serde::{Deserialize, Serialize};

use crate::error::{io_err, Error, Result};
use crate::process::Cancel;
use crate::{exif, imageconv, paths};

pub const A4_W: f64 = 595.276;
pub const A4_H: f64 = 841.89;
/// 이미지 크기 그대로일 때 1px = 0.75pt (96 DPI)
pub const PX_TO_PT: f64 = 72.0 / 96.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageSize {
    /// 이미지 크기 그대로
    Image,
    A4Portrait,
    A4Landscape,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Img2PdfOpts {
    pub page: PageSize,
    /// 여백(mm)
    #[serde(default)]
    pub margin_mm: f64,
    /// 0.3.2 "사진을 PDF 하나로": 용량을 알맞게 — 찍히는 크기에서 200 DPI보다 큰 사진은 줄이고,
    /// 사진 같은 PNG·WEBP·HEIC는 무손실 대신 JPEG(품질 85)로 넣는다. 끄면(기본) 예전처럼 원본 그대로.
    #[serde(default)]
    pub compact: bool,
}

/// compact일 때 찍히는 크기 기준 최대 DPI
pub const COMPACT_DPI: f64 = 200.0;

/// 사진처럼 색이 많은 그림인지(표본 64×64에서 서로 다른 색 1024개 이상). 화면 갈무리·도표는 무손실이 더 작고 선명하다.
pub fn looks_photographic(img: &DynamicImage) -> bool {
    let (w, h) = (img.width().max(1), img.height().max(1));
    let rgb = img.to_rgb8();
    let mut seen = std::collections::HashSet::new();
    for j in 0..64u32 {
        for i in 0..64u32 {
            let p = rgb.get_pixel(i * w / 64, j * h / 64);
            seen.insert((p[0] >> 2, p[1] >> 2, p[2] >> 2));
        }
    }
    seen.len() >= 1024
}

#[derive(Debug, Clone, Serialize)]
pub struct PageInfo {
    pub source: PathBuf,
    /// 쪽 크기(pt)
    pub page_w: f64,
    pub page_h: f64,
    /// 그린 크기(pt)
    pub draw_w: f64,
    pub draw_h: f64,
    /// 화면에 보이는 방향 기준 이미지 픽셀 크기
    pub px_w: u32,
    pub px_h: u32,
    /// JPEG을 다시 인코딩하지 않고 넣었는지
    pub jpeg_passthrough: bool,
    pub orientation: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct Img2PdfOut {
    pub output: PathBuf,
    pub pages: Vec<PageInfo>,
    pub note: String,
}

/// JPEG SOF에서 (가로, 세로, 성분 수, SOF 종류)를 읽는다.
pub fn jpeg_info(b: &[u8]) -> Option<(u32, u32, u8, u8)> {
    if b.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let m = b[i + 1];
        if m == 0xFF {
            i += 1;
            continue;
        }
        if m == 0xD8 || (0xD0..=0xD7).contains(&m) || m == 0x01 {
            i += 2;
            continue;
        }
        if m == 0xD9 || m == 0xDA {
            return None;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if matches!(m, 0xC0..=0xCF) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
            let s = b.get(i + 4..i + 2 + len)?;
            let h = u16::from_be_bytes([s[1], s[2]]) as u32;
            let w = u16::from_be_bytes([s[3], s[4]]) as u32;
            return Some((w, h, s[5], m));
        }
        i += 2 + len;
    }
    None
}

/// EXIF 회전 값(1~8)과 그릴 상자(x0, y0, W, H — 보이는 방향 기준)로 이미지 행렬 `cm`을 만든다.
/// 이미지 공간의 단위 정사각형(왼쪽 아래 원점)을 상자에 맞게 돌리고 뒤집는다.
pub fn placement_matrix(orientation: u16, x0: f64, y0: f64, w: f64, h: f64) -> [f64; 6] {
    match orientation {
        2 => [-w, 0.0, 0.0, h, x0 + w, y0],
        3 => [-w, 0.0, 0.0, -h, x0 + w, y0 + h],
        4 => [w, 0.0, 0.0, -h, x0, y0 + h],
        5 => [0.0, -h, -w, 0.0, x0 + w, y0 + h],
        6 => [0.0, -h, w, 0.0, x0, y0 + h],
        7 => [0.0, h, w, 0.0, x0, y0],
        8 => [0.0, h, -w, 0.0, x0 + w, y0],
        _ => [w, 0.0, 0.0, h, x0, y0],
    }
}

/// 쪽 크기와 그릴 상자 (page_w, page_h, x0, y0, draw_w, draw_h)
pub fn layout(page: PageSize, margin_mm: f64, px_w: u32, px_h: u32) -> (f64, f64, f64, f64, f64, f64) {
    let m = (margin_mm.max(0.0) * 72.0 / 25.4).min(200.0);
    let iw = px_w as f64 * PX_TO_PT;
    let ih = px_h as f64 * PX_TO_PT;
    match page {
        PageSize::Image => (iw + 2.0 * m, ih + 2.0 * m, m, m, iw, ih),
        PageSize::A4Portrait | PageSize::A4Landscape => {
            let (pw, ph) = if page == PageSize::A4Portrait { (A4_W, A4_H) } else { (A4_H, A4_W) };
            let (aw, ah) = ((pw - 2.0 * m).max(1.0), (ph - 2.0 * m).max(1.0));
            let s = (aw / iw).min(ah / ih);
            let (dw, dh) = (iw * s, ih * s);
            (pw, ph, (pw - dw) / 2.0, (ph - dh) / 2.0, dw, dh)
        }
    }
}

enum Embedded {
    /// JPEG 그대로: (바이트, 원래 가로, 세로, 성분 수, EXIF 회전)
    Jpeg(Vec<u8>, u32, u32, u8, u16),
    /// 풀어 둔 이미지(회전 적용됨) + JPEG로 넣을지
    Pixels(DynamicImage, bool),
}

fn load_one(ffmpeg: Option<&Path>, path: &Path, cancel: Option<&Cancel>) -> Result<Embedded> {
    let is_heif = imageconv::is_heif_path(path) || imageconv::sniff_heif(path);
    if is_heif {
        let ff = ffmpeg.ok_or_else(|| Error::ToolMissing {
            tool: "ffmpeg".into(),
            hint: "HEIC(아이폰 사진)를 PDF에 넣으려면 ffmpeg가 필요합니다".into(),
        })?;
        let (img, _) = imageconv::load_heif(ff, path, cancel)?;
        return Ok(Embedded::Pixels(img, true));
    }
    let bytes = std::fs::read(path).map_err(io_err(path))?;
    if let Some((w, h, comps, sof)) = jpeg_info(&bytes) {
        // baseline(C0)·extended(C1)·progressive(C2), RGB/흑백만 그대로 넣는다
        if matches!(sof, 0xC0..=0xC2) && (comps == 3 || comps == 1) && w > 0 && h > 0 {
            let o = exif::from_jpeg(&bytes).and_then(|e| exif::orientation(&e)).filter(|o| (1..=8).contains(o)).unwrap_or(1);
            return Ok(Embedded::Jpeg(bytes, w, h, comps, o));
        }
    }
    let (img, _) = imageconv::decode(&bytes, path)?;
    Ok(Embedded::Pixels(img, false))
}

const JPEG_Q: u8 = 88;

fn add_pixels(doc: &mut Document, img: &DynamicImage, as_jpeg: bool) -> Result<lopdf::ObjectId> {
    let (w, h) = (img.width(), img.height());
    let gray = matches!(img.color(), image::ColorType::L8 | image::ColorType::La8 | image::ColorType::L16 | image::ColorType::La16);
    let cs = if gray { "DeviceGray" } else { "DeviceRGB" };
    let mut smask = None;
    if img.color().has_alpha() {
        let alpha: Vec<u8> = img.to_rgba8().pixels().map(|p| p[3]).collect();
        if alpha.iter().any(|&a| a != 255) {
            let mut s = Stream::new(
                dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
                    "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8 },
                alpha,
            );
            let _ = s.compress();
            smask = Some(doc.add_object(s));
        }
    }
    let raw: Vec<u8> = if gray { img.to_luma8().into_raw() } else { img.to_rgb8().into_raw() };
    let mut dict = dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
        "ColorSpace" => cs, "BitsPerComponent" => 8 };
    if let Some(m) = smask {
        dict.set("SMask", m);
    }
    let stream = if as_jpeg {
        let mut buf = Vec::new();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_Q);
        let ct = if gray { image::ExtendedColorType::L8 } else { image::ExtendedColorType::Rgb8 };
        enc.write_image(&raw, w, h, ct).map_err(|e| Error::Invalid(e.to_string()))?;
        dict.set("Filter", "DCTDecode");
        Stream::new(dict, buf).with_compression(false)
    } else {
        let mut s = Stream::new(dict, raw);
        let _ = s.compress();
        s
    };
    Ok(doc.add_object(stream))
}

/// compact: 찍히는 크기에서 200 DPI를 넘는 사진은 줄이고, 사진 같은 무손실 그림은 JPEG로
fn compact(emb: Embedded, opts: &Img2PdfOpts, path: &Path) -> Result<Embedded> {
    let (vw, vh) = match &emb {
        Embedded::Jpeg(_, w, h, _, o) => if (5..=8).contains(o) { (*h, *w) } else { (*w, *h) },
        Embedded::Pixels(img, _) => (img.width(), img.height()),
    };
    let (_, _, _, _, dw, dh) = layout(opts.page, opts.margin_mm, vw, vh);
    let max_w = (dw / 72.0 * COMPACT_DPI).ceil() as u32;
    let max_h = (dh / 72.0 * COMPACT_DPI).ceil() as u32;
    // "사진 크기 그대로"는 쪽이 사진 크기라 줄일 일이 없다(96 DPI 기준) — 형식만 본다
    let too_big = opts.page != PageSize::Image && (vw as f64 > max_w as f64 * 1.15 || vh as f64 > max_h as f64 * 1.15);
    Ok(match emb {
        Embedded::Jpeg(bytes, w, h, comps, o) => {
            if !too_big {
                Embedded::Jpeg(bytes, w, h, comps, o)
            } else {
                let (img, _) = imageconv::decode(&bytes, path)?; // 회전을 적용해 풀어 줌
                Embedded::Pixels(img.resize(max_w, max_h, image::imageops::FilterType::Lanczos3), true)
            }
        }
        Embedded::Pixels(img, as_jpeg) => {
            let img = if too_big { img.resize(max_w, max_h, image::imageops::FilterType::Lanczos3) } else { img };
            let photo = !img.color().has_alpha() && (as_jpeg || looks_photographic(&img));
            Embedded::Pixels(img, photo)
        }
    })
}

/// 기본 출력 이름: 한 장이면 `이름.pdf`, 여러 장이면 `첫이름_외N장.pdf`
pub fn default_output(inputs: &[PathBuf], out_dir: Option<&Path>) -> PathBuf {
    let first = inputs.first().cloned().unwrap_or_else(|| PathBuf::from("이미지"));
    let suffix = if inputs.len() > 1 { format!("_외{}장", inputs.len() - 1) } else { String::new() };
    paths::output_path(&first, out_dir, &suffix, "pdf")
}

pub fn images_to_pdf(
    ffmpeg: Option<&Path>,
    inputs: &[PathBuf],
    output: &Path,
    opts: &Img2PdfOpts,
    cancel: Option<&Cancel>,
    progress: &mut dyn FnMut(usize),
) -> Result<Img2PdfOut> {
    if inputs.is_empty() {
        return Err(Error::Invalid("PDF로 묶을 이미지를 넣어 주세요".into()));
    }
    if output.exists() {
        return Err(Error::AlreadyExists(output.to_path_buf()));
    }
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();
    let mut infos = Vec::new();
    for (i, path) in inputs.iter().enumerate() {
        if let Some(c) = cancel {
            c.check()?;
        }
        let mut emb = load_one(ffmpeg, path, cancel)?;
        if opts.compact {
            emb = compact(emb, opts, path)?;
        }
        let (img_id, px_w, px_h, orientation, pass) = match emb {
            Embedded::Jpeg(bytes, w, h, comps, o) => {
                let cs = if comps == 1 { "DeviceGray" } else { "DeviceRGB" };
                let s = Stream::new(
                    dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
                        "ColorSpace" => cs, "BitsPerComponent" => 8, "Filter" => "DCTDecode" },
                    bytes,
                )
                .with_compression(false);
                let id = doc.add_object(s);
                let (vw, vh) = if (5..=8).contains(&o) { (h, w) } else { (w, h) };
                (id, vw, vh, o, true)
            }
            Embedded::Pixels(img, as_jpeg) => {
                let id = add_pixels(&mut doc, &img, as_jpeg)?;
                (id, img.width(), img.height(), 1, false)
            }
        };
        let (pw, ph, x0, y0, dw, dh) = layout(opts.page, opts.margin_mm, px_w, px_h);
        let m = placement_matrix(orientation, x0, y0, dw, dh);
        let content = Content {
            operations: vec![
                Operation::new("q", vec![]),
                Operation::new("cm", m.iter().map(|v| Object::Real(*v as f32)).collect()),
                Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]),
                Operation::new("Q", vec![]),
            ],
        };
        let enc = content.encode().map_err(|e| Error::Invalid(e.to_string()))?;
        let cid = doc.add_object(Stream::new(dictionary! {}, enc));
        let pid = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => cid,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real(pw as f32), Object::Real(ph as f32)],
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => img_id } },
        });
        kids.push(Object::Reference(pid));
        infos.push(PageInfo {
            source: path.clone(),
            page_w: pw,
            page_h: ph,
            draw_w: dw,
            draw_h: dh,
            px_w,
            px_h,
            jpeg_passthrough: pass,
            orientation,
        });
        progress(i + 1);
    }
    let n = kids.len() as i64;
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => n }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    doc.compress();
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(io_err(output))?;
    paths::write_new(output, &bytes)?;
    let pass = infos.iter().filter(|i| i.jpeg_passthrough).count();
    let note = format!("{}쪽, JPEG {}장은 다시 인코딩하지 않고 넣음", infos.len(), pass);
    Ok(Img2PdfOut { output: output.to_path_buf(), pages: infos, note })
}
