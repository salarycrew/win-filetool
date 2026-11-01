//! 스캔 PDF·그림에 글자 인식 쓰기.
//!
//! - [`make_searchable`]: 쪽 모양은 그대로 두고 보이지 않는 글자 층을 얹어 `이름_글자인식.pdf`(검색·선택·복사)
//! - [`file_to_txt`]: JPG/PNG/HEIC/스캔 PDF → `이름_글자.txt`(UTF-8 BOM, CRLF). 글자가 있는 PDF 쪽은 그 글자를 그대로 쓴다
//! - [`layout_lines`]: PDF → Word "글자 인식해서 넣기"용 줄(pdflayout `Line`)

use std::path::{Path, PathBuf};
use std::time::Instant;

use image::DynamicImage;
use lopdf::Object;
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};

use super::textlayer::{self, Placed};
use super::{language_hint, recognize, Recognized, Recognizer};
use crate::error::{io_err, Error, Result};
use crate::pdfedit::{self, add_content, add_resource, mat_apply, mat_invert, page_geom, Job, PageGeom};
use crate::pdflayout::{Cell, Line, Span};
use crate::process::Cancel;

fn d300() -> u32 {
    300
}

/// 검색 가능하게 만들기 옵션
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OcrPdfOpts {
    /// 인식용으로 쪽을 그리는 해상도(150~400, 기본 300)
    #[serde(default = "d300")]
    pub dpi: u32,
    /// 이미 글자가 있는 쪽도 다시 인식(기본 끔). 켜면 원래 글자와 겹치는 낱말은 빼고 그림 속 글자만 더한다
    #[serde(default)]
    pub redo_text_pages: bool,
    /// 쪽 선택(`1-3,5`, 없으면 전체)
    #[serde(default)]
    pub pages: Option<String>,
}

impl Default for OcrPdfOpts {
    fn default() -> Self {
        OcrPdfOpts { dpi: 300, redo_text_pages: false, pages: None }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OcrPdfOut {
    pub output: PathBuf,
    pub pages: u32,
    /// 글자 층을 얹은 쪽
    pub ocr_pages: u32,
    /// 이미 글자가 있어 건너뛴 쪽
    pub skipped_text_pages: u32,
    /// 인식했지만 글자가 없던 쪽
    pub empty_pages: u32,
    /// 누워 있어 돌려 읽은 쪽
    pub rotated_pages: u32,
    pub words: usize,
    pub chars: usize,
    pub language: String,
    pub millis: u64,
    /// 인식한 쪽 하나에 걸린 시간(ms, 그리기 포함)
    pub ms_per_page: u64,
    pub note: String,
    /// 한국어 인식이 없을 때 안내
    pub hint: Option<String>,
}

pub(crate) fn clamp_dpi(dpi: u32) -> u32 {
    dpi.clamp(100, 400)
}

/// 쪽의 글자 수(공백 제외, 보이지 않는 글자 포함)
pub(crate) fn text_chars(page: &PdfPage) -> usize {
    page.text().map(|t| t.chars().iter().filter(|c| c.unicode_char().is_some_and(|c| !c.is_whitespace() && c != '\u{FFFE}' && c != '\u{2}')).count()).unwrap_or(0)
}

/// 쪽을 그림으로(보이는 방향, 흰 바탕)
pub(crate) fn render(page: &PdfPage, dpi: u32, path: &Path) -> Result<DynamicImage> {
    let cfg = PdfRenderConfig::new().scale_page_by_factor(dpi as f32 / 72.0).render_form_data(true).set_clear_color(PdfColor::WHITE);
    let bmp = page.render_with_config(&cfg).map_err(|e| crate::pdfrender::render_err(path, e))?;
    Ok(DynamicImage::ImageRgb8(bmp.as_image().map_err(|e| crate::pdfrender::render_err(path, e))?.to_rgb8()))
}

/// 사용자 공간 상자 → 보이는 쪽(왼쪽 위 원점) [x0, y0, x1, y1]
pub(crate) fn user_to_visual(g: &PageGeom, r: [f32; 4]) -> [f32; 4] {
    let inv = mat_invert(g.visual_to_user());
    let (_, vh) = g.visual_size();
    let pts = [mat_apply(inv, r[0], r[1]), mat_apply(inv, r[2], r[1]), mat_apply(inv, r[0], r[3]), mat_apply(inv, r[2], r[3])];
    let x0 = pts.iter().map(|p| p.0).fold(f32::MAX, f32::min);
    let x1 = pts.iter().map(|p| p.0).fold(f32::MIN, f32::max);
    let y0 = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min);
    let y1 = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max);
    [x0, vh - y1, x1, vh - y0]
}

