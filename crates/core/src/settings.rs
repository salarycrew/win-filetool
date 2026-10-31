//! 앱 설정(도구 경로, 감시 폴더 규칙, 2a 검색)을 JSON 파일로 저장한다. 쓸 때는 임시 파일에 쓴 뒤 바꿔치기한다.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{io_err, Error, Result};
use crate::tools::ToolPaths;
use crate::watch::WatchRule;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub tools: ToolPaths,
    #[serde(default)]
    pub watch_rules: Vec<WatchRule>,
    #[serde(default)]
    pub search: SearchSettings,
    /// ux3: 다시 켤 때 기억할 화면 상태(마지막 탭, 창 위치·크기, 검색 정렬, 열 너비)
    #[serde(default)]
    pub ui: UiState,
}

/// ux3: 화면 상태. 모두 없으면 기본값(검색 탭, 기본 창 크기, 이름 오름차순)
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UiState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowGeom>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_desc: Option<bool>,
    /// 검색 결과 열 너비(px): 이름, 크기, 수정한 날짜(폴더 열은 남는 만큼). 비어 있으면 기본
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub col_widths: Vec<u32>,
}

/// 창 위치(바깥 왼쪽 위)·크기(안쪽), 물리 픽셀
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowGeom {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub maximized: bool,
}

/// 모니터 작업 영역(물리 픽셀)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl WindowGeom {
    /// 지금 연결된 모니터 안으로 맞춘다: 가장 많이 겹치는 모니터(겹치는 곳이 없으면 첫 모니터 = 주 모니터)를 고르고,
    /// 크기는 그 작업 영역보다 크지 않게, 위치는 창 전체가 그 안에 들어오게. 모니터 목록이 비면 그대로.
    pub fn clamp_to(&self, monitors: &[MonitorRect]) -> WindowGeom {
        if monitors.is_empty() {
            return *self;
        }
        let overlap = |m: &MonitorRect| -> i64 {
            let x0 = (self.x as i64).max(m.x as i64);
            let y0 = (self.y as i64).max(m.y as i64);
            let x1 = (self.x as i64 + self.width as i64).min(m.x as i64 + m.width as i64);
            let y1 = (self.y as i64 + self.height as i64).min(m.y as i64 + m.height as i64);
            (x1 - x0).max(0) * (y1 - y0).max(0)
        };
        let best = monitors.iter().max_by_key(|m| overlap(m)).copied().unwrap();
        let (m, centered) = if overlap(&best) > 0 { (best, false) } else { (monitors[0], true) };
        let width = self.width.min(m.width).max(1);
        let height = self.height.min(m.height).max(1);
        let (x, y) = if centered {
            (m.x + (m.width - width) as i32 / 2, m.y + (m.height - height) as i32 / 2)
        } else {
            (
                self.x.clamp(m.x, m.x + (m.width - width) as i32),
                self.y.clamp(m.y, m.y + (m.height - height) as i32),
            )
        };
        WindowGeom { x, y, width, height, maximized: self.maximized }
    }
}

/// 2a 검색 설정
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchSettings {
    /// 색인할 폴더. 비어 있으면 모든 고정 드라이브(C:\, D:\ …)
    #[serde(default)]
    pub roots: Vec<PathBuf>,
    /// 제외 패턴(`\`가 있으면 전체 경로, 없으면 폴더 이름. `*` `?` 가능)
    #[serde(default = "crate::search::scan::default_excludes")]
    pub excludes: Vec<String>,
    /// 전역 단축키(기본 꺼짐)
    #[serde(default)]
    pub hotkey_enabled: bool,
    /// 예: `Ctrl+Alt+F`
    #[serde(default)]
    pub hotkey: String,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self { roots: Vec::new(), excludes: crate::search::scan::default_excludes(), hotkey_enabled: false, hotkey: String::new() }
    }
}

impl SearchSettings {
    /// 실제로 색인할 루트(비어 있으면 고정 드라이브)
    pub fn effective_roots(&self) -> Vec<PathBuf> {
        if self.roots.is_empty() {
            crate::search::scan::fixed_drives()
        } else {
            self.roots.clone()
        }
    }
}

pub const SETTINGS_FILE: &str = "settings.json";

pub fn load_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(io_err(p))?;
    }
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| Error::Invalid(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(io_err(&tmp))?;
    std::fs::rename(&tmp, path).map_err(io_err(path))?;
    Ok(())
}

impl Settings {
    pub fn path_in(dir: &Path) -> PathBuf {
        dir.join(SETTINGS_FILE)
    }
    pub fn load(dir: &Path) -> Self {
        load_json(&Self::path_in(dir))
    }
    pub fn save(&self, dir: &Path) -> Result<()> {
        save_json(&Self::path_in(dir), self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: MonitorRect = MonitorRect { x: 0, y: 0, width: 1920, height: 1040 };
    const RIGHT: MonitorRect = MonitorRect { x: 1920, y: -200, width: 2560, height: 1400 };

    fn g(x: i32, y: i32, w: u32, h: u32) -> WindowGeom {
        WindowGeom { x, y, width: w, height: h, maximized: false }
    }

    #[test]
    fn window_clamped_to_visible_monitor() {
        // 안에 있으면 그대로
        assert_eq!(g(100, 50, 820, 860).clamp_to(&[MAIN, RIGHT]), g(100, 50, 820, 860));
        // 두 번째 모니터에 있으면 그 모니터 기준
        assert_eq!(g(2000, -100, 820, 860).clamp_to(&[MAIN, RIGHT]), g(2000, -100, 820, 860));
        // 오른쪽 모니터를 뺐을 때: 겹치는 곳이 없으면 주 모니터 가운데로
        assert_eq!(g(2000, -100, 820, 860).clamp_to(&[MAIN]), g(550, 90, 820, 860));
        // 조금 걸치면 가장 많이 겹치는 모니터 안으로 밀어 넣음
        assert_eq!(g(1500, 900, 820, 860).clamp_to(&[MAIN]), g(1100, 180, 820, 860));
        assert_eq!(g(-300, -50, 820, 860).clamp_to(&[MAIN]), g(0, 0, 820, 860));
        // 모니터보다 크면 줄임
        assert_eq!(g(10, 10, 3000, 2000).clamp_to(&[MAIN]), g(0, 0, 1920, 1040));
        // 모니터 정보가 없으면 그대로
        assert_eq!(g(5000, 5000, 820, 860).clamp_to(&[]), g(5000, 5000, 820, 860));
        // 최대화 표시는 유지
        let mut m = g(4000, 0, 820, 860);
        m.maximized = true;
        assert!(m.clamp_to(&[MAIN]).maximized);
    }

    #[test]
    fn ui_state_round_trip_and_old_files() {
        // 예전 설정 파일(ui 없음)도 읽힘
        let s: Settings = serde_json::from_str(r#"{ "search": { "roots": ["C:\\"] } }"#).unwrap();
        assert_eq!(s.ui, UiState::default());
        let d = std::env::temp_dir().join(format!("wft-ui-{}", std::process::id()));
        let mut s = Settings::default();
        s.ui = UiState {
            tab: Some("pdf".into()),
            window: Some(g(10, 20, 900, 700)),
            sort_key: Some("size".into()),
            sort_desc: Some(true),
            col_widths: vec![300, 90, 140],
        };
        s.save(&d).unwrap();
        assert_eq!(Settings::load(&d).ui, s.ui);
        let _ = std::fs::remove_dir_all(&d);
    }
}
