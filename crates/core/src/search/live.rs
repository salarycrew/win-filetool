//! 실시간 변경 알림. 앱이 켜져 있는 동안(창이든 트레이든) 루트마다 변경을 받아 [`Change`]로 보낸다.
//!
//! Windows: `ReadDirectoryChangesW`를 직접 쓴다. 루트 자체는 하위 폴더 없이, 루트 바로 아래 폴더마다는
//! 하위 전체를 감시한다(`notify` 크레이트는 버퍼가 넘치면 알림을 조용히 버리기 때문). 버퍼가 넘치거나(돌려받은 크기 0)
//! 오류가 나면 그 감시 폴더만 [`Change::Rescan`]으로 다시 훑게 한다.
//! 그 밖(Linux, 시험용): `notify`(inotify). 넘침 표시(`need_rescan`)·오류는 루트 다시 스캔.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::scan::Excludes;

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// 이 경로가 생기거나 바뀌거나 사라졌을 수 있음 → 확인해서 반영
    Path(PathBuf),
    /// 이 폴더 아래를 알 수 없게 됨(버퍼 넘침·오류) → 다시 훑기
    Rescan(PathBuf),
}

/// 감시를 멈추려면 drop
pub struct LiveWatch {
    stop: Arc<AtomicBool>,
    #[cfg(windows)]
    handles: Arc<std::sync::Mutex<Vec<win::DirHandle>>>,
    #[cfg(not(windows))]
    _watchers: Vec<notify::RecommendedWatcher>,
}

impl Drop for LiveWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        #[cfg(windows)]
        for h in self.handles.lock().unwrap().iter() {
            h.cancel();
        }
    }
}

/// 감시 시작. 오류(루트가 없음 등)는 건너뛴다.
pub fn start(roots: &[PathBuf], excludes: &Excludes, tx: Sender<Change>) -> LiveWatch {
    let stop = Arc::new(AtomicBool::new(false));
    #[cfg(windows)]
    {
        let handles = Arc::new(std::sync::Mutex::new(Vec::new()));
        for r in roots {
            win::watch_root(r, excludes, &tx, &stop, &handles);
        }
        LiveWatch { stop, handles }
    }
    #[cfg(not(windows))]
    {
        let _ = excludes;
        let mut ws = Vec::new();
        for r in roots {
            if let Some(w) = notify_watch(r, tx.clone(), stop.clone()) {
                ws.push(w);
            }
        }
        LiveWatch { stop, _watchers: ws }
    }
}

#[cfg(not(windows))]
fn notify_watch(root: &Path, tx: Sender<Change>, stop: Arc<AtomicBool>) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    let r = root.to_path_buf();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match res {
            Ok(ev) if ev.need_rescan() => {
                let _ = tx.send(Change::Rescan(r.clone()));
            }
            Ok(ev) => {
                for p in ev.paths {
                    let _ = tx.send(Change::Path(p));
                }
            }
            Err(_) => {
                let _ = tx.send(Change::Rescan(r.clone()));
            }
        }
    })
    .ok()?;
    w.watch(root, RecursiveMode::Recursive).ok()?;
    Some(w)
}

