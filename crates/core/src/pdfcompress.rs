//! PDF 압축(순수 Rust, lopdf + image). 글자·벡터(콘텐츠 스트림, 글꼴)는 그대로 두고
//! **들어 있는 사진(래스터 이미지)만** 줄인다.
//!
//! 1. 각 쪽의 콘텐츠를 읽어(`q/Q/cm/Do`, Form XObject 안까지) 이미지가 실제로 몇 pt 크기로 찍히는지 구한다.
//!    여러 번 쓰인 이미지는 가장 크게 찍힌 크기를 기준으로 한다. 찾지 못하면(주석·패턴 안 등) 줄이지 않고 품질만 바꾼다.
//! 2. 프리셋의 최대 DPI보다 해상도가 높으면 그 DPI로 줄이고, JPEG(품질 = 프리셋)로 다시 인코딩한다.
//!    다시 인코딩한 결과가 원래 스트림보다 크면 그 이미지는 원래대로 둔다.
//!    건드리지 않는 이미지: 마스크(ImageMask, 다른 이미지의 SMask/Mask), 8비트가 아닌 것, CMYK·Indexed·Lab 등,
//!    `/Decode`가 있는 것, JPX/JBIG2/CCITT 등 다른 필터, 4 KB 미만의 작은 이미지.
//! 3. 쓰지 않는 객체를 지우고(prune), 압축되지 않은 스트림은 Flate로, 객체 스트림 + xref 스트림으로 저장한다.
//!
//! 결과가 원본보다 작지 않으면 새 파일을 만들지 않고 그렇다고 알린다. 암호가 걸린 PDF는 다루지 않는다.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageEncoder};
use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId, SaveOptions};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::paths::{output_path, write_new};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompressPreset {
    /// 화면용: 96 DPI, JPEG 품질 55
    Screen,
    /// 보통: 150 DPI, 품질 70
    Normal,
    /// 인쇄용: 300 DPI, 품질 85
    Print,
}

impl CompressPreset {
    pub fn max_dpi(self) -> f64 {
        match self {
            CompressPreset::Screen => 96.0,
            CompressPreset::Normal => 150.0,
            CompressPreset::Print => 300.0,
        }
    }
    pub fn quality(self) -> u8 {
        match self {
            CompressPreset::Screen => 55,
            CompressPreset::Normal => 70,
            CompressPreset::Print => 85,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            CompressPreset::Screen => "화면용",
            CompressPreset::Normal => "보통",
            CompressPreset::Print => "인쇄용",
        }
    }
    pub fn level(self) -> Level {
        Level { max_dpi: self.max_dpi(), quality: self.quality() }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "screen" | "화면용" => Some(Self::Screen),
            "normal" | "보통" => Some(Self::Normal),
            "print" | "인쇄용" => Some(Self::Print),
            _ => None,
        }
    }
}

/// 사진을 줄이는 정도: 찍히는 크기 기준 최대 DPI와 JPEG 품질
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    pub max_dpi: f64,
    pub quality: u8,
}

/// 0.3.2 목표 크기 맞추기에서 차례로 해 보는 단계(보통 → 가장 작게). 글자·선은 어느 단계에서도 그대로다.
pub const TARGET_LADDER: [Level; 8] = [
    Level { max_dpi: 150.0, quality: 75 },
    Level { max_dpi: 150.0, quality: 60 },
    Level { max_dpi: 120.0, quality: 55 },
    Level { max_dpi: 100.0, quality: 50 },
    Level { max_dpi: 85.0, quality: 45 },
    Level { max_dpi: 72.0, quality: 40 },
    Level { max_dpi: 60.0, quality: 35 },
    Level { max_dpi: 50.0, quality: 30 },
];

#[derive(Debug, Clone, Serialize)]
pub struct CompressOut {
    pub input: PathBuf,
    /// 만든 파일(원본보다 작지 않으면 None — 새 파일을 만들지 않음)
    pub output: Option<PathBuf>,
    pub before: u64,
    /// 압축해 본 결과 크기(새 파일을 만들지 않았어도 표시)
    pub after: u64,
    pub pages: u32,
    pub images_total: usize,
    pub images_recompressed: usize,
    pub images_downsampled: usize,
    /// 사람이 읽을 요약
    pub note: String,
    /// 0.3.2: 목표 크기(바이트, 목표 없이 줄였으면 None)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<u64>,
    /// 0.3.2: 목표 이하로 만들었는지(목표가 없으면 true)
    pub reached: bool,
    /// 0.3.2: 원본이 이미 목표보다 작아 손대지 않았는지
    pub already_small: bool,
}

