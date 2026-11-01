//! 0.4 글자 인식(OCR). Windows에 들어 있는 `Windows.Media.Ocr`(WinRT)를 쓴다 — 모델을 받거나 인터넷에 보내지 않는다.
//!
//! - [`Recognizer`]: 그림 한 장 → 줄·낱말 상자(px). Windows는 [`winrt`], 테스트는 직접 만든 가짜 인식기.
//! - [`recognize`]: 그림 다듬기(흐린 사진은 대비를 늘림, 너무 크면 줄이고 작으면 키움) → 인식. 글이 거의 안 읽히면
//!   90·180·270도로 돌려 다시 읽어 가장 잘 읽힌 방향을 고른다(옆으로 누운 스캔).
//! - [`textlayer`]: 보이지 않는 글자 층(렌더 모드 3, 글리프 없는 글꼴 + ToUnicode)을 쪽에 얹기
//! - [`pdf`]: 스캔 PDF를 검색 가능하게, 그림·스캔 PDF → TXT, PDF → Word용 줄 만들기
//!
//! 좌표: 그림 px(원점 왼쪽 위). 글 방향 `rot`(0/90/180/270)은 그림 안에서 글이 시계 방향으로 돈 각도
//! (0 = 왼쪽→오른쪽, 90 = 위→아래).

pub mod pdf;
pub mod textlayer;
#[cfg(windows)]
pub mod winrt;

use image::{DynamicImage, GrayImage};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::process::Cancel;

/// 낱말 하나(그림 px, 원점 왼쪽 위)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Word {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// 줄 하나
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TextLine {
    /// 엔진이 만든 줄 글자(낱말 사이 빈칸 포함)
    pub text: String,
    pub words: Vec<Word>,
    /// 글 방향(시계 방향 각도)
    pub rot: u16,
}

/// 그림 한 장의 인식 결과(원래 그림 px 기준)
#[derive(Debug, Clone, Serialize)]
pub struct Recognized {
    pub width: u32,
    pub height: u32,
    pub lines: Vec<TextLine>,
    /// 읽기 순서대로 줄을 이은 글(문단 사이 빈 줄)
    pub text: String,
    /// 고른 방향(0이 아니면 그림이 누워 있었음)
    pub rot: u16,
    /// 엔진이 알려 준 기울기(도), 없으면 0
    pub skew: f32,
}

impl Recognized {
    pub fn chars(&self) -> usize {
        self.text.chars().filter(|c| !c.is_whitespace()).count()
    }
    pub fn words(&self) -> usize {
        self.lines.iter().map(|l| l.words.len()).sum()
    }
}

/// 엔진 한 번 돌린 결과(엔진에 넣은 그림 px)
#[derive(Debug, Clone, Default)]
pub struct RawResult {
    pub lines: Vec<(String, Vec<Word>)>,
    pub skew: Option<f32>,
}

/// 글자 인식 엔진. Windows OCR 객체는 만든 스레드에서만 쓴다(`Send` 아님).
pub trait Recognizer {
    /// 쓰는 언어 태그(예: `ko`, `en-US`)
    fn language(&self) -> String;
    /// 엔진이 받는 그림의 가장 긴 변(px)
    fn max_dim(&self) -> u32;
    /// 똑바로 선 그림 한 장(흑백) 읽기
    fn recognize_raw(&self, img: &GrayImage) -> Result<RawResult>;
}

/// 한국어 OCR을 더하는 방법(설정 앱)
pub const KO_HINT: &str = "한글을 읽으려면 한국어 글자 인식을 설치해 주세요: 설정 → 시간 및 언어 → 언어 → 한국어 → 언어 옵션 → 광학 문자 인식(설치). 한국어가 목록에 없으면 [언어 추가]로 한국어를 먼저 넣어 주세요.";

/// 이 PC의 글자 인식 상태(화면·selftest)
#[derive(Debug, Clone, Serialize)]
pub struct OcrStatus {
    /// 글자 인식을 쓸 수 있는지
    pub available: bool,
    /// 설치된 인식 언어 태그
    pub languages: Vec<String>,
    /// 한국어 인식이 있는지
    pub korean: bool,
    /// 실제로 쓸 언어
    pub using: Option<String>,
    pub max_image_dimension: u32,
    /// 사용자에게 보여 줄 안내(한국어가 없을 때 등)
    pub hint: Option<String>,
}

/// 기본 엔진(Windows OCR). `lang`: `None`/`"auto"` = 한국어 → 사용자 언어 → 설치된 첫 언어.
pub fn engine(lang: Option<&str>) -> Result<Box<dyn Recognizer>> {
    #[cfg(windows)]
    {
        Ok(Box::new(winrt::WinOcr::new(lang)?))
    }
    #[cfg(not(windows))]
    {
        let _ = lang;
        Err(Error::Unsupported("글자 인식(OCR)은 Windows 10·11에서만 쓸 수 있어요".into()))
    }
}

