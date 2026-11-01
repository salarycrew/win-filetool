//! 0.4 진짜 가리기(redaction). 가린 내용이 덮이기만 하는 게 아니라 파일에서 없어지게 한다.
//!
//! [`apply`] 순서:
//! 1. 양식 칸을 쪽 내용으로 굳힌다(값이 양식 사전에 남지 않게) — 다른 주석은 그대로
//! 2. 문서 전체: 문서 정보(Info)·XMP·PieceInfo, 첨부 파일(EmbeddedFiles·FileAttachment 주석·AF), 자바스크립트(이름 트리·동작·AA),
//!    숨긴 레이어(OCG가 꺼진 내용은 내용 스트림에서 지움), 태그 구조(StructTreeRoot — 대체 글에 원문이 남을 수 있음)를 지운다
//! 3. 가릴 곳이 있는 쪽만: PDFium으로 쪽을 그림(기본 200dpi, 300 가능)으로 그리고 검은 상자를 그림에 직접 칠한 뒤,
//!    쪽 내용을 그 그림 하나로 바꾼다(글자·글꼴·주석·링크·썸네일이 모두 없어짐). 가리지 않은 쪽은 손대지 않는다(글자 그대로)
//! 4. (기본 켬) 가린 쪽 그림을 글자 인식해 보이지 않는 글자 층을 다시 얹는다 — 검은 상자와 겹치는 낱말은 넣지 않는다
//! 5. 새 파일로 저장(쓰지 않는 객체는 지움, 예전 판 기록 없음) → 가린 곳 안에 글자가 남지 않았는지, 찾은 글이 어디에도 없는지 확인
//!
//! [`find`]: 찾는 말 + 미리 만든 규칙(주민등록번호·전화번호·이메일·계좌번호)을 PDFium 글자(글자마다 상자)에서 찾고,
//! 글자가 없는 쪽(스캔)은 글자 인식 결과에서 찾는다. 결과는 쪽마다 상자 목록(보이는 쪽 pt, 왼쪽 위 원점) — 화면이 고르게 보여 주고,
//! 사용자가 [가리기]를 눌러야 [`apply`]로 간다.
//!
//! [`verify`]: PDFium 글자 추출, 모든 스트림을 풀어 바이트로(UTF-8·UTF-16BE·16진 글자), 모든 문자열·이름 객체, 문서 정보에서 찾는다.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use image::{DynamicImage, RgbImage};
use lopdf::content::Content;
use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};

use super::{inherited, open, page_geom, page_ids, resolve, EditOut, Job, PageGeom};
use crate::error::{io_err, Error, Result};
use crate::ocr::pdf::{add_layer, placed_words, render, text_chars, user_to_visual};
use crate::ocr::{recognize, Recognizer};

/// 가릴 곳 하나(보이는 쪽 pt, 원점 왼쪽 위 — 미리보기와 같은 방향)
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    /// 1부터
    pub page: u32,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

fn d200() -> u32 {
    200
}
fn dtrue() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactOpts {
    /// 가린 쪽을 그릴 해상도: 200(기본) | 300
    #[serde(default = "d200")]
    pub dpi: u32,
    /// 가린 쪽도 검색되게 글자 인식(기본 켬, 검은 상자 안은 빼고)
    #[serde(default = "dtrue")]
    pub ocr: bool,
    /// 결과에 남지 않았는지 확인할 글(찾아서 가리기에서 고른 글)
    #[serde(default)]
    pub check_terms: Vec<String>,
}

impl Default for RedactOpts {
    fn default() -> Self {
        RedactOpts { dpi: 200, ocr: true, check_terms: vec![] }
    }
}

/// 지운 것(보고용)
#[derive(Debug, Clone, Default, Serialize)]
pub struct Removed {
    /// 가린 쪽의 주석(메모·도형 등, 링크·양식 칸 제외)
    pub annotations: usize,
    /// 가린 쪽의 링크
    pub links: usize,
    /// 문서의 양식 칸(굳힌 뒤 양식 정보는 지움)
    pub form_fields: usize,
    pub attachments: usize,
    pub javascript: usize,
    /// 숨긴 레이어에서 지운 내용 조각
    pub hidden_layers: usize,
    /// 문서 정보·XMP가 있었는지
    pub metadata: bool,
    /// 태그 구조(접근성 정보)가 있었는지
    pub structure: bool,
}

/// 확인 결과
#[derive(Debug, Clone, Default, Serialize)]
pub struct VerifyOut {
    /// 아무 데서도 안 나왔는지
    pub clean: bool,
    /// 확인한 글 수
    pub terms: usize,
    /// PDFium 글자에서 찾은 (쪽, 글)
    pub text_hits: Vec<(u32, String)>,
    /// 풀어 본 스트림 수 / 그 안에서 찾은 수
    pub streams: usize,
    pub stream_hits: usize,
    /// 문자열·이름 객체에서 찾은 수
    pub string_hits: usize,
    /// 문서 정보(Info)·XMP가 남아 있는지
    pub has_info: bool,
    pub has_xmp: bool,
    /// 가린 곳 안에 남은 글자 수(가린 곳을 줬을 때)
    pub chars_in_marks: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RedactOut {
    pub output: PathBuf,
    pub pages: u32,
    /// 그림으로 바꾼 쪽(1부터)
    pub redacted_pages: Vec<u32>,
    pub marks: usize,
    /// 글자 층을 다시 얹은 쪽 수
    pub ocr_pages: u32,
    pub removed: Removed,
    pub verify: VerifyOut,
    pub note: String,
    /// 글자 인식을 못 했을 때 안내
    pub hint: Option<String>,
}

// ---------- 문서 전체 정리 ----------

fn all_ids(doc: &Document) -> Vec<ObjectId> {
    doc.objects.keys().copied().collect()
}

fn dict_of_mut(o: &mut Object) -> Option<&mut Dictionary> {
    match o {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&mut s.dict),
        _ => None,
    }
}

fn name_is(d: &Dictionary, key: &[u8], v: &[u8]) -> bool {
    d.get(key).ok().and_then(|o| o.as_name().ok()).is_some_and(|n| n == v)
}

/// 문서 정보·XMP·PieceInfo. 있었으면 true
fn strip_metadata(doc: &mut Document) -> bool {
    let mut had = doc.trailer.remove(b"Info").is_some();
    for id in all_ids(doc) {
        if let Some(d) = doc.objects.get_mut(&id).and_then(dict_of_mut) {
            had |= d.remove(b"Metadata").is_some();
            d.remove(b"PieceInfo");
        }
    }
    // Info로 쓰이던 사전이 다른 곳에서 안 쓰이면 저장할 때 지워진다(prune)
    had
}

