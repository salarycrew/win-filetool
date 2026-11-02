//! 0.3.0 W1: 실행 환경 명령(앱 정보·오픈소스 고지, 앱에 든 도구 상태·미리 풀기)과 시작 전 WebView2 확인.
//!
//! 화면(UI 작업자)과의 약속은 docs/design.md `## 0.3.0 바탕(W1)`에 적었다.

use std::path::PathBuf;

use filetool_core::portable::ConfigSource;
use filetool_core::{bundle, notices, runtime};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// setup에서 정한 설정 폴더(앱 정보 화면용)
pub struct RuntimeInfo {
    pub config_dir: PathBuf,
    pub config_source: ConfigSource,
}

#[derive(Clone, Serialize)]
pub struct UnpackProgress {
    /// "ffmpeg" | "pdfium" | "lo" | "notices"
    pub tool: String,
    /// 0.0..=1.0
    pub fraction: f64,
}

/// 도구를 푸는 동안 `tools_unpack_progress` 이벤트를 보낸다(문서 첫 변환 때 "문서 도구 준비 중(약 3초)")
pub fn install_progress_events(app: &AppHandle) {
    let app = app.clone();
    bundle::set_progress_hook(move |tool, fraction| {
        let _ = app.emit("tools_unpack_progress", UnpackProgress { tool: tool.to_string(), fraction });
    });
}

#[derive(Serialize)]
pub struct AppInfoOut {
    pub name: &'static str,
    pub version: &'static str,
    pub notices: String,
    pub notices_path: Option<PathBuf>,
    pub config_dir: PathBuf,
    /// "env" | "portable" | "default"
    pub config_source: ConfigSource,
    pub webview2: Option<String>,
    /// 0.3.3: 휴대용 exe로 실행 중인지(설치본은 exe 옆에 tools.pack이 있다) — [설정]의 "우클릭 메뉴·시작 프로그램 지우기"
    pub portable: bool,
}

/// 앱 정보: 버전, 오픈소스 고지 전문, 설정 위치(휴대용 여부), WebView2 버전
#[tauri::command]
pub async fn app_info(info: tauri::State<'_, RuntimeInfo>) -> Result<AppInfoOut, String> {
    let (config_dir, config_source) = (info.config_dir.clone(), info.config_source);
    tauri::async_runtime::spawn_blocking(move || {
        let a = notices::app_info();
        AppInfoOut {
            name: a.name,
            version: a.version,
            notices: a.notices,
            notices_path: a.notices_path,
            config_dir,
            config_source,
            webview2: runtime::webview2_version(),
            portable: !std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("tools.pack").is_file())).unwrap_or(false),
        }
    })
    .await
    .map_err(|e| format!("작업 스레드 오류: {e}"))
}

/// 앱에 든 도구 묶음 상태(어떤 도구가 이미 풀렸는지)
#[tauri::command]
pub async fn tools_bundle_status() -> Result<bundle::BundleStatus, String> {
    tauri::async_runtime::spawn_blocking(bundle::status).await.map_err(|e| format!("작업 스레드 오류: {e}"))
}

/// 도구를 미리 푼다(예: 문서 탭을 열 때 "lo"). 진행은 `tools_unpack_progress` 이벤트. 푼 폴더를 돌려준다.
#[tauri::command]
pub async fn tools_prepare(tool: String) -> Result<PathBuf, String> {
    tauri::async_runtime::spawn_blocking(move || match bundle::current() {
        None => Err("이 앱에는 든 도구가 없습니다(설정의 도구 경로를 씁니다).".to_string()),
        Some(b) => b.ensure(&tool).map_err(|e| e.to_string()),
    })
    .await
    .map_err(|e| format!("작업 스레드 오류: {e}"))?
}

/// 창을 열기 전 WebView2 확인. 없으면 한국어 안내 창을 띄우고(확인 → 내려받기 주소 열기) false.
pub fn ensure_webview2() -> bool {
    if cfg!(not(windows)) || runtime::webview2_version().is_some() {
        return true;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, IDOK, MB_ICONWARNING, MB_OKCANCEL, SW_SHOWNORMAL};
        let w = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (text, title) = (w(runtime::WEBVIEW2_MISSING_TEXT), w(runtime::WEBVIEW2_MISSING_TITLE));
        // 시험용: 창 없이 확인만(자동 점검에서 멈추지 않게)
        if std::env::var_os("WIN_FILETOOL_NO_DIALOG").is_some() {
            eprintln!("{}", runtime::WEBVIEW2_MISSING_TEXT);
            return false;
        }
        let r = unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OKCANCEL | MB_ICONWARNING) };
        if r == IDOK {
            let (op, url) = (w("open"), w(runtime::WEBVIEW2_DOWNLOAD));
            unsafe { ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), url.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
        }
    }
    false
}
