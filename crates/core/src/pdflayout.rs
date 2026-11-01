//! PDF 글자 배치 분석(PDF→Word·PPT(글자 고칠 수 있게)·TXT가 함께 씀). PDFium(BSD)으로 글자 하나하나의 위치를 읽어
//! 줄 → 칸(cell) → 표/문단으로 다시 묶는다. 고정 위치 글상자를 만들지 않는 것이 목적이다.
//!
//! - 좌표: 글자 방향 기준 틀(frame). 글자가 대부분 90°/180°/270° 돌아 있으면(가로 쪽을 돌려 넣은 PDF 등) 그 방향을
//!   바로 세운 틀에서 읽는다. y는 위에서 아래로 커진다. 쪽의 /Rotate와 글자 방향이 같으면 틀 = 화면에 보이는 쪽.
//! - 여러 단(column): 쪽 가운데 쪽에 세로로 빈 띠가 있고 양쪽에 긴 글줄이 여럿이면 단으로 본다. 단을 가로지르는 줄
//!   (제목 등)이 나오면 그 앞까지 왼쪽 단 → 오른쪽 단 순서로 읽는다.
//! - 머리글·바닥글·쪽 번호: 쪽 위·아래 10% 안에서 여러 쪽에 되풀이되는 줄(숫자는 같게 봄)과 쪽 번호 모양 줄은 뺀다.
//! - 표: 칸이 2개 이상인 줄이 2줄 넘게 이어지면 표. 선이 없어도 칸의 가로 범위가 겹치는 것끼리 한 열로 묶는다.
//!   가로 선이 있으면 선 사이 여러 줄을 한 행으로 합친다. 세로 선이 있으면 선에서 칸을 나눈다.
//! - 한글 줄 잇기: 앞줄이 오른쪽 끝까지 찼고 앞뒤가 모두 한글이면 빈칸 없이 붙인다(글자 단위 줄바꿈, 예: `지적\n된` → `지적된`).
//!   앞줄이 일찍 끝났으면(다음 낱말이 안 들어가서 넘긴 것) 빈칸을 넣는다.
//! - 스캔본: 글자가 20자 미만이고 쪽 절반 넘는 그림(또는 선·도형만 많은 쪽)이 있으면 쪽 그림으로 다룬다(OCR 없음).

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

use pdfium_render::prelude::*;

use crate::error::{Error, Result};
use crate::process::Cancel;

// ---------- 결과 구조 ----------

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub size: f32,
    /// PDF 글꼴 이름(부분 집합 접두어 뺌)
    pub font: String,
    pub color: [u8; 3],
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub x0: f32,
    pub x1: f32,
    pub spans: Vec<Span>,
}

impl Cell {
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub x0: f32,
    pub x1: f32,
    pub top: f32,
    pub bot: f32,
    pub base: f32,
    /// 글자 크기 중앙값(pt)
    pub size: f32,
    pub bold: bool,
    pub cells: Vec<Cell>,
    /// 0 = 단 없음/가로지름, 1 = 왼쪽 단, 2 = 오른쪽 단
    pub col: u8,
    /// 앞줄 끝에 진짜 빈칸 글자가 있었음(PDF가 줄 끝 빈칸을 남기는 경우: LibreOffice·Word)
    pub lead_space: bool,
    /// 줄 왼쪽에 도형으로 그린 글머리표(Edge·Chrome 인쇄)
    pub bullet: bool,
}