/// 자바스크립트: 이름 트리, JS 동작을 가리키는 A/OpenAction/Next, 추가 동작 AA. 지운 개수
fn strip_javascript(doc: &mut Document) -> usize {
    let mut n = 0;
    let js_ids: HashSet<ObjectId> = doc
        .objects
        .iter()
        .filter_map(|(id, o)| match o {
            Object::Dictionary(d) if name_is(d, b"S", b"JavaScript") || d.has(b"JS") => Some(*id),
            _ => None,
        })
        .collect();
    let is_js = |o: &Object, ids: &HashSet<ObjectId>| match o {
        Object::Reference(r) => ids.contains(r),
        Object::Dictionary(d) => name_is(d, b"S", b"JavaScript") || d.has(b"JS"),
        _ => false,
    };
    for id in all_ids(doc) {
        let Some(d) = doc.objects.get_mut(&id).and_then(dict_of_mut) else { continue };
        for key in [&b"A"[..], b"OpenAction", b"Next"] {
            if d.get(key).ok().is_some_and(|o| is_js(o, &js_ids)) {
                d.remove(key);
                n += 1;
            }
        }
        if d.remove(b"AA").is_some() {
            n += 1;
        }
    }
    for id in &js_ids {
        if let Some(Object::Dictionary(d)) = doc.objects.get_mut(id) {
            if d.remove(b"JS").is_some() {
                n += 1;
            }
        }
    }
    if let Some(names) = names_dict_id(doc) {
        if let Some(d) = doc.objects.get_mut(&names).and_then(dict_of_mut) {
            if d.remove(b"JavaScript").is_some() {
                n += 1;
            }
        }
    } else if let Ok(cat) = doc.catalog_mut() {
        if let Ok(Object::Dictionary(names)) = cat.get_mut(b"Names") {
            if names.remove(b"JavaScript").is_some() {
                n += 1;
            }
        }
    }
    n
}

/// 카탈로그 /Names가 간접 객체면 그 id
fn names_dict_id(doc: &Document) -> Option<ObjectId> {
    doc.catalog().ok().and_then(|c| c.get(b"Names").ok()).and_then(|o| o.as_reference().ok())
}

/// 첨부 파일: EmbeddedFiles 이름 트리, FileAttachment 주석, AF(연결된 파일), 포트폴리오(Collection). 첨부 개수
fn strip_attachments(doc: &mut Document) -> usize {
    let embedded = doc.objects.values().filter(|o| matches!(o, Object::Stream(s) if name_is(&s.dict, b"Type", b"EmbeddedFile"))).count();
    let mut n = embedded;
    if let Some(names) = names_dict_id(doc) {
        if let Some(d) = doc.objects.get_mut(&names).and_then(dict_of_mut) {
            d.remove(b"EmbeddedFiles");
        }
    } else if let Ok(cat) = doc.catalog_mut() {
        if let Ok(Object::Dictionary(names)) = cat.get_mut(b"Names") {
            names.remove(b"EmbeddedFiles");
        }
    }
    if let Ok(cat) = doc.catalog_mut() {
        cat.remove(b"Collection");
    }
    for id in all_ids(doc) {
        if let Some(d) = doc.objects.get_mut(&id).and_then(dict_of_mut) {
            d.remove(b"AF");
        }
    }
    // FileAttachment 주석
    for pid in page_ids(doc) {
        let annots = match doc.get_dictionary(pid).ok().and_then(|d| d.get(b"Annots").ok()).map(|a| resolve(doc, a).clone()) {
            Some(Object::Array(a)) => a,
            _ => continue,
        };
        let keep: Vec<Object> = annots
            .iter()
            .filter(|a| {
                let d = match resolve(doc, a) {
                    Object::Dictionary(d) => d,
                    _ => return true,
                };
                let fa = name_is(d, b"Subtype", b"FileAttachment");
                if fa && embedded == 0 {
                    n += 1;
                }
                !fa
            })
            .cloned()
            .collect();
        if keep.len() != annots.len() {
            if let Ok(d) = doc.get_dictionary_mut(pid) {
                d.set("Annots", Object::Array(keep));
            }
        }
    }
    n
}

/// 태그 구조(대체 글·실제 글에 원문이 남을 수 있음). 있었으면 true
fn strip_structure(doc: &mut Document) -> bool {
    let mut had = false;
    if let Ok(cat) = doc.catalog_mut() {
        had = cat.remove(b"StructTreeRoot").is_some();
        cat.remove(b"MarkInfo");
    }
    for id in all_ids(doc) {
        if let Some(d) = doc.objects.get_mut(&id).and_then(dict_of_mut) {
            d.remove(b"StructParents");
            d.remove(b"StructParent");
        }
    }
    had
}

// ---------- 숨긴 레이어(OCG) ----------

struct Ocg {
    hidden: HashSet<ObjectId>,
}

impl Ocg {
    fn read(doc: &Document) -> Option<Ocg> {
        let cat = doc.catalog().ok()?;
        let ocp = resolve(doc, cat.get(b"OCProperties").ok()?).as_dict().ok()?;
        let all: Vec<ObjectId> = ocp.get(b"OCGs").ok().map(|o| resolve(doc, o)).and_then(|o| o.as_array().ok()).map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect()).unwrap_or_default();
        let d = ocp.get(b"D").ok().map(|o| resolve(doc, o)).and_then(|o| o.as_dict().ok());
        let list = |k: &[u8]| -> HashSet<ObjectId> {
            d.and_then(|d| d.get(k).ok()).map(|o| resolve(doc, o)).and_then(|o| o.as_array().ok()).map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect()).unwrap_or_default()
        };
        let base_off = d.is_some_and(|d| name_is(d, b"BaseState", b"OFF"));
        let hidden = if base_off {
            let on = list(b"ON");
            all.into_iter().filter(|g| !on.contains(g)).collect()
        } else {
            list(b"OFF")
        };
        Some(Ocg { hidden })
    }

    /// OCG 또는 OCMD(값)가 지금 안 보이는지
    fn is_hidden(&self, doc: &Document, o: &Object) -> bool {
        let id = o.as_reference().ok();
        if let Some(id) = id {
            if self.hidden.contains(&id) {
                return true;
            }
        }
        let Ok(d) = resolve(doc, o).as_dict() else { return false };
        if !name_is(d, b"Type", b"OCMD") {
            return false;
        }
        let groups: Vec<ObjectId> = match d.get(b"OCGs").ok().map(|o| resolve(doc, o)) {
            Some(Object::Array(a)) => a.iter().filter_map(|o| o.as_reference().ok()).collect(),
            Some(_) => d.get(b"OCGs").ok().and_then(|o| o.as_reference().ok()).into_iter().collect(),
            None => vec![],
        };
        if groups.is_empty() {
            return false;
        }
        let vis: Vec<bool> = groups.iter().map(|g| !self.hidden.contains(g)).collect();
        let p = d.get(b"P").ok().and_then(|o| o.as_name().ok()).unwrap_or(b"AnyOn");
        let visible = match p {
            b"AllOn" => vis.iter().all(|v| *v),
            b"AnyOff" => vis.iter().any(|v| !*v),
            b"AllOff" => vis.iter().all(|v| !*v),
            _ => vis.iter().any(|v| *v),
        };
        !visible
    }
}

