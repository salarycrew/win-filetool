//! 앱 설정(도구 경로, 감시 폴더 규칙)을 JSON 파일로 저장한다. 쓸 때는 임시 파일에 쓴 뒤 바꿔치기한다.

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