impl Line {
    pub fn text(&self) -> String {
        self.cells.iter().map(Cell::text).collect::<Vec<_>>().join(" ")
    }
    pub fn chars(&self) -> usize {
        self.cells.iter().flat_map(|c| c.spans.iter()).map(|s| s.text.chars().filter(|c| !c.is_whitespace()).count()).sum()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PImage {
    pub x0: f32,
    pub top: f32,
    pub w: f32,
    pub h: f32,
    /// 원본 바이트(JPEG면 그대로) 또는 PNG
    pub bytes: Vec<u8>,
    pub ext: &'static str,
    pub col: u8,
    /// 쪽 대부분을 덮는 배경 그림
    pub background: bool,
}

/// 칠한 사각형·선(PPT 편집 가능본에서 쓴다)
#[derive(Debug, Clone, PartialEq)]
pub enum PShape {
    Rect { x0: f32, top: f32, w: f32, h: f32, fill: Option<[u8; 3]>, stroke: Option<([u8; 3], f32)> },
    Line { x1: f32, y1: f32, x2: f32, y2: f32, color: [u8; 3], width: f32 },
}

/// 쪽 안 읽기 순서의 한 항목
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Line(Line),
    Image(PImage),
}

impl Item {
    pub fn top(&self) -> f32 {
        match self {
            Item::Line(l) => l.top,
            Item::Image(i) => i.top,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PageLayout {
    pub index: usize,
    /// 틀 크기(pt)
    pub width: f32,
    pub height: f32,
    /// 화면에 보이는 쪽 크기(pt, /Rotate 반영)
    pub disp_w: f32,
    pub disp_h: f32,
    /// 틀이 화면과 같은 방향인지(PPT 위치 맞춤에 필요)
    pub upright: bool,
    pub items: Vec<Item>,
    pub shapes: Vec<PShape>,
    /// 가로 선(y, x0, x1) — 표 행 나누기
    pub hrules: Vec<(f32, f32, f32)>,
    /// 세로 선(x, top, bot) — 표 열 나누기
    pub vrules: Vec<(f32, f32, f32)>,
    pub columns: u8,
    /// 스캔본(또는 그림만 있는 쪽) → 쪽 그림 JPEG
    pub scanned: Option<Vec<u8>>,
    /// 뺀 머리글·바닥글 줄 수
    pub dropped: usize,
}

#[derive(Debug, Clone)]
pub struct DocLayout {
    pub pages: Vec<PageLayout>,
    /// 본문 글자 크기(글자 수로 가중한 최빈값)
    pub body_size: f32,
    /// 이 PDF가 줄 끝 빈칸을 글자로 남기는지(남기면 빈칸이 없는 줄바꿈은 낱말 안에서 끊긴 것)
    pub trailing_spaces: bool,
}

// ---------- 글자 모으기 ----------

#[derive(Debug, Clone)]
struct Glyph {
    c: char,
    x0: f32,
    x1: f32,
    top: f32,
    bot: f32,
    base: f32,
    size: f32,
    bold: bool,
    italic: bool,
    font: u16,
    color: [u8; 3],
    link: Option<u16>,
    /// 바로 앞에 공백 글자가 있었는지
    space_before: bool,
}

/// 틀 변환: 사용자 좌표(y 위로) → 글자 방향 틀(y 아래로). q = 글자 각도 / 90°(반시계)
#[derive(Debug, Clone, Copy)]
struct Frame {
    q: u8,
    ox: f32,
    oy: f32,
}

impl Frame {
    fn raw(q: u8, x: f32, y: f32) -> (f32, f32) {
        match q & 3 {
            0 => (x, -y),
            1 => (y, x),
            2 => (-x, y),
            _ => (-y, -x),
        }
    }
    fn new(q: u8, page: (f32, f32, f32, f32)) -> (Self, f32, f32) {
        let (l, b, r, t) = page;
        let pts = [(l, b), (l, t), (r, b), (r, t)].map(|(x, y)| Self::raw(q, x, y));
        let minx = pts.iter().map(|p| p.0).fold(f32::MAX, f32::min);
        let maxx = pts.iter().map(|p| p.0).fold(f32::MIN, f32::max);
        let miny = pts.iter().map(|p| p.1).fold(f32::MAX, f32::min);
        let maxy = pts.iter().map(|p| p.1).fold(f32::MIN, f32::max);
        (Self { q, ox: minx, oy: miny }, maxx - minx, maxy - miny)
    }
    fn pt(&self, x: f32, y: f32) -> (f32, f32) {
        let (a, b) = Self::raw(self.q, x, y);
        (a - self.ox, b - self.oy)
    }
    /// 사용자 좌표 사각형 → 틀 사각형(x0, top, x1, bot)
    fn rect(&self, l: f32, b: f32, r: f32, t: f32) -> (f32, f32, f32, f32) {
        let p1 = self.pt(l, b);
        let p2 = self.pt(r, t);
        (p1.0.min(p2.0), p1.1.min(p2.1), p1.0.max(p2.0), p1.1.max(p2.1))
    }
}

fn strip_subset(name: &str) -> String {
    match name.split_once('+') {
        Some((pre, rest)) if pre.len() == 6 && pre.chars().all(|c| c.is_ascii_uppercase()) => rest.to_string(),
        _ => name.to_string(),
    }
}

/// PDF 글꼴 이름 → Word/PowerPoint 글꼴(라틴, 동아시아). None이면 기본(맑은 고딕).
pub fn map_font(pdf_font: &str) -> Option<(String, String)> {
    let l = strip_subset(pdf_font).to_lowercase().replace([' ', '-', '_'], "");
    let has = |ks: &[&str]| ks.iter().any(|k| l.contains(k));
    let pair = |a: &str, b: &str| Some((a.to_string(), b.to_string()));
    if has(&["malgun", "맑은"]) {
        None
    } else if has(&["batang", "바탕"]) {
        pair("Batang", "바탕")
    } else if has(&["gulim", "굴림"]) {
        pair("Gulim", "굴림")
    } else if has(&["dotum", "돋움"]) {
        pair("Dotum", "돋움")
    } else if has(&["gungsuh", "궁서"]) {
        pair("Gungsuh", "궁서")
    } else if has(&["myeongjo", "명조", "serifcjk", "serifkr", "notoserif", "sourcehanserif", "hymyeong", "hysm"]) {
        pair("Batang", "바탕")
    } else if has(&["courier", "consola", "mono", "d2coding", "cascadia"]) {
        pair("Courier New", "맑은 고딕")
    } else if has(&["times", "tinos", "liberationserif", "dejavuserif", "nimbusrom"]) {
        pair("Times New Roman", "바탕")
    } else if has(&["georgia"]) {
        pair("Georgia", "바탕")
    } else if has(&["cambria", "caladea"]) {
        pair("Cambria", "바탕")
    } else if has(&["calibri", "carlito"]) {
        pair("Calibri", "맑은 고딕")
    } else if has(&["arial", "helvetica", "arimo", "liberationsans", "nimbussans"]) {
        pair("Arial", "맑은 고딕")
    } else if has(&["verdana"]) {
        pair("Verdana", "맑은 고딕")
    } else if has(&["tahoma"]) {
        pair("Tahoma", "맑은 고딕")
    } else if has(&["segoe"]) {
        pair("Segoe UI", "맑은 고딕")
    } else {
        // 나눔고딕·Noto Sans·본고딕·애플 고딕 등 고딕 계열과 모르는 글꼴은 맑은 고딕
        None
    }
}

pub fn is_hangul(c: char) -> bool {
    ('\u{AC00}'..='\u{D7A3}').contains(&c) || ('\u{1100}'..='\u{11FF}').contains(&c) || ('\u{3130}'..='\u{318F}').contains(&c)
}

fn is_cjk(c: char) -> bool {
    is_hangul(c) || ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3040}'..='\u{30FF}').contains(&c)
}

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

// ---------- 쪽 하나 읽기 ----------

struct RawPage {
    glyphs: Vec<Glyph>,
    fonts: Vec<String>,
    links: Vec<String>,
    images: Vec<PImage>,
    shapes: Vec<PShape>,
    vrules: Vec<(f32, f32, f32)>,
    hrules: Vec<(f32, f32, f32)>,
    width: f32,
    height: f32,
    upright: bool,
    big_image: bool,
    paths: usize,
    /// 작은 칠한 점(도형 글머리표 후보): (x0, x1, 가운데 y)
    dots: Vec<(f32, f32, f32)>,
}

fn color_of(c: PdfColor) -> [u8; 3] {
    [c.red(), c.green(), c.blue()]
}

fn read_page(page: &PdfPage, want_images: bool) -> std::result::Result<RawPage, PdfiumError> {
    let text = page.text()?;
    struct Raw {
        c: char,
        l: f32,
        b: f32,
        r: f32,
        t: f32,
        ox: f32,
        oy: f32,
        size: f32,
        bold: bool,
        italic: bool,
        font: String,
        color: [u8; 3],
        angle: f32,
        space: bool,
    }
    let mut raws: Vec<Raw> = Vec::new();
    for ch in text.chars().iter() {
        let Some(c) = ch.unicode_char() else { continue };
        if c == '\r' || c == '\n' || c == '\u{2}' || c == '\u{FFFE}' || c == '\0' {
            continue;
        }
        if matches!(ch.render_mode(), Ok(PdfPageTextRenderMode::Invisible)) {
            continue;
        }
        let Ok(bd) = ch.loose_bounds() else { continue };
        let space = c.is_whitespace() || c == '\u{A0}';
        let (ox, oy) = ch.origin().map(|(x, y)| (x.value, y.value)).unwrap_or((bd.left().value, bd.bottom().value));
        let font = ch.font_name();
        let fl = font.to_lowercase();
        let weight = ch.font_weight().map(|w| matches!(w, PdfFontWeight::Weight600 | PdfFontWeight::Weight700Bold | PdfFontWeight::Weight800 | PdfFontWeight::Weight900 | PdfFontWeight::Custom(600..))).unwrap_or(false);
        let bold = weight || fl.contains("bold") || fl.contains("black") || fl.contains("heavy") || ch.font_is_bold_reenforced();
        let italic = ch.font_is_italic() || fl.contains("italic") || fl.contains("oblique");
        raws.push(Raw {
            c,
            l: bd.left().value,
            b: bd.bottom().value,
            r: bd.right().value,
            t: bd.top().value,
            ox,
            oy,
            size: ch.scaled_font_size().value,
            bold,
            italic,
            font,
            color: ch.fill_color().map(color_of).unwrap_or([0, 0, 0]),
            angle: ch.angle_degrees().unwrap_or(0.0),
            space,
        });
    }
    // 글자 방향: 공백이 아닌 글자 각도의 최빈 사분면
    let mut qcount = [0usize; 4];
    for r in raws.iter().filter(|r| !r.space) {
        let q = ((r.angle.rem_euclid(360.0) + 45.0) / 90.0).floor() as usize % 4;
        qcount[q] += 1;
    }
    let q = (0..4).max_by_key(|i| (qcount[*i], usize::from(*i == 0))).unwrap_or(0) as u8;
    let ub = page.boundaries().crop().or_else(|_| page.boundaries().media()).map(|b| b.bounds);
    let (pl, pb, pr, pt) = match ub {
        Ok(r) => (r.left().value, r.bottom().value, r.right().value, r.top().value),
        Err(_) => (0.0, 0.0, page.width().value, page.height().value),
    };
    let (fr, width, height) = Frame::new(q, (pl, pb, pr, pt));
    let rot = match page.rotation() {
        Ok(PdfPageRenderRotation::Degrees90) => 1,
        Ok(PdfPageRenderRotation::Degrees180) => 2,
        Ok(PdfPageRenderRotation::Degrees270) => 3,
        _ => 0,
    };
    let upright = rot == q;

    // 링크
    let mut links: Vec<String> = Vec::new();
    let mut link_rects: Vec<((f32, f32, f32, f32), u16)> = Vec::new();
    for link in page.links().iter() {
        let Ok(rc) = link.rect() else { continue };
        let uri = link.action().and_then(|a| a.as_uri_action().and_then(|u| u.uri().ok()));
        let Some(uri) = uri.filter(|u| u.starts_with("http://") || u.starts_with("https://") || u.starts_with("mailto:")) else { continue };
        let k = match links.iter().position(|x| *x == uri) {
            Some(k) => k,
            None => {
                links.push(uri);
                links.len() - 1
            }
        };
        link_rects.push((fr.rect(rc.left().value, rc.bottom().value, rc.right().value, rc.top().value), k as u16));
    }

    let mut fonts: Vec<String> = Vec::new();
    let mut glyphs: Vec<Glyph> = Vec::new();
    let mut pending_space = false;
    for r in raws {
        if r.space {
            pending_space = true;
            continue;
        }
        let (x0, top, x1, bot) = fr.rect(r.l, r.b, r.r, r.t);
        let (_, base) = fr.pt(r.ox, r.oy);
        let fname = strip_subset(&r.font);
        let font = match fonts.iter().position(|f| *f == fname) {
            Some(k) => k,
            None => {
                fonts.push(fname);
                fonts.len() - 1
            }
        } as u16;
        let (cx, cy) = ((x0 + x1) / 2.0, (top + bot) / 2.0);
        let link = link_rects.iter().find(|((a, b, c, d), _)| cx >= *a && cx <= *c && cy >= *b - 1.0 && cy <= *d + 1.0).map(|(_, k)| *k);
        // 크기가 0에 가까우면 상자 높이로
        let size = if r.size > 0.5 { r.size } else { (bot - top).max(1.0) };
        glyphs.push(Glyph { c: r.c, x0, x1, top, bot, base, size, bold: r.bold, italic: r.italic, font, color: r.color, link, space_before: pending_space });
        pending_space = false;
    }

    // 그림·선·도형
    let area = width * height;
    let mut images = Vec::new();
    let mut shapes = Vec::new();
    let mut vrules = Vec::new();
    let mut hrules = Vec::new();
    let mut big_image = false;
    let mut paths = 0usize;
    let mut dots = Vec::new();
    for obj in page.objects().iter() {
        let Ok(bd) = obj.bounds() else { continue };
        let (x0, top, x1, bot) = fr.rect(bd.left().value, bd.bottom().value, bd.right().value, bd.top().value);
        let (w, h) = (x1 - x0, bot - top);
        if let Some(img) = obj.as_image_object() {
            if w * h > 0.5 * area {
                big_image = true;
            }
            if !want_images || w < 4.0 || h < 4.0 {
                continue;
            }
            let is_jpeg = img.filters().iter().any(|f| f.name() == "DCTDecode") && img.filters().len() == 1;
            let (bytes, ext) = if is_jpeg {
                (img.get_raw_image_data().unwrap_or_default(), "jpg")
            } else {
                let mut v = Vec::new();
                if let Ok(im) = img.get_raw_image() {
                    let _ = im.write_to(&mut Cursor::new(&mut v), image::ImageFormat::Png);
                }
                (v, "png")
            };
            if !bytes.is_empty() {
                images.push(PImage { x0, top, w, h, bytes, ext, col: 0, background: w * h > 0.85 * area });
            }
        } else if let Some(p) = obj.as_path_object() {
            paths += 1;
            if (1.5..=8.0).contains(&w) && (1.5..=8.0).contains(&h) && (w / h - 1.0).abs() < 0.35 && p.fill_mode().map(|m| m != PdfPathFillMode::None).unwrap_or(false) {
                dots.push((x0, x1, (top + bot) / 2.0));
            }
            let fill = obj.fill_color().ok().filter(|c| c.alpha() > 0).map(color_of);
            let filled = p.fill_mode().map(|m| m != PdfPathFillMode::None).unwrap_or(false);
            let stroked = p.is_stroked().unwrap_or(false);
            let sw = obj.stroke_width().map(|v| v.value).unwrap_or(1.0).max(0.1);
            let stroke = if stroked { obj.stroke_color().ok().filter(|c| c.alpha() > 0).map(|c| (color_of(c), sw)) } else { None };
            // 얇은 사각형·선 → 표의 선
            if w <= 2.5 && h > 4.0 && (filled || stroked) {
                vrules.push(((x0 + x1) / 2.0, top, bot));
            } else if h <= 2.5 && w > 4.0 && (filled || stroked) {
                hrules.push(((top + bot) / 2.0, x0, x1));
            } else if stroked && !filled {
                // 테두리만 그린 사각형(표 칸): 네 변을 선으로
                hrules.push((top, x0, x1));
                hrules.push((bot, x0, x1));
                vrules.push((x0, top, bot));
                vrules.push((x1, top, bot));
            }
            if (w * h) < 0.9 * area {
                if w <= 2.5 || h <= 2.5 {
                    let c = fill.or(stroke.map(|s| s.0)).unwrap_or([0, 0, 0]);
                    if w > h {
                        shapes.push(PShape::Line { x1: x0, y1: (top + bot) / 2.0, x2: x1, y2: (top + bot) / 2.0, color: c, width: h.max(sw).min(2.5) });
                    } else {
                        shapes.push(PShape::Line { x1: (x0 + x1) / 2.0, y1: top, x2: (x0 + x1) / 2.0, y2: bot, color: c, width: w.max(sw).min(2.5) });
                    }
                } else if (filled && fill.is_some_and(|c| c != [255, 255, 255])) || stroke.is_some() {
                    shapes.push(PShape::Rect { x0, top, w, h, fill: if filled { fill.filter(|c| *c != [255, 255, 255]) } else { None }, stroke });
                }
            }
        }
    }
    // 칸마다 따로 그린 선 조각을 이어 붙인다(같은 위치, 끊김 3pt 이하)
    let vrules = merge_rules(vrules);
    let hrules = merge_rules(hrules);
    Ok(RawPage { glyphs, fonts, links, images, shapes, vrules, hrules, width, height, upright, big_image, paths, dots })
}

/// (위치, 시작, 끝) 선 조각 합치기
fn merge_rules(mut v: Vec<(f32, f32, f32)>) -> Vec<(f32, f32, f32)> {
    v.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let mut out: Vec<(f32, f32, f32)> = Vec::new();
    for r in v {
        match out.last_mut() {
            Some(o) if (o.0 - r.0).abs() < 1.0 && r.1 <= o.2 + 3.0 => o.2 = o.2.max(r.2),
            _ => out.push(r),
        }
    }
    out
}

// ---------- 줄 만들기 ----------

/// 같은 기준선 글자 묶음(행) → 칸 나누기
fn build_rows(mut gs: Vec<Glyph>) -> Vec<Vec<Glyph>> {
    gs.sort_by(|a, b| a.base.total_cmp(&b.base).then(a.x0.total_cmp(&b.x0)));
    let mut rows: Vec<Vec<Glyph>> = Vec::new();
    for g in gs {
        // 같은 행: 기준선 차이가 글자 크기 절반 미만(위첨자·아래첨자 포함)
        let target = rows.iter_mut().rev().take(3).find(|r| {
            let rb = r[0].base;
            let rs = r[0].size.max(g.size);
            (rb - g.base).abs() < 0.45 * rs
        });
        match target {
            Some(r) => r.push(g),
            None => rows.push(vec![g]),
        }
    }
    for r in rows.iter_mut() {
        r.sort_by(|a, b| a.x0.total_cmp(&b.x0));
    }
    rows
}

#[derive(Debug, Clone)]
struct Seg {
    glyphs: Vec<Glyph>,
    x0: f32,
    x1: f32,
    top: f32,
    bot: f32,
    base: f32,
    size: f32,
}

fn seg_of(glyphs: Vec<Glyph>) -> Seg {
    let x0 = glyphs.iter().map(|g| g.x0).fold(f32::MAX, f32::min);
    let x1 = glyphs.iter().map(|g| g.x1).fold(f32::MIN, f32::max);
    let top = glyphs.iter().map(|g| g.top).fold(f32::MAX, f32::min);
    let bot = glyphs.iter().map(|g| g.bot).fold(f32::MIN, f32::max);
    let size = median(glyphs.iter().map(|g| g.size).collect());
    let base = median(glyphs.iter().map(|g| g.base).collect());
    Seg { glyphs, x0, x1, top, bot, base, size }
}

fn is_bullet_char(c: char) -> bool {
    matches!(c, '•' | '·' | '▪' | '◦' | '●' | '■' | '□' | '○' | '◆' | '◇' | '▶' | '►' | '-' | '–' | '—' | '※' | '✓' | '✔' | '\u{F0B7}' | '\u{F0A7}' | '\u{F076}' | '\u{F0D8}' | '\u{F0FC}') || ('\u{E000}'..='\u{F8FF}').contains(&c)
}

fn is_list_marker(s: &str) -> bool {
    let t = s.trim();
    let mut cs = t.chars();
    match (cs.next(), cs.next()) {
        (Some(c), None) => is_bullet_char(c),
        _ => {
            // 1. 1) (1) 가. a) i.
            let core = t.trim_start_matches('(');
            let digits: String = core.chars().take_while(|c| c.is_ascii_digit()).collect();
            let tail: String = core.chars().skip(digits.chars().count()).collect();
            (!digits.is_empty() && digits.len() <= 3 && matches!(tail.as_str(), "." | ")"))
                || (core.chars().count() == 2 && core.chars().next().is_some_and(|c| "가나다라마바사아자차카타파하".contains(c) || c.is_ascii_lowercase()) && matches!(core.chars().nth(1), Some('.') | Some(')')))
        }
    }
}

/// 행을 칸(seg)으로 나눈다: 넓은 틈(글자 크기의 1.2배 넘게) 또는 그 사이에 세로 선이 있으면
fn split_row(row: Vec<Glyph>, vrules: &[(f32, f32, f32)]) -> Vec<Seg> {
    let mut segs: Vec<Vec<Glyph>> = Vec::new();
    for g in row {
        let split = match segs.last().and_then(|s| s.last()) {
            None => true,
            Some(p) => {
                let gap = g.x0 - p.x1;
                let sz = p.size.max(g.size);
                let mid_y = (g.top + g.bot) / 2.0;
                let rule = vrules.iter().any(|(x, t, b)| *x > p.x1 - 0.5 && *x < g.x0 + 0.5 && mid_y >= *t - 1.0 && mid_y <= *b + 1.0);
                rule || gap > 1.2 * sz
            }
        };
        if split {
            segs.push(vec![g]);
        } else if let Some(s) = segs.last_mut() {
            s.push(g);
        }
    }
    let mut out: Vec<Seg> = segs.into_iter().map(seg_of).collect();
    // 글머리표·번호와 글이 넓게 떨어진 경우(목록) → 한 칸으로
    if out.len() >= 2 {
        let first: String = out[0].glyphs.iter().map(|g| g.c).collect();
        if is_list_marker(&first) && out[1].x0 - out[0].x1 < 4.0 * out[1].size {
            let a = out.remove(0);
            let mut b = out.remove(0);
            let mut gs = a.glyphs;
            if let Some(g) = b.glyphs.first_mut() {
                g.space_before = true;
            }
            gs.append(&mut b.glyphs);
            out.insert(0, seg_of(gs));
        }
    }
    out
}

fn spans_of(glyphs: &[Glyph], fonts: &[String], links: &[String]) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut prev: Option<&Glyph> = None;
    for g in glyphs {
        let mut text = String::new();
        if let Some(p) = prev {
            let gap = g.x0 - p.x1;
            if g.space_before || gap > 0.22 * p.size.max(g.size) {
                text.push(' ');
            }
        }
        text.push(g.c);
        let font = fonts.get(g.font as usize).cloned().unwrap_or_default();
        let link = g.link.and_then(|k| links.get(k as usize).cloned());
        match spans.last_mut() {
            Some(s) if s.bold == g.bold && s.italic == g.italic && s.font == font && s.color == g.color && s.link == link && (s.size - g.size).abs() < 0.6 => s.text.push_str(&text),
            _ => {
                // 빈칸은 앞 span 끝에 붙인다(링크·굵게 범위에 빈칸이 들어가지 않게)
                if text.starts_with(' ') {
                    if let Some(s) = spans.last_mut() {
                        s.text.push(' ');
                        text.remove(0);
                    }
                }
                spans.push(Span { text, bold: g.bold, italic: g.italic, size: g.size, font, color: g.color, link });
            }
        }
        prev = Some(g);
    }
    spans
}

fn line_of(segs: Vec<Seg>, col: u8, fonts: &[String], links: &[String]) -> Line {
    let x0 = segs.iter().map(|s| s.x0).fold(f32::MAX, f32::min);
    let x1 = segs.iter().map(|s| s.x1).fold(f32::MIN, f32::max);
    let top = segs.iter().map(|s| s.top).fold(f32::MAX, f32::min);
    let bot = segs.iter().map(|s| s.bot).fold(f32::MIN, f32::max);
    let all: Vec<&Glyph> = segs.iter().flat_map(|s| s.glyphs.iter()).collect();
    let size = median(all.iter().map(|g| g.size).collect());
    let base = median(all.iter().map(|g| g.base).collect());
    let bold = all.iter().filter(|g| g.bold).count() * 2 > all.len();
    let cells = segs.iter().map(|s| Cell { x0: s.x0, x1: s.x1, spans: spans_of(&s.glyphs, fonts, links) }).collect();
    let lead_space = segs.first().and_then(|s| s.glyphs.first()).is_some_and(|g| g.space_before);
    Line { x0, x1, top, bot, base, size, bold, cells, col, lead_space, bullet: false }
}

/// 단 나누기: 가운데 쪽(25~75%)에 세로 빈 띠가 있고 양쪽에 긴 글줄이 4개 이상이면 그 띠
fn find_gutter(segs: &[Seg], width: f32) -> Option<(f32, f32)> {
    let long: Vec<&Seg> = segs.iter().filter(|s| s.glyphs.len() >= 8 && (s.x1 - s.x0) > width * 0.18).collect();
    if long.len() < 8 {
        return None;
    }
    let bins = width.ceil().max(1.0) as usize;
    let mut cover = vec![0u32; bins + 1];
    for s in &long {
        let a = s.x0.max(0.0) as usize;
        let b = (s.x1.max(0.0) as usize).min(bins);
        for c in cover.iter_mut().take(b + 1).skip(a) {
            *c += 1;
        }
    }
    let lo = (width * 0.25) as usize;
    let hi = ((width * 0.75) as usize).min(bins);
    // 가장 넓은 '거의 빈'(가로지르는 긴 줄이 전체의 10% 이하) 띠
    let limit = (long.len() as f32 * 0.1).floor() as u32;
    let mut best: Option<(usize, usize)> = None;
    let mut i = lo;
    while i < hi {
        if cover[i] <= limit {
            let s = i;
            while i < hi && cover[i] <= limit {
                i += 1;
            }
            if best.is_none_or(|(a, b)| i - s > b - a) {
                best = Some((s, i));
            }
        }
        i += 1;
    }
    let (g0, g1) = best?;
    if g1 - g0 < 8 {
        return None;
    }
    let (g0, g1) = (g0 as f32, g1 as f32);
    let left = long.iter().filter(|s| s.x1 <= g0 + 1.0).count();
    let right = long.iter().filter(|s| s.x0 >= g1 - 1.0).count();
    (left >= 4 && right >= 4).then_some((g0, g1))
}

fn analyze_text(raw: &RawPage) -> (Vec<Line>, u8) {
    let rows = build_rows(raw.glyphs.clone());
    let mut segs: Vec<Seg> = Vec::new();
    for r in rows {
        segs.extend(split_row(r, &raw.vrules));
    }
    let gutter = find_gutter(&segs, raw.width);
    let col_of = |s: &Seg| -> u8 {
        match gutter {
            Some((g0, _)) if s.x1 <= g0 + 2.0 => 1,
            Some((_, g1)) if s.x0 >= g1 - 2.0 => 2,
            _ => 0,
        }
    };
    // 줄: 같은 단, 같은 기준선의 칸들
    let mut tagged: Vec<(u8, Seg)> = segs.into_iter().map(|s| (col_of(&s), s)).collect();
    tagged.sort_by(|a, b| a.1.base.total_cmp(&b.1.base).then(a.0.cmp(&b.0)).then(a.1.x0.total_cmp(&b.1.x0)));
    let mut groups: Vec<(u8, Vec<Seg>)> = Vec::new();
    for (c, s) in tagged {
        let hit = groups.iter_mut().rev().take(4).find(|(gc, g)| *gc == c && (g[0].base - s.base).abs() < 0.45 * g[0].size.max(s.size));
        match hit {
            Some((_, g)) => g.push(s),
            None => groups.push((c, vec![s])),
        }
    }
    let mut lines: Vec<Line> = groups
        .into_iter()
        .map(|(c, mut g)| {
            g.sort_by(|a, b| a.x0.total_cmp(&b.x0));
            line_of(g, c, &raw.fonts, &raw.links)
        })
        .filter(|l| l.chars() > 0)
        .collect();
    lines.sort_by(|a, b| a.top.total_cmp(&b.top));
    // 도형 글머리표: 줄 바로 왼쪽(글자 크기 3배 안)에 같은 높이의 작은 점
    for l in lines.iter_mut() {
        l.bullet = raw.dots.iter().any(|(a, b, cy)| *cy >= l.top - 1.0 && *cy <= l.bot + 1.0 && *b <= l.x0 + 0.5 && *b > l.x0 - 3.0 * l.size && *a < l.x0);
    }
    (lines, if gutter.is_some() { 2 } else { 1 })
}

/// 단을 고려한 읽기 순서: 가로지르는 항목(col 0)이 나오면, 그 앞의 왼쪽 단 → 오른쪽 단 순서로 내보낸다
fn reading_order(mut items: Vec<Item>) -> Vec<Item> {
    items.sort_by(|a, b| a.top().total_cmp(&b.top()));
    let col = |i: &Item| match i {
        Item::Line(l) => l.col,
        Item::Image(m) => m.col,
    };
    let mut out = Vec::new();
    let mut left = Vec::new();
    let mut right = Vec::new();
    for it in items {
        match col(&it) {
            1 => left.push(it),
            2 => right.push(it),
            _ => {
                out.append(&mut left);
                out.append(&mut right);
                out.push(it);
            }
        }
    }
    out.append(&mut left);
    out.append(&mut right);
    out
}

fn norm_key(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).map(|c| if c.is_ascii_digit() { '#' } else { c }).collect()
}

/// 쪽 번호 모양: "3", "- 3 -", "3 / 10", "3쪽", "Page 3", "p. 3"
pub fn is_page_number(s: &str) -> bool {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
    if t.is_empty() || t.chars().count() > 14 || !t.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let stripped = t.trim_start_matches("page").trim_start_matches("p.").trim_matches(|c| matches!(c, '-' | '–' | '—' | '[' | ']' | '(' | ')' | '|'));
    let stripped = stripped.trim_end_matches("쪽").trim_end_matches("페이지").trim_end_matches('p');
    stripped.chars().all(|c| c.is_ascii_digit() || c == '/' || c == '|') || {
        let parts: Vec<&str> = stripped.split("of").collect();
        parts.len() == 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    }
}

/// 바닥글 끝의 쪽 번호: "… · 3 쪽", "… - 3 -", "… | 3", "… Page 3"
fn ends_with_page_number(s: &str) -> bool {
    let t = s.trim();
    for sep in ['·', '|', '-', '—', '–', '/'] {
        if let Some((head, tail)) = t.rsplit_once(sep) {
            if !head.trim().is_empty() && is_page_number(tail) && tail.trim().chars().any(|c| c.is_ascii_digit()) && !tail.trim().ends_with('%') {
                // "2026 - 3" 같은 날짜·범위는 빼지 않게, 꼬리는 쪽 표시가 붙었거나 짧은 숫자만
                let tt = tail.trim();
                if tt.ends_with('쪽') || tt.ends_with("페이지") || tt.to_lowercase().starts_with("page") || tt.chars().filter(|c| c.is_ascii_digit()).count() <= 3 {
                    return true;
                }
            }
        }
    }
    false
}

/// 여러 쪽에 되풀이되는 위·아래 줄과 쪽 번호를 뺀다
fn drop_running(pages: &mut [PageLayout]) {
    let n = pages.iter().filter(|p| p.scanned.is_none()).count();
    let band = |p: &PageLayout, l: &Line| l.bot < p.height * 0.1 || l.top > p.height * 0.9;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for p in pages.iter() {
        let mut seen = std::collections::HashSet::new();
        for it in &p.items {
            if let Item::Line(l) = it {
                if band(p, l) {
                    let k = norm_key(&l.text());
                    if seen.insert(k.clone()) {
                        *counts.entry(k).or_default() += 1;
                    }
                }
            }
        }
    }
    let need = if n >= 3 { (n as f32 * 0.5).ceil() as usize } else { 2 };
    for p in pages.iter_mut() {
        let h = p.height;
        let before = p.items.len();
        p.items.retain(|it| match it {
            Item::Line(l) => {
                let in_band = l.bot < h * 0.1 || l.top > h * 0.9;
                !(in_band && (is_page_number(&l.text()) || ends_with_page_number(&l.text()) || (n >= 2 && counts.get(&norm_key(&l.text())).copied().unwrap_or(0) >= need)))
            }
            Item::Image(_) => true,
        });
        p.dropped += before - p.items.len();
    }
}

/// 쪽 그림(JPEG, 150dpi)
pub(crate) fn render_jpeg(page: &PdfPage, dpi: f32, quality: u8) -> std::result::Result<Vec<u8>, PdfiumError> {
    let cfg = PdfRenderConfig::new().scale_page_by_factor(dpi / 72.0).render_form_data(true).set_clear_color(PdfColor::WHITE);
    let im = page.render_with_config(&cfg)?.as_image()?.to_rgb8();
    let mut v = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut v, quality).encode_image(&im).map_err(|_| PdfiumError::ImageError)?;
    Ok(v)
}

