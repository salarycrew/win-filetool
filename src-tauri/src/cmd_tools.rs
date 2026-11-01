//! 0.3.0 W4 이미지·영상·파일 정리 Tauri 명령. 계약(이름·인자·반환·이벤트)은 docs/design.md `## 0.3.0 이미지·영상·파일 정리 (W4)`.
//!
//! - 오류는 `{ code, message }` JSON: code = `cancelled` | `tool_missing` | `error` (+ PDF 편집과 같은 `password_*`는 쓰지 않음)
//! - 여러 파일·오래 걸리는 작업은 `job`(화면이 정한 번호)을 주면 "tools_progress" {job, done, total, current} 이벤트를 보내고
//!   `cancel_job {job}`(또는 예전 `cancel_jobs`)으로 멈춘다.
//! - 결과는 원본 옆(또는 `outDir`)의 새 파일(`이름_편집.확장자` 등, 있으면 ` (1)`). 원본은 그대로.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use filetool_core::dupes::{self, DupGroup, ScanOpts, ScanOut, TrashOut};
use filetool_core::imgedit::{self, EditOps, ImageProbe};
use filetool_core::mediaedit::{self, MediaEditOut, MediaOps, MediaProbe};
use filetool_core::process::Cancel;
use filetool_core::rename::{self, ApplyOut, Pair, PreviewItem, Rule, SortBy, UndoOut};
use filetool_core::tools::Tool;
use filetool_core::ziptool::{self, CreateOut, ExtractOut, NameEnc, ZipListing};
use filetool_core::Error;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::cmd_pdfedit::CmdError;
use crate::AppState;

type R<T> = Result<T, CmdError>;

fn err(e: Error) -> CmdError {
    match &e {
        Error::ToolMissing { .. } => CmdError { code: "tool_missing", message: e.to_string() },
        _ => CmdError::from(e),
    }
}

fn dir(d: &Option<String>) -> Option<PathBuf> {
    d.as_ref().filter(|s| !s.is_empty()).map(PathBuf::from)
}

async fn work<T, F>(f: F) -> R<T>
where
    T: Send + 'static,
    F: FnOnce() -> filetool_core::Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| CmdError { code: "error", message: format!("작업 스레드 오류: {e}") })?
        .map_err(err)
}

#[derive(Clone, Serialize)]
pub struct ToolsProgress {
    job: u64,
    done: f64,
    total: f64,
    current: String,
}

/// 진행 보내기(같은 값은 다시 보내지 않음)
struct Emit {
    app: AppHandle,
    job: Option<u64>,
    last: (i64, String),
}

impl Emit {
    fn send(&mut self, done: f64, total: f64, current: &str) {
        let Some(job) = self.job else { return };
        let pct = if total > 0.0 { (done / total * 200.0) as i64 } else { done as i64 };
        if (pct, current) == (self.last.0, self.last.1.as_str()) {
            return;
        }
        self.last = (pct, current.to_string());
        let _ = self.app.emit("tools_progress", ToolsProgress { job, done, total, current: current.to_string() });
    }
}

