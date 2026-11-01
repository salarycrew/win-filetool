//! 0.3.0 PDF 편집 코어(원래 글자 고치기는 범위 밖, 위에 새 내용을 얹는 것은 범위 안).
//!
//! - [`thumbs`]: 쪽 미리보기(PDFium 렌더 → PNG)·쪽 크기/회전 정보
//! - [`plan`]: 쪽 편집 계획 한 번에 적용(순서·회전·지우기·뽑기·빈 쪽·다른 PDF 끼워 넣기·합치기)
//! - [`stamp`]: 쪽 번호·워터마크(글자/그림)·서명/도장 그림·글상자
//! - [`crypt`]: 암호 걸기(AES-128/256, 인쇄·복사·편집 제한)·풀기
//! - [`layout`]: 여백 자르기(직접/자동)·A4에 맞추기·모아 찍기(2/4쪽)
//! - [`info`]: 문서 정보(Info 사전 + XMP) 읽기·바꾸기·지우기
//! - [`forms`]: 양식 칸 목록·채우기·평평하게(양식·주석을 쪽 내용으로)
//! - [`redact`]: 0.4 진짜 가리기(가린 쪽은 그림으로 바꾸고 숨은 내용·문서 정보·첨부·자바스크립트 지우기), 찾아서 가리기, 남았는지 확인
//! - [`font`]: 한글 글꼴 찾기·부분 넣기(시스템 맑은 고딕 등, 앱에 글꼴을 넣지 않음)
//!
//! 공통 규칙: 입력이 암호 PDF면 [`Error::PasswordRequired`]/[`Error::WrongPassword`]로 끝나고(화면은 암호 칸을 띄움),
//! 결과는 늘 새 파일([`crate::paths::write_new`]), 원본은 그대로 둔다.
//! 좌표는 "보이는 쪽" 기준 pt(1/72 in), 원점은 **왼쪽 위**(화면 미리보기와 같은 방향, /Rotate 반영)이다.

pub mod crypt;
pub mod font;
pub mod forms;
pub mod info;
pub mod layout;
pub mod plan;
pub mod redact;
pub mod stamp;
pub mod testpdf;
pub mod testpdf040;
pub mod thumbs;

use std::path::{Path, PathBuf};

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};
pub use serde::Serialize;
use serde::Deserialize;

use crate::error::{io_err, Error, Result};
use crate::process::Cancel;

/// 결과 하나(대부분의 작업)
#[derive(Debug, Clone, Serialize)]
pub struct EditOut {
    pub output: PathBuf,
    pub pages: u32,
    /// 사용자에게 보여 줄 짧은 설명
    pub note: String,
}

/// 진행률·취소. `progress(i, n)`은 i쪽까지 끝났다는 뜻.
pub struct Job<'a> {
    pub cancel: Option<&'a Cancel>,
    pub progress: Option<&'a mut dyn FnMut(u32, u32)>,
}

impl<'a> Job<'a> {
    pub fn none() -> Job<'static> {
        Job { cancel: None, progress: None }
    }
    pub fn new(cancel: Option<&'a Cancel>, progress: &'a mut dyn FnMut(u32, u32)) -> Self {
        Job { cancel, progress: Some(progress) }
    }
    pub fn check(&self) -> Result<()> {
        match self.cancel {
            Some(c) => c.check(),
            None => Ok(()),
        }
    }
    pub fn tick(&mut self, i: u32, n: u32) -> Result<()> {
        self.check()?;
        if let Some(p) = self.progress.as_mut() {
            p(i, n);
        }
        Ok(())
    }
}

pub(crate) fn pdf_err(path: &Path) -> impl Fn(lopdf::Error) -> Error + '_ {
    move |e| Error::Pdf { path: path.to_path_buf(), message: e.to_string() }
}