/// 내용 스트림에서 숨긴 레이어 조각(`/OC /이름 BDC … EMC`)과 숨긴 XObject 그리기(`Do`)를 뺀다. (새 바이트, 뺀 조각 수)
fn filter_hidden(doc: &Document, ocg: &Ocg, bytes: &[u8], res: Option<&Dictionary>) -> Result<(Vec<u8>, usize)> {
    let content = Content::decode(bytes).map_err(|e| Error::Invalid(format!("쪽 내용을 읽지 못했습니다: {e}")))?;
    let props = res.and_then(|r| r.get(b"Properties").ok()).map(|o| resolve(doc, o)).and_then(|o| o.as_dict().ok());
    let xobjs = res.and_then(|r| r.get(b"XObject").ok()).map(|o| resolve(doc, o)).and_then(|o| o.as_dict().ok());
    let mut out = Vec::with_capacity(content.operations.len());
    let mut depth = 0usize;
    let mut skip: Option<usize> = None;
    let mut removed = 0;
    for op in content.operations {
        match op.operator.as_str() {
            "BDC" | "BMC" => {
                depth += 1;
                if skip.is_some() {
                    continue;
                }
                if op.operator == "BDC" && op.operands.first().and_then(|o| o.as_name().ok()) == Some(b"OC") {
                    let target = match op.operands.get(1) {
                        Some(Object::Name(n)) => props.and_then(|p| p.get(n).ok()).cloned(),
                        Some(o) => Some(o.clone()),
                        None => None,
                    };
                    if target.is_some_and(|t| ocg.is_hidden(doc, &t)) {
                        skip = Some(depth);
                        removed += 1;
                        continue;
                    }
                }
            }
            "EMC" => {
                if let Some(d) = skip {
                    if depth == d {
                        skip = None;
                    }
                    depth = depth.saturating_sub(1);
                    continue;
                }
                depth = depth.saturating_sub(1);
            }
            "Do" if skip.is_none() => {
                let hidden = op.operands.first().and_then(|o| o.as_name().ok()).and_then(|n| xobjs.and_then(|x| x.get(n).ok())).map(|o| resolve(doc, o)).and_then(|o| match o {
                    Object::Stream(s) => s.dict.get(b"OC").ok().cloned(),
                    _ => None,
                });
                if hidden.is_some_and(|oc| ocg.is_hidden(doc, &oc)) {
                    removed += 1;
                    continue;
                }
            }
            _ => {
                if skip.is_some() {
                    continue;
                }
            }
        }
        out.push(op);
    }
    if removed == 0 {
        return Ok((bytes.to_vec(), 0));
    }
    let enc = Content { operations: out }.encode().map_err(|e| Error::Invalid(e.to_string()))?;
    Ok((enc, removed))
}

/// 숨긴 레이어 내용을 지우고 OCProperties를 없앤다. 내용을 못 읽은 쪽(0부터)은 `raster`에 넣어 그림으로 바꾸게 한다.
fn strip_hidden_ocg(doc: &mut Document, raster: &mut BTreeSet<usize>) -> Result<usize> {
    let Some(ocg) = Ocg::read(doc) else { return Ok(0) };
    let mut removed = 0;
    if !ocg.hidden.is_empty() {
        let mut done_forms: HashSet<ObjectId> = HashSet::new();
        for (i, pid) in page_ids(doc).into_iter().enumerate() {
            let res = inherited(doc, pid, b"Resources").map(|o| resolve(doc, &o).clone()).and_then(|o| o.as_dict().ok().cloned());
            let bytes = super::page_content_bytes(doc, pid)?;
            match filter_hidden(doc, &ocg, &bytes, res.as_ref()) {
                Ok((nb, k)) if k > 0 => {
                    let mut s = Stream::new(dictionary! {}, nb);
                    let _ = s.compress();
                    let sid = doc.add_object(s);
                    doc.get_dictionary_mut(pid).map_err(|e| Error::Invalid(e.to_string()))?.set("Contents", sid);
                    removed += k;
                }
                Ok(_) => {}
                Err(_) => {
                    raster.insert(i);
                }
            }
            // 쪽이 그리는 Form XObject 안쪽(한 단계)
            let forms: Vec<ObjectId> = res
                .as_ref()
                .and_then(|r| r.get(b"XObject").ok())
                .map(|o| resolve(doc, o))
                .and_then(|o| o.as_dict().ok())
                .map(|x| x.iter().filter_map(|(_, v)| v.as_reference().ok()).collect())
                .unwrap_or_default();
            for fid in forms {
                if !done_forms.insert(fid) {
                    continue;
                }
                let (bytes, fres) = match doc.get_object(fid) {
                    Ok(Object::Stream(s)) if name_is(&s.dict, b"Subtype", b"Form") => {
                        let b = s.decompressed_content().unwrap_or_else(|_| s.content.clone());
                        let r = s.dict.get(b"Resources").ok().map(|o| resolve(doc, o).clone()).and_then(|o| o.as_dict().ok().cloned()).or_else(|| res.clone());
                        (b, r)
                    }
                    _ => continue,
                };
                match filter_hidden(doc, &ocg, &bytes, fres.as_ref()) {
                    Ok((nb, k)) if k > 0 => {
                        if let Ok(Object::Stream(s)) = doc.get_object_mut(fid) {
                            s.dict.remove(b"Filter");
                            s.dict.remove(b"DecodeParms");
                            s.set_content(nb);
                            let _ = s.compress();
                        }
                        removed += k;
                    }
                    Ok(_) => {}
                    Err(_) => {
                        raster.insert(i);
                    }
                }
            }
            // 숨긴 레이어의 주석
            let annots = match doc.get_dictionary(pid).ok().and_then(|d| d.get(b"Annots").ok()).map(|a| resolve(doc, a).clone()) {
                Some(Object::Array(a)) => a,
                _ => continue,
            };
            let keep: Vec<Object> = annots.iter().filter(|a| !resolve(doc, a).as_dict().ok().and_then(|d| d.get(b"OC").ok()).is_some_and(|oc| ocg.is_hidden(doc, oc))).cloned().collect();
            if keep.len() != annots.len() {
                removed += annots.len() - keep.len();
                if let Ok(d) = doc.get_dictionary_mut(pid) {
                    d.set("Annots", Object::Array(keep));
                }
            }
        }
    }
    if let Ok(cat) = doc.catalog_mut() {
        cat.remove(b"OCProperties");
    }
    Ok(removed)
}