/// 이 PC의 글자 인식 언어·크기 제한
pub fn status() -> OcrStatus {
    #[cfg(windows)]
    {
        winrt::status()
    }
    #[cfg(not(windows))]
    {
        OcrStatus { available: false, languages: vec![], korean: false, using: None, max_image_dimension: 0, hint: Some("글자 인식(OCR)은 Windows 10·11에서만 쓸 수 있어요".into()) }
    }
}

/// 한국어가 아닌 언어로 읽을 때 붙일 안내(한국어면 None)
pub fn language_hint(lang: &str) -> Option<String> {
    if lang.to_ascii_lowercase().starts_with("ko") {
        None
    } else {
        Some(format!("이 PC에는 한국어 글자 인식이 없어 {lang}(으)로 읽었어요. 한글은 잘 안 읽혀요. {KO_HINT}"))
    }
}

// ---------- 그림 다듬기 ----------

/// 흑백으로 바꾸고, 대비가 낮으면(어두운 사진·흐린 스캔) 1%~99% 밝기를 0~255로 늘린다.
pub fn prepare(img: &DynamicImage) -> GrayImage {
    let mut g = img.to_luma8();
    let mut hist = [0u64; 256];
    for p in g.pixels() {
        hist[p[0] as usize] += 1;
    }
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return g;
    }
    let pick = |frac: f64| {
        let target = (total as f64 * frac) as u64;
        let mut acc = 0u64;
        for (i, &h) in hist.iter().enumerate() {
            acc += h;
            if acc > target {
                return i as i32;
            }
        }
        255
    };
    let (lo, hi) = (pick(0.01), pick(0.99));
    if hi - lo < 170 && hi > lo + 8 {
        let span = (hi - lo) as f32;
        for p in g.pixels_mut() {
            let v = ((p[0] as i32 - lo) as f32 * 255.0 / span).round().clamp(0.0, 255.0);
            p[0] = v as u8;
        }
    }
    g
}

/// 엔진에 넣을 배율: 긴 변이 `max_dim`을 넘으면 줄이고, 1200px보다 작으면(작은 사진·캡처) 키운다.
fn scale_for(w: u32, h: u32, max_dim: u32) -> f32 {
    let long = w.max(h).max(1) as f32;
    let max_dim = max_dim.max(256) as f32;
    // 메모리: 4천만 화소를 넘지 않게
    let area_cap = (40_000_000.0 / (w as f32 * h as f32).max(1.0)).sqrt();
    let s = if long > max_dim {
        max_dim / long
    } else if long < 1200.0 {
        (1600.0 / long).min(2.5)
    } else {
        1.0
    };
    s.min(area_cap).max(0.05)
}

fn resize(g: &GrayImage, s: f32) -> GrayImage {
    if (s - 1.0).abs() < 1e-3 {
        return g.clone();
    }
    let w = ((g.width() as f32 * s).round() as u32).max(1);
    let h = ((g.height() as f32 * s).round() as u32).max(1);
    image::imageops::resize(g, w, h, if s < 1.0 { image::imageops::FilterType::Triangle } else { image::imageops::FilterType::CatmullRom })
}

/// 읽힌 정도: 2자 이상이고 한글·영문·숫자만으로 된 낱말의 글자 수, 그리고 그런 낱말의 비율
fn score(raw: &RawResult) -> (usize, f32) {
    let mut good = 0usize;
    let mut words = 0usize;
    let mut good_words = 0usize;
    for (_, ws) in &raw.lines {
        for w in ws {
            let t: Vec<char> = w.text.chars().filter(|c| !matches!(c, '.' | ',' | '(' | ')' | ':' | '-' | '·' | '\'' | '"')).collect();
            words += 1;
            if t.len() >= 2 && t.iter().all(|c| crate::pdflayout::is_hangul(*c) || c.is_ascii_alphanumeric()) {
                good += t.len();
                good_words += 1;
            }
        }
    }
    (good, if words == 0 { 0.0 } else { good_words as f32 / words as f32 })
}

fn rotate(g: &GrayImage, rot: u16) -> GrayImage {
    match rot {
        90 => image::imageops::rotate90(g),
        180 => image::imageops::rotate180(g),
        270 => image::imageops::rotate270(g),
        _ => g.clone(),
    }
}

