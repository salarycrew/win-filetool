//! 0.3.4 OneDrive가 가져간 폴더 되돌리기 — Tauri 명령. 계약은 docs/design.md `## 0.3.4`.
//!
//! - `onedrive_check` → 진단(읽기만) · `onedrive_restore {folder, allowDownload, job}` → OneDrive 폴더에서 이 PC 폴더로 복사
//! - `onedrive_undo {id}` → 그 복사본 중 바뀌지 않은 것만 지움 · `onedrive_open {what}` → backup(Windows 백업 설정)·web·recycle
//! - 레지스트리(폴더 위치)는 고치지 않고 OneDrive 설정도 바꾸지 않는다. 가짜 환경 변수는 시험용 앱만 읽는다(testhooks).

use filetool_core::onedrive::{self, Env, Folder, Report, RestoreOpts, RestoreOut, UndoOut};
use filetool_core::Error;
use tauri::{AppHandle, State};

use crate::cmd_pdfedit::CmdError;
use crate::cmd_tools::job_work;
use crate::AppState;

type R<T> = Result<T, CmdError>;

fn bad(m: String) -> CmdError {
    CmdError::from(Error::Invalid(m))
}

#[tauri::command]
pub async fn onedrive_check() -> R<Report> {
    tauri::async_runtime::spawn_blocking(|| Env::current().map(|e| onedrive::diagnose(&e)))
        .await
        .map_err(|e| bad(format!("작업 스레드 오류: {e}")))?
        .map_err(CmdError::from)
}

#[tauri::command]
pub async fn onedrive_restore(app: AppHandle, state: State<'_, AppState>, folder: String, allow_download: bool, job: Option<u64>) -> R<RestoreOut> {
    let f = Folder::from_key(&folder).ok_or_else(|| bad(format!("폴더를 알 수 없습니다: {folder}")))?;
    let log_dir = state.config_dir.join("onedrive-restore");
    job_work(app, &state, job, move |cancel, emit| {
        let env = Env::current()?;
        onedrive::restore(&env, f, RestoreOpts { allow_download, dry_run: false }, &log_dir, cancel, &mut |d, t, cur| emit.send(d as f64, t as f64, cur))
    })
    .await
}

#[tauri::command]
pub async fn onedrive_undo(state: State<'_, AppState>, id: String) -> R<UndoOut> {
    let log_dir = state.config_dir.join("onedrive-restore");
    tauri::async_runtime::spawn_blocking(move || Env::current().and_then(|e| onedrive::undo(&e, &log_dir, &id)))
        .await
        .map_err(|e| bad(format!("작업 스레드 오류: {e}")))?
        .map_err(CmdError::from)
}

/// backup: Windows 설정 → Windows 백업(OneDrive 폴더 동기화 → [동기화 설정 관리]) · web: OneDrive 웹 · recycle: 이 PC 휴지통
#[tauri::command]
pub fn onedrive_open(what: String) -> Result<(), String> {
    let target = match what.as_str() {
        "backup" => onedrive::BACKUP_SETTINGS_URI,
        "web" => onedrive::WEB_URL,
        "recycle" => "shell:RecycleBinFolder",
        _ => return Err(format!("알 수 없는 열기: {what}")),
    };
    shell_open(target, what == "recycle")
}

#[cfg(windows)]
fn shell_open(target: &str, explorer: bool) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let w = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (op, file, params) = if explorer { (w("open"), w("explorer.exe"), w(target)) } else { (w("open"), w(target), w("")) };
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), file.as_ptr(), params.as_ptr(), std::ptr::null(), SW_SHOWNORMAL) };
    if (r as isize) <= 32 {
        return Err(format!("열지 못했습니다({})", r as isize));
    }
    Ok(())
}

#[cfg(not(windows))]
fn shell_open(target: &str, _explorer: bool) -> Result<(), String> {
    std::process::Command::new("xdg-open").arg(target).spawn().map(|_| ()).map_err(|e| e.to_string())
}