/// 쪽에 이미 있는 글자 상자(보이는 쪽 pt, 왼쪽 위 원점)
pub(crate) fn char_boxes(page: &PdfPage, g: &PageGeom) -> Vec<[f32; 4]> {
    let Ok(t) = page.text() else { return vec![] };
    t.chars()
        .iter()
        .filter(|c| c.unicode_char().is_some_and(|c| !c.is_whitespace()))
        .filter_map(|c| c.loose_bounds().ok())
        .map(|b| user_to_visual(g, [b.left().value, b.bottom().value, b.right().value, b.top().value]))
        .collect()
}

fn overlap(a: [f32; 4], b: [f32; 4]) -> f32 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    w * h
}

/// 인식 결과(그림 px) → 쪽 위 낱말(pt). `skip`: 이 상자들과 많이 겹치는 낱말은 뺀다.
pub(crate) fn placed_words(rec: &Recognized, vw: f32, vh: f32, skip: &[[f32; 4]], pad_skip: bool) -> Vec<Placed> {
    let kx = vw / rec.width.max(1) as f32;
    let ky = vh / rec.height.max(1) as f32;
    let mut out = Vec::new();
    for l in &rec.lines {
        let ws: Vec<Placed> = l
            .words
            .iter()
            .map(|w| Placed { text: w.text.clone(), x: w.x * kx, y: w.y * ky, w: w.w * kx, h: w.h * ky, rot: l.rot })
            .filter(|p| {
                let r = [p.x, p.y, p.x + p.w, p.y + p.h];
                let area = (p.w * p.h).max(1e-3);
                let covered: f32 = skip
                    .iter()
                    .map(|s| {
                        let s = if pad_skip { [s[0] - 1.0, s[1] - 1.0, s[2] + 1.0, s[3] + 1.0] } else { *s };
                        overlap(r, s)
                    })
                    .sum();
                if pad_skip {
                    covered <= 0.0
                } else {
                    covered / area < 0.4
                }
            })
            .collect();
        if !ws.is_empty() {
            out.extend(textlayer::line_words(&ws));
        }
    }
    out
}

/// 쪽 하나에 글자 층 얹기. 반환: 얹은 낱말 수
pub(crate) fn add_layer(doc: &mut lopdf::Document, pid: lopdf::ObjectId, font: &mut Option<lopdf::ObjectId>, g: &PageGeom, words: &[Placed]) -> Result<usize> {
    if words.is_empty() {
        return Ok(0);
    }
    let fid = *font.get_or_insert_with(|| textlayer::add_font(doc));
    let name = add_resource(doc, pid, b"Font", "OCR", Object::Reference(fid))?;
    let ops = textlayer::ops(&name, g, words);
    add_content(doc, pid, ops.as_bytes(), true)?;
    Ok(words.len())
}

