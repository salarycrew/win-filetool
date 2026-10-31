//! 검색 서비스: 저장한 색인 불러오기 → 실시간 감시 시작 → 루트마다 배경에서 다시 스캔 → 저장.
//! 앱(창·트레이)과 CLI(`search-live`)가 같이 쓴다.
//!
//! - 색인이 없던 루트는 살아 있는 색인에 바로 채운다(스캔 중에도 부분 결과로 검색됨).
//! - 이미 있던 루트는 새 색인에 따로 스캔한 뒤 통째로 바꿔 끼운다(그동안은 불러온 색인으로 검색).
//! - 스캔 중인 루트의 변경 알림은 모아 두었다가 스캔이 끝나면 다시 반영한다.
//! - 변경은 0.3초 모아서 한 번에 반영. 버퍼 넘침·오류는 그 폴더 아래만 다시 훑는다.
//! - 저장: 스캔이 모두 끝났을 때, 그 뒤 바뀐 것이 있으면 10분마다, 끝낼 때.
//! - 2b: 루트가 NTFS 드라이브 전체이고 볼륨을 열 수 있으면(관리자 권한) MFT로 읽는다([`super::mft`]).
//!   저장한 색인에 같은 저널의 위치가 있으면 전체 스캔 대신 그 뒤 변경만 따라잡고, 켜져 있는 동안은
//!   `ReadDirectoryChangesW` 대신 USN 저널을 0.5초마다 읽어 반영한다. 안 되면 조용히 2a 길(폴더 훑기).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::index::{now_unix, Index, RootStats, SourceKind, INDEX_FILE};
use super::live::{self, Change, LiveWatch};
use super::mft::{self, FrnMap, MftSource, Plan, SourceMode, Volume};
use super::query::{self, Query, QueryCtx, Row, SortKey};
use super::scan::{self, DirWalkSource, Excludes, IndexSource, ScanControl, ScanOpts};

const DEBOUNCE: Duration = Duration::from_millis(300);
const SAVE_EVERY: Duration = Duration::from_secs(600);

/// 색인 폴더: `WIN_FILETOOL_INDEX_DIR` → `%LOCALAPPDATA%\kr.salarycrew.winfiletool\index`
/// (Windows가 아니면 `~/.local/share/kr.salarycrew.winfiletool/index`)
pub fn default_index_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("WIN_FILETOOL_INDEX_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("kr.salarycrew.winfiletool").join("index")
}

#[derive(Debug, Clone)]
pub struct SearchConfig {
    pub roots: Vec<PathBuf>,
    pub excludes: Vec<String>,
    /// 색인 파일(보통 `default_index_dir()/index-v2.bin`)
    pub index_file: PathBuf,
    /// 색인 원본(기본 auto: 관리자 권한이고 NTFS 드라이브 전체면 MFT)
    pub source: SourceMode,
}

