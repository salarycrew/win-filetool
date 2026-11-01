//! Word → PowerPoint(제목마다 한 장). 모양을 그대로 옮기는 것이 아니라 문서 구조로 슬라이드를 만든다.
//!
//! 규칙
//! - Title(문서 제목) → 첫 슬라이드(+ 바로 뒤 보통 문단 2개는 부제)
//! - Heading 1/2 → 한 장씩(H2 슬라이드 위에는 상위 H1을 작게). H3 이하 → 굵은 줄
//! - 문단·목록 → 글머리표(목록 단계 유지, 번호 목록은 번호). 7줄 또는 420자를 넘으면 고르게 나눠 "(계속)"
//! - 섹션의 첫 그림은 글 오른쪽, 나머지는 한 장씩. 그림 설명(Caption)은 그림 아래 작은 글씨(글머리표 아님)
//! - 표는 진짜 표로 한 장(12줄 넘으면 머리글 줄을 반복해 나눔). 글이 짧으면 글과 표를 한 장에
//! - 머리글·바닥글은 넣지 않음. 표 안 그림은 표 다음에 그림으로. EMF/WMF 그림은 바꾸지 않고 그대로 넣음
//! - `.doc`/`.hwp`/`.odt`/`.pages`/`.rtf` 등은 먼저 LibreOffice로 docx를 만든 뒤 같은 규칙으로 바꾼다([`crate::docs::convert`]).

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use serde::Serialize;

use crate::docs::{self, DocTarget};
use crate::error::{io_err, Error, Result};
use crate::ooxml::pptx::{Align, Anchor, Bullet, Deck, Para, Run, Shape, TextBox, H, W};
use crate::paths;
use crate::process::Cancel;
use crate::tempdir::JobDir;

// ---------- docx 읽기 ----------

#[derive(Debug, Clone, PartialEq)]
pub enum DocBlock {
    Heading { level: u32, text: String },
    Para { text: String, list: Option<(u32, bool)> },
    Caption { text: String },
    Image { name: String, bytes: Vec<u8>, cx: i64, cy: i64 },
    Table { rows: Vec<Vec<String>> },
}

fn attr(e: &BytesStart, key: &str) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.as_ref() == key).map(|a| quick_xml::escape::unescape(&a.value).map(|v| v.into_owned()).unwrap_or_else(|_| a.value.to_string()))
}