pub struct AnalyzeOpts {
    /// 그림 바이트를 꺼낼지(TXT는 필요 없음)
    pub images: bool,
    /// 스캔본 쪽을 그림으로 그릴지
    pub render_scanned: bool,
}

/// PDF 전체 분석. 진행률은 쪽 단위.
pub fn analyze(input: &Path, opts: &AnalyzeOpts, cancel: Option<&Cancel>, progress: &mut dyn FnMut(u32, u32)) -> Result<DocLayout> {
    let pdfium = crate::pdfrender::pdfium()?;
    let doc = pdfium.load_pdf_from_file(input, None).map_err(|e| crate::pdfrender::open_err(input, e))?;
    analyze_doc(input, &doc, opts, cancel, progress)
}

pub(crate) fn analyze_doc(input: &Path, doc: &PdfDocument, opts: &AnalyzeOpts, cancel: Option<&Cancel>, progress: &mut dyn FnMut(u32, u32)) -> Result<DocLayout> {
    let n = doc.pages().len() as u32;
    if n == 0 {
        return Err(Error::Pdf { path: input.into(), message: "쪽이 없습니다".into() });
    }
    let perr = |e: PdfiumError| Error::Pdf { path: input.into(), message: format!("쪽을 읽지 못했습니다: {e}") };
    let mut pages = Vec::new();
    for (i, page) in doc.pages().iter().enumerate() {
        if let Some(c) = cancel {
            c.check()?;
        }
        let raw = read_page(&page, opts.images).map_err(perr)?;
        let nonspace = raw.glyphs.len();
        let drawing_only = nonspace < 20 && raw.images.is_empty() && raw.paths > 60;
        let scanned = if nonspace < 20 && (raw.big_image || drawing_only) {
            Some(if opts.render_scanned { render_jpeg(&page, 150.0, 85).map_err(perr)? } else { Vec::new() })
        } else {
            None
        };
        let (lines, columns) = if scanned.is_some() { (Vec::new(), 1) } else { analyze_text(&raw) };
        let mut items: Vec<Item> = lines.into_iter().map(Item::Line).collect();
        if scanned.is_none() {
            let gutter = if columns == 2 {
                // 그림의 단: 왼쪽 단 줄들의 오른쪽 끝 / 오른쪽 단 줄들의 왼쪽 끝 기준
                let lmax = items.iter().filter_map(|i| if let Item::Line(l) = i { (l.col == 1).then_some(l.x1) } else { None }).fold(f32::MIN, f32::max);
                let rmin = items.iter().filter_map(|i| if let Item::Line(l) = i { (l.col == 2).then_some(l.x0) } else { None }).fold(f32::MAX, f32::min);
                Some((lmax, rmin))
            } else {
                None
            };
            for mut im in raw.images.into_iter() {
                if let Some((lmax, rmin)) = gutter {
                    im.col = if im.x0 + im.w <= lmax + 4.0 { 1 } else if im.x0 >= rmin - 4.0 { 2 } else { 0 };
                }
                items.push(Item::Image(im));
            }
        }
        let items = reading_order(items);
        let (dw, dh) = (page.width().value, page.height().value);
        pages.push(PageLayout {
            index: i,
            width: raw.width,
            height: raw.height,
            disp_w: dw,
            disp_h: dh,
            upright: raw.upright,
            items,
            shapes: raw.shapes,
            hrules: raw.hrules,
            vrules: raw.vrules,
            columns,
            scanned,
            dropped: 0,
        });
        progress(i as u32 + 1, n);
    }
    drop_running(&mut pages);
    let mut hist: HashMap<i32, usize> = HashMap::new();
    for p in &pages {
        for it in &p.items {
            if let Item::Line(l) = it {
                *hist.entry((l.size * 2.0).round() as i32).or_default() += l.chars();
            }
        }
    }
    let body_size = hist.iter().max_by_key(|(s, n)| (**n, -**s)).map(|(s, _)| *s as f32 / 2.0).unwrap_or(10.0);
    let trailing_spaces = pages.iter().flat_map(|p| p.items.iter()).filter(|i| matches!(i, Item::Line(l) if l.lead_space)).count() >= 2;
    Ok(DocLayout { pages, body_size, trailing_spaces })
}