impl SearchConfig {
    pub fn new(roots: Vec<PathBuf>, excludes: Vec<String>, index_dir: &Path) -> Self {
        Self { roots, excludes, index_file: index_dir.join(INDEX_FILE), source: SourceMode::Auto }
    }
    pub fn opts(&self) -> ScanOpts {
        let skip = self.index_file.parent().map(|p| vec![p.to_path_buf()]).unwrap_or_default();
        ScanOpts { excludes: Excludes::new(&self.excludes), skip, threads: 0 }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RootStatus {
    pub path: String,
    pub scanning: bool,
    /// 스캔 중이면 지금까지 넣은 항목 수
    pub scanned_so_far: u64,
    pub last_scan: i64,
    pub stats: RootStats,
    /// 이 루트를 채우는 길: "빠른 색인(MFT)" / "일반 색인"
    pub source: String,
    /// MFT를 쓰지 않는 이유(관리자 권한 없음 등)
    pub source_note: Option<String>,
    /// 마지막 저널 따라잡기 결과
    pub catchup: Option<String>,
}

/// 루트마다 고른 길(상태 표시용)
#[derive(Debug, Clone, Default)]
struct RootNote {
    mft: bool,
    reason: Option<String>,
    catchup: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct SearchStatus {
    pub entries: usize,
    pub roots: Vec<RootStatus>,
    /// 저장한 색인을 불러왔으면 걸린 시간(ms)
    pub loaded_ms: Option<u64>,
    pub load_error: Option<String>,
    pub last_save: i64,
    pub index_bytes: u64,
    pub index_file: String,
    /// 건너뛴 폴더(권한 없음 + 연결 지점) 합
    pub skipped: u64,
    pub excluded: u64,
    pub version: u64,
    pub generation: u64,
    /// 처음 스캔(또는 불러오기)이 끝나 검색 결과를 믿을 수 있는지
    pub ready: bool,
    pub live_updates: u64,
    pub memory_bytes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub total: usize,
    pub millis: f64,
    pub generation: u64,
    pub version: u64,
}

enum Msg {
    Change(Change),
    /// USN 저널에서 읽은 변경(루트, 바뀐 FRN, 다음 USN)
    Journal(PathBuf, Vec<u64>, i64),
    /// 저널이 다시 만들어짐·한 바퀴 돎 → 그 루트를 MFT로 다시 읽기
    JournalLost(PathBuf),
    Scanned(PathBuf),
    Reconfigure(SearchConfig),
    SaveNow(Sender<Result<u64, String>>),
    Stop,
}

#[derive(Default)]
struct Info {
    loaded_ms: Option<u64>,
    load_error: Option<String>,
    last_save: i64,
    index_bytes: u64,
    ready: bool,
}

pub struct SearchService {
    pub index: Arc<RwLock<Index>>,
    tx: Mutex<Sender<Msg>>,
    cfg: Arc<Mutex<SearchConfig>>,
    scanning: Arc<Mutex<HashMap<PathBuf, Arc<ScanControl>>>>,
    info: Arc<Mutex<Info>>,
    live_updates: Arc<AtomicU64>,
    notes: Arc<Mutex<HashMap<PathBuf, RootNote>>>,
}

impl SearchService {
    /// 서비스 시작(배경 스레드). 바로 돌아온다.
    pub fn start(cfg: SearchConfig) -> Arc<SearchService> {
        let (tx, rx) = channel();
        let svc = Arc::new(SearchService {
            index: Arc::new(RwLock::new(Index::new())),
            tx: Mutex::new(tx.clone()),
            cfg: Arc::new(Mutex::new(cfg)),
            scanning: Arc::new(Mutex::new(HashMap::new())),
            info: Arc::new(Mutex::new(Info::default())),
            live_updates: Arc::new(AtomicU64::new(0)),
            notes: Arc::new(Mutex::new(HashMap::new())),
        });
        let s2 = svc.clone();
        std::thread::Builder::new().name("search".into()).spawn(move || s2.main_loop(rx, tx)).ok();
        svc
    }

    fn send(&self, m: Msg) {
        let _ = self.tx.lock().unwrap().send(m);
    }

    pub fn config(&self) -> SearchConfig {
        self.cfg.lock().unwrap().clone()
    }

    pub fn reconfigure(&self, cfg: SearchConfig) {
        self.send(Msg::Reconfigure(cfg));
    }

    /// 다시 스캔(모든 루트)
    pub fn rescan_all(&self) {
        let cfg = self.config();
        self.send(Msg::Reconfigure(cfg));
    }

    /// 지금 저장(끝낼 때). 저장한 파일 크기
    pub fn save_now(&self) -> Result<u64, String> {
        let (tx, rx) = channel();
        self.send(Msg::SaveNow(tx));
        rx.recv_timeout(Duration::from_secs(60)).map_err(|e| e.to_string())?
    }

    pub fn stop(&self) {
        for c in self.scanning.lock().unwrap().values() {
            c.cancel.store(true, Ordering::SeqCst);
        }
        self.send(Msg::Stop);
    }

    pub fn status(&self) -> SearchStatus {
        let ix = self.index.read().unwrap();
        let cfg = self.config();
        let scanning = self.scanning.lock().unwrap();
        let info = self.info.lock().unwrap();
        let notes = self.notes.lock().unwrap();
        let mut st = SearchStatus {
            entries: ix.live_count(),
            loaded_ms: info.loaded_ms,
            load_error: info.load_error.clone(),
            last_save: info.last_save,
            index_bytes: info.index_bytes,
            index_file: cfg.index_file.to_string_lossy().into_owned(),
            version: ix.version,
            generation: ix.generation,
            ready: info.ready,
            live_updates: self.live_updates.load(Ordering::Relaxed),
            memory_bytes: ix.heap_bytes(),
            ..Default::default()
        };
        for r in &cfg.roots {
            let info = ix.root_by_path(r);
            let ctl = scanning.get(r);
            let stats = info.map(|i| i.stats.clone()).unwrap_or_default();
            st.skipped += stats.denied + stats.reparse;
            st.excluded += stats.excluded;
            let note = notes.get(r).cloned().unwrap_or_default();
            let kind = if note.mft { SourceKind::Mft } else { info.map(|i| i.source).unwrap_or_default() };
            st.roots.push(RootStatus {
                path: r.to_string_lossy().into_owned(),
                scanning: ctl.is_some(),
                scanned_so_far: ctl.map(|c| c.entries.load(Ordering::Relaxed)).unwrap_or(0),
                last_scan: info.map(|i| i.last_scan).unwrap_or(0),
                stats,
                source: kind.label().to_string(),
                source_note: note.reason,
                catchup: note.catchup,
            });
        }
        st
    }

    /// 검색해서 결과 번호를 정렬해 돌려준다.
    pub fn query(&self, q: &str, sort: SortKey, desc: bool) -> Result<(Vec<u32>, SearchResult), String> {
        let query = Query::parse(q, &QueryCtx::local_now())?;
        let t0 = Instant::now();
        let ix = self.index.read().unwrap();
        if query.is_empty() {
            return Ok((Vec::new(), SearchResult { total: 0, millis: 0.0, generation: ix.generation, version: ix.version }));
        }
        let mut ids = query::run(&ix, &query);
        query::sort(&ix, &mut ids, sort, desc);
        let r = SearchResult { total: ids.len(), millis: t0.elapsed().as_secs_f64() * 1000.0, generation: ix.generation, version: ix.version };
        Ok((ids, r))
    }

    /// 결과 줄. 색인이 바뀌어(번호가 달라져) 더는 없는 항목은 빠진다.
    pub fn rows(&self, ids: &[u32], generation: u64) -> Option<Vec<Row>> {
        let ix = self.index.read().unwrap();
        if ix.generation != generation {
            return None;
        }
        Some(ids.iter().filter(|&&id| ix.is_live(id)).map(|&id| query::row(&ix, id)).collect())
    }

    // ---------- 배경 스레드 ----------

    fn main_loop(self: Arc<Self>, rx: Receiver<Msg>, tx: Sender<Msg>) {
        let src: Arc<dyn IndexSource> = Arc::new(DirWalkSource);
        let mut cfg = self.config();
        // 1) 저장한 색인
        let t0 = Instant::now();
        match Index::load_dir_file(&cfg.index_file) {
            Ok(mut ix) => {
                // 설정에서 빠진 루트는 버린다
                let gone: Vec<u32> = ix.roots().iter().filter(|r| !cfg.roots.iter().any(|c| same_root(c, &r.path))).map(|r| r.id).collect();
                for id in gone {
                    ix = ix.compacted(Some(id));
                }
                ix.build_name_rank();
                *self.index.write().unwrap() = ix;
                let mut info = self.info.lock().unwrap();
                info.loaded_ms = Some(t0.elapsed().as_millis() as u64);
                info.index_bytes = std::fs::metadata(&cfg.index_file).map(|m| m.len()).unwrap_or(0);
                info.ready = true;
            }
            Err(e) => {
                if cfg.index_file.exists() {
                    self.info.lock().unwrap().load_error = Some(e.to_string());
                }
            }
        }
        // 루트마다 길 고르기(MFT: NTFS 드라이브 전체 + 볼륨 열림)
        let mut vols = self.plan(&cfg);
        // 2) 실시간 감시 + 3) 배경 스캔(MFT 루트는 저널 따라잡기 또는 MFT 읽기)
        let mut watch = self.start_live(&cfg, &vols, &tx);
        self.spawn_scan(cfg.roots.iter().map(|r| (r.clone(), vols.get(r).cloned())).collect(), &cfg, tx.clone());
        let mut followers: HashMap<PathBuf, Arc<std::sync::atomic::AtomicBool>> = HashMap::new();
        let mut maps: HashMap<PathBuf, FrnMap> = HashMap::new();
        let mut journal_pending: HashMap<PathBuf, (Vec<u64>, i64)> = HashMap::new();

        let mut pending: Vec<Change> = Vec::new();
        let mut deferred: HashMap<PathBuf, Vec<Change>> = HashMap::new();
        let mut deadline: Option<Instant> = None;
        let mut saved_version = u64::MAX;
        let mut last_save = Instant::now();
        loop {
            let wait = deadline.map(|d| d.saturating_duration_since(Instant::now())).unwrap_or(Duration::from_secs(5));
            match rx.recv_timeout(wait) {
                Ok(Msg::Change(c)) => {
                    let p = match &c {
                        Change::Path(p) | Change::Rescan(p) => p.clone(),
                    };
                    let scanning_root =
                        self.scanning.lock().unwrap().iter().find(|(r, _)| p.starts_with(r)).map(|(r, c)| (r.clone(), c.background.load(Ordering::SeqCst)));
                    match scanning_root {
                        // 새 색인에 따로 스캔 중: 지금 색인에도 바로 반영하고, 바꿔 끼운 뒤 한 번 더
                        Some((r, true)) => {
                            deferred.entry(r).or_default().push(c.clone());
                            pending.push(c);
                            deadline.get_or_insert_with(|| Instant::now() + DEBOUNCE);
                        }
                        // 살아 있는 색인에 바로 채우는 중: 끝난 뒤 반영(부모가 아직 없을 수 있음)
                        Some((r, false)) => deferred.entry(r).or_default().push(c),
                        None => {
                            pending.push(c);
                            deadline.get_or_insert_with(|| Instant::now() + DEBOUNCE);
                        }
                    }
                }
                Ok(Msg::Journal(root, frns, next)) => {
                    let e = journal_pending.entry(root).or_insert_with(|| (Vec::new(), next));
                    e.0.extend(frns);
                    e.1 = next;
                    deadline.get_or_insert_with(|| Instant::now() + DEBOUNCE);
                }
                Ok(Msg::JournalLost(root)) => {
                    if let Some(f) = followers.remove(&root) {
                        f.store(true, Ordering::SeqCst);
                    }
                    journal_pending.remove(&root);
                    if let Some(v) = vols.get(&root).cloned() {
                        // 저장한 위치를 버리고 MFT 전체 다시 읽기
                        if let Some(r) = self.index.write().unwrap().roots_mut().iter_mut().find(|r| same_root(&r.path, &root)) {
                            r.usn = None;
                        }
                        self.spawn_scan(vec![(root, Some(v))], &cfg, tx.clone());
                    }
                }
                Ok(Msg::Scanned(root)) => {
                    // MFT 루트: 저널 따라가기 시작(저널이 없으면 start_live가 ReadDirectoryChangesW로 감시 중)
                    if let Some(v) = vols.get(&root) {
                        let cur = self.index.read().unwrap().root_by_path(&root).and_then(|r| if r.source == SourceKind::Mft { r.usn } else { None });
                        if let Some(c) = cur {
                            if !followers.contains_key(&root) {
                                if debug() {
                            eprintln!("[usn] {} 따라가기 시작 USN {}", root.display(), c.next_usn);
                        }
                        followers.insert(root.clone(), spawn_follower(v.clone(), root.clone(), c, tx.clone()));
                            }
                        }
                    }
                    if let Some(v) = deferred.remove(&root) {
                        pending.extend(v);
                        deadline.get_or_insert_with(|| Instant::now() + DEBOUNCE);
                    }
                    if self.scanning.lock().unwrap().is_empty() {
                        self.index.read().unwrap().build_name_rank();
                        self.info.lock().unwrap().ready = true;
                        saved_version = self.save(&cfg).map(|_| self.index.read().unwrap().version).unwrap_or(saved_version);
                        last_save = Instant::now();
                    }
                }
                Ok(Msg::Reconfigure(new)) => {
                    drop(watch);
                    for f in followers.values() {
                        f.store(true, Ordering::SeqCst);
                    }
                    followers.clear();
                    journal_pending.clear();
                    maps.clear();
                    for c in self.scanning.lock().unwrap().values() {
                        c.cancel.store(true, Ordering::SeqCst);
                    }
                    {
                        let mut g = self.index.write().unwrap();
                        let gone: Vec<u32> = g.roots().iter().filter(|r| !new.roots.iter().any(|c| same_root(c, &r.path))).map(|r| r.id).collect();
                        for id in gone {
                            *g = g.compacted(Some(id));
                        }
                    }
                    *self.cfg.lock().unwrap() = new.clone();
                    cfg = new;
                    vols = self.plan(&cfg);
                    watch = self.start_live(&cfg, &vols, &tx);
                    self.spawn_scan(cfg.roots.iter().map(|r| (r.clone(), vols.get(r).cloned())).collect(), &cfg, tx.clone());
                }
                Ok(Msg::SaveNow(reply)) => {
                    let r = self.save(&cfg);
                    if r.is_ok() {
                        saved_version = self.index.read().unwrap().version;
                        last_save = Instant::now();
                    }
                    let _ = reply.send(r);
                }
                Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => {
                    drop(watch);
                    for f in followers.values() {
                        f.store(true, Ordering::SeqCst);
                    }
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            if deadline.map(|d| Instant::now() >= d).unwrap_or(false) {
                deadline = None;
                let batch = std::mem::take(&mut pending);
                self.apply_batch(&*src, &cfg, batch, &tx);
                for (root, (frns, next)) in std::mem::take(&mut journal_pending) {
                    let Some(v) = vols.get(&root) else { continue };
                    if self.scanning.lock().unwrap().contains_key(&root) {
                        continue;
                    }
                    let n = self.apply_journal(v, &root, &frns, next, &cfg, &mut maps);
                    self.live_updates.fetch_add(n as u64, Ordering::Relaxed);
                }
            }
            if self.scanning.lock().unwrap().is_empty()
                && last_save.elapsed() >= SAVE_EVERY
                && self.index.read().unwrap().version != saved_version
            {
                if self.save(&cfg).is_ok() {
                    saved_version = self.index.read().unwrap().version;
                }
                last_save = Instant::now();
            }
        }
    }

    /// 루트마다 MFT를 쓸 수 있는지(볼륨 열기). 상태 표시용 메모도 채운다
    fn plan(&self, cfg: &SearchConfig) -> HashMap<PathBuf, Arc<Volume>> {
        let mut out = HashMap::new();
        let mut notes = self.notes.lock().unwrap();
        notes.clear();
        for r in &cfg.roots {
            let mode = if cfg.source == SourceMode::Mft { SourceMode::Auto } else { cfg.source };
            match mft::plan_for(r, mode) {
                Ok(Plan::Mft(v)) => {
                    notes.insert(r.clone(), RootNote { mft: true, ..Default::default() });
                    out.insert(r.clone(), v);
                }
                Ok(Plan::Walk { reason }) => {
                    notes.insert(r.clone(), RootNote { mft: false, reason, catchup: None });
                }
                Err(e) => {
                    notes.insert(r.clone(), RootNote { mft: false, reason: Some(e), catchup: None });
                }
            }
        }
        out
    }

    /// 저널 변경 반영 → 루트의 저널 위치 갱신. 바뀐 항목 수
    fn apply_journal(&self, v: &Arc<Volume>, root: &Path, frns: &[u64], next: i64, cfg: &SearchConfig, maps: &mut HashMap<PathBuf, FrnMap>) -> usize {
        let Some(rid) = self.index.read().unwrap().root_by_path(root).map(|r| r.id) else { return 0 };
        // 처음이거나 루트 번호가 바뀌었으면 빈 표(→ apply_changes가 색인에서 만든다)
        let map = maps.entry(root.to_path_buf()).or_insert_with(|| FrnMap::empty(rid));
        if map.root != rid {
            *map = FrnMap::empty(rid);
        }
        let st = mft::apply_changes(&self.index, map, root, frns, &*v.view(), &cfg.opts());
        if debug() {
            eprintln!("[usn] {} 반영: FRN {} → {:?}, 다음 USN {next}", root.display(), frns.len(), st);
        }
        if let Some(r) = self.index.write().unwrap().roots_mut().iter_mut().find(|r| r.id == rid) {
            if let Some(c) = r.usn.as_mut() {
                c.next_usn = next;
            }
        }
        st.changed()
    }

    fn start_live(&self, cfg: &SearchConfig, vols: &HashMap<PathBuf, Arc<Volume>>, tx: &Sender<Msg>) -> LiveWatch {
        let (ctx, crx) = channel::<Change>();
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("live-forward".into())
            .spawn(move || {
                for c in crx {
                    if tx.send(Msg::Change(c)).is_err() {
                        break;
                    }
                }
            })
            .ok();
        // MFT 루트는 저널을 따라간다(저널이 없는 볼륨만 ReadDirectoryChangesW)
        let roots: Vec<PathBuf> = cfg.roots.iter().filter(|r| vols.get(*r).map(|v| v.cursor().is_none()).unwrap_or(true)).cloned().collect();
        live::start(&roots, &cfg.opts().excludes, ctx)
    }

    fn save(&self, cfg: &SearchConfig) -> Result<u64, String> {
        let ix = self.index.read().unwrap();
        let n = ix.save(&cfg.index_file).map_err(|e| e.to_string())?;
        drop(ix);
        let mut info = self.info.lock().unwrap();
        info.last_save = now_unix();
        info.index_bytes = n;
        Ok(n)
    }

    /// 루트들을 차례로 스캔(각 루트 안은 병렬)
    fn spawn_scan(&self, roots: Vec<(PathBuf, Option<Arc<Volume>>)>, cfg: &SearchConfig, tx: Sender<Msg>) {
        let index = self.index.clone();
        let scanning = self.scanning.clone();
        let notes = self.notes.clone();
        let opts = cfg.opts();
        // 시작 전에 표시해 두어 그 사이 알림이 미뤄지게 한다
        let ctls: Vec<(PathBuf, Arc<ScanControl>)> = {
            let mut s = scanning.lock().unwrap();
            roots
                .iter()
                .map(|(r, _)| {
                    let c = Arc::new(ScanControl::default());
                    c.background.store(index.read().unwrap().root_by_path(r).is_some(), Ordering::SeqCst);
                    s.insert(r.clone(), c.clone());
                    (r.clone(), c)
                })
                .collect()
        };
        let vols: Vec<Option<Arc<Volume>>> = roots.into_iter().map(|(_, v)| v).collect();
        std::thread::Builder::new()
            .name("search-scan".into())
            .spawn(move || {
                for ((root, ctl), vol) in ctls.into_iter().zip(vols) {
                    // MFT 루트에 같은 저널의 저장 위치가 있으면 그 뒤 변경만 따라잡는다
                    if let Some(v) = &vol {
                        if !ctl.cancel.load(Ordering::SeqCst) {
                            if let Some(msg) = try_catch_up(&index, &*v.view(), &root, &opts) {
                                if let Some(n) = notes.lock().unwrap().get_mut(&root) {
                                    n.catchup = Some(msg);
                                }
                                {
                                    let mut s = scanning.lock().unwrap();
                                    if s.get(&root).map(|c| Arc::ptr_eq(c, &ctl)).unwrap_or(false) {
                                        s.remove(&root);
                                    }
                                }
                                let _ = tx.send(Msg::Scanned(root));
                                continue;
                            }
                        }
                    }
                    let src: Arc<dyn IndexSource> = match &vol {
                        Some(v) => Arc::new(MftSource::new(v.reader(), v.cursor())),
                        None => Arc::new(DirWalkSource),
                    };
                    if !ctl.cancel.load(Ordering::SeqCst) {
                        let exists = index.read().unwrap().root_by_path(&root).is_some();
                        ctl.background.store(exists, Ordering::SeqCst);
                        if exists {
                            let fresh = RwLock::new(Index::new());
                            scan::scan_root_into(&*src, &root, &fresh, &opts, &ctl);
                            if !ctl.cancel.load(Ordering::SeqCst) {
                                let fresh = fresh.into_inner().unwrap();
                                let mut g = index.write().unwrap();
                                let new = g.with_root_replaced(&fresh);
                                *g = new;
                            }
                        } else {
                            scan::scan_root_into(&*src, &root, &index, &opts, &ctl);
                        }
                    }
                    {
                        let mut s = scanning.lock().unwrap();
                        if s.get(&root).map(|c| Arc::ptr_eq(c, &ctl)).unwrap_or(false) {
                            s.remove(&root);
                        }
                    }
                    let _ = tx.send(Msg::Scanned(root));
                }
            })
            .ok();
    }

    fn apply_batch(&self, src: &dyn IndexSource, cfg: &SearchConfig, batch: Vec<Change>, tx: &Sender<Msg>) {
        let opts = cfg.opts();
        // 같은 것 빼기(순서 유지), 다시 훑을 폴더 아래의 개별 변경은 버림
        let mut seen = HashSet::new();
        let mut rescans: Vec<PathBuf> = Vec::new();
        let mut paths: Vec<PathBuf> = Vec::new();
        for c in batch {
            match c {
                Change::Rescan(p) => {
                    if seen.insert((true, p.clone())) {
                        rescans.push(p)
                    }
                }
                Change::Path(p) => {
                    if seen.insert((false, p.clone())) {
                        paths.push(p)
                    }
                }
            }
        }
        let all = rescans.clone();
        rescans.retain(|r| !all.iter().any(|o| o != r && r.starts_with(o)));
        paths.retain(|p| !rescans.iter().any(|r| p.starts_with(r)));
        for r in rescans {
            if cfg.roots.iter().any(|x| same_root(x, &r)) {
                self.spawn_scan(vec![(r, None)], cfg, tx.clone());
                continue;
            }
            rescan_dir(src, &self.index, &r, &opts);
            self.live_updates.fetch_add(1, Ordering::Relaxed);
        }
        for p in paths {
            if apply_path(src, &self.index, &cfg.roots, &p, &opts) {
                self.live_updates.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// 저장한 색인의 MFT 루트를 저널로 따라잡기. 됐으면 결과 문장, 안 되면 None(→ 전체 읽기).
/// 앱(관리자 실행)과 2c 서비스(`filetool-svc`, 자기 색인 파일)가 같이 쓴다.
pub fn try_catch_up(index: &RwLock<Index>, view: &dyn mft::VolumeView, root: &Path, opts: &ScanOpts) -> Option<String> {
    let (rid, cur) = {
        let ix = index.read().unwrap();
        let r = ix.root_by_path(root)?;
        if r.source != SourceKind::Mft {
            return None;
        }
        (r.id, r.usn?)
    };
    let t0 = Instant::now();
    match mft::catch_up(view, &cur) {
        Ok((frns, newc, nrec)) => {
            let mut map = FrnMap::empty(rid);
            let st = mft::apply_changes(index, &mut map, root, &frns, view, opts);
            let mut ix = index.write().unwrap();
            if let Some(r) = ix.roots_mut().iter_mut().find(|r| r.id == rid) {
                r.usn = Some(newc);
                r.last_scan = now_unix();
            }
            Some(format!(
                "저널 따라잡기: 기록 {nrec}건, 파일 {}개, 반영 {}(추가 {} / 삭제 {} / 변경 {} / 이동 {}), {:.2} 초",
                st.files,
                st.changed(),
                st.added,
                st.removed,
                st.updated,
                st.moved,
                t0.elapsed().as_secs_f64()
            ))
        }
        Err(e) => {
            eprintln!("{}: {e} → MFT 전체 읽기", root.display());
            None
        }
    }
}

/// 저널 따라가기(0.5초마다 읽기). 멈추려면 돌려받은 깃발을 켠다
fn spawn_follower(v: Arc<Volume>, root: PathBuf, cur: super::index::UsnCursor, tx: Sender<Msg>) -> Arc<std::sync::atomic::AtomicBool> {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s2 = stop.clone();
    std::thread::Builder::new()
        .name("usn-follow".into())
        .spawn(move || {
            let view = v.view();
            let mut usn = cur.next_usn;
            while !s2.load(Ordering::SeqCst) {
                match view.read_journal(cur.journal_id, usn) {
                    Ok((next, recs)) => {
                        if debug() && !recs.is_empty() {
                            eprintln!("[usn] 읽음 {}건 {usn} → {next}", recs.len());
                        }
                        if !recs.is_empty() {
                            let frns: Vec<u64> = recs.iter().map(|r| r.frn).collect();
                            if tx.send(Msg::Journal(root.clone(), frns, next)).is_err() {
                                return;
                            }
                        }
                        let more = next > usn && !recs.is_empty();
                        usn = next.max(usn);
                        if more {
                            continue;
                        }
                    }
                    Err(mft::JournalErr::Reset(m)) => {
                        if debug() {
                            eprintln!("[usn] {} 저널 끊김: {m}", root.display());
                        }
                        let _ = tx.send(Msg::JournalLost(root.clone()));
                        return;
                    }
                    Err(mft::JournalErr::Io(e)) => {
                        if debug() {
                            eprintln!("[usn] {} 읽기 오류: {e}", root.display());
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .ok();
    stop
}

/// `WFT_DEBUG=1`이면 저널 따라가기 기록을 stderr에
fn debug() -> bool {
    std::env::var_os("WFT_DEBUG").is_some()
}

fn same_root(a: &Path, b: &Path) -> bool {
    super::index::root_key(a) == super::index::root_key(b)
}

/// 폴더 아래를 지우고 다시 훑는다(버퍼 넘침·오류 뒤)
pub fn rescan_dir(src: &dyn IndexSource, index: &RwLock<Index>, dir: &Path, opts: &ScanOpts) {
    let id = index.read().unwrap().lookup(dir);
    match id {
        Some(id) if index.read().unwrap().is_dir(id) => {
            index.write().unwrap().clear_root(id);
            scan::scan_subtree(src, dir, id, index, opts);
        }
        _ => {
            apply_path(src, index, &[], dir, opts);
        }
    }
}

/// 경로 하나의 지금 상태를 색인에 반영한다(생김·바뀜·사라짐). 바뀐 것이 있으면 true.
/// `roots`가 비어 있으면 색인에 있는 루트를 쓴다.
pub fn apply_path(src: &dyn IndexSource, index: &RwLock<Index>, roots: &[PathBuf], p: &Path, opts: &ScanOpts) -> bool {
    let under_root = {
        let ix = index.read().unwrap();
        if roots.is_empty() {
            ix.roots().iter().any(|r| p.starts_with(&r.path) && p != r.path)
        } else {
            roots.iter().any(|r| p.starts_with(r) && p != r)
        }
    };
    if !under_root || opts.skip.iter().any(|s| p.starts_with(s)) || opts.excludes.matches_any_ancestor(p) {
        return false;
    }
    match std::fs::symlink_metadata(p) {
        Err(_) => {
            let id = index.read().unwrap().lookup(p);
            match id {
                Some(id) => index.write().unwrap().remove_subtree(id) > 0,
                None => false,
            }
        }
        Ok(m) => {
            let (Some(parent), Some(name)) = (p.parent(), p.file_name()) else { return false };
            let pid = index.read().unwrap().lookup(parent);
            let pid = match pid {
                Some(id) => id,
                // 부모 폴더도 모름 → 부모부터(부모를 훑으면 이 항목도 들어감)
                None => return apply_path(src, index, roots, parent, opts),
            };
            if !index.read().unwrap().is_dir(pid) {
                return false;
            }
            let link = m.file_type().is_symlink();
            let is_dir = m.is_dir() || (link && std::fs::metadata(p).map(|x| x.is_dir()).unwrap_or(false));
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let name = name.to_string_lossy();
            let (id, created) = index.write().unwrap().upsert_child(pid, &name, is_dir, m.len(), mtime);
            if is_dir && created && !link {
                scan::scan_subtree(src, p, id, index, opts);
            }
            true
        }
    }
}