/// 스캔 PDF → 같은 모양 + 보이지 않는 글자 층(`output`). 이미 글자가 있는 쪽은 건너뛴다(옵션으로 다시 인식).
pub fn make_searchable(input: &Path, password: Option<&str>, opts: &OcrPdfOpts, output: &Path, rec: &dyn Recognizer, job: &mut Job) -> Result<OcrPdfOut> {
    let t0 = Instant::now();
    let mut doc = pdfedit::open(input, password)?;
    let ids = doc.get_pages().into_values().collect::<Vec<_>>();
    let n = ids.len() as u32;
    let sel = pdfedit::select_pages(opts.pages.as_deref(), n)?;
    let p = crate::pdfrender::pdfium()?;
    let pdf = crate::pdfedit::thumbs::pdfium_open(p, input, password)?;
    let dpi = clamp_dpi(opts.dpi);
    let mut font = None;
    let (mut ocr_pages, mut skipped, mut empty, mut rotated, mut words, mut chars) = (0u32, 0u32, 0u32, 0u32, 0usize, 0usize);
    let mut ocr_ms = 0u64;
    let total = sel.len() as u32;
    for (k, &i) in sel.iter().enumerate() {
        job.check()?;
        let page = pdf.pages().get(i as PdfPageIndex).map_err(|e| crate::pdfrender::render_err(input, e))?;
        let have = text_chars(&page);
        if have >= 10 && !opts.redo_text_pages {
            skipped += 1;
            job.tick(k as u32 + 1, total)?;
            continue;
        }
        let tp = Instant::now();
        let pid = ids[i];
        let g = page_geom(&doc, pid);
        let (vw, vh) = g.visual_size();
        let img = render(&page, dpi, input)?;
        let r = recognize(rec, &img, job.cancel)?;
        drop(img);
        let skip = if have > 0 { char_boxes(&page, &g) } else { vec![] };
        let placed = placed_words(&r, vw, vh, &skip, false);
        if r.rot != 0 {
            rotated += 1;
        }
        let added = add_layer(&mut doc, pid, &mut font, &g, &placed)?;
        if added == 0 {
            empty += 1;
        } else {
            ocr_pages += 1;
            words += added;
            chars += placed.iter().map(|p| p.text.chars().filter(|c| !c.is_whitespace()).count()).sum::<usize>();
        }
        ocr_ms += tp.elapsed().as_millis() as u64;
        job.tick(k as u32 + 1, total)?;
    }
    drop(pdf);
    let lang = rec.language();
    let hint = language_hint(&lang);
    if ocr_pages == 0 {
        if skipped == total {
            return Err(Error::Invalid("모든 쪽에 이미 글자가 있어서 새로 만들 것이 없어요. 쪽 안 그림 속 글자도 읽으려면 [이미 글자가 있는 쪽도 다시 인식]을 켜 주세요".into()));
        }
        return Err(Error::Invalid(format!("글자를 찾지 못했어요({}쪽 인식). 글자가 너무 작거나 흐리면 더 선명하게 스캔해 주세요{}", total - skipped, hint.as_ref().map(|h| format!(" · {h}")).unwrap_or_default())));
    }
    job.check()?;
    pdfedit::save(doc, output)?;
    let done = (total - skipped).max(1) as u64;
    let mut note = format!("{ocr_pages}쪽에 글자 층을 넣었어요(낱말 {words}개)");
    if skipped > 0 {
        note.push_str(&format!(" · 이미 글자가 있는 {skipped}쪽은 그대로"));
    }
    if empty > 0 {
        note.push_str(&format!(" · 글자를 못 찾은 쪽 {empty}"));
    }
    if rotated > 0 {
        note.push_str(&format!(" · 누운 쪽 {rotated}개는 돌려서 읽었어요"));
    }
    Ok(OcrPdfOut {
        output: output.to_path_buf(),
        pages: n,
        ocr_pages,
        skipped_text_pages: skipped,
        empty_pages: empty,
        rotated_pages: rotated,
        words,
        chars,
        language: lang,
        millis: t0.elapsed().as_millis() as u64,
        ms_per_page: ocr_ms / done,
        note,
        hint,
    })
}

// ---------- 그림·PDF → 글자(TXT) ----------

#[derive(Debug, Clone, Serialize)]
pub struct TextOut {
    pub input: PathBuf,
    pub output: PathBuf,
    pub pages: u32,
    /// 글자 인식으로 읽은 쪽(그림이면 1)
    pub ocr_pages: u32,
    /// 공백을 뺀 글자 수
    pub chars: usize,
    pub language: String,
    pub millis: u64,
    pub note: String,
    /// 화면에서 복사할 글(앞 20만 자까지)
    pub text: String,
    pub hint: Option<String>,
}

const OCR_IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "bmp", "tif", "tiff", "gif", "webp", "heic", "heif"];

/// 글자 뽑기를 할 수 있는 파일인지(확장자)
pub fn is_ocr_input(p: &Path) -> bool {
    let e = p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default();
    e == "pdf" || OCR_IMAGE_EXTS.contains(&e.as_str())
}

