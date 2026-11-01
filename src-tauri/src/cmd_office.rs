//! 0.3.0 W2 문서 변환 Tauri 명령. 계약(이름·인자·돌려주는 JSON·진행 이벤트)은 docs/design.md "## 0.3.0 문서 변환(W2)".
//!
//! 모두 `job`(화면이 정한 번호)을 받아 진행률을 "job-progress" `{ job, fraction }`(0~1)으로 보내고,
//! 기존 `cancel_jobs`로 멈출 수 있다. 실패는 한국어 문장(문자열)으로 돌려준다.

use std::path::{Path, PathBuf};

use filetool_core::docx2pptx::{self, DocToPptxOut};
use filetool_core::officeimages::{self, ImagesOut};
use filetool_core::pdf2docx::{self, PdfToDocxOut, PdfToTxtOut};
use filetool_core::pdf2pptx::{self, PdfToPptxOut, PptMode};
use filetool_core::tools::Tool;
use tauri::{AppHandle, Emitter, State};

use crate::{blocking, out_dir, AppState, CmdResult, Progress};

/// (i, n) 진행 → "job-progress" (1% 단위로만 보냄)
fn reporter(app: AppHandle, job: u64) -> impl FnMut(u32, u32) {
    let mut last = -1i64;
    move |i, n| {
        let f = i as f64 / n.max(1) as f64;
        let pct = (f * 100.0) as i64;
        if pct != last {
            last = pct;
            let _ = app.emit("job-progress", Progress { job, fraction: f });
        }
    }
}

/// Word(.docx, 그리고 LibreOffice로 여는 .doc/.hwp/.odt/.pages/.rtf) → `이름_PPT.pptx`
#[tauri::command]
pub async fn doc_to_pptx(app: AppHandle, state: State<'_, AppState>, job: u64, path: String, out_dir: Option<String>) -> CmdResult<DocToPptxOut> {
    let dir = self::out_dir(&out_dir);
    let cancel = state.begin_job(job);
    // 앱에 든(또는 설정에서 고른) LibreOffice — docx면 쓰지 않는다
    let soffice: Option<PathBuf> = state.toolset().get(Tool::Soffice).ok().map(Path::to_path_buf);
    let res = blocking(move || {
        let mut p = reporter(app, job);
        docx2pptx::doc_to_pptx(Path::new(&path), dir.as_deref(), soffice.as_deref(), Some(&cancel), &mut p)
    })
    .await;
    state.end_job(job);
    res
}

/// PDF → `이름_Word.docx`. 0.4 `ocr: true` = 스캔본 쪽을 글자 인식해서 문단으로(기본은 쪽 그림)
#[tauri::command]
pub async fn pdf_to_docx(app: AppHandle, state: State<'_, AppState>, job: u64, path: String, out_dir: Option<String>, ocr: Option<bool>) -> CmdResult<PdfToDocxOut> {
    let dir = self::out_dir(&out_dir);
    let cancel = state.begin_job(job);
    let res = blocking(move || {
        let mut p = reporter(app, job);
        if ocr.unwrap_or(false) {
            let rec = filetool_core::ocr::engine(None)?;
            return pdf2docx::pdf_to_docx_with(Path::new(&path), dir.as_deref(), Some(rec.as_ref()), Some(&cancel), &mut p);
        }
        pdf2docx::pdf_to_docx(Path::new(&path), dir.as_deref(), Some(&cancel), &mut p)
    })
    .await;
    state.end_job(job);
    res
}

/// PDF → `이름_PPT.pptx`. mode: "image"(기본, 그대로 보이게) | "editable"(글자 고칠 수 있게)
#[tauri::command]
pub async fn pdf_to_pptx(app: AppHandle, state: State<'_, AppState>, job: u64, path: String, mode: Option<PptMode>, dpi: Option<f32>, out_dir: Option<String>) -> CmdResult<PdfToPptxOut> {
    let dir = self::out_dir(&out_dir);
    let cancel = state.begin_job(job);
    let res = blocking(move || {
        let mut p = reporter(app, job);
        pdf2pptx::pdf_to_pptx(Path::new(&path), dir.as_deref(), mode.unwrap_or_default(), dpi, Some(&cancel), &mut p)
    })
    .await;
    state.end_job(job);
    res
}

/// PDF → `이름.txt`(UTF-8 BOM)
#[tauri::command]
pub async fn pdf_to_txt(app: AppHandle, state: State<'_, AppState>, job: u64, path: String, out_dir: Option<String>) -> CmdResult<PdfToTxtOut> {
    let dir = self::out_dir(&out_dir);
    let cancel = state.begin_job(job);
    let res = blocking(move || {
        let mut p = reporter(app, job);
        pdf2docx::pdf_to_txt(Path::new(&path), dir.as_deref(), Some(&cancel), &mut p)
    })
    .await;
    state.end_job(job);
    res
}

/// Word·PowerPoint·Excel·PDF 안 그림 → `이름_이미지\`(원본 그대로). 빠르므로 진행률은 시작·끝만.
#[tauri::command]
pub async fn extract_images(app: AppHandle, job: u64, path: String, out_dir: Option<String>) -> CmdResult<ImagesOut> {
    let dir = self::out_dir(&out_dir);
    let _ = app.emit("job-progress", Progress { job, fraction: 0.0 });
    let res = blocking(move || officeimages::extract_images(Path::new(&path), dir.as_deref())).await;
    let _ = app.emit("job-progress", Progress { job, fraction: 1.0 });
    res
}