// ---------- 쪽 바꾸기 ----------

fn burn(img: &mut RgbImage, marks: &[Mark], px_per_pt_x: f32, px_per_pt_y: f32) {
    let (w, h) = (img.width() as i64, img.height() as i64);
    for m in marks {
        // 1px 더 넓게(안티에일리어싱 가장자리까지)
        let x0 = ((m.x * px_per_pt_x).floor() as i64 - 1).clamp(0, w);
        let y0 = ((m.y * px_per_pt_y).floor() as i64 - 1).clamp(0, h);
        let x1 = (((m.x + m.w) * px_per_pt_x).ceil() as i64 + 1).clamp(0, w);
        let y1 = (((m.y + m.h) * px_per_pt_y).ceil() as i64 + 1).clamp(0, h);
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x as u32, y as u32, image::Rgb([0, 0, 0]));
            }
        }
    }
}

fn is_gray(img: &RgbImage) -> bool {
    img.pixels().all(|p| {
        let [r, g, b] = p.0;
        r.abs_diff(g) <= 6 && g.abs_diff(b) <= 6 && r.abs_diff(b) <= 6
    })
}

/// 쪽 그림 → JPEG 이미지 XObject(흑백이면 DeviceGray)
fn jpeg_xobject(doc: &mut Document, img: &RgbImage) -> Result<ObjectId> {
    let gray = is_gray(img);
    let mut bytes = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 88);
    let r = if gray { enc.encode_image(&DynamicImage::ImageRgb8(img.clone()).to_luma8()) } else { enc.encode_image(img) };
    r.map_err(|e| Error::Invalid(format!("쪽 그림을 만들지 못했습니다: {e}")))?;
    let s = Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Image", "Width" => img.width() as i64, "Height" => img.height() as i64,
            "ColorSpace" => if gray { "DeviceGray" } else { "DeviceRGB" }, "BitsPerComponent" => 8, "Filter" => "DCTDecode",
        },
        bytes,
    )
    .with_compression(false);
    Ok(doc.add_object(s))
}

/// 쪽의 내용·리소스·주석을 모두 버리고 그림 한 장만 남긴다(보이는 크기, 회전 0)
fn replace_page(doc: &mut Document, pid: ObjectId, image: ObjectId, vw: f32, vh: f32) -> Result<()> {
    let ops = format!("q {} 0 0 {} 0 0 cm /Im0 Do Q\n", super::f(vw), super::f(vh));
    let mut cs = Stream::new(dictionary! {}, ops.into_bytes());
    let _ = cs.compress();
    let cid = doc.add_object(cs);
    let d = doc.get_dictionary_mut(pid).map_err(|e| Error::Invalid(e.to_string()))?;
    for k in [&b"Annots"[..], b"Thumb", b"PieceInfo", b"Metadata", b"StructParents", b"B", b"AA", b"CropBox", b"BleedBox", b"TrimBox", b"ArtBox", b"VP", b"Group", b"SeparationInfo", b"PresSteps", b"Trans", b"Tabs", b"AF", b"UserUnit"] {
        d.remove(k);
    }
    d.set("MediaBox", vec![0.into(), 0.into(), Object::Real(vw), Object::Real(vh)]);
    d.set("Rotate", 0);
    d.set("Contents", cid);
    d.set("Resources", dictionary! { "XObject" => dictionary! { "Im0" => image } });
    Ok(())
}

fn count_annots(doc: &Document, pid: ObjectId) -> (usize, usize) {
    let mut links = 0;
    let mut other = 0;
    if let Some(Object::Array(a)) = doc.get_dictionary(pid).ok().and_then(|d| d.get(b"Annots").ok()).map(|a| resolve(doc, a)) {
        for o in a {
            if let Ok(d) = resolve(doc, o).as_dict() {
                if name_is(d, b"Subtype", b"Link") {
                    links += 1;
                } else if !name_is(d, b"Subtype", b"Widget") && !name_is(d, b"Subtype", b"Popup") {
                    other += 1;
                }
            }
        }
    }
    (other, links)
}

fn count_fields(doc: &Document) -> usize {
    fn walk(doc: &Document, o: &Object, depth: usize) -> usize {
        if depth > 32 {
            return 0;
        }
        match resolve(doc, o) {
            Object::Array(a) => a.iter().map(|k| walk(doc, k, depth + 1)).sum(),
            Object::Dictionary(d) => match d.get(b"Kids").ok() {
                Some(k) if d.has(b"T") && resolve(doc, k).as_array().is_ok_and(|a| a.iter().all(|x| resolve(doc, x).as_dict().is_ok_and(|kd| !kd.has(b"T")))) => 1,
                Some(k) => walk(doc, k, depth + 1),
                None => 1,
            },
            _ => 0,
        }
    }
    doc.catalog()
        .ok()
        .and_then(|c| c.get(b"AcroForm").ok())
        .map(|o| resolve(doc, o))
        .and_then(|o| o.as_dict().ok())
        .and_then(|af| af.get(b"Fields").ok())
        .map(|f| walk(doc, f, 0))
        .unwrap_or(0)
}