// ---------- 문단·표·제목 다시 만들기(Word·TXT 공용) ----------

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading { level: u32, spans: Vec<Span> },
    Para { spans: Vec<Span>, list: Option<bool> },
    Table { rows: Vec<Vec<Vec<Span>>> },
    Image { bytes: Vec<u8>, ext: &'static str, w: f32, h: f32 },
    /// 스캔본 쪽 그림(JPEG) + 쪽 크기(pt)
    PageImage { jpeg: Vec<u8>, w: f32, h: f32 },
    PageBreak { page: usize },
    /// 새 쪽 시작(TXT 쪽 표시용, Word는 무시)
    PageStart { page: usize },
}

pub fn spans_text(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

fn line_spans(l: &Line) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for (i, c) in l.cells.iter().enumerate() {
        if i > 0 {
            if let Some(s) = out.last_mut() {
                s.text.push(' ');
            }
        }
        out.extend(c.spans.iter().cloned());
    }
    out
}

/// 문단 이어 붙이기: 한글끼리 글자 단위로 끊긴 줄이면 빈칸 없이
fn join_spans(dst: &mut Vec<Span>, mut next: Vec<Span>, no_space: bool) {
    let last_ch = dst.iter().rev().find_map(|s| s.text.trim_end().chars().last());
    // 줄 끝 하이픈으로 나뉜 영어 낱말
    let hyphen = last_ch == Some('-') && next.first().and_then(|s| s.text.chars().next()).is_some_and(|c| c.is_ascii_lowercase());
    if let Some(s) = dst.last_mut() {
        let t = s.text.trim_end().to_string();
        s.text = t;
        if hyphen {
            s.text.pop();
        } else if !no_space {
            s.text.push(' ');
        }
    }
    if let Some(f) = next.first_mut() {
        f.text = f.text.trim_start().to_string();
    }
    for sp in next {
        match dst.last_mut() {
            Some(d) if d.bold == sp.bold && d.italic == sp.italic && d.font == sp.font && d.color == sp.color && d.link == sp.link && (d.size - sp.size).abs() < 0.6 => d.text.push_str(&sp.text),
            _ => dst.push(sp),
        }
    }
}