/// 경로가 감시 루트 아래인지
pub fn under(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

#[cfg(windows)]
mod win {
    use super::{Change, Excludes};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_CREATION,
        FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::CancelIoEx;

    /// 루트 바로 아래 폴더가 이보다 많으면 루트 하나를 통째로 감시한다(스레드 수 제한)
    const MAX_TOP_DIRS: usize = 64;
    const BUF_BYTES: usize = 64 * 1024;

    /// 다른 스레드에서 취소할 수 있게 핸들 값만 들고 있다
    pub struct DirHandle(isize);

    impl DirHandle {
        pub fn cancel(&self) {
            unsafe { CancelIoEx(self.0 as HANDLE, std::ptr::null()) };
        }
    }

    pub fn watch_root(root: &Path, ex: &Excludes, tx: &Sender<Change>, stop: &Arc<AtomicBool>, hs: &Arc<Mutex<Vec<DirHandle>>>) {
        let tops: Vec<PathBuf> = match super::super::scan::list_dir(root) {
            Ok(l) => l.into_iter().filter(|e| e.is_dir && !e.link).map(|e| root.join(e.name)).filter(|p| !ex.matches(p)).collect(),
            Err(_) => return,
        };
        if tops.len() > MAX_TOP_DIRS {
            spawn(root.to_path_buf(), true, tx.clone(), stop.clone(), hs.clone(), None, None);
            return;
        }
        let watched = Arc::new(Mutex::new(tops.clone()));
        for t in tops {
            spawn(t, true, tx.clone(), stop.clone(), hs.clone(), None, Some(watched.clone()));
        }
        spawn(root.to_path_buf(), false, tx.clone(), stop.clone(), hs.clone(), Some((watched, ex.clone())), None);
    }

    type TopState = (Arc<Mutex<Vec<PathBuf>>>, Excludes);

    fn open_dir(dir: &Path) -> Option<HANDLE> {
        let w = crate::search::scan::long_path_wide(dir, "");
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        (h != INVALID_HANDLE_VALUE).then_some(h)
    }

    /// `registry`: 이 감시가 끝나면(폴더가 사라짐) 목록에서 뺀다(같은 이름으로 다시 생기면 다시 감시하도록)
    fn spawn(
        dir: PathBuf,
        recursive: bool,
        tx: Sender<Change>,
        stop: Arc<AtomicBool>,
        hs: Arc<Mutex<Vec<DirHandle>>>,
        top: Option<TopState>,
        registry: Option<Arc<Mutex<Vec<PathBuf>>>>,
    ) {
        let Some(h) = open_dir(&dir) else {
            if let Some(r) = &registry {
                r.lock().unwrap().retain(|p| p != &dir);
            }
            return;
        };
        hs.lock().unwrap().push(DirHandle(h as isize));
        let hs2 = hs.clone();
        let hv = h as isize; // 원시 포인터는 스레드로 못 넘겨서 값으로
        std::thread::Builder::new()
            .name("live-watch".into())
            .spawn(move || {
                let h = hv as HANDLE;
                run(&dir, h, recursive, &tx, &stop, &hs2, top);
                unsafe { CloseHandle(h) };
                if let Some(r) = &registry {
                    r.lock().unwrap().retain(|p| p != &dir);
                }
            })
            .ok();
    }

    fn run(dir: &Path, h: HANDLE, recursive: bool, tx: &Sender<Change>, stop: &Arc<AtomicBool>, hs: &Arc<Mutex<Vec<DirHandle>>>, top: Option<TopState>) {
        // FILE_NOTIFY_INFORMATION은 DWORD 정렬이어야 한다
        let mut buf = vec![0u32; BUF_BYTES / 4];
        let filter = FILE_NOTIFY_CHANGE_FILE_NAME
            | FILE_NOTIFY_CHANGE_DIR_NAME
            | FILE_NOTIFY_CHANGE_SIZE
            | FILE_NOTIFY_CHANGE_LAST_WRITE
            | FILE_NOTIFY_CHANGE_CREATION;
        let mut failures = 0u32;
        while !stop.load(Ordering::SeqCst) {
            let mut got: u32 = 0;
            let ok = unsafe {
                ReadDirectoryChangesW(
                    h,
                    buf.as_mut_ptr() as *mut core::ffi::c_void,
                    BUF_BYTES as u32,
                    recursive as i32,
                    filter,
                    &mut got,
                    std::ptr::null_mut(),
                    None,
                )
            };
            if stop.load(Ordering::SeqCst) {
                return;
            }
            if ok == 0 {
                // 오류: 폴더가 사라졌으면 끝, 아니면 다시 훑게 하고 잠시 뒤 다시 시도
                if !dir.exists() {
                    let _ = tx.send(Change::Path(dir.to_path_buf()));
                    return;
                }
                let _ = tx.send(Change::Rescan(dir.to_path_buf()));
                failures += 1;
                if failures > 5 {
                    return;
                }
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
            failures = 0;
            if got == 0 {
                // 버퍼가 넘쳐 무엇이 바뀌었는지 모름 → 이 감시 폴더만 다시 훑기
                let _ = tx.send(Change::Rescan(dir.to_path_buf()));
                continue;
            }
            let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, got as usize) };
            let mut off = 0usize;
            loop {
                if off + 12 > bytes.len() {
                    break;
                }
                let next = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
                let name_len = u32::from_le_bytes(bytes[off + 8..off + 12].try_into().unwrap()) as usize;
                let start = off + 12;
                if start + name_len > bytes.len() {
                    break;
                }
                let name: Vec<u16> = bytes[start..start + name_len].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                let rel = String::from_utf16_lossy(&name);
                let p = dir.join(&rel);
                // 루트 바로 아래에 새 폴더가 생기면 그 폴더도 감시한다
                if let Some((watched, ex)) = &top {
                    if !rel.contains('\\') && p.is_dir() && !ex.matches(&p) {
                        let mut w = watched.lock().unwrap();
                        if !w.contains(&p) {
                            w.push(p.clone());
                            spawn(p.clone(), true, tx.clone(), stop.clone(), hs.clone(), None, Some(watched.clone()));
                        }
                    }
                }
                let _ = tx.send(Change::Path(p));
                if next == 0 {
                    break;
                }
                off += next;
            }
        }
    }
}
