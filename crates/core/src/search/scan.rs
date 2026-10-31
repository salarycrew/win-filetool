//! 스캐너. 색인을 채우는 "원본"은 [`IndexSource`] 뒤에 둔다:
//! 2a는 관리자 권한 없이 폴더를 훑는 [`DirWalkSource`], 2b에서 NTFS MFT/USN 원본을 같은 자리에 꽂는다.
//!
//! [`DirWalkSource`]: 폴더마다 병렬로 목록을 읽는다(Windows는 `FindFirstFileExW` + `FindExInfoBasic`
//! + `FIND_FIRST_EX_LARGE_FETCH`, 그 밖은 `read_dir`). 정션·심볼릭 링크 같은 연결 지점(이름 대리 reparse)은
//! 항목으로만 넣고 따라가지 않는다(고리 방지). OneDrive 같은 클라우드 자리 표시 폴더(reparse지만 이름 대리가 아님)는 따라간다.
//! 열 수 없는 폴더(권한 없음)는 건너뛰고 센다.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, RwLock};
use std::time::Instant;

use super::index::{Index, RootStats};
use super::text::{fold, glob_chars};

/// 폴더 목록 한 줄
#[derive(Debug, Clone)]
pub struct RawEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
    /// 따라가면 안 되는 연결 지점(정션·심볼릭 링크)
    pub link: bool,
}

/// 기본 제외 목록(설정에서 보이고 고칠 수 있음)
pub fn default_excludes() -> Vec<String> {
    vec!["$Recycle.Bin".into(), "System Volume Information".into(), r"?:\Windows\WinSxS".into()]
}

/// 제외 규칙: `\`가 있으면 전체 경로에(와일드카드 가능, 대소문자 무시), 없으면 폴더 이름에 맞춘다.
#[derive(Debug, Clone, Default)]
pub struct Excludes {
    name_pats: Vec<Vec<char>>,
    path_pats: Vec<Vec<char>>,
}

impl Excludes {
    pub fn new(pats: &[String]) -> Self {
        let mut e = Excludes::default();
        for p in pats {
            let k = fold(p.trim()).replace('/', "\\");
            let k = k.trim_end_matches('\\');
            if k.is_empty() {
                continue;
            }
            if k.contains('\\') {
                e.path_pats.push(k.chars().collect());
            } else {
                e.name_pats.push(k.chars().collect());
            }
        }
        e
    }

    pub fn is_empty(&self) -> bool {
        self.name_pats.is_empty() && self.path_pats.is_empty()
    }

    /// 폴더 하나가 제외 대상인지
    pub fn matches(&self, path: &Path) -> bool {
        if self.is_empty() {
            return false;
        }
        if !self.name_pats.is_empty() {
            if let Some(n) = path.file_name() {
                let n: Vec<char> = fold(&n.to_string_lossy()).chars().collect();
                if self.name_pats.iter().any(|p| glob_chars(p, &n)) {
                    return true;
                }
            }
        }
        if !self.path_pats.is_empty() {
            let k: Vec<char> = fold(&path.to_string_lossy()).replace('/', "\\").trim_end_matches('\\').chars().collect();
            if self.path_pats.iter().any(|p| glob_chars(p, &k)) {
                return true;
            }
        }
        false
    }

