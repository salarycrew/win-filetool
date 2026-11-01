//! PDF → Word(.docx). 글자 PDF는 문단·제목·목록·표·그림으로 다시 만들고(고정 위치 글상자 없음),
//! 스캔본 쪽은 쪽 그림 한 장으로 넣는다(글자 인식 없음). 분석은 [`crate::pdflayout`].
//!
//! - 제목: 본문보다 큰 글자 크기를 순위로 Title/제목 1~3, 굵고 짧은 줄도 제목
//! - 굵게·기울임·하이퍼링크(https/mailto)는 그대로, 글꼴은 [`crate::pdflayout::map_font`] 표로 Windows 글꼴에 맞춤
//! - 머리글·바닥글·쪽 번호는 뺀다. 여러 단은 단 순서대로 한 단 본문으로 흘린다
//! - 암호가 걸린 PDF는 쉬운 말 오류(암호 입력은 PDF 편집 기능 몫)

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::ooxml::docx::{Block as DBlock, Doc, Run};
use crate::paths;
use crate::pdflayout::{self, AnalyzeOpts, Block, DocLayout, Span};
use crate::process::Cancel;

#[derive(Debug, Clone, Default, Serialize)]
pub struct PdfToDocxOut {
    pub input: PathBuf,
    pub output: PathBuf,
    pub pages: usize,
    pub headings: usize,
    pub paragraphs: usize,
    pub list_items: usize,
    pub tables: usize,
    pub images: usize,
    /// 쪽 그림으로 넣은 스캔본 쪽
    pub scanned_pages: usize,
    /// 두 단으로 읽은 쪽
    pub two_column_pages: usize,
    /// 뺀 머리글·바닥글·쪽 번호 줄
    pub dropped_lines: usize,
    pub millis: u64,
    pub note: String,
}

pub(crate) fn runs_of(spans: &[Span]) -> Vec<Run> {
    spans
        .iter()
        .filter(|s| !s.text.is_empty())
        .map(|s| Run { text: s.text.clone(), bold: s.bold, italic: s.italic, link: s.link.clone(), font: pdflayout::map_font(&s.font), size_pt: None })
        .collect()
}

const EMU_PER_PT: f32 = 12_700.0;

/// 분석 결과 → Word 문서
pub fn build_doc(layout: &DocLayout, title: &str) -> Doc {
    let mut doc = Doc::new(title);
    if let Some(p) = layout.pages.first() {
        // 쪽 크기는 첫 쪽(화면 방향). 여백 2cm
        doc.page_w_twips = (p.disp_w * 20.0).round() as i64;
        doc.page_h_twips = (p.disp_h * 20.0).round() as i64;
        doc.margin_twips = 1_134.min(doc.page_w_twips / 6).min(doc.page_h_twips / 6);
    }
    for b in pdflayout::blocks(layout) {
        match b {
            Block::Heading { level, spans } => doc.blocks.push(DBlock::Heading { level, runs: runs_of(&spans) }),
            Block::Para { spans, list: None } => doc.blocks.push(DBlock::Para(runs_of(&spans))),
            Block::Para { spans, list: Some(ordered) } => doc.blocks.push(DBlock::ListItem { runs: runs_of(&spans), level: 0, ordered }),
            Block::Table { rows } => {
                // 첫 줄이 모두 굵으면 머리글 줄
                let header = rows.first().is_some_and(|r| r.iter().flatten().filter(|s| !s.text.trim().is_empty()).all(|s| s.bold) && r.iter().flatten().any(|s| !s.text.trim().is_empty()));
                doc.blocks.push(DBlock::Table { rows: rows.iter().map(|r| r.iter().map(|c| runs_of(c)).collect()).collect(), header });
            }
            Block::Image { bytes, ext, w, h } => doc.blocks.push(DBlock::Image { bytes, name: format!("image.{ext}"), w: (w * EMU_PER_PT) as i64, h: (h * EMU_PER_PT) as i64 }),
            Block::PageImage { jpeg, w, h } => doc.blocks.push(DBlock::Image { bytes: jpeg, name: "page.jpg".into(), w: (w * EMU_PER_PT) as i64, h: (h * EMU_PER_PT) as i64 }),
            Block::PageBreak { .. } => doc.blocks.push(DBlock::PageBreak),
            Block::PageStart { .. } => {}
        }
    }
    doc
}

