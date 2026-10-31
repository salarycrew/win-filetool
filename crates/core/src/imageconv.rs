//! 이미지 변환: JPG/PNG/WebP/BMP/TIFF/GIF(첫 장면) 사이 (Rust `image` 크레이트, WebP 손실 압축은 libwebp).
//! HEIC/HEIF는 ffmpeg로 PNG로 푼 뒤(타일 grid·irot 회전 반영) 같은 과정을 거친다.
//!
//! 보존하는 것: EXIF 블록(촬영 날짜 포함) — JPG/PNG 출력에만. 회전은 픽셀에 적용하고 EXIF 회전 값은 1로 바꾼다.
//! 출력 파일의 수정 시각은 원본과 같게 맞춘다. WebP/BMP/TIFF/GIF 출력에는 EXIF를 넣지 않는다.

use std::ffi::OsString;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};

use crate::error::{io_err, Error, Result};
use crate::process::{self, Cancel};
use crate::{exif, heif, paths};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImgFormat {
    Jpg,
    Png,
    Webp,
    Bmp,
    Tiff,
    Gif,
}

impl ImgFormat {
    pub fn ext(self) -> &'static str {
        match self {
            ImgFormat::Jpg => "jpg",
            ImgFormat::Png => "png",
            ImgFormat::Webp => "webp",
            ImgFormat::Bmp => "bmp",
            ImgFormat::Tiff => "tif",
            ImgFormat::Gif => "gif",
        }
    }
}

fn default_quality() -> u8 {
    90
}
fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageOpts {
    pub format: ImgFormat,
    /// JPG/WebP 품질 1~100
    #[serde(default = "default_quality")]
    pub quality: u8,
    /// 긴 변 최대 픽셀(이보다 크면 줄임)
    #[serde(default)]
    pub max_side: Option<u32>,
    #[serde(default = "yes")]
    pub keep_exif: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageOut {
    pub output: PathBuf,
    pub width: u32,
    pub height: u32,
    /// 출력에 넣은 EXIF 촬영 날짜(없으면 None)
    pub exif_date: Option<String>,
}

pub const RASTER_EXTS: [&str; 9] = ["jpg", "jpeg", "png", "webp", "bmp", "tif", "tiff", "gif", "jfif"];
pub const HEIF_EXTS: [&str; 3] = ["heic", "heif", "hif"];

pub fn is_heif_path(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| HEIF_EXTS.contains(&e.to_ascii_lowercase().as_str())).unwrap_or(false)
}

fn img_err(path: &Path) -> impl FnOnce(image::ImageError) -> Error + '_ {
    move |e| Error::Image { path: path.to_path_buf(), message: e.to_string() }
}

/// 바이트에서 이미지를 풀고 EXIF 회전을 픽셀에 적용한다. (이미지, 회전 값을 지운 EXIF)
pub fn decode(bytes: &[u8], path: &Path) -> Result<(DynamicImage, Option<Vec<u8>>)> {
    let reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(io_err(path))?;
    if reader.format().is_none() {
        return Err(Error::Unsupported(path.display().to_string()));
    }
    let mut decoder = reader.into_decoder().map_err(img_err(path))?;
    let mut exif = decoder.exif_metadata().ok().flatten();
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(img_err(path))?;
    img.apply_orientation(orientation);
    if let Some(e) = exif.as_mut() {
        let _ = Orientation::remove_from_exif_chunk(e);
    }
    Ok((img, exif))
}

fn flatten_alpha(img: &DynamicImage) -> DynamicImage {
    if !img.color().has_alpha() {
        return DynamicImage::ImageRgb8(img.to_rgb8());
    }
    // 투명한 곳은 흰색으로
    let rgba = img.to_rgba8();
    let mut out = image::RgbImage::new(rgba.width(), rgba.height());
    for (o, p) in out.pixels_mut().zip(rgba.pixels()) {
        let a = p[3] as u32;
        for c in 0..3 {
            o[c] = ((p[c] as u32 * a + 255 * (255 - a)) / 255) as u8;
        }
    }
    DynamicImage::ImageRgb8(out)
}