    /// 경로나 그 위 폴더 중 하나라도 제외 대상인지(실시간 갱신용)
    pub fn matches_any_ancestor(&self, path: &Path) -> bool {
        !self.is_empty() && path.ancestors().any(|a| self.matches(a))
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScanOpts {
    pub excludes: Excludes,
    /// 이 폴더들은 넣지 않음(예: 색인 파일이 있는 폴더)
    pub skip: Vec<PathBuf>,
    /// 동시에 목록을 읽을 스레드 수(0 = CPU 수, 최대 8)
    pub threads: usize,
}

/// 진행 상황·취소
#[derive(Debug, Default)]
pub struct ScanControl {
    pub cancel: AtomicBool,
    /// 지금까지 넣은 항목 수
    pub entries: AtomicU64,
    /// 따로 만든 새 색인에 스캔 중(살아 있는 색인은 그대로 쓸 수 있음 → 실시간 변경을 바로 반영해도 됨)
    pub background: AtomicBool,
}

/// 색인을 채우는 원본. `root_id`(이미 `target`에 있는 폴더 항목) 아래를 채운다.
pub trait IndexSource: Send + Sync {
    fn name(&self) -> &'static str;
    fn scan(&self, dir: &Path, dir_id: u32, target: &RwLock<Index>, opts: &ScanOpts, ctl: &ScanControl) -> RootStats;
}

/// 관리자 권한 없이 폴더를 병렬로 훑는 원본(2a)
pub struct DirWalkSource;

struct Queue {
    items: Vec<(u32, PathBuf)>,
    pending: usize,
}

#[derive(Default)]
struct Counters {
    dirs: AtomicU64,
    files: AtomicU64,
    denied: AtomicU64,
    reparse: AtomicU64,
    excluded: AtomicU64,
}

impl IndexSource for DirWalkSource {
    fn name(&self) -> &'static str {
        "폴더 훑기"
    }

    fn scan(&self, dir: &Path, dir_id: u32, target: &RwLock<Index>, opts: &ScanOpts, ctl: &ScanControl) -> RootStats {
        let t0 = Instant::now();
        let threads = if opts.threads == 0 {
            std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8)
        } else {
            opts.threads
        };
        let q = Mutex::new(Queue { items: vec![(dir_id, dir.to_path_buf())], pending: 1 });
        let cv = Condvar::new();
        let c = Counters::default();
        std::thread::scope(|sc| {
            for _ in 0..threads.max(1) {
                sc.spawn(|| worker(&q, &cv, &c, target, opts, ctl));
            }
        });
        RootStats {
            dirs: c.dirs.load(Ordering::Relaxed),
            files: c.files.load(Ordering::Relaxed),
            denied: c.denied.load(Ordering::Relaxed),
            reparse: c.reparse.load(Ordering::Relaxed),
            excluded: c.excluded.load(Ordering::Relaxed),
            millis: t0.elapsed().as_millis() as u64,
        }
    }
}