/// `rot`만큼 시계 방향으로 돌린 그림(가로 rw)의 상자 → 돌리기 전 그림(가로 W, 세로 H)의 상자
fn unrotate_rect(rot: u16, (x, y, w, h): (f32, f32, f32, f32), big_w: f32, big_h: f32) -> (f32, f32, f32, f32) {
    match rot {
        // 시계 방향 90: 원래 (x, y) → (H-1-y, x)
        90 => (y, big_h - x - w, h, w),
        180 => (big_w - x - w, big_h - y - h, w, h),
        // 270(반시계 90): 원래 (x, y) → (y, W-1-x)
        270 => (big_w - y - h, x, h, w),
        _ => (x, y, w, h),
    }
}

/// 그림 한 장 인식(방향 자동). 결과 좌표는 넣은 그림 px.
pub fn recognize(r: &dyn Recognizer, img: &DynamicImage, cancel: Option<&Cancel>) -> Result<Recognized> {
    let (w0, h0) = (img.width(), img.height());
    if w0 < 8 || h0 < 8 {
        return Ok(Recognized { width: w0, height: h0, lines: vec![], text: String::new(), rot: 0, skew: 0.0 });
    }
    let g = prepare(img);
    let s = scale_for(w0, h0, r.max_dim());
    let g = resize(&g, s);
    let check = || cancel.map(|c| c.check()).unwrap_or(Ok(()));
    check()?;
    let first = r.recognize_raw(&g)?;
    let (sc0, ratio0) = score(&first);
    let mut best = (0u16, first, sc0);
    // 잘 읽혔으면 그대로. 아니면 돌려 본다(빈 쪽은 네 번 읽지만 빠름)
    if !(sc0 >= 40 && ratio0 >= 0.55) {
        for rot in [90u16, 270, 180] {
            check()?;
            let rg = rotate(&g, rot);
            // 돌린 그림이 엔진 크기 제한을 넘지 않음(정사각 제한)
            let raw = r.recognize_raw(&rg)?;
            let (sc, _) = score(&raw);
            if sc as f32 > best.2 as f32 * 1.5 + 8.0 {
                best = (rot, raw, sc);
            }
        }
    }
    let (rot, raw, _) = best;
    let (gw, gh) = (g.width() as f32, g.height() as f32);
    // 돌린 그림 좌표 → 엔진에 넣은(돌리기 전) 그림 → 원래 그림(배율 되돌림)
    let mut lines = Vec::new();
    // 문단 나누기용: 돌린 틀(글이 가로인 좌표)의 줄 위·아래
    let mut frame: Vec<(f32, f32)> = Vec::new();
    for (text, words) in raw.lines {
        let ws: Vec<Word> = words
            .into_iter()
            .filter(|w| !w.text.trim().is_empty())
            .map(|w| {
                let (x, y, ww, hh) = unrotate_rect(rot, (w.x, w.y, w.w, w.h), gw, gh);
                Word { text: w.text, x: x / s, y: y / s, w: ww / s, h: hh / s }
            })
            .collect();
        if ws.is_empty() {
            continue;
        }
        let top = ws.iter().map(|w| w.y).fold(f32::MAX, f32::min);
        let _ = top;
        lines.push(TextLine { text: text.trim().to_string(), words: ws, rot });
    }
    // 돌린 틀에서 줄 위치(읽기 순서는 엔진이 준 순서를 그대로 쓴다)
    for l in &lines {
        let (a, b) = match rot {
            90 => (l.words.iter().map(|w| -(w.x + w.w)).fold(f32::MAX, f32::min), l.words.iter().map(|w| -w.x).fold(f32::MIN, f32::max)),
            180 => (l.words.iter().map(|w| -(w.y + w.h)).fold(f32::MAX, f32::min), l.words.iter().map(|w| -w.y).fold(f32::MIN, f32::max)),
            270 => (l.words.iter().map(|w| w.x).fold(f32::MAX, f32::min), l.words.iter().map(|w| w.x + w.w).fold(f32::MIN, f32::max)),
            _ => (l.words.iter().map(|w| w.y).fold(f32::MAX, f32::min), l.words.iter().map(|w| w.y + w.h).fold(f32::MIN, f32::max)),
        };
        frame.push((a, b));
    }
    let text = join_lines(&lines, &frame);
    Ok(Recognized { width: w0, height: h0, lines, text, rot, skew: raw.skew.unwrap_or(0.0) })
}