/// PDF 열기. 암호가 걸려 있으면 `password`로 푼다(빈 문자열 = 없음).
pub fn open(path: &Path, password: Option<&str>) -> Result<Document> {
    crate::error::ensure_nonempty(path)?;
    let pw = password.filter(|s| !s.is_empty());
    let loaded = match pw {
        Some(p) => Document::load_with_password(path, p),
        None => Document::load(path),
    };
    let doc = match loaded {
        Ok(d) => d,
        Err(lopdf::Error::InvalidPassword) => return Err(Error::WrongPassword(path.to_path_buf())),
        Err(lopdf::Error::IO(e)) => return Err(Error::Io { path: path.to_path_buf(), source: e }),
        Err(e) => return Err(Error::Pdf { path: path.to_path_buf(), message: e.to_string() }),
    };
    if doc.is_encrypted() {
        return Err(if pw.is_some() { Error::WrongPassword(path.to_path_buf()) } else { Error::PasswordRequired(path.to_path_buf()) });
    }
    if doc.get_pages().is_empty() {
        return Err(Error::Pdf { path: path.to_path_buf(), message: "페이지가 없습니다".into() });
    }
    Ok(doc)
}

/// 정리 → 압축 → 새 파일로 저장
pub(crate) fn save(mut doc: Document, output: &Path) -> Result<()> {
    if output.exists() {
        return Err(Error::AlreadyExists(output.to_path_buf()));
    }
    doc.prune_objects();
    doc.compress();
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(io_err(output))?;
    crate::paths::write_new(output, &bytes)
}

/// 원본 옆(또는 `out_dir`)에 `이름_접미사.pdf`
pub fn default_output(input: &Path, out_dir: Option<&Path>, suffix: &str) -> PathBuf {
    crate::paths::output_path(input, out_dir, suffix, "pdf")
}

// ---------- 객체 도우미 ----------

pub(crate) fn resolve<'a>(doc: &'a Document, obj: &'a Object) -> &'a Object {
    let mut cur = obj;
    for _ in 0..16 {
        match cur {
            Object::Reference(id) => match doc.get_object(*id) {
                Ok(o) => cur = o,
                Err(_) => return cur,
            },
            _ => return cur,
        }
    }
    cur
}

