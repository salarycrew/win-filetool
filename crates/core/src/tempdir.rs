//! 작업마다 쓰는 임시 폴더(%TEMP%\win-filetool\…). 작업이 끝나면(Drop) 지운다.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{io_err, Result};

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct JobDir(PathBuf);

impl JobDir {
    pub fn new(prefix: &str) -> Result<Self> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir()
            .join("win-filetool")
            .join(format!("{prefix}-{}-{n}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p).map_err(io_err(&p))?;
        Ok(Self(p))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for JobDir {
    fn drop(&mut self) {
        // LibreOffice가 막 끝난 직후에는 파일이 잠깐 잠겨 있을 수 있어 몇 번 다시 시도한다
        for _ in 0..5 {
            if std::fs::remove_dir_all(&self.0).is_ok() || !self.0.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }
}