/// 이미지를 형식에 맞게 인코딩한다.
pub fn encode(img: &DynamicImage, opts: &ImageOpts, exif: Option<Vec<u8>>, path: &Path) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let q = opts.quality.clamp(1, 100);
    let exif = exif.filter(|_| opts.keep_exif);
    match opts.format {
        ImgFormat::Jpg => {
            let rgb = flatten_alpha(img);
            let mut enc = JpegEncoder::new_with_quality(&mut buf, q);
            if let Some(e) = exif {
                let _ = enc.set_exif_metadata(e);
            }
            enc.write_image(rgb.as_bytes(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8).map_err(img_err(path))?;
        }
        ImgFormat::Png => {
            let im = if img.color().has_alpha() { DynamicImage::ImageRgba8(img.to_rgba8()) } else { DynamicImage::ImageRgb8(img.to_rgb8()) };
            let mut enc = PngEncoder::new(&mut buf);
            if let Some(e) = exif {
                let _ = enc.set_exif_metadata(e);
            }
            enc.write_image(im.as_bytes(), im.width(), im.height(), im.color().into()).map_err(img_err(path))?;
        }
        ImgFormat::Webp => {
            let rgba = img.to_rgba8();
            let has_alpha = img.color().has_alpha();
            let mem = if has_alpha {
                webp::Encoder::from_rgba(rgba.as_raw(), rgba.width(), rgba.height()).encode(q as f32)
            } else {
                let rgb = img.to_rgb8();
                webp::Encoder::from_rgb(rgb.as_raw(), rgb.width(), rgb.height()).encode(q as f32)
            };
            buf.extend_from_slice(&mem);
        }
        ImgFormat::Bmp | ImgFormat::Tiff | ImgFormat::Gif => {
            let fmt = match opts.format {
                ImgFormat::Bmp => ImageFormat::Bmp,
                ImgFormat::Tiff => ImageFormat::Tiff,
                _ => ImageFormat::Gif,
            };
            let im = if img.color().has_alpha() && fmt != ImageFormat::Bmp {
                DynamicImage::ImageRgba8(img.to_rgba8())
            } else {
                flatten_alpha(img)
            };
            im.write_to(&mut Cursor::new(&mut buf), fmt).map_err(img_err(path))?;
        }
    }
    Ok(buf)
}

fn resize(img: DynamicImage, max_side: Option<u32>) -> DynamicImage {
    match max_side {
        Some(m) if m > 0 && img.width().max(img.height()) > m => img.resize(m, m, image::imageops::FilterType::Lanczos3),
        _ => img,
    }
}

/// 출력 파일의 수정 시각을 원본과 같게(날짜순 정렬이 유지되도록).
pub fn copy_mtime(src: &Path, dst: &Path) {
    if let Ok(t) = std::fs::metadata(src).and_then(|m| m.modified()) {
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open(dst) {
            let _ = f.set_modified(t);
        }
    }
}

fn finish(src: &Path, out_dir: Option<&Path>, img: DynamicImage, exif: Option<Vec<u8>>, opts: &ImageOpts) -> Result<ImageOut> {
    let img = resize(img, opts.max_side);
    let exif_date = if opts.keep_exif && matches!(opts.format, ImgFormat::Jpg | ImgFormat::Png) {
        exif.as_deref().and_then(exif::date_taken)
    } else {
        None
    };
    let bytes = encode(&img, opts, exif, src)?;
    let output = paths::output_path(src, out_dir, "", opts.format.ext());
    paths::write_new(&output, &bytes)?;
    copy_mtime(src, &output);
    Ok(ImageOut { output, width: img.width(), height: img.height(), exif_date })
}

/// 일반 이미지 변환(HEIC 제외).
pub fn convert_raster(src: &Path, out_dir: Option<&Path>, opts: &ImageOpts) -> Result<ImageOut> {
    let bytes = std::fs::read(src).map_err(io_err(src))?;
    let (img, exif) = decode(&bytes, src)?;
    finish(src, out_dir, img, exif, opts)
}

/// HEIC/HEIF → (ffmpeg로 PNG) → 지정 형식. EXIF는 HEIF 안의 Exif 항목에서 옮긴다.
///
/// 아이폰 사진은 대개 512px 타일을 이어 붙인 "grid" 이미지다. ffmpeg 버전에 따라 grid를 스스로
/// 합쳐 주지 않으므로(8.0.1은 첫 타일만 냄), grid 배치·회전(irot/imir)은 직접 읽고 타일만 ffmpeg로 푼 뒤 합친다.
pub fn convert_heif(ffmpeg: &Path, src: &Path, out_dir: Option<&Path>, opts: &ImageOpts, cancel: Option<&Cancel>) -> Result<ImageOut> {
    let bytes = std::fs::read(src).map_err(io_err(src))?;
    let info = heif::HeifInfo::parse(&bytes);
    let mut exif = info.as_ref().and_then(|i| i.exif());
    if let Some(e) = exif.as_mut() {
        // 회전은 픽셀에 적용하므로 EXIF 회전 값은 지운다
        let _ = Orientation::remove_from_exif_chunk(e);
    }
    let grid = info.as_ref().and_then(|i| i.grid());
    let img = match grid {
        Some(g) => decode_grid(ffmpeg, src, &g, cancel)?,
        None => decode_single(ffmpeg, src, cancel)?,
    };
    finish(src, out_dir, img, exif, opts)
}

