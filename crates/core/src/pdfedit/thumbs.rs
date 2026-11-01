//! 쪽 미리보기(쪽 편집 화면의 격자): 쪽 크기·회전 정보(lopdf, 빠름)와 썸네일 PNG(PDFium).

use std::path::Path;
use std::time::Instant;

use image::ImageEncoder;
use pdfium_render::prelude::*;
use serde::Serialize;

use super::{open, page_geom, page_ids};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize)]
pub struct PageInfo {
    /// 1부터
    pub page: u32,
    /// 보이는 상자(CropBox/MediaBox) 크기, pt
    pub width: f32,
    pub height: f32,
    pub rotate: i64,
    /// 회전을 반영한 보이는 크기, pt
    pub visual_width: f32,
    pub visual_height: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocPages {
    pub pages: Vec<PageInfo>,
    /// 원본에 암호가 걸려 있었는지(암호로 열었음)
    pub encrypted: bool,
}

pub fn page_count(path: &Path, password: Option<&str>) -> Result<u32> {
    Ok(open(path, password)?.get_pages().len() as u32)
}

pub fn page_infos(path: &Path, password: Option<&str>) -> Result<DocPages> {
    let doc = open(path, password)?;
    let pages = page_ids(&doc)
        .into_iter()
        .enumerate()
        .map(|(i, id)| {
            let g = page_geom(&doc, id);
            let (vw, vh) = g.visual_size();
            PageInfo { page: i as u32 + 1, width: g.w(), height: g.h(), rotate: g.rotate, visual_width: vw, visual_height: vh }
        })
        .collect();
    Ok(DocPages { pages, encrypted: doc.was_encrypted() })
}

#[derive(Debug, Clone, Serialize)]
pub struct Thumb {
    pub page: u32,
    pub width: u32,
    pub height: u32,
    /// PNG 바이트(Tauri에서는 data URL로 바꿔 보냄)
    #[serde(skip)]
    pub png: Vec<u8>,
    pub ms: f64,
}

pub(crate) fn pdfium_open<'a>(p: &'a Pdfium, path: &Path, password: Option<&str>) -> Result<PdfDocument<'a>> {
    let pw = password.filter(|s| !s.is_empty());
    p.load_pdf_from_file(path, pw).map_err(|e| match e {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            if pw.is_some() {
                Error::WrongPassword(path.to_path_buf())
            } else {
                Error::PasswordRequired(path.to_path_buf())
            }
        }
        e => crate::pdfrender::render_err(path, e),
    })
}

/// 쪽들(1부터)을 가로 `width_px`로 그려 PNG로. 쪽마다 `each`를 부른다(하나씩 화면에 보내기용).
pub fn thumbnails(path: &Path, password: Option<&str>, pages: &[u32], width_px: u32, each: &mut dyn FnMut(&Thumb) -> Result<()>) -> Result<Vec<Thumb>> {
    if !(16..=2000).contains(&width_px) {
        return Err(Error::Invalid("미리보기 너비는 16~2000px 사이로 정하세요".into()));
    }
    let p = crate::pdfrender::pdfium()?;
    let doc = pdfium_open(p, path, password)?;
    let n = doc.pages().len() as u32;
    let mut out = Vec::with_capacity(pages.len());
    for &pno in pages {
        if pno == 0 || pno > n {
            return Err(Error::PageSpec(format!("{pno}쪽은 범위를 벗어났습니다 (1~{n}쪽)")));
        }
        let t0 = Instant::now();
        let page = doc.pages().get((pno - 1) as PdfPageIndex).map_err(|e| crate::pdfrender::render_err(path, e))?;
        let cfg = PdfRenderConfig::new().set_target_width(width_px as i32).render_form_data(true).set_clear_color(PdfColor::WHITE);
        let bmp = page.render_with_config(&cfg).map_err(|e| crate::pdfrender::render_err(path, e))?;
        let img = bmp.as_image().map_err(|e| crate::pdfrender::render_err(path, e))?.to_rgb8();
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new_with_quality(&mut png, image::codecs::png::CompressionType::Fast, image::codecs::png::FilterType::Sub)
            .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        let t = Thumb { page: pno, width: img.width(), height: img.height(), png, ms: t0.elapsed().as_secs_f64() * 1000.0 };
        each(&t)?;
        out.push(t);
    }
    Ok(out)
}

/// 쪽 하나를 RGB 그림으로(자동 여백 찾기·테스트용). `scale` = px/pt.
pub fn render_rgb(path: &Path, password: Option<&str>, page: u32, scale: f32) -> Result<image::RgbImage> {
    let p = crate::pdfrender::pdfium()?;
    let doc = pdfium_open(p, path, password)?;
    let n = doc.pages().len() as u32;
    if page == 0 || page > n {
        return Err(Error::PageSpec(format!("{page}쪽은 범위를 벗어났습니다 (1~{n}쪽)")));
    }
    let pg = doc.pages().get((page - 1) as PdfPageIndex).map_err(|e| crate::pdfrender::render_err(path, e))?;
    let cfg = PdfRenderConfig::new().scale_page_by_factor(scale).render_form_data(true).set_clear_color(PdfColor::WHITE);
    let bmp = pg.render_with_config(&cfg).map_err(|e| crate::pdfrender::render_err(path, e))?;
    Ok(bmp.as_image().map_err(|e| crate::pdfrender::render_err(path, e))?.to_rgb8())
}

/// 모든 쪽을 한 번씩 그려 걸린 시간(ms/쪽) — 점검용
pub fn bench(path: &Path, password: Option<&str>, width_px: u32) -> Result<(u32, f64, f64)> {
    let n = page_count(path, password).or_else(|e| match e {
        // lopdf가 못 여는 PDF도 PDFium으로는 잴 수 있게
        Error::Pdf { .. } => Ok(0),
        e => Err(e),
    })?;
    let p = crate::pdfrender::pdfium()?;
    let t_open = Instant::now();
    let doc = pdfium_open(p, path, password)?;
    let open_ms = t_open.elapsed().as_secs_f64() * 1000.0;
    let total = if n == 0 { doc.pages().len() as u32 } else { n };
    drop(doc);
    let pages: Vec<u32> = (1..=total).collect();
    let t = Instant::now();
    thumbnails(path, password, &pages, width_px, &mut |_| Ok(()))?;
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    Ok((total, ms / total.max(1) as f64, open_ms))
}

/// 쪽의 글자(PDFium 글자 추출) — 넣은 한글이 복사되는지 확인용
pub fn page_text(path: &Path, password: Option<&str>, page: u32) -> Result<String> {
    let p = crate::pdfrender::pdfium()?;
    let doc = pdfium_open(p, path, password)?;
    let pg = doc.pages().get((page.max(1) - 1) as PdfPageIndex).map_err(|e| crate::pdfrender::render_err(path, e))?;
    let t = pg.text().map_err(|e| crate::pdfrender::render_err(path, e))?;
    Ok(t.all())
}

impl Thumb {
    /// `data:image/png;base64,…` (화면의 `<img src>`에 바로)
    pub fn data_url(&self) -> String {
        format!("data:image/png;base64,{}", base64(&self.png))
    }
}

fn base64(b: &[u8]) -> String {
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

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foobar"), "Zm9vYmFy");
    }
}
