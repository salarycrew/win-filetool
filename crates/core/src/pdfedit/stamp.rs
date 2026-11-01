//! 쪽 위에 새 내용 얹기: 쪽 번호, 워터마크(글자/그림), 서명·도장 그림, 글상자.
//! 모두 "보이는 쪽" 좌표로 그린다(/Rotate가 있는 쪽도 화면에서 똑바로 보이게).

use std::path::{Path, PathBuf};

use lopdf::{Document, Object};
use serde::{Deserialize, Serialize};

use super::font::PdfFont;
use super::{add_content, add_resource, alpha_gs, cm, f, mat_mul, open, page_geom, page_ids, parse_color, rg, save, select_pages, EditOut, Job, Matrix};
use crate::error::{Error, Result};

// ---------- 쪽 번호 ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Position {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    #[default]
    BottomCenter,
    BottomRight,
}

impl Position {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "top-left" | "tl" => Position::TopLeft,
            "top-center" | "top" | "tc" => Position::TopCenter,
            "top-right" | "tr" => Position::TopRight,
            "bottom-left" | "bl" => Position::BottomLeft,
            "bottom-center" | "bottom" | "bc" => Position::BottomCenter,
            "bottom-right" | "br" => Position::BottomRight,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NumberFormat {
    /// `1`
    #[default]
    Plain,
    /// `1 / 12`
    OfTotal,
    /// `- 1 -`
    Dash,
}

fn d_start() -> u32 {
    1
}
fn d_size() -> f32 {
    10.0
}
fn d_margin() -> f32 {
    28.35
}
fn d_black() -> String {
    "#000000".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageNumberOpts {
    #[serde(default)]
    pub position: Position,
    #[serde(default = "d_start")]
    pub start: u32,
    #[serde(default)]
    pub format: NumberFormat,
    #[serde(default = "d_size")]
    pub font_size: f32,
    /// 왼쪽·오른쪽 여백(pt, 기본 10mm)
    #[serde(default = "d_margin")]
    pub margin_x: f32,
    /// 위·아래 여백(pt, 기본 10mm)
    #[serde(default = "d_margin")]
    pub margin_y: f32,
    /// 첫 쪽(표지)에는 번호를 넣지 않는다. 번호는 둘째 쪽부터 `start`.
    #[serde(default)]
    pub skip_first: bool,
    #[serde(default = "d_black")]
    pub color: String,
    /// 번호를 넣을 쪽(빈 값 = 전체)
    #[serde(default)]
    pub pages: Option<String>,
}

impl Default for PageNumberOpts {
    fn default() -> Self {
        serde_json::from_str("{}").unwrap()
    }
}

pub fn page_numbers(input: &Path, password: Option<&str>, opts: &PageNumberOpts, output: &Path, job: &mut Job) -> Result<EditOut> {
    if !(4.0..=96.0).contains(&opts.font_size) {
        return Err(Error::Invalid("글자 크기는 4~96 사이로 정하세요".into()));
    }
    let color = parse_color(&opts.color)?;
    let mut doc = open(input, password)?;
    let ids = page_ids(&doc);
    let n = ids.len() as u32;
    let mut targets = select_pages(opts.pages.as_deref(), n)?;
    if opts.skip_first {
        targets.retain(|&i| i != 0);
    }
    let last = opts.start + targets.len().saturating_sub(1) as u32;
    let mut font = PdfFont::new(&mut doc)?;
    let total = targets.len() as u32;
    for (k, &i) in targets.iter().enumerate() {
        job.tick(k as u32, total)?;
        let num = opts.start + k as u32;
        let text = match opts.format {
            NumberFormat::Plain => num.to_string(),
            NumberFormat::OfTotal => format!("{num} / {last}"),
            NumberFormat::Dash => format!("- {num} -"),
        };
        let pid = ids[i];
        let g = page_geom(&doc, pid);
        let (vw, vh) = g.visual_size();
        let size = opts.font_size;
        let w = font.width(&text, size);
        let x = match opts.position {
            Position::TopLeft | Position::BottomLeft => opts.margin_x,
            Position::TopCenter | Position::BottomCenter => (vw - w) / 2.0,
            Position::TopRight | Position::BottomRight => vw - opts.margin_x - w,
        };
        let y = match opts.position {
            Position::TopLeft | Position::TopCenter | Position::TopRight => vh - opts.margin_y - font.data().ascent() * size,
            _ => opts.margin_y - font.data().descent() * size,
        };
        let name = add_resource(&mut doc, pid, b"Font", "FPn", Object::Reference(font.id))?;
        let mut ops = cm(g.visual_to_user());
        ops.push_str(&rg(color));
        ops.push_str(&font.show(&name, size, x, y, &text));
        add_content(&mut doc, pid, ops.as_bytes(), true)?;
    }
    font.finish(&mut doc)?;
    save(doc, output)?;
    job.tick(total, total)?;
    Ok(EditOut { output: output.to_path_buf(), pages: n, note: format!("{}쪽에 쪽 번호를 넣었습니다({}~{})", total, opts.start, last) })
}

// ---------- 워터마크 ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    /// 내용 위(가려도 보이게)
    #[default]
    Over,
    /// 내용 아래(글자를 가리지 않음, 흰 바탕 그림이 있는 쪽에서는 안 보일 수 있음)
    Under,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tiling {
    /// 가운데 하나
    #[default]
    Center,
    /// 쪽 전체에 바둑판처럼
    Tile,
}

fn d_opacity() -> f32 {
    0.3
}
fn d_angle() -> f32 {
    45.0
}
fn d_wsize() -> f32 {
    48.0
}
fn d_gray() -> String {
    "#808080".into()
}
fn d_scale() -> f32 {
    0.5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WatermarkKind {
    Text {
        text: String,
        #[serde(default = "d_wsize")]
        font_size: f32,
        #[serde(default = "d_gray")]
        color: String,
    },
    Image {
        path: PathBuf,
        /// 쪽 너비에 대한 그림 너비 비율(0.05~1)
        #[serde(default = "d_scale")]
        scale: f32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatermarkOpts {
    #[serde(flatten)]
    pub kind: WatermarkKind,
    /// 0(투명)~1(불투명)
    #[serde(default = "d_opacity")]
    pub opacity: f32,
    /// 시계 반대 방향 각도(도)
    #[serde(default = "d_angle")]
    pub angle: f32,
    #[serde(default)]
    pub tiling: Tiling,
    #[serde(default)]
    pub layer: Layer,
    #[serde(default)]
    pub pages: Option<String>,
}

fn rot(angle_deg: f32) -> Matrix {
    let (s, c) = angle_deg.to_radians().sin_cos();
    [c, s, -s, c, 0.0, 0.0]
}

fn translate(x: f32, y: f32) -> Matrix {
    [1.0, 0.0, 0.0, 1.0, x, y]
}

/// 가운데 하나 또는 바둑판 자리: 각 항목을 놓을 행렬(항목 가운데가 원점인 좌표 → 보이는 쪽).
/// 바둑판은 돌린 좌표계에서 줄을 맞춰 깔아(줄 사이 간격 유지) 쪽 전체를 덮는다.
fn placements(vw: f32, vh: f32, item_w: f32, item_h: f32, angle: f32, tiling: Tiling) -> Vec<Matrix> {
    let base = mat_mul(rot(angle), translate(vw / 2.0, vh / 2.0));
    match tiling {
        Tiling::Center => vec![base],
        Tiling::Tile => {
            let sx = item_w + item_h.max(20.0) * 2.0;
            let sy = item_h * 3.5 + 20.0;
            let reach = (vw * vw + vh * vh).sqrt() / 2.0 + sx.max(sy);
            let mut v = Vec::new();
            let mut row = 0;
            let mut y = -reach;
            while y <= reach {
                let off = if row % 2 == 1 { sx / 2.0 } else { 0.0 };
                let mut x = -reach - off;
                while x <= reach {
                    v.push(mat_mul(translate(x, y), base));
                    x += sx;
                }
                y += sy;
                row += 1;
            }
            v
        }
    }
}

pub fn watermark(input: &Path, password: Option<&str>, opts: &WatermarkOpts, output: &Path, job: &mut Job) -> Result<EditOut> {
    if !(0.0..=1.0).contains(&opts.opacity) {
        return Err(Error::Invalid("투명도는 0~1 사이로 정하세요".into()));
    }
    let mut doc = open(input, password)?;
    let ids = page_ids(&doc);
    let n = ids.len() as u32;
    let targets = select_pages(opts.pages.as_deref(), n)?;
    let gs = alpha_gs(&mut doc, opts.opacity);
    let mut font = None;
    let mut image = None;
    match &opts.kind {
        WatermarkKind::Text { text, font_size, color } => {
            if text.trim().is_empty() {
                return Err(Error::Invalid("워터마크 글자를 적어 주세요".into()));
            }
            if !(4.0..=400.0).contains(font_size) {
                return Err(Error::Invalid("글자 크기는 4~400 사이로 정하세요".into()));
            }
            parse_color(color)?;
            font = Some(PdfFont::new(&mut doc)?);
        }
        WatermarkKind::Image { path, scale } => {
            if !(0.01..=1.0).contains(scale) {
                return Err(Error::Invalid("그림 크기는 쪽 너비의 1~100% 사이로 정하세요".into()));
            }
            image = Some(super::image_xobject(&mut doc, path, false)?);
        }
    }
    let total = targets.len() as u32;
    for (k, &i) in targets.iter().enumerate() {
        job.tick(k as u32, total)?;
        let pid = ids[i];
        let g = page_geom(&doc, pid);
        let (vw, vh) = g.visual_size();
        let gs_name = add_resource(&mut doc, pid, b"ExtGState", "GSw", Object::Reference(gs))?;
        let mut ops = cm(g.visual_to_user());
        ops.push_str(&format!("/{gs_name} gs\n"));
        match &opts.kind {
            WatermarkKind::Text { text, font_size, color } => {
                let fnt = font.as_mut().unwrap();
                let name = add_resource(&mut doc, pid, b"Font", "FWm", Object::Reference(fnt.id))?;
                let size = *font_size;
                let lines: Vec<&str> = text.lines().collect();
                let lh = size * 1.2;
                let tw = lines.iter().map(|l| fnt.width(l, size)).fold(0.0, f32::max);
                let th = lh * lines.len() as f32;
                ops.push_str(&rg(parse_color(color)?));
                for m in placements(vw, vh, tw, th, opts.angle, opts.tiling) {
                    ops.push_str("q\n");
                    ops.push_str(&cm(m));
                    for (li, l) in lines.iter().enumerate() {
                        let w = fnt.width(l, size);
                        // 여러 줄은 가운데 정렬, 덩어리 가운데가 (cx, cy)
                        let y = th / 2.0 - lh * (li as f32 + 1.0) + (lh - size) / 2.0 - fnt.data().descent() * size;
                        ops.push_str(&fnt.show(&name, size, -w / 2.0, y, l));
                    }
                    ops.push_str("Q\n");
                }
            }
            WatermarkKind::Image { scale, .. } => {
                let (img_id, iw, ih) = image.unwrap();
                let name = add_resource(&mut doc, pid, b"XObject", "IWm", Object::Reference(img_id))?;
                let w = vw * scale;
                let h = w * ih as f32 / iw.max(1) as f32;
                for pm in placements(vw, vh, w, h, opts.angle, opts.tiling) {
                    let m = mat_mul([w, 0.0, 0.0, h, -w / 2.0, -h / 2.0], pm);
                    ops.push_str(&format!("q\n{}/{name} Do\nQ\n", cm(m)));
                }
            }
        }
        add_content(&mut doc, pid, ops.as_bytes(), opts.layer == Layer::Over)?;
    }
    if let Some(fnt) = font {
        fnt.finish(&mut doc)?;
    }
    save(doc, output)?;
    job.tick(total, total)?;
    Ok(EditOut { output: output.to_path_buf(), pages: n, note: format!("{}쪽에 워터마크를 넣었습니다", total) })
}

// ---------- 서명·도장 그림 ----------

fn d_one() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagePlacement {
    /// 1부터
    pub page: u32,
    /// 보이는 쪽 왼쪽 위 기준(pt)
    pub x: f32,
    pub y: f32,
    pub w: f32,
    /// 0이면 그림 비율대로
    #[serde(default)]
    pub h: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignatureOpts {
    pub image: PathBuf,
    pub placements: Vec<ImagePlacement>,
    /// 흰색에 가까운 배경을 투명하게
    #[serde(default)]
    pub remove_white: bool,
    #[serde(default = "d_one")]
    pub opacity: f32,
}

pub fn place_image(input: &Path, password: Option<&str>, opts: &SignatureOpts, output: &Path, job: &mut Job) -> Result<EditOut> {
    if opts.placements.is_empty() {
        return Err(Error::Invalid("그림을 넣을 자리를 하나 이상 고르세요".into()));
    }
    let mut doc = open(input, password)?;
    let ids = page_ids(&doc);
    let n = ids.len() as u32;
    let (img, iw, ih) = super::image_xobject(&mut doc, &opts.image, opts.remove_white)?;
    let gs = if opts.opacity < 1.0 { Some(alpha_gs(&mut doc, opts.opacity)) } else { None };
    let total = opts.placements.len() as u32;
    for (k, p) in opts.placements.iter().enumerate() {
        job.tick(k as u32, total)?;
        if p.page == 0 || p.page > n {
            return Err(Error::PageSpec(format!("{}쪽은 범위를 벗어났습니다 (1~{n}쪽)", p.page)));
        }
        if p.w <= 0.0 {
            return Err(Error::Invalid("그림 너비는 0보다 커야 합니다".into()));
        }
        let pid = ids[p.page as usize - 1];
        let g = page_geom(&doc, pid);
        let h = if p.h > 0.0 { p.h } else { p.w * ih as f32 / iw.max(1) as f32 };
        let r = g.top_left_rect(p.x, p.y, p.w, h);
        let name = add_resource(&mut doc, pid, b"XObject", "ISg", Object::Reference(img))?;
        let mut ops = cm(g.visual_to_user());
        if let Some(gs) = gs {
            let gn = add_resource(&mut doc, pid, b"ExtGState", "GSs", Object::Reference(gs))?;
            ops.push_str(&format!("/{gn} gs\n"));
        }
        ops.push_str(&cm([p.w, 0.0, 0.0, h, r[0], r[1]]));
        ops.push_str(&format!("/{name} Do\n"));
        add_content(&mut doc, pid, ops.as_bytes(), true)?;
    }
    save(doc, output)?;
    job.tick(total, total)?;
    Ok(EditOut { output: output.to_path_buf(), pages: n, note: format!("그림을 {}곳에 넣었습니다", total) })
}

// ---------- 글상자 ----------

fn d_tsize() -> f32 {
    12.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextBox {
    pub page: u32,
    /// 보이는 쪽 왼쪽 위 기준(pt) — 글상자의 왼쪽 위
    pub x: f32,
    pub y: f32,
    pub text: String,
    #[serde(default = "d_tsize")]
    pub font_size: f32,
    #[serde(default = "d_black")]
    pub color: String,
    /// 바탕 상자 색(예: `#FFFFFF`). 없으면 바탕 없음.
    #[serde(default)]
    pub background: Option<String>,
    /// 이 너비(pt)에서 줄바꿈. 0이면 줄바꿈 글자(`\n`)에서만.
    #[serde(default)]
    pub width: f32,
}

pub fn text_boxes(input: &Path, password: Option<&str>, boxes: &[TextBox], output: &Path, job: &mut Job) -> Result<EditOut> {
    if boxes.is_empty() {
        return Err(Error::Invalid("넣을 글상자가 없습니다".into()));
    }
    let mut doc = open(input, password)?;
    let ids = page_ids(&doc);
    let n = ids.len() as u32;
    let mut font = PdfFont::new(&mut doc)?;
    let total = boxes.len() as u32;
    let mut missing = Vec::new();
    for (k, b) in boxes.iter().enumerate() {
        job.tick(k as u32, total)?;
        if b.page == 0 || b.page > n {
            return Err(Error::PageSpec(format!("{}쪽은 범위를 벗어났습니다 (1~{n}쪽)", b.page)));
        }
        if b.text.trim().is_empty() {
            return Err(Error::Invalid("글상자에 넣을 글자를 적어 주세요".into()));
        }
        if !(4.0..=200.0).contains(&b.font_size) {
            return Err(Error::Invalid("글자 크기는 4~200 사이로 정하세요".into()));
        }
        missing.extend(font.data().missing(&b.text));
        let color = parse_color(&b.color)?;
        let pid = ids[b.page as usize - 1];
        let g = page_geom(&doc, pid);
        let (_, vh) = g.visual_size();
        let size = b.font_size;
        let lines = font.wrap(&b.text, size, b.width);
        let lh = size * 1.25;
        let tw = if b.width > 0.0 { b.width } else { lines.iter().map(|l| font.width(l, size)).fold(0.0, f32::max) };
        let th = lh * lines.len() as f32;
        let pad = size * 0.3;
        let name = add_resource(&mut doc, pid, b"Font", "FTb", Object::Reference(font.id))?;
        let mut ops = cm(g.visual_to_user());
        let top = vh - b.y;
        if let Some(bg) = &b.background {
            let c = parse_color(bg)?;
            ops.push_str(&format!("{}{} {} {} {} re f\n", rg(c), f(b.x - pad), f(top - th - pad), f(tw + 2.0 * pad), f(th + 2.0 * pad)));
        }
        ops.push_str(&rg(color));
        for (li, l) in lines.iter().enumerate() {
            let base = top - lh * li as f32 - (lh - size) / 2.0 - font.data().ascent() * size;
            ops.push_str(&font.show(&name, size, b.x, base, l));
        }
        add_content(&mut doc, pid, ops.as_bytes(), true)?;
    }
    font.finish(&mut doc)?;
    save(doc, output)?;
    job.tick(total, total)?;
    missing.sort();
    missing.dedup();
    let mut note = format!("글상자 {}개를 넣었습니다", total);
    if !missing.is_empty() {
        let s: String = missing.iter().take(10).collect();
        note.push_str(&format!(". 글꼴에 없는 글자는 빈 네모로 보입니다: {s}"));
    }
    Ok(EditOut { output: output.to_path_buf(), pages: n, note })
}

/// 테스트·CLI용: 지정한 쪽 하나의 보이는 크기(pt)
pub fn visual_size_of(doc: &Document, page: u32) -> Option<(f32, f32)> {
    let ids = page_ids(doc);
    ids.get(page as usize - 1).map(|&id| page_geom(doc, id).visual_size())
}
