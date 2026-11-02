//! 0.3.0 W1: 휴대용 모드와 설정 위치.
//!
//! exe 옆에 `win-filetool-data\` 폴더(또는 `portable.txt` 파일)가 있고 그 폴더에 쓸 수 있으면 설정(`settings.json`)과
//! 감시 상태(`watch-state.json`)를 그 안에 둔다 — USB에 들고 다니면 설정도 따라간다.
//! 검색 색인과 풀어 놓은 도구는 언제나 그 PC의 LOCALAPPDATA에 둔다(PC마다 드라이브가 다르고 크며, USB 쓰기를 줄인다).
//! 표식이 없거나 쓸 수 없으면(읽기 전용 USB 등) 예전처럼 `%APPDATA%\kr.salarycrew.winfiletool`.

use std::path::{Path, PathBuf};

use serde::Serialize;

pub const PORTABLE_DIR: &str = "win-filetool-data";
pub const PORTABLE_MARK: &str = "portable.txt";

/// 설정 폴더가 어디서 왔는지(앱 정보 화면용)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    /// `WIN_FILETOOL_CONFIG_DIR`(시험용 — 점검용 CLI·시험용 앱만 읽음)
    Env,
    /// exe 옆 `win-filetool-data\`
    Portable,
    /// `%APPDATA%`
    Default,
}

/// exe 옆 휴대용 설정 폴더(표식이 있고 쓸 수 있을 때만)
pub fn portable_dir(exe_dir: &Path) -> Option<PathBuf> {
    let d = exe_dir.join(PORTABLE_DIR);
    let marked = d.is_dir() || exe_dir.join(PORTABLE_MARK).is_file();
    (marked && crate::bundle::writable_dir(&d)).then_some(d)
}

/// 설정 폴더 고르기: (시험용) `WIN_FILETOOL_CONFIG_DIR` → 휴대용 → `default`(보통 Tauri의 app_config_dir = %APPDATA%\…).
/// 0.3.3(보안 강화): 휴대용 모드는 exe 옆 `win-filetool-data\`만 쓰고 환경 변수는 필요 없으므로, 배포 앱은 이 변수를 읽지 않는다.
pub fn config_dir(exe_dir: Option<&Path>, default: PathBuf) -> (PathBuf, ConfigSource) {
    if let Some(d) = crate::testhooks::var("WIN_FILETOOL_CONFIG_DIR") {
        return (PathBuf::from(d), ConfigSource::Env);
    }
    if let Some(p) = exe_dir.and_then(portable_dir) {
        return (p, ConfigSource::Portable);
    }
    (default, ConfigSource::Default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_marker_rules() {
        let t = tempfile::tempdir().unwrap();
        let exe = t.path().join("usb");
        std::fs::create_dir_all(&exe).unwrap();
        let def = t.path().join("appdata");
        // 표식 없음 → 기본
        assert_eq!(portable_dir(&exe), None);
        // portable.txt → 폴더를 만들어 씀
        std::fs::write(exe.join(PORTABLE_MARK), b"").unwrap();
        assert_eq!(portable_dir(&exe), Some(exe.join(PORTABLE_DIR)));
        assert!(exe.join(PORTABLE_DIR).is_dir());
        // 폴더만 있어도
        std::fs::remove_file(exe.join(PORTABLE_MARK)).unwrap();
        assert_eq!(portable_dir(&exe), Some(exe.join(PORTABLE_DIR)));
        // 같은 이름의 파일이 막고 있으면(쓸 수 없음) 기본으로
        let exe2 = t.path().join("ro");
        std::fs::create_dir_all(&exe2).unwrap();
        std::fs::write(exe2.join(PORTABLE_DIR), b"not a dir").unwrap();
        std::fs::write(exe2.join(PORTABLE_MARK), b"").unwrap();
        assert_eq!(portable_dir(&exe2), None);
        if crate::testhooks::var("WIN_FILETOOL_CONFIG_DIR").is_none() {
            assert_eq!(config_dir(Some(&exe2), def.clone()), (def.clone(), ConfigSource::Default));
            assert_eq!(config_dir(Some(&exe), def.clone()).1, ConfigSource::Portable);
            assert_eq!(config_dir(None, def.clone()).1, ConfigSource::Default);
        }
    }
}