fn ffmpeg_err(out: &process::RunOutput) -> Error {
    Error::Tool { tool: "ffmpeg".into(), message: format!("HEIC를 풀지 못했습니다: {}", process::last_lines(&out.stderr_tail, 3)) }
}

fn base_args(src: &Path) -> Vec<OsString> {
    ["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"].iter().map(OsString::from).chain([src.into()]).collect()
}

/// 단일 이미지 HEIC: ffmpeg가 irot(표시 행렬)을 적용해 PNG로.
fn decode_single(ffmpeg: &Path, src: &Path, cancel: Option<&Cancel>) -> Result<DynamicImage> {
    let tmp = crate::tempdir::JobDir::new("heic")?;
    let png = tmp.path().join("decoded.png");
    let mut args = base_args(src);
    args.extend(["-frames:v", "1", "-update", "1"].iter().map(OsString::from));
    args.push(png.clone().into());
    let out = process::run(ffmpeg, &args, Duration::from_secs(120), cancel, &mut |_| {})?;
    if !out.success() || !png.is_file() {
        return Err(ffmpeg_err(&out));
    }
    let png_bytes = std::fs::read(&png).map_err(io_err(&png))?;
    Ok(decode(&png_bytes, &png)?.0)
}

/// grid HEIC: 타일마다 PNG(한 번의 ffmpeg 실행) → 이어 붙이기 → 출력 크기로 자르기 → irot/imir.
fn decode_grid(ffmpeg: &Path, src: &Path, g: &heif::Grid, cancel: Option<&Cancel>) -> Result<DynamicImage> {
    let tmp = crate::tempdir::JobDir::new("heicgrid")?;
    let mut args = base_args(src);
    let mut pngs = Vec::new();
    for (k, id) in g.tiles.iter().enumerate() {
        let p = tmp.path().join(format!("t{k}.png"));
        args.extend(["-map".into(), format!("0:i:{id}").into(), "-frames:v".into(), "1".into(), "-update".into(), "1".into()]);
        args.push(p.clone().into());
        pngs.push(p);
    }
    let out = process::run(ffmpeg, &args, Duration::from_secs(300), cancel, &mut |_| {})?;
    if !out.success() || pngs.iter().any(|p| !p.is_file()) {
        return Err(ffmpeg_err(&out));
    }
    let mut canvas: Option<image::RgbaImage> = None;
    let (mut tw, mut th) = (0, 0);
    for (k, p) in pngs.iter().enumerate() {
        let b = std::fs::read(p).map_err(io_err(p))?;
        let tile = decode(&b, p)?.0.to_rgba8();
        if k == 0 {
            tw = tile.width();
            th = tile.height();
            canvas = Some(image::RgbaImage::new(tw * g.cols, th * g.rows));
        }
        let (r, c) = (k as u32 / g.cols, k as u32 % g.cols);
        image::imageops::replace(canvas.as_mut().unwrap(), &tile, (c * tw) as i64, (r * th) as i64);
    }
    let canvas = canvas.ok_or_else(|| Error::Invalid("HEIC 타일이 없습니다".into()))?;
    let w = g.out_w.min(canvas.width());
    let h = g.out_h.min(canvas.height());
    let mut img = DynamicImage::ImageRgba8(image::imageops::crop_imm(&canvas, 0, 0, w, h).to_image());
    for t in &g.transforms {
        img = match *t {
            heif::Transform::Rotate(1) => img.rotate270(),
            heif::Transform::Rotate(2) => img.rotate180(),
            heif::Transform::Rotate(3) => img.rotate90(),
            heif::Transform::Rotate(_) => img,
            heif::Transform::Mirror(0) => img.fliph(),
            heif::Transform::Mirror(_) => img.flipv(),
        };
    }
    // 불투명이면 RGB로(JPG 등에서 불필요한 변환 줄이기)
    Ok(DynamicImage::ImageRgb8(img.to_rgb8()))
}

/// 이미지 파일인지(확장자)
pub fn is_raster_path(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| RASTER_EXTS.contains(&e.to_ascii_lowercase().as_str())).unwrap_or(false)
}

/// 실제 내용이 HEIF인지(확장자가 잘못 붙은 경우 대비)
pub fn sniff_heif(p: &Path) -> bool {
    use std::io::Read;
    let mut b = [0u8; 12];
    std::fs::File::open(p).and_then(|mut f| f.read_exact(&mut b)).is_ok() && heif::is_heif(&b)
}