/// 줄을 잇는다: 앞줄과 사이가 줄 높이(중앙값)보다 크게 벌어지면 빈 줄로 문단을 나눈다.
fn join_lines(lines: &[TextLine], frame: &[(f32, f32)]) -> String {
    let mut hs: Vec<f32> = frame.iter().map(|(a, b)| b - a).filter(|h| *h > 0.0).collect();
    hs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let med = hs.get(hs.len() / 2).copied().unwrap_or(10.0);
    let mut out = String::new();
    let mut prev: Option<(f32, f32)> = None;
    for (l, f) in lines.iter().zip(frame) {
        let t = if l.text.is_empty() { l.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ") } else { l.text.clone() };
        if let Some((_, pb)) = prev {
            out.push('\n');
            if f.0 - pb > med * 0.9 {
                out.push('\n');
            }
        }
        out.push_str(&t);
        prev = Some(*f);
    }
    out
}

/// 테스트·점검용 인식기: 정해 둔 낱말을 그림 크기 비율(0~1) 위치에 돌려준다.
/// 그림에 실제로 무엇이 있는지는 보지 않는다(쪽 층 위치·검색·가리기 경로를 Linux에서 시험하려고).
pub struct FixedRecognizer {
    /// (줄 글, [(낱말, x, y, w, h)]) — 비율 좌표
    pub lines: Vec<Vec<(String, f32, f32, f32, f32)>>,
    pub lang: String,
}

impl Recognizer for FixedRecognizer {
    fn language(&self) -> String {
        self.lang.clone()
    }
    fn max_dim(&self) -> u32 {
        10_000
    }
    fn recognize_raw(&self, img: &GrayImage) -> Result<RawResult> {
        let (w, h) = (img.width() as f32, img.height() as f32);
        // 세로로 긴 그림(돌린 그림)에는 아무것도 없다고 답해서 방향 고르기가 0도를 고르게
        let lines = self
            .lines
            .iter()
            .map(|ws| {
                let words: Vec<Word> = ws.iter().map(|(t, x, y, ww, hh)| Word { text: t.clone(), x: x * w, y: y * h, w: ww * w, h: hh * h }).collect();
                (ws.iter().map(|w| w.0.as_str()).collect::<Vec<_>>().join(" "), words)
            })
            .collect();
        Ok(RawResult { lines, skew: None })
    }
}

/// 엔진이 없을 때 쓰는 오류(화면 문구)
pub fn unavailable() -> Error {
    Error::Unsupported("글자 인식(OCR)을 쓸 수 없어요. Windows 10·11에서 언어 팩의 광학 문자 인식이 설치되어 있어야 해요".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrotate_round_trip() {
        // 300x200 그림의 (10,20,30,40) 상자
        let (bw, bh) = (300.0f32, 200.0f32);
        let r = (10.0, 20.0, 30.0, 40.0);
        // 시계 방향 90으로 돌린 그림(200x300)에서의 상자: (H - y - h, x, h, w)
        let rot90 = (bh - r.1 - r.3, r.0, r.3, r.2);
        assert_eq!(unrotate_rect(90, rot90, bw, bh), r);
        let rot180 = (bw - r.0 - r.2, bh - r.1 - r.3, r.2, r.3);
        assert_eq!(unrotate_rect(180, rot180, bw, bh), r);
        // 270: (y, W - x - w, h, w)
        let rot270 = (r.1, bw - r.0 - r.2, r.3, r.2);
        assert_eq!(unrotate_rect(270, rot270, bw, bh), r);
    }

    #[test]
    fn contrast_stretch_only_when_flat() {
        let img = DynamicImage::ImageLuma8(GrayImage::from_fn(100, 100, |x, _| image::Luma([if x < 50 { 100 } else { 160 }])));
        let g = prepare(&img);
        assert!(g.get_pixel(0, 0)[0] < 10 && g.get_pixel(99, 0)[0] > 245);
        let img = DynamicImage::ImageLuma8(GrayImage::from_fn(100, 100, |x, _| image::Luma([if x < 50 { 0 } else { 255 }])));
        assert_eq!(prepare(&img).get_pixel(0, 0)[0], 0);
    }

    #[test]
    fn scale_limits() {
        assert!((scale_for(20_000, 1000, 10_000) - 0.5).abs() < 1e-3);
        assert!(scale_for(600, 400, 10_000) > 2.0);
        assert_eq!(scale_for(2480, 3508, 10_000), 1.0);
    }

    #[test]
    fn fixed_recognizer_paragraphs() {
        let r = FixedRecognizer {
            lines: vec![
                vec![("가나다".into(), 0.1, 0.10, 0.2, 0.03), ("ABC".into(), 0.35, 0.10, 0.1, 0.03)],
                vec![("둘째".into(), 0.1, 0.14, 0.2, 0.03)],
                vec![("새문단".into(), 0.1, 0.30, 0.2, 0.03)],
            ],
            lang: "ko".into(),
        };
        let img = DynamicImage::ImageLuma8(GrayImage::from_pixel(1000, 1400, image::Luma([255])));
        let out = recognize(&r, &img, None).unwrap();
        assert_eq!(out.rot, 0);
        assert_eq!(out.text, "가나다 ABC\n둘째\n\n새문단");
        assert!((out.lines[0].words[0].x - 100.0).abs() < 1.0);
    }
}
