//! 출력 경로 도우미. 원본·기존 파일을 덮어쓰지 않는 것이 원칙이다.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::{io_err, Error, Result};

/// `path`가 비어 있으면 그대로, 있으면 `이름 (1).ext`, `이름 (2).ext` … 중 처음 비는 경로.
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().into_owned());
    for n in 1.. {
        let name = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

/// 원본 옆 폴더(또는 지정 폴더)에 `stem + suffix + .ext` 이름을 만들고 겹치지 않게 한다.
pub fn output_path(source: &Path, out_dir: Option<&Path>, suffix: &str, ext: &str) -> PathBuf {
    let dir = out_dir
        .map(Path::to_path_buf)
        .or_else(|| source.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let stem = source
        .file_stem()
        .map(|s| crate::nfc::to_nfc(&s.to_string_lossy()))
        .unwrap_or_else(|| "output".into());
    unique_path(&dir.join(format!("{stem}{suffix}.{ext}")))
}

/// 새 파일로만 쓴다(이미 있으면 오류). 경합 상황에서도 덮어쓰기를 막는다.
pub fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(io_err(parent))?;
        }
    }
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                Error::AlreadyExists(path.to_path_buf())
            } else {
                Error::Io { path: path.to_path_buf(), source: e }
            }
        })?;
    f.write_all(bytes).map_err(io_err(path))?;
    Ok(())
}

/// 0.3.3: 폴더가 있는 드라이브의 남은 공간(바이트). 폴더가 아직 없으면 있는 위쪽 폴더로 본다. 모르면 None.
pub fn free_space(dir: &Path) -> Option<u64> {
    let mut d = dir;
    while !d.exists() {
        d = d.parent()?;
    }
    free_space_imp(d)
}

#[cfg(windows)]
fn free_space_imp(d: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let w: Vec<u16> = d.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut avail = 0u64;
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, std::ptr::null_mut(), std::ptr::null_mut()) } != 0;
    ok.then_some(avail)
}

#[cfg(unix)]
fn free_space_imp(d: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(d.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
}
