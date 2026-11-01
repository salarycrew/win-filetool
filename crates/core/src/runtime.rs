//! 0.3.0 W1: 실행 환경 확인(WebView2 런타임).
//!
//! 앱 화면은 Microsoft Edge WebView2(Evergreen)로 그린다. Win11에는 기본으로 있고 Win10도 대부분 있지만,
//! 없으면 창을 만들다 앱이 그냥 꺼지므로 창을 열기 전에 레지스트리로 확인한다.
//! 키: `HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-…}`, `HKLM\SOFTWARE\Microsoft\EdgeUpdate\Clients\{…}`,
//! `HKCU\Software\Microsoft\EdgeUpdate\Clients\{…}`의 `pv`(버전). 비어 있거나 `0.0.0.0`이면 없음.

pub const WEBVIEW2_CLIENT: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
/// Evergreen 부트스트래퍼(마이크로소프트 공식 짧은 주소)
pub const WEBVIEW2_DOWNLOAD: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

pub const WEBVIEW2_MISSING_TITLE: &str = "파일 도구";
pub const WEBVIEW2_MISSING_TEXT: &str = "이 앱의 화면을 그리려면 Microsoft Edge WebView2 런타임이 필요한데, 이 PC에서 찾지 못했습니다.\n\n\
[확인]을 누르면 마이크로소프트의 내려받기 주소가 열립니다. 받은 파일(MicrosoftEdgeWebview2Setup.exe)을 실행해 설치한 뒤 \
이 앱을 다시 실행해 주세요. 설치는 1–2분 걸리고 인터넷이 필요합니다.\n\n\
주소: https://go.microsoft.com/fwlink/p/?LinkId=2124703";

/// `pv` 값이 쓸 만한 버전인지
pub fn usable_version(pv: &str) -> bool {
    let v = pv.trim();
    !v.is_empty() && v != "0.0.0.0" && v.split('.').all(|p| p.parse::<u32>().is_ok())
}

/// 설치된 WebView2 버전(없으면 None). Windows가 아니면 언제나 None.
/// 시험용: `WIN_FILETOOL_FAKE_NO_WEBVIEW2=1`이면 없다고 답한다.
pub fn webview2_version() -> Option<String> {
    if std::env::var_os("WIN_FILETOOL_FAKE_NO_WEBVIEW2").is_some_and(|v| v == "1") {
        return None;
    }
    platform::read_pv()
}

#[cfg(windows)]
mod platform {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    pub fn read_pv() -> Option<String> {
        let c = super::WEBVIEW2_CLIENT;
        let tries = [
            (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{c}")),
            (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{c}")),
            (HKEY_CURRENT_USER, format!(r"Software\Microsoft\EdgeUpdate\Clients\{c}")),
        ];
        tries.iter().find_map(|(hive, path)| {
            let k = RegKey::predef(*hive).open_subkey_with_flags(path, KEY_READ).ok()?;
            let pv: String = k.get_value("pv").ok()?;
            super::usable_version(&pv).then_some(pv)
        })
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn read_pv() -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_values() {
        assert!(usable_version("156.0.4314.50"));
        assert!(!usable_version(""));
        assert!(!usable_version("0.0.0.0"));
        assert!(!usable_version("abc"));
        assert!(WEBVIEW2_MISSING_TEXT.contains(WEBVIEW2_DOWNLOAD));
    }
}
