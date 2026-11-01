//! 쪽 모양 바꾸기: 여백 자르기(CropBox), A4에 맞추기, 모아 찍기(2쪽/4쪽을 한 장에).

use std::path::Path;

use lopdf::{dictionary, Document, Object, ObjectId, Stream};
use serde::{Deserialize, Serialize};

use super::{cm, mat_bbox, mat_invert, mat_mul, open, page_geom, page_ids, page_to_form, rect_obj, save, select_pages, EditOut, Job, Matrix, Orientation, A4};
use crate::error::{Error, Result};

// ---------- 여백 자르기 ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum CropMode {
    /// 보이는 쪽 기준으로 위·오른쪽·아래·왼쪽에서 잘라 낼 폭(pt)
    Margins { top: f32, right: f32, bottom: f32, left: f32 },
    /// 그려 보고 내용이 있는 곳만 남김(`padding` pt만큼 여유)
    Auto {
        #[serde(default = "d_pad")]
        padding: f32,
    },
}

fn d_pad() -> f32 {
    8.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CropOpts {
    #[serde(flatten)]
    pub mode: CropMode,
    #[serde(default)]
    pub pages: Option<String>,
}

/// 흰 바탕이 아닌 부분의 상자(px). 없으면 None.
pub fn content_bounds(img: &image::RgbImage, threshold: u8) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = img.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if p.0.iter().any(|&c| c < threshold) {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 == u32::MAX || w == 0 || h == 0 {
        None
    } else {
        Some((x0, y0, x1 + 1, y1 + 1))
    }
}

pub fn crop(input: &Path, password: Option<&str>, opts: &CropOpts, output: &Path, job: &mut Job) -> Result<EditOut> {
    let mut doc = open(input, password)?;
    let ids = page_ids(&doc);
    let n = ids.len() as u32;
    let targets = select_pages(opts.pages.as_deref(), n)?;
    let total = targets.len() as u32;
    let mut skipped = 0;
    for (k, &i) in targets.iter().enumerate() {
        job.tick(k as u32, total)?;
        let pid = ids[i];
        let g = page_geom(&doc, pid);
        let (vw, vh) = g.visual_size();
        // 남길 사각형(보이는 쪽, 왼쪽 아래 원점)
        let keep = match &opts.mode {
            CropMode::Margins { top, right, bottom, left } => {
                if [*top, *right, *bottom, *left].iter().any(|v| *v < 0.0) {
                    return Err(Error::Invalid("자를 폭은 0 이상이어야 합니다".into()));
                }
                [*left, *bottom, vw - right, vh - top]
            }
            CropMode::Auto { padding } => {
                // 1pt = 1px 정도로 그려서 찾는다(작은 쪽은 더 크게)
                let scale = (600.0 / vw.max(vh)).clamp(0.5, 2.0);
                let img = super::thumbs::render_rgb(input, password, i as u32 + 1, scale)?;
                match content_bounds(&img, 245) {
                    Some((x0, y0, x1, y1)) => {
                        let sx = vw / img.width() as f32;
                        let sy = vh / img.height() as f32;
                        [
                            (x0 as f32 * sx - padding).max(0.0),
                            (vh - y1 as f32 * sy - padding).max(0.0),
                            (x1 as f32 * sx + padding).min(vw),
                            (vh - y0 as f32 * sy + padding).min(vh),
                        ]
                    }
                    None => {
                        skipped += 1;
                        continue;
                    }
                }
            }
        };
        if keep[2] - keep[0] < 18.0 || keep[3] - keep[1] < 18.0 {
            return Err(Error::Invalid(format!("{}쪽: 자르고 남는 부분이 너무 작습니다", i + 1)));
        }
        let user = mat_bbox(g.visual_to_user(), keep);
        let page = doc.get_dictionary_mut(pid).map_err(|e| Error::Invalid(e.to_string()))?;
        page.set("CropBox", rect_obj(user));
        // 잘라 낸 쪽을 보기·인쇄 모두 같게
        page.remove(b"TrimBox");
        page.remove(b"ArtBox");
        page.remove(b"BleedBox");
    }
    save(doc, output)?;
    job.tick(total, total)?;
    let mut note = format!("{}쪽의 여백을 잘랐습니다", total - skipped);
    if skipped > 0 {
        note.push_str(&format!(" (빈 쪽 {skipped}쪽은 그대로)"));
    }
    Ok(EditOut { output: output.to_path_buf(), pages: n, note })
}

// ---------- A4에 맞추기 ----------

fn d_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FitOpts {
    #[serde(default)]
    pub orientation: Orientation,
    /// 종이 가장자리 여백(pt)
    #[serde(default)]
    pub margin: f32,
    /// 작은 쪽도 A4 크기까지 키운다(끄면 줄이기만)
    #[serde(default = "d_true")]
    pub enlarge: bool,
}

impl Default for FitOpts {
    fn default() -> Self {
        FitOpts { orientation: Orientation::Auto, margin: 0.0, enlarge: true }
    }
}

/// 보이는 쪽(vw×vh)을 상자 [x0,y0,x1,y1] 가운데에 맞추는 행렬(보이는 쪽 좌표 → 종이)
fn fit_matrix(vw: f32, vh: f32, cell: [f32; 4], enlarge: bool) -> Matrix {
    let cw = cell[2] - cell[0];
    let ch = cell[3] - cell[1];
    let mut s = (cw / vw).min(ch / vh);
    if !enlarge {
        s = s.min(1.0);
    }
    let ox = cell[0] + (cw - vw * s) / 2.0;
    let oy = cell[1] + (ch - vh * s) / 2.0;
    [s, 0.0, 0.0, s, ox, oy]
}

fn sheet_size(o: Orientation, landscape_hint: bool) -> (f32, f32) {
    match o {
        Orientation::Portrait => A4,
        Orientation::Landscape => (A4.1, A4.0),
        Orientation::Auto => {
            if landscape_hint {
                (A4.1, A4.0)
            } else {
                A4
            }
        }
    }
}

/// 쪽 하나를 "종이 + Form XObject 한 장"으로 바꾼다(같은 쪽 객체를 고쳐 링크·책갈피가 계속 그 쪽을 가리키게).
fn replace_with_sheet(doc: &mut Document, pid: ObjectId, sheet: (f32, f32), placements: &[(ObjectId, Matrix)]) -> Result<()> {
    let mut xo = lopdf::Dictionary::new();
    let mut ops = String::new();
    for (k, (form, m)) in placements.iter().enumerate() {
        let name = format!("Pg{}", k + 1);
        xo.set(name.clone().into_bytes(), Object::Reference(*form));
        ops.push_str(&format!("q\n{}/{} Do\nQ\n", cm(*m), name));
    }
    let content = doc.add_object(Stream::new(dictionary! {}, ops.into_bytes()));
    let page = doc.get_dictionary_mut(pid).map_err(|e| Error::Invalid(e.to_string()))?;
    for key in [&b"CropBox"[..], b"TrimBox", b"ArtBox", b"BleedBox", b"Rotate", b"Annots", b"Group", b"Thumb", b"UserUnit"] {
        page.remove(key);
    }
    page.set("MediaBox", rect_obj([0.0, 0.0, sheet.0, sheet.1]));
    page.set("Resources", dictionary! { "XObject" => Object::Dictionary(xo) });
    page.set("Contents", Object::Reference(content));
    Ok(())
}

pub fn fit_a4(input: &Path, password: Option<&str>, opts: &FitOpts, output: &Path, job: &mut Job) -> Result<EditOut> {
    if !(0.0..=144.0).contains(&opts.margin) {
        return Err(Error::Invalid("여백은 0~144pt 사이로 정하세요".into()));
    }
    let mut doc = open(input, password)?;
    crate::pdf::flatten(&mut doc, input)?;
    let ids = page_ids(&doc);
    let n = ids.len() as u32;
    for (k, &pid) in ids.iter().enumerate() {
        job.tick(k as u32, n)?;
        let (form, g) = page_to_form(&mut doc, pid)?;
        let (vw, vh) = g.visual_size();
        let sheet = sheet_size(opts.orientation, vw > vh);
        let m = opts.margin;
        // 폼 공간(사용자 공간) → 보이는 쪽 → 종이
        let place = mat_mul(mat_invert(g.visual_to_user()), fit_matrix(vw, vh, [m, m, sheet.0 - m, sheet.1 - m], opts.enlarge));
        replace_with_sheet(&mut doc, pid, sheet, &[(form, place)])?;
    }
    remove_form_catalog(&mut doc);
    save(doc, output)?;
    job.tick(n, n)?;
    Ok(EditOut { output: output.to_path_buf(), pages: n, note: format!("{n}쪽을 A4 크기에 맞췄습니다") })
}

/// 쪽 모양을 바꾼 뒤 양식 칸 위치가 맞지 않으므로 양식을 뺀다(보이는 모양은 쪽 내용에 남음)
fn remove_form_catalog(doc: &mut Document) {
    if let Ok(cat) = doc.catalog_mut() {
        cat.remove(b"AcroForm");
    }
}

// ---------- 모아 찍기 ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NupOrder {
    /// 왼쪽→오른쪽, 그다음 줄
    #[default]
    Across,
    /// 위→아래, 그다음 칸
    Down,
}

