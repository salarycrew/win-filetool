//! 0.4 글자 인식(OCR)·진짜 가리기 Tauri 명령. 계약은 docs/design.md `## 0.4 OCR·가리기`.
//!
//! - 오류는 PDF 편집과 같은 `{ code, message }`(password_required·wrong_password·cancelled·error …)
//! - 오래 걸리는 명령은 `job`으로 "job-progress" `{ job, fraction }`을 보내고 `cancel_jobs`로 멈춘다.
//! - Windows OCR 엔진은 작업 스레드 안에서 만든다(스레드 사이로 넘기지 않음).

use std::path::{Path, PathBuf};

use filetool_core::imgredact::{self, ImgPreview, ImgRedactOpts, ImgRedactOut};
use filetool_core::ocr::pdf::{self as ocrpdf, OcrPdfOpts, OcrPdfOut, TextOut};
use filetool_core::ocr::{self, OcrStatus};
use filetool_core::pdfedit::redact::{self, FindOut, Mark, Preset, RedactOpts, RedactOut};
use filetool_core::pdfedit::{default_output, Job};
use filetool_core::tools::Tool;
use tauri::{AppHandle, Emitter, State};

use crate::cmd_pdfedit::CmdError;
use crate::{AppState, Progress};

type R<T> = Result<T, CmdError>;

fn pw(p: &Option<String>) -> Option<String> {
    p.clone().filter(|s| !s.is_empty())
}

fn dir(d: &Option<String>) -> Option<PathBuf> {
    d.as_ref().filter(|s| !s.is_empty()).map(PathBuf::from)
}

/// 진행률(1% 단위)·취소가 있는 작업
async fn job_work<T, F>(app: AppHandle, state: &State<'_, AppState>, job: u64, f: F) -> R<T>
where
    T: Send + 'static,
    F: FnOnce(&mut Job) -> filetool_core::Result<T> + Send + 'static,
{
    let cancel = state.begin_job(job);
    let res = tauri::async_runtime::spawn_blocking(move || {
        let mut last = -1i64;
        let mut cb = |i: u32, n: u32| {
            let fr = i as f64 / n.max(1) as f64;
            let pct = (fr * 100.0) as i64;
            if pct != last {
                last = pct;
                let _ = app.emit("job-progress", Progress { job, fraction: fr });
            }
        };
        let mut j = Job::new(Some(&cancel), &mut cb);
        f(&mut j)
    })
    .await
    .map_err(|e| CmdError { code: "error", message: format!("작업 스레드 오류: {e}") })?
    .map_err(CmdError::from);
    state.end_job(job);
    res
}

/// 이 PC의 글자 인식 언어(한국어가 없으면 설치 방법 안내)
#[tauri::command]
pub async fn ocr_status() -> R<OcrStatus> {
    tauri::async_runtime::spawn_blocking(ocr::status).await.map_err(|e| CmdError { code: "error", message: e.to_string() })
}

/// 스캔 PDF → `이름_글자인식.pdf`(모양 그대로 + 보이지 않는 글자 층)
#[tauri::command]
pub async fn ocr_pdf(app: AppHandle, state: State<'_, AppState>, job: u64, path: String, password: Option<String>, opts: Option<OcrPdfOpts>, out_dir: Option<String>) -> R<OcrPdfOut> {
    let (pw, out) = (pw(&password), dir(&out_dir));
    job_work(app, &state, job, move |j| {
        let rec = ocr::engine(None)?;
        let input = Path::new(&path);
        let output = default_output(input, out.as_deref(), "_글자인식");
        ocrpdf::make_searchable(input, pw.as_deref(), &opts.unwrap_or_default(), &output, rec.as_ref(), j)
    })
    .await
}

/// 그림(JPG·PNG·HEIC…)·스캔 PDF → `이름_글자.txt` + 화면에서 복사할 글
#[tauri::command]
pub async fn ocr_text(app: AppHandle, state: State<'_, AppState>, job: u64, path: String, password: Option<String>, out_dir: Option<String>) -> R<TextOut> {
    let (pw, out) = (pw(&password), dir(&out_dir));
    let ffmpeg: Option<PathBuf> = state.toolset().get(Tool::Ffmpeg).ok().map(Path::to_path_buf);
    job_work(app, &state, job, move |j| {
        let rec = ocr::engine(None)?;
        ocrpdf::file_to_txt(Path::new(&path), pw.as_deref(), ffmpeg.as_deref(), out.as_deref(), rec.as_ref(), j)
    })
    .await
}

/// 찾아서 가리기: 찾은 곳 목록(아무것도 바꾸지 않음). `ocr`: 글자가 없는 쪽은 글자 인식으로 찾기(기본 켬)
#[tauri::command]
pub async fn redact_find(app: AppHandle, state: State<'_, AppState>, job: u64, path: String, password: Option<String>, terms: Vec<String>, presets: Vec<Preset>, ocr: Option<bool>) -> R<FindOut> {
    let pw = pw(&password);
    job_work(app, &state, job, move |j| {
        let rec = if ocr.unwrap_or(true) { ocr::engine(None).ok() } else { None };
        redact::find(Path::new(&path), pw.as_deref(), &terms, &presets, rec.as_deref(), j)
    })
    .await
}

/// 가리기 → `이름_가림.pdf`(원본 그대로). 결과에 확인(verify) 포함
#[tauri::command]
pub async fn redact_apply(app: AppHandle, state: State<'_, AppState>, job: u64, path: String, password: Option<String>, marks: Vec<Mark>, opts: Option<RedactOpts>, out_dir: Option<String>) -> R<RedactOut> {
    let (pw, out) = (pw(&password), dir(&out_dir));
    job_work(app, &state, job, move |j| {
        let opts = opts.unwrap_or_default();
        let rec = if opts.ocr { ocr::engine(None).ok() } else { None };
        let input = Path::new(&path);
        let output = redact::default_output(input, out.as_deref());
        redact::apply(input, pw.as_deref(), &marks, &opts, &output, rec.as_deref(), j)
    })
    .await
}

/// 사진 가리기 미리보기(방향을 바로잡은 사진, 긴 변 1600px 안) + 원래 크기(px)
#[tauri::command]
pub async fn image_redact_preview(state: State<'_, AppState>, path: String) -> R<ImgPreview> {
    let ffmpeg: Option<PathBuf> = state.toolset().get(Tool::Ffmpeg).ok().map(Path::to_path_buf);
    tauri::async_runtime::spawn_blocking(move || imgredact::preview(Path::new(&path), ffmpeg.as_deref(), 1600))
        .await
        .map_err(|e| CmdError { code: "error", message: e.to_string() })?
        .map_err(CmdError::from)
}

/// 사진 가리기 → `이름_가림.jpg`(PNG는 .png). 상자는 원래 사진 px
#[tauri::command]
pub async fn image_redact(state: State<'_, AppState>, path: String, opts: ImgRedactOpts, out_dir: Option<String>) -> R<ImgRedactOut> {
    let ffmpeg: Option<PathBuf> = state.toolset().get(Tool::Ffmpeg).ok().map(Path::to_path_buf);
    let out = dir(&out_dir);
    tauri::async_runtime::spawn_blocking(move || imgredact::redact(Path::new(&path), ffmpeg.as_deref(), &opts, out.as_deref()))
        .await
        .map_err(|e| CmdError { code: "error", message: e.to_string() })?
        .map_err(CmdError::from)
}
