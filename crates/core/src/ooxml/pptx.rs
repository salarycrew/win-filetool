//! PPTX 쓰기: 자체 템플릿(마스터 1, 빈 레이아웃 1, 테마 1 — 맑은 고딕). 기본 16:9(12,192,000 × 6,858,000 EMU).
//! 슬라이드에는 위치를 직접 정한 도형만 둔다: 글상자, 그림, 표, 사각형, 선.
//! 단위는 EMU(1 pt = 12,700 EMU, 1 in = 914,400 EMU).

use std::collections::HashMap;
use std::path::Path;

use super::{app_xml, core_xml, esc, hex, media_type, sniff_ext, Package, FONT_EA, FONT_LATIN};
use crate::error::Result;

pub const W: i64 = 12_192_000;
pub const H: i64 = 6_858_000;
pub const EMU_PER_PT: f64 = 12_700.0;
/// PowerPoint가 받는 슬라이드 크기: 1~56인치
pub const MIN_SIDE: i64 = 914_400;
pub const MAX_SIDE: i64 = 51_206_400;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub text: String,
    pub size_pt: f32,
    pub bold: bool,
    pub italic: bool,
    pub color: Option<[u8; 3]>,
    /// (라틴, 동아시아) 글꼴. 없으면 테마 글꼴(맑은 고딕)
    pub font: Option<(String, String)>,
}

