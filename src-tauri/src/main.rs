// 릴리스 빌드에서는 콘솔 창 없이 GUI로만 뜬다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! 파일 도구(Tauri 2). 실제 작업은 `filetool-core`가 하고, 여기서는
//! 화면이 멈추지 않도록 작업을 별도 스레드(spawn_blocking)에서 돌린 뒤 결과만 돌려준다.
//! 오류는 코어의 한국어 메시지를 그대로 문자열로 넘긴다.

use std::path::{Path, PathBuf};

use filetool_core::pdf::{self, SplitMode};
use filetool_core::{nfc, paths, zipnfc};
use serde::Serialize;

type CmdResult<T> = Result<T, String>;

/// 무거운 작업을 작업 스레드에서 실행한다(창이 멈추지 않게).
async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> filetool_core::Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("작업 스레드 오류: {e}"))?
        .map_err(|e| e.to_string())
}

fn out_dir(dir: &Option<String>) -> Option<PathBuf> {
    dir.as_ref().filter(|s| !s.is_empty()).map(PathBuf::from)
}

#[derive(Serialize)]
struct Item {
    path: PathBuf,
    kind: &'static str,
    pages: Option<u32>,
}

#[tauri::command]
async fn inspect_paths(paths: Vec<String>) -> CmdResult<Vec<Item>> {
    blocking(move || {
        Ok(paths
            .into_iter()
            .map(|p| {
                let path = PathBuf::from(p);
                let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
                let (kind, pages) = if path.is_dir() {
                    ("dir", None)
                } else if !path.exists() {
                    ("missing", None)
                } else {
                    match ext.as_deref() {
                        Some("pdf") => ("pdf", pdf::page_count(&path).ok()),
                        Some("zip") => ("zip", None),
                        _ => ("other", None),
                    }
                };
                Item { path, kind, pages }
            })
            .collect())
    })
    .await
}

#[tauri::command]
async fn nfc_preview(folder: String) -> CmdResult<nfc::FolderPlan> {
    blocking(move || nfc::plan_folder(Path::new(&folder))).await
}

#[tauri::command]
async fn nfc_apply(folder: String) -> CmdResult<nfc::ApplyReport> {
    blocking(move || nfc::normalize_folder(Path::new(&folder))).await
}

#[derive(Serialize)]
struct ZipResult {
    extract: zipnfc::ExtractReport,
    rezip: Option<zipnfc::RezipReport>,
}

#[tauri::command]
async fn zip_extract(zip: String, out_dir: Option<String>, rezip: bool) -> CmdResult<ZipResult> {
    let dir = self::out_dir(&out_dir);
    blocking(move || {
        let zip = PathBuf::from(zip);
        let target = match &dir {
            Some(d) => {
                let stem = zip.file_stem().map(|s| nfc::to_nfc(&s.to_string_lossy())).unwrap_or_default();
                paths::unique_path(&d.join(stem))
            }
            None => zipnfc::default_extract_dir(&zip),
        };
        let extract = zipnfc::extract_nfc(&zip, &target)?;
        let rezip = if rezip {
            Some(zipnfc::rezip_nfc(&zip, &zipnfc::default_rezip_path(&zip, dir.as_deref()))?)
        } else {
            None
        };
        Ok(ZipResult { extract, rezip })
    })
    .await
}

#[derive(Serialize)]
struct PdfOut {
    output: PathBuf,
    pages: u32,
}

#[tauri::command]
async fn pdf_merge(paths: Vec<String>, out_dir: Option<String>) -> CmdResult<PdfOut> {
    let dir = self::out_dir(&out_dir);
    blocking(move || {
        let inputs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        let first = inputs.first().cloned().unwrap_or_default();
        let output = paths::output_path(&first, dir.as_deref(), "_합침", "pdf");
        let pages = pdf::merge(&inputs, &output)?;
        Ok(PdfOut { output, pages })
    })
    .await
}

#[tauri::command]
async fn pdf_split(path: String, mode: SplitMode, out_dir: Option<String>) -> CmdResult<Vec<PathBuf>> {
    let dir = self::out_dir(&out_dir);
    blocking(move || pdf::split(Path::new(&path), &mode, dir.as_deref())).await
}

#[tauri::command]
async fn pdf_rotate(path: String, pages: String, degrees: i64, out_dir: Option<String>) -> CmdResult<PathBuf> {
    let dir = self::out_dir(&out_dir);
    blocking(move || {
        let src = PathBuf::from(path);
        let output = paths::output_path(&src, dir.as_deref(), &format!("_회전{degrees}"), "pdf");
        pdf::rotate(&src, &pages, degrees, &output)?;
        Ok(output)
    })
    .await
}

#[tauri::command]
async fn pdf_delete(path: String, pages: String, out_dir: Option<String>) -> CmdResult<PdfOut> {
    let dir = self::out_dir(&out_dir);
    blocking(move || {
        let src = PathBuf::from(path);
        let output = paths::output_path(&src, dir.as_deref(), "_페이지삭제", "pdf");
        let pages = pdf::delete_pages(&src, &pages, &output)?;
        Ok(PdfOut { output, pages })
    })
    .await
}

#[tauri::command]
async fn pdf_reorder(path: String, order: String, out_dir: Option<String>) -> CmdResult<PathBuf> {
    let dir = self::out_dir(&out_dir);
    blocking(move || {
        let src = PathBuf::from(path);
        let output = paths::output_path(&src, dir.as_deref(), "_순서변경", "pdf");
        pdf::reorder(&src, &order, &output)?;
        Ok(output)
    })
    .await
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            inspect_paths,
            nfc_preview,
            nfc_apply,
            zip_extract,
            pdf_merge,
            pdf_split,
            pdf_rotate,
            pdf_delete,
            pdf_reorder
        ])
        .run(tauri::generate_context!())
        .expect("파일 도구를 시작하지 못했습니다");
}
