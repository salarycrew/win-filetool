//! 양식(AcroForm): 칸 목록, 값 채우기, 평평하게(양식 칸·주석을 쪽 내용으로 바꿔 더는 고칠 수 없게).
//! 글자 칸은 채울 때 모양(Appearance)을 직접 만든다(한글 글꼴 부분 넣기) — 보는 프로그램이 다시 그리지 않아도
//! 한글이 보이고, 평평하게 할 때 그 모양이 그대로 쪽에 남는다.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
use serde::{Deserialize, Serialize};

use super::font::PdfFont;
use super::{add_content, add_resource, cm, f, mat_bbox, open, page_ids, rect, resolve, EditOut, Job, IDENTITY};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FieldInfo {
    /// 전체 이름(`부모.자식`)
    pub name: String,
    /// text | checkbox | radio | combo | list | button | signature
    pub kind: String,
    pub value: String,
    /// 체크 상자·라디오: 켜짐 상태 이름들, 선택 칸: 고를 수 있는 값
    pub options: Vec<String>,
    pub read_only: bool,
    /// 처음 보이는 쪽(1부터)
    pub page: Option<u32>,
}

struct Field {
    id: ObjectId,
    name: String,
    ft: Vec<u8>,
    ff: i64,
    widgets: Vec<ObjectId>,
}

const FF_READONLY: i64 = 1;
const FF_MULTILINE: i64 = 1 << 12;
const FF_RADIO: i64 = 1 << 15;
const FF_PUSH: i64 = 1 << 16;
const FF_COMBO: i64 = 1 << 17;
const FF_EDIT: i64 = 1 << 18;

fn acroform(doc: &Document) -> Option<Dictionary> {
    let cat = doc.catalog().ok()?;
    let af = cat.get(b"AcroForm").ok()?;
    resolve(doc, af).as_dict().ok().cloned()
}

