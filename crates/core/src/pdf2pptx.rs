//! PDF → PowerPoint. LibreOffice를 쓰지 않는다(그 PDF 가져오기 부분은 GPL).
//!
//! - `image`(기본, "그대로 보이게"): 쪽마다 PDFium으로 그린 그림 한 장. 슬라이드 크기 = 첫 쪽 크기(A4 세로면 세로).
//!   크기가 다른 쪽은 가운데에 맞춰 넣는다. 글자는 고칠 수 없다.
//! - `editable`("글자 고칠 수 있게"): [`crate::pdflayout`]로 읽은 글줄을 원래 자리에 글상자로 놓고(줄마다 원래 줄 간격),
//!   그림은 제자리에, 칠한 사각형·선은 간단한 도형으로 넣는다. 스캔본 쪽과 글자가 돌아간 쪽은 쪽 그림으로 둔다.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ooxml::pptx::{slide_size_for_page, Anchor, Deck, Para, Run, Shape, TextBox};
use crate::paths;
use crate::pdflayout::{self, AnalyzeOpts, Item, Line, PShape, PageLayout};
use crate::process::Cancel;
use pdfium_render::prelude::PdfPageIndex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PptMode {
    /// 그대로 보이게(쪽 그림)
    #[default]
    Image,
    /// 글자 고칠 수 있게(글상자)
    Editable,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PdfToPptxOut {
    pub input: PathBuf,
    pub output: PathBuf,
    pub mode: PptMode,
    pub slides: usize,
    pub text_boxes: usize,
    pub pictures: usize,
    pub shapes: usize,
    /// 편집 가능본에서 쪽 그림으로 둔 쪽(스캔본·돌아간 글자)
    pub image_pages: usize,
    pub millis: u64,
    pub note: String,
}

const EMU: f64 = 12_700.0;
pub const DEFAULT_DPI: f32 = 150.0;

/// 쪽(pt) → 슬라이드 안 위치: (배율, x 여백, y 여백)
fn place(slide: (i64, i64), w_pt: f32, h_pt: f32) -> (f64, i64, i64) {
    let s = (slide.0 as f64 / (w_pt as f64 * EMU)).min(slide.1 as f64 / (h_pt as f64 * EMU));
    let (w, h) = ((w_pt as f64 * EMU * s) as i64, (h_pt as f64 * EMU * s) as i64);
    (s, (slide.0 - w) / 2, (slide.1 - h) / 2)
}

fn full_picture(deck: &mut Deck, jpeg: Vec<u8>, w_pt: f32, h_pt: f32, k: usize) -> Shape {
    let (s, ox, oy) = place(deck.size, w_pt, h_pt);
    let m = deck.add_media(&format!("page{k}.jpg"), jpeg);
    Shape::Pic { x: ox, y: oy, w: (w_pt as f64 * EMU * s) as i64, h: (h_pt as f64 * EMU * s) as i64, media: m }
}

fn runs_of_line(l: &Line, scale: f64, cell: Option<usize>) -> Vec<Run> {
    let cells: Vec<&pdflayout::Cell> = match cell {
        Some(k) => l.cells.get(k).into_iter().collect(),
        None => l.cells.iter().collect(),
    };
    let mut runs = Vec::new();
    for (ci, c) in cells.iter().enumerate() {
        for (si, sp) in c.spans.iter().enumerate() {
            let mut text = sp.text.clone();
            if ci > 0 && si == 0 {
                text.insert(0, ' ');
            }
            if text.is_empty() {
                continue;
            }
            runs.push(Run {
                text,
                size_pt: ((sp.size as f64) * scale).max(1.0) as f32,
                bold: sp.bold,
                italic: sp.italic,
                color: (sp.color != [0, 0, 0]).then_some(sp.color),
                font: pdflayout::map_font(&sp.font),
            });
        }
    }
    runs
}

/// 글상자 묶음: 같은 단·같은 크기·왼쪽이 맞고 줄 간격이 촘촘한 한 칸 줄들을 한 상자로(줄은 그대로 유지)
fn editable_slide(deck: &mut Deck, p: &PageLayout, k: usize) -> (Vec<Shape>, usize) {
    let (s, ox, oy) = place(deck.size, p.width, p.height);
    let e = |v: f32| (v as f64 * EMU * s) as i64;
    let mut shapes = Vec::new();
    // 1) 도형(맨 아래), 2) 그림, 3) 글
    for sh in p.shapes.iter().take(600) {
        match sh {
            PShape::Rect { x0, top, w, h, fill, stroke } => shapes.push(Shape::Rect { x: ox + e(*x0), y: oy + e(*top), w: e(*w), h: e(*h), fill: *fill, line: stroke.map(|(c, w)| (c, e(w).max(3175))) }),
            PShape::Line { x1, y1, x2, y2, color, width } => shapes.push(Shape::Line { x1: ox + e(*x1), y1: oy + e(*y1), x2: ox + e(*x2), y2: oy + e(*y2), color: *color, width: e(*width).max(3175) }),
        }
    }
    for it in &p.items {
        if let Item::Image(im) = it {
            let m = deck.add_media(&format!("p{k}.{}", im.ext), im.bytes.clone());
            shapes.push(Shape::Pic { x: ox + e(im.x0), y: oy + e(im.top), w: e(im.w), h: e(im.h), media: m });
        }
    }
    let lines: Vec<&Line> = p.items.iter().filter_map(|i| if let Item::Line(l) = i { Some(l) } else { None }).collect();
    let mut boxes = 0;
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if l.cells.len() >= 2 {
            // 표처럼 칸이 나뉜 줄: 칸마다 상자
            for (ci, c) in l.cells.iter().enumerate() {
                let runs = runs_of_line(l, s, Some(ci));
                if runs.is_empty() {
                    continue;
                }
                let h = e((l.bot - l.top).max(l.size * 1.2));
                shapes.push(Shape::Text(TextBox { x: ox + e(c.x0), y: oy + e(l.top), w: e(c.x1 - c.x0) + e(l.size), h, paras: vec![Para { runs, line_pt: None, ..Default::default() }], anchor: Anchor::Top, wrap: false, inset: false }));
                boxes += 1;
            }
            i += 1;
            continue;
        }
        let mut group = vec![l];
        let mut j = i + 1;
        while j < lines.len() {
            let m = lines[j];
            let prev = group[group.len() - 1];
            let pitch = m.base - prev.base;
            if m.cells.len() == 1 && m.col == l.col && (m.size - l.size).abs() < 0.6 && (m.x0 - l.x0).abs() < 2.0 * l.size && pitch > 0.0 && pitch < 2.0 * l.size {
                group.push(m);
                j += 1;
            } else {
                break;
            }
        }
        let pitch = if group.len() >= 2 { (group[group.len() - 1].base - group[0].base) / (group.len() - 1) as f32 } else { l.size * 1.2 };
        let x0 = group.iter().map(|g| g.x0).fold(f32::MAX, f32::min);
        let x1 = group.iter().map(|g| g.x1).fold(f32::MIN, f32::max);
        let paras: Vec<Para> = group
            .iter()
            .map(|g| Para { runs: runs_of_line(g, s, None), line_pt: Some(((pitch as f64) * s) as f32), ..Default::default() })
            .filter(|p| !p.runs.is_empty())
            .collect();
        if !paras.is_empty() {
            let top = group[0].top;
            let h = e(pitch * group.len() as f32 + 0.3 * l.size);
            // 글꼴이 바뀌어 조금 넓어져도 줄이 바뀌지 않게 줄바꿈 없음 + 여유 폭
            shapes.push(Shape::Text(TextBox { x: ox + e(x0), y: oy + e(top - 0.1 * l.size), w: e(x1 - x0) + e(l.size * 2.0), h, paras, anchor: Anchor::Top, wrap: false, inset: false }));
            boxes += 1;
        }
        i = j;
    }
    (shapes, boxes)
}

