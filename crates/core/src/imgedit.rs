//! 0.3.0 W4 이미지 고치기: 크기 바꾸기(px·%), 자르기(직접·비율), 돌리기, 뒤집기, 위치·촬영 정보 지우기,
//! 형식 바꾸기, 용량 맞추기(목표 KB). HEIC는 [`imageconv::load_heif`](ffmpeg)로 푼다.
//!
//! 적용 순서: EXIF 회전을 픽셀에 적용 → 자르기 → 돌리기 → 뒤집기 → 크기 바꾸기 → 저장.
//! 자르기 좌표는 "보이는 그대로"(EXIF 회전을 적용한 뒤) 픽셀 기준이다.
//!
//! `strip_meta`이면 EXIF(GPS·카메라·촬영 날짜)·XMP·IPTC를 넣지 않는다. 회전은 이미 픽셀에 적용되어 있다.
//! 고칠 것이 정보 지우기뿐인 JPEG(회전 값 없음)는 다시 압축하지 않고 정보 조각만 빼서 화질이 그대로다.
//! 결과는 `이름_편집.확장자`(겹치면 ` (1)`), 원본은 그대로. 수정 시각은 원본과 같게.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageReader};
use serde::{Deserialize, Serialize};

use crate::error::{io_err, Error, Result};
use crate::imageconv::{self, ImageOpts, ImgFormat};
use crate::process::Cancel;
use crate::{exif, heif, paths};

pub const SUFFIX: &str = "_편집";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ImageProbe {
    /// 보이는 크기(EXIF 회전 반영)
    pub width: u32,
    pub height: u32,
    /// "jpg" | "png" | "webp" | "heic" | "bmp" | "tif" | "gif"
    pub format: String,
    /// 바이트
    pub size: u64,
    pub has_gps: bool,
    /// EXIF 촬영 날짜 "YYYY-MM-DD HH:MM:SS"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taken_at: Option<String>,
}

/// EXIF 날짜 "2024:05:01 10:20:30" → "2024-05-01 10:20:30"
pub fn exif_date_iso(d: &str) -> String {
    let mut s = d.to_string();
    if s.len() >= 10 && s.as_bytes()[4] == b':' && s.as_bytes()[7] == b':' {
        s.replace_range(4..5, "-");
        s.replace_range(7..8, "-");
    }
    s
}

fn fmt_name(f: image::ImageFormat) -> &'static str {
    use image::ImageFormat as F;
    match f {
        F::Jpeg => "jpg",
        F::Png => "png",
        F::WebP => "webp",
        F::Bmp => "bmp",
        F::Tiff => "tif",
        F::Gif => "gif",
        _ => "기타",
    }
}

