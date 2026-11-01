//! PDF → 이미지.
//!
//! - 쪽 렌더링: PDFium(BSD-3-Clause, `pdfium-render`로 실행 중에 불러옴). `pdfium.dll`은 앱 exe 옆에 둔다
//!   (설치 파일·휴대용 묶음 모두 같은 위치). 없으면 한국어 안내로 끝낸다.
//! - 들어 있는 이미지만 꺼내기: lopdf로 이미지 객체를 그대로 저장(JPEG는 다시 인코딩하지 않음, Flate RGB/흑백은 PNG).
//!   PDFium이 필요 없다.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use image::{DynamicImage, ImageEncoder};
use lopdf::Object;
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};

use crate::error::{io_err, Error, Result};
use crate::pages::parse_list;
use crate::paths;
use crate::process::Cancel;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageImageFormat {
    Png,
    Jpg,
}

impl PageImageFormat {
    pub fn ext(self) -> &'static str {
        match self {
            PageImageFormat::Png => "png",
            PageImageFormat::Jpg => "jpg",
        }
    }
}

fn default_q() -> u8 {
    90
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderOpts {
    pub format: PageImageFormat,
    pub dpi: u32,
    #[serde(default = "default_q")]
    pub quality: u8,
    /// "all"/빈 값이면 전체, 아니면 `1-3,5`
    #[serde(default)]
    pub pages: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderedPage {
    pub page: u32,
    pub output: PathBuf,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderOut {
    pub input: PathBuf,
    pub folder: PathBuf,
    pub pages: Vec<RenderedPage>,
    pub note: String,
}

pub const PDFIUM_HINT: &str = "PDF를 그림으로 바꾸려면 pdfium.dll이 앱(win-filetool.exe)과 같은 폴더에 있어야 합니다. 설치 파일로 설치했거나 휴대용 묶음(zip)을 통째로 풀었다면 들어 있습니다";

/// pdfium 라이브러리 파일 이름(pdfium.dll / libpdfium.so)
pub fn library_name() -> std::ffi::OsString {
    Pdfium::pdfium_platform_library_name()
}

/// 찾는 순서: 환경 변수 `WIN_FILETOOL_PDFIUM`(파일 또는 폴더) → exe 옆 → exe 옆 `pdfium\` → exe 옆 `tools\pdfium\(bin)`
pub fn find_library() -> Option<PathBuf> {
    let name = library_name();
    let mut cands = Vec::new();
    if let Some(v) = std::env::var_os("WIN_FILETOOL_PDFIUM").map(PathBuf::from) {
        if v.is_file() {
            cands.push(v.clone());
        }
        cands.push(v.join(&name));
        cands.push(v.join("lib").join(&name));
        cands.push(v.join("bin").join(&name));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        cands.push(dir.join(&name));
        cands.push(dir.join("pdfium").join(&name));
        cands.push(dir.join("tools").join("pdfium").join(&name));
        cands.push(dir.join("tools").join("pdfium").join("bin").join(&name));
    }
    cands.into_iter().find(|p| p.is_file())
}

static PDFIUM: Mutex<Option<&'static Pdfium>> = Mutex::new(None);

pub(crate) fn pdfium() -> Result<&'static Pdfium> {
    let mut g = PDFIUM.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = *g {
        return Ok(p);
    }
    let lib = find_library().ok_or_else(|| Error::ToolMissing { tool: "PDFium(pdfium.dll)".into(), hint: PDFIUM_HINT.into() })?;
    let bindings = Pdfium::bind_to_library(&lib)
        .map_err(|e| Error::Tool { tool: "PDFium".into(), message: format!("{}을(를) 불러오지 못했습니다: {e}", lib.display()) })?;
    let p: &'static Pdfium = Box::leak(Box::new(Pdfium::new(bindings)));
    *g = Some(p);
    Ok(p)
}

/// PDFium을 쓸 수 있는지(설정 화면·selftest용): 찾은 경로
pub fn pdfium_status() -> std::result::Result<PathBuf, String> {
    let lib = find_library().ok_or_else(|| PDFIUM_HINT.to_string())?;
    pdfium().map(|_| lib).map_err(|e| e.to_string())
}

pub(crate) fn render_err(path: &Path, e: PdfiumError) -> Error {
    let msg = match &e {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => {
            "암호가 걸린 PDF는 열 수 없습니다. PDF 프로그램에서 암호를 풀어 저장한 뒤 다시 시도하세요".to_string()
        }
        _ => format!("PDF를 그리지 못했습니다: {e}"),
    };
    Error::Pdf { path: path.into(), message: msg }
}

fn encode_page(img: &DynamicImage, fmt: PageImageFormat, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let rgb = img.to_rgb8();
    let r = match fmt {
        PageImageFormat::Png => {
            image::codecs::png::PngEncoder::new(&mut buf).write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
        }
        PageImageFormat::Jpg => image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100))
            .write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8),
    };
    r.map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(buf)
}

