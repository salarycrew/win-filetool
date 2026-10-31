//! 2c: 빠른 검색 서비스(`filetool-svc`)에 붙기. 서비스가 답하면 그 서비스가 색인한 드라이브(NTFS 전체)는 서비스에 묻고,
//! 나머지 루트(폴더 루트, NTFS가 아닌 드라이브)는 앱의 색인(2a/2b)을 그대로 쓴다. 파이프가 사라지면 조용히 앱 색인으로 돌아간다.

use std::path::PathBuf;
use std::sync::Mutex;

use filetool_core::search::index::root_key;
use filetool_core::search::query::{Row, SortKey};
use filetool_core::search::svcpipe::{self, ServiceInfo, SvcClient};
use filetool_core::search::svcproto::{ClientView, QueryReply, SvcStatus};
use serde::Serialize;

#[derive(Default)]
pub struct Remote {
    client: Mutex<Option<SvcClient>>,
    /// 마지막으로 받은 상태와 호출자 정보
    last: Mutex<Option<(SvcStatus, ClientView)>>,
    /// 화면에 보인 서비스 색인 (세대, 버전) — 상태·질의 중 늦은 것(줄어들지 않음)
    seen: Mutex<(u64, u64)>,
    pub last_error: Mutex<Option<String>>,
}

/// 설정 화면용
#[derive(Debug, Clone, Serialize, Default)]
pub struct ServiceView {
    pub info: ServiceInfo,
    pub connected: bool,
    pub entries: usize,
    pub ready: bool,
    pub roots: Vec<String>,
    pub visibility: String,
    pub error: Option<String>,
    /// 앱 폴더에 filetool-svc.exe가 있는지(설치 단추)
    pub exe: Option<String>,
}

impl Remote {
    /// 연결돼 있지 않으면 붙어 보고, 상태를 갱신한다. 실패하면 끊긴 것으로
    pub fn poll(&self) -> Option<SvcStatus> {
        let mut g = self.client.lock().unwrap();
        if g.is_none() {
            match svcpipe::connect(&format!("win-filetool {}", env!("CARGO_PKG_VERSION"))) {
                Ok(c) => *g = Some(c),
                Err(e) => {
                    *self.last_error.lock().unwrap() = Some(e);
                    *self.last.lock().unwrap() = None;
                    return None;
                }
            }
        }
        let c = g.as_mut().unwrap();
        match c.status() {
            Ok(st) => {
                let me = c.me.clone();
                self.bump(st.generation, st.version);
                *self.last.lock().unwrap() = Some((st.clone(), me));
                *self.last_error.lock().unwrap() = None;
                Some(st)
            }
            Err(e) => {
                *g = None;
                *self.last.lock().unwrap() = None;
                *self.last_error.lock().unwrap() = Some(e);
                None
            }
        }
    }

    fn bump(&self, generation: u64, version: u64) {
        let mut s = self.seen.lock().unwrap();
        if (generation, version) > *s {
            *s = (generation, version);
        }
    }

    pub fn seen(&self) -> (u64, u64) {
        *self.seen.lock().unwrap()
    }

    pub fn status(&self) -> Option<(SvcStatus, ClientView)> {
        self.last.lock().unwrap().clone()
    }

    /// 서비스가 다 색인해 둔 루트 중 `roots`에 있는 것(드라이브 루트만)
    pub fn covered(&self, roots: &[PathBuf]) -> Vec<PathBuf> {
        let Some((st, _)) = self.status() else { return Vec::new() };
        if !st.ready {
            return Vec::new();
        }
        roots
            .iter()
            .filter(|r| filetool_core::search::mft::volume_letter(r).is_some())
            .filter(|r| st.roots.iter().any(|s| !s.scanning && s.last_scan > 0 && root_key(&PathBuf::from(&s.path)) == root_key(r)))
            .cloned()
            .collect()
    }

    fn with<T>(&self, f: impl FnOnce(&mut SvcClient) -> Result<T, String>) -> Result<T, String> {
        let mut g = self.client.lock().unwrap();
        let c = g.as_mut().ok_or("빠른 검색 서비스에 연결되어 있지 않습니다")?;
        let r = f(c);
        if r.is_err() {
            // 끊기거나 멈춘 서비스 → 다음 상태 확인 때 다시 붙어 봄(그동안은 앱 색인)
            *g = None;
            *self.last.lock().unwrap() = None;
        }
        r
    }

    pub fn query(&self, q: &str, sort: SortKey, desc: bool, excludes: &[String]) -> Result<QueryReply, String> {
        let r = self.with(|c| c.query(q, sort, desc, 0, excludes))?;
        self.bump(r.generation, r.version);
        Ok(r)
    }

    pub fn rows(&self, offset: usize, limit: usize) -> Result<Option<Vec<Row>>, String> {
        self.with(|c| c.rows(offset, limit))
    }

    pub fn view(&self, exe: Option<String>) -> ServiceView {
        let info = svcpipe::service_info();
        let mut v = ServiceView { info, exe, error: self.last_error.lock().unwrap().clone(), ..Default::default() };
        if let Some((st, me)) = self.status() {
            v.connected = true;
            v.entries = st.entries;
            v.ready = st.ready;
            v.roots = st.roots.iter().map(|r| r.path.clone()).collect();
            v.visibility = me.visibility;
            v.error = None;
        }
        v
    }
}