/// 가리기. `rec`이 없거나 `opts.ocr`가 꺼져 있으면 가린 쪽은 그림만(검색 안 됨).
pub fn apply(input: &Path, password: Option<&str>, marks: &[Mark], opts: &RedactOpts, output: &Path, rec: Option<&dyn Recognizer>, job: &mut Job) -> Result<RedactOut> {
    let mut doc = open(input, password)?;
    let ids = page_ids(&doc);
    let n = ids.len() as u32;
    let mut by_page: BTreeMap<usize, Vec<Mark>> = BTreeMap::new();
    for m in marks {
        if m.page == 0 || m.page > n {
            return Err(Error::PageSpec(format!("{}쪽은 범위를 벗어났습니다 (1~{n}쪽)", m.page)));
        }
        if !(m.w > 0.0 && m.h > 0.0 && m.x.is_finite() && m.y.is_finite()) {
            continue;
        }
        by_page.entry(m.page as usize - 1).or_default().push(*m);
    }
    if by_page.is_empty() {
        return Err(Error::Invalid("가릴 곳을 먼저 정해 주세요".into()));
    }
    let dpi = if opts.dpi >= 250 { 300 } else { 200 };
    let mut removed = Removed { form_fields: count_fields(&doc), ..Default::default() };
    for &i in by_page.keys() {
        let (a, l) = count_annots(&doc, ids[i]);
        removed.annotations += a;
        removed.links += l;
    }
    // 1) 양식 칸 굳히기(값이 쪽 내용으로만 남게)
    super::forms::flatten_doc_with(&mut doc, &mut Job::none(), true)?;
    job.check()?;
    // 2) 문서 전체 정리
    let mut raster: BTreeSet<usize> = BTreeSet::new();
    removed.javascript = strip_javascript(&mut doc);
    removed.attachments = strip_attachments(&mut doc);
    removed.metadata = strip_metadata(&mut doc);
    removed.hidden_layers = strip_hidden_ocg(&mut doc, &mut raster)?;
    removed.structure = strip_structure(&mut doc);
    for i in raster {
        by_page.entry(i).or_default();
    }
    // 3) 가릴 쪽 그리기(정리한 문서로 — 숨긴 레이어·양식이 반영된 모양)
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(io_err(output))?;
    let p = crate::pdfrender::pdfium()?;
    let pdf = p.load_pdf_from_byte_vec(bytes, None).map_err(|e| crate::pdfrender::render_err(input, e))?;
    let total = by_page.len() as u32;
    let mut rendered: Vec<(usize, RgbImage, f32, f32)> = Vec::new();
    for (k, (&i, ms)) in by_page.iter().enumerate() {
        job.check()?;
        let page = pdf.pages().get(i as PdfPageIndex).map_err(|e| crate::pdfrender::render_err(input, e))?;
        let g = page_geom(&doc, ids[i]);
        let (vw, vh) = g.visual_size();
        let mut img = render(&page, dpi, input)?.to_rgb8();
        let (sx, sy) = (img.width() as f32 / vw, img.height() as f32 / vh);
        burn(&mut img, ms, sx, sy);
        rendered.push((i, img, vw, vh));
        job.tick(k as u32 + 1, total * 2)?;
    }
    drop(pdf);
    // 4) 쪽 바꾸기 + 글자 층
    let mut font = None;
    let mut ocr_pages = 0u32;
    let mut hint = None;
    let use_ocr = opts.ocr && rec.is_some();
    if opts.ocr && rec.is_none() {
        hint = Some("이 PC에서는 글자 인식을 쓸 수 없어 가린 쪽은 그림으로만 남았어요(그 쪽은 검색되지 않아요)".to_string());
    }
    for (k, (i, img, vw, vh)) in rendered.into_iter().enumerate() {
        job.check()?;
        let pid = ids[i];
        let im = jpeg_xobject(&mut doc, &img)?;
        replace_page(&mut doc, pid, im, vw, vh)?;
        if let (true, Some(r)) = (use_ocr, rec) {
            let dynimg = DynamicImage::ImageRgb8(img);
            let recd = recognize(r, &dynimg, job.cancel)?;
            let ms = by_page.get(&i).cloned().unwrap_or_default();
            let skip: Vec<[f32; 4]> = ms.iter().map(|m| [m.x, m.y, m.x + m.w, m.y + m.h]).collect();
            let words = placed_words(&recd, vw, vh, &skip, true);
            let g = PageGeom { bx: [0.0, 0.0, vw, vh], rotate: 0 };
            if add_layer(&mut doc, pid, &mut font, &g, &words)? > 0 {
                ocr_pages += 1;
            }
            if hint.is_none() {
                hint = crate::ocr::language_hint(&r.language());
            }
        }
        job.tick(total + k as u32 + 1, total * 2)?;
    }
    job.check()?;
    super::save(doc, output)?;
    // 5) 확인
    let mut v = verify(output, None, &opts.check_terms)?;
    v.chars_in_marks = chars_in_marks(output, marks)?;
    v.clean = v.clean && v.chars_in_marks == 0;
    let redacted_pages: Vec<u32> = by_page.keys().map(|i| *i as u32 + 1).collect();
    let mut note = format!("{}쪽에서 {}곳을 가렸어요", redacted_pages.len(), marks.len());
    if v.clean {
        note.push_str(" · 가린 글자가 파일에 남지 않은 것을 확인했어요");
    } else {
        note.push_str(" · 확인: 가린 글이 다른 곳에 남아 있어요(가리지 않은 쪽에 같은 글이 있는지 보세요)");
    }
    Ok(RedactOut { output: output.to_path_buf(), pages: n, redacted_pages, marks: marks.len(), ocr_pages, removed, verify: v, note, hint })
}

/// 결과 이름 `이름_가림.pdf`
pub fn default_output(input: &Path, out_dir: Option<&Path>) -> PathBuf {
    super::default_output(input, out_dir, "_가림")
}

/// `EditOut` 모양(다른 PDF 도구와 같은 결과 칸용)
pub fn as_edit_out(o: &RedactOut) -> EditOut {
    EditOut { output: o.output.clone(), pages: o.pages, note: o.note.clone() }
}

// ---------- 확인 ----------

/// 가린 곳(보이는 쪽 pt) 안에 남은 글자 수(PDFium)
pub fn chars_in_marks(path: &Path, marks: &[Mark]) -> Result<usize> {
    if marks.is_empty() {
        return Ok(0);
    }
    let p = crate::pdfrender::pdfium()?;
    let pdf = p.load_pdf_from_file(path, None).map_err(|e| crate::pdfrender::render_err(path, e))?;
    let doc = Document::load(path).map_err(super::pdf_err(path))?;
    let ids = page_ids(&doc);
    let mut n = 0;
    for m in marks {
        let Some(&pid) = ids.get(m.page as usize - 1) else { continue };
        let g = page_geom(&doc, pid);
        let Ok(page) = pdf.pages().get((m.page - 1) as PdfPageIndex) else { continue };
        let Ok(t) = page.text() else { continue };
        for c in t.chars().iter() {
            if !c.unicode_char().is_some_and(|c| !c.is_whitespace()) {
                continue;
            }
            let Ok(b) = c.loose_bounds() else { continue };
            let r = user_to_visual(&g, [b.left().value, b.bottom().value, b.right().value, b.top().value]);
            let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
            if cx >= m.x && cx <= m.x + m.w && cy >= m.y && cy <= m.y + m.h {
                n += 1;
            }
        }
    }
    Ok(n)
}

