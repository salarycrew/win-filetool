//! DOCX 쓰기: 자체 스타일(맑은 고딕) — Title, Heading1~3, 목록(ListBullet / ListNumber, 3단계), 표(TableGrid),
//! Caption, Hyperlink. 그림은 본문 안 인라인(쪽 너비에 맞춰 줄임), 표는 격자 표, 쪽 나누기.
//! 글꼴 표(fontTable.xml)에 글꼴 계열을 적어 둬서, 그 글꼴이 없는 PC(LibreOffice 등)도 비슷한 계열로 바꿔 그린다.

use std::collections::BTreeSet;
use std::path::Path;

use super::{app_xml, core_xml, esc, media_type, sniff_ext, Package, FONT_EA, FONT_LATIN};
use crate::error::Result;

pub const EMU_PER_TWIP: i64 = 635;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    /// 하이퍼링크 주소
    pub link: Option<String>,
    /// (라틴, 동아시아) 글꼴. 없으면 문서 기본(맑은 고딕)
    pub font: Option<(String, String)>,
    /// 글자 크기(pt). 없으면 스타일 크기
    pub size_pt: Option<f32>,
}

impl Run {
    pub fn plain(text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Default::default() }
    }
}

pub fn runs_text(runs: &[Run]) -> String {
    runs.iter().map(|r| r.text.as_str()).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// 0 = Title, 1~3 = Heading1~3(4 이상은 3으로)
    Heading { level: u32, runs: Vec<Run> },
    Para(Vec<Run>),
    ListItem { runs: Vec<Run>, level: u32, ordered: bool },
    Caption(Vec<Run>),
    /// 그림(원본 바이트). 크기는 EMU
    Image { bytes: Vec<u8>, name: String, w: i64, h: i64 },
    /// 셀마다 run 목록. header면 첫 줄을 굵게·머리글 반복
    Table { rows: Vec<Vec<Vec<Run>>>, header: bool },
    PageBreak,
}

#[derive(Debug, Clone)]
pub struct Doc {
    pub blocks: Vec<Block>,
    pub title: String,
    pub page_w_twips: i64,
    pub page_h_twips: i64,
    pub margin_twips: i64,
}

impl Doc {
    /// A4, 여백 2cm
    pub fn new(title: impl Into<String>) -> Self {
        Self { blocks: Vec::new(), title: title.into(), page_w_twips: 11_906, page_h_twips: 16_838, margin_twips: 1_134 }
    }

    pub fn text_width_emu(&self) -> i64 {
        (self.page_w_twips - 2 * self.margin_twips).max(1_440) * EMU_PER_TWIP
    }
}