impl Run {
    pub fn new(text: impl Into<String>, size_pt: f32, bold: bool) -> Self {
        Self { text: text.into(), size_pt, bold, ..Default::default() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Bullet {
    #[default]
    None,
    Char(String),
    Number,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Para {
    pub runs: Vec<Run>,
    pub level: u32,
    pub bullet: Bullet,
    pub align: Align,
    /// 문단 앞 간격(pt)
    pub space_before_pt: f32,
    /// 줄 간격을 pt로 고정(PDF 글자 위치를 그대로 두기 위해). 없으면 기본
    pub line_pt: Option<f32>,
    /// 문단 안 줄바꿈(<a:br/>) 위치: runs를 이 개수씩 끊는 대신, 각 run 끝에 줄바꿈을 넣을지
    pub breaks_after: Vec<usize>,
}

impl Para {
    pub fn plain(text: impl Into<String>, size_pt: f32, bold: bool) -> Self {
        Self { runs: vec![Run::new(text, size_pt, bold)], space_before_pt: 6.0, ..Default::default() }
    }
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }
    pub fn chars(&self) -> usize {
        self.runs.iter().map(|r| r.text.chars().count()).sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Anchor {
    #[default]
    Top,
    Middle,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextBox {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
    pub paras: Vec<Para>,
    pub anchor: Anchor,
    /// false면 줄을 바꾸지 않음(PDF 위치 그대로 둘 때)
    pub wrap: bool,
    /// false면 안쪽 여백 0
    pub inset: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Text(TextBox),
    Pic { x: i64, y: i64, w: i64, h: i64, media: usize },
    Table { x: i64, y: i64, w: i64, rows: Vec<Vec<String>>, size_pt: f32, header: bool },
    Rect { x: i64, y: i64, w: i64, h: i64, fill: Option<[u8; 3]>, line: Option<([u8; 3], i64)> },
    Line { x1: i64, y1: i64, x2: i64, y2: i64, color: [u8; 3], width: i64 },
}

#[derive(Debug, Clone)]
pub struct Media {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Deck {
    pub slides: Vec<Vec<Shape>>,
    pub media: Vec<Media>,
    pub title: String,
    /// 슬라이드 크기(EMU)
    pub size: (i64, i64),
    dedupe: HashMap<(u64, usize), Vec<usize>>,
}

/// FNV-1a 64
pub(crate) fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, x| (h ^ *x as u64).wrapping_mul(0x0100_0000_01b3))
}

/// 쪽 크기(pt) → 슬라이드 크기(EMU). PowerPoint 범위(1~56인치) 안으로 비율을 지켜 줄이거나 늘린다.
pub fn slide_size_for_page(w_pt: f64, h_pt: f64) -> (i64, i64) {
    let (mut w, mut h) = (w_pt.max(1.0) * EMU_PER_PT, h_pt.max(1.0) * EMU_PER_PT);
    let big = w.max(h);
    if big > MAX_SIDE as f64 {
        let s = MAX_SIDE as f64 / big;
        w *= s;
        h *= s;
    }
    let small = w.min(h);
    if small < MIN_SIDE as f64 {
        let s = (MIN_SIDE as f64 / small).min(MAX_SIDE as f64 / w.max(h));
        w *= s;
        h *= s;
    }
    (w.round() as i64, h.round() as i64)
}

fn run_xml(r: &Run) -> String {
    let mut props = format!(r#"<a:rPr lang="ko-KR" altLang="en-US" sz="{}" b="{}" i="{}" dirty="0">"#, (r.size_pt.clamp(1.0, 4000.0) * 100.0).round() as i32, r.bold as u8, r.italic as u8);
    if let Some(c) = r.color {
        props.push_str(&format!(r#"<a:solidFill><a:srgbClr val="{}"/></a:solidFill>"#, hex(c)));
    }
    let (lat, ea) = match &r.font {
        Some((l, e)) => (l.as_str(), e.as_str()),
        None => (FONT_LATIN, FONT_EA),
    };
    props.push_str(&format!(r#"<a:latin typeface="{}"/><a:ea typeface="{}"/></a:rPr>"#, esc(lat), esc(ea)));
    format!("<a:r>{props}<a:t>{}</a:t></a:r>", esc(&r.text))
}

fn paras_xml(paras: &[Para], in_table: bool) -> String {
    let mut s = String::new();
    for p in paras {
        let indent = 342_900i64;
        let algn = match p.align {
            Align::Left => "l",
            Align::Center => "ctr",
            Align::Right => "r",
        };
        let mut inner = String::new();
        if let Some(l) = p.line_pt {
            inner.push_str(&format!(r#"<a:lnSpc><a:spcPts val="{}"/></a:lnSpc>"#, (l * 100.0).round() as i32));
        }
        if !in_table && p.space_before_pt > 0.0 {
            inner.push_str(&format!(r#"<a:spcBef><a:spcPts val="{}"/></a:spcBef>"#, (p.space_before_pt * 100.0).round() as i32));
        }
        let ppr = match &p.bullet {
            Bullet::Number => format!(r#"<a:pPr marL="{}" indent="-{indent}" lvl="{}" algn="{algn}">{inner}<a:buFont typeface="+mj-lt"/><a:buAutoNum type="arabicPeriod"/></a:pPr>"#, indent * (p.level as i64 + 1), p.level.min(8)),
            Bullet::Char(b) => format!(r#"<a:pPr marL="{}" indent="-{indent}" lvl="{}" algn="{algn}">{inner}<a:buFont typeface="Arial"/><a:buChar char="{}"/></a:pPr>"#, indent * (p.level as i64 + 1), p.level.min(8), esc(b)),
            Bullet::None => format!(r#"<a:pPr marL="0" indent="0" algn="{algn}">{inner}<a:buNone/></a:pPr>"#),
        };
        let mut runs = String::new();
        for (i, r) in p.runs.iter().enumerate() {
            runs.push_str(&run_xml(r));
            if p.breaks_after.contains(&i) && i + 1 < p.runs.len() {
                runs.push_str(&format!(r#"<a:br><a:rPr lang="ko-KR" sz="{}"/></a:br>"#, (r.size_pt.clamp(1.0, 4000.0) * 100.0).round() as i32));
            }
        }
        let end_sz = p.runs.last().map(|r| r.size_pt).unwrap_or(18.0);
        s.push_str(&format!(r#"<a:p>{ppr}{runs}<a:endParaRPr lang="ko-KR" sz="{}"/></a:p>"#, (end_sz.clamp(1.0, 4000.0) * 100.0).round() as i32));
    }
    s
}

fn ln_xml(c: [u8; 3], w: i64) -> String {
    format!(r#"<a:ln w="{w}"><a:solidFill><a:srgbClr val="{}"/></a:solidFill></a:ln>"#, hex(c))
}

fn shape_xml(id: usize, sh: &Shape, rid_of_media: &dyn Fn(usize) -> String) -> String {
    match sh {
        Shape::Text(t) => {
            let (l, tp) = if t.inset { (91_440, 45_720) } else { (0, 0) };
            format!(
                r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="TextBox {id}"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr wrap="{}" lIns="{l}" tIns="{tp}" rIns="{l}" bIns="{tp}" rtlCol="0" anchor="{}"><a:noAutofit/></a:bodyPr><a:lstStyle/>{}</p:txBody></p:sp>"#,
                t.x,
                t.y,
                t.w.max(1),
                t.h.max(1),
                if t.wrap { "square" } else { "none" },
                if t.anchor == Anchor::Middle { "ctr" } else { "t" },
                paras_xml(&t.paras, false)
            )
        }
        Shape::Pic { x, y, w, h, media } => format!(
            r#"<p:pic><p:nvPicPr><p:cNvPr id="{id}" name="Picture {id}"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="{}"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#,
            rid_of_media(*media),
            (*w).max(1),
            (*h).max(1)
        ),
        Shape::Rect { x, y, w, h, fill, line } => {
            let f = match fill {
                Some(c) => format!(r#"<a:solidFill><a:srgbClr val="{}"/></a:solidFill>"#, hex(*c)),
                None => "<a:noFill/>".into(),
            };
            let l = match line {
                Some((c, w)) => ln_xml(*c, *w),
                None => "<a:ln><a:noFill/></a:ln>".into(),
            };
            format!(
                r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="Rectangle {id}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>{f}{l}</p:spPr></p:sp>"#,
                (*w).max(1),
                (*h).max(1)
            )
        }
        Shape::Line { x1, y1, x2, y2, color, width } => {
            let (x, y) = ((*x1).min(*x2), (*y1).min(*y2));
            let (w, h) = ((x2 - x1).abs(), (y2 - y1).abs());
            let flip = format!("{}{}", if x2 < x1 { r#" flipH="1""# } else { "" }, if y2 < y1 { r#" flipV="1""# } else { "" });
            format!(
                r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="{id}" name="Line {id}"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr><a:xfrm{flip}><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm><a:prstGeom prst="line"><a:avLst/></a:prstGeom>{}</p:spPr></p:cxnSp>"#,
                ln_xml(*color, (*width).max(3175))
            )
        }
        Shape::Table { x, y, w, rows, size_pt, header } => {
            let ncol = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
            let cw = w / ncol as i64;
            let rh = ((*size_pt as f64) * EMU_PER_PT * 2.0).max(300_000.0) as i64;
            let mut g = String::new();
            for _ in 0..ncol {
                g.push_str(&format!(r#"<a:gridCol w="{cw}"/>"#));
            }
            let border = |side: &str| format!(r#"<a:{side} w="9525"><a:solidFill><a:srgbClr val="808080"/></a:solidFill></a:{side}>"#);
            let ln = format!("{}{}{}{}", border("lnL"), border("lnR"), border("lnT"), border("lnB"));
            let mut body = String::new();
            for (ri, r) in rows.iter().enumerate() {
                body.push_str(&format!(r#"<a:tr h="{rh}">"#));
                for ci in 0..ncol {
                    let t = r.get(ci).cloned().unwrap_or_default();
                    let head = *header && ri == 0;
                    let p = Para { runs: vec![Run::new(t, *size_pt, head)], ..Default::default() };
                    let fill = if head { r#"<a:solidFill><a:srgbClr val="DCE6F2"/></a:solidFill>"# } else { "" };
                    body.push_str(&format!(r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/>{}</a:txBody><a:tcPr>{ln}{fill}</a:tcPr></a:tc>"#, paras_xml(&[p], true)));
                }
                body.push_str("</a:tr>");
            }
            let h = rh * rows.len().max(1) as i64;
            format!(
                r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="{id}" name="Table {id}"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp="1"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr firstRow="{}" bandRow="0"/><a:tblGrid>{g}</a:tblGrid>{body}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>"#,
                *header as u8
            )
        }
    }
}

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;
const EMPTY_TREE: &str = r#"<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>"#;
const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn theme() -> String {
    let clr = |n: &str, v: &str| format!("<a:{n}><a:srgbClr val=\"{v}\"/></a:{n}>");
    let font = format!(r#"<a:latin typeface="{FONT_LATIN}"/><a:ea typeface="{FONT_EA}"/><a:cs typeface=""/><a:font script="Hang" typeface="{FONT_EA}"/>"#);
    let three = |inner: &str| inner.repeat(3);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Filetool"><a:themeElements><a:clrScheme name="Filetool"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>{}{}{}{}{}{}{}{}{}{}</a:clrScheme><a:fontScheme name="Filetool"><a:majorFont>{font}</a:majorFont><a:minorFont>{font}</a:minorFont></a:fontScheme><a:fmtScheme name="Filetool"><a:fillStyleLst>{}</a:fillStyleLst><a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst>{}</a:effectStyleLst><a:bgFillStyleLst>{}</a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#,
        clr("dk2", "1F3864"),
        clr("lt2", "E7E6E6"),
        clr("accent1", "2F5597"),
        clr("accent2", "ED7D31"),
        clr("accent3", "A5A5A5"),
        clr("accent4", "FFC000"),
        clr("accent5", "5B9BD5"),
        clr("accent6", "70AD47"),
        clr("hlink", "0563C1"),
        clr("folHlink", "954F72"),
        three(r#"<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>"#),
        three("<a:effectStyle><a:effectLst/></a:effectStyle>"),
        three(r#"<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>"#),
    )
}

impl Deck {
    pub fn new(title: impl Into<String>) -> Self {
        Self { slides: Vec::new(), media: Vec::new(), title: title.into(), size: (W, H), dedupe: HashMap::new() }
    }

    /// 그림을 넣고 번호를 돌려준다. 같은 바이트는 한 번만 넣는다.
    pub fn add_media(&mut self, name: &str, bytes: Vec<u8>) -> usize {
        let key = (fnv(&bytes), bytes.len());
        if let Some(ids) = self.dedupe.get(&key) {
            if let Some(i) = ids.iter().find(|i| self.media[**i].bytes == bytes) {
                return *i;
            }
        }
        // 확장자가 없거나 틀리면 내용으로 정한다
        let name = match sniff_ext(&bytes) {
            Some(e) if media_type(name).0 != e => format!("{name}.{e}"),
            _ => name.to_string(),
        };
        self.media.push(Media { name, bytes });
        let i = self.media.len() - 1;
        self.dedupe.entry(key).or_default().push(i);
        i
    }

    /// 표·글상자 수 등 구조 세기(시험·보고용)
    pub fn count(&self) -> DeckStats {
        let mut s = DeckStats { slides: self.slides.len(), media: self.media.len(), ..Default::default() };
        for sl in &self.slides {
            for sh in sl {
                match sh {
                    Shape::Text(_) => s.text_boxes += 1,
                    Shape::Pic { .. } => s.pictures += 1,
                    Shape::Table { .. } => s.tables += 1,
                    Shape::Rect { .. } | Shape::Line { .. } => s.shapes += 1,
                }
            }
        }
        s
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut z = Package::new();
        let mut exts: Vec<(&str, &str)> = self.media.iter().map(|m| media_type(&m.name)).collect();
        exts.sort();
        exts.dedup();
        let mut ct = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>"#);
        for (e, c) in &exts {
            ct.push_str(&format!(r#"<Default Extension="{e}" ContentType="{c}"/>"#));
        }
        ct.push_str(r#"<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/><Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/><Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>"#);
        for i in 1..=self.slides.len() {
            ct.push_str(&format!(r#"<Override PartName="/ppt/slides/slide{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>"#));
        }
        ct.push_str("</Types>");
        z.put("[Content_Types].xml", ct.as_bytes())?;
        z.put("_rels/.rels", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="{REL_NS}"><Relationship Id="rId1" Type="{R}/officeDocument" Target="ppt/presentation.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="{R}/extended-properties" Target="docProps/app.xml"/></Relationships>"#).as_bytes())?;
        z.put("docProps/core.xml", core_xml(&self.title).as_bytes())?;
        z.put("docProps/app.xml", app_xml().as_bytes())?;
        let mut pres = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:presentation {NS} saveSubsetFonts="1"><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst>"#);
        let mut prels = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="{REL_NS}"><Relationship Id="rId1" Type="{R}/slideMaster" Target="slideMasters/slideMaster1.xml"/><Relationship Id="rId2" Type="{R}/theme" Target="theme/theme1.xml"/>"#);
        for i in 1..=self.slides.len() {
            pres.push_str(&format!(r#"<p:sldId id="{}" r:id="rId{}"/>"#, 255 + i, i + 10));
            prels.push_str(&format!(r#"<Relationship Id="rId{}" Type="{R}/slide" Target="slides/slide{i}.xml"/>"#, i + 10));
        }
        let (sw, sh) = self.size;
        pres.push_str(&format!(r#"</p:sldIdLst><p:sldSz cx="{sw}" cy="{sh}"/><p:notesSz cx="6858000" cy="9144000"/><p:defaultTextStyle><a:defPPr><a:defRPr lang="ko-KR"/></a:defPPr></p:defaultTextStyle></p:presentation>"#));
        prels.push_str("</Relationships>");
        z.put("ppt/presentation.xml", pres.as_bytes())?;
        z.put("ppt/_rels/presentation.xml.rels", prels.as_bytes())?;
        z.put("ppt/theme/theme1.xml", theme().as_bytes())?;
        z.put("ppt/slideMasters/slideMaster1.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sldMaster {NS}><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree>{EMPTY_TREE}</p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle><a:lvl1pPr><a:defRPr sz="3200"><a:latin typeface="+mj-lt"/><a:ea typeface="+mj-ea"/></a:defRPr></a:lvl1pPr></p:titleStyle><p:bodyStyle><a:lvl1pPr><a:defRPr sz="1800"><a:latin typeface="+mn-lt"/><a:ea typeface="+mn-ea"/></a:defRPr></a:lvl1pPr></p:bodyStyle><p:otherStyle><a:lvl1pPr><a:defRPr sz="1800"><a:latin typeface="+mn-lt"/><a:ea typeface="+mn-ea"/></a:defRPr></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"#).as_bytes())?;
        z.put("ppt/slideMasters/_rels/slideMaster1.xml.rels", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="{REL_NS}"><Relationship Id="rId1" Type="{R}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rId2" Type="{R}/theme" Target="../theme/theme1.xml"/></Relationships>"#).as_bytes())?;
        z.put("ppt/slideLayouts/slideLayout1.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sldLayout {NS} type="blank" preserve="1"><p:cSld name="Blank"><p:spTree>{EMPTY_TREE}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#).as_bytes())?;
        z.put("ppt/slideLayouts/_rels/slideLayout1.xml.rels", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="{REL_NS}"><Relationship Id="rId1" Type="{R}/slideMaster" Target="../slideMasters/slideMaster1.xml"/></Relationships>"#).as_bytes())?;
        for (mi, m) in self.media.iter().enumerate() {
            z.put(&format!("ppt/media/image{}.{}", mi + 1, media_type(&m.name).0), &m.bytes)?;
        }
        for (si, shapes) in self.slides.iter().enumerate() {
            let mut used: Vec<usize> = shapes.iter().filter_map(|s| if let Shape::Pic { media, .. } = s { Some(*media) } else { None }).collect();
            used.sort_unstable();
            used.dedup();
            let rid = |m: usize| format!("rId{}", 10 + used.iter().position(|x| *x == m).unwrap_or(0));
            let mut tree = String::new();
            for (k, sh) in shapes.iter().enumerate() {
                tree.push_str(&shape_xml(k + 2, sh, &rid));
            }
            z.put(&format!("ppt/slides/slide{}.xml", si + 1), format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><p:sld {NS}><p:cSld><p:spTree>{EMPTY_TREE}{tree}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#).as_bytes())?;
            let mut rels = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="{REL_NS}"><Relationship Id="rId1" Type="{R}/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>"#);
            for (k, m) in used.iter().enumerate() {
                rels.push_str(&format!(r#"<Relationship Id="rId{}" Type="{R}/image" Target="../media/image{}.{}"/>"#, 10 + k, m + 1, media_type(&self.media[*m].name).0));
            }
            rels.push_str("</Relationships>");
            z.put(&format!("ppt/slides/_rels/slide{}.xml.rels", si + 1), rels.as_bytes())?;
        }
        z.finish()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        super::save(path, &self.to_bytes()?)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct DeckStats {
    pub slides: usize,
    pub text_boxes: usize,
    pub pictures: usize,
    pub tables: usize,
    pub shapes: usize,
    pub media: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn part(bytes: &[u8], name: &str) -> String {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut s = String::new();
        z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn deck_parts_and_dedupe() {
        let mut d = Deck::new("시험 <제목>");
        let png = vec![0x89, b'P', b'N', b'G', 1, 2, 3];
        let a = d.add_media("a.png", png.clone());
        let b = d.add_media("b.png", png);
        assert_eq!(a, b);
        let c = d.add_media("noext", vec![0xFF, 0xD8, 0xFF, 0, 1]);
        assert_eq!(d.media[c].name, "noext.jpeg");
        d.slides.push(vec![
            Shape::Text(TextBox { x: 1, y: 2, w: 3, h: 4, paras: vec![Para::plain("한글 & 영어", 20.0, true)], wrap: true, inset: true, ..Default::default() }),
            Shape::Pic { x: 0, y: 0, w: 10, h: 10, media: a },
            Shape::Table { x: 0, y: 0, w: 100, rows: vec![vec!["가".into(), "나".into()], vec!["1".into()]], size_pt: 14.0, header: true },
            Shape::Line { x1: 10, y1: 10, x2: 0, y2: 10, color: [0, 0, 0], width: 12700 },
            Shape::Rect { x: 0, y: 0, w: 5, h: 5, fill: Some([255, 0, 0]), line: None },
        ]);
        d.slides.push(vec![Shape::Pic { x: 0, y: 0, w: 10, h: 10, media: c }]);
        let st = d.count();
        assert_eq!((st.slides, st.text_boxes, st.pictures, st.tables, st.shapes, st.media), (2, 1, 2, 1, 2, 2));
        let bytes = d.to_bytes().unwrap();
        let s1 = part(&bytes, "ppt/slides/slide1.xml");
        assert!(s1.contains("한글 &amp; 영어"));
        assert!(s1.contains(r#"flipH="1""#));
        assert!(s1.contains("<a:gridCol"));
        assert!(part(&bytes, "ppt/slides/_rels/slide2.xml.rels").contains("../media/image2.jpeg"));
        assert!(part(&bytes, "[Content_Types].xml").contains(r#"Extension="jpeg""#));
        assert!(part(&bytes, "docProps/core.xml").contains("시험 &lt;제목&gt;"));
        assert!(part(&bytes, "ppt/presentation.xml").contains(r#"<p:sldSz cx="12192000" cy="6858000"/>"#));
    }

    #[test]
    fn slide_size_limits() {
        // A4 세로 595×842pt
        let (w, h) = slide_size_for_page(595.0, 842.0);
        assert_eq!((w, h), (7_556_500, 10_693_400));
        // 너무 큰 쪽은 56인치 안으로
        let (w, h) = slide_size_for_page(10_000.0, 5_000.0);
        assert!(w <= MAX_SIDE && h <= MAX_SIDE && (w as f64 / h as f64 - 2.0).abs() < 0.01);
        // 너무 작은 쪽은 1인치 이상으로
        let (w, h) = slide_size_for_page(36.0, 72.0);
        assert!(w >= MIN_SIDE && h >= MIN_SIDE);
    }
}