fn utf16be(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
}

fn hex(b: &[u8], upper: bool) -> Vec<u8> {
    b.iter().flat_map(|x| if upper { format!("{x:02X}") } else { format!("{x:02x}") }.into_bytes()).collect()
}

/// 찾을 바이트 모양: UTF-8, UTF-16BE, 그 16진 글자(대·소문자)
fn needle_forms(s: &str) -> Vec<Vec<u8>> {
    let u8b = s.as_bytes().to_vec();
    let u16b = utf16be(s);
    let mut v = vec![u8b.clone(), u16b.clone(), hex(&u8b, true), hex(&u8b, false), hex(&u16b, true), hex(&u16b, false)];
    // 1바이트 글자(Latin-1)로 적힌 문자열
    if s.chars().all(|c| (c as u32) < 256) {
        v.push(s.chars().map(|c| c as u8).collect());
    }
    v.sort();
    v.dedup();
    v.retain(|n| !n.is_empty());
    v
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(hay, needle).is_some()
}

fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).flat_map(|c| c.to_lowercase()).collect()
}

fn string_text(b: &[u8]) -> String {
    if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
        let u: Vec<u16> = b[2..].chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u)
    } else {
        match std::str::from_utf8(b) {
            Ok(s) => s.to_string(),
            Err(_) => b.iter().map(|&x| x as char).collect(),
        }
    }
}

fn walk_strings(o: &Object, f: &mut dyn FnMut(&[u8])) {
    match o {
        Object::String(b, _) => f(b),
        Object::Name(n) => f(n),
        Object::Array(a) => a.iter().for_each(|x| walk_strings(x, f)),
        Object::Dictionary(d) => d.iter().for_each(|(k, v)| {
            f(k);
            walk_strings(v, f)
        }),
        Object::Stream(s) => s.dict.iter().for_each(|(k, v)| {
            f(k);
            walk_strings(v, f)
        }),
        _ => {}
    }
}

/// 결과 파일에 `terms`가 남아 있는지(글자 추출·스트림 바이트·문자열·문서 정보)
pub fn verify(path: &Path, password: Option<&str>, terms: &[String]) -> Result<VerifyOut> {
    let terms: Vec<String> = terms.iter().map(|t| t.trim().to_string()).filter(|t| t.chars().filter(|c| !c.is_whitespace()).count() >= 2).collect();
    let mut v = VerifyOut { terms: terms.len(), ..Default::default() };
    // lopdf: 스트림·문자열
    let doc = open(path, password)?;
    v.has_info = doc.trailer.get(b"Info").is_ok();
    v.has_xmp = doc.objects.values().any(|o| matches!(o, Object::Stream(s) if name_is(&s.dict, b"Type", b"Metadata")));
    let forms: Vec<(String, Vec<Vec<u8>>)> = terms.iter().map(|t| (squash(t), needle_forms(t))).collect();
    for o in doc.objects.values() {
        if let Object::Stream(s) = o {
            v.streams += 1;
            let raw = s.decompressed_content().unwrap_or_else(|_| s.content.clone());
            for (_, fs) in &forms {
                if fs.iter().any(|n| contains(&raw, n) || contains(&s.content, n)) {
                    v.stream_hits += 1;
                }
            }
        }
        walk_strings(o, &mut |b| {
            let t = squash(&string_text(b));
            for (sq, fs) in &forms {
                if t.contains(sq.as_str()) || fs.iter().any(|n| contains(b, n)) {
                    v.string_hits += 1;
                }
            }
        });
    }
    walk_strings(&Object::Dictionary(doc.trailer.clone()), &mut |b| {
        let t = squash(&string_text(b));
        for (sq, _) in &forms {
            if t.contains(sq.as_str()) {
                v.string_hits += 1;
            }
        }
    });
    // PDFium 글자 추출(글꼴 코드로 적힌 한글은 바이트로는 안 보이므로 이것이 기준)
    if !terms.is_empty() {
        let p = crate::pdfrender::pdfium()?;
        let pdf = crate::pdfedit::thumbs::pdfium_open(p, path, password)?;
        for (i, page) in pdf.pages().iter().enumerate() {
            let all = squash(&page.text().map(|t| t.all()).unwrap_or_default());
            for (t, (sq, _)) in terms.iter().zip(&forms) {
                if all.contains(sq.as_str()) {
                    v.text_hits.push((i as u32 + 1, t.clone()));
                }
            }
        }
    }
    v.clean = v.text_hits.is_empty() && v.stream_hits == 0 && v.string_hits == 0;
    Ok(v)
}

// ---------- 찾아서 가리기 ----------

/// 미리 만든 찾기 규칙
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// 주민등록번호(외국인등록번호 포함)
    Rrn,
    /// 휴대폰·전화번호
    Phone,
    Email,
    /// 계좌번호(숫자 묶음 3~5개, 10~16자리)
    Account,
}

impl Preset {
    pub fn label(self) -> &'static str {
        match self {
            Preset::Rrn => "주민등록번호",
            Preset::Phone => "전화번호",
            Preset::Email => "이메일",
            Preset::Account => "계좌번호",
        }
    }
}

/// 찾은 것 하나
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub id: usize,
    pub page: u32,
    pub text: String,
    /// 규칙 이름(`rrn` 등) 또는 `word`
    pub kind: String,
    /// 화면에 보일 이름(주민등록번호, 찾는 말 …)
    pub label: String,
    /// 가릴 상자들(줄이 바뀌면 여러 개)
    pub rects: Vec<Mark>,
    /// `text` | `ocr`
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FindOut {
    pub hits: Vec<Hit>,
    pub pages: u32,
    /// 글자 인식으로 찾은 쪽 수
    pub ocr_pages: u32,
    /// 글자가 없는데 글자 인식을 못 해 못 찾은 쪽
    pub unread_pages: Vec<u32>,
    pub note: String,
    pub hint: Option<String>,
}

const DASH: &str = r"[-‐‑‒–—−－]";