pub(crate) fn num(doc: &Document, obj: &Object) -> Option<f32> {
    match resolve(doc, obj) {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

pub(crate) fn rect(doc: &Document, obj: &Object) -> Option<[f32; 4]> {
    let arr = resolve(doc, obj).as_array().ok()?;
    if arr.len() != 4 {
        return None;
    }
    let v: Vec<f32> = arr.iter().filter_map(|o| num(doc, o)).collect();
    if v.len() != 4 {
        return None;
    }
    Some([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

pub(crate) fn rect_obj(r: [f32; 4]) -> Object {
    Object::Array(r.iter().map(|&v| Object::Real(round3(v))).collect())
}

fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

/// 페이지에서 (부모로부터 물려받는 것까지) 키 값
pub(crate) fn inherited(doc: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut cur = Some(page);
    let mut guard = 0;
    while let Some(id) = cur {
        guard += 1;
        if guard > 64 {
            break;
        }
        let d = doc.get_dictionary(id).ok()?;
        if let Ok(v) = d.get(key) {
            return Some(v.clone());
        }
        cur = d.get(b"Parent").and_then(Object::as_reference).ok();
    }
    None
}

/// 쪽의 보이는 상자(CropBox, 없으면 MediaBox)와 회전
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PageGeom {
    /// 사용자 공간의 [x0, y0, x1, y1]
    pub bx: [f32; 4],
    /// 0/90/180/270
    pub rotate: i64,
}

pub type Matrix = [f32; 6];

pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `m1`을 먼저, 그다음 `m2`
pub fn mat_mul(m1: Matrix, m2: Matrix) -> Matrix {
    [
        m1[0] * m2[0] + m1[1] * m2[2],
        m1[0] * m2[1] + m1[1] * m2[3],
        m1[2] * m2[0] + m1[3] * m2[2],
        m1[2] * m2[1] + m1[3] * m2[3],
        m1[4] * m2[0] + m1[5] * m2[2] + m2[4],
        m1[4] * m2[1] + m1[5] * m2[3] + m2[5],
    ]
}

pub fn mat_apply(m: Matrix, x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

pub fn mat_invert(m: Matrix) -> Matrix {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return IDENTITY;
    }
    let a = m[3] / det;
    let b = -m[1] / det;
    let c = -m[2] / det;
    let d = m[0] / det;
    [a, b, c, d, -(m[4] * a + m[5] * c), -(m[4] * b + m[5] * d)]
}

/// 상자 네 꼭짓점을 변환한 뒤 감싸는 상자
pub fn mat_bbox(m: Matrix, r: [f32; 4]) -> [f32; 4] {
    let pts = [mat_apply(m, r[0], r[1]), mat_apply(m, r[2], r[1]), mat_apply(m, r[0], r[3]), mat_apply(m, r[2], r[3])];
    let xs = pts.iter().map(|p| p.0);
    let ys = pts.iter().map(|p| p.1);
    [
        xs.clone().fold(f32::INFINITY, f32::min),
        ys.clone().fold(f32::INFINITY, f32::min),
        xs.fold(f32::NEG_INFINITY, f32::max),
        ys.fold(f32::NEG_INFINITY, f32::max),
    ]
}

impl PageGeom {
    pub fn w(&self) -> f32 {
        self.bx[2] - self.bx[0]
    }
    pub fn h(&self) -> f32 {
        self.bx[3] - self.bx[1]
    }
    /// 보이는 쪽의 가로·세로(90/270이면 바뀜)
    pub fn visual_size(&self) -> (f32, f32) {
        if self.rotate % 180 == 0 {
            (self.w(), self.h())
        } else {
            (self.h(), self.w())
        }
    }
    /// 보이는 쪽 좌표(원점 왼쪽 아래, y 위로) → 사용자 공간
    pub fn visual_to_user(&self) -> Matrix {
        let [x0, y0, _, _] = self.bx;
        let (w, h) = (self.w(), self.h());
        match self.rotate {
            90 => [0.0, 1.0, -1.0, 0.0, x0 + w, y0],
            180 => [-1.0, 0.0, 0.0, -1.0, x0 + w, y0 + h],
            270 => [0.0, -1.0, 1.0, 0.0, x0, y0 + h],
            _ => [1.0, 0.0, 0.0, 1.0, x0, y0],
        }
    }
    /// 왼쪽 위 원점 사각형(x, y, w, h) → 보이는 쪽(왼쪽 아래 원점) 사각형 [x0,y0,x1,y1]
    pub fn top_left_rect(&self, x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
        let (_, vh) = self.visual_size();
        [x, vh - y - h, x + w, vh - y]
    }
}

pub fn page_geom(doc: &Document, page: ObjectId) -> PageGeom {
    let media = inherited(doc, page, b"MediaBox").and_then(|o| rect(doc, &o)).unwrap_or([0.0, 0.0, 595.276, 841.89]);
    let crop = inherited(doc, page, b"CropBox").and_then(|o| rect(doc, &o));
    // CropBox는 MediaBox 안으로 자른다
    let bx = match crop {
        Some(c) => {
            let r = [c[0].max(media[0]), c[1].max(media[1]), c[2].min(media[2]), c[3].min(media[3])];
            if r[2] - r[0] > 1.0 && r[3] - r[1] > 1.0 {
                r
            } else {
                media
            }
        }
        None => media,
    };
    let rotate = inherited(doc, page, b"Rotate").and_then(|o| num(doc, &o)).map(|v| v as i64).unwrap_or(0).rem_euclid(360);
    let rotate = (rotate / 90) * 90;
    PageGeom { bx, rotate }
}

/// 쪽 id 목록(1쪽부터)
pub(crate) fn page_ids(doc: &Document) -> Vec<ObjectId> {
    doc.get_pages().into_values().collect()
}

/// 숫자 → 내용 스트림용 짧은 문자열
pub(crate) fn f(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".into()
    } else {
        s.to_string()
    }
}

pub(crate) fn cm(m: Matrix) -> String {
    format!("{} {} {} {} {} {} cm\n", f(m[0]), f(m[1]), f(m[2]), f(m[3]), f(m[4]), f(m[5]))
}

/// 쪽의 내용 스트림 id 목록(직접 스트림이면 간접 객체로 바꾼다)
pub(crate) fn content_ids(doc: &mut Document, page: ObjectId) -> Result<Vec<ObjectId>> {
    let contents = doc.get_dictionary(page).ok().and_then(|d| d.get(b"Contents").ok().cloned());
    let mut ids = Vec::new();
    match contents {
        Some(Object::Reference(id)) => match doc.get_object(id) {
            Ok(Object::Array(arr)) => {
                for o in arr.clone() {
                    if let Object::Reference(r) = o {
                        ids.push(r);
                    }
                }
            }
            Ok(Object::Stream(_)) => ids.push(id),
            _ => {}
        },
        Some(Object::Array(arr)) => {
            for o in arr {
                match o {
                    Object::Reference(r) => ids.push(r),
                    Object::Stream(s) => ids.push(doc.add_object(s)),
                    _ => {}
                }
            }
        }
        Some(Object::Stream(s)) => ids.push(doc.add_object(s)),
        _ => {}
    }
    Ok(ids)
}

/// 쪽 내용을 하나의 바이트열로(풀어서 이어 붙임)
pub(crate) fn page_content_bytes(doc: &mut Document, page: ObjectId) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for id in content_ids(doc, page)? {
        if let Ok(Object::Stream(s)) = doc.get_object(id) {
            let data = s.decompressed_content().unwrap_or_else(|_| s.content.clone());
            out.extend_from_slice(&data);
            out.push(b'\n');
        }
    }
    Ok(out)
}

/// 쪽에 내용을 덧붙인다. `over`면 원래 내용 위, 아니면 아래.
/// 원래 내용은 `q … Q`로 감싸 그래픽 상태가 새 내용에 번지지 않게 한다.
pub(crate) fn add_content(doc: &mut Document, page: ObjectId, ops: &[u8], over: bool) -> Result<()> {
    let ids = content_ids(doc, page)?;
    let mut ours = b"q\n".to_vec();
    ours.extend_from_slice(ops);
    ours.extend_from_slice(b"\nQ\n");
    let ours_id = doc.add_object(Stream::new(dictionary! {}, ours));
    let mut arr: Vec<Object> = Vec::new();
    if over {
        let q = doc.add_object(Stream::new(dictionary! {}, b"q\n".to_vec()));
        let qq = doc.add_object(Stream::new(dictionary! {}, b"\nQ\n".to_vec()));
        arr.push(Object::Reference(q));
        arr.extend(ids.iter().map(|&i| Object::Reference(i)));
        arr.push(Object::Reference(qq));
        arr.push(Object::Reference(ours_id));
    } else {
        arr.push(Object::Reference(ours_id));
        let q = doc.add_object(Stream::new(dictionary! {}, b"q\n".to_vec()));
        let qq = doc.add_object(Stream::new(dictionary! {}, b"\nQ\n".to_vec()));
        arr.push(Object::Reference(q));
        arr.extend(ids.iter().map(|&i| Object::Reference(i)));
        arr.push(Object::Reference(qq));
    }
    let d = doc.get_dictionary_mut(page).map_err(|e| Error::Invalid(e.to_string()))?;
    d.set("Contents", Object::Array(arr));
    Ok(())
}

/// 쪽이 자기만의 Resources 사전(직접 객체)을 갖게 하고, `category` 하위 사전도 직접 객체로 만든다.
fn own_resources(doc: &mut Document, page: ObjectId, category: &[u8]) -> Result<()> {
    let res = inherited(doc, page, b"Resources");
    let mut dict = match res.as_ref().map(|o| resolve(doc, o)) {
        Some(Object::Dictionary(d)) => d.clone(),
        _ => Dictionary::new(),
    };
    let sub = match dict.get(category).ok().map(|o| resolve(doc, o)) {
        Some(Object::Dictionary(d)) => d.clone(),
        _ => Dictionary::new(),
    };
    dict.set(category.to_vec(), Object::Dictionary(sub));
    let p = doc.get_dictionary_mut(page).map_err(|e| Error::Invalid(e.to_string()))?;
    p.set("Resources", Object::Dictionary(dict));
    Ok(())
}

/// 쪽 리소스에 이름을 겹치지 않게 붙여 넣고 그 이름을 돌려준다(예: `/Fx1`).
pub(crate) fn add_resource(doc: &mut Document, page: ObjectId, category: &[u8], prefix: &str, value: Object) -> Result<String> {
    own_resources(doc, page, category)?;
    let p = doc.get_dictionary_mut(page).map_err(|e| Error::Invalid(e.to_string()))?;
    let res = p.get_mut(b"Resources").and_then(Object::as_dict_mut).map_err(|e| Error::Invalid(e.to_string()))?;
    let sub = res.get_mut(category).and_then(Object::as_dict_mut).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut n = 1;
    let name = loop {
        let cand = format!("{prefix}{n}");
        if !sub.has(cand.as_bytes()) {
            break cand;
        }
        n += 1;
    };
    sub.set(name.clone().into_bytes(), value);
    Ok(name)
}

/// 투명도용 ExtGState(채우기·선 모두)
pub(crate) fn alpha_gs(doc: &mut Document, alpha: f32) -> ObjectId {
    let a = alpha.clamp(0.0, 1.0);
    doc.add_object(dictionary! { "Type" => "ExtGState", "ca" => Object::Real(a), "CA" => Object::Real(a) })
}

/// `#RRGGBB`/`RRGGBB`/`#RGB` → 0..1 RGB
pub fn parse_color(s: &str) -> Result<[f32; 3]> {
    let t = s.trim().trim_start_matches('#');
    let hex = |h: &str| u8::from_str_radix(h, 16).map(|v| v as f32 / 255.0);
    let r = match t.len() {
        6 => (hex(&t[0..2]), hex(&t[2..4]), hex(&t[4..6])),
        3 => {
            let d = |i: usize| u8::from_str_radix(&t[i..i + 1].repeat(2), 16).map(|v| v as f32 / 255.0);
            (d(0), d(1), d(2))
        }
        _ => return Err(Error::Invalid(format!("색은 #RRGGBB 형식으로 적어 주세요 ({s})"))),
    };
    match r {
        (Ok(a), Ok(b), Ok(c)) => Ok([a, b, c]),
        _ => Err(Error::Invalid(format!("색은 #RRGGBB 형식으로 적어 주세요 ({s})"))),
    }
}

pub(crate) fn rg(c: [f32; 3]) -> String {
    format!("{} {} {} rg\n", f(c[0]), f(c[1]), f(c[2]))
}

/// 쪽 선택(`1-3,5`, 빈 값/`all` = 전체) → 0부터 시작하는 번호
pub(crate) fn select_pages(spec: Option<&str>, n: u32) -> Result<Vec<usize>> {
    let list = match spec.map(str::trim) {
        None | Some("") => (1..=n).collect(),
        Some(s) if s.eq_ignore_ascii_case("all") || s == "전체" => (1..=n).collect(),
        Some(s) => crate::pages::parse_list(s, n)?,
    };
    let mut seen = std::collections::BTreeSet::new();
    Ok(list.into_iter().filter(|p| seen.insert(*p)).map(|p| p as usize - 1).collect())
}

/// 그림 파일 → 이미지 XObject. `white_to_alpha`면 흰색에 가까운 배경을 투명하게(서명·도장용).
/// 반환: (객체 id, 가로 px, 세로 px)
pub(crate) fn image_xobject(doc: &mut Document, path: &Path, white_to_alpha: bool) -> Result<(ObjectId, u32, u32)> {
    let bytes = std::fs::read(path).map_err(io_err(path))?;
    image_xobject_bytes(doc, &bytes, path, white_to_alpha)
}

pub(crate) fn image_xobject_bytes(doc: &mut Document, bytes: &[u8], path: &Path, white_to_alpha: bool) -> Result<(ObjectId, u32, u32)> {
    let img_err = |m: String| Error::Image { path: path.to_path_buf(), message: m };
    let fmt = image::guess_format(bytes).map_err(|e| img_err(format!("그림 형식을 알 수 없습니다: {e}")))?;
    // JPEG(RGB/흑백)는 다시 압축하지 않고 그대로 넣는다
    if fmt == image::ImageFormat::Jpeg && !white_to_alpha {
        use image::ImageDecoder;
        if let Ok(dec) = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(bytes)) {
            let (w, h) = dec.dimensions();
            let cs = match dec.color_type() {
                image::ColorType::L8 => Some("DeviceGray"),
                image::ColorType::Rgb8 => Some("DeviceRGB"),
                _ => None,
            };
            if let Some(cs) = cs {
                let s = Stream::new(
                    dictionary! {
                        "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
                        "ColorSpace" => cs, "BitsPerComponent" => 8, "Filter" => "DCTDecode",
                    },
                    bytes.to_vec(),
                )
                .with_compression(false);
                return Ok((doc.add_object(s), w, h));
            }
        }
    }
    let img = image::load_from_memory(bytes).map_err(|e| img_err(format!("그림을 읽지 못했습니다: {e}")))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    let mut alpha = Vec::with_capacity((w * h) as usize);
    let mut any_alpha = false;
    for p in rgba.pixels() {
        let [r, g, b, a] = p.0;
        rgb.extend_from_slice(&[r, g, b]);
        let mut a2 = a;
        if white_to_alpha {
            // 가장 어두운 채널 기준: 235 이상은 투명, 200~235는 부드럽게
            let m = r.min(g).min(b) as i32;
            let k = if m >= 235 { 0 } else if m <= 200 { 255 } else { (235 - m) * 255 / 35 };
            a2 = ((a as i32 * k) / 255) as u8;
        }
        if a2 != 255 {
            any_alpha = true;
        }
        alpha.push(a2);
    }
    let mut dict = dictionary! {
        "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64,
        "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8,
    };
    if any_alpha {
        let mut sm = Stream::new(
            dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64, "ColorSpace" => "DeviceGray", "BitsPerComponent" => 8 },
            alpha,
        );
        let _ = sm.compress();
        dict.set("SMask", Object::Reference(doc.add_object(sm)));
    }
    let mut s = Stream::new(dict, rgb);
    let _ = s.compress();
    Ok((doc.add_object(s), w, h))
}

/// 쪽(내용+리소스)을 Form XObject로 만든다(A4 맞춤·모아 찍기용). BBox = 보이는 상자(사용자 공간).
pub(crate) fn page_to_form(doc: &mut Document, page: ObjectId) -> Result<(ObjectId, PageGeom)> {
    let g = page_geom(doc, page);
    let content = page_content_bytes(doc, page)?;
    let res = inherited(doc, page, b"Resources").unwrap_or(Object::Dictionary(Dictionary::new()));
    let mut s = Stream::new(
        dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => rect_obj(g.bx), "Resources" => res },
        content,
    );
    let _ = s.compress();
    Ok((doc.add_object(s), g))
}

