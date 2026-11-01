//! 0.3.0 W4 같은 파일 찾기(내용이 같은 파일)와 휴지통으로 보내기.
//!
//! 찾기: 크기가 같은 것끼리 → 앞·뒤 64KiB 해시가 같은 것끼리 → 전체 SHA-256이 같은 것끼리 묶는다.
//! 빈 파일(0바이트)·바로 가기(링크)는 보지 않는다. 숨김·시스템 파일은 `include_hidden`일 때만.
//!
//! 지우기: **휴지통으로만** 보낸다(완전히 지우지 않음). 묶음의 모든 파일을 지우게 되는 요청은 통째로 거절한다.
//! 보내기 직전에 내용(크기·해시)이 찾을 때와 같은지 다시 확인하고, 남길 파일이 아직 있는지도 확인한다.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{io_err, Error, Result};
use crate::process::Cancel;

pub const EDGE: u64 = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DupFile {
    pub path: PathBuf,
    /// 수정 시각(유닉스 밀리초)
    pub modified: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DupGroup {
    pub size: u64,
    /// SHA-256(16진수)
    pub hash: String,
    pub files: Vec<DupFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScanOut {
    pub groups: Vec<DupGroup>,
    /// 살펴본 파일 수
    pub scanned: usize,
    /// 한 벌만 남기면 비울 수 있는 바이트
    pub wasted_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct ScanOpts {
    pub min_size: u64,
    pub include_hidden: bool,
}

/// 진행: (단계 "list"|"hash", 한 것, 전체, 지금 파일)
pub type Progress<'a> = &'a mut dyn FnMut(&str, usize, usize, &Path);

fn is_hidden(e: &walkdir::DirEntry) -> bool {
    let name = e.file_name().to_string_lossy();
    if e.depth() > 0 && name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if e.depth() > 0 {
            if let Ok(m) = e.metadata() {
                // FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM
                if m.file_attributes() & 0x6 != 0 {
                    return true;
                }
            }
        }
    }
    let _ = &name;
    false
}

fn mtime_ms(m: &std::fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 앞·뒤 64KiB 해시(작은 파일은 전체)
pub fn edge_hash(path: &Path, size: u64) -> std::io::Result<[u8; 32]> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    if size <= 2 * EDGE {
        let mut b = Vec::with_capacity(size as usize);
        f.read_to_end(&mut b)?;
        h.update(&b);
    } else {
        let mut b = vec![0u8; EDGE as usize];
        f.read_exact(&mut b)?;
        h.update(&b);
        f.seek(SeekFrom::Start(size - EDGE))?;
        f.read_exact(&mut b)?;
        h.update(&b);
    }
    Ok(h.finalize().into())
}

pub fn full_hash(path: &Path, cancel: Option<&Cancel>) -> Result<[u8; 32]> {
    let mut f = std::fs::File::open(path).map_err(io_err(path))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        if let Some(c) = cancel {
            c.check()?;
        }
        let n = f.read(&mut buf).map_err(io_err(path))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().into())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// 같은 파일 찾기
pub fn scan(roots: &[PathBuf], opts: &ScanOpts, cancel: Option<&Cancel>, progress: Progress) -> Result<ScanOut> {
    if roots.is_empty() {
        return Err(Error::Invalid("찾을 폴더를 고르세요".into()));
    }
    let min = opts.min_size.max(1);
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut by_size: HashMap<u64, Vec<(PathBuf, i64)>> = HashMap::new();
    let mut scanned = 0usize;
    for root in roots {
        if !root.exists() {
            return Err(Error::Invalid(format!("폴더가 없습니다: {}", root.display())));
        }
        let walker = walkdir::WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| opts.include_hidden || !is_hidden(e));
        for e in walker.filter_map(|e| e.ok()) {
            if scanned % 256 == 0 {
                if let Some(c) = cancel {
                    c.check()?;
                }
                progress("list", scanned, 0, e.path());
            }
            if !e.file_type().is_file() {
                continue;
            }
            let Ok(m) = e.metadata() else { continue };
            scanned += 1;
            let canon = e.path().canonicalize().unwrap_or_else(|_| e.path().to_path_buf());
            if !seen.insert(canon) {
                continue;
            }
            if m.len() < min {
                continue;
            }
            by_size.entry(m.len()).or_default().push((e.path().to_path_buf(), mtime_ms(&m)));
        }
    }
    let candidates: Vec<(u64, Vec<(PathBuf, i64)>)> = by_size.into_iter().filter(|(_, v)| v.len() > 1).collect();
    let total: usize = candidates.iter().map(|(_, v)| v.len()).sum();
    let mut done = 0usize;
    let mut groups = Vec::new();
    for (size, files) in candidates {
        // 앞·뒤
        let mut by_edge: HashMap<[u8; 32], Vec<(PathBuf, i64)>> = HashMap::new();
        for (p, m) in files {
            if let Some(c) = cancel {
                c.check()?;
            }
            progress("hash", done, total, &p);
            done += 1;
            if let Ok(h) = edge_hash(&p, size) {
                by_edge.entry(h).or_default().push((p, m));
            }
        }
        for (eh, files) in by_edge.into_iter().filter(|(_, v)| v.len() > 1) {
            let mut by_full: HashMap<[u8; 32], Vec<DupFile>> = HashMap::new();
            for (p, m) in files {
                // 작은 파일은 앞·뒤 해시가 곧 전체 해시
                let h = if size <= 2 * EDGE { Ok(eh) } else { full_hash(&p, cancel) };
                match h {
                    Ok(h) => by_full.entry(h).or_default().push(DupFile { path: p, modified: m }),
                    Err(Error::Cancelled) => return Err(Error::Cancelled),
                    Err(_) => {}
                }
            }
            for (h, mut files) in by_full.into_iter().filter(|(_, v)| v.len() > 1) {
                files.sort_by(|a, b| a.path.cmp(&b.path));
                groups.push(DupGroup { size, hash: hex(&h), files });
            }
        }
    }
    groups.sort_by(|a, b| (b.size * (b.files.len() as u64 - 1)).cmp(&(a.size * (a.files.len() as u64 - 1))).then_with(|| a.files[0].path.cmp(&b.files[0].path)));
    let wasted_bytes = groups.iter().map(|g| g.size * (g.files.len() as u64 - 1)).sum();
    progress("hash", total, total, Path::new(""));
    Ok(ScanOut { groups, scanned, wasted_bytes })
}

#[derive(Debug, Clone, Serialize)]
pub struct TrashFailed {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrashOut {
    pub done: usize,
    pub failed: Vec<TrashFailed>,
}

fn same_path(a: &Path, b: &Path) -> bool {
    let k = |p: &Path| crate::nfc::to_nfc(&p.to_string_lossy()).to_lowercase().replace('\\', "/");
    k(a) == k(b)
}

/// 고른 파일을 휴지통으로. `groups`는 찾기 결과(같은 내용 묶음).
/// 어떤 묶음이든 한 벌도 남지 않게 되면 아무것도 보내지 않고 오류. `recycle`은 실제 휴지통 보내기(윈도: [`recycle`]).
pub fn trash(paths: &[PathBuf], groups: &[DupGroup], recycle: &mut dyn FnMut(&Path) -> Result<()>) -> Result<TrashOut> {
    if paths.is_empty() {
        return Err(Error::Invalid("휴지통으로 보낼 파일을 고르세요".into()));
    }
    // 고른 파일마다 묶음 찾기
    let mut picked: Vec<(&PathBuf, &DupGroup)> = Vec::new();
    for p in paths {
        match groups.iter().find(|g| g.files.iter().any(|f| same_path(&f.path, p))) {
            Some(g) => picked.push((p, g)),
            None => return Err(Error::Invalid(format!("같은 파일 찾기 결과에 없는 파일이라 보내지 않았습니다: {}", p.display()))),
        }
    }
    // 묶음마다 남는 파일이 하나는 있어야 한다(지금도 있고 내용이 같은 것)
    let mut checked: HashSet<String> = HashSet::new();
    for (_, g) in &picked {
        if !checked.insert(g.hash.clone()) {
            continue;
        }
        let keepers: Vec<&DupFile> = g.files.iter().filter(|f| !paths.iter().any(|p| same_path(&f.path, p))).collect();
        if keepers.is_empty() {
            return Err(Error::Invalid(format!(
                "같은 파일 {}개를 모두 지우게 되어 하나도 보내지 않았습니다. 한 개는 남겨 두세요 ({})",
                g.files.len(),
                g.files[0].path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
            )));
        }
        let alive = keepers.iter().any(|k| std::fs::metadata(&k.path).map(|m| m.len() == g.size).unwrap_or(false) && full_hash(&k.path, None).map(|h| hex(&h) == g.hash).unwrap_or(false));
        if !alive {
            return Err(Error::Invalid(format!(
                "남길 파일이 그사이 바뀌었거나 없어져 하나도 보내지 않았습니다. 다시 찾아 주세요 ({})",
                keepers[0].path.display()
            )));
        }
    }
    let mut out = TrashOut { done: 0, failed: vec![] };
    for (p, g) in picked {
        let same = std::fs::metadata(p).map(|m| m.len() == g.size).unwrap_or(false) && full_hash(p, None).map(|h| hex(&h) == g.hash).unwrap_or(false);
        if !same {
            out.failed.push(TrashFailed { path: p.clone(), reason: "찾은 뒤에 바뀌었거나 없어져 보내지 않았습니다".into() });
            continue;
        }
        match recycle(p) {
            Ok(()) => out.done += 1,
            Err(e) => out.failed.push(TrashFailed { path: p.clone(), reason: e.to_string() }),
        }
    }
    Ok(out)
}

/// 휴지통으로 보내기(윈도). 휴지통이 없는 드라이브(USB·네트워크 등)는 거절한다 — 완전히 지우지 않는다.
#[cfg(windows)]
pub fn recycle(path: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};
    use windows_sys::Win32::UI::Shell::{SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, FO_DELETE, SHFILEOPSTRUCTW};
    const DRIVE_FIXED: u32 = 3;
    let abs = std::path::absolute(path).map_err(io_err(path))?;
    if !abs.is_file() {
        return Err(Error::Invalid(format!("파일이 없습니다: {}", abs.display())));
    }
    let wide: Vec<u16> = abs.as_os_str().encode_wide().chain([0]).collect();
    let mut vol = vec![0u16; 1024];
    let ok = unsafe { GetVolumePathNameW(wide.as_ptr(), vol.as_mut_ptr(), vol.len() as u32) };
    if ok == 0 || unsafe { GetDriveTypeW(vol.as_ptr()) } != DRIVE_FIXED {
        return Err(Error::Invalid("이 드라이브(USB·네트워크 등)에는 휴지통이 없어 보내지 않았습니다".into()));
    }
    // 이중 NUL로 끝나는 목록
    let mut from: Vec<u16> = abs.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        pTo: std::ptr::null(),
        // 휴지통으로(ALLOWUNDO). 휴지통에 못 넣는 경우엔 묻는 창(WANTNUKEWARNING)이 떠 완전히 지우기를 막을 수 있다
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING | FOF_NOERRORUI | FOF_SILENT) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };
    let r = unsafe { SHFileOperationW(&mut op) };
    if r != 0 || op.fAnyOperationsAborted != 0 {
        return Err(Error::Invalid(format!("휴지통으로 보내지 못했습니다(코드 {r:#x})")));
    }
    if abs.exists() {
        return Err(Error::Invalid("휴지통으로 보내지 못했습니다(파일이 그대로 있습니다)".into()));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn recycle(_path: &Path) -> Result<()> {
    Err(Error::Unsupported("휴지통 보내기는 윈도에서만 됩니다".into()))
}