pub const MIN_IMAGE_BYTES: usize = 4 * 1024;

const ENCRYPTED_MSG: &str = "암호가 걸린 PDF는 압축할 수 없습니다. PDF 프로그램에서 암호를 풀어 저장한 뒤 다시 시도하세요";

fn pdf_err(path: &Path) -> impl Fn(lopdf::Error) -> Error + '_ {
    move |e| Error::Pdf { path: path.to_path_buf(), message: e.to_string() }
}

/// 암호 검사까지 해서 연다(압축·이미지 추출 공통).
pub fn load_unencrypted(path: &Path) -> Result<Document> {
    crate::error::ensure_nonempty(path)?;
    let doc = match Document::load(path) {
        Ok(d) => d,
        Err(e) => {
            // 암호 때문에 못 연 경우도 같은 안내를
            let raw = std::fs::read(path).unwrap_or_default();
            if raw.windows(8).any(|w| w == b"/Encrypt") {
                return Err(Error::Pdf { path: path.into(), message: ENCRYPTED_MSG.into() });
            }
            return Err(pdf_err(path)(e));
        }
    };
    if doc.is_encrypted() || doc.was_encrypted() {
        return Err(Error::Pdf { path: path.into(), message: ENCRYPTED_MSG.into() });
    }
    if doc.get_pages().is_empty() {
        return Err(Error::Pdf { path: path.into(), message: "페이지가 없습니다".into() });
    }
    Ok(doc)
}

// ---------- 이미지가 찍히는 크기 구하기 ----------

type Matrix = [f64; 6];
const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn mul(m: &Matrix, n: &Matrix) -> Matrix {
    // m × n (PDF 행 벡터 규칙: 새 CTM = cm × 기존 CTM)
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
        m[4] * n[0] + m[5] * n[2] + n[4],
        m[4] * n[1] + m[5] * n[3] + n[5],
    ]
}