fn strip_marker(spans: &mut Vec<Span>) -> Option<bool> {
    let text = spans_text(spans);
    let t = text.trim_start();
    let first = t.chars().next()?;
    let ordered;
    let cut;
    if is_bullet_char(first) && t.chars().nth(1).is_some_and(|c| c.is_whitespace() || is_hangul(c) || c.is_alphanumeric()) {
        // "-5%" 같은 음수는 목록이 아니다
        if matches!(first, '-' | '–' | '—') && t.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) {
            return None;
        }
        ordered = false;
        cut = first.len_utf8();
    } else {
        let word: String = t.split_whitespace().next().unwrap_or("").to_string();
        if !is_list_marker(&word) || word.chars().count() < 2 || t.split_whitespace().nth(1).is_none() {
            return None;
        }
        ordered = true;
        cut = word.len();
    }
    // 앞에서 cut 바이트만큼(앞 공백 포함) 지운다
    let mut remove = text.len() - t.len() + cut;
    while remove > 0 && !spans.is_empty() {
        let s = &mut spans[0];
        if s.text.len() <= remove {
            remove -= s.text.len();
            spans.remove(0);
        } else {
            s.text = s.text[remove..].to_string();
            remove = 0;
        }
    }
    if let Some(s) = spans.first_mut() {
        s.text = s.text.trim_start().to_string();
    }
    Some(ordered)
}

