//! 0.3.0 W1: 앱 정보(버전)와 오픈소스 고지(THIRD-PARTY-NOTICES.txt).
//!
//! 고지 파일은 빌드 때 `scripts/make-notices.py`가 만든다(cargo 의존성 + ffmpeg·LAME·dav1d·zlib·LibreOffice·PDFium 라이선스 +
//! LGPL/MPL 부품의 "소스 받는 곳"). 찾는 순서: 앱 묶음의 `notices` 항목 → exe 옆 `THIRD-PARTY-NOTICES.txt` → 짧은 안내문.

use std::path::{Path, PathBuf};

use serde::Serialize;

pub const NOTICES_FILE: &str = "THIRD-PARTY-NOTICES.txt";
/// 앱 묶음 안 이름
pub const BUNDLE_ENTRY: &str = "notices";

pub const FALLBACK: &str = "이 앱은 오픈소스 부품을 씁니다: ffmpeg(LGPL-2.1), LAME(LGPL-2.0), dav1d(BSD-2-Clause), zlib, \
LibreOffice(MPL-2.0), PDFium(BSD-3-Clause), Rust 라이브러리(MIT·Apache-2.0 등). \
전체 고지 파일(THIRD-PARTY-NOTICES.txt)을 이 빌드에서 찾지 못했습니다.";

pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[derive(Debug, Clone, Serialize)]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    /// 고지 전문
    pub notices: String,
    /// 고지를 어디서 읽었는지(묶음 안 파일 경로, exe 옆 파일, 또는 None = 짧은 안내문)
    pub notices_path: Option<PathBuf>,
}

fn read(p: &Path) -> Option<String> {
    std::fs::read(p).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// 고지 파일 위치(필요하면 묶음에서 푼다)
pub fn notices_path() -> Option<PathBuf> {
    if let Some(Ok(p)) = crate::bundle::ensure_file(BUNDLE_ENTRY, &[NOTICES_FILE]) {
        if p.is_file() {
            return Some(p);
        }
    }
    let beside = std::env::current_exe().ok()?.parent()?.join(NOTICES_FILE);
    beside.is_file().then_some(beside)
}

pub fn app_info() -> AppInfo {
    let path = notices_path();
    let notices = path.as_deref().and_then(read).unwrap_or_else(|| FALLBACK.to_string());
    AppInfo { name: "파일 도구", version: app_version(), notices, notices_path: path }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_without_bundle_has_fallback() {
        let i = app_info();
        assert_eq!(i.version, env!("CARGO_PKG_VERSION"));
        assert!(!i.notices.is_empty());
    }
}