/// 그림 파일 읽기(HEIC는 ffmpeg)
pub fn load_image(input: &Path, ffmpeg: Option<&Path>, cancel: Option<&Cancel>) -> Result<DynamicImage> {
    crate::error::ensure_nonempty(input)?;
    if crate::imageconv::is_heif_path(input) || crate::imageconv::sniff_heif(input) {
        let ff = ffmpeg.ok_or_else(|| Error::ToolMissing { tool: "ffmpeg".into(), hint: "HEIC 사진을 읽으려면 앱에 든 영상 도구가 필요해요".into() })?;
        return Ok(crate::imageconv::load_heif(ff, input, cancel)?.0);
    }
    let bytes = std::fs::read(input).map_err(io_err(input))?;
    Ok(crate::imageconv::decode(&bytes, input)?.0)
}

fn write_txt(input: &Path, out_dir: Option<&Path>, text: &str) -> Result<PathBuf> {
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(text.replace("\r\n", "\n").replace('\n', "\r\n").as_bytes());
    let output = crate::paths::output_path(input, out_dir, "_글자", "txt");
    crate::paths::write_new(&output, &bytes)?;
    Ok(output)
}

fn clip(s: &str) -> String {
    s.chars().take(200_000).collect()
}

/// JPG/PNG/HEIC/…/PDF → `이름_글자.txt`
pub fn file_to_txt(input: &Path, password: Option<&str>, ffmpeg: Option<&Path>, out_dir: Option<&Path>, rec: &dyn Recognizer, job: &mut Job) -> Result<TextOut> {
    let is_pdf = input.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if is_pdf {
        return pdf_to_txt(input, password, out_dir, rec, job);
    }
    let t0 = Instant::now();
    if !OCR_IMAGE_EXTS.iter().any(|e| input.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case(e))) {
        return Err(Error::Unsupported(format!("{} (JPG·PNG·HEIC 같은 사진이나 PDF를 넣어 주세요)", input.display())));
    }
    job.tick(0, 1)?;
    let img = load_image(input, ffmpeg, job.cancel)?;
    let r = recognize(rec, &img, job.cancel)?;
    let lang = rec.language();
    let hint = language_hint(&lang);
    let chars = r.chars();
    if chars == 0 {
        return Err(Error::Invalid(format!("그림에서 글자를 찾지 못했어요. 글자가 잘 보이게 다시 찍어 주세요{}", hint.as_ref().map(|h| format!(" · {h}")).unwrap_or_default())));
    }
    job.check()?;
    let output = write_txt(input, out_dir, &r.text)?;
    job.tick(1, 1)?;
    let mut note = format!("글자 {chars}자를 읽었어요");
    if r.rot != 0 {
        note.push_str(" · 누운 사진이라 돌려서 읽었어요");
    }
    Ok(TextOut { input: input.into(), output, pages: 1, ocr_pages: 1, chars, language: lang, millis: t0.elapsed().as_millis() as u64, note, text: clip(&r.text), hint })
}

/// PDF → 글자(글자가 있는 쪽은 그대로, 스캔 쪽은 인식). 쪽 사이 "— N쪽 —"(PDF → TXT와 같음)
pub fn pdf_to_txt(input: &Path, password: Option<&str>, out_dir: Option<&Path>, rec: &dyn Recognizer, job: &mut Job) -> Result<TextOut> {
    let t0 = Instant::now();
    crate::error::ensure_nonempty(input)?;
    let p = crate::pdfrender::pdfium()?;
    let pdf = crate::pdfedit::thumbs::pdfium_open(p, input, password)?;
    let n = pdf.pages().len() as u32;
    let mut parts = Vec::new();
    let mut ocr_pages = 0;
    for i in 0..n {
        job.check()?;
        let page = pdf.pages().get(i as PdfPageIndex).map_err(|e| crate::pdfrender::render_err(input, e))?;
        let t = if text_chars(&page) >= 10 {
            page.text().map(|t| t.all()).unwrap_or_default().replace("\r\n", "\n").trim().to_string()
        } else {
            let img = render(&page, 300, input)?;
            ocr_pages += 1;
            recognize(rec, &img, job.cancel)?.text
        };
        parts.push(t);
        job.tick(i + 1, n)?;
    }
    drop(pdf);
    let mut text = String::new();
    for (i, t) in parts.iter().enumerate() {
        if i > 0 {
            text.push_str(&format!("\n\n— {}쪽 —\n\n", i + 1));
        }
        text.push_str(if t.is_empty() { "(글자를 찾지 못한 쪽입니다)" } else { t });
    }
    let chars = parts.iter().map(|t| t.chars().filter(|c| !c.is_whitespace()).count()).sum::<usize>();
    let lang = rec.language();
    let hint = if ocr_pages > 0 { language_hint(&lang) } else { None };
    if chars == 0 {
        return Err(Error::Invalid(format!("PDF에서 글자를 찾지 못했어요{}", hint.as_ref().map(|h| format!(" · {h}")).unwrap_or_default())));
    }
    let output = write_txt(input, out_dir, &text)?;
    let mut note = format!("{n}쪽 → 글자 {chars}자");
    if ocr_pages > 0 {
        note.push_str(&format!(" · 스캔 {ocr_pages}쪽은 글자 인식으로 읽었어요"));
    }
    Ok(TextOut { input: input.into(), output, pages: n, ocr_pages, chars, language: lang, millis: t0.elapsed().as_millis() as u64, note, text: clip(&text), hint })
}