fn worker(q: &Mutex<Queue>, cv: &Condvar, c: &Counters, target: &RwLock<Index>, opts: &ScanOpts, ctl: &ScanControl) {
    loop {
        let (id, path) = {
            let mut g = q.lock().unwrap();
            loop {
                if ctl.cancel.load(Ordering::Relaxed) {
                    g.items.clear();
                    g.pending = 0;
                    cv.notify_all();
                    return;
                }
                if let Some(it) = g.items.pop() {
                    break it;
                }
                if g.pending == 0 {
                    cv.notify_all();
                    return;
                }
                g = cv.wait(g).unwrap();
            }
        };
        let mut subdirs = Vec::new();
        match list_dir(&path) {
            Ok(list) => {
                let mut ix = target.write().unwrap();
                for e in list {
                    if e.is_dir && !e.link {
                        let child = path.join(&e.name);
                        if opts.excludes.matches(&child) || opts.skip.iter().any(|s| s == &child) {
                            c.excluded.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        let cid = ix.push_child(id, &e.name, true, 0, e.mtime);
                        c.dirs.fetch_add(1, Ordering::Relaxed);
                        subdirs.push((cid, child));
                    } else {
                        if e.link {
                            c.reparse.fetch_add(1, Ordering::Relaxed);
                        }
                        ix.push_child(id, &e.name, e.is_dir, if e.is_dir { 0 } else { e.size }, e.mtime);
                        if e.is_dir {
                            c.dirs.fetch_add(1, Ordering::Relaxed);
                        } else {
                            c.files.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    ctl.entries.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(_) => {
                c.denied.fetch_add(1, Ordering::Relaxed);
            }
        }
        let mut g = q.lock().unwrap();
        g.pending += subdirs.len();
        g.items.extend(subdirs);
        g.pending -= 1;
        cv.notify_all();
    }
}

/// 폴더 하나의 목록
#[cfg(not(windows))]
pub fn list_dir(dir: &Path) -> std::io::Result<Vec<RawEntry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let Ok(ft) = e.file_type() else { continue };
        let md = e.metadata().ok();
        let mtime = md
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let link = ft.is_symlink();
        let is_dir = if link { std::fs::metadata(e.path()).map(|m| m.is_dir()).unwrap_or(false) } else { ft.is_dir() };
        out.push(RawEntry {
            name: e.file_name().to_string_lossy().into_owned(),
            is_dir,
            size: if is_dir { 0 } else { md.map(|m| m.len()).unwrap_or(0) },
            mtime,
            link,
        });
    }
    Ok(out)
}

#[cfg(windows)]
pub use win::{fixed_drives, list_dir, long_path_wide};

#[cfg(windows)]
mod win {
    use super::RawEntry;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW, GetDriveTypeW, GetLogicalDrives,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FIND_FIRST_EX_LARGE_FETCH, WIN32_FIND_DATAW,
    };

    const DRIVE_FIXED: u32 = 3;

    /// `\\?\` 붙인 넓은 문자열(긴 경로 허용), 끝에 NUL
    pub fn long_path_wide(p: &Path, suffix: &str) -> Vec<u16> {
        let s = p.as_os_str().to_string_lossy().replace('/', "\\");
        let full = if s.starts_with(r"\\?\") {
            s
        } else if let Some(unc) = s.strip_prefix(r"\\") {
            format!(r"\\?\UNC\{unc}")
        } else {
            format!(r"\\?\{s}")
        };
        let mut full = full;
        if !suffix.is_empty() {
            if !full.ends_with('\\') {
                full.push('\\');
            }
            full.push_str(suffix);
        }
        std::ffi::OsStr::new(&full).encode_wide().chain(std::iter::once(0)).collect()
    }

    fn filetime_to_unix(lo: u32, hi: u32) -> i64 {
        let t = ((hi as u64) << 32) | lo as u64;
        if t == 0 {
            return 0;
        }
        (t as i64 - 116_444_736_000_000_000) / 10_000_000
    }

    pub fn list_dir(dir: &Path) -> std::io::Result<Vec<RawEntry>> {
        let pat = long_path_wide(dir, "*");
        let mut data: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
        let h = unsafe {
            FindFirstFileExW(
                pat.as_ptr(),
                FindExInfoBasic,
                &mut data as *mut _ as *mut core::ffi::c_void,
                FindExSearchNameMatch,
                std::ptr::null(),
                FIND_FIRST_EX_LARGE_FETCH,
            )
        };
        if h == INVALID_HANDLE_VALUE {
            let e = unsafe { GetLastError() };
            if e == ERROR_FILE_NOT_FOUND {
                return Ok(Vec::new());
            }
            return Err(std::io::Error::from_raw_os_error(e as i32));
        }
        let mut out = Vec::new();
        loop {
            let len = data.cFileName.iter().position(|&c| c == 0).unwrap_or(data.cFileName.len());
            let name = String::from_utf16_lossy(&data.cFileName[..len]);
            if name != "." && name != ".." {
                let attr = data.dwFileAttributes;
                let is_dir = attr & FILE_ATTRIBUTE_DIRECTORY != 0;
                // 이름 대리(정션·심볼릭 링크·마운트 지점)만 따라가지 않는다. 0x20000000 = IsReparseTagNameSurrogate
                let link = attr & FILE_ATTRIBUTE_REPARSE_POINT != 0 && data.dwReserved0 & 0x2000_0000 != 0;
                out.push(RawEntry {
                    name,
                    is_dir,
                    size: ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64,
                    mtime: filetime_to_unix(data.ftLastWriteTime.dwLowDateTime, data.ftLastWriteTime.dwHighDateTime),
                    link,
                });
            }
            if unsafe { FindNextFileW(h, &mut data) } == 0 {
                let e = unsafe { GetLastError() };
                unsafe { FindClose(h) };
                if e != ERROR_NO_MORE_FILES {
                    // 중간에 끊김: 읽은 만큼만 쓴다
                }
                break;
            }
        }
        Ok(out)
    }

    /// 고정 드라이브(C:\, D:\ …)
    pub fn fixed_drives() -> Vec<PathBuf> {
        let mask = unsafe { GetLogicalDrives() };
        let mut v = Vec::new();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let root = format!("{}:\\", (b'A' + i as u8) as char);
            let w: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
            if unsafe { GetDriveTypeW(w.as_ptr()) } == DRIVE_FIXED {
                v.push(PathBuf::from(root));
            }
        }
        v
    }
}

/// Windows가 아니면 기본 루트는 집 폴더(시험용)
#[cfg(not(windows))]
pub fn fixed_drives() -> Vec<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).into_iter().collect()
}

/// 폴더 하나와 그 아래를 `dir_id` 밑에 채운다(실시간 갱신·부분 다시 스캔용)
pub fn scan_subtree(src: &dyn IndexSource, dir: &Path, dir_id: u32, target: &RwLock<Index>, opts: &ScanOpts) -> RootStats {
    let ctl = ScanControl::default();
    src.scan(dir, dir_id, target, opts, &ctl)
}

/// 루트 하나를 새 색인에 스캔한다(다른 스레드는 `out`을 읽으며 부분 결과를 볼 수 있음)
pub fn scan_root_into(src: &dyn IndexSource, root: &Path, out: &RwLock<Index>, opts: &ScanOpts, ctl: &ScanControl) -> RootStats {
    let id = out.write().unwrap().add_root(root);
    let st = src.scan(root, id, out, opts, ctl);
    let mut ix = out.write().unwrap();
    if let Some(r) = ix.roots_mut().iter_mut().find(|r| r.id == id) {
        r.stats = st.clone();
        if !ctl.cancel.load(Ordering::Relaxed) {
            r.last_scan = super::index::now_unix();
        }
    }
    st
}