/// 결과 폴더: 원본 옆(또는 지정 폴더)에 `이름_쪽이미지`(있으면 `(1)`)
pub fn output_folder(input: &Path, out_dir: Option<&Path>, suffix: &str) -> PathBuf {
    let dir = out_dir.map(Path::to_path_buf).or_else(|| input.parent().map(Path::to_path_buf)).unwrap_or_default();
    let stem = input.file_stem().map(|s| crate::nfc::to_nfc(&s.to_string_lossy())).unwrap_or_else(|| "pdf".into());
    paths::unique_path(&dir.join(format!("{stem}{suffix}")))
}

/// 쪽마다 그림 파일로. DPI 기준 크기 = 쪽 크기(pt) / 72 × DPI (쪽의 /Rotate 반영).
pub fn render_pages(input: &Path, out_dir: Option<&Path>, opts: &RenderOpts, cancel: Option<&Cancel>, progress: &mut dyn FnMut(u32, u32)) -> Result<RenderOut> {
    if !(10..=1200).contains(&opts.dpi) {
        return Err(Error::Invalid(format!("DPI는 10~1200 사이로 정하세요 ({})", opts.dpi)));
    }
    let p = pdfium()?;
    let doc = p.load_pdf_from_file(input, None).map_err(|e| render_err(input, e))?;
    let n = doc.pages().len() as u32;
    if n == 0 {
        return Err(Error::Pdf { path: input.into(), message: "페이지가 없습니다".into() });
    }
    let list: Vec<u32> = match opts.pages.as_deref().map(str::trim) {
        None | Some("") | Some("all") => (1..=n).collect(),
        Some(spec) => parse_list(spec, n)?,
    };
    let folder = output_folder(input, out_dir, "_쪽이미지");
    std::fs::create_dir_all(&folder).map_err(io_err(&folder))?;
    let stem = input.file_stem().map(|s| crate::nfc::to_nfc(&s.to_string_lossy())).unwrap_or_else(|| "pdf".into());
    let width = n.to_string().len().max(2);
    let scale = opts.dpi as f32 / 72.0;
    let mut out = Vec::new();
    let result = (|| -> Result<()> {
        for (k, &pno) in list.iter().enumerate() {
            if let Some(c) = cancel {
                c.check()?;
            }
            let page = doc.pages().get((pno - 1) as PdfPageIndex).map_err(|e| render_err(input, e))?;
            let cfg = PdfRenderConfig::new().scale_page_by_factor(scale).render_form_data(true).set_clear_color(PdfColor::WHITE);
            let bmp = page.render_with_config(&cfg).map_err(|e| render_err(input, e))?;
            let img = bmp.as_image().map_err(|e| render_err(input, e))?;
            let bytes = encode_page(&img, opts.format, opts.quality)?;
            let path = paths::unique_path(&folder.join(format!("{stem}_p{pno:0width$}.{}", opts.format.ext())));
            paths::write_new(&path, &bytes)?;
            out.push(RenderedPage { page: pno, output: path, width: img.width(), height: img.height() });
            progress(k as u32 + 1, list.len() as u32);
        }
        Ok(())
    })();
    if let Err(e) = result {
        // 실패·취소: 만든 폴더를 지운다
        let _ = std::fs::remove_dir_all(&folder);
        return Err(e);
    }
    let note = match out.first() {
        Some(f) => format!("{}쪽 → {} {}장, {} DPI (첫 장 {}×{})", n, opts.format.ext().to_uppercase(), out.len(), opts.dpi, f.width, f.height),
        None => "만든 그림이 없습니다".into(),
    };
    Ok(RenderOut { input: input.into(), folder, pages: out, note })
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtractOut {
    pub input: PathBuf,
    pub folder: Option<PathBuf>,
    pub files: Vec<PathBuf>,
    pub skipped: usize,
    pub note: String,
}

/// PDF에 들어 있는 이미지를 그대로 꺼낸다(JPEG는 바이트 그대로 .jpg, 8비트 Flate RGB/흑백은 .png).
/// 마스크·CMYK·JPX 등은 건너뛰고 개수만 알린다.
pub fn extract_images(input: &Path, out_dir: Option<&Path>) -> Result<ExtractOut> {
    let doc = crate::pdfcompress::load_unencrypted(input)?;
    let mut masks = HashSet::new();
    for o in doc.objects.values() {
        if let Ok(s) = o.as_stream() {
            if let Ok(Object::Reference(id)) = s.dict.get(b"SMask") {
                masks.insert(*id);
            }
        }
    }
    // 쪽 순서대로(같은 이미지는 한 번만)
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    for (_, pid) in doc.get_pages() {
        let (own, inh) = doc.get_page_resources(pid).unwrap_or((None, vec![]));
        let mut dicts: Vec<&lopdf::Dictionary> = own.into_iter().collect();
        dicts.extend(inh.iter().filter_map(|id| doc.get_dictionary(*id).ok()));
        for d in dicts {
            let xo = match d.get(b"XObject") {
                Ok(Object::Dictionary(x)) => Some(x),
                Ok(Object::Reference(r)) => doc.get_dictionary(*r).ok(),
                _ => None,
            };
            if let Some(xo) = xo {
                for (_, v) in xo.iter() {
                    if let Object::Reference(id) = v {
                        if seen.insert(*id) {
                            order.push(*id);
                        }
                    }
                }
            }
        }
    }
    let stem = input.file_stem().map(|s| crate::nfc::to_nfc(&s.to_string_lossy())).unwrap_or_else(|| "pdf".into());
    let mut files = Vec::new();
    let mut skipped = 0;
    let mut folder: Option<PathBuf> = None;
    for id in order {
        let Ok(s) = doc.get_object(id).and_then(Object::as_stream) else { continue };
        if s.dict.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Image".as_slice()) {
            continue;
        }
        if masks.contains(&id) || s.dict.get(b"ImageMask").and_then(Object::as_bool).unwrap_or(false) {
            continue;
        }
        let w = s.dict.get(b"Width").and_then(Object::as_i64).unwrap_or(0) as u32;
        let h = s.dict.get(b"Height").and_then(Object::as_i64).unwrap_or(0) as u32;
        let bpc = s.dict.get(b"BitsPerComponent").and_then(Object::as_i64).unwrap_or(0);
        let filter = match s.dict.get(b"Filter") {
            Ok(Object::Name(n)) => Some(n.clone()),
            Ok(Object::Array(a)) if a.len() == 1 => a[0].as_name().ok().map(<[u8]>::to_vec),
            Err(_) => None,
            _ => Some(b"?".to_vec()),
        };
        let cs = match s.dict.get(b"ColorSpace") {
            Ok(Object::Name(n)) => n.clone(),
            Ok(Object::Array(a)) if a.first().and_then(|o| o.as_name().ok()) == Some(b"ICCBased") => {
                let n = a.get(1).and_then(|o| o.as_reference().ok()).and_then(|r| doc.get_object(r).ok()).and_then(|o| o.as_stream().ok())
                    .and_then(|st| st.dict.get(b"N").and_then(Object::as_i64).ok()).unwrap_or(0);
                match n { 1 => b"DeviceGray".to_vec(), 3 => b"DeviceRGB".to_vec(), _ => b"?".to_vec() }
            }
            _ => b"?".to_vec(),
        };
        let (bytes, ext): (Vec<u8>, &str) = match (filter.as_deref(), cs.as_slice()) {
            (Some(b"DCTDecode"), b"DeviceRGB" | b"DeviceGray") => (s.content.clone(), "jpg"),
            (None | Some(b"FlateDecode"), b"DeviceRGB" | b"DeviceGray") if bpc == 8 && w > 0 && h > 0 => {
                let Ok(raw) = s.decompressed_content() else { skipped += 1; continue };
                let n = if cs == b"DeviceRGB" { 3 } else { 1 };
                let need = w as usize * h as usize * n;
                if raw.len() < need {
                    skipped += 1;
                    continue;
                }
                let ct = if n == 3 { image::ExtendedColorType::Rgb8 } else { image::ExtendedColorType::L8 };
                let mut buf = Vec::new();
                if image::codecs::png::PngEncoder::new(&mut buf).write_image(&raw[..need], w, h, ct).is_err() {
                    skipped += 1;
                    continue;
                }
                (buf, "png")
            }
            _ => {
                skipped += 1;
                continue;
            }
        };
        let dir = match &folder {
            Some(d) => d.clone(),
            None => {
                let d = output_folder(input, out_dir, "_추출이미지");
                std::fs::create_dir_all(&d).map_err(io_err(&d))?;
                folder = Some(d.clone());
                d
            }
        };
        let path = paths::unique_path(&dir.join(format!("{stem}_img{:03}.{ext}", files.len() + 1)));
        paths::write_new(&path, &bytes)?;
        files.push(path);
    }
    let note = if files.is_empty() {
        format!("꺼낼 수 있는 이미지가 없습니다{}", if skipped > 0 { format!(" (지원하지 않는 형식 {skipped}개)") } else { String::new() })
    } else {
        format!("이미지 {}개 꺼냄{}", files.len(), if skipped > 0 { format!(", 지원하지 않는 형식 {skipped}개 건너뜀") } else { String::new() })
    };
    Ok(ExtractOut { input: input.into(), folder, files, skipped, note })
}