/// 표 후보 줄들 → 행·열. 칸의 가로 범위가 겹치는 것끼리 한 열(가운데 정렬 머리글·오른쪽 정렬 숫자도 같은 열)
fn build_table(lines: &[Line], hrules: &[(f32, f32, f32)], vrules: &[(f32, f32, f32)]) -> Option<Vec<Vec<Vec<Span>>>> {
    let ttop = lines.iter().map(|l| l.top).fold(f32::MAX, f32::min);
    let tbot = lines.iter().map(|l| l.bot).fold(f32::MIN, f32::max);
    // 세로 선이 표 높이 대부분에 걸쳐 있으면 선 사이를 열로(가운데 정렬 머리글도 같은 열)
    let mut xs: Vec<f32> = vrules.iter().filter(|(_, t, b)| *t <= ttop + 2.0 && *b >= tbot - 2.0 || (b.min(tbot) - t.max(ttop)) > (tbot - ttop) * 0.5).map(|v| v.0).collect();
    xs.sort_by(|a, b| a.total_cmp(b));
    xs.dedup_by(|a, b| (*a - *b).abs() < 2.0);
    let mut cols: Vec<(f32, f32)> = Vec::new();
    if xs.len() >= 3 {
        cols = xs.windows(2).map(|w| (w[0], w[1])).collect();
        // 글이 없는 열은 뺀다
        cols.retain(|(a, b)| lines.iter().any(|l| l.cells.iter().any(|c| (c.x0 + c.x1) / 2.0 > *a && (c.x0 + c.x1) / 2.0 < *b)));
    }
    let by_rules = cols.len() >= 2;
    if !by_rules {
        cols.clear();
        let mut iv: Vec<(f32, f32)> = lines.iter().flat_map(|l| l.cells.iter().map(|c| (c.x0, c.x1))).collect();
        iv.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (a, b) in iv {
            match cols.last_mut() {
                Some(c) if a <= c.1 + 1.0 => c.1 = c.1.max(b),
                _ => cols.push((a, b)),
            }
        }
    }
    if cols.len() < 2 {
        return None;
    }
    let col_of = |c: &Cell| {
        let m = (c.x0 + c.x1) / 2.0;
        if by_rules {
            cols.iter().position(|(a, b)| m > *a && m < *b).unwrap_or(0)
        } else {
            cols.iter().position(|(a, b)| c.x0 >= a - 1.0 && c.x0 <= b + 1.0).unwrap_or(0)
        }
    };
    let tx0 = lines.iter().map(|l| l.x0).fold(f32::MAX, f32::min);
    let tx1 = lines.iter().map(|l| l.x1).fold(f32::MIN, f32::max);
    // 표 너비 대부분을 가로지르는 선
    let rules: Vec<f32> = hrules.iter().filter(|(_, a, b)| *a <= tx0 + 20.0 && *b >= tx1 - 20.0 || (b - a) > (tx1 - tx0) * 0.6).map(|r| r.0).collect();
    let mut rows: Vec<Vec<Vec<Span>>> = Vec::new();
    let mut prev: Option<&Line> = None;
    for l in lines {
        let new_row = match prev {
            None => true,
            Some(p) => rules.is_empty() || rules.iter().any(|y| *y > p.base - 0.5 && *y < l.top + 0.5 * l.size),
        };
        if new_row {
            rows.push(vec![Vec::new(); cols.len()]);
        }
        let row = rows.last_mut()?;
        for c in &l.cells {
            let k = col_of(c);
            let dst = &mut row[k];
            let sp = c.spans.clone();
            if dst.is_empty() {
                *dst = sp;
            } else {
                join_spans(dst, sp, false);
            }
        }
        prev = Some(l);
    }
    (rows.len() >= 2).then_some(rows)
}