/// 크기·형식·위치 정보 유무·촬영 날짜. HEIC는 ffmpeg 없이 상자 정보만 읽는다.
pub fn probe(path: &Path) -> Result<ImageProbe> {
    let bytes = std::fs::read(path).map_err(io_err(path))?;
    let size = bytes.len() as u64;
    if heif::is_heif(&bytes) {
        let info = heif::HeifInfo::parse(&bytes).ok_or_else(|| Error::Image { path: path.into(), message: "HEIC 구조를 읽지 못했습니다".into() })?;
        let (width, height) = info.display_size().unwrap_or((0, 0));
        let ex = info.exif();
        return Ok(ImageProbe {
            width,
            height,
            format: "heic".into(),
            size,
            has_gps: ex.as_deref().is_some_and(exif::has_gps),
            taken_at: ex.as_deref().and_then(exif::date_taken).map(|d| exif_date_iso(&d)),
        });
    }
    let reader = ImageReader::new(Cursor::new(&bytes)).with_guessed_format().map_err(io_err(path))?;
    let format = reader.format().ok_or_else(|| Error::Unsupported(path.display().to_string()))?;
    let mut dec = reader.into_decoder().map_err(|e| Error::Image { path: path.into(), message: e.to_string() })?;
    use image::ImageDecoder;
    let (w, h) = dec.dimensions();
    let ex = dec.exif_metadata().ok().flatten();
    let orient = ex.as_deref().and_then(exif::orientation).unwrap_or(1);
    let (width, height) = if (5..=8).contains(&orient) { (h, w) } else { (w, h) };
    Ok(ImageProbe {
        width,
        height,
        format: fmt_name(format).into(),
        size,
        has_gps: ex.as_deref().is_some_and(exif::has_gps),
        taken_at: ex.as_deref().and_then(exif::date_taken).map(|d| exif_date_iso(&d)),
    })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Resize {
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub percent: Option<f32>,
    #[serde(default = "yes")]
    pub keep_ratio: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Crop {
    Rect { x: u32, y: u32, w: u32, h: u32 },
    /// aspect: "1:1" | "4:3" | "16:9" | "3:4" | "9:16", anchor: "center"
    Aspect {
        aspect: String,
        #[serde(default = "center")]
        anchor: String,
    },
}

fn center() -> String {
    "center".into()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutFormat {
    Keep,
    Jpg,
    Png,
    Webp,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EditOps {
    #[serde(default)]
    pub resize: Option<Resize>,
    #[serde(default)]
    pub crop: Option<Crop>,
    /// 시계 방향 90 | 180 | 270
    #[serde(default)]
    pub rotate: Option<u16>,
    /// "h"(좌우) | "v"(상하)
    #[serde(default)]
    pub flip: Option<String>,
    #[serde(default)]
    pub strip_meta: bool,
    #[serde(default)]
    pub format: Option<OutFormat>,
    /// JPG/WebP 품질 1~100(없으면 90)
    #[serde(default)]
    pub quality: Option<u8>,
    /// 목표 용량(KB = 1024바이트). 품질을, 그래도 크면 크기를 줄여 맞춘다.
    #[serde(default)]
    pub target_kb: Option<u32>,
}

impl EditOps {
    fn only_strip(&self) -> bool {
        self.resize.is_none()
            && self.crop.is_none()
            && self.rotate.is_none()
            && self.flip.is_none()
            && self.quality.is_none()
            && self.target_kb.is_none()
            && matches!(self.format, None | Some(OutFormat::Keep) | Some(OutFormat::Jpg))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EditOut {
    pub input: PathBuf,
    pub output: PathBuf,
    /// 바이트
    pub before: u64,
    pub after: u64,
    pub width: u32,
    pub height: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn out_format(src: &Path, f: Option<OutFormat>) -> ImgFormat {
    match f.unwrap_or(OutFormat::Keep) {
        OutFormat::Jpg => ImgFormat::Jpg,
        OutFormat::Png => ImgFormat::Png,
        OutFormat::Webp => ImgFormat::Webp,
        OutFormat::Keep => {
            let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            match ext.as_str() {
                "png" => ImgFormat::Png,
                "webp" => ImgFormat::Webp,
                "bmp" => ImgFormat::Bmp,
                "tif" | "tiff" => ImgFormat::Tiff,
                "gif" => ImgFormat::Gif,
                // jpg·jpeg·jfif, HEIC(쓸 수 없으므로 JPG로)
                _ => ImgFormat::Jpg,
            }
        }
    }
}

/// 비율 "16:9" → 가운데 맞춰 가장 큰 사각형
pub fn aspect_rect(w: u32, h: u32, aspect: &str) -> Result<(u32, u32, u32, u32)> {
    let (a, b) = aspect
        .split_once(':')
        .and_then(|(a, b)| Some((a.trim().parse::<u32>().ok()?, b.trim().parse::<u32>().ok()?)))
        .filter(|(a, b)| *a > 0 && *b > 0)
        .ok_or_else(|| Error::Invalid(format!("비율을 알 수 없습니다: {aspect}")))?;
    // w/h 가 a/b보다 넓으면 가로를 줄인다
    let (cw, ch) = if (w as u64) * (b as u64) > (h as u64) * (a as u64) {
        (((h as u64 * a as u64) / b as u64) as u32, h)
    } else {
        (w, ((w as u64 * b as u64) / a as u64) as u32)
    };
    let (cw, ch) = (cw.max(1), ch.max(1));
    Ok(((w - cw) / 2, (h - ch) / 2, cw, ch))
}

/// 크기 바꾸기 목표(가로, 세로). None이면 그대로.
pub fn resize_target(w: u32, h: u32, r: &Resize) -> Result<Option<(u32, u32)>> {
    let round = |v: f64| (v.round() as u32).max(1);
    if let Some(p) = r.percent {
        if !(p > 0.0 && p <= 1000.0) {
            return Err(Error::Invalid(format!("비율(%) 값이 올바르지 않습니다: {p}")));
        }
        let f = p as f64 / 100.0;
        return Ok(Some((round(w as f64 * f), round(h as f64 * f))));
    }
    let (tw, th) = (r.width.filter(|v| *v > 0), r.height.filter(|v| *v > 0));
    Ok(match (tw, th) {
        (None, None) => None,
        (Some(tw), None) => Some((tw, if r.keep_ratio { round(h as f64 * tw as f64 / w as f64) } else { h })),
        (None, Some(th)) => Some((if r.keep_ratio { round(w as f64 * th as f64 / h as f64) } else { w }, th)),
        (Some(tw), Some(th)) if r.keep_ratio => {
            // 상자 안에 들어가게
            let f = (tw as f64 / w as f64).min(th as f64 / h as f64);
            Some((round(w as f64 * f), round(h as f64 * f)))
        }
        (Some(tw), Some(th)) => Some((tw, th)),
    })
}

/// 자르기·돌리기·뒤집기·크기 바꾸기 적용(EXIF 회전은 이미 적용된 이미지)
pub fn apply_ops(mut img: DynamicImage, ops: &EditOps) -> Result<DynamicImage> {
    if let Some(c) = &ops.crop {
        let (w, h) = (img.width(), img.height());
        let (x, y, cw, ch) = match c {
            Crop::Rect { x, y, w: cw, h: ch } => {
                if *x >= w || *y >= h || *cw == 0 || *ch == 0 {
                    return Err(Error::Invalid(format!("자를 영역이 사진 밖입니다 (사진 {w}×{h})")));
                }
                (*x, *y, (*cw).min(w - x), (*ch).min(h - y))
            }
            Crop::Aspect { aspect, anchor } => {
                if anchor != "center" {
                    return Err(Error::Invalid(format!("자르기 기준을 알 수 없습니다: {anchor}")));
                }
                aspect_rect(w, h, aspect)?
            }
        };
        img = img.crop_imm(x, y, cw, ch);
    }
    img = match ops.rotate.unwrap_or(0) {
        0 | 360 => img,
        90 => img.rotate90(),
        180 => img.rotate180(),
        270 => img.rotate270(),
        r => return Err(Error::Invalid(format!("돌리기는 90·180·270도만 됩니다: {r}"))),
    };
    img = match ops.flip.as_deref() {
        None | Some("") => img,
        Some("h") => img.fliph(),
        Some("v") => img.flipv(),
        Some(f) => return Err(Error::Invalid(format!("뒤집기는 h(좌우)·v(상하)만 됩니다: {f}"))),
    };
    if let Some(r) = &ops.resize {
        if let Some((tw, th)) = resize_target(img.width(), img.height(), r)? {
            if (tw, th) != (img.width(), img.height()) {
                img = img.resize_exact(tw, th, image::imageops::FilterType::Lanczos3);
            }
        }
    }
    Ok(img)
}

/// JPEG에서 정보 조각(APP1 Exif/XMP, APP13 IPTC, COM)만 뺀다. 다시 압축하지 않는다.
/// 회전 값(1이 아닌)이 있으면 None(픽셀에 적용해야 하므로 다시 압축 경로로).
pub fn strip_jpeg_lossless(jpeg: &[u8]) -> Option<Vec<u8>> {
    if jpeg.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    if exif::from_jpeg(jpeg).as_deref().and_then(exif::orientation).is_some_and(|o| o != 1) {
        return None;
    }
    let mut out = vec![0xFF, 0xD8];
    let mut i = 2;
    loop {
        if i + 4 > jpeg.len() || jpeg[i] != 0xFF {
            return None;
        }
        let marker = jpeg[i + 1];
        if marker == 0xDA {
            out.extend_from_slice(&jpeg[i..]);
            return Some(out);
        }
        let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        let seg = jpeg.get(i..i + 2 + len)?;
        let drop = matches!(marker, 0xE1 | 0xED | 0xFE) || (marker == 0xE2 && seg.get(4..).is_some_and(|b| !b.starts_with(b"ICC_PROFILE")));
        if !drop {
            out.extend_from_slice(seg);
        }
        i += 2 + len;
    }
}

fn encode_with(img: &DynamicImage, format: ImgFormat, quality: u8, exif: Option<Vec<u8>>, path: &Path) -> Result<Vec<u8>> {
    let keep = exif.is_some();
    imageconv::encode(img, &ImageOpts { format, quality, max_side: None, keep_exif: keep }, exif, path)
}

/// 목표 바이트 안에 맞춘다: 품질(JPG/WebP)을 이분 탐색, 가장 낮은 품질로도 크면 크기를 줄여 다시.
/// (결과 바이트, 최종 이미지 크기, 쓴 품질, 메모)
pub fn fit_to_size(
    img: &DynamicImage,
    format: ImgFormat,
    target: u64,
    exif: Option<Vec<u8>>,
    path: &Path,
    cancel: Option<&Cancel>,
) -> Result<(Vec<u8>, (u32, u32), Option<u8>, Option<String>)> {
    let lossy = matches!(format, ImgFormat::Jpg | ImgFormat::Webp);
    let (q_lo, q_hi) = (12u8, 95u8);
    let mut cur = img.clone();
    let mut best: Option<(Vec<u8>, (u32, u32), Option<u8>)> = None;
    let mut scaled = false;
    for _round in 0..8 {
        if let Some(c) = cancel {
            c.check()?;
        }
        let dims = (cur.width(), cur.height());
        if lossy {
            let hi = encode_with(&cur, format, q_hi, exif.clone(), path)?;
            if hi.len() as u64 <= target {
                best = Some((hi, dims, Some(q_hi)));
                break;
            }
            let lo = encode_with(&cur, format, q_lo, exif.clone(), path)?;
            if lo.len() as u64 <= target {
                // 이분 탐색: 목표 안에서 가장 높은 품질
                let (mut a, mut b) = (q_lo, q_hi); // a: 맞음, b: 넘음
                let mut ok = (lo, q_lo);
                while b - a > 1 {
                    if let Some(c) = cancel {
                        c.check()?;
                    }
                    let m = a + (b - a) / 2;
                    let e = encode_with(&cur, format, m, exif.clone(), path)?;
                    if e.len() as u64 <= target {
                        a = m;
                        ok = (e, m);
                    } else {
                        b = m;
                    }
                }
                best = Some((ok.0, dims, Some(ok.1)));
                break;
            }
            let f = ((target as f64 / lo.len() as f64).sqrt() * 0.92).clamp(0.2, 0.95);
            best.get_or_insert((lo, dims, Some(q_lo)));
            cur = scale(&cur, f);
        } else {
            let e = encode_with(&cur, format, 90, exif.clone(), path)?;
            let len = e.len() as u64;
            let fits = len <= target;
            if best.as_ref().is_none_or(|b| fits || (b.0.len() as u64) > len) {
                best = Some((e, dims, None));
            }
            if fits {
                break;
            }
            let f = ((target as f64 / len as f64).sqrt() * 0.95).clamp(0.2, 0.95);
            cur = scale(&cur, f);
        }
        scaled = true;
        if cur.width() <= 16 || cur.height() <= 16 {
            break;
        }
    }
    let (bytes, dims, q) = best.ok_or_else(|| Error::Invalid("용량을 맞추지 못했습니다".into()))?;
    let kb = |b: u64| (b as f64 / 1024.0).round() as u64;
    let note = if bytes.len() as u64 > target {
        Some(format!("목표 {}KB까지 줄이지 못해 가장 작게 만든 {}KB로 저장했습니다", kb(target), kb(bytes.len() as u64)))
    } else if scaled {
        Some(format!("화질만으로는 {}KB가 넘어 크기를 {}×{}로 줄였습니다", kb(target), dims.0, dims.1))
    } else if !lossy {
        None
    } else if q == Some(q_hi) {
        Some(format!("이미 {}KB보다 작습니다", kb(target)))
    } else {
        None
    };
    Ok((bytes, dims, q, note))
}

fn scale(img: &DynamicImage, f: f64) -> DynamicImage {
    let w = ((img.width() as f64 * f).round() as u32).max(1);
    let h = ((img.height() as f64 * f).round() as u32).max(1);
    img.resize_exact(w, h, image::imageops::FilterType::Lanczos3)
}

/// 사진 한 장 고치기. HEIC는 `ffmpeg`가 있어야 한다.
pub fn edit(ffmpeg: Option<&Path>, src: &Path, ops: &EditOps, out_dir: Option<&Path>, cancel: Option<&Cancel>) -> Result<EditOut> {
    let bytes = std::fs::read(src).map_err(io_err(src))?;
    let before = bytes.len() as u64;
    let is_heif = heif::is_heif(&bytes);
    let format = if is_heif && matches!(ops.format, None | Some(OutFormat::Keep)) { ImgFormat::Jpg } else { out_format(src, ops.format) };
    let output = paths::output_path(src, out_dir, SUFFIX, format.ext());

    // 정보만 지우는 JPEG: 다시 압축하지 않는다
    if ops.strip_meta && ops.only_strip() && !is_heif && format == ImgFormat::Jpg {
        if let Some(clean) = strip_jpeg_lossless(&bytes) {
            let (w, h) = image::load_from_memory(&clean).map(|i| (i.width(), i.height())).unwrap_or((0, 0));
            paths::write_new(&output, &clean)?;
            imageconv::copy_mtime(src, &output);
            return Ok(EditOut {
                input: src.into(),
                output,
                before,
                after: clean.len() as u64,
                width: w,
                height: h,
                note: Some("다시 압축하지 않아 화질이 그대로입니다".into()),
            });
        }
    }

    let (img, exif) = if is_heif {
        let ff = ffmpeg.ok_or_else(|| Error::ToolMissing { tool: "ffmpeg".into(), hint: "HEIC 사진을 풀려면 ffmpeg가 필요합니다.".into() })?;
        imageconv::load_heif(ff, src, cancel)?
    } else {
        imageconv::decode(&bytes, src)?
    };
    if let Some(c) = cancel {
        c.check()?;
    }
    let img = apply_ops(img, ops)?;
    let exif = if ops.strip_meta { None } else { exif };
    let keeps_exif = matches!(format, ImgFormat::Jpg | ImgFormat::Png);
    let exif = exif.filter(|_| keeps_exif);
    let quality = ops.quality.unwrap_or(90).clamp(1, 100);
    let (out_bytes, (w, h), note) = match ops.target_kb.filter(|k| *k > 0) {
        Some(kb) => {
            let (b, dims, _q, note) = fit_to_size(&img, format, kb as u64 * 1024, exif, src, cancel)?;
            (b, dims, note)
        }
        None => (encode_with(&img, format, quality, exif, src)?, (img.width(), img.height()), None),
    };
    paths::write_new(&output, &out_bytes)?;
    imageconv::copy_mtime(src, &output);
    Ok(EditOut { input: src.into(), output, before, after: out_bytes.len() as u64, width: w, height: h, note })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_and_resize_math() {
        assert_eq!(aspect_rect(4000, 3000, "1:1").unwrap(), (500, 0, 3000, 3000));
        assert_eq!(aspect_rect(4000, 3000, "16:9").unwrap(), (0, 375, 4000, 2250));
        assert_eq!(aspect_rect(3000, 4000, "9:16").unwrap(), (375, 0, 2250, 4000));
        assert!(aspect_rect(10, 10, "x").is_err());
        let r = |w, h, p, k| Resize { width: w, height: h, percent: p, keep_ratio: k };
        assert_eq!(resize_target(4000, 3000, &r(Some(1000), None, None, true)).unwrap(), Some((1000, 750)));
        assert_eq!(resize_target(4000, 3000, &r(None, Some(300), None, true)).unwrap(), Some((400, 300)));
        assert_eq!(resize_target(4000, 3000, &r(Some(1000), Some(1000), None, true)).unwrap(), Some((1000, 750)));
        assert_eq!(resize_target(4000, 3000, &r(Some(1000), Some(1000), None, false)).unwrap(), Some((1000, 1000)));
        assert_eq!(resize_target(4000, 3000, &r(None, None, Some(25.0), true)).unwrap(), Some((1000, 750)));
        assert_eq!(resize_target(4000, 3000, &r(None, None, None, true)).unwrap(), None);
        assert!(resize_target(4000, 3000, &r(None, None, Some(0.0), true)).is_err());
    }

    #[test]
    fn ops_json_shapes() {
        let o: EditOps = serde_json::from_str(r#"{"crop":{"aspect":"4:3","anchor":"center"},"rotate":90,"flip":"h","strip_meta":true,"format":"webp","target_kb":100}"#).unwrap();
        assert_eq!(o.crop, Some(Crop::Aspect { aspect: "4:3".into(), anchor: "center".into() }));
        assert_eq!(o.format, Some(OutFormat::Webp));
        let o: EditOps = serde_json::from_str(r#"{"crop":{"x":1,"y":2,"w":3,"h":4},"resize":{"percent":50},"strip_meta":false}"#).unwrap();
        assert_eq!(o.crop, Some(Crop::Rect { x: 1, y: 2, w: 3, h: 4 }));
        assert!(o.resize.unwrap().keep_ratio);
    }
}