fn preset_regex(p: Preset) -> regex::Regex {
    let s = match p {
        Preset::Rrn => format!(r"\d{{6}}\s*{DASH}\s*[1-8](?:\d{{6}}|[*xX●]{{6}})|\d{{13}}"),
        Preset::Phone => format!(
            r"\+82{DASH}?\s?1[016789](?:{DASH}|[.\s])?\d{{3,4}}(?:{DASH}|[.\s])?\d{{4}}|\(0\d{{1,2}}\)\s?\d{{3,4}}(?:{DASH}|[.\s])?\d{{4}}|(?:01[016789]|0(?:2|[3-6][1-5]|70|50[2-8]?))(?:{DASH}|[.\s)])?\d{{3,4}}(?:{DASH}|[.\s])?\d{{4}}|1[5-9]\d{{2}}{DASH}\d{{4}}"
        ),
        Preset::Email => r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9\-]+(?:\.[A-Za-z0-9\-]+)*\.[A-Za-z]{2,}".to_string(),
        Preset::Account => format!(r"\d{{2,6}}(?:{DASH}\d{{2,8}}){{2,4}}"),
    };
    regex::Regex::new(&s).expect("규칙 정규식")
}

fn digits(s: &str) -> usize {
    s.chars().filter(|c| c.is_ascii_digit()).count()
}

/// 규칙별 추가 확인(앞뒤가 숫자가 아님, 날짜·자릿수)
fn accept(p: Option<Preset>, s: &str, before: Option<char>, after: Option<char>) -> bool {
    let Some(p) = p else { return true };
    if p != Preset::Email && (before.is_some_and(|c| c.is_ascii_digit()) || after.is_some_and(|c| c.is_ascii_digit())) {
        return false;
    }
    match p {
        Preset::Rrn => {
            let d: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
            if d.len() < 7 {
                return false;
            }
            let mm = d[2] * 10 + d[3];
            let dd = d[4] * 10 + d[5];
            (1..=12).contains(&mm) && (1..=31).contains(&dd) && (1..=8).contains(&d[6])
        }
        Preset::Phone => {
            let n = digits(s);
            (8..=13).contains(&n) && (s.chars().any(|c| !c.is_ascii_digit()) || n >= 10)
        }
        Preset::Email => true,
        Preset::Account => {
            let n = digits(s);
            // 날짜(2026-11-01)·전화번호 모양은 뺀다
            (10..=16).contains(&n) && !(preset_regex(Preset::Phone).find(s).is_some_and(|m| m.as_str() == s) && n <= 11)
        }
    }
}

fn term_regex(t: &str) -> Option<regex::Regex> {
    let chars: Vec<String> = t.chars().filter(|c| !c.is_whitespace()).map(|c| regex::escape(&c.to_string())).collect();
    if chars.is_empty() {
        return None;
    }
    regex::Regex::new(&format!("(?i){}", chars.join(r"\s*"))).ok()
}

/// 쪽 글자 + 글자마다 상자(보이는 쪽 pt, x0,y0,x1,y1)
struct PageText {
    s: String,
    /// `s`의 바이트 위치 → 글자 번호
    starts: Vec<usize>,
    boxes: Vec<Option<[f32; 4]>>,
}

impl PageText {
    fn new() -> Self {
        PageText { s: String::new(), starts: vec![], boxes: vec![] }
    }
    fn push(&mut self, c: char, b: Option<[f32; 4]>) {
        self.starts.push(self.s.len());
        self.s.push(c);
        self.boxes.push(b);
    }
    fn index_of(&self, byte: usize) -> usize {
        self.starts.partition_point(|&s| s < byte)
    }
}

/// 같은 줄 글자 상자를 합친다
fn merge_rects(rs: &[[f32; 4]]) -> Vec<[f32; 4]> {
    let mut out: Vec<[f32; 4]> = Vec::new();
    for r in rs {
        if let Some(last) = out.last_mut() {
            // 가로 글: 세로로 절반 넘게 겹치고 가로 틈이 글자 높이 3배 이내 / 세로 글(돌린 쪽): 반대로
            let h = (r[3] - r[1]).max(last[3] - last[1]).max(0.1);
            let w = (r[2] - r[0]).max(last[2] - last[0]).max(0.1);
            let vo = (r[3].min(last[3]) - r[1].max(last[1])) / h;
            let ho = (r[2].min(last[2]) - r[0].max(last[0])) / h;
            let xo = (r[2].min(last[2]) - r[0].max(last[0])) / w;
            let yo = (r[3].min(last[3]) - r[1].max(last[1])) / w;
            if (vo > 0.5 && ho > -3.0) || (xo > 0.5 && yo > -3.0) {
                *last = [last[0].min(r[0]), last[1].min(r[1]), last[2].max(r[2]), last[3].max(r[3])];
                continue;
            }
        }
        out.push(*r);
    }
    out
}

fn search_page(pt: &PageText, page: u32, rules: &[(Option<Preset>, String, regex::Regex)], source: &str, out: &mut Vec<Hit>) {
    let mut taken: Vec<(usize, usize)> = Vec::new();
    for (preset, term, re) in rules {
        for m in re.find_iter(&pt.s) {
            let (a, b) = (pt.index_of(m.start()), pt.index_of(m.end()));
            if taken.iter().any(|&(x, y)| a < y && x < b) {
                continue;
            }
            let before = pt.s[..m.start()].chars().last();
            let after = pt.s[m.end()..].chars().next();
            if !accept(*preset, m.as_str(), before, after) {
                continue;
            }
            let rs: Vec<[f32; 4]> = pt.boxes[a..b].iter().flatten().copied().collect();
            if rs.is_empty() {
                continue;
            }
            taken.push((a, b));
            let rects = merge_rects(&rs)
                .into_iter()
                .map(|r| Mark { page, x: r[0] - 1.0, y: r[1] - 1.0, w: r[2] - r[0] + 2.0, h: r[3] - r[1] + 2.0 })
                .collect();
            let (kind, label) = match preset {
                Some(p) => (serde_json::to_value(p).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default(), p.label().to_string()),
                None => ("word".to_string(), format!("찾는 말 '{term}'")),
            };
            out.push(Hit { id: 0, page, text: m.as_str().replace(['\r', '\n'], " "), kind, label, rects, source: source.into() });
        }
    }
}

