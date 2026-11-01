//! 0.4 사진 가리기(신분증 사본 제출용). JPG/PNG/HEIC → 검은 상자를 그림에 직접 칠하고(흐림 없음),
//! 원하면 "○○ 제출용" 같은 글을 비스듬히 바둑판으로 얹어 `이름_가림.jpg`(PNG는 `.png`)로 저장한다.
//! 위치·촬영 정보(EXIF)는 넣지 않는다(새로 인코딩). 사진 방향(EXIF 회전)은 픽셀에 적용한 뒤 상자 좌표를 쓴다.
//!
//! 글 얹기는 PDF 워터마크 코드([`crate::pdfedit::stamp::watermark`], 한글 부분 글꼴)를 그대로 쓴다:
//! 그림 한 장짜리 임시 PDF → 워터마크 → PDFium으로 원래 픽셀 크기로 다시 그림. 임시 파일은 작업이 끝나면 지운다.

use std::path::{Path, PathBuf};

use image::{DynamicImage, RgbImage};
use lopdf::{dictionary, Document, Object, Stream};
use serde::{Deserialize, Serialize};

use crate::error::{io_err, Error, Result};
use crate::pdfedit::stamp::{self, Layer, Tiling, WatermarkKind, WatermarkOpts};
use crate::pdfedit::Job;

/// 가릴 상자(사진 px, 원점 왼쪽 위, 방향을 바로잡은 사진 기준)
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PxBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImgRedactOpts {
    #[serde(default)]
    pub boxes: Vec<PxBox>,
    /// 비스듬히 얹을 글(예: "○○은행 제출용"). 비우면 없음
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImgRedactOut {
    pub input: PathBuf,
    pub output: PathBuf,
    pub width: u32,
    pub height: u32,
    pub boxes: usize,
    pub text: bool,
    pub note: String,
}

/// 미리보기(방향을 바로잡은 사진을 긴 변 `max_px` 안으로 줄인 JPEG) + 원래 크기
#[derive(Debug, Clone, Serialize)]
pub struct ImgPreview {
    pub data_url: String,
    pub width: u32,
    pub height: u32,
}

fn b64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

fn flatten(img: DynamicImage) -> RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    let mut out = RgbImage::new(rgba.width(), rgba.height());
    for (o, p) in out.pixels_mut().zip(rgba.pixels()) {
        let a = p[3] as u32;
        for c in 0..3 {
            o[c] = ((p[c] as u32 * a + 255 * (255 - a)) / 255) as u8;
        }
    }
    out
}

pub fn preview(input: &Path, ffmpeg: Option<&Path>, max_px: u32) -> Result<ImgPreview> {
    let img = crate::ocr::pdf::load_image(input, ffmpeg, None)?;
    let (w, h) = (img.width(), img.height());
    let max_px = max_px.clamp(200, 2400);
    let small = if w.max(h) > max_px { img.resize(max_px, max_px, image::imageops::FilterType::Triangle) } else { img };
    let mut jpg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 82).encode_image(&flatten(small)).map_err(|e| Error::Image { path: input.into(), message: e.to_string() })?;
    Ok(ImgPreview { data_url: format!("data:image/jpeg;base64,{}", b64(&jpg)), width: w, height: h })
}

fn burn(img: &mut RgbImage, boxes: &[PxBox]) -> usize {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let mut n = 0;
    for b in boxes {
        if !(b.w > 0.0 && b.h > 0.0 && b.x.is_finite() && b.y.is_finite()) {
            continue;
        }
        let x0 = (b.x.floor() as i64).clamp(0, w);
        let y0 = (b.y.floor() as i64).clamp(0, h);
        let x1 = ((b.x + b.w).ceil() as i64).clamp(0, w);
        let y1 = ((b.y + b.h).ceil() as i64).clamp(0, h);
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x as u32, y as u32, image::Rgb([0, 0, 0]));
            }
        }
        n += 1;
    }
    n
}