fn collect(doc: &Document) -> Vec<Field> {
    let mut out = Vec::new();
    let Some(af) = acroform(doc) else { return out };
    let Ok(fields) = af.get(b"Fields").map(|o| resolve(doc, o)).and_then(Object::as_array) else { return out };
    let mut seen = std::collections::HashSet::new();
    for f in fields {
        if let Object::Reference(id) = f {
            walk(doc, *id, "", Vec::new(), 0, &mut out, &mut seen, 0);
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn walk(doc: &Document, id: ObjectId, parent: &str, ft: Vec<u8>, ff: i64, out: &mut Vec<Field>, seen: &mut std::collections::HashSet<ObjectId>, depth: u32) {
    if depth > 32 || !seen.insert(id) {
        return;
    }
    let Ok(d) = doc.get_dictionary(id) else { return };
    let part = d.get(b"T").ok().and_then(|o| lopdf::decode_text_string(resolve(doc, o)).ok());
    let name = match (&part, parent.is_empty()) {
        (Some(p), true) => p.clone(),
        (Some(p), false) => format!("{parent}.{p}"),
        (None, _) => parent.to_string(),
    };
    let ft = d.get(b"FT").ok().and_then(|o| resolve(doc, o).as_name().ok()).map(|n| n.to_vec()).unwrap_or(ft);
    let ff = d.get(b"Ff").ok().and_then(|o| resolve(doc, o).as_i64().ok()).unwrap_or(ff);
    let kids: Vec<ObjectId> = d
        .get(b"Kids")
        .ok()
        .and_then(|o| resolve(doc, o).as_array().ok())
        .map(|a| a.iter().filter_map(|k| k.as_reference().ok()).collect())
        .unwrap_or_default();
    let mut widgets = Vec::new();
    for k in &kids {
        let has_t = doc.get_dictionary(*k).map(|kd| kd.has(b"T")).unwrap_or(false);
        if has_t {
            walk(doc, *k, &name, ft.clone(), ff, out, seen, depth + 1);
        } else {
            widgets.push(*k);
        }
    }
    if kids.is_empty() {
        widgets.push(id);
    }
    if !widgets.is_empty() {
        out.push(Field { id, name, ft, ff, widgets });
    }
}

fn kind(ft: &[u8], ff: i64) -> &'static str {
    match ft {
        b"Tx" => "text",
        b"Btn" if ff & FF_PUSH != 0 => "button",
        b"Btn" if ff & FF_RADIO != 0 => "radio",
        b"Btn" => "checkbox",
        b"Ch" if ff & FF_COMBO != 0 => "combo",
        b"Ch" => "list",
        b"Sig" => "signature",
        _ => "unknown",
    }
}

fn value_of(doc: &Document, field: ObjectId) -> String {
    let Some(v) = super::inherited(doc, field, b"V") else { return String::new() };
    obj_text(doc, &v)
}

fn obj_text(doc: &Document, v: &Object) -> String {
    match resolve(doc, v) {
        Object::Name(n) => String::from_utf8_lossy(n).into_owned(),
        Object::String(..) => lopdf::decode_text_string(resolve(doc, v)).unwrap_or_default(),
        Object::Array(a) => a.iter().map(|o| obj_text(doc, o)).collect::<Vec<_>>().join(", "),
        _ => String::new(),
    }
}

/// 위젯의 AP /N 사전에 있는 켜짐 상태 이름(Off 제외)
fn on_states(doc: &Document, widget: ObjectId) -> Vec<String> {
    let Ok(w) = doc.get_dictionary(widget) else { return Vec::new() };
    let Some(n) = w.get(b"AP").ok().and_then(|ap| resolve(doc, ap).as_dict().ok()).and_then(|ap| ap.get(b"N").ok()) else { return Vec::new() };
    match resolve(doc, n).as_dict() {
        Ok(d) => d.iter().map(|(k, _)| String::from_utf8_lossy(k).into_owned()).filter(|k| k != "Off").collect(),
        Err(_) => Vec::new(),
    }
}

fn choice_options(doc: &Document, field: ObjectId) -> Vec<String> {
    let Some(opt) = super::inherited(doc, field, b"Opt") else { return Vec::new() };
    let Ok(arr) = resolve(doc, &opt).as_array() else { return Vec::new() };
    arr.iter()
        .map(|o| match resolve(doc, o) {
            Object::Array(pair) if !pair.is_empty() => obj_text(doc, &pair[0]),
            other => obj_text(doc, other),
        })
        .collect()
}

fn annot_pages(doc: &Document) -> HashMap<ObjectId, u32> {
    let mut m = HashMap::new();
    for (i, pid) in page_ids(doc).into_iter().enumerate() {
        if let Some(arr) = doc.get_dictionary(pid).ok().and_then(|d| d.get(b"Annots").ok()).and_then(|a| resolve(doc, a).as_array().ok()) {
            for a in arr {
                if let Ok(id) = a.as_reference() {
                    m.entry(id).or_insert(i as u32 + 1);
                }
            }
        }
    }
    m
}

fn infos(doc: &Document) -> Vec<FieldInfo> {
    let pages = annot_pages(doc);
    collect(doc)
        .into_iter()
        .map(|fl| {
            let k = kind(&fl.ft, fl.ff);
            let options = match k {
                "checkbox" | "radio" => {
                    let mut v: Vec<String> = fl.widgets.iter().flat_map(|w| on_states(doc, *w)).collect();
                    v.dedup();
                    v
                }
                "combo" | "list" => choice_options(doc, fl.id),
                _ => Vec::new(),
            };
            let mut value = value_of(doc, fl.id);
            if (k == "checkbox" || k == "radio") && value.is_empty() {
                value = "Off".into();
            }
            FieldInfo {
                page: fl.widgets.iter().find_map(|w| pages.get(w).copied()),
                name: fl.name,
                kind: k.into(),
                value,
                options,
                read_only: fl.ff & FF_READONLY != 0,
            }
        })
        .collect()
}

pub fn list(input: &Path, password: Option<&str>) -> Result<Vec<FieldInfo>> {
    let doc = open(input, password)?;
    Ok(infos(&doc))
}

/// DA(`/Helv 12 Tf 0 g`)에서 글자 크기와 색
fn parse_da(da: &str) -> (f32, [f32; 3]) {
    let toks: Vec<&str> = da.split_whitespace().collect();
    let mut size = 0.0;
    let mut color = [0.0, 0.0, 0.0];
    for (i, t) in toks.iter().enumerate() {
        match *t {
            "Tf" if i >= 1 => size = toks[i - 1].parse().unwrap_or(0.0),
            "g" if i >= 1 => {
                let g = toks[i - 1].parse().unwrap_or(0.0);
                color = [g, g, g];
            }
            "rg" if i >= 3 => {
                color = [toks[i - 3].parse().unwrap_or(0.0), toks[i - 2].parse().unwrap_or(0.0), toks[i - 1].parse().unwrap_or(0.0)];
            }
            _ => {}
        }
    }
    (size, color)
}

/// 화면(JSON)에서 오는 칸 값. 글자 칸·선택 칸은 문자열, 체크 상자는 `true`/`false`로 와도 받는다(0.3.0 QA-01).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum FormValue {
    Text(String),
    Flag(bool),
    Number(f64),
}

impl FormValue {
    pub fn into_text(self) -> String {
        match self {
            FormValue::Text(s) => s,
            FormValue::Flag(b) => if b { "true" } else { "false" }.into(),
            FormValue::Number(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { n.to_string() }
            }
        }
    }
}

/// 화면에서 받은 값 묶음을 [`fill`]이 쓰는 문자열 묶음으로.
pub fn text_values(values: BTreeMap<String, FormValue>) -> BTreeMap<String, String> {
    values.into_iter().map(|(k, v)| (k, v.into_text())).collect()
}

fn truthy(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on" | "y" | "v" | "예" | "켜기" | "체크")
}

/// 글자/선택 칸의 모양 스트림을 만들어 위젯 AP에 넣는다.
fn text_appearance(doc: &mut Document, font: &mut PdfFont, field: &Field, widget: ObjectId, value: &str, da_default: &str, q_default: i64) -> Result<()> {
    let w = doc.get_dictionary(widget).map_err(|e| Error::Invalid(e.to_string()))?.clone();
    let r = w.get(b"Rect").ok().and_then(|o| rect(doc, o)).unwrap_or([0.0, 0.0, 100.0, 20.0]);
    let (bw, bh) = (r[2] - r[0], r[3] - r[1]);
    let da = super::inherited(doc, widget, b"DA")
        .or_else(|| super::inherited(doc, field.id, b"DA"))
        .and_then(|o| lopdf::decode_text_string(resolve(doc, &o)).ok())
        .unwrap_or_else(|| da_default.to_string());
    let (mut size, color) = parse_da(&da);
    let multiline = field.ff & FF_MULTILINE != 0;
    if size <= 0.0 {
        size = if multiline { 10.0 } else { (bh * 0.65).clamp(4.0, 14.0) };
    }
    let q = super::inherited(doc, field.id, b"Q").and_then(|o| resolve(doc, &o).as_i64().ok()).unwrap_or(q_default);
    let pad = 2.0;
    let lines: Vec<String> = if multiline { font.wrap(value, size, bw - 2.0 * pad) } else { vec![value.replace(['\r', '\n'], " ")] };
    // 위젯 MK의 바탕색(BG)·테두리색(BC)을 그대로 그린다
    let mk = w.get(b"MK").ok().and_then(|o| resolve(doc, o).as_dict().ok()).cloned();
    let mk_color = |key: &[u8]| -> Option<String> {
        let arr = mk.as_ref()?.get(key).ok().and_then(|o| resolve(doc, o).as_array().ok())?.clone();
        let v: Vec<f32> = arr.iter().filter_map(|o| super::num(doc, o)).collect();
        match v.len() {
            1 => Some(format!("{} g", f(v[0]))),
            3 => Some(format!("{} {} {} rg", f(v[0]), f(v[1]), f(v[2]))),
            4 => Some(format!("{} {} {} {} k", f(v[0]), f(v[1]), f(v[2]), f(v[3]))),
            _ => None,
        }
    };
    let mut ops = String::new();
    if let Some(bg) = mk_color(b"BG") {
        ops.push_str(&format!("q {bg} 0 0 {} {} re f Q\n", f(bw), f(bh)));
    }
    if let Some(bc) = mk_color(b"BC") {
        let stroke = bc.replace(" rg", " RG").replace(" g", " G").replace(" k", " K");
        ops.push_str(&format!("q {stroke} 1 w 0.5 0.5 {} {} re S Q\n", f(bw - 1.0), f(bh - 1.0)));
    }
    ops.push_str(&format!("/Tx BMC\nq\n{} {} {} {} re W n\n", f(1.0), f(1.0), f(bw - 2.0), f(bh - 2.0)));
    ops.push_str(&super::rg(color));
    let asc = font.data().ascent() * size;
    let desc = font.data().descent() * size;
    for (i, l) in lines.iter().enumerate() {
        let tw = font.width(l, size);
        let x = match q {
            1 => (bw - tw) / 2.0,
            2 => bw - pad - tw,
            _ => pad,
        };
        let y = if multiline { bh - pad - asc - i as f32 * size * 1.15 } else { (bh - (asc - desc)) / 2.0 - desc };
        ops.push_str(&font.show("KFnt", size, x, y, l));
    }
    ops.push_str("Q\nEMC\n");
    let ap = Stream::new(
        dictionary! {
            "Type" => "XObject", "Subtype" => "Form", "BBox" => super::rect_obj([0.0, 0.0, bw, bh]),
            "Resources" => dictionary! { "Font" => dictionary! { "KFnt" => font.id } },
        },
        ops.into_bytes(),
    );
    let ap_id = doc.add_object(ap);
    let wd = doc.get_dictionary_mut(widget).map_err(|e| Error::Invalid(e.to_string()))?;
    wd.set("AP", dictionary! { "N" => ap_id });
    Ok(())
}

/// 값 채우기. `values`: 칸 이름 → 값(체크 상자는 `true`/`false` 또는 상태 이름, 라디오는 고를 상태 이름).
/// `flatten`이면 채운 뒤 평평하게 만든다.
pub fn fill(input: &Path, password: Option<&str>, values: &BTreeMap<String, String>, flatten_after: bool, output: &Path, job: &mut Job) -> Result<EditOut> {
    let mut doc = open(input, password)?;
    let fields = collect(&doc);
    if fields.is_empty() {
        return Err(Error::Invalid("이 PDF에는 채울 양식 칸이 없습니다".into()));
    }
    let af = acroform(&doc).unwrap_or_default();
    let da_default = af.get(b"DA").ok().and_then(|o| lopdf::decode_text_string(resolve(&doc, o)).ok()).unwrap_or_else(|| "/Helv 0 Tf 0 g".into());
    let q_default = af.get(b"Q").ok().and_then(|o| o.as_i64().ok()).unwrap_or(0);
    let unknown: Vec<&String> = values.keys().filter(|k| !fields.iter().any(|fl| &fl.name == *k)).collect();
    if !unknown.is_empty() {
        return Err(Error::Invalid(format!(
            "없는 칸 이름입니다: {}",
            unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        )));
    }
    let needs_font = fields.iter().any(|fl| values.contains_key(&fl.name) && matches!(fl.ft.as_slice(), b"Tx" | b"Ch"));
    let mut font = if needs_font { Some(PdfFont::new(&mut doc)?) } else { None };
    let total = values.len() as u32;
    let mut done = 0;
    for fl in &fields {
        let Some(v) = values.get(&fl.name) else { continue };
        job.tick(done, total)?;
        done += 1;
        if fl.ff & FF_READONLY != 0 {
            return Err(Error::Invalid(format!("'{}' 칸은 읽기 전용이라 채울 수 없습니다", fl.name)));
        }
        match kind(&fl.ft, fl.ff) {
            "text" => {
                set_field(&mut doc, fl.id, "V", lopdf::text_string(v))?;
                for &w in &fl.widgets {
                    text_appearance(&mut doc, font.as_mut().unwrap(), fl, w, v, &da_default, q_default)?;
                }
            }
            "combo" | "list" => {
                let opts = choice_options(&doc, fl.id);
                let editable = fl.ff & FF_EDIT != 0;
                if !v.is_empty() && !opts.iter().any(|o| o == v) && !editable {
                    return Err(Error::Invalid(format!("'{}' 칸에서 고를 수 있는 값: {}", fl.name, opts.join(", "))));
                }
                set_field(&mut doc, fl.id, "V", lopdf::text_string(v))?;
                if let Ok(d) = doc.get_dictionary_mut(fl.id) {
                    d.remove(b"I");
                }
                for &w in &fl.widgets {
                    text_appearance(&mut doc, font.as_mut().unwrap(), fl, w, v, &da_default, q_default)?;
                }
            }
            "checkbox" => {
                let states: Vec<String> = fl.widgets.iter().flat_map(|w| on_states(&doc, *w)).collect();
                let on = if states.iter().any(|s| s == v) {
                    Some(v.clone())
                } else if truthy(v) {
                    Some(states.first().cloned().unwrap_or_else(|| "Yes".into()))
                } else {
                    None
                };
                let name = on.clone().unwrap_or_else(|| "Off".into());
                set_field(&mut doc, fl.id, "V", Object::Name(name.clone().into_bytes()))?;
                for &w in &fl.widgets {
                    let has = on_states(&doc, w).contains(&name);
                    let st = if has { name.clone() } else { "Off".into() };
                    doc.get_dictionary_mut(w).map_err(|e| Error::Invalid(e.to_string()))?.set("AS", Object::Name(st.into_bytes()));
                }
            }
            "radio" => {
                let states: Vec<String> = fl.widgets.iter().flat_map(|w| on_states(&doc, *w)).collect();
                let name = if v.is_empty() || v == "Off" {
                    "Off".to_string()
                } else if states.iter().any(|s| s == v) {
                    v.clone()
                } else {
                    return Err(Error::Invalid(format!("'{}' 칸에서 고를 수 있는 값: {}", fl.name, states.join(", "))));
                };
                set_field(&mut doc, fl.id, "V", Object::Name(name.clone().into_bytes()))?;
                for &w in &fl.widgets {
                    let st = if on_states(&doc, w).contains(&name) { name.clone() } else { "Off".into() };
                    doc.get_dictionary_mut(w).map_err(|e| Error::Invalid(e.to_string()))?.set("AS", Object::Name(st.into_bytes()));
                }
            }
            other => return Err(Error::Invalid(format!("'{}' 칸({other})은 채울 수 없습니다", fl.name))),
        }
    }
    if let Some(fnt) = font {
        fnt.finish(&mut doc)?;
    }
    // 모양을 직접 만들었으니 보는 프로그램이 (한글 없는 글꼴로) 다시 그리지 않게
    if let Some(af_ref) = doc.catalog().ok().and_then(|c| c.get(b"AcroForm").ok()).cloned() {
        match af_ref {
            Object::Reference(id) => {
                if let Ok(d) = doc.get_dictionary_mut(id) {
                    d.set("NeedAppearances", false);
                }
            }
            Object::Dictionary(mut d) => {
                d.set("NeedAppearances", false);
                doc.catalog_mut().map_err(super::pdf_err(input))?.set("AcroForm", d);
            }
            _ => {}
        }
    }
    let n = doc.get_pages().len() as u32;
    let flat = if flatten_after { flatten_doc(&mut doc, job)? } else { 0 };
    super::save(doc, output)?;
    job.tick(total, total)?;
    let note = if flatten_after {
        format!("양식 칸 {total}개를 채우고 {flat}개를 평평하게 만들었습니다")
    } else {
        format!("양식 칸 {total}개를 채웠습니다")
    };
    Ok(EditOut { output: output.to_path_buf(), pages: n, note })
}

fn set_field(doc: &mut Document, id: ObjectId, key: &str, v: Object) -> Result<()> {
    doc.get_dictionary_mut(id).map_err(|e| Error::Invalid(e.to_string()))?.set(key, v);
    Ok(())
}

/// 양식 칸과 주석을 쪽 내용으로 그려 넣고 없앤다(링크는 남김). 그린 개수.
fn flatten_doc(doc: &mut Document, job: &mut Job) -> Result<u32> {
    flatten_doc_with(doc, job, false)
}

/// `widgets_only`면 양식 칸(Widget)만 굳히고 다른 주석은 그대로 둔다(0.4 가리기).
pub(crate) fn flatten_doc_with(doc: &mut Document, job: &mut Job, widgets_only: bool) -> Result<u32> {
    let dr = acroform(doc).and_then(|af| af.get(b"DR").ok().cloned());
    let ids = page_ids(doc);
    let n = ids.len() as u32;
    let mut drawn = 0;
    for (k, &pid) in ids.iter().enumerate() {
        job.check()?;
        let _ = k;
        let annots: Vec<Object> = match doc.get_dictionary(pid).ok().and_then(|d| d.get(b"Annots").ok()).map(|a| resolve(doc, a).clone()) {
            Some(Object::Array(a)) => a,
            _ => continue,
        };
        let mut keep = Vec::new();
        let mut ops = String::new();
        for a in annots {
            let (aid, ad) = match &a {
                Object::Reference(id) => match doc.get_dictionary(*id) {
                    Ok(d) => (Some(*id), d.clone()),
                    Err(_) => continue,
                },
                Object::Dictionary(d) => (None, d.clone()),
                _ => continue,
            };
            let subtype = ad.get(b"Subtype").ok().and_then(|o| o.as_name().ok()).map(|n| n.to_vec()).unwrap_or_default();
            if widgets_only && subtype != b"Widget" {
                keep.push(a.clone());
                continue;
            }
            let flags = ad.get(b"F").ok().and_then(|o| resolve(doc, o).as_i64().ok()).unwrap_or(0);
            if subtype == b"Popup" || flags & 2 != 0 || flags & 32 != 0 {
                continue;
            }
            let ap = ad.get(b"AP").ok().and_then(|o| resolve(doc, o).as_dict().ok()).and_then(|ap| ap.get(b"N").ok()).cloned();
            let stream_ref = match ap.as_ref() {
                Some(Object::Reference(r)) => match doc.get_object(*r) {
                    Ok(Object::Stream(_)) => Some(*r),
                    Ok(Object::Dictionary(states)) => {
                        let st = ad.get(b"AS").ok().and_then(|o| o.as_name().ok()).map(|n| n.to_vec());
                        st.and_then(|s| states.get(&s).ok()).and_then(|o| o.as_reference().ok())
                    }
                    _ => None,
                },
                Some(Object::Dictionary(states)) => {
                    let st = ad.get(b"AS").ok().and_then(|o| o.as_name().ok()).map(|n| n.to_vec());
                    st.and_then(|s| states.get(&s).ok()).and_then(|o| o.as_reference().ok())
                }
                Some(Object::Stream(s)) => Some(doc.add_object(s.clone())),
                _ => None,
            };
            let Some(sid) = stream_ref else {
                if subtype == b"Widget" {
                    continue; // 모양 없는 양식 칸은 그냥 뺀다
                }
                keep.push(a.clone());
                continue;
            };
            let r = ad.get(b"Rect").ok().and_then(|o| rect(doc, o));
            let Some(r) = r else { continue };
            // 모양 스트림을 Form XObject로 다듬는다
            let (bbox, matrix) = {
                let Ok(Object::Stream(s)) = doc.get_object_mut(sid) else { continue };
                s.dict.set("Type", "XObject");
                s.dict.set("Subtype", "Form");
                if !s.dict.has(b"Resources") {
                    if let Some(dr) = &dr {
                        s.dict.set("Resources", dr.clone());
                    }
                }
                let bbox = s.dict.get(b"BBox").ok().cloned();
                let m = s.dict.get(b"Matrix").ok().cloned();
                (bbox, m)
            };
            let bbox = bbox.and_then(|o| rect(doc, &o)).unwrap_or([0.0, 0.0, r[2] - r[0], r[3] - r[1]]);
            let matrix = matrix
                .and_then(|o| resolve(doc, &o).as_array().ok().cloned())
                .map(|a| a.iter().filter_map(|v| super::num(doc, v)).collect::<Vec<f32>>())
                .filter(|v| v.len() == 6)
                .map(|v| [v[0], v[1], v[2], v[3], v[4], v[5]])
                .unwrap_or(IDENTITY);
            let tb = mat_bbox(matrix, bbox);
            let (tw, th) = (tb[2] - tb[0], tb[3] - tb[1]);
            if tw <= 0.0 || th <= 0.0 {
                continue;
            }
            let sx = (r[2] - r[0]) / tw;
            let sy = (r[3] - r[1]) / th;
            let a_m = [sx, 0.0, 0.0, sy, r[0] - tb[0] * sx, r[1] - tb[1] * sy];
            let name = add_resource(doc, pid, b"XObject", "FlA", Object::Reference(sid))?;
            ops.push_str(&format!("q\n{}/{} Do\nQ\n", cm(a_m), name));
            drawn += 1;
            let _ = aid;
        }
        if !ops.is_empty() {
            add_content(doc, pid, ops.as_bytes(), true)?;
        }
        let pd = doc.get_dictionary_mut(pid).map_err(|e| Error::Invalid(e.to_string()))?;
        if keep.is_empty() {
            pd.remove(b"Annots");
        } else {
            pd.set("Annots", Object::Array(keep));
        }
        job.tick(k as u32 + 1, n)?;
    }
    if let Ok(cat) = doc.catalog_mut() {
        cat.remove(b"AcroForm");
    }
    Ok(drawn)
}

/// 양식·주석을 평평하게(따로 쓰는 작업)
pub fn flatten(input: &Path, password: Option<&str>, output: &Path, job: &mut Job) -> Result<EditOut> {
    let mut doc = open(input, password)?;
    let n = doc.get_pages().len() as u32;
    let drawn = flatten_doc(&mut doc, job)?;
    super::save(doc, output)?;
    Ok(EditOut { output: output.to_path_buf(), pages: n, note: format!("양식 칸·주석 {drawn}개를 쪽 내용으로 바꿨습니다") })
}