/// 쪽들 → 블록(제목·문단·목록·표·그림). 표가 다음 쪽으로 넘어가면 이어 붙인다.
pub fn blocks(doc: &DocLayout) -> Vec<Block> {
    blocks_with(doc, false)
}

/// `page_marks`: 쪽마다 [`Block::PageStart`]를 넣고 문단을 쪽에서 끊는다(TXT용). 아니면 쪽을 넘는 문단을 잇는다(Word용).
pub fn blocks_with(doc: &DocLayout, page_marks: bool) -> Vec<Block> {
    let body = doc.body_size;
    let is_head = |l: &Line| -> bool {
        let t = l.text();
        let n = t.chars().count();
        l.cells.len() == 1 && n > 0 && n < 90 && (l.size >= body * 1.15 || (l.bold && l.size >= body * 0.98 && n < 60 && !t.trim_end().ends_with(['.', ',', ':'])))
    };
    // 제목 크기 순위
    let mut hsizes: Vec<i32> = Vec::new();
    let mut first_page_sizes: Vec<i32> = Vec::new();
    let mut size_count: HashMap<i32, usize> = HashMap::new();
    for p in &doc.pages {
        for it in &p.items {
            if let Item::Line(l) = it {
                if is_head(l) {
                    let k = (l.size * 2.0).round() as i32;
                    hsizes.push(k);
                    *size_count.entry(k).or_default() += 1;
                    if p.index == 0 {
                        first_page_sizes.push(k);
                    }
                }
            }
        }
    }
    hsizes.sort_unstable();
    hsizes.dedup();
    hsizes.reverse();
    // 가장 큰 크기가 첫 쪽에 한 번만 → 문서 제목(Title)
    let has_title = hsizes.len() >= 2 && size_count.get(&hsizes[0]) == Some(&1) && first_page_sizes.contains(&hsizes[0]) && hsizes[0] as f32 / 2.0 >= body * 1.4;
    let level_of = |l: &Line| -> u32 {
        let k = (l.size * 2.0).round() as i32;
        let rank = hsizes.iter().position(|s| *s == k).unwrap_or(hsizes.len().saturating_sub(1)) as u32;
        if has_title {
            rank
        } else {
            rank + 1
        }
        .min(3)
        .max(if has_title && rank > 0 { 1 } else { 0 })
    };

    let mut out: Vec<Block> = Vec::new();
    let mut prev_scan = false;
    let flush = |para: &mut Option<(Line, Vec<Span>, Option<bool>)>, out: &mut Vec<Block>| {
        if let Some((_, spans, list)) = para.take() {
            if !spans_text(&spans).trim().is_empty() {
                out.push(Block::Para { spans, list });
            }
        }
    };
    // 앞 쪽에서 이어지는 문단(Word용): (문단, 앞 쪽 오른쪽 끝, 앞 쪽 높이)
    let mut para: Option<(Line, Vec<Span>, Option<bool>)> = None;
    let mut prev_page: Option<(f32, f32)> = None;
    for (pi, p) in doc.pages.iter().enumerate() {
        if page_marks && pi > 0 {
            flush(&mut para, &mut out);
            out.push(Block::PageStart { page: pi });
        }
        if let Some(jpg) = &p.scanned {
            flush(&mut para, &mut out);
            if pi > 0 && !page_marks {
                out.push(Block::PageBreak { page: pi });
            }
            out.push(Block::PageImage { jpeg: jpg.clone(), w: p.disp_w, h: p.disp_h });
            prev_scan = true;
            prev_page = None;
            continue;
        }
        if prev_scan && !page_marks {
            out.push(Block::PageBreak { page: pi });
        }
        prev_scan = false;
        // 단마다 오른쪽 끝(줄이 찼는지 판단)
        let right_of = |col: u8| p.items.iter().filter_map(|i| if let Item::Line(l) = i { (l.col == col).then_some(l.x1) } else { None }).fold(f32::MIN, f32::max);
        let rights = [right_of(0), right_of(1), right_of(2)];
        let carried = prev_page.take();
        let mut first_line = true;
        let items = &p.items;
        let mut i = 0;
        while i < items.len() {
            match &items[i] {
                Item::Image(im) => {
                    if im.background {
                        i += 1;
                        continue;
                    }
                    first_line = false;
                    flush(&mut para, &mut out);
                    out.push(Block::Image { bytes: im.bytes.clone(), ext: im.ext, w: im.w, h: im.h });
                    i += 1;
                }
                Item::Line(l) => {
                    // 표: 칸 2개 이상 줄이 이어짐(가로 선 사이 한 칸 줄도 같은 표로)
                    if l.cells.len() >= 2 {
                        let mut j = i + 1;
                        while j < items.len() {
                            match &items[j] {
                                Item::Line(m) if m.col == l.col && (m.cells.len() >= 2 || (m.cells.len() == 1 && j + 1 < items.len() && matches!(&items[j + 1], Item::Line(n) if n.cells.len() >= 2 && n.col == l.col) && m.x0 > l.x0 + 2.0 * l.size)) => j += 1,
                                _ => break,
                            }
                        }
                        let run: Vec<Line> = items[i..j].iter().filter_map(|x| if let Item::Line(m) = x { Some(m.clone()) } else { None }).collect();
                        if run.len() >= 2 {
                            if let Some(rows) = build_table(&run, &p.hrules, &p.vrules) {
                                first_line = false;
                                flush(&mut para, &mut out);
                                // 앞 쪽 끝 표와 열 수가 같으면 이어 붙임
                                let joined = match out.last_mut() {
                                    Some(Block::Table { rows: prev }) if prev.first().map(Vec::len) == rows.first().map(Vec::len) && i == 0 => {
                                        prev.extend(rows.clone());
                                        true
                                    }
                                    _ => false,
                                };
                                if !joined {
                                    out.push(Block::Table { rows });
                                }
                                i = j;
                                continue;
                            }
                        }
                    }
                    if is_head(l) {
                        let lvl = level_of(l);
                        // 여러 줄 제목은 합친다
                        if let (None, Some(Block::Heading { level, spans })) = (&para, out.last_mut()) {
                            if *level == lvl && i > 0 && matches!(&items[i - 1], Item::Line(pl) if is_head(pl) && (pl.size - l.size).abs() < 0.6 && l.top - pl.bot < 0.8 * l.size) {
                                join_spans(spans, line_spans(l), false);
                                i += 1;
                                continue;
                            }
                        }
                        first_line = false;
                        flush(&mut para, &mut out);
                        out.push(Block::Heading { level: lvl, spans: line_spans(l) });
                        i += 1;
                        continue;
                    }
                    let mut spans = line_spans(l);
                    // 앞줄이 끝까지 찼고 같은 왼쪽에서 시작하는 줄의 "다." "가." 등은 목록 번호가 아니라 이어지는 글
                    let continues = para.as_ref().is_some_and(|(pl, _, _)| {
                        pl.col == l.col && (pl.size - l.size).abs() < 0.8 && (l.x0 - pl.x0).abs() < 1.0 * l.size && pl.x1 > rights[l.col as usize] - 1.5 * pl.size && l.top - pl.bot < 1.0 * l.size && l.top > pl.top
                    });
                    let first_word = spans_text(&spans).split_whitespace().next().unwrap_or("").to_string();
                    let letter_marker = first_word.chars().next().is_some_and(|c| !c.is_ascii_digit() && !is_bullet_char(c) && c != '(');
                    let marker = if l.bullet {
                        Some(false)
                    } else if continues && letter_marker {
                        None
                    } else {
                        strip_marker(&mut spans)
                    };
                    let mut right = rights[l.col as usize];
                    let cross = std::mem::replace(&mut first_line, false) && carried.is_some();
                    let joins = match &para {
                        // 쪽을 넘어 이어지는 문단: 앞 쪽 아래 끝까지 찬 줄 → 이 쪽 맨 위 줄
                        Some((pl, _, _)) if cross => {
                            let (prev_right, prev_h) = carried.unwrap_or((right, p.height));
                            right = prev_right;
                            marker.is_none() && (pl.size - l.size).abs() < 0.8 && pl.bot > prev_h * 0.6 && l.top < p.height * 0.4 && pl.x1 > prev_right - 1.2 * pl.size
                        }
                        // 왼쪽 단 끝까지 찬 줄 → 오른쪽 단 첫 줄
                        Some((pl, _, _)) if pl.col == 1 && l.col == 2 && l.top < pl.top => {
                            right = rights[1];
                            marker.is_none() && (pl.size - l.size).abs() < 0.8 && pl.x1 > rights[1] - 1.2 * pl.size && i > 0 && matches!(&items[i - 1], Item::Line(x) if x.col == 1)
                        }
                        Some((pl, _, _)) => {
                            marker.is_none()
                                && pl.col == l.col
                                && (pl.size - l.size).abs() < 0.8
                                && l.top - pl.bot < 1.0 * l.size
                                && l.top > pl.top
                                // 앞줄이 오른쪽 끝 가까이까지 찼거나(일반 문단) 다음 줄이 들여쓰기 안쪽에 있음
                                && pl.x1 > right - 8.0 * pl.size
                                && l.x0 > pl.x0 - 2.5 * l.size
                                && l.x0 < pl.x0 + 3.0 * l.size
                        }
                        None => false,
                    };
                    if joins {
                        let (pl, dst, _) = para.as_mut().expect("para");
                        let last = spans_text(dst).trim_end().chars().last();
                        let first = spans_text(&spans).trim_start().chars().next();
                        // 한글끼리 끊긴 줄: 줄 끝 빈칸을 남기는 PDF면 빈칸이 없을 때, 아니면 앞줄이 끝까지 찼을 때 붙인다
                        let full = pl.x1 > right - 1.2 * pl.size;
                        let cjk_both = last.is_some_and(is_cjk) && first.is_some_and(is_cjk);
                        let glue = cjk_both && !l.lead_space && (doc.trailing_spaces || full);
                        join_spans(dst, spans, glue);
                        *pl = l.clone();
                    } else {
                        flush(&mut para, &mut out);
                        para = Some((l.clone(), spans, marker));
                    }
                    i += 1;
                }
            }
        }
        // 마지막 줄이 쪽 끝까지 찬 문단은 다음 쪽으로 넘겨 이어 볼 수 있게 둔다
        let last_right = para.as_ref().map(|(pl, _, _)| rights[pl.col as usize]);
        if page_marks || last_right.is_none() {
            flush(&mut para, &mut out);
        } else {
            prev_page = Some((last_right.unwrap_or(0.0), p.height));
        }
    }
    flush(&mut para, &mut out);
    out
}