/// PDF → `이름_PPT.pptx`
pub fn pdf_to_pptx(input: &Path, out_dir: Option<&Path>, mode: PptMode, dpi: Option<f32>, cancel: Option<&Cancel>, progress: &mut dyn FnMut(u32, u32)) -> Result<PdfToPptxOut> {
    let t0 = Instant::now();
    if !input.is_file() {
        return Err(Error::Invalid(format!("파일이 없습니다: {}", input.display())));
    }
    let dpi = dpi.unwrap_or(DEFAULT_DPI);
    if !(36.0..=600.0).contains(&dpi) {
        return Err(Error::Invalid(format!("해상도는 36~600 DPI 사이로 정하세요 ({dpi})")));
    }
    let pdfium = crate::pdfrender::pdfium()?;
    let doc = pdfium.load_pdf_from_file(input, None).map_err(|e| crate::pdfrender::open_err(input, e))?;
    let n = doc.pages().len() as usize;
    if n == 0 {
        return Err(Error::Pdf { path: input.into(), message: "쪽이 없습니다".into() });
    }
    let stem = input.file_stem().map(|s| crate::nfc::to_nfc(&s.to_string_lossy())).unwrap_or_else(|| "PDF".into());
    let mut deck = Deck::new(stem);
    let perr = |e| Error::Pdf { path: input.into(), message: format!("쪽을 그리지 못했습니다: {e}") };
    let mut boxes = 0;
    let mut image_pages = 0;
    let mut mismatched = 0;
    {
        let first = doc.pages().get(0).map_err(perr)?;
        deck.size = slide_size_for_page(first.width().value as f64, first.height().value as f64);
    }
    let fw = |p: &pdfium_render::prelude::PdfPage| (p.width().value, p.height().value);
    match mode {
        PptMode::Image => {
            for (i, page) in doc.pages().iter().enumerate() {
                if let Some(c) = cancel {
                    c.check()?;
                }
                let (w, h) = fw(&page);
                let (dw, dh) = (deck.size.0 as f32 / EMU as f32, deck.size.1 as f32 / EMU as f32);
                if ((w / h) - (dw / dh)).abs() > 0.02 {
                    mismatched += 1;
                }
                let jpg = pdflayout::render_jpeg(&page, dpi, 90).map_err(perr)?;
                let pic = full_picture(&mut deck, jpg, w, h, i + 1);
                deck.slides.push(vec![pic]);
                image_pages += 1;
                progress(i as u32 + 1, n as u32);
            }
        }
        PptMode::Editable => {
            // 분석이 쪽 진행률의 앞 70%, 슬라이드 만들기가 뒤 30%
            let total = n as u32 * 10;
            let layout = pdflayout::analyze_doc(input, &doc, &AnalyzeOpts { images: true, render_scanned: true }, cancel, &mut |i, _| progress(i * 7, total))?;
            for (i, p) in layout.pages.iter().enumerate() {
                if let Some(c) = cancel {
                    c.check()?;
                }
                if let Some(jpg) = &p.scanned {
                    let pic = full_picture(&mut deck, jpg.clone(), p.disp_w, p.disp_h, i + 1);
                    deck.slides.push(vec![pic]);
                    image_pages += 1;
                } else if !p.upright {
                    let page = doc.pages().get(i as PdfPageIndex).map_err(perr)?;
                    let jpg = pdflayout::render_jpeg(&page, dpi, 90).map_err(perr)?;
                    let pic = full_picture(&mut deck, jpg, p.disp_w, p.disp_h, i + 1);
                    deck.slides.push(vec![pic]);
                    image_pages += 1;
                } else {
                    let (shapes, b) = editable_slide(&mut deck, p, i + 1);
                    boxes += b;
                    deck.slides.push(shapes);
                }
                progress(n as u32 * 7 + (i as u32 + 1) * 3, total);
            }
        }
    }
    let output = paths::output_path(input, out_dir, "_PPT", "pptx");
    deck.save(&output)?;
    let st = deck.count();
    let mut note = match mode {
        PptMode::Image => format!("{}쪽 → 슬라이드 {}장(쪽 그림, {} DPI)", n, st.slides, dpi as u32),
        PptMode::Editable => format!("{}쪽 → 슬라이드 {}장(글상자 {}개, 그림 {}개, 도형 {}개)", n, st.slides, boxes, st.pictures, st.shapes),
    };
    if mode == PptMode::Editable && image_pages > 0 {
        note.push_str(&format!(" · {image_pages}쪽은 글자를 읽을 수 없거나 돌아가 있어 쪽 그림으로 넣었습니다"));
    }
    if mismatched > 0 {
        note.push_str(&format!(" · 크기가 다른 {mismatched}쪽은 가운데에 맞췄습니다"));
    }
    Ok(PdfToPptxOut {
        input: input.into(),
        output,
        mode,
        slides: st.slides,
        text_boxes: st.text_boxes,
        pictures: st.pictures,
        shapes: st.shapes,
        image_pages,
        millis: t0.elapsed().as_millis() as u64,
        note,
    })
}