/// 찾아서 가리기: 찾은 것 목록(아직 아무것도 바꾸지 않음)
pub fn find(input: &Path, password: Option<&str>, terms: &[String], presets: &[Preset], rec: Option<&dyn Recognizer>, job: &mut Job) -> Result<FindOut> {
    let mut rules: Vec<(Option<Preset>, String, regex::Regex)> = Vec::new();
    // 규칙 먼저(겹치면 앞의 것), 그다음 찾는 말
    for p in [Preset::Rrn, Preset::Phone, Preset::Email, Preset::Account] {
        if presets.contains(&p) {
            rules.push((Some(p), String::new(), preset_regex(p)));
        }
    }
    for t in terms {
        if let Some(re) = term_regex(t) {
            rules.push((None, t.trim().to_string(), re));
        }
    }
    if rules.is_empty() {
        return Err(Error::Invalid("찾을 말을 적거나 찾을 종류를 하나 이상 골라 주세요".into()));
    }
    let doc = open(input, password)?;
    let ids = page_ids(&doc);
    let p = crate::pdfrender::pdfium()?;
    let pdf = crate::pdfedit::thumbs::pdfium_open(p, input, password)?;
    let n = pdf.pages().len() as u32;
    let mut hits = Vec::new();
    let mut ocr_pages = 0;
    let mut unread = Vec::new();
    let mut hint = None;
    for i in 0..n {
        job.check()?;
        let page = pdf.pages().get(i as PdfPageIndex).map_err(|e| crate::pdfrender::render_err(input, e))?;
        let g = page_geom(&doc, ids[i as usize]);
        let mut pt = PageText::new();
        let have = text_chars(&page);
        if have > 0 {
            let t = page.text().map_err(|e| crate::pdfrender::render_err(input, e))?;
            for c in t.chars().iter() {
                let Some(ch) = c.unicode_char() else { continue };
                let ch = if ch == '\r' { '\n' } else { ch };
                let b = if ch.is_whitespace() { None } else { c.loose_bounds().ok().map(|b| user_to_visual(&g, [b.left().value, b.bottom().value, b.right().value, b.top().value])) };
                pt.push(ch, b);
            }
            search_page(&pt, i + 1, &rules, "text", &mut hits);
            pt = PageText::new();
        }
        if let Some(r) = rec.filter(|_| have < 10) {
            let (vw, vh) = g.visual_size();
            let img = render(&page, 300, input)?;
            let recd = recognize(r, &img, job.cancel)?;
            let (kx, ky) = (vw / recd.width.max(1) as f32, vh / recd.height.max(1) as f32);
            for l in &recd.lines {
                for (wi, w) in l.words.iter().enumerate() {
                    if wi > 0 {
                        pt.push(' ', None);
                    }
                    let cs: Vec<char> = w.text.chars().collect();
                    let k = cs.len().max(1) as f32;
                    for (ci, c) in cs.iter().enumerate() {
                        // 낱말 상자를 글자 수로 고르게 나눈다(글 방향 따라)
                        let (x, y, ww, hh) = match l.rot {
                            90 => (w.x, w.y + w.h * ci as f32 / k, w.w, w.h / k),
                            180 => (w.x + w.w * (1.0 - (ci + 1) as f32 / k), w.y, w.w / k, w.h),
                            270 => (w.x, w.y + w.h * (1.0 - (ci + 1) as f32 / k), w.w, w.h / k),
                            _ => (w.x + w.w * ci as f32 / k, w.y, w.w / k, w.h),
                        };
                        pt.push(*c, Some([x * kx, y * ky, (x + ww) * kx, (y + hh) * ky]));
                    }
                }
                pt.push('\n', None);
            }
            ocr_pages += 1;
            if hint.is_none() {
                hint = crate::ocr::language_hint(&r.language());
            }
            search_page(&pt, i + 1, &rules, "ocr", &mut hits);
        } else if have == 0 {
            unread.push(i + 1);
        }
        job.tick(i + 1, n)?;
    }
    for (k, h) in hits.iter_mut().enumerate() {
        h.id = k + 1;
    }
    let mut note = if hits.is_empty() { "찾은 것이 없어요".to_string() } else { format!("{}곳을 찾았어요", hits.len()) };
    if ocr_pages > 0 {
        note.push_str(&format!(" · 스캔 {ocr_pages}쪽은 글자 인식으로 찾았어요(틀리게 읽은 글자는 못 찾을 수 있어요)"));
    }
    if !unread.is_empty() {
        note.push_str(&format!(" · 글자가 없는 {}쪽은 찾지 못했어요(직접 상자로 가려 주세요)", unread.len()));
    }
    Ok(FindOut { hits, pages: n, ocr_pages, unread_pages: unread, note, hint })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(p: Preset, s: &str) -> Vec<String> {
        let re = preset_regex(p);
        re.find_iter(s)
            .filter(|m| accept(Some(p), m.as_str(), s[..m.start()].chars().last(), s[m.end()..].chars().next()))
            .map(|m| m.as_str().to_string())
            .collect()
    }

    #[test]
    fn presets_match_korean_formats() {
        assert_eq!(hits(Preset::Rrn, "주민번호 900101-1234567 입니다"), vec!["900101-1234567"]);
        assert_eq!(hits(Preset::Rrn, "900101 - 2******"), vec!["900101 - 2******"]);
        assert!(hits(Preset::Rrn, "주문번호 991399-1234567").is_empty(), "13월은 날짜가 아님");
        assert_eq!(hits(Preset::Phone, "연락처 010-1234-5678, 02-123-4567, (031) 123-4567, 1588-1234"), vec!["010-1234-5678", "02-123-4567", "(031) 123-4567", "1588-1234"]);
        assert_eq!(hits(Preset::Phone, "+82-10-1234-5678"), vec!["+82-10-1234-5678"]);
        assert_eq!(hits(Preset::Email, "메일: hong.gil-dong@example.co.kr 로"), vec!["hong.gil-dong@example.co.kr"]);
        assert_eq!(hits(Preset::Account, "국민 123456-78-901234, 신한 110-123-456789"), vec!["123456-78-901234", "110-123-456789"]);
        assert!(hits(Preset::Account, "날짜 2026-11-01, 전화 010-1234-5678").is_empty());
    }

    #[test]
    fn term_ignores_spaces_and_case() {
        let re = term_regex("홍길동").unwrap();
        assert!(re.is_match("홍 길동"));
        let re = term_regex("Secret").unwrap();
        assert!(re.is_match("SECRET"));
    }

    #[test]
    fn needle_forms_cover_hex_and_utf16() {
        let f = needle_forms("AB");
        assert!(f.contains(&b"AB".to_vec()));
        assert!(f.contains(&vec![0, b'A', 0, b'B']));
        assert!(f.contains(&b"4142".to_vec()));
        assert!(f.contains(&b"00410042".to_vec()));
    }
}
