//! 0.3.1: 새 버전 알림 명령. 판 비교·24시간 제한은 `filetool_core::update`, 여기는 WinHTTP 요청과 설정 저장만.
//!
//! 앱이 인터넷에 보내는 것은 이 요청 하나뿐이다: `GET https://api.github.com/repos/salarycrew/win-filetool/releases/latest`
//! (User-Agent `win-filetool/<판>`, 쿠키·식별자 없음, 5초 시간 제한). 끄면 [지금 확인]을 누를 때만 묻는다.

use std::time::{SystemTime, UNIX_EPOCH};

use filetool_core::update::{self, Fetch, Outcome};
use serde::Serialize;
use tauri::State;

use crate::AppState;

type CmdResult<T> = Result<T, String>;

const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// 확인할 주소. 시험용 바꾸기(`WIN_FILETOOL_UPDATE_URL`, 예: 로컬 가짜 서버)는 디버그 빌드나 `update-test` 기능으로 만든
/// 시험용 exe에서만 읽는다 — 배포 빌드에는 이 코드가 없다.
fn check_url() -> String {
    #[cfg(any(debug_assertions, feature = "update-test"))]
    {
        if let Some(u) = std::env::var("WIN_FILETOOL_UPDATE_URL").ok().filter(|u| !u.is_empty()) {
            return u;
        }
    }
    update::LATEST_URL.to_string()
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Serialize)]
pub struct UpdateView {
    enabled: bool,
    current: &'static str,
    /// 알림 띠에 보일 새 판(없으면 null)
    banner: Option<BannerInfo>,
}

#[derive(Serialize)]
pub struct BannerInfo {
    version: String,
    url: String,
}

fn view_of(st: &update::UpdateSettings) -> UpdateView {
    UpdateView { enabled: st.enabled, current: CURRENT, banner: update::banner(st, CURRENT).map(|(version, url)| BannerInfo { version, url }) }
}

#[tauri::command]
pub fn update_view(state: State<'_, AppState>) -> UpdateView {
    view_of(&state.settings.lock().unwrap().update)
}

/// 확인하기. `force` = [지금 확인](24시간·켬/끔과 상관없이). 실패는 조용히 `failed`로.
#[tauri::command]
pub async fn update_check(state: State<'_, AppState>, force: bool) -> CmdResult<Outcome> {
    let mut st = state.settings.lock().unwrap().update.clone();
    if !force && (!st.enabled || !update::due(st.last_check, now())) {
        return Ok(Outcome::NotChecked);
    }
    let url = check_url();
    let (out, st) = tauri::async_runtime::spawn_blocking(move || {
        let o = update::check(&WinHttp, &url, CURRENT, &mut st, now(), force);
        (o, st)
    })
    .await
    .map_err(|e| e.to_string())?;
    {
        let mut s = state.settings.lock().unwrap();
        // 기다리는 동안 바뀐 켬/끔·넘긴 판은 그대로 둔다
        s.update.last_check = st.last_check;
        s.update.latest = st.latest;
        s.update.latest_url = st.latest_url;
    }
    let _ = state.save();
    Ok(out)
}

#[tauri::command]
pub fn update_set_enabled(state: State<'_, AppState>, enabled: bool) -> CmdResult<UpdateView> {
    let v = {
        let mut s = state.settings.lock().unwrap();
        s.update.enabled = enabled;
        view_of(&s.update)
    };
    state.save()?;
    Ok(v)
}

/// [이 버전 넘기기]
#[tauri::command]
pub fn update_skip(state: State<'_, AppState>, version: String) -> CmdResult<UpdateView> {
    let v = {
        let mut s = state.settings.lock().unwrap();
        s.update.skipped = Some(version);
        view_of(&s.update)
    };
    state.save()?;
    Ok(v)
}

/// [받으러 가기]: 알아 둔 릴리스 쪽(이 저장소 주소만)을 기본 브라우저로 연다
#[tauri::command]
pub fn update_open(state: State<'_, AppState>) -> CmdResult<()> {
    let url = {
        let s = state.settings.lock().unwrap();
        s.update.latest_url.clone().filter(|u| update::safe_release_url(u)).unwrap_or_else(|| update::RELEASES_LATEST_PAGE.to_string())
    };
    open_url(&url)
}

#[cfg(windows)]
fn open_url(url: &str) -> CmdResult<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let w = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (op, file) = (w("open"), w(url));
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), op.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    if (r as isize) <= 32 {
        return Err(format!("브라우저를 열지 못했습니다({})", r as isize));
    }
    Ok(())
}

#[cfg(not(windows))]
fn open_url(url: &str) -> CmdResult<()> {
    std::process::Command::new("xdg-open").arg(url).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// WinHTTP로 GET 한 번(시스템 프록시 설정을 따름). 5초 시간 제한, 2 MB 상한, 상태 200만 성공.
struct WinHttp;

#[cfg(windows)]
impl Fetch for WinHttp {
    fn get(&self, url: &str) -> Result<String, String> {
        use std::ffi::c_void;
        use windows_sys::Win32::Networking::WinHttp::*;
        let (secure, host, port, path) = update::split_url(url).ok_or("주소가 올바르지 않습니다")?;
        let w = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        struct H(*mut c_void);
        impl Drop for H {
            fn drop(&mut self) {
                if !self.0.is_null() {
                    unsafe { WinHttpCloseHandle(self.0) };
                }
            }
        }
        let agent = w(&format!("win-filetool/{CURRENT}"));
        unsafe {
            let ses = H(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0));
            if ses.0.is_null() {
                return Err("WinHttpOpen".into());
            }
            WinHttpSetTimeouts(ses.0, 5000, 5000, 5000, 5000);
            let hw = w(&host);
            let con = H(WinHttpConnect(ses.0, hw.as_ptr(), port, 0));
            if con.0.is_null() {
                return Err("WinHttpConnect".into());
            }
            let (verb, pw) = (w("GET"), w(&path));
            let req = H(WinHttpOpenRequest(con.0, verb.as_ptr(), pw.as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null(), if secure { WINHTTP_FLAG_SECURE } else { 0 }));
            if req.0.is_null() {
                return Err("WinHttpOpenRequest".into());
            }
            let hdr = w("Accept: application/vnd.github+json\r\n");
            if WinHttpSendRequest(req.0, hdr.as_ptr(), u32::MAX, std::ptr::null(), 0, 0, 0) == 0 || WinHttpReceiveResponse(req.0, std::ptr::null_mut()) == 0 {
                return Err("연결하지 못했습니다".into());
            }
            let mut code: u32 = 0;
            let mut len = std::mem::size_of::<u32>() as u32;
            if WinHttpQueryHeaders(req.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, std::ptr::null(), &mut code as *mut u32 as *mut c_void, &mut len, std::ptr::null_mut()) == 0 {
                return Err("응답 상태를 읽지 못했습니다".into());
            }
            if code != 200 {
                return Err(format!("HTTP {code}"));
            }
            let mut body = Vec::new();
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                let mut n: u32 = 0;
                if WinHttpReadData(req.0, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut n) == 0 {
                    return Err("응답을 읽지 못했습니다".into());
                }
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&buf[..n as usize]);
                if body.len() > update::MAX_BODY {
                    return Err("응답이 너무 큽니다".into());
                }
            }
            String::from_utf8(body).map_err(|_| "응답이 UTF-8이 아닙니다".into())
        }
    }
}

#[cfg(not(windows))]
impl Fetch for WinHttp {
    fn get(&self, _url: &str) -> Result<String, String> {
        Err("Windows에서만 확인합니다".into())
    }
}