// ---------- PDF → Word: 인식한 줄 → pdflayout 줄 ----------

/// 인식 결과를 글이 가로가 되게 돌린 틀(pt)의 줄로. 반환: (줄, 틀 가로, 틀 세로)
pub fn layout_lines(rec: &Recognized, pt_per_px: f32) -> (Vec<Line>, f32, f32) {
    let (w, h) = (rec.width as f32, rec.height as f32);
    // 원래 그림 상자 → 돌린 틀(글이 가로)
    let fwd = |x: f32, y: f32, ww: f32, hh: f32| -> (f32, f32, f32, f32) {
        match rec.rot {
            90 => (h - y - hh, x, hh, ww),
            180 => (w - x - ww, h - y - hh, ww, hh),
            270 => (y, w - x - ww, hh, ww),
            _ => (x, y, ww, hh),
        }
    };
    let (fw, fh) = if rec.rot % 180 == 90 { (h, w) } else { (w, h) };
    let k = pt_per_px;
    let mut lines = Vec::new();
    for l in &rec.lines {
        let mut ws: Vec<(String, f32, f32, f32, f32)> = l
            .words
            .iter()
            .map(|wd| {
                let (x, y, ww, hh) = fwd(wd.x, wd.y, wd.w, wd.h);
                (wd.text.clone(), x * k, y * k, ww * k, hh * k)
            })
            .collect();
        if ws.is_empty() {
            continue;
        }
        ws.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let top = ws.iter().map(|w| w.2).fold(f32::MAX, f32::min);
        let bot = ws.iter().map(|w| w.2 + w.4).fold(f32::MIN, f32::max);
        let hgt = (bot - top).max(1.0);
        let size = ((hgt * 0.8) * 2.0).round() / 2.0;
        // 낱말 사이가 줄 높이의 2.5배 넘게 벌어지면 칸을 나눈다(표)
        let mut cells: Vec<Cell> = Vec::new();
        let mut cur: Vec<&(String, f32, f32, f32, f32)> = Vec::new();
        let flush = |cur: &mut Vec<&(String, f32, f32, f32, f32)>, cells: &mut Vec<Cell>| {
            if cur.is_empty() {
                return;
            }
            let text = cur.iter().map(|w| w.0.as_str()).collect::<Vec<_>>().join(" ");
            let x0 = cur.first().map(|w| w.1).unwrap_or(0.0);
            let x1 = cur.last().map(|w| w.1 + w.3).unwrap_or(0.0);
            cells.push(Cell { x0, x1, spans: vec![Span { text, bold: false, italic: false, size, font: String::new(), color: [0, 0, 0], link: None }] });
            cur.clear();
        };
        for wd in &ws {
            if let Some(prev) = cur.last() {
                if wd.1 - (prev.1 + prev.3) > hgt * 2.5 {
                    flush(&mut cur, &mut cells);
                }
            }
            cur.push(wd);
        }
        flush(&mut cur, &mut cells);
        let x0 = ws.first().map(|w| w.1).unwrap_or(0.0);
        let x1 = ws.iter().map(|w| w.1 + w.3).fold(f32::MIN, f32::max);
        lines.push(Line { x0, x1, top, bot, base: bot - hgt * 0.2, size, bold: false, cells, col: 0, lead_space: false, bullet: false });
    }
    lines.sort_by(|a, b| a.top.partial_cmp(&b.top).unwrap_or(std::cmp::Ordering::Equal));
    (lines, fw * k, fh * k)
}