fn num(o: &Object) -> Option<f64> {
    match o {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

fn matrix_of(arr: &[Object]) -> Option<Matrix> {
    if arr.len() != 6 {
        return None;
    }
    let mut m = [0.0; 6];
    for (i, o) in arr.iter().enumerate() {
        m[i] = num(o)?;
    }
    Some(m)
}

fn resolve<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Object> {
    match o {
        Object::Reference(id) => doc.get_object(*id).ok(),
        other => Some(other),
    }
}

/// 이름 → XObject id (리소스 사전 여러 개를 순서대로)
fn xobjects_of(doc: &Document, res: &[&Dictionary]) -> HashMap<Vec<u8>, ObjectId> {
    let mut map = HashMap::new();
    for r in res.iter().rev() {
        if let Some(Object::Dictionary(xo)) = r.get(b"XObject").ok().and_then(|o| resolve(doc, o)) {
            for (k, v) in xo.iter() {
                if let Object::Reference(id) = v {
                    map.insert(k.clone(), *id);
                }
            }
        }
    }
    map
}

/// 이미지 id → 찍힌 가장 큰 (가로 pt, 세로 pt)
fn scan_content(
    doc: &Document,
    content: &[u8],
    xobjs: &HashMap<Vec<u8>, ObjectId>,
    base: Matrix,
    depth: u32,
    out: &mut HashMap<ObjectId, (f64, f64)>,
) {
    if depth > 8 {
        return;
    }
    let Ok(ops) = Content::decode(content) else { return };
    let mut stack: Vec<Matrix> = Vec::new();
    let mut ctm = base;
    for op in ops.operations {
        match op.operator.as_str() {
            "q" => stack.push(ctm),
            "Q" => ctm = stack.pop().unwrap_or(base),
            "cm" => {
                if let Some(m) = matrix_of(&op.operands) {
                    ctm = mul(&m, &ctm);
                }
            }
            "Do" => {
                let Some(name) = op.operands.first().and_then(|o| o.as_name().ok()) else { continue };
                let Some(&id) = xobjs.get(name) else { continue };
                let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else { continue };
                let sub = stream.dict.get(b"Subtype").and_then(Object::as_name).unwrap_or(b"");
                if sub == b"Image" {
                    let w = (ctm[0] * ctm[0] + ctm[1] * ctm[1]).sqrt();
                    let h = (ctm[2] * ctm[2] + ctm[3] * ctm[3]).sqrt();
                    let e = out.entry(id).or_insert((0.0, 0.0));
                    e.0 = e.0.max(w);
                    e.1 = e.1.max(h);
                } else if sub == b"Form" {
                    let m = stream.dict.get(b"Matrix").ok().and_then(|o| o.as_array().ok()).and_then(|a| matrix_of(a)).unwrap_or(IDENTITY);
                    let form_ctm = mul(&m, &ctm);
                    let mut sub_x = xobjs.clone();
                    if let Some(Object::Dictionary(r)) = stream.dict.get(b"Resources").ok().and_then(|o| resolve(doc, o)) {
                        sub_x.extend(xobjects_of(doc, &[r]));
                    }
                    if let Ok(c) = stream.decompressed_content() {
                        scan_content(doc, &c, &sub_x, form_ctm, depth + 1, out);
                    } else {
                        scan_content(doc, &stream.content, &sub_x, form_ctm, depth + 1, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// 문서 전체에서 이미지마다 가장 크게 찍힌 크기(pt)
pub fn image_display_sizes(doc: &Document) -> HashMap<ObjectId, (f64, f64)> {
    let mut out = HashMap::new();
    for (_, pid) in doc.get_pages() {
        let (own, inherited) = doc.get_page_resources(pid).unwrap_or((None, vec![]));
        let mut res: Vec<&Dictionary> = Vec::new();
        if let Some(d) = own {
            res.push(d);
        }
        for id in inherited {
            if let Ok(d) = doc.get_dictionary(id) {
                res.push(d);
            }
        }
        let xobjs = xobjects_of(doc, &res);
        let content = doc.get_page_content(pid);
        scan_content(doc, &content, &xobjs, IDENTITY, 0, &mut out);
    }
    out
}

// ---------- 이미지 다시 인코딩 ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Comps {
    Gray,
    Rgb,
}

fn color_comps(doc: &Document, cs: Option<&Object>) -> Option<Comps> {
    let cs = resolve(doc, cs?)?;
    match cs {
        Object::Name(n) if n == b"DeviceRGB" => Some(Comps::Rgb),
        Object::Name(n) if n == b"DeviceGray" => Some(Comps::Gray),
        Object::Array(a) if a.len() == 2 && a[0].as_name().ok() == Some(b"ICCBased") => {
            let s = resolve(doc, &a[1])?.as_stream().ok()?;
            match s.dict.get(b"N").ok().and_then(num) {
                Some(n) if n == 3.0 => Some(Comps::Rgb),
                Some(n) if n == 1.0 => Some(Comps::Gray),
                _ => None,
            }
        }
        _ => None,
    }
}

/// 다른 이미지의 마스크로 쓰이는 이미지 id
fn mask_ids(doc: &Document) -> HashSet<ObjectId> {
    let mut set = HashSet::new();
    for obj in doc.objects.values() {
        if let Ok(s) = obj.as_stream() {
            for key in [b"SMask".as_slice(), b"Mask".as_slice()] {
                if let Ok(Object::Reference(id)) = s.dict.get(key) {
                    set.insert(*id);
                }
            }
        }
    }
    set
}

enum Plan {
    Skip,
    Replace { bytes: Vec<u8>, w: u32, h: u32, downsampled: bool },
}

fn plan_image(doc: &Document, id: ObjectId, disp: Option<(f64, f64)>, preset: Level, masks: &HashSet<ObjectId>) -> Plan {
    let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else { return Plan::Skip };
    let d = &stream.dict;
    if masks.contains(&id)
        || d.get(b"ImageMask").and_then(Object::as_bool).unwrap_or(false)
        || d.has(b"Decode")
        || d.has(b"Mask") && !matches!(d.get(b"Mask"), Ok(Object::Reference(_)))
        || stream.content.len() < MIN_IMAGE_BYTES
    {
        return Plan::Skip;
    }
    let bpc = d.get(b"BitsPerComponent").ok().and_then(num).unwrap_or(0.0);
    let (Some(w), Some(h)) = (d.get(b"Width").ok().and_then(num), d.get(b"Height").ok().and_then(num)) else { return Plan::Skip };
    let (w, h) = (w as u32, h as u32);
    if bpc != 8.0 || w == 0 || h == 0 || (w as u64) * (h as u64) > 200_000_000 {
        return Plan::Skip;
    }
    let Some(comps) = color_comps(doc, d.get(b"ColorSpace").ok()) else { return Plan::Skip };
    let filters: Vec<Vec<u8>> = match d.get(b"Filter") {
        Ok(Object::Name(n)) => vec![n.clone()],
        Ok(Object::Array(a)) => a.iter().filter_map(|o| o.as_name().ok().map(<[u8]>::to_vec)).collect(),
        Err(_) => vec![],
        _ => return Plan::Skip,
    };
    let img: DynamicImage = match filters.iter().map(Vec::as_slice).collect::<Vec<_>>().as_slice() {
        [b"DCTDecode"] => {
            let Ok(im) = image::load_from_memory_with_format(&stream.content, image::ImageFormat::Jpeg) else { return Plan::Skip };
            if im.width() != w || im.height() != h {
                return Plan::Skip;
            }
            match (comps, im.color().channel_count()) {
                (Comps::Rgb, 3) | (Comps::Gray, 1) => im,
                _ => return Plan::Skip, // CMYK JPEG 등
            }
        }
        [] | [b"FlateDecode"] => {
            let Ok(raw) = stream.decompressed_content() else { return Plan::Skip };
            let n = if comps == Comps::Rgb { 3 } else { 1 };
            let need = (w as usize) * (h as usize) * n;
            if raw.len() < need {
                return Plan::Skip;
            }
            let raw = raw[..need].to_vec();
            match comps {
                Comps::Rgb => image::RgbImage::from_raw(w, h, raw).map(DynamicImage::ImageRgb8),
                Comps::Gray => image::GrayImage::from_raw(w, h, raw).map(DynamicImage::ImageLuma8),
            }
            .unwrap_or_else(|| DynamicImage::new_rgb8(0, 0))
        }
        _ => return Plan::Skip,
    };
    if img.width() == 0 {
        return Plan::Skip;
    }
    // 찍히는 크기 기준 목표 픽셀 수
    let mut img = img;
    let mut downsampled = false;
    if let Some((dw, dh)) = disp {
        if dw > 0.0 && dh > 0.0 {
            let tw = (dw / 72.0 * preset.max_dpi).ceil();
            let th = (dh / 72.0 * preset.max_dpi).ceil();
            let scale = (tw / w as f64).max(th / h as f64);
            // 10% 넘게 줄일 때만 줄인다
            if scale < 0.9 {
                let nw = ((w as f64 * scale).round() as u32).max(1);
                let nh = ((h as f64 * scale).round() as u32).max(1);
                img = img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3);
                downsampled = true;
            }
        }
    }
    let mut buf = Vec::new();
    let enc = JpegEncoder::new_with_quality(&mut buf, preset.quality);
    let ok = match comps {
        Comps::Rgb => {
            let rgb = img.to_rgb8();
            enc.write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
        }
        Comps::Gray => {
            let g = img.to_luma8();
            enc.write_image(g.as_raw(), g.width(), g.height(), image::ExtendedColorType::L8)
        }
    };
    if ok.is_err() || buf.len() >= stream.content.len() {
        // 다시 인코딩하면 커지는 이미지는 그대로
        return Plan::Skip;
    }
    Plan::Replace { bytes: buf, w: img.width(), h: img.height(), downsampled }
}

fn is_image(obj: &Object) -> bool {
    obj.as_stream().ok().and_then(|s| s.dict.get(b"Subtype").ok()).and_then(|o| o.as_name().ok()) == Some(b"Image")
}

/// 문서를 압축해 바이트로(파일은 쓰지 않음). (바이트, 이미지 수, 다시 인코딩한 수, 줄인 수)
pub fn compress_doc(doc: Document, preset: CompressPreset) -> Result<(Vec<u8>, usize, usize, usize)> {
    compress_doc_level(doc, preset.level(), None)
}

/// 0.3.2: 정한 해상도·품질로 압축(목표 크기 맞추기에서 여러 번 부름). 취소되면 `Error::Cancelled`.
pub fn compress_doc_level(mut doc: Document, level: Level, cancel: Option<&crate::process::Cancel>) -> Result<(Vec<u8>, usize, usize, usize)> {
    let sizes = image_display_sizes(&doc);
    let masks = mask_ids(&doc);
    let ids: Vec<ObjectId> = doc.objects.iter().filter(|(_, o)| is_image(o)).map(|(id, _)| *id).collect();
    let total = ids.len();
    let (mut re, mut down) = (0, 0);
    for id in ids {
        if let Some(c) = cancel {
            c.check()?;
        }
        if let Plan::Replace { bytes, w, h, downsampled } = plan_image(&doc, id, sizes.get(&id).copied(), level, &masks) {
            if let Ok(Object::Stream(s)) = doc.get_object_mut(id) {
                s.dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
                s.dict.remove(b"DecodeParms");
                s.dict.set("Width", Object::Integer(w as i64));
                s.dict.set("Height", Object::Integer(h as i64));
                s.dict.set("BitsPerComponent", Object::Integer(8));
                s.set_content(bytes);
                s.allows_compression = false;
                re += 1;
                if downsampled {
                    down += 1;
                }
            }
        }
    }
    doc.prune_objects();
    doc.delete_zero_length_streams();
    doc.compress();
    if doc.version.as_str() < "1.5" {
        doc.version = "1.5".into();
    }
    let mut bytes = Vec::new();
    doc.save_with_options(&mut bytes, SaveOptions::builder().use_object_streams(true).use_xref_streams(true).build())
        .map_err(|e| Error::Invalid(format!("PDF 저장 실패: {e}")))?;
    Ok((bytes, total, re, down))
}

pub fn human(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / 1048576.0)
    } else {
        format!("{:.0} KB", (n as f64 / 1024.0).max(1.0))
    }
}

/// 줄어든 비율(소수 한 자리, 예: "99.7%"). 결과가 0 바이트일 때만 "100%"이고,
/// 반올림하면 100이 되는 경우(99.96% 등)는 "99.9%"로 둔다. 늘어났거나 같으면 "0.0%".
pub fn reduction_pct(before: u64, after: u64) -> String {
    if before == 0 || after >= before {
        return "0.0%".into();
    }
    if after == 0 {
        return "100%".into();
    }
    let pct = 100.0 - after as f64 * 100.0 / before as f64;
    let tenths = ((pct * 10.0).round() as i64).clamp(0, 999);
    format!("{}.{}%", tenths / 10, tenths % 10)
}

/// PDF 하나 압축. 결과 이름은 `이름_압축.pdf`(있으면 `(1)`), 원본보다 작지 않으면 새 파일을 만들지 않는다.
pub fn compress(input: &Path, out_dir: Option<&Path>, preset: CompressPreset) -> Result<CompressOut> {
    let before = std::fs::metadata(input).map_err(crate::error::io_err(input))?.len();
    let doc = load_unencrypted(input)?;
    let pages = doc.get_pages().len() as u32;
    let (bytes, total, re, down) = compress_doc(doc, preset)?;
    let after = bytes.len() as u64;
    let mut out = CompressOut {
        input: input.into(),
        output: None,
        before,
        after,
        pages,
        images_total: total,
        images_recompressed: re,
        images_downsampled: down,
        note: String::new(),
        target: None,
        reached: true,
        already_small: false,
    };
    let img_note = format!("이미지 {total}개 중 {re}개 다시 압축({down}개 해상도 줄임)");
    if after >= before {
        out.note = format!("{} → {}: 더 줄지 않아 새 파일을 만들지 않았습니다(원본 그대로). {img_note}", human(before), human(after));
        return Ok(out);
    }
    let path = output_path(input, out_dir, "_압축", "pdf");
    write_new(&path, &bytes)?;
    out.note = format!("{} → {} ({} 줄어듦, {}, {pages}쪽). {img_note}", human(before), human(after), reduction_pct(before, after), preset.label());
    out.output = Some(path);
    Ok(out)
}

/// 0.3.2: 목표 크기(바이트) 이하가 될 때까지 [`TARGET_LADDER`]를 차례로 해 본다.
/// - 원본이 이미 목표 이하이면 새 파일을 만들지 않는다(`already_small`).
/// - 목표에 닿으면 그 결과를 저장한다(`reached = true`).
/// - 끝까지 해도 안 되면 **가장 작은 결과**를 저장하고(원본보다 작을 때만) `reached = false`로 얼마나 줄었는지 알린다.
/// `progress(i, n)`은 단계마다 부른다. 취소하면 `Error::Cancelled`이고 파일을 남기지 않는다.
pub fn compress_to_target(
    input: &Path,
    out_dir: Option<&Path>,
    target: u64,
    cancel: Option<&crate::process::Cancel>,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<CompressOut> {
    let before = std::fs::metadata(input).map_err(crate::error::io_err(input))?.len();
    let doc = load_unencrypted(input)?;
    let pages = doc.get_pages().len() as u32;
    let mut out = CompressOut {
        input: input.into(),
        output: None,
        before,
        after: before,
        pages,
        images_total: 0,
        images_recompressed: 0,
        images_downsampled: 0,
        note: String::new(),
        target: Some(target),
        reached: true,
        already_small: false,
    };
    if before <= target {
        out.already_small = true;
        out.note = format!("이미 {} 이하예요({}). 새 파일을 만들지 않았어요.", target_label(target), human(before));
        return Ok(out);
    }
    let n = TARGET_LADDER.len();
    let mut best: Option<(Vec<u8>, usize, usize, usize, Level)> = None;
    let mut i = 0;
    while i < n {
        if let Some(c) = cancel {
            c.check()?;
        }
        progress(i, n);
        let level = TARGET_LADDER[i];
        let (bytes, total, re, down) = compress_doc_level(doc.clone(), level, cancel)?;
        let len = bytes.len() as u64;
        let better = best.as_ref().map(|b| (len as usize) < b.0.len()).unwrap_or(true);
        if better {
            best = Some((bytes, total, re, down, level));
        }
        if len <= target || total == 0 {
            // 목표에 닿았거나, 사진이 하나도 없으면(글자·선만 있는 PDF) 더 해 봐야 같다
            break;
        }
        // 크기가 목표의 2배를 넘으면 한 단계씩 말고 두 단계 건너뛴다(50쪽 스캔에서 시간을 줄임)
        i += if len > target.saturating_mul(2) && i + 2 < n { 2 } else { 1 };
    }
    progress(n, n);
    let (bytes, total, re, down, level) = best.expect("한 번은 해 봄");
    let after = bytes.len() as u64;
    out.after = after;
    out.images_total = total;
    out.images_recompressed = re;
    out.images_downsampled = down;
    out.reached = after <= target;
    if after >= before {
        out.reached = false;
        out.note = format!(
            "더 줄일 수 없어요. 이 PDF는 사진보다 글자·선이 대부분이라 {}보다 작게 만들 수 없어요(원본 {} 그대로).",
            target_label(target),
            human(before)
        );
        return Ok(out);
    }
    let path = output_path(input, out_dir, "_압축", "pdf");
    write_new(&path, &bytes)?;
    out.output = Some(path);
    out.note = if out.reached {
        format!("{} → {} ({} 줄어듦, 목표 {} 이하, {pages}쪽, 사진 {}dpi·품질 {})", human(before), human(after), reduction_pct(before, after), target_label(target), level.max_dpi as u32, level.quality)
    } else {
        format!(
            "목표 {}까지는 못 줄였어요. 가장 작게 만든 것이 {}예요({} → {}, {} 줄어듦). 글자·선은 줄일 수 없어서 더 작게는 안 돼요.",
            target_label(target),
            human(after),
            human(before),
            human(after),
            reduction_pct(before, after)
        )
    };
    Ok(out)
}

/// 목표 크기 표시: 1,000,000 바이트 단위면 "5MB"(공공기관 "5MB 이하"는 보통 10진), 아니면 [`human`]
pub fn target_label(n: u64) -> String {
    if n >= 1_000_000 && n % 1_000_000 == 0 {
        format!("{}MB", n / 1_000_000)
    } else if n >= 1000 && n % 1000 == 0 && n < 1_000_000 {
        format!("{}KB", n / 1000)
    } else {
        human(n)
    }
}

#[cfg(test)]
mod tests {
    use super::reduction_pct;

    #[test]
    fn reduction_one_decimal_never_100_unless_empty() {
        assert_eq!(reduction_pct(1000, 3), "99.7%");
        assert_eq!(reduction_pct(100_000, 1), "99.9%"); // 99.999% → 100으로 반올림하지 않음
        assert_eq!(reduction_pct(10_000_000, 1), "99.9%");
        assert_eq!(reduction_pct(1000, 0), "100%");
        assert_eq!(reduction_pct(1000, 500), "50.0%");
        assert_eq!(reduction_pct(1000, 999), "0.1%");
        assert_eq!(reduction_pct(100_000, 99_999), "0.0%");
        assert_eq!(reduction_pct(1000, 1000), "0.0%");
        assert_eq!(reduction_pct(0, 0), "0.0%");
    }
}