/// PDF → `이름_Word.docx`
pub fn pdf_to_docx(input: &Path, out_dir: Option<&Path>, cancel: Option<&Cancel>, progress: &mut dyn FnMut(u32, u32)) -> Result<PdfToDocxOut> {
    let t0 = Instant::now();
    if !input.is_file() {
        return Err(Error::Invalid(format!("파일이 없습니다: {}", input.display())));
    }
    let layout = pdflayout::analyze(input, &AnalyzeOpts { images: true, render_scanned: true }, cancel, progress)?;
    let stem = input.file_stem().map(|s| crate::nfc::to_nfc(&s.to_string_lossy())).unwrap_or_else(|| "문서".into());
    let doc = build_doc(&layout, &stem);
    if let Some(c) = cancel {
        c.check()?;
    }
    let output = paths::output_path(input, out_dir, "_Word", "docx");
    doc.save(&output)?;
    let st = doc.count();
    let scanned = layout.pages.iter().filter(|p| p.scanned.is_some()).count();
    let two = layout.pages.iter().filter(|p| p.columns == 2).count();
    let dropped = layout.pages.iter().map(|p| p.dropped).sum();
    let images = st.images.saturating_sub(scanned);
    let mut note = format!("{}쪽 → 제목 {}, 문단 {}, 목록 {}, 표 {}, 그림 {}", layout.pages.len(), st.headings, st.paragraphs, st.list_items, st.tables, images);
    if scanned > 0 {
        note.push_str(&format!(" · 스캔본 {scanned}쪽은 쪽 그림으로 넣었습니다(글자는 고칠 수 없음)"));
    }
    Ok(PdfToDocxOut {
        input: input.into(),
        output,
        pages: layout.pages.len(),
        headings: st.headings,
        paragraphs: st.paragraphs,
        list_items: st.list_items,
        tables: st.tables,
        images,
        scanned_pages: scanned,
        two_column_pages: two,
        dropped_lines: dropped,
        millis: t0.elapsed().as_millis() as u64,
        note,
    })
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PdfToTxtOut {
    pub input: PathBuf,
    pub output: PathBuf,
    pub pages: usize,
    /// 공백을 뺀 글자 수
    pub chars: usize,
    pub scanned_pages: usize,
    pub millis: u64,
    pub note: String,
}

/// PDF → `이름.txt`(UTF-8 BOM, CRLF — 메모장에서 바로 열림). 쪽 사이에 빈 줄 + "— N쪽 —" + 빈 줄.
pub fn pdf_to_txt(input: &Path, out_dir: Option<&Path>, cancel: Option<&Cancel>, progress: &mut dyn FnMut(u32, u32)) -> Result<PdfToTxtOut> {
    let t0 = Instant::now();
    if !input.is_file() {
        return Err(Error::Invalid(format!("파일이 없습니다: {}", input.display())));
    }
    let layout = pdflayout::analyze(input, &AnalyzeOpts { images: false, render_scanned: false }, cancel, progress)?;
    let text = pdflayout::plain_text(&layout);
    let chars = text.chars().filter(|c| !c.is_whitespace()).count();
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(text.replace("\r\n", "\n").replace('\n', "\r\n").as_bytes());
    let output = paths::output_path(input, out_dir, "", "txt");
    paths::write_new(&output, &bytes)?;
    let scanned = layout.pages.iter().filter(|p| p.scanned.is_some()).count();
    let mut note = format!("{}쪽 → 글자 {chars}자", layout.pages.len());
    if scanned > 0 {
        note.push_str(&format!(" · 스캔본 {scanned}쪽은 글자를 읽을 수 없어 비워 두었습니다"));
    }
    Ok(PdfToTxtOut { input: input.into(), output, pages: layout.pages.len(), chars, scanned_pages: scanned, millis: t0.elapsed().as_millis() as u64, note })
}
