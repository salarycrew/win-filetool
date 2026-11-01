//! Windows 내장 글자 인식(`Windows.Media.Ocr`, WinRT). `windows` 크레이트(MIT/Apache-2.0)로 부른다.
//! 언어 팩의 "광학 문자 인식"이 설치된 언어만 쓸 수 있다(한국어 Windows는 보통 한국어·영어가 있음).
//! 그림은 Bgra8 SoftwareBitmap으로 넘기고, 긴 변은 `OcrEngine::MaxImageDimension`(보통 10000px)을 넘지 않게 [`super::recognize`]가 줄인다.

use image::GrayImage;
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

use super::{language_hint, OcrStatus, RawResult, Recognizer, Word, KO_HINT};
use crate::error::{Error, Result};

fn werr(what: &str) -> impl Fn(windows::core::Error) -> Error + '_ {
    move |e| Error::Tool { tool: "Windows 글자 인식".into(), message: format!("{what}: {}", e.message()) }
}

/// 설치된 인식 언어 태그
pub fn languages() -> Vec<String> {
    let Ok(list) = OcrEngine::AvailableRecognizerLanguages() else { return vec![] };
    let n = list.Size().unwrap_or(0);
    (0..n).filter_map(|i| list.GetAt(i).ok()).filter_map(|l| l.LanguageTag().ok()).map(|t| t.to_string()).collect()
}

fn max_dim() -> u32 {
    OcrEngine::MaxImageDimension().unwrap_or(4096)
}

pub fn status() -> OcrStatus {
    let langs = languages();
    let korean = langs.iter().any(|l| l.to_ascii_lowercase().starts_with("ko"));
    let using = WinOcr::new(None).ok().map(|e| e.lang);
    let hint = if langs.is_empty() {
        Some(format!("이 PC에는 글자 인식 언어가 없어요. {KO_HINT}"))
    } else if !korean {
        using.as_deref().and_then(language_hint)
    } else {
        None
    };
    OcrStatus { available: using.is_some(), languages: langs, korean, using, max_image_dimension: max_dim(), hint }
}

pub struct WinOcr {
    engine: OcrEngine,
    lang: String,
    max: u32,
}

impl WinOcr {
    /// `lang`: None/"auto" = 한국어 → 사용자 프로필 언어 → 설치된 첫 언어
    pub fn new(lang: Option<&str>) -> Result<Self> {
        let langs = languages();
        if langs.is_empty() {
            return Err(Error::Unsupported(format!("이 PC에는 글자 인식(OCR) 언어가 없어요. {KO_HINT}")));
        }
        let want = lang.map(str::trim).filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("auto"));
        let mut order: Vec<String> = Vec::new();
        if let Some(w) = want {
            order.push(w.to_string());
        }
        if let Some(ko) = langs.iter().find(|l| l.to_ascii_lowercase().starts_with("ko")) {
            order.push(ko.clone());
        }
        for (k, tag) in order.iter().enumerate() {
            let Ok(l) = Language::CreateLanguage(&HSTRING::from(tag.as_str())) else { continue };
            if !OcrEngine::IsLanguageSupported(&l).unwrap_or(false) {
                if k == 0 && want.is_some() {
                    continue;
                }
                continue;
            }
            if let Ok(e) = OcrEngine::TryCreateFromLanguage(&l) {
                let tag = e.RecognizerLanguage().and_then(|l| l.LanguageTag()).map(|t| t.to_string()).unwrap_or_else(|_| tag.clone());
                return Ok(WinOcr { engine: e, lang: tag, max: max_dim() });
            }
        }
        if let Ok(e) = OcrEngine::TryCreateFromUserProfileLanguages() {
            let tag = e.RecognizerLanguage().and_then(|l| l.LanguageTag()).map(|t| t.to_string()).unwrap_or_default();
            return Ok(WinOcr { engine: e, lang: tag, max: max_dim() });
        }
        for tag in &langs {
            let Ok(l) = Language::CreateLanguage(&HSTRING::from(tag.as_str())) else { continue };
            if let Ok(e) = OcrEngine::TryCreateFromLanguage(&l) {
                return Ok(WinOcr { engine: e, lang: tag.clone(), max: max_dim() });
            }
        }
        Err(Error::Unsupported(format!("글자 인식(OCR)을 시작하지 못했어요. {KO_HINT}")))
    }
}

impl Recognizer for WinOcr {
    fn language(&self) -> String {
        self.lang.clone()
    }
    fn max_dim(&self) -> u32 {
        self.max
    }
    fn recognize_raw(&self, img: &GrayImage) -> Result<RawResult> {
        let (w, h) = img.dimensions();
        if w.max(h) > self.max {
            return Err(Error::Invalid(format!("그림이 너무 큽니다({w}x{h}px, 최대 {}px)", self.max)));
        }
        let mut bgra = Vec::with_capacity((w * h * 4) as usize);
        for p in img.pixels() {
            let v = p[0];
            bgra.extend_from_slice(&[v, v, v, 255]);
        }
        let writer = DataWriter::new().map_err(werr("그림 준비"))?;
        writer.WriteBytes(&bgra).map_err(werr("그림 준비"))?;
        drop(bgra);
        let buf = writer.DetachBuffer().map_err(werr("그림 준비"))?;
        let bmp = SoftwareBitmap::CreateCopyFromBuffer(&buf, BitmapPixelFormat::Bgra8, w as i32, h as i32).map_err(werr("그림 준비"))?;
        let res = self.engine.RecognizeAsync(&bmp).map_err(werr("글자 인식"))?.join().map_err(werr("글자 인식"))?;
        let skew = res.TextAngle().ok().and_then(|r| r.Value().ok()).map(|v| v as f32);
        let lines_v = res.Lines().map_err(werr("글자 인식 결과"))?;
        let mut lines = Vec::new();
        for i in 0..lines_v.Size().unwrap_or(0) {
            let Ok(line) = lines_v.GetAt(i) else { continue };
            let text = line.Text().map(|t| t.to_string()).unwrap_or_default();
            let mut words = Vec::new();
            if let Ok(ws) = line.Words() {
                for j in 0..ws.Size().unwrap_or(0) {
                    let Ok(word) = ws.GetAt(j) else { continue };
                    let (Ok(t), Ok(r)) = (word.Text(), word.BoundingRect()) else { continue };
                    words.push(Word { text: t.to_string(), x: r.X, y: r.Y, w: r.Width, h: r.Height });
                }
            }
            lines.push((text, words));
        }
        let _ = bmp.Close();
        Ok(RawResult { lines, skew })
    }
}