fn d_two() -> u32 {
    2
}
fn d_gap() -> f32 {
    8.0
}
fn d_margin() -> f32 {
    14.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NupOpts {
    /// 2 또는 4
    #[serde(default = "d_two")]
    pub per_sheet: u32,
    /// Auto면 2쪽은 가로, 4쪽은 세로 A4
    #[serde(default)]
    pub orientation: Orientation,
    #[serde(default)]
    pub order: NupOrder,
    /// 칸 사이 간격(pt)
    #[serde(default = "d_gap")]
    pub gap: f32,
    /// 종이 가장자리 여백(pt)
    #[serde(default = "d_margin")]
    pub margin: f32,
}

impl Default for NupOpts {
    fn default() -> Self {
        NupOpts { per_sheet: 2, orientation: Orientation::Auto, order: NupOrder::Across, gap: 8.0, margin: 14.0 }
    }
}

pub fn nup(input: &Path, password: Option<&str>, opts: &NupOpts, output: &Path, job: &mut Job) -> Result<EditOut> {
    if ![2, 4].contains(&opts.per_sheet) {
        return Err(Error::Invalid("한 장에 2쪽 또는 4쪽만 됩니다".into()));
    }
    if !(0.0..=72.0).contains(&opts.gap) || !(0.0..=144.0).contains(&opts.margin) {
        return Err(Error::Invalid("간격은 0~72pt, 여백은 0~144pt 사이로 정하세요".into()));
    }
    let mut doc = open(input, password)?;
    crate::pdf::flatten(&mut doc, input)?;
    let root = crate::pdf::pages_root(&doc, input)?;
    let ids = page_ids(&doc);
    let n = ids.len() as u32;
    let landscape = match opts.orientation {
        Orientation::Auto => opts.per_sheet == 2,
        Orientation::Landscape => true,
        Orientation::Portrait => false,
    };
    let sheet = if landscape { (A4.1, A4.0) } else { A4 };
    // 칸 나누기(열 × 행)
    let (cols, rows) = match (opts.per_sheet, landscape) {
        (2, true) => (2, 1),
        (2, false) => (1, 2),
        _ => (2, 2),
    };
    let m = opts.margin;
    let gap = opts.gap;
    let cw = (sheet.0 - 2.0 * m - gap * (cols as f32 - 1.0)) / cols as f32;
    let ch = (sheet.1 - 2.0 * m - gap * (rows as f32 - 1.0)) / rows as f32;
    let cells: Vec<[f32; 4]> = (0..opts.per_sheet)
        .map(|k| {
            let (c, r) = match opts.order {
                NupOrder::Across => (k % cols, k / cols),
                NupOrder::Down => (k / rows, k % rows),
            };
            let x0 = m + c as f32 * (cw + gap);
            let top = sheet.1 - m - r as f32 * (ch + gap);
            [x0, top - ch, x0 + cw, top]
        })
        .collect();
    let mut sheets = Vec::new();
    let chunks: Vec<&[ObjectId]> = ids.chunks(opts.per_sheet as usize).collect();
    let total = chunks.len() as u32;
    for (k, group) in chunks.iter().enumerate() {
        job.tick(k as u32, total)?;
        let mut placements = Vec::new();
        for (slot, &pid) in group.iter().enumerate() {
            let (form, g) = page_to_form(&mut doc, pid)?;
            let (vw, vh) = g.visual_size();
            placements.push((form, mat_mul(mat_invert(g.visual_to_user()), fit_matrix(vw, vh, cells[slot], true))));
        }
        let new_page = doc.add_object(dictionary! { "Type" => "Page", "Parent" => root });
        replace_with_sheet(&mut doc, new_page, sheet, &placements)?;
        sheets.push(new_page);
    }
    crate::pdf::set_kids(&mut doc, root, &sheets, input)?;
    // 옛 쪽을 가리키는 책갈피·이름 붙은 위치·구조 정보는 뺀다
    if let Ok(cat) = doc.catalog_mut() {
        for key in [&b"Outlines"[..], b"Dests", b"StructTreeRoot", b"MarkInfo", b"PageLabels", b"OpenAction", b"AcroForm"] {
            cat.remove(key);
        }
        if cat.get(b"PageMode").and_then(Object::as_name).map(|v| v == b"UseOutlines").unwrap_or(false) {
            cat.remove(b"PageMode");
        }
    }
    let count = sheets.len() as u32;
    save(doc, output)?;
    job.tick(total, total)?;
    Ok(EditOut { output: output.to_path_buf(), pages: count, note: format!("{n}쪽을 한 장에 {}쪽씩 {count}장으로 모았습니다", opts.per_sheet) })
}
