//! 윈도 시작 시 실행: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`에 값 하나(관리자 권한 불필요).
//! 켜면 `"<exe 경로>" --autostart`를 적고, 앱은 이 인자로 시작하면 창 없이 트레이에서 감시 폴더를 돌린다.
//! 끄면 값을 지운다. 설정 파일에 따로 적지 않고 레지스트리 값 자체를 상태로 쓴다(앱·CLI가 같은 값을 봄).

use std::path::Path;

use serde::Serialize;

use crate::error::{Error, Result};

pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const VALUE_NAME: &str = "win-filetool";
pub const AUTOSTART_ARG: &str = "--autostart";

/// Run 값: 경로는 따옴표로 감싼다(공백·한글 경로).
pub fn run_value(exe: &Path) -> String {
    format!("\"{}\" {AUTOSTART_ARG}", exe.display())
}

/// Run 값에서 exe 경로를 꺼낸다.
pub fn exe_of_value(v: &str) -> Option<String> {
    let v = v.trim();
    if let Some(rest) = v.strip_prefix('"') {
        rest.find('"').map(|i| rest[..i].to_string())
    } else {
        v.split_whitespace().next().map(str::to_string)
    }
}

/// `--autostart`로 시작했는지. 0.3.2: `--` 뒤(우클릭 메뉴가 넘긴 파일 경로)는 옵션으로 보지 않는다.
pub fn is_autostart_launch<I: IntoIterator<Item = S>, S: AsRef<str>>(args: I) -> bool {
    args.into_iter().take_while(|a| a.as_ref() != "--").any(|a| a.as_ref() == AUTOSTART_ARG)
}

#[derive(Debug, Clone, Serialize)]
pub struct AutostartStatus {
    pub enabled: bool,
    /// 레지스트리에 적힌 값
    pub value: Option<String>,
    /// 적힌 경로가 지금 실행 중인 exe와 같은지(앱을 옮겼으면 false → 다시 켜기)
    pub matches_exe: bool,
    pub supported: bool,
}

pub fn status_from(value: Option<String>, exe: &Path) -> AutostartStatus {
    let matches_exe = value
        .as_deref()
        .and_then(exe_of_value)
        .map(|p| p.eq_ignore_ascii_case(&exe.display().to_string()))
        .unwrap_or(false);
    AutostartStatus { enabled: value.is_some(), value, matches_exe, supported: cfg!(windows) }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
    use winreg::RegKey;

    fn reg_err(e: std::io::Error) -> Error {
        Error::Invalid(format!("레지스트리(HKCU Run)를 바꾸지 못했습니다: {e}"))
    }

    pub fn read() -> Option<String> {
        RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_READ).ok()?.get_value::<String, _>(VALUE_NAME).ok()
    }

    pub fn enable(exe: &Path) -> Result<()> {
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY).map_err(reg_err)?;
        key.set_value(VALUE_NAME, &run_value(exe)).map_err(reg_err)
    }

    pub fn disable() -> Result<()> {
        let key = match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE) {
            Ok(k) => k,
            Err(_) => return Ok(()),
        };
        match key.delete_value(VALUE_NAME) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(reg_err(e)),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub fn read() -> Option<String> {
        None
    }
    pub fn enable(_exe: &Path) -> Result<()> {
        Err(Error::Unsupported("윈도 시작 시 실행은 Windows에서만 됩니다".into()))
    }
    pub fn disable() -> Result<()> {
        Ok(())
    }
}

pub fn status(exe: &Path) -> AutostartStatus {
    status_from(imp::read(), exe)
}

pub fn set(enabled: bool, exe: &Path) -> Result<AutostartStatus> {
    if enabled {
        imp::enable(exe)?;
    } else {
        imp::disable()?;
    }
    Ok(status(exe))
}