const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn styles_xml() -> String {
    let fonts = format!(r#"<w:rFonts w:ascii="{FONT_LATIN}" w:eastAsia="{FONT_EA}" w:hAnsi="{FONT_LATIN}" w:cs="{FONT_LATIN}"/>"#);
    let heading = |id: &str, name: &str, lvl: u32, sz: u32, before: u32, after: u32| {
        format!(r#"<w:style w:type="paragraph" w:styleId="{id}"><w:name w:val="{name}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="9"/><w:qFormat/><w:pPr><w:keepNext/><w:keepLines/><w:spacing w:before="{before}" w:after="{after}"/><w:outlineLvl w:val="{lvl}"/></w:pPr><w:rPr><w:b/><w:bCs/><w:sz w:val="{sz}"/><w:szCs w:val="{sz}"/></w:rPr></w:style>"#)
    };
    let list = |id: &str, name: &str, num: u32| {
        format!(r#"<w:style w:type="paragraph" w:styleId="{id}"><w:name w:val="{name}"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:numPr><w:numId w:val="{num}"/></w:numPr><w:spacing w:after="60"/><w:contextualSpacing/></w:pPr></w:style>"#)
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles {W_NS}><w:docDefaults><w:rPrDefault><w:rPr>{fonts}<w:sz w:val="21"/><w:szCs w:val="21"/><w:lang w:val="en-US" w:eastAsia="ko-KR" w:bidi="ar-SA"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="120" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/><w:pPr><w:wordWrap w:val="0"/></w:pPr><w:rPr>{fonts}</w:rPr></w:style><w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont"><w:name w:val="Default Paragraph Font"/><w:uiPriority w:val="1"/><w:semiHidden/></w:style><w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="10"/><w:qFormat/><w:pPr><w:spacing w:after="240"/><w:jc w:val="center"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:bCs/><w:sz w:val="44"/><w:szCs w:val="44"/></w:rPr></w:style>{}{}{}{}{}<w:style w:type="paragraph" w:styleId="Caption"><w:name w:val="caption"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="35"/><w:qFormat/><w:pPr><w:jc w:val="center"/><w:spacing w:after="200"/></w:pPr><w:rPr><w:i/><w:color w:val="404040"/><w:sz w:val="18"/><w:szCs w:val="18"/></w:rPr></w:style><w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:basedOn w:val="DefaultParagraphFont"/><w:uiPriority w:val="99"/><w:unhideWhenUsed/><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr></w:style><w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:uiPriority w:val="99"/><w:semiHidden/><w:tblPr><w:tblInd w:w="0" w:type="dxa"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style><w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:basedOn w:val="TableNormal"/><w:uiPriority w:val="39"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="808080"/><w:left w:val="single" w:sz="4" w:space="0" w:color="808080"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="808080"/><w:right w:val="single" w:sz="4" w:space="0" w:color="808080"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="808080"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="808080"/></w:tblBorders></w:tblPr></w:style></w:styles>"#,
        heading("Heading1", "heading 1", 0, 32, 360, 120),
        heading("Heading2", "heading 2", 1, 26, 240, 80),
        heading("Heading3", "heading 3", 2, 23, 200, 60),
        list("ListBullet", "List Bullet", 1),
        list("ListNumber", "List Number", 2),
    )
}

fn numbering_xml(ordered_lists: usize) -> String {
    let lvl = |i: u32, fmt: &str, text: &str| {
        let left = 400 * (i + 1);
        format!(r#"<w:lvl w:ilvl="{i}"><w:start w:val="1"/><w:numFmt w:val="{fmt}"/><w:lvlText w:val="{text}"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{left}" w:hanging="400"/></w:pPr></w:lvl>"#)
    };
    let mut s = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:numbering {W_NS}><w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="hybridMultilevel"/>"#);
    for (i, b) in ["•", "–", "▪"].iter().enumerate() {
        s.push_str(&lvl(i as u32, "bullet", b));
    }
    s.push_str(r#"</w:abstractNum><w:abstractNum w:abstractNumId="1"><w:multiLevelType w:val="hybridMultilevel"/>"#);
    s.push_str(&lvl(0, "decimal", "%1."));
    s.push_str(&lvl(1, "lowerLetter", "%2)"));
    s.push_str(&lvl(2, "lowerRoman", "%3."));
    s.push_str(r#"</w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>"#);
    // 번호 목록마다 1부터 다시
    for k in 0..ordered_lists {
        s.push_str(&format!(r#"<w:num w:numId="{}"><w:abstractNumId w:val="1"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="1"/></w:lvlOverride></w:num>"#, 3 + k));
    }
    s.push_str("</w:numbering>");
    s
}

/// 글꼴 이름 → (계열, 문자 집합). 없는 PC에서 비슷한 글꼴로 바꾸는 데 쓰인다.
fn font_family(name: &str) -> (&'static str, &'static str) {
    let l = name.to_lowercase();
    let serif = ["batang", "바탕", "gungsuh", "궁서", "times", "georgia", "cambria", "myeongjo", "명조", "serif"].iter().any(|k| l.contains(k)) && !l.contains("sans");
    let mono = ["courier", "consolas", "mono"].iter().any(|k| l.contains(k));
    let korean = name.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c)) || ["malgun", "batang", "gulim", "dotum", "gungsuh"].iter().any(|k| l.contains(k));
    (if mono { "modern" } else if serif { "roman" } else { "swiss" }, if korean { "81" } else { "00" })
}

fn font_table(fonts: &BTreeSet<String>) -> String {
    let mut s = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:fonts {W_NS}>"#);
    for f in fonts {
        let (fam, cs) = font_family(f);
        s.push_str(&format!(r#"<w:font w:name="{}"><w:charset w:val="{cs}"/><w:family w:val="{fam}"/><w:pitch w:val="variable"/></w:font>"#, esc(f)));
    }
    s.push_str("</w:fonts>");
    s
}

struct Ctx {
    media: Vec<(String, Vec<u8>)>,
    links: Vec<String>,
    fonts: BTreeSet<String>,
    ordered_lists: usize,
}

impl Ctx {
    fn run(&mut self, r: &Run) -> String {
        let mut p = String::new();
        if r.link.is_some() {
            p.push_str(r#"<w:rStyle w:val="Hyperlink"/>"#);
        }
        if let Some((l, e)) = &r.font {
            self.fonts.insert(l.clone());
            self.fonts.insert(e.clone());
            p.push_str(&format!(r#"<w:rFonts w:ascii="{0}" w:eastAsia="{1}" w:hAnsi="{0}" w:cs="{0}"/>"#, esc(l), esc(e)));
        }
        if r.bold {
            p.push_str("<w:b/><w:bCs/>");
        }
        if r.italic {
            p.push_str("<w:i/><w:iCs/>");
        }
        if let Some(sz) = r.size_pt {
            let v = (sz * 2.0).round().clamp(2.0, 3276.0) as i32;
            p.push_str(&format!(r#"<w:sz w:val="{v}"/><w:szCs w:val="{v}"/>"#));
        }
        let rpr = if p.is_empty() { String::new() } else { format!("<w:rPr>{p}</w:rPr>") };
        // 탭·줄바꿈은 Word 요소로
        let mut body = String::new();
        let mut buf = String::new();
        let flush = |buf: &mut String, body: &mut String| {
            if !buf.is_empty() {
                body.push_str(&format!(r#"<w:t xml:space="preserve">{}</w:t>"#, esc(buf)));
                buf.clear();
            }
        };
        for c in r.text.chars() {
            match c {
                '\t' => {
                    flush(&mut buf, &mut body);
                    body.push_str("<w:tab/>");
                }
                '\n' => {
                    flush(&mut buf, &mut body);
                    body.push_str("<w:br/>");
                }
                c => buf.push(c),
            }
        }
        flush(&mut buf, &mut body);
        format!("<w:r>{rpr}{body}</w:r>")
    }

    fn runs(&mut self, runs: &[Run]) -> String {
        let mut s = String::new();
        let mut i = 0;
        while i < runs.len() {
            if let Some(url) = &runs[i].link {
                // 같은 주소의 이어진 run을 하나의 링크로
                let mut j = i;
                let mut inner = String::new();
                while j < runs.len() && runs[j].link.as_ref() == Some(url) {
                    inner.push_str(&self.run(&runs[j]));
                    j += 1;
                }
                let id = match self.links.iter().position(|u| u == url) {
                    Some(k) => k + 1,
                    None => {
                        self.links.push(url.clone());
                        self.links.len()
                    }
                };
                s.push_str(&format!(r#"<w:hyperlink r:id="rIdLink{id}" w:history="1">{inner}</w:hyperlink>"#));
                i = j;
            } else {
                s.push_str(&self.run(&runs[i]));
                i += 1;
            }
        }
        s
    }

    fn image(&mut self, bytes: &[u8], name: &str, w: i64, h: i64, max_w: i64, max_h: i64) -> String {
        let ext = sniff_ext(bytes).unwrap_or_else(|| media_type(name).0);
        // 같은 그림은 한 번만
        let id = match self.media.iter().position(|(_, b)| b == bytes) {
            Some(k) => k + 1,
            None => {
                self.media.push((format!("image{}.{ext}", self.media.len() + 1), bytes.to_vec()));
                self.media.len()
            }
        };
        let (mut w, mut h) = (w.max(1), h.max(1));
        if w > max_w {
            h = h * max_w / w;
            w = max_w;
        }
        if h > max_h {
            w = w * max_h / h;
            h = max_h;
        }
        format!(
            r#"<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{w}" cy="{h}"/><wp:docPr id="{{DOCPR}}" name="그림 {id}"/><wp:cNvGraphicFramePr><a:graphicFrameLocks xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" noChangeAspect="1"/></wp:cNvGraphicFramePr><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:nvPicPr><pic:cNvPr id="{id}" name="image{id}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rIdImg{id}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{w}" cy="{h}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#
        )
    }
}

impl Doc {
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut cx = Ctx { media: Vec::new(), links: Vec::new(), fonts: BTreeSet::new(), ordered_lists: 0 };
        cx.fonts.insert(FONT_LATIN.into());
        cx.fonts.insert(FONT_EA.into());
        let text_w_twips = (self.page_w_twips - 2 * self.margin_twips).max(1_440);
        let max_w = text_w_twips * EMU_PER_TWIP;
        let max_h = (self.page_h_twips - 2 * self.margin_twips - 400).max(1_440) * EMU_PER_TWIP;
        let mut body = String::new();
        let mut prev_ordered = false;
        let mut cur_num = 2;
        for b in &self.blocks {
            let is_ordered = matches!(b, Block::ListItem { ordered: true, .. });
            if is_ordered && !prev_ordered {
                cur_num = 3 + cx.ordered_lists;
                cx.ordered_lists += 1;
            }
            prev_ordered = is_ordered || (prev_ordered && matches!(b, Block::ListItem { .. }));
            match b {
                Block::Heading { level, runs } => {
                    let style = if *level == 0 { "Title".to_string() } else { format!("Heading{}", (*level).min(3)) };
                    body.push_str(&format!(r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr>{}</w:p>"#, cx.runs(runs)));
                }
                Block::Para(runs) => body.push_str(&format!("<w:p>{}</w:p>", cx.runs(runs))),
                Block::Caption(runs) => body.push_str(&format!(r#"<w:p><w:pPr><w:pStyle w:val="Caption"/></w:pPr>{}</w:p>"#, cx.runs(runs))),
                Block::ListItem { runs, level, ordered } => {
                    let (style, num) = if *ordered { ("ListNumber", cur_num) } else { ("ListBullet", 1) };
                    body.push_str(&format!(r#"<w:p><w:pPr><w:pStyle w:val="{style}"/><w:numPr><w:ilvl w:val="{}"/><w:numId w:val="{num}"/></w:numPr></w:pPr>{}</w:p>"#, (*level).min(2), cx.runs(runs)));
                }
                Block::PageBreak => body.push_str(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#),
                Block::Image { bytes, name, w, h } => {
                    let r = cx.image(bytes, name, *w, *h, max_w, max_h);
                    body.push_str(&format!(r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr>{r}</w:p>"#));
                }
                Block::Table { rows, header } => {
                    let ncol = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                    let cw = text_w_twips / ncol as i64;
                    body.push_str(r#"<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="5000" w:type="pct"/><w:tblLook w:val="04A0" w:firstRow="1" w:lastRow="0" w:firstColumn="0" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/></w:tblPr><w:tblGrid>"#);
                    for _ in 0..ncol {
                        body.push_str(&format!(r#"<w:gridCol w:w="{cw}"/>"#));
                    }
                    body.push_str("</w:tblGrid>");
                    for (ri, r) in rows.iter().enumerate() {
                        let head = *header && ri == 0;
                        body.push_str(if head { "<w:tr><w:trPr><w:tblHeader/></w:trPr>" } else { "<w:tr>" });
                        for c in 0..ncol {
                            let cell: Vec<Run> = r.get(c).cloned().unwrap_or_default().into_iter().map(|mut x| {
                                x.bold |= head;
                                x
                            }).collect();
                            let shade = if head { r#"<w:shd w:val="clear" w:color="auto" w:fill="DCE6F2"/>"# } else { "" };
                            body.push_str(&format!(r#"<w:tc><w:tcPr><w:tcW w:w="{cw}" w:type="dxa"/>{shade}</w:tcPr><w:p><w:pPr><w:spacing w:after="0"/></w:pPr>{}</w:p></w:tc>"#, cx.runs(&cell)));
                        }
                        body.push_str("</w:tr>");
                    }
                    body.push_str(r#"</w:tbl><w:p><w:pPr><w:spacing w:after="0"/></w:pPr></w:p>"#);
                }
            }
        }
        // 그림 docPr id는 문서 안에서 서로 달라야 한다
        let mut n = 0;
        while let Some(p) = body.find("{DOCPR}") {
            n += 1;
            body.replace_range(p..p + 7, &n.to_string());
        }
        let doc = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W_NS} xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><w:body>{body}<w:sectPr><w:pgSz w:w="{}" w:h="{}"{}/><w:pgMar w:top="{m}" w:right="{m}" w:bottom="{m}" w:left="{m}" w:header="567" w:footer="567" w:gutter="0"/><w:cols w:space="425"/><w:docGrid w:linePitch="360"/></w:sectPr></w:body></w:document>"#,
            self.page_w_twips,
            self.page_h_twips,
            if self.page_w_twips > self.page_h_twips { r#" w:orient="landscape""# } else { "" },
            m = self.margin_twips
        );
        let mut z = Package::new();
        let mut exts: Vec<(&str, &str)> = cx.media.iter().map(|(n, _)| media_type(n)).collect();
        exts.sort();
        exts.dedup();
        let mut ct = String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>"#);
        for (e, c) in &exts {
            ct.push_str(&format!(r#"<Default Extension="{e}" ContentType="{c}"/>"#));
        }
        ct.push_str(r#"<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/><Override PartName="/word/fontTable.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.fontTable+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/><Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/></Types>"#);
        z.put("[Content_Types].xml", ct.as_bytes())?;
        z.put("_rels/.rels", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="{REL_NS}"><Relationship Id="rId1" Type="{R}/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/><Relationship Id="rId3" Type="{R}/extended-properties" Target="docProps/app.xml"/></Relationships>"#).as_bytes())?;
        z.put("docProps/core.xml", core_xml(&self.title).as_bytes())?;
        z.put("docProps/app.xml", app_xml().as_bytes())?;
        let mut rels = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="{REL_NS}"><Relationship Id="rId1" Type="{R}/styles" Target="styles.xml"/><Relationship Id="rId2" Type="{R}/numbering" Target="numbering.xml"/><Relationship Id="rId3" Type="{R}/fontTable" Target="fontTable.xml"/><Relationship Id="rId4" Type="{R}/settings" Target="settings.xml"/>"#);
        for (i, (n, _)) in cx.media.iter().enumerate() {
            rels.push_str(&format!(r#"<Relationship Id="rIdImg{}" Type="{R}/image" Target="media/{n}"/>"#, i + 1));
        }
        for (i, u) in cx.links.iter().enumerate() {
            rels.push_str(&format!(r#"<Relationship Id="rIdLink{}" Type="{R}/hyperlink" Target="{}" TargetMode="External"/>"#, i + 1, esc(u)));
        }
        rels.push_str("</Relationships>");
        z.put("word/_rels/document.xml.rels", rels.as_bytes())?;
        z.put("word/document.xml", doc.as_bytes())?;
        z.put("word/styles.xml", styles_xml().as_bytes())?;
        z.put("word/numbering.xml", numbering_xml(cx.ordered_lists).as_bytes())?;
        z.put("word/fontTable.xml", font_table(&cx.fonts).as_bytes())?;
        z.put("word/settings.xml", format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:settings {W_NS}><w:defaultTabStop w:val="800"/><w:characterSpacingControl w:val="doNotCompress"/><w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat></w:settings>"#).as_bytes())?;
        for (n, d) in &cx.media {
            z.put(&format!("word/media/{n}"), d)?;
        }
        z.finish()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        super::save(path, &self.to_bytes()?)
    }

    /// 구조 세기(시험·보고용)
    pub fn count(&self) -> DocStats {
        let mut s = DocStats::default();
        for b in &self.blocks {
            match b {
                Block::Heading { .. } => s.headings += 1,
                Block::Para(_) => s.paragraphs += 1,
                Block::ListItem { .. } => s.list_items += 1,
                Block::Caption(_) => s.captions += 1,
                Block::Image { .. } => s.images += 1,
                Block::Table { .. } => s.tables += 1,
                Block::PageBreak => {}
            }
        }
        s
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct DocStats {
    pub headings: usize,
    pub paragraphs: usize,
    pub list_items: usize,
    pub captions: usize,
    pub tables: usize,
    pub images: usize,
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
    fn doc_parts() {
        let mut d = Doc::new("보고서");
        let png = vec![0x89, b'P', b'N', b'G', 0, 0];
        d.blocks = vec![
            Block::Heading { level: 0, runs: vec![Run::plain("제목")] },
            Block::Heading { level: 5, runs: vec![Run::plain("깊은 제목")] },
            Block::Para(vec![Run::plain("굵게 "), Run { text: "링크".into(), link: Some("https://example.com/?a=1&b=2".into()), ..Default::default() }, Run { text: "기울임\t탭".into(), italic: true, font: Some(("Times New Roman".into(), "바탕".into())), ..Default::default() }]),
            Block::ListItem { runs: vec![Run::plain("하나")], level: 0, ordered: true },
            Block::ListItem { runs: vec![Run::plain("둘")], level: 0, ordered: true },
            Block::Para(vec![Run::plain("사이")]),
            Block::ListItem { runs: vec![Run::plain("다시 하나")], level: 0, ordered: true },
            Block::ListItem { runs: vec![Run::plain("점")], level: 1, ordered: false },
            Block::Image { bytes: png.clone(), name: "x.png".into(), w: 99_000_000, h: 9_900_000 },
            Block::Caption(vec![Run::plain("그림 1. 설명")]),
            Block::Image { bytes: png, name: "y.png".into(), w: 100, h: 100 },
            Block::Table { rows: vec![vec![vec![Run::plain("항목")], vec![Run::plain("값")]], vec![vec![Run::plain("가")]]], header: true },
            Block::PageBreak,
        ];
        let st = d.count();
        assert_eq!((st.headings, st.paragraphs, st.list_items, st.captions, st.images, st.tables), (2, 2, 4, 1, 2, 1));
        let b = d.to_bytes().unwrap();
        let doc = part(&b, "word/document.xml");
        assert!(doc.contains(r#"<w:pStyle w:val="Title"/>"#) && doc.contains(r#"<w:pStyle w:val="Heading3"/>"#));
        assert!(doc.contains(r#"<w:hyperlink r:id="rIdLink1""#));
        assert!(doc.contains("<w:tab/>"));
        assert!(doc.contains(r#"<w:numId w:val="3"/>"#) && doc.contains(r#"<w:numId w:val="4"/>"#), "두 번호 목록은 각각 1부터");
        assert!(doc.contains(r#"<w:pStyle w:val="Caption"/>"#));
        assert!(doc.contains(r#"<wp:docPr id="1""#) && doc.contains(r#"<wp:docPr id="2""#) && !doc.contains("{DOCPR}"));
        // 쪽 너비보다 큰 그림은 줄임
        let max_w = d.text_width_emu();
        assert!(doc.contains(&format!(r#"<wp:extent cx="{max_w}""#)));
        let rels = part(&b, "word/_rels/document.xml.rels");
        assert!(rels.contains("https://example.com/?a=1&amp;b=2") && rels.contains(r#"TargetMode="External""#));
        assert_eq!(rels.matches("/image\"").count(), 1, "같은 그림은 한 번만");
        assert!(part(&b, "word/numbering.xml").contains(r#"<w:num w:numId="4">"#));
        let ft = part(&b, "word/fontTable.xml");
        assert!(ft.contains(r#"w:name="Times New Roman""#) && ft.contains(r#"<w:family w:val="roman"/>"#));
        assert!(part(&b, "word/styles.xml").contains(r#"w:styleId="ListNumber""#));
    }
}
