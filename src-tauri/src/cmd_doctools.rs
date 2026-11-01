//! 0.3.3 문서 도구 명령: 상태(한글·Office·문서 도구), 받기(진행·멈추기·이어받기), 파일로 넣기, 지우기.
//!
//! 인터넷에 보내는 것: [받기]를 누르거나 "받을까요?"에 [받기]를 눌렀을 때만 `GET` 한 번(+ 끊겼으면 이어받기 `Range`)
//! — 주소는 공개 저장소 릴리스의 고정 주소(`doctools::URL`), User-Agent `win-filetool/<판>`, 쿠키·식별자 없음.
//! 받은 파일은 exe에 고정한 SHA-256과 같을 때만 푼다. 시험용 주소 바꾸기는 `test-hooks` 기능으로 만든 시험용 exe만.

use std::path::PathBuf;

use filetool_core::docs::{DocTarget, Engines};
use filetool_core::{doctools, msoffice};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::AppState;

type CmdResult<T> = Result<T, String>;

#[derive(Serialize)]
pub struct DocToolsView {
    #[serde(flatten)]
    status: doctools::Status,
    engines: Engines,
    office: msoffice::Presence,
}

#[tauri::command]
pub async fn doctools_status() -> CmdResult<DocToolsView> {
    tauri::async_runtime::spawn_blocking(|| DocToolsView { status: doctools::status(), engines: Engines::detect(), office: msoffice::presence() })
        .await
        .map_err(|e| e.to_string())
}

/// 이 파일들을 바꾸기 전에 문서 도구를 받아야 하는지(화면이 미리 물을 때)
#[tauri::command]
pub async fn doctools_needed(state: State<'_, AppState>, paths: Vec<String>, target: DocTarget) -> CmdResult<bool> {
    let ready = state.toolset().soffice.is_some();
    tauri::async_runtime::spawn_blocking(move || {
        let ps: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        filetool_core::docs::needs_doctools(&ps, target, &Engines::detect(), ready)
    })
    .await
    .map_err(|e| e.to_string())
}

#[derive(Clone, Serialize)]
struct DlProgress {
    job: u64,
    done: u64,
    total: u64,
}

/// [받기]: 받기 → SHA-256 확인 → 풀기. 진행은 `doctools-progress`, 멈추기는 `cancel_jobs`(받던 파일은 남겨 다음에 이어받기).
#[tauri::command]
pub async fn doctools_download(app: AppHandle, state: State<'_, AppState>, job: u64) -> CmdResult<PathBuf> {
    let cancel = state.begin_job(job);
    let res = tauri::async_runtime::spawn_blocking(move || {
        let fetch = doctools::WinHttpFetch { user_agent: format!("win-filetool/{}", env!("CARGO_PKG_VERSION")) };
        doctools::fetch_and_install(&fetch, Some(&cancel), &mut |done, total| {
            let _ = app.emit("doctools-progress", DlProgress { job, done, total });
        })
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r.map_err(|e| e.to_string()));
    state.end_job(job);
    res
}

/// [파일로 넣기](인터넷 없는 PC): 고른 파일을 확인해서 푼다. 고른 파일은 지우지 않는다.
#[tauri::command]
pub async fn doctools_install_file(path: String) -> CmdResult<PathBuf> {
    tauri::async_runtime::spawn_blocking(move || doctools::install_file(std::path::Path::new(&path), None))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// [지우기]
#[tauri::command]
pub async fn doctools_remove() -> CmdResult<DocToolsView> {
    tauri::async_runtime::spawn_blocking(|| doctools::remove(&filetool_core::bundle::tools_root()))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    doctools_status().await
}