/// job이 있으면 취소 표지를 등록하고, 끝나면 지운다.
async fn job_work<T, F>(app: AppHandle, state: &State<'_, AppState>, job: Option<u64>, f: F) -> R<T>
where
    T: Send + 'static,
    F: FnOnce(&Cancel, &mut Emit) -> filetool_core::Result<T> + Send + 'static,
{
    let cancel = match job {
        Some(j) => state.begin_job(j),
        None => Cancel::new(),
    };
    let mut emit = Emit { app, job, last: (-1, String::new()) };
    let res = work(move || f(&cancel, &mut emit)).await;
    if let Some(j) = job {
        state.end_job(j);
    }
    res
}

/// 작업 하나만 멈추기
#[tauri::command]
pub fn cancel_job(state: State<'_, AppState>, job: u64) -> bool {
    match state.jobs.lock().unwrap().get(&job) {
        Some(c) => {
            c.cancel();
            true
        }
        None => false,
    }
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

// ---------- 사진 ----------

#[tauri::command]
pub async fn image_probe(path: String) -> R<ImageProbe> {
    work(move || imgedit::probe(Path::new(&path))).await
}

#[derive(Serialize)]
pub struct ImageEditItem {
    input: PathBuf,
    output: Option<PathBuf>,
    before: u64,
    after: u64,
    width: u32,
    height: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    /// 이 파일만 실패한 까닭(나머지는 계속)
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// 사진 여러 장 고치기. 한 장이 실패해도 나머지는 계속한다(그 항목의 `error`).
#[tauri::command]
pub async fn image_edit(app: AppHandle, state: State<'_, AppState>, inputs: Vec<String>, ops: EditOps, out_dir: Option<String>, job: Option<u64>) -> R<Vec<ImageEditItem>> {
    let d = dir(&out_dir);
    let ts = state.toolset();
    job_work(app, &state, job, move |cancel, emit| {
        let ffmpeg = ts.ffmpeg.clone();
        let n = inputs.len() as f64;
        let mut out = Vec::new();
        for (i, p) in inputs.iter().enumerate() {
            cancel.check()?;
            emit.send(i as f64, n, p);
            let src = PathBuf::from(p);
            match imgedit::edit(ffmpeg.as_deref(), &src, &ops, d.as_deref(), Some(cancel)) {
                Ok(o) => out.push(ImageEditItem { input: o.input, output: Some(o.output), before: o.before, after: o.after, width: o.width, height: o.height, note: o.note, error: None }),
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(e) => {
                    let before = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
                    out.push(ImageEditItem { input: src, output: None, before, after: 0, width: 0, height: 0, note: None, error: Some(e.to_string()) })
                }
            }
        }
        emit.send(n, n, "");
        Ok(out)
    })
    .await
}

// ---------- 영상 ----------

#[tauri::command]
pub async fn media_probe(state: State<'_, AppState>, path: String) -> R<MediaProbe> {
    let ts = state.toolset();
    work(move || mediaedit::probe(ts.get(Tool::Ffprobe)?, Path::new(&path))).await
}

/// 미리보기용 화면 한 장: JPG 바이트를 base64 글자로(앞에 `data:image/jpeg;base64,`를 붙여 <img>에 씀)
#[tauri::command]
pub async fn media_frame(state: State<'_, AppState>, path: String, at: f64, width: Option<u32>) -> R<String> {
    let ts = state.toolset();
    work(move || {
        let p = Path::new(&path);
        let dur = ts.ffprobe.as_deref().and_then(|fp| mediaedit::probe(fp, p).ok()).map(|i| i.duration);
        let jpg = mediaedit::frame_jpeg(ts.get(Tool::Ffmpeg)?, p, at, width, dur, None)?;
        Ok(base64(&jpg))
    })
    .await
}

#[tauri::command]
pub async fn media_frame_save(state: State<'_, AppState>, path: String, at: f64, out_dir: Option<String>) -> R<PathBuf> {
    let ts = state.toolset();
    let d = dir(&out_dir);
    work(move || mediaedit::frame_save(ts.get(Tool::Ffmpeg)?, ts.ffprobe.as_deref(), Path::new(&path), at, d.as_deref(), None)).await
}

#[tauri::command]
pub async fn media_edit(app: AppHandle, state: State<'_, AppState>, input: String, ops: MediaOps, out_dir: Option<String>, job: Option<u64>) -> R<MediaEditOut> {
    let ts = state.toolset();
    let d = dir(&out_dir);
    job_work(app, &state, job, move |cancel, emit| {
        let (ff, fp) = (ts.get(Tool::Ffmpeg)?.to_path_buf(), ts.get(Tool::Ffprobe)?.to_path_buf());
        mediaedit::edit(&ff, &fp, Path::new(&input), &ops, d.as_deref(), Some(cancel), &mut |f| emit.send(f, 1.0, &input))
    })
    .await
}

// ---------- zip ----------

#[tauri::command]
pub async fn zip_list(path: String, encoding: Option<NameEnc>) -> R<ZipListing> {
    work(move || ziptool::list(Path::new(&path), encoding.unwrap_or_default())).await
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum ZipExtractResult {
    New(ExtractOut),
    /// 예전 화면(`zip`, `rezip` 인자) 그대로
    Legacy(crate::ZipResult),
}

/// zip 풀기. 새 계약: `{path, encoding, outDir?, job?}` → `{folder, count, skipped}`.
/// 예전 화면이 쓰던 `{zip, outDir, rezip}`도 받아 예전 결과를 돌려준다(W5 화면으로 바뀌면 안 씀).
#[tauri::command]
pub async fn zip_extract(
    app: AppHandle,
    state: State<'_, AppState>,
    path: Option<String>,
    encoding: Option<NameEnc>,
    out_dir: Option<String>,
    job: Option<u64>,
    zip: Option<String>,
    rezip: Option<bool>,
) -> R<ZipExtractResult> {
    let d = dir(&out_dir);
    if let (None, Some(z)) = (&path, zip) {
        return work(move || crate::zip_extract_legacy(PathBuf::from(z), d, rezip.unwrap_or(false))).await.map(ZipExtractResult::Legacy);
    }
    let path = path.ok_or_else(|| CmdError { code: "error", message: "풀 zip 파일을 고르세요".into() })?;
    job_work(app, &state, job, move |cancel, emit| {
        ziptool::extract(Path::new(&path), encoding.unwrap_or_default(), d.as_deref(), Some(cancel), &mut |i, n, cur| emit.send(i as f64, n as f64, cur))
    })
    .await
    .map(ZipExtractResult::New)
}

#[tauri::command]
pub async fn zip_create(app: AppHandle, state: State<'_, AppState>, inputs: Vec<String>, output: Option<String>, job: Option<u64>) -> R<CreateOut> {
    let out = dir(&output);
    job_work(app, &state, job, move |cancel, emit| {
        let inputs: Vec<PathBuf> = inputs.into_iter().map(PathBuf::from).collect();
        ziptool::create(&inputs, out.as_deref(), Some(cancel), &mut |i, n, cur| emit.send(i as f64, n as f64, cur))
    })
    .await
}

// ---------- 이름 바꾸기 ----------

#[tauri::command]
pub async fn rename_preview(files: Vec<String>, rule: Rule, sort: Option<SortBy>) -> R<Vec<PreviewItem>> {
    work(move || {
        let files: Vec<PathBuf> = files.into_iter().map(PathBuf::from).collect();
        rename::preview(&files, &rule, sort.unwrap_or_default())
    })
    .await
}

#[tauri::command]
pub async fn rename_apply(state: State<'_, AppState>, pairs: Vec<Pair>) -> R<ApplyOut> {
    let undo_dir = state.config_dir.clone();
    work(move || rename::apply(&pairs, &undo_dir)).await
}

#[tauri::command]
pub async fn rename_undo(state: State<'_, AppState>, undo_id: String) -> R<UndoOut> {
    let undo_dir = state.config_dir.clone();
    work(move || rename::undo(&undo_id, &undo_dir)).await
}

// ---------- 같은 파일 ----------

/// 마지막 찾기 결과(휴지통으로 보낼 때 "모두 지우기"를 막는 데 쓴다)
static LAST_SCAN: Mutex<Vec<DupGroup>> = Mutex::new(Vec::new());

#[tauri::command]
pub async fn dup_scan(app: AppHandle, state: State<'_, AppState>, roots: Vec<String>, min_size: Option<u64>, include_hidden: bool, job: Option<u64>) -> R<ScanOut> {
    let r = job_work(app, &state, job, move |cancel, emit| {
        let roots: Vec<PathBuf> = roots.into_iter().map(PathBuf::from).collect();
        let opts = ScanOpts { min_size: min_size.unwrap_or(1), include_hidden };
        dupes::scan(&roots, &opts, Some(cancel), &mut |_stage, done, total, cur| emit.send(done as f64, total as f64, &path_str(cur)))
    })
    .await?;
    *LAST_SCAN.lock().unwrap() = r.groups.clone();
    Ok(r)
}

/// 휴지통으로만 보낸다. 마지막 찾기 결과의 묶음을 한 벌도 남기지 않게 되면 거절.
#[tauri::command]
pub async fn dup_trash(paths: Vec<String>) -> R<TrashOut> {
    let groups = LAST_SCAN.lock().unwrap().clone();
    if groups.is_empty() {
        return Err(CmdError { code: "error", message: "먼저 같은 파일 찾기를 해 주세요".into() });
    }
    let r = work(move || {
        let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        dupes::trash(&paths, &groups, &mut |p| dupes::recycle(p))
    })
    .await?;
    Ok(r)
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}
