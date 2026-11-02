//! 0.3.3: 시험용 바꾸기(환경 변수)를 읽을지 정하는 스위치.
//!
//! 배포하는 앱(exe)은 이 스위치를 켜지 않으므로 아래 환경 변수를 **읽지 않는다**:
//! - `WIN_FILETOOL_HIDE_COM=1` — 한글·Microsoft Office가 없는 것처럼(문서 도구 받기 길을 시험)
//! - `WIN_FILETOOL_HIDE_HANCOM=1` — 한글만 없는 것처럼, `WIN_FILETOOL_HIDE_OFFICE=1` — Office만 없는 것처럼
//! - `WIN_FILETOOL_HANCOM_TIMEOUT_SECS`, `WIN_FILETOOL_OFFICE_TIMEOUT_SECS` — 시간 제한 줄이기
//! - `WIN_FILETOOL_DOCTOOLS_URL` — 문서 도구를 받을 주소(로컬 가짜 서버)
//! - 0.3.4 `WIN_FILETOOL_ONEDRIVE_FAKE` — OneDrive 되돌리기 화면의 가짜 환경(JSON)
//! - 0.3.3(보안 강화) 도구·DLL·설정 위치: `WIN_FILETOOL_TOOLS_DIR`, `WIN_FILETOOL_BUNDLE`, `WIN_FILETOOL_PDFIUM`,
//!   `WIN_FILETOOL_CONFIG_DIR`, `WIN_FILETOOL_INDEX_DIR`, `WIN_FILETOOL_LO_PROFILES`
//!   (이 앱을 띄우는 다른 프로그램의 환경이 도구·DLL 위치를 바꾸지 못하게. 휴대용 모드는 exe 옆 `win-filetool-data\`만 쓰고
//!   환경 변수가 필요 없다)
//!
//! 켜는 곳: 점검용 CLI(`filetool-cli`, 배포하지 않음), 시험 기능(`test-hooks`)을 켜고 만든 시험용 앱, 단위 시험.
//! 문서 도구 묶음의 SHA-256은 여기로 바꿀 수 없다(언제나 exe에 고정한 값).

use std::sync::atomic::{AtomicBool, Ordering};

static ON: AtomicBool = AtomicBool::new(false);

/// 시험용 바꾸기를 읽게 한다(CLI·시험용 앱만 부른다)
pub fn enable() {
    ON.store(true, Ordering::SeqCst);
}

pub fn enabled() -> bool {
    cfg!(test) || ON.load(Ordering::SeqCst)
}

/// 켜져 있을 때만 환경 변수 값(빈 값은 없음으로)
pub fn var(name: &str) -> Option<String> {
    if !enabled() {
        return None;
    }
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// 켜져 있을 때만 "1"처럼 켠 값인지(빈 값·"0"은 끔)
pub fn flag(name: &str) -> bool {
    var(name).is_some_and(|v| v.trim() != "0")
}

pub const HIDE_COM: &str = "WIN_FILETOOL_HIDE_COM";
pub const HIDE_OFFICE: &str = "WIN_FILETOOL_HIDE_OFFICE";
pub const DOCTOOLS_URL: &str = "WIN_FILETOOL_DOCTOOLS_URL";