/// JSON 문자열(CLI·점검용; CLI는 serde에 직접 의존하지 않음)
pub fn to_json<T: Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

/// JSON → 값(쪽 편집 계획 등)
pub fn from_json<T: serde::de::DeserializeOwned>(s: &str) -> Result<T> {
    serde_json::from_str(s).map_err(|e| Error::Invalid(format!("JSON을 읽지 못했습니다: {e}")))
}

/// 내용이 자주 쓰는 값(쪽 크기 등)
pub const A4: (f32, f32) = (595.276, 841.89);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    /// 쪽 모양에 따라(가로가 길면 가로)
    #[default]
    Auto,
    Portrait,
    Landscape,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrices_round_trip_and_rotations() {
        for rot in [0, 90, 180, 270] {
            let g = PageGeom { bx: [10.0, 20.0, 210.0, 320.0], rotate: rot };
            let v = g.visual_to_user();
            let (vw, vh) = g.visual_size();
            // 보이는 쪽 네 모서리는 상자 네 모서리로
            let bb = mat_bbox(v, [0.0, 0.0, vw, vh]);
            for (a, b) in bb.iter().zip(g.bx.iter()) {
                assert!((a - b).abs() < 1e-3, "rot {rot}: {bb:?}");
            }
            let inv = mat_invert(v);
            let (x, y) = mat_apply(mat_mul(v, inv), 3.0, 7.0);
            assert!((x - 3.0).abs() < 1e-3 && (y - 7.0).abs() < 1e-3);
        }
        // 90도: 보이는 쪽 왼쪽 위(0, vh)는 사용자 공간 왼쪽 아래(x0, y0)
        let g = PageGeom { bx: [0.0, 0.0, 200.0, 300.0], rotate: 90 };
        let (_, vh) = g.visual_size();
        assert_eq!(mat_apply(g.visual_to_user(), 0.0, vh), (0.0, 0.0));
    }

    #[test]
    fn colors_and_numbers() {
        assert_eq!(parse_color("#ff0000").unwrap(), [1.0, 0.0, 0.0]);
        assert_eq!(parse_color("0f0").unwrap(), [0.0, 1.0, 0.0]);
        assert!(parse_color("red").is_err());
        assert_eq!(f(1.5), "1.5");
        assert_eq!(f(2.0), "2");
        assert_eq!(f(-0.0001), "0");
    }
}