fn read_part<R: std::io::Read + std::io::Seek>(z: &mut zip::ZipArchive<R>, name: &str) -> Option<String> {
    let mut f = z.by_name(name).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

#[derive(Default, Debug)]
struct Style {
    name: String,
    based: Option<String>,
    outline: Option<u32>,
    num: Option<(String, u32)>,
}

fn parse_styles(xml: &str) -> HashMap<String, Style> {
    let mut m = HashMap::new();
    let mut r = Reader::from_str(xml);
    let mut cur: Option<(String, Style)> = None;
    let mut num_id: Option<String> = None;
    let mut ilvl = 0;
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.name().as_ref() {
                "w:style" => {
                    cur = attr(&e, "w:styleId").map(|id| (id, Style::default()));
                    num_id = None;
                    ilvl = 0;
                }
                "w:name" => {
                    if let Some((_, s)) = cur.as_mut() {
                        s.name = attr(&e, "w:val").unwrap_or_default().to_lowercase();
                    }
                }
                "w:basedOn" => {
                    if let Some((_, s)) = cur.as_mut() {
                        s.based = attr(&e, "w:val");
                    }
                }
                "w:outlineLvl" => {
                    if let Some((_, s)) = cur.as_mut() {
                        s.outline = attr(&e, "w:val").and_then(|v| v.parse().ok());
                    }
                }
                "w:numId" => num_id = attr(&e, "w:val"),
                "w:ilvl" => ilvl = attr(&e, "w:val").and_then(|v| v.parse().ok()).unwrap_or(0),
                _ => {}
            },
            Ok(Event::End(e)) if e.name().as_ref() == "w:style" => {
                if let Some((id, mut s)) = cur.take() {
                    if let Some(n) = num_id.take() {
                        s.num = Some((n, ilvl));
                    }
                    m.insert(id, s);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    m
}

/// numId → (ilvl → 번호 목록인지)
fn parse_numbering(xml: &str) -> HashMap<String, HashMap<u32, bool>> {
    let mut abs: HashMap<String, HashMap<u32, bool>> = HashMap::new();
    let mut nums: HashMap<String, String> = HashMap::new();
    let mut r = Reader::from_str(xml);
    let (mut cur_abs, mut cur_lvl, mut cur_num) = (None::<String>, 0u32, None::<String>);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.name().as_ref() {
                "w:abstractNum" => cur_abs = attr(&e, "w:abstractNumId"),
                "w:lvl" => cur_lvl = attr(&e, "w:ilvl").and_then(|v| v.parse().ok()).unwrap_or(0),
                "w:numFmt" => {
                    if let Some(a) = &cur_abs {
                        let f = attr(&e, "w:val").unwrap_or_default();
                        abs.entry(a.clone()).or_default().insert(cur_lvl, f != "bullet" && f != "none");
                    }
                }
                "w:num" => cur_num = attr(&e, "w:numId"),
                "w:abstractNumId" => {
                    if let Some(n) = &cur_num {
                        nums.insert(n.clone(), attr(&e, "w:val").unwrap_or_default());
                    }
                }
                _ => {}
            },
            Ok(Event::End(e)) if e.name().as_ref() == "w:abstractNum" => cur_abs = None,
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    nums.into_iter().map(|(n, a)| (n, abs.get(&a).cloned().unwrap_or_default())).collect()
}

pub(crate) fn parse_rels(xml: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let mut r = Reader::from_str(xml);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if e.name().as_ref() == "Relationship" => {
                if let (Some(i), Some(t)) = (attr(&e, "Id"), attr(&e, "Target")) {
                    if attr(&e, "TargetMode").as_deref() != Some("External") {
                        m.insert(i, t);
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    m
}

/// 상대 경로(`../media/a.png`)를 zip 안 경로로
pub(crate) fn resolve(base_dir: &str, target: &str) -> String {
    let mut parts: Vec<&str> = if target.starts_with('/') { vec![] } else { base_dir.split('/').filter(|p| !p.is_empty()).collect() };
    for seg in target.trim_start_matches('/').split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn heading_level(styles: &HashMap<String, Style>, id: &str) -> Option<u32> {
    let mut cur = Some(id.to_string());
    for _ in 0..10 {
        let s = styles.get(cur.as_deref()?)?;
        if s.name == "title" {
            return Some(0);
        }
        for p in ["heading ", "제목 "] {
            if let Some(n) = s.name.strip_prefix(p).and_then(|n| n.trim().parse::<u32>().ok()) {
                return Some(n);
            }
        }
        if let Some(o) = s.outline {
            if o < 9 {
                return Some(o + 1);
            }
        }
        cur = s.based.clone();
    }
    None
}

fn is_caption_style(styles: &HashMap<String, Style>, id: &str) -> bool {
    let mut cur = Some(id.to_string());
    for _ in 0..10 {
        let Some(k) = cur.as_deref() else { return false };
        if k.eq_ignore_ascii_case("caption") || k == "캡션" {
            return true;
        }
        let Some(s) = styles.get(k) else { return false };
        if s.name == "caption" || s.name == "캡션" {
            return true;
        }
        cur = s.based.clone();
    }
    false
}

fn style_num(styles: &HashMap<String, Style>, id: &str) -> Option<(String, u32)> {
    let mut cur = Some(id.to_string());
    for _ in 0..10 {
        let s = styles.get(cur.as_deref()?)?;
        if s.num.is_some() {
            return s.num.clone();
        }
        cur = s.based.clone();
    }
    None
}

/// "그림 1", "표 2.", "Figure 3", "Table 1:" 로 시작하는 짧은 줄
fn looks_like_caption(t: &str) -> bool {
    let t = t.trim();
    if t.chars().count() > 120 {
        return false;
    }
    for p in ["그림", "표", "사진", "도표", "Figure", "Fig.", "Table"] {
        if let Some(rest) = t.strip_prefix(p) {
            let rest = rest.trim_start();
            if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return true;
            }
        }
    }
    false
}

/// VML style의 `width:72pt;height:36pt` → EMU
fn vml_size(style: &str) -> (i64, i64) {
    let mut w = 0;
    let mut h = 0;
    for kv in style.split(';') {
        let mut it = kv.splitn(2, ':');
        let (k, v) = (it.next().unwrap_or("").trim(), it.next().unwrap_or("").trim());
        let emu = if let Some(n) = v.strip_suffix("pt") {
            n.trim().parse::<f64>().ok().map(|x| (x * 12_700.0) as i64)
        } else if let Some(n) = v.strip_suffix("in") {
            n.trim().parse::<f64>().ok().map(|x| (x * 914_400.0) as i64)
        } else if let Some(n) = v.strip_suffix("px") {
            n.trim().parse::<f64>().ok().map(|x| (x * 9_525.0) as i64)
        } else {
            None
        };
        match k {
            "width" => w = emu.unwrap_or(0),
            "height" => h = emu.unwrap_or(0),
            _ => {}
        }
    }
    (w, h)
}

#[derive(Default)]
struct PState {
    text: String,
    pstyle: Option<String>,
    num: Option<(String, u32)>,
    outline: Option<u32>,
    pics: Vec<(String, i64, i64)>,
}

/// docx를 블록 목록으로 읽는다. 두 번째 값은 사용자에게 알릴 참고 사항.
pub fn read_docx(path: &Path) -> Result<(Vec<DocBlock>, Vec<String>)> {
    let f = std::fs::File::open(path).map_err(io_err(path))?;
    let bad = |m: String| Error::Invalid(format!("Word 파일을 읽지 못했습니다({}). 파일이 손상되었거나 Word 형식이 아닐 수 있습니다: {m}", path.display()));
    let mut z = zip::ZipArchive::new(f).map_err(|e| bad(e.to_string()))?;
    let doc = read_part(&mut z, "word/document.xml").ok_or_else(|| bad("word/document.xml 없음".into()))?;
    let styles = read_part(&mut z, "word/styles.xml").map(|s| parse_styles(&s)).unwrap_or_default();
    let numbering = read_part(&mut z, "word/numbering.xml").map(|s| parse_numbering(&s)).unwrap_or_default();
    let rels = read_part(&mut z, "word/_rels/document.xml.rels").map(|s| parse_rels(&s)).unwrap_or_default();
    let mut notes: Vec<String> = Vec::new();
    let mut out: Vec<DocBlock> = Vec::new();
    let mut r = Reader::from_str(&doc);
    let mut stack: Vec<PState> = Vec::new();
    let mut in_t = false;
    let (mut ext_cx, mut ext_cy) = (0i64, 0i64);
    let mut vml_wh = (0i64, 0i64);
    let mut tbl_depth = 0u32;
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut cell = String::new();
    let mut table_pics: Vec<(String, i64, i64)> = Vec::new();
    let mut in_numpr = false;
    let mut num_id_tmp: Option<String> = None;
    let mut ilvl_tmp = 0u32;
    // mc:Fallback 안은 같은 내용의 옛 형식이므로 건너뛴다(두 번 들어가지 않게)
    let mut fallback = 0u32;
    let load_pic = |z: &mut zip::ZipArchive<std::fs::File>, rid: &str| -> Option<(String, Vec<u8>)> {
        let target = rels.get(rid)?;
        let name = resolve("word", target);
        let mut bytes = Vec::new();
        z.by_name(&name).ok()?.read_to_end(&mut bytes).ok()?;
        (!bytes.is_empty()).then_some((name, bytes))
    };
    loop {
        let ev = r.read_event().map_err(|e| bad(format!("XML: {e}")))?;
        match &ev {
            Event::Start(e) if e.name().as_ref() == "mc:Fallback" => {
                fallback += 1;
                continue;
            }
            Event::End(e) if e.name().as_ref() == "mc:Fallback" => {
                fallback = fallback.saturating_sub(1);
                continue;
            }
            Event::Eof => break,
            _ => {}
        }
        if fallback > 0 {
            continue;
        }
        match ev {
            Event::Start(e) => match e.name().as_ref() {
                "w:p" => stack.push(PState::default()),
                "w:t" => in_t = true,
                "w:numPr" => {
                    in_numpr = true;
                    num_id_tmp = None;
                    ilvl_tmp = 0;
                }
                "w:tbl" => {
                    tbl_depth += 1;
                    if tbl_depth == 1 {
                        rows.clear();
                    }
                }
                "w:tr" if tbl_depth == 1 => rows.push(Vec::new()),
                "w:tc" if tbl_depth == 1 => cell.clear(),
                "wp:extent" => {
                    ext_cx = attr(&e, "cx").and_then(|v| v.parse().ok()).unwrap_or(0);
                    ext_cy = attr(&e, "cy").and_then(|v| v.parse().ok()).unwrap_or(0);
                }
                "v:shape" | "v:rect" => vml_wh = attr(&e, "style").map(|s| vml_size(&s)).unwrap_or((0, 0)),
                _ => {}
            },
            Event::Empty(e) => match e.name().as_ref() {
                "w:pStyle" => {
                    if let Some(p) = stack.last_mut() {
                        p.pstyle = attr(&e, "w:val");
                    }
                }
                "w:outlineLvl" => {
                    if let Some(p) = stack.last_mut() {
                        p.outline = attr(&e, "w:val").and_then(|v| v.parse().ok());
                    }
                }
                "w:numId" if in_numpr => num_id_tmp = attr(&e, "w:val"),
                "w:ilvl" if in_numpr => ilvl_tmp = attr(&e, "w:val").and_then(|v| v.parse().ok()).unwrap_or(0),
                "w:tab" | "w:br" | "w:cr" => {
                    if let Some(p) = stack.last_mut() {
                        p.text.push(' ');
                    }
                }
                "w:noBreakHyphen" => {
                    if let Some(p) = stack.last_mut() {
                        p.text.push('-');
                    }
                }
                "wp:extent" => {
                    ext_cx = attr(&e, "cx").and_then(|v| v.parse().ok()).unwrap_or(0);
                    ext_cy = attr(&e, "cy").and_then(|v| v.parse().ok()).unwrap_or(0);
                }
                "a:blip" => {
                    if let (Some(id), Some(p)) = (attr(&e, "r:embed"), stack.last_mut()) {
                        p.pics.push((id, ext_cx, ext_cy));
                    }
                }
                "v:imagedata" => {
                    if let (Some(id), Some(p)) = (attr(&e, "r:id"), stack.last_mut()) {
                        p.pics.push((id, vml_wh.0, vml_wh.1));
                    }
                }
                _ => {}
            },
            Event::Text(t) if in_t => {
                if let Some(p) = stack.last_mut() {
                    p.text.push_str(&t.xml10_content());
                }
            }
            Event::GeneralRef(g) if in_t => {
                if let Some(p) = stack.last_mut() {
                    let n = g.as_ref().to_string();
                    match n.as_str() {
                        "amp" => p.text.push('&'),
                        "lt" => p.text.push('<'),
                        "gt" => p.text.push('>'),
                        "quot" => p.text.push('"'),
                        "apos" => p.text.push('\''),
                        _ => {
                            if let Some(c) = n.strip_prefix("#x").and_then(|h| u32::from_str_radix(h, 16).ok()).or_else(|| n.strip_prefix('#').and_then(|d| d.parse().ok())).and_then(char::from_u32) {
                                p.text.push(c);
                            }
                        }
                    }
                }
            }
            Event::End(e) => match e.name().as_ref() {
                "w:t" => in_t = false,
                "w:numPr" => {
                    in_numpr = false;
                    if let (Some(n), Some(p)) = (num_id_tmp.take(), stack.last_mut()) {
                        p.num = Some((n, ilvl_tmp));
                    }
                }
                "w:p" => {
                    let Some(mut p) = stack.pop() else { continue };
                    let t = p.text.split_whitespace().collect::<Vec<_>>().join(" ");
                    if tbl_depth > 0 {
                        if !cell.is_empty() && !t.is_empty() {
                            cell.push(' ');
                        }
                        cell.push_str(&t);
                        table_pics.append(&mut p.pics);
                        continue;
                    }
                    let lvl = p.outline.map(|o| o + 1).or_else(|| p.pstyle.as_deref().and_then(|s| heading_level(&styles, s)));
                    if !t.is_empty() {
                        let caption = p.pstyle.as_deref().is_some_and(|s| is_caption_style(&styles, s))
                            || (looks_like_caption(&t) && matches!(out.last(), Some(DocBlock::Image { .. }) | Some(DocBlock::Table { .. })));
                        match lvl {
                            Some(l) if l <= 9 && !caption => out.push(DocBlock::Heading { level: l, text: t }),
                            _ if caption => out.push(DocBlock::Caption { text: t }),
                            _ => {
                                let n = p.num.clone().or_else(|| p.pstyle.as_deref().and_then(|s| style_num(&styles, s)));
                                let list = n.filter(|(id, _)| id != "0").map(|(id, il)| (il, numbering.get(&id).and_then(|m| m.get(&il)).copied().unwrap_or(false)));
                                out.push(DocBlock::Para { text: t, list });
                            }
                        }
                    }
                    for (rid, cx, cy) in p.pics.drain(..) {
                        if let Some((name, bytes)) = load_pic(&mut z, &rid) {
                            out.push(DocBlock::Image { name, bytes, cx, cy });
                        }
                    }
                }
                "w:tc" if tbl_depth == 1 => {
                    if let Some(r) = rows.last_mut() {
                        r.push(cell.trim().to_string());
                    }
                }
                "w:tbl" => {
                    if tbl_depth == 1 {
                        let rows = std::mem::take(&mut rows);
                        if rows.iter().any(|r| r.iter().any(|c| !c.is_empty())) {
                            out.push(DocBlock::Table { rows });
                        }
                        if !table_pics.is_empty() {
                            notes.push(format!("표 안 그림 {}개는 표 다음 슬라이드에 넣었습니다", table_pics.len()));
                        }
                        for (rid, cx, cy) in std::mem::take(&mut table_pics) {
                            if let Some((name, bytes)) = load_pic(&mut z, &rid) {
                                out.push(DocBlock::Image { name, bytes, cx, cy });
                            }
                        }
                    }
                    tbl_depth = tbl_depth.saturating_sub(1);
                }
                _ => {}
            },
            _ => {}
        }
    }
    Ok((out, notes))
}

// ---------- 슬라이드 만들기 ----------

const M: i64 = 457_200; // 0.5in
const TITLE_Y: i64 = 380_000;
const TITLE_H: i64 = 900_000;
const BODY_Y: i64 = 1_400_000;
const MAX_LINES: usize = 7;
const MAX_CHARS: usize = 420;
const TABLE_ROWS_PER_SLIDE: usize = 12;

struct Img {
    media: usize,
    cx: i64,
    cy: i64,
    caption: Option<String>,
}

struct Section {
    crumb: Option<String>,
    title: String,
    items: Vec<Para>,
    images: Vec<Img>,
    tables: Vec<Vec<Vec<String>>>,
}

impl Section {
    fn new(crumb: Option<String>, title: String) -> Self {
        Self { crumb, title, items: vec![], images: vec![], tables: vec![] }
    }
}

fn fit(cx: i64, cy: i64, bw: i64, bh: i64) -> (i64, i64) {
    let (cx, cy) = (cx.max(1) as f64, cy.max(1) as f64);
    let s = (bw as f64 / cx).min(bh as f64 / cy);
    ((cx * s) as i64, (cy * s) as i64)
}

fn text_box(x: i64, y: i64, w: i64, h: i64, paras: Vec<Para>, middle: bool) -> Shape {
    Shape::Text(TextBox { x, y, w, h, paras, anchor: if middle { Anchor::Middle } else { Anchor::Top }, wrap: true, inset: true })
}

fn title_shapes(sec: &Section, suffix: &str) -> Vec<Shape> {
    let mut v = Vec::new();
    if let Some(c) = &sec.crumb {
        v.push(text_box(M, 120_000, W - 2 * M, 300_000, vec![Para { runs: vec![Run { color: Some([0x59, 0x59, 0x59]), ..Run::new(c.clone(), 12.0, false) }], ..Default::default() }], false));
    }
    v.push(text_box(M, TITLE_Y, W - 2 * M, TITLE_H, vec![Para { runs: vec![Run::new(format!("{}{}", sec.title, suffix), 30.0, true)], ..Default::default() }], true));
    v
}

/// 고르게 나누기: 7줄·420자 이하가 되도록 필요한 장 수만큼 나누고, 줄을 고르게 펴 담는다("(계속)"에 한 줄만 남지 않게)
fn chunk(items: &[Para]) -> Vec<Vec<Para>> {
    if items.is_empty() {
        return vec![];
    }
    let chars: usize = items.iter().map(Para::chars).sum();
    let n = items.len().div_ceil(MAX_LINES).max(chars.div_ceil(MAX_CHARS)).min(items.len()).max(1);
    let per = items.len().div_ceil(n);
    items.chunks(per).map(<[Para]>::to_vec).collect()
}

fn body_size(paras: &[Para], half: bool) -> f32 {
    let chars: usize = paras.iter().map(Para::chars).sum();
    let budget = if half { 200 } else { 320 };
    if chars > budget {
        16.0
    } else if chars > budget / 2 {
        18.0
    } else {
        20.0
    }
}

fn sized(paras: &[Para], size: f32) -> Vec<Para> {
    paras
        .iter()
        .map(|p| {
            let mut p = p.clone();
            for r in &mut p.runs {
                r.size_pt = if r.bold && p.bullet == Bullet::None { size + 2.0 } else { size };
            }
            p
        })
        .collect()
}

fn table_size(rows: usize) -> f32 {
    if rows > 10 {
        12.0
    } else if rows > 6 {
        14.0
    } else {
        16.0
    }
}

/// 그림 + (있으면) 그림 설명을 상자 안에 맞춰 놓는다
fn place_pic(s: &mut Vec<Shape>, img: &Img, bx: i64, by: i64, bw: i64, bh: i64) {
    let cap_h = if img.caption.is_some() { 420_000 } else { 0 };
    let (w, h) = fit(img.cx, img.cy, bw, bh - cap_h);
    let y = by + (bh - cap_h - h) / 2;
    s.push(Shape::Pic { x: bx + (bw - w) / 2, y, w, h, media: img.media });
    if let Some(c) = &img.caption {
        s.push(text_box(bx, y + h + 40_000, bw, cap_h - 40_000, vec![Para { runs: vec![Run { color: Some([0x40, 0x40, 0x40]), italic: true, ..Run::new(c.clone(), 12.0, false) }], align: Align::Center, ..Default::default() }], false));
    }
}

fn table_slides(deck: &mut Deck, sec: &Section, t: &[Vec<String>], lead: Option<Vec<Para>>) {
    // 머리글 줄은 첫 줄로 본다(Word 표 대부분)
    let header = t.len() > 1;
    let body: Vec<&Vec<String>> = t.iter().skip(header as usize).collect();
    let per = TABLE_ROWS_PER_SLIDE - header as usize;
    let parts: Vec<Vec<&Vec<String>>> = if body.is_empty() { vec![vec![]] } else { body.chunks(per).map(<[&Vec<String>]>::to_vec).collect() };
    let n = parts.len();
    for (k, part) in parts.into_iter().enumerate() {
        let mut rows: Vec<Vec<String>> = Vec::new();
        if header {
            rows.push(t[0].clone());
        }
        rows.extend(part.into_iter().cloned());
        let suffix = match (&lead, k) {
            (Some(_), 0) => String::new(),
            _ if n > 1 => format!(" (표 {}/{n})", k + 1),
            _ => " (표)".to_string(),
        };
        let mut s = title_shapes(sec, &suffix);
        let mut y = BODY_Y;
        if k == 0 {
            if let Some(l) = &lead {
                let h = 380_000 * l.len() as i64 + 200_000;
                s.push(text_box(M, BODY_Y, W - 2 * M, h, sized(l, 18.0), false));
                y += h + 100_000;
            }
        }
        s.push(Shape::Table { x: M, y, w: W - 2 * M, size_pt: table_size(rows.len()), rows, header });
        deck.slides.push(s);
    }
}

fn emit(deck: &mut Deck, sec: &Section) {
    let body_h = H - BODY_Y - M;
    // 짧은 글 + 작은 표 하나 → 한 장
    let short = !sec.items.is_empty() && sec.items.len() <= 3 && sec.items.iter().map(Para::chars).sum::<usize>() <= 200 && sec.images.is_empty();
    if short && sec.tables.first().is_some_and(|t| t.len() <= 7) {
        table_slides(deck, sec, &sec.tables[0], Some(sec.items.clone()));
        for t in &sec.tables[1..] {
            table_slides(deck, sec, t, None);
        }
        return;
    }
    let chunks = chunk(&sec.items);
    let mut imgs = sec.images.iter();
    if chunks.is_empty() && sec.images.is_empty() && sec.tables.is_empty() {
        // 내용 없는 제목 → 구역 표시 슬라이드
        let mut v = Vec::new();
        if let Some(c) = &sec.crumb {
            v.push(text_box(M, H / 2 - 1_000_000, W - 2 * M, 400_000, vec![Para { runs: vec![Run::new(c.clone(), 16.0, false)], align: Align::Center, ..Default::default() }], false));
        }
        v.push(text_box(M, H / 2 - 600_000, W - 2 * M, 1_200_000, vec![Para { runs: vec![Run::new(sec.title.clone(), 36.0, true)], align: Align::Center, ..Default::default() }], true));
        deck.slides.push(v);
        return;
    }
    for (i, ch) in chunks.iter().enumerate() {
        let mut s = title_shapes(sec, if i > 0 { " (계속)" } else { "" });
        let side = if i == 0 { imgs.next() } else { None };
        let half = side.is_some();
        let size = body_size(ch, half);
        let tw = if half { (W - 2 * M) * 55 / 100 } else { W - 2 * M };
        s.push(text_box(M, BODY_Y, tw, body_h, sized(ch, size), false));
        if let Some(img) = side {
            let bx = M + tw + 200_000;
            place_pic(&mut s, img, bx, BODY_Y, W - M - bx, body_h);
        }
        deck.slides.push(s);
    }
    let rest: Vec<&Img> = imgs.collect();
    for (k, img) in rest.iter().enumerate() {
        let suffix = if rest.len() > 1 {
            format!(" (그림 {})", k + 1)
        } else if chunks.is_empty() {
            String::new()
        } else {
            " (그림)".into()
        };
        let mut s = title_shapes(sec, &suffix);
        place_pic(&mut s, img, M, BODY_Y, W - 2 * M, body_h);
        deck.slides.push(s);
    }
    for t in &sec.tables {
        table_slides(deck, sec, t, None);
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DocToPptxOut {
    pub input: PathBuf,
    pub output: PathBuf,
    pub slides: usize,
    pub sections: usize,
    pub images: usize,
    pub tables: usize,
    /// LibreOffice로 먼저 docx를 만들었는지(.doc/.hwp/.odt/.pages 등)
    pub via_libreoffice: bool,
    pub millis: u64,
    pub note: String,
}

/// 블록 목록 → 슬라이드 묶음
pub fn build_deck(blocks: Vec<DocBlock>, fallback_title: &str) -> (Deck, usize) {
    let mut deck = Deck::new(fallback_title);
    let mut secs: Vec<Section> = Vec::new();
    let mut doc_title: Option<String> = None;
    let mut subtitle: Vec<String> = Vec::new();
    let mut h1: Option<String> = None;
    // 바로 앞이 그림/표였는지(설명을 붙이려고)
    let mut last_was_img = false;
    for b in blocks {
        let was_img = std::mem::replace(&mut last_was_img, false);
        match b {
            DocBlock::Heading { level: 0, text } if doc_title.is_none() && secs.is_empty() => doc_title = Some(text),
            DocBlock::Heading { level, text } if level <= 2 => {
                let crumb = if level == 2 { h1.clone() } else { None };
                if level <= 1 {
                    h1 = Some(text.clone());
                }
                secs.push(Section::new(crumb, text));
            }
            DocBlock::Heading { text, .. } => {
                if secs.is_empty() {
                    secs.push(Section::new(None, doc_title.clone().unwrap_or_else(|| fallback_title.to_string())));
                }
                if let Some(s) = secs.last_mut() {
                    s.items.push(Para { runs: vec![Run::new(text, 20.0, true)], space_before_pt: 6.0, ..Default::default() });
                }
            }
            DocBlock::Caption { text } => {
                if was_img {
                    if let Some(img) = secs.last_mut().and_then(|s| s.images.last_mut()) {
                        img.caption = Some(text);
                        continue;
                    }
                }
                // 표 설명이나 따로 떨어진 설명은 글머리표 없는 작은 줄로
                if secs.is_empty() {
                    secs.push(Section::new(None, doc_title.clone().unwrap_or_else(|| fallback_title.to_string())));
                }
                if let Some(s) = secs.last_mut() {
                    s.items.push(Para { runs: vec![Run { italic: true, ..Run::new(text, 16.0, false) }], space_before_pt: 6.0, ..Default::default() });
                }
            }
            DocBlock::Para { text, list } => {
                if secs.is_empty() {
                    if doc_title.is_some() && subtitle.len() < 2 && list.is_none() {
                        subtitle.push(text);
                        continue;
                    }
                    secs.push(Section::new(None, doc_title.clone().unwrap_or_else(|| fallback_title.to_string())));
                }
                let (level, bullet) = match list {
                    Some((l, true)) => (l, Bullet::Number),
                    Some((l, false)) => (l, Bullet::Char(if l == 0 { "•" } else { "–" }.into())),
                    None => (0, Bullet::Char("•".into())),
                };
                if let Some(s) = secs.last_mut() {
                    s.items.push(Para { runs: vec![Run::new(text, 20.0, false)], level, bullet, space_before_pt: 6.0, ..Default::default() });
                }
            }
            DocBlock::Image { name, bytes, cx, cy } => {
                let short = name.rsplit('/').next().unwrap_or(&name).to_string();
                let m = deck.add_media(&short, bytes);
                if secs.is_empty() {
                    secs.push(Section::new(None, doc_title.clone().unwrap_or_else(|| fallback_title.to_string())));
                }
                if let Some(s) = secs.last_mut() {
                    s.images.push(Img { media: m, cx, cy, caption: None });
                }
                last_was_img = true;
            }
            DocBlock::Table { rows } => {
                if secs.is_empty() {
                    secs.push(Section::new(None, doc_title.clone().unwrap_or_else(|| fallback_title.to_string())));
                }
                if let Some(s) = secs.last_mut() {
                    s.tables.push(rows);
                }
            }
        }
    }
    let title = doc_title.unwrap_or_else(|| fallback_title.to_string());
    deck.title = title.clone();
    let mut first = vec![text_box(M, H / 2 - 1_100_000, W - 2 * M, 1_400_000, vec![Para { runs: vec![Run::new(title, 40.0, true)], align: Align::Center, ..Default::default() }], true)];
    if !subtitle.is_empty() {
        first.push(text_box(
            M,
            H / 2 + 400_000,
            W - 2 * M,
            900_000,
            subtitle.iter().map(|t| Para { runs: vec![Run { color: Some([0x40, 0x40, 0x40]), ..Run::new(t.clone(), 18.0, false) }], align: Align::Center, ..Default::default() }).collect(),
            false,
        ));
    }
    deck.slides.push(first);
    for s in &secs {
        emit(&mut deck, s);
    }
    (deck, secs.len())
}

fn is_docx(p: &Path) -> bool {
    matches!(p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("docx" | "docm" | "dotx" | "dotm"))
}

/// 이 기능이 받는 파일인지(Word·한글·ODT·Pages·RTF 등 글 문서)
pub fn accepts(p: &Path) -> bool {
    is_docx(p) || docs::family_of(p) == Some(docs::DocFamily::Text) && !matches!(p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("txt"))
}

/// Word(또는 LibreOffice가 읽는 글 문서) → `이름_PPT.pptx`.
/// `soffice`는 docx가 아닌 문서에만 쓴다(없으면 한국어 안내 오류).
pub fn doc_to_pptx(input: &Path, out_dir: Option<&Path>, soffice: Option<&Path>, cancel: Option<&Cancel>, progress: &mut dyn FnMut(u32, u32)) -> Result<DocToPptxOut> {
    let t0 = Instant::now();
    if !input.is_file() {
        return Err(Error::Invalid(format!("파일이 없습니다: {}", input.display())));
    }
    if !accepts(input) {
        return Err(Error::Unsupported(format!("Word → PPT는 Word·한글·ODT·Pages 문서만 됩니다: {}", input.display())));
    }
    let check = || cancel.map_or(Ok(()), Cancel::check);
    progress(0, 3);
    let job;
    let (docx, via) = if is_docx(input) {
        (input.to_path_buf(), false)
    } else {
        let so = soffice.ok_or_else(|| Error::ToolMissing { tool: "LibreOffice".into(), hint: "Word(.docx)가 아닌 문서는 먼저 Word 형식으로 바꾼 뒤 슬라이드를 만듭니다".into() })?;
        job = JobDir::new("w2p")?;
        let made = docs::convert(so, input, Some(job.path()), DocTarget::Docx, cancel, docs::DOC_TIMEOUT.min(Duration::from_secs(300)))?;
        (made, true)
    };
    check()?;
    progress(1, 3);
    let (blocks, mut notes) = read_docx(&docx)?;
    let stem = input.file_stem().map(|s| crate::nfc::to_nfc(&s.to_string_lossy())).unwrap_or_else(|| "문서".into());
    let (deck, sections) = build_deck(blocks, &stem);
    check()?;
    progress(2, 3);
    let output = paths::output_path(input, out_dir, "_PPT", "pptx");
    deck.save(&output)?;
    progress(3, 3);
    let st = deck.count();
    if sections == 0 {
        notes.push("제목(Heading) 스타일이 없어 내용을 한 덩어리로 나눴습니다".into());
    }
    let note = format!("슬라이드 {}장(제목 {}개, 그림 {}개, 표 {}개){}", st.slides, sections, st.media, st.tables, if notes.is_empty() { String::new() } else { format!(" · {}", notes.join(" · ")) });
    Ok(DocToPptxOut { input: input.into(), output, slides: st.slides, sections, images: st.media, tables: st.tables, via_libreoffice: via, millis: t0.elapsed().as_millis() as u64, note })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn para(t: &str) -> DocBlock {
        DocBlock::Para { text: t.into(), list: None }
    }

    #[test]
    fn rules() {
        let png = vec![0x89, b'P', b'N', b'G', 9];
        let mut blocks = vec![
            DocBlock::Heading { level: 0, text: "3분기 보고서".into() },
            para("영업팀"),
            para("2026년 10월"),
            DocBlock::Heading { level: 1, text: "개요".into() },
        ];
        for i in 0..8 {
            blocks.push(DocBlock::Para { text: format!("항목 {i}"), list: Some((0, false)) });
        }
        blocks.push(DocBlock::Image { name: "word/media/a.png".into(), bytes: png.clone(), cx: 100, cy: 50 });
        blocks.push(DocBlock::Caption { text: "그림 1. 매출".into() });
        blocks.push(DocBlock::Heading { level: 2, text: "세부".into() });
        blocks.push(para("짧은 글"));
        blocks.push(DocBlock::Table { rows: vec![vec!["가".into(), "나".into()], vec!["1".into(), "2".into()]] });
        blocks.push(DocBlock::Heading { level: 1, text: "큰 표".into() });
        let mut big = vec![vec!["머리".into(), "값".into()]];
        for i in 0..20 {
            big.push(vec![format!("{i}"), "x".into()]);
        }
        blocks.push(DocBlock::Table { rows: big });
        blocks.push(DocBlock::Heading { level: 1, text: "빈 장".into() });
        let (deck, secs) = build_deck(blocks, "파일");
        assert_eq!(secs, 4);
        // 제목 1 + 개요 2(8줄 → 4+4) + 세부(글+표 한 장) 1 + 큰 표 2(머리글 반복) + 빈 장 1
        assert_eq!(deck.slides.len(), 7, "{:#?}", deck.slides.iter().map(|s| s.len()).collect::<Vec<_>>());
        let texts = |i: usize| -> Vec<String> {
            deck.slides[i].iter().filter_map(|s| if let Shape::Text(t) = s { Some(t.paras.iter().map(Para::text).collect::<Vec<_>>().join("|")) } else { None }).collect()
        };
        assert_eq!(texts(0), vec!["3분기 보고서".to_string(), "영업팀|2026년 10월".to_string()]);
        assert!(texts(2).iter().any(|t| t == "개요 (계속)"));
        // 그림 설명은 글머리표가 아니라 그림 아래 글상자
        assert!(texts(1).iter().any(|t| t == "그림 1. 매출"));
        assert!(deck.slides[1].iter().any(|s| matches!(s, Shape::Pic { .. })));
        // 세부: crumb + 제목 + 글 + 표 한 장
        assert!(texts(3).contains(&"개요".to_string()) && texts(3).contains(&"세부".to_string()));
        assert!(deck.slides[3].iter().any(|s| matches!(s, Shape::Table { .. })));
        let rows: Vec<usize> = deck.slides[4..6].iter().flat_map(|s| s.iter()).filter_map(|s| if let Shape::Table { rows, .. } = s { Some(rows.len()) } else { None }).collect();
        assert_eq!(rows, vec![12, 10], "11줄 + 머리글, 9줄 + 머리글");
        assert!(texts(4).iter().any(|t| t == "큰 표 (표 1/2)"));
        assert_eq!(texts(6), vec!["빈 장".to_string()]);
    }

    #[test]
    fn helpers() {
        assert!(looks_like_caption("그림 3. 매출 추이"));
        assert!(looks_like_caption("Table 2: Results"));
        assert!(!looks_like_caption("그림을 보면"));
        assert_eq!(vml_size("position:absolute;width:72pt;height:1in"), (914_400, 914_400));
        assert_eq!(resolve("word", "media/image1.png"), "word/media/image1.png");
        assert_eq!(resolve("ppt/slides", "../media/a.emf"), "ppt/media/a.emf");
        assert_eq!(resolve("word", "/word/media/x.png"), "word/media/x.png");
        assert!(accepts(Path::new("a.hwp")) && accepts(Path::new("a.DOCX")) && accepts(Path::new("a.pages")) && !accepts(Path::new("a.txt")) && !accepts(Path::new("a.pdf")));
    }
}