/// 일반 텍스트(TXT): 읽는 순서, 문단은 한 줄, 표는 칸을 탭으로. 쪽 사이에는 빈 줄 + "— N쪽 —" + 빈 줄
/// (메모장에서 폼 피드 문자는 이상한 기호로 보이므로 쓰지 않는다).
pub fn plain_text(doc: &DocLayout) -> String {
    let mut s = String::new();
    let mut num = 0;
    for b in blocks_with(doc, true) {
        if !matches!(b, Block::Para { list: Some(true), .. }) {
            num = 0;
        }
        match b {
            Block::Heading { spans, .. } => {
                if !s.is_empty() && !s.ends_with("\n\n") {
                    s.push('\n');
                }
                s.push_str(spans_text(&spans).trim());
                s.push('\n');
            }
            Block::Para { spans, list } => {
                match list {
                    Some(true) => {
                        num += 1;
                        s.push_str(&format!("{num}. "));
                    }
                    Some(false) => s.push_str("• "),
                    None => {}
                }
                if list != Some(true) {
                    num = 0;
                }
                s.push_str(spans_text(&spans).trim());
                s.push('\n');
                continue;
            }
            Block::Table { rows } => {
                for r in rows {
                    s.push_str(&r.iter().map(|c| spans_text(c).trim().replace(['\t', '\n'], " ")).collect::<Vec<_>>().join("\t"));
                    s.push('\n');
                }
            }
            Block::Image { .. } | Block::PageBreak { .. } => {}
            Block::PageImage { .. } => s.push_str("(글자가 없는 쪽입니다 — 스캔한 그림)\n"),
            Block::PageStart { page } => s.push_str(&format!("\n— {}쪽 —\n\n", page + 1)),
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_numbers() {
        for s in ["3", "- 3 -", "3 / 10", "3쪽", "Page 3", "p. 12", "— 7 —", "2 of 5", "[4]"] {
            assert!(is_page_number(s), "{s}");
        }
        for s in ["2026년", "3분기 보고서", "표 1", "", "합계 120"] {
            assert!(!is_page_number(s), "{s}");
        }
    }

    #[test]
    fn markers_and_fonts() {
        assert!(is_list_marker("•") && is_list_marker("1.") && is_list_marker("(2)") && is_list_marker("가.") && is_list_marker("a)"));
        assert!(!is_list_marker("2026.") && !is_list_marker("서울"));
        assert_eq!(map_font("ABCDEF+NanumMyeongjo"), Some(("Batang".into(), "바탕".into())));
        assert_eq!(map_font("BAAAAA+TimesNewRomanPS-BoldMT"), Some(("Times New Roman".into(), "바탕".into())));
        assert_eq!(map_font("NotoSansCJKkr-Regular"), None);
        assert_eq!(map_font("MalgunGothic"), None);
        assert_eq!(map_font("Carlito"), Some(("Calibri".into(), "맑은 고딕".into())));
    }

    fn sp(t: &str) -> Span {
        Span { text: t.into(), bold: false, italic: false, size: 10.0, font: "F".into(), color: [0, 0, 0], link: None }
    }

    #[test]
    fn joining() {
        let mut a = vec![sp("다음 분기에는 지적")];
        join_spans(&mut a, vec![sp("된 문제를")], true);
        assert_eq!(spans_text(&a), "다음 분기에는 지적된 문제를");
        let mut b = vec![sp("hello wor-")];
        join_spans(&mut b, vec![sp("ld again")], false);
        assert_eq!(spans_text(&b), "hello world again");
        let mut c = vec![sp("낱말 ")];
        join_spans(&mut c, vec![sp(" 다음")], false);
        assert_eq!(spans_text(&c), "낱말 다음");
        let mut m = vec![sp("• 항목 하나")];
        assert_eq!(strip_marker(&mut m), Some(false));
        assert_eq!(spans_text(&m), "항목 하나");
        let mut m = vec![sp("12. "), sp("열두째")];
        assert_eq!(strip_marker(&mut m), Some(true));
        assert_eq!(spans_text(&m), "열두째");
        let mut m = vec![sp("-2%")];
        assert_eq!(strip_marker(&mut m), None);
    }

    #[test]
    fn frames() {
        // q=1(글자가 90° 돌아감): 사용자 (x, y) → 틀 (y, x)
        let (f, w, h) = Frame::new(1, (0.0, 0.0, 595.0, 842.0));
        assert_eq!((w, h), (842.0, 595.0));
        assert_eq!(f.pt(100.0, 700.0), (700.0, 100.0));
        let (f0, w0, h0) = Frame::new(0, (0.0, 0.0, 595.0, 842.0));
        assert_eq!((w0, h0), (595.0, 842.0));
        assert_eq!(f0.pt(100.0, 800.0), (100.0, 42.0));
    }
}
