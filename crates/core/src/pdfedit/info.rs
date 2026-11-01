//! 문서 정보: Info 사전(제목·작성자·주제·키워드·만든 프로그램·PDF 변환기)과 XMP 메타데이터.
//! 바꾸기는 Info와 XMP를 함께 새로 쓰고, 지우기는 둘 다 없앤다.

use std::path::Path;

use lopdf::{dictionary, Document, Object, Stream};
use serde::{Deserialize, Serialize};

use super::{open, EditOut};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DocInfo {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub keywords: Option<String>,
    #[serde(default)]
    pub creator: Option<String>,
    #[serde(default)]
    pub producer: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InfoRead {
    #[serde(flatten)]
    pub info: DocInfo,
    pub created: Option<String>,
    pub modified: Option<String>,
    /// XMP 메타데이터가 들어 있는지와 크기(바이트)
    pub xmp_bytes: Option<usize>,
    pub pages: u32,
}

const KEYS: [(&str, &[u8]); 6] =
    [("title", b"Title"), ("author", b"Author"), ("subject", b"Subject"), ("keywords", b"Keywords"), ("creator", b"Creator"), ("producer", b"Producer")];

fn field<'a>(i: &'a DocInfo, k: &str) -> &'a Option<String> {
    match k {
        "title" => &i.title,
        "author" => &i.author,
        "subject" => &i.subject,
        "keywords" => &i.keywords,
        "creator" => &i.creator,
        _ => &i.producer,
    }
}

fn field_mut<'a>(i: &'a mut DocInfo, k: &str) -> &'a mut Option<String> {
    match k {
        "title" => &mut i.title,
        "author" => &mut i.author,
        "subject" => &mut i.subject,
        "keywords" => &mut i.keywords,
        "creator" => &mut i.creator,
        _ => &mut i.producer,
    }
}

fn info_dict(doc: &Document) -> Option<lopdf::Dictionary> {
    let o = doc.trailer.get(b"Info").ok()?;
    super::resolve(doc, o).as_dict().ok().cloned()
}

fn text_of(doc: &Document, o: &Object) -> Option<String> {
    let o = super::resolve(doc, o);
    lopdf::decode_text_string(o).ok().map(|s| s.trim_end_matches('\0').to_string())
}

fn xmp_len(doc: &Document) -> Option<usize> {
    let cat = doc.catalog().ok()?;
    let m = cat.get(b"Metadata").ok()?;
    let s = super::resolve(doc, m).as_stream().ok()?;
    Some(s.decompressed_content().map(|v| v.len()).unwrap_or(s.content.len()))
}

pub fn read(input: &Path, password: Option<&str>) -> Result<InfoRead> {
    let doc = open(input, password)?;
    let mut info = DocInfo::default();
    let (mut created, mut modified) = (None, None);
    if let Some(d) = info_dict(&doc) {
        for (k, key) in KEYS {
            if let Ok(v) = d.get(key) {
                *field_mut(&mut info, k) = text_of(&doc, v).filter(|s| !s.is_empty());
            }
        }
        created = d.get(b"CreationDate").ok().and_then(|v| text_of(&doc, v));
        modified = d.get(b"ModDate").ok().and_then(|v| text_of(&doc, v));
    }
    Ok(InfoRead { info, created, modified, xmp_bytes: xmp_len(&doc), pages: doc.get_pages().len() as u32 })
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn pdf_date_now() -> String {
    chrono::Local::now().format("D:%Y%m%d%H%M%S").to_string()
}

fn xmp_packet(i: &DocInfo) -> String {
    let now = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string();
    let mut body = String::new();
    if let Some(t) = &i.title {
        body.push_str(&format!("<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:title>\n", xml_escape(t)));
    }
    if let Some(a) = &i.author {
        body.push_str(&format!("<dc:creator><rdf:Seq><rdf:li>{}</rdf:li></rdf:Seq></dc:creator>\n", xml_escape(a)));
    }
    if let Some(s) = &i.subject {
        body.push_str(&format!("<dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:description>\n", xml_escape(s)));
    }
    if let Some(k) = &i.keywords {
        body.push_str(&format!("<pdf:Keywords>{}</pdf:Keywords>\n", xml_escape(k)));
    }
    if let Some(c) = &i.creator {
        body.push_str(&format!("<xmp:CreatorTool>{}</xmp:CreatorTool>\n", xml_escape(c)));
    }
    if let Some(p) = &i.producer {
        body.push_str(&format!("<pdf:Producer>{}</pdf:Producer>\n", xml_escape(p)));
    }
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n<rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">\n{body}<xmp:ModifyDate>{now}</xmp:ModifyDate>\n<xmp:MetadataDate>{now}</xmp:MetadataDate>\n</rdf:Description>\n</rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>"
    )
}

/// 바꿀 값만 넣는다: `Some("값")` = 바꿈, `Some("")` = 그 항목 지움, `None` = 그대로.
pub fn set(input: &Path, password: Option<&str>, update: &DocInfo, output: &Path) -> Result<EditOut> {
    let mut doc = open(input, password)?;
    let mut d = info_dict(&doc).unwrap_or_default();
    let mut merged = DocInfo::default();
    for (k, key) in KEYS {
        match field(update, k) {
            Some(v) if v.is_empty() => {
                d.remove(key);
            }
            Some(v) => {
                if v.chars().count() > 2000 {
                    return Err(Error::Invalid("문서 정보 값이 너무 깁니다(2000자까지)".into()));
                }
                d.set(key.to_vec(), lopdf::text_string(v));
            }
            None => {}
        }
        *field_mut(&mut merged, k) = d.get(key).ok().and_then(|v| text_of(&doc, v)).filter(|s| !s.is_empty());
    }
    d.set("ModDate", Object::string_literal(pdf_date_now()));
    let info_id = doc.add_object(d);
    doc.trailer.set("Info", info_id);
    // XMP도 같은 값으로 새로 쓴다(예전 XMP에 남은 옛 제목이 보이지 않게)
    let mut xmp = Stream::new(dictionary! { "Type" => "Metadata", "Subtype" => "XML" }, xmp_packet(&merged).into_bytes());
    xmp.allows_compression = false;
    let xmp_id = doc.add_object(xmp);
    doc.catalog_mut().map_err(super::pdf_err(input))?.set("Metadata", xmp_id);
    let pages = doc.get_pages().len() as u32;
    super::save(doc, output)?;
    Ok(EditOut { output: output.to_path_buf(), pages, note: "문서 정보를 바꿨습니다".into() })
}

/// 문서 정보(Info)와 XMP를 모두 지운다.
pub fn clear(input: &Path, password: Option<&str>, output: &Path) -> Result<EditOut> {
    let mut doc = open(input, password)?;
    doc.trailer.remove(b"Info");
    doc.catalog_mut().map_err(super::pdf_err(input))?.remove(b"Metadata");
    // 쪽·그림에 붙은 XMP도 지운다
    let ids: Vec<_> = doc.objects.keys().copied().collect();
    for id in ids {
        match doc.get_object_mut(id) {
            Ok(Object::Dictionary(d)) => {
                if d.get(b"Type").and_then(Object::as_name).map(|t| t == b"Page").unwrap_or(false) {
                    d.remove(b"Metadata");
                }
            }
            Ok(Object::Stream(s)) => {
                s.dict.remove(b"Metadata");
            }
            _ => {}
        }
    }
    let pages = doc.get_pages().len() as u32;
    super::save(doc, output)?;
    Ok(EditOut { output: output.to_path_buf(), pages, note: "문서 정보와 XMP 메타데이터를 지웠습니다".into() })
}