/// 사진 위에 비스듬한 글을 바둑판으로(PDF 워터마크 코드 + PDFium으로 다시 그림)
fn stamp_text(img: &RgbImage, text: &str) -> Result<RgbImage> {
    let (w, h) = img.dimensions();
    let dir = crate::tempdir::JobDir::new("imgredact")?;
    let src = dir.path().join("in.pdf");
    let dst = dir.path().join("out.pdf");
    // 쪽 크기: 긴 변 1600pt 안(글자 크기 제한 4~400pt 안에 들게), 다시 그릴 때 원래 px로
    let k = (1600.0 / w.max(h) as f32).min(1.0);
    let (pw, ph) = (w as f32 * k, h as f32 * k);
    let mut doc = Document::with_version("1.5");
    let mut png = Vec::new();
    DynamicImage::ImageRgb8(img.clone()).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| Error::Invalid(e.to_string()))?;
    let (im, _, _) = crate::pdfedit::image_xobject_bytes(&mut doc, &png, &src, false)?;
    let pages_id = doc.new_object_id();
    let c = doc.add_object(Stream::new(dictionary! {}, format!("q {pw} 0 0 {ph} 0 0 cm /Im0 Do Q").into_bytes()));
    let page = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => c,
        "MediaBox" => vec![0.into(), 0.into(), Object::Real(pw), Object::Real(ph)],
        "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => im } },
    });
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1 }));
    let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", cat);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(io_err(&src))?;
    std::fs::write(&src, &bytes).map_err(io_err(&src))?;
    let size = (pw.max(ph) / 16.0).clamp(10.0, 400.0);
    let opts = WatermarkOpts {
        kind: WatermarkKind::Text { text: text.to_string(), font_size: size, color: "#3C3C3C".into() },
        opacity: 0.42,
        angle: 30.0,
        tiling: Tiling::Tile,
        layer: Layer::Over,
        pages: None,
    };
    stamp::watermark(&src, None, &opts, &dst, &mut Job::none())?;
    let out = crate::pdfedit::thumbs::render_rgb(&dst, None, 1, 1.0 / k)?;
    Ok(if out.dimensions() == (w, h) { out } else { image::imageops::resize(&out, w, h, image::imageops::FilterType::Triangle) })
}

/// 사진 가리기 → `이름_가림.jpg`(PNG는 `.png`, HEIC 등은 JPG)
pub fn redact(input: &Path, ffmpeg: Option<&Path>, opts: &ImgRedactOpts, out_dir: Option<&Path>) -> Result<ImgRedactOut> {
    let text = opts.text.as_deref().map(str::trim).filter(|t| !t.is_empty());
    if opts.boxes.is_empty() && text.is_none() {
        return Err(Error::Invalid("가릴 곳을 그리거나 얹을 글을 적어 주세요".into()));
    }
    let img = crate::ocr::pdf::load_image(input, ffmpeg, None)?;
    let mut rgb = flatten(img);
    let n = burn(&mut rgb, &opts.boxes);
    if n == 0 && text.is_none() {
        return Err(Error::Invalid("가릴 상자가 사진 밖에 있어요. 다시 그려 주세요".into()));
    }
    if let Some(t) = text {
        rgb = stamp_text(&rgb, t)?;
        // 글을 얹은 뒤에도 상자는 완전히 검게(글이 상자 위로 지나가도 가린 곳이 비치지 않게)
        burn(&mut rgb, &opts.boxes);
    }
    let png = input.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("png"));
    let mut bytes = Vec::new();
    let enc = if png {
        DynamicImage::ImageRgb8(rgb.clone()).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
    } else {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 92).encode_image(&rgb)
    };
    enc.map_err(|e| Error::Image { path: input.into(), message: e.to_string() })?;
    let output = crate::paths::output_path(input, out_dir, "_가림", if png { "png" } else { "jpg" });
    crate::paths::write_new(&output, &bytes)?;
    let mut note = format!("{n}곳을 검게 가렸어요");
    if let Some(t) = text {
        note.push_str(&format!(" · '{t}' 글을 얹었어요"));
    }
    note.push_str(" · 위치·촬영 정보는 넣지 않았어요");
    Ok(ImgRedactOut { input: input.into(), output, width: rgb.width(), height: rgb.height(), boxes: n, text: text.is_some(), note })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn burn_clamps_and_counts() {
        let mut img = RgbImage::from_pixel(50, 40, image::Rgb([255, 255, 255]));
        let n = burn(&mut img, &[PxBox { x: 40.0, y: 30.0, w: 100.0, h: 100.0 }, PxBox { x: 100.0, y: 0.0, w: 5.0, h: 5.0 }]);
        assert_eq!(n, 1);
        assert_eq!(img.get_pixel(49, 39)[0], 0);
        assert_eq!(img.get_pixel(39, 29)[0], 255);
    }
}
