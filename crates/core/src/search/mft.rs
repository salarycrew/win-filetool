//! 2b: NTFS 빠른 색인. 루트가 NTFS 볼륨 전체(`C:\`)이고 볼륨을 열 수 있을 때(관리자 권한)만 쓴다.
//!
//! - [`MftSource`]: `$MFT`를 직접 읽어([`super::ntfs::read_mft`]) 폴더 트리를 만든다. 크기·시각은 MFT 레코드에서.
//! - USN 저널: 스캔 직전의 저널 ID·다음 USN을 루트에 적어 두고([`UsnCursor`]), 다음 시작 때 같은 저널이면
//!   그 뒤 변경만 읽어 반영한다([`catch_up`] → [`apply_changes`]). 앱이 켜져 있는 동안도 저널을 따라간다.
//! - 변경 반영은 "그 파일의 지금 상태"를 다시 읽어 맞춘다(레코드 하나 읽기). 그래서 같은 변경을 두 번 반영해도 같다.
//!
//! Windows가 아니면 볼륨을 열 수 없다(이미지 파일로 시험할 때는 [`MftSource::new`]에 직접 넘긴다).

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use super::index::{Index, RootStats, SourceKind, UsnCursor, NONE};
use super::ntfs::{self, frn, frn_rec, MftTable, VolRead, FIRST_USER_REC, ROOT_REC, T_DIR, T_IN_USE, T_LINK};
use super::scan::{IndexSource, RawEntry, ScanControl, ScanOpts};
use super::text::fold;

/// 색인 원본 고르기(CLI `--source`, 설정)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceMode {
    /// NTFS 볼륨 전체이고 열 수 있으면 MFT, 아니면 폴더 훑기
    #[default]
    Auto,
    /// MFT만(안 되면 오류)
    Mft,
    /// 폴더 훑기만(2a)
    Walk,
}

impl SourceMode {
    pub fn parse(s: &str) -> Option<SourceMode> {
        match s {
            "auto" => Some(SourceMode::Auto),
            "mft" => Some(SourceMode::Mft),
            "walk" => Some(SourceMode::Walk),
            _ => None,
        }
    }
}

/// `C:\` 같은 볼륨 루트면 드라이브 문자
pub fn volume_letter(root: &Path) -> Option<char> {
    let s = root.to_string_lossy().replace('/', "\\");
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    let b = s.as_bytes();
    if (b.len() == 3 && b[2] == b'\\' || b.len() == 2) && b[1] == b':' && b[0].is_ascii_alphabetic() {
        Some((b[0] as char).to_ascii_uppercase())
    } else {
        None
    }
}

// ---------- 전체 읽기 ----------

/// MFT를 읽어 루트를 채우는 원본. `cursor`는 부르는 쪽이 읽기 **직전에** 물어 둔 저널 위치(읽는 동안의 변경은 그 뒤에 따라잡는다).
pub struct MftSource {
    pub vol: Arc<dyn VolRead>,
    pub cursor: Option<UsnCursor>,
    /// 읽기·트리 만들기 시간 등(출력용)
    pub last: std::sync::Mutex<Option<MftScanInfo>>,
}

#[derive(Debug, Clone, Default)]
pub struct MftScanInfo {
    pub records_total: u64,
    pub records_in_use: u64,
    pub bad_records: u64,
    pub extension_records: u64,
    pub bytes_read: u64,
    pub read_ms: u64,
    pub tree_ms: u64,
    /// 부모를 찾지 못해(지워진 폴더 아래 등) 빠진 이름
    pub orphans: u64,
}

impl MftSource {
    pub fn new(vol: Arc<dyn VolRead>, cursor: Option<UsnCursor>) -> Self {
        MftSource { vol, cursor, last: std::sync::Mutex::new(None) }
    }
}

impl IndexSource for MftSource {
    fn name(&self) -> &'static str {
        "MFT"
    }

    fn scan(&self, dir: &Path, dir_id: u32, target: &RwLock<Index>, opts: &ScanOpts, ctl: &ScanControl) -> RootStats {
        let t0 = Instant::now();
        let threads = if opts.threads == 0 { std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8) } else { opts.threads };
        let table = match ntfs::read_mft(&*self.vol, threads, &ctl.cancel) {
            Ok(t) => t,
            Err(e) => {
                // 읽다 실패하면 폴더 훑기로(관리자 권한이니 대부분 열린다)
                if ctl.cancel.load(Ordering::Relaxed) {
                    return RootStats::default();
                }
                *self.last.lock().unwrap() = None;
                eprintln!("MFT 읽기 실패({e}) → 폴더 훑기");
                return super::scan::DirWalkSource.scan(dir, dir_id, target, opts, ctl);
            }
        };
        let t1 = Instant::now();
        let mut info = MftScanInfo {
            records_total: table.len() as u64,
            records_in_use: table.in_use,
            bad_records: table.bad,
            extension_records: table.extensions,
            bytes_read: table.bytes_read,
            read_ms: table.read_ms,
            ..Default::default()
        };
        let mut st = build_tree(&table, dir, dir_id, target, opts, ctl, &mut info.orphans);
        info.tree_ms = t1.elapsed().as_millis() as u64;
        st.millis = t0.elapsed().as_millis() as u64;
        {
            let mut ix = target.write().unwrap();
            if let Some(r) = ix.roots_mut().iter_mut().find(|r| r.id == dir_id) {
                r.source = SourceKind::Mft;
                r.usn = self.cursor;
            }
        }
        *self.last.lock().unwrap() = Some(info);
        st
    }
}

/// 레코드 표 → 색인 트리. 루트 폴더(레코드 5)부터 너비 우선으로 넣는다(부모가 먼저 들어감).
/// 메타데이터 파일(레코드 16 아래: `$MFT`, `$Extend` 등과 그 아래)은 넣지 않는다. 제외 규칙·`skip`은 폴더 훑기와 같다.
pub fn build_tree(t: &MftTable, root: &Path, root_id: u32, target: &RwLock<Index>, opts: &ScanOpts, ctl: &ScanControl, orphans: &mut u64) -> RootStats {
    let n = t.len();
    // 부모 → 이름들(CSR)
    let mut count = vec![0u32; n + 1];
    let ok = |l: &ntfs::Link| {
        let r = l.rec as usize;
        (l.rec as u64) >= FIRST_USER_REC && r < n && t.flags[r] & T_IN_USE != 0 && t.in_use_dir(l.parent_rec as usize, l.parent_seq)
    };
    for l in &t.links {
        if ok(l) {
            count[l.parent_rec as usize + 1] += 1;
        } else if (l.rec as u64) >= FIRST_USER_REC && (l.rec as usize) < n && t.flags[l.rec as usize] & T_IN_USE != 0 {
            *orphans += 1;
        }
    }
    for i in 0..n {
        count[i + 1] += count[i];
    }
    let mut fill = count.clone();
    let mut kids = vec![0u32; count[n] as usize];
    for (i, l) in t.links.iter().enumerate() {
        if ok(l) {
            let p = l.parent_rec as usize;
            kids[fill[p] as usize] = i as u32;
            fill[p] += 1;
        }
    }
    let mut st = RootStats::default();
    let mut queue: std::collections::VecDeque<(usize, u32, PathBuf)> = std::collections::VecDeque::new();
    if (ROOT_REC as usize) < n {
        queue.push_back((ROOT_REC as usize, root_id, root.to_path_buf()));
    }
    while let Some((rec, id, path)) = queue.pop_front() {
        if ctl.cancel.load(Ordering::Relaxed) {
            break;
        }
        let range = count[rec] as usize..count[rec + 1] as usize;
        if range.is_empty() {
            continue;
        }
        let mut ix = target.write().unwrap();
        for &li in &kids[range] {
            let l = &t.links[li as usize];
            let r = l.rec as usize;
            let name = t.link_name(l);
            let f = t.flags[r];
            let is_dir = f & T_DIR != 0;
            let link = f & T_LINK != 0;
            let fr = frn(r as u64, t.seq[r]);
            if is_dir && !link {
                let child = path.join(name);
                if opts.excludes.matches(&child) || opts.skip.iter().any(|s| s == &child) {
                    st.excluded += 1;
                    continue;
                }
                let cid = ix.push_child_frn(id, name, true, 0, t.mtime[r], fr);
                st.dirs += 1;
                queue.push_back((r, cid, child));
            } else {
                if link {
                    st.reparse += 1;
                }
                ix.push_child_frn(id, name, is_dir, t.size[r], t.mtime[r], fr);
                if is_dir {
                    st.dirs += 1;
                } else {
                    st.files += 1;
                }
            }
            ctl.entries.fetch_add(1, Ordering::Relaxed);
        }
    }
    st
}

// ---------- 저널 ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JournalInfo {
    pub journal_id: u64,
    pub first_usn: i64,
    pub next_usn: i64,
}

#[derive(Debug)]
pub enum JournalErr {
    /// 저널이 없거나 다시 만들어졌거나, 저장한 위치가 이미 지워짐 → MFT 전체 다시 읽기
    Reset(String),
    Io(io::Error),
}

impl std::fmt::Display for JournalErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JournalErr::Reset(m) => write!(f, "저널이 바뀜: {m}"),
            JournalErr::Io(e) => write!(f, "저널 읽기 오류: {e}"),
        }
    }
}

/// 볼륨의 지금 상태(레코드 하나·폴더 목록)와 USN 저널
pub trait VolumeView: Send + Sync {
    fn serial(&self) -> u64;
    fn query_journal(&self) -> Result<JournalInfo, JournalErr>;
    /// `start`부터 읽은 레코드와 다음 위치. 더 없으면 빈 목록
    fn read_journal(&self, journal_id: u64, start: i64) -> Result<(i64, Vec<ntfs::UsnRec>), JournalErr>;
    /// 파일 참조 번호의 지금 상태. 없거나(지워짐) 순번이 다르면(레코드가 다른 파일에 다시 쓰임) None
    fn record(&self, frn: u64) -> Option<FileState>;
    /// 폴더 목록 + 각 항목의 FRN(새로 나타난 폴더 아래를 채울 때)
    fn list_dir(&self, path: &Path) -> io::Result<Vec<(u64, RawEntry)>>;
}

/// 파일 하나의 지금 상태(MFT 레코드에서)
#[derive(Debug, Clone, PartialEq)]
pub struct FileState {
    pub frn: u64,
    pub is_dir: bool,
    pub link: bool,
    pub size: u64,
    pub mtime: i64,
    /// (부모 FRN, 이름) — DOS 이름 뺀 것
    pub names: Vec<(u64, String)>,
}

impl FileState {
    /// 레코드(+ 확장 레코드들)에서
    pub fn from_records(frn: u64, base: &ntfs::Record, ext: &[ntfs::Record]) -> FileState {
        let mut size = base.size;
        let mut names: Vec<ntfs::FileName> = base.names.clone();
        for e in ext {
            if size.is_none() {
                size = e.size;
            }
            names.extend(e.names.iter().cloned());
        }
        FileState {
            frn,
            is_dir: base.is_dir,
            link: base.reparse_tag.map(ntfs::is_name_surrogate).unwrap_or(false),
            size: size.unwrap_or(0),
            mtime: base.mtime.map(ntfs::filetime_to_unix).unwrap_or(0),
            names: ntfs::visible_names(&names).into_iter().map(|n| (n.parent, n.name.clone())).collect(),
        }
    }
}

/// 저장한 위치부터 지금까지의 변경(바뀐 FRN, 처음 나온 순서). (FRN들, 새 커서, 읽은 레코드 수)
pub fn catch_up(view: &dyn VolumeView, cur: &UsnCursor) -> Result<(Vec<u64>, UsnCursor, u64), JournalErr> {
    if view.serial() != cur.volume_serial {
        return Err(JournalErr::Reset("볼륨 일련번호가 다름".into()));
    }
    let info = view.query_journal()?;
    if info.journal_id != cur.journal_id {
        return Err(JournalErr::Reset("저널 ID가 다름(다시 만들어짐)".into()));
    }
    if cur.next_usn < info.first_usn {
        return Err(JournalErr::Reset("저장한 위치가 이미 지워짐(저널이 한 바퀴 돎)".into()));
    }
    let mut seen = HashSet::new();
    let mut frns = Vec::new();
    let mut usn = cur.next_usn;
    let mut n = 0u64;
    while usn < info.next_usn {
        let (next, recs) = view.read_journal(info.journal_id, usn)?;
        if recs.is_empty() || next <= usn {
            usn = next.max(usn);
            break;
        }
        for r in recs {
            n += 1;
            if seen.insert(r.frn) {
                frns.push(r.frn);
            }
        }
        usn = next;
    }
    Ok((frns, UsnCursor { next_usn: usn, ..*cur }, n))
}

// ---------- 변경 반영 ----------

/// FRN(레코드 번호) → 색인 번호. 하드 링크는 `more`에. 색인 세대(`generation`)가 바뀌면 다시 만든다.
/// 새로 만들 때는 [`FrnMap::empty`](세대를 맞지 않게 해 두어 처음 반영 때 만들어짐) — `Default`는 세대 0이라 쓰지 않는다.
#[derive(Debug, Default)]
pub struct FrnMap {
    pub generation: u64,
    pub root: u32,
    first: Vec<u32>,
    more: HashMap<u64, Vec<u32>>,
}

impl FrnMap {
    /// 빈 표(처음 반영할 때 만든다)
    pub fn empty(root: u32) -> FrnMap {
        FrnMap { root, generation: u64::MAX, ..Default::default() }
    }

    /// 루트 `root` 아래 항목들로 만든다(번호 순서 = 부모가 먼저라 한 번에 루트를 안다)
    pub fn build(ix: &Index, root: u32) -> FrnMap {
        let n = ix.len();
        let mut root_of = vec![NONE; n];
        let mut m = FrnMap { generation: ix.generation, root, ..Default::default() };
        for id in 0..n as u32 {
            let p = ix.parent(id);
            let r = if p == NONE { id } else { root_of[p as usize] };
            root_of[id as usize] = r;
            if r == root && ix.is_live(id) && ix.frn(id) != 0 {
                m.add(ix, ix.frn(id), id);
            }
        }
        m
    }

    fn add(&mut self, ix: &Index, f: u64, id: u32) {
        let rec = frn_rec(f) as usize;
        if rec >= self.first.len() {
            self.first.resize(rec + 1024, NONE);
        }
        let cur = self.first[rec];
        if cur == NONE || cur == id || !ix.is_live(cur) || ix.frn(cur) != f {
            self.first[rec] = id;
        } else {
            let v = self.more.entry(rec as u64).or_default();
            v.retain(|&x| ix.is_live(x) && x != id);
            v.push(id);
        }
    }

    /// 레코드 번호에 걸린 살아 있는 항목(순번 상관없이)
    fn ids_for_rec(&self, ix: &Index, rec: u64) -> Vec<u32> {
        let mut v = Vec::new();
        if let Some(&x) = self.first.get(rec as usize) {
            if x != NONE && ix.is_live(x) && frn_rec(ix.frn(x)) == rec {
                v.push(x);
            }
        }
        if let Some(m) = self.more.get(&rec) {
            for &x in m {
                if ix.is_live(x) && frn_rec(ix.frn(x)) == rec && !v.contains(&x) {
                    v.push(x);
                }
            }
        }
        v
    }

    /// 이 FRN인 살아 있는 폴더
    fn dir(&self, ix: &Index, f: u64) -> Option<u32> {
        if frn_rec(f) == ROOT_REC {
            return Some(self.root);
        }
        self.ids_for_rec(ix, frn_rec(f)).into_iter().find(|&x| ix.frn(x) == f && ix.is_dir(x))
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ApplyStats {
    pub files: usize,
    pub added: usize,
    pub removed: usize,
    pub updated: usize,
    pub moved: usize,
    /// 부모를 끝내 찾지 못한 것(제외한 폴더 아래 등)
    pub unresolved: usize,
}

impl ApplyStats {
    pub fn changed(&self) -> usize {
        self.added + self.removed + self.updated + self.moved
    }
}

/// 바뀐 FRN들을 색인에 반영한다. 상태는 먼저(잠금 없이) 읽어 두고 쓰기 잠금 안에서 맞춘다.
pub fn apply_changes(index: &RwLock<Index>, map: &mut FrnMap, root_path: &Path, frns: &[u64], view: &dyn VolumeView, opts: &ScanOpts) -> ApplyStats {
    // 같은 FRN은 한 번만(처음 나온 순서). 저널 따라가기는 묶음마다 중복을 그대로 넘긴다
    let mut seen = HashSet::new();
    let states: Vec<(u64, Option<FileState>)> =
        frns.iter().filter(|&&f| frn_rec(f) >= FIRST_USER_REC && seen.insert(f)).map(|&f| (f, view.record(f))).collect();
    let mut ix = index.write().unwrap();
    if map.generation != ix.generation {
        *map = FrnMap::build(&ix, map.root);
    }
    let mut st = ApplyStats { files: states.len(), ..Default::default() };
    let mut pending: Vec<&(u64, Option<FileState>)> = states.iter().collect();
    for pass in 0..4 {
        let last = pass == 3;
        let mut next = Vec::new();
        for item in &pending {
            if !apply_one(&mut ix, map, root_path, item.0, item.1.as_ref(), view, opts, last, &mut st) {
                next.push(*item);
            }
        }
        if next.is_empty() {
            break;
        }
        if next.len() == pending.len() && !last {
            // 더 나아지지 않음: 마지막 차례로 한 번 더(남은 것은 버림)
            for item in &next {
                apply_one(&mut ix, map, root_path, item.0, item.1.as_ref(), view, opts, true, &mut st);
            }
            break;
        }
        pending = next;
    }
    st
}

/// 하나 맞추기. 부모를 아직 모르면(나중에 만들어진 폴더 등) false(다음 차례에 다시)
#[allow(clippy::too_many_arguments)]
fn apply_one(ix: &mut Index, map: &mut FrnMap, root_path: &Path, f: u64, s: Option<&FileState>, view: &dyn VolumeView, opts: &ScanOpts, last: bool, st: &mut ApplyStats) -> bool {
    let rec = frn_rec(f);
    let cur = map.ids_for_rec(ix, rec);
    // 같은 레코드의 옛 파일(순번이 다름) → 지워진 것
    let (mine, stale): (Vec<u32>, Vec<u32>) = cur.into_iter().partition(|&x| ix.frn(x) == f);
    for x in stale {
        if ix.is_live(x) {
            st.removed += ix.remove_subtree(x).min(1);
        }
    }
    if s.is_none() && std::env::var_os("WFT_DEBUG").is_some() {
        eprintln!("[apply] frn {f:#x} 레코드 없음/순번 다름 → 지움 {:?}", mine.iter().map(|&x| ix.full_path(x)).collect::<Vec<_>>());
    }
    let Some(s) = s else {
        for x in mine {
            if ix.is_live(x) {
                st.removed += ix.remove_subtree(x).min(1);
            }
        }
        return true;
    };
    // 있어야 할 자리: (부모 번호, 이름)
    let mut want: Vec<(u32, String)> = Vec::new();
    let mut missing_parent = false;
    for (pf, name) in &s.names {
        match map.dir(ix, *pf) {
            Some(pid) => {
                if s.is_dir && !s.link {
                    let p = ix.full_path(pid).join(name);
                    if opts.excludes.matches(&p) || opts.skip.iter().any(|x| x == &p) || opts.excludes.matches_any_ancestor(&p) {
                        continue;
                    }
                }
                want.push((pid, name.clone()));
            }
            None => missing_parent = true,
        }
    }
    if missing_parent && !last {
        return false;
    }
    let mut unmatched_old = Vec::new();
    let mut taken = vec![false; want.len()];
    for x in mine {
        if !ix.is_live(x) {
            continue;
        }
        let k = ix.key(x).to_vec();
        let px = ix.parent(x);
        match want.iter().enumerate().position(|(i, (p, n))| !taken[i] && *p == px && fold(n).as_bytes() == k.as_slice()) {
            Some(i) if ix.is_dir(x) == s.is_dir => {
                taken[i] = true;
                if ix.name(x) != want[i].1 || ix.size(x) != (if s.is_dir { 0 } else { s.size }) || ix.mtime(x) != s.mtime {
                    ix.update_entry(x, &want[i].1, s.size, s.mtime);
                    st.updated += 1;
                }
            }
            _ => unmatched_old.push(x),
        }
    }
    let mut free: Vec<usize> = (0..want.len()).filter(|&i| !taken[i]).collect();
    for x in unmatched_old {
        if s.is_dir && ix.is_dir(x) && !free.is_empty() {
            // 폴더 이동·이름 바꾸기: 아래 항목째 옮긴다
            let i = free.remove(0);
            let pairs = ix.relocate(x, want[i].0, &want[i].1);
            for (_, nid) in &pairs {
                let fr = ix.frn(*nid);
                if fr != 0 {
                    map.add(ix, fr, *nid);
                }
            }
            if let Some((_, nid)) = pairs.first() {
                ix.update_entry(*nid, &want[i].1, 0, s.mtime);
            }
            st.moved += 1;
        } else {
            st.removed += ix.remove_subtree(x).min(1);
        }
    }
    for i in free {
        let (pid, name) = (want[i].0, want[i].1.clone());
        // 같은 이름의 다른 항목(FRN 없음·다른 FRN)이 남아 있으면 지운다
        if let Some(old) = ix.find_child(pid, &name) {
            if ix.frn(old) != f {
                ix.remove_subtree(old);
            }
        }
        let nid = ix.push_child_frn(pid, &name, s.is_dir, s.size, s.mtime, f);
        map.add(ix, f, nid);
        st.added += 1;
        if s.is_dir && !s.link {
            // 다른 곳(제외한 폴더 등)에서 들어온 폴더면 아래가 저널에 안 나온다 → 목록으로 채움
            let p = ix.full_path(nid);
            fill_dir(ix, map, view, &p, nid, opts, st);
        }
    }
    let _ = root_path;
    true
}

/// 새로 나타난 폴더 아래를 폴더 목록(FRN 포함)으로 채운다
fn fill_dir(ix: &mut Index, map: &mut FrnMap, view: &dyn VolumeView, path: &Path, id: u32, opts: &ScanOpts, st: &mut ApplyStats) {
    let mut stack = vec![(path.to_path_buf(), id)];
    while let Some((p, id)) = stack.pop() {
        let Ok(list) = view.list_dir(&p) else { continue };
        for (f, e) in list {
            if ix.find_child(id, &e.name).is_some() {
                continue;
            }
            let child = p.join(&e.name);
            if e.is_dir && !e.link && (opts.excludes.matches(&child) || opts.skip.iter().any(|s| s == &child)) {
                continue;
            }
            let cid = ix.push_child_frn(id, &e.name, e.is_dir, e.size, e.mtime, f);
            if f != 0 {
                map.add(ix, f, cid);
            }
            st.added += 1;
            if e.is_dir && !e.link {
                stack.push((child, cid));
            }
        }
    }
}

// ---------- Windows 볼륨 ----------

/// 볼륨을 열 수 없는 이유(한국어 문장)
#[derive(Debug, Clone, PartialEq)]
pub enum OpenErr {
    NotVolumeRoot,
    NotNtfs(String),
    NotAdmin,
    Other(String),
}

impl std::fmt::Display for OpenErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenErr::NotVolumeRoot => write!(f, "드라이브 전체(예: C:\\)가 아니라 폴더라서 빠른 색인(MFT)을 쓸 수 없습니다"),
            OpenErr::NotNtfs(fs) => write!(f, "NTFS가 아니라({fs}) 빠른 색인(MFT)을 쓸 수 없습니다"),
            OpenErr::NotAdmin => write!(f, "관리자 권한이 없어 볼륨을 열 수 없습니다(빠른 색인(MFT)은 관리자 권한으로 실행해야 합니다)"),
            OpenErr::Other(m) => write!(f, "볼륨을 열 수 없습니다: {m}"),
        }
    }
}

#[cfg(windows)]
pub use win::Volume;

/// 루트에 쓸 볼륨 열기(Windows, 관리자 권한)
#[cfg(windows)]
pub fn open_volume(root: &Path) -> Result<Arc<Volume>, OpenErr> {
    Volume::open(root).map(Arc::new)
}

/// Windows가 아니면 언제나 폴더 훑기
#[cfg(not(windows))]
pub struct Volume;

#[cfg(not(windows))]
pub fn open_volume(root: &Path) -> Result<Arc<Volume>, OpenErr> {
    if volume_letter(root).is_none() {
        return Err(OpenErr::NotVolumeRoot);
    }
    Err(OpenErr::Other("Windows가 아님".into()))
}

#[cfg(not(windows))]
impl Volume {
    pub fn reader(self: &Arc<Self>) -> Arc<dyn VolRead> {
        unreachable!()
    }
    pub fn view(self: &Arc<Self>) -> Arc<dyn VolumeView> {
        unreachable!()
    }
    pub fn cursor(&self) -> Option<UsnCursor> {
        None
    }
}

#[cfg(windows)]
mod win {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ACCESS_DENIED, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo, GetFileInformationByHandleEx, GetVolumeInformationW,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const GENERIC_READ: u32 = 0x8000_0000;
    const FSCTL_QUERY_USN_JOURNAL: u32 = 0x0009_00f4;
    const FSCTL_READ_USN_JOURNAL: u32 = 0x0009_00bb;
    const FSCTL_GET_NTFS_FILE_RECORD: u32 = 0x0009_0068;
    const ERROR_JOURNAL_NOT_ACTIVE: u32 = 1179;
    const ERROR_JOURNAL_DELETE_IN_PROGRESS: u32 = 1178;
    const ERROR_JOURNAL_ENTRY_DELETED: u32 = 1181;
    const ERROR_NO_MORE_FILES: u32 = 18;

    pub struct Volume {
        file: std::fs::File,
        handle: HANDLE,
        pub letter: char,
        pub serial: u64,
        pub record_size: u32,
    }
    unsafe impl Send for Volume {}
    unsafe impl Sync for Volume {}

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }

    impl Volume {
        pub fn open(root: &Path) -> Result<Volume, OpenErr> {
            let letter = volume_letter(root).ok_or(OpenErr::NotVolumeRoot)?;
            let mut fs = [0u16; 64];
            let r = wide(&format!("{letter}:\\"));
            let ok = unsafe {
                GetVolumeInformationW(r.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), fs.as_mut_ptr(), fs.len() as u32)
            };
            if ok == 0 {
                return Err(OpenErr::Other(format!("드라이브 정보를 읽을 수 없음(오류 {})", unsafe { GetLastError() })));
            }
            let fsname = String::from_utf16_lossy(&fs[..fs.iter().position(|&c| c == 0).unwrap_or(0)]);
            if fsname != "NTFS" {
                return Err(OpenErr::NotNtfs(fsname));
            }
            let p = wide(&format!(r"\\.\{letter}:"));
            let open = |flags: u32| unsafe {
                CreateFileW(p.as_ptr(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, std::ptr::null(), OPEN_EXISTING, flags, std::ptr::null_mut())
            };
            let mut h = open(0);
            // 2c 서비스(가상 계정 + 백업 권한)는 백업 의미로 열어 본다
            if h == INVALID_HANDLE_VALUE && unsafe { GetLastError() } == ERROR_ACCESS_DENIED {
                h = open(FILE_FLAG_BACKUP_SEMANTICS);
            }
            if h == INVALID_HANDLE_VALUE {
                let e = unsafe { GetLastError() };
                return Err(if e == ERROR_ACCESS_DENIED { OpenErr::NotAdmin } else { OpenErr::Other(format!("오류 {e}")) });
            }
            let file = unsafe { std::fs::File::from_raw_handle(h as _) };
            let boot = ntfs::read_aligned(&file, 0, 512, 4096).map_err(|e| OpenErr::Other(e.to_string()))?;
            let g = ntfs::parse_boot(&boot).ok_or_else(|| OpenErr::NotNtfs("부트 섹터".into()))?;
            Ok(Volume { file, handle: h, letter, serial: g.serial, record_size: g.record_size })
        }

        pub fn reader(self: &Arc<Self>) -> Arc<dyn VolRead> {
            self.clone()
        }

        pub fn view(self: &Arc<Self>) -> Arc<dyn VolumeView> {
            self.clone()
        }

        /// 지금 저널 위치(저널이 없으면 None — 그때는 실시간 갱신을 ReadDirectoryChangesW로)
        pub fn cursor(&self) -> Option<UsnCursor> {
            self.query_journal().ok().map(|j| UsnCursor { volume_serial: self.serial, journal_id: j.journal_id, next_usn: j.next_usn })
        }

        fn ioctl(&self, code: u32, input: &[u8], out: &mut [u8]) -> Result<usize, u32> {
            let mut got = 0u32;
            let ok = unsafe {
                DeviceIoControl(
                    self.handle,
                    code,
                    input.as_ptr() as *const _,
                    input.len() as u32,
                    out.as_mut_ptr() as *mut _,
                    out.len() as u32,
                    &mut got,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                Err(unsafe { GetLastError() })
            } else {
                Ok(got as usize)
            }
        }

        fn raw_record(&self, rec: u64) -> Option<Vec<u8>> {
            let mut out = vec![0u8; 12 + self.record_size as usize + 64];
            let n = self.ioctl(FSCTL_GET_NTFS_FILE_RECORD, &rec.to_le_bytes(), &mut out).ok()?;
            // NTFS_FILE_RECORD_OUTPUT_BUFFER: FileReferenceNumber i64, FileRecordLength u32, FileRecordBuffer
            let got = u64::from_le_bytes(out[0..8].try_into().unwrap());
            // 쓰지 않는 레코드를 물으면 그 아래의 쓰는 레코드를 준다 → 번호가 다르면 없는 것
            if frn_rec(got) != rec {
                return None;
            }
            let len = u32::from_le_bytes(out[8..12].try_into().unwrap()) as usize;
            if 12 + len > n {
                return None;
            }
            Some(out[12..12 + len].to_vec())
        }
    }

    impl VolRead for Volume {
        fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<()> {
            self.file.read_at(off, buf)
        }
    }

    impl VolumeView for Volume {
        fn serial(&self) -> u64 {
            self.serial
        }

        fn query_journal(&self) -> Result<JournalInfo, JournalErr> {
            let mut out = [0u8; 80];
            match self.ioctl(FSCTL_QUERY_USN_JOURNAL, &[], &mut out) {
                Ok(_) => Ok(JournalInfo {
                    journal_id: u64::from_le_bytes(out[0..8].try_into().unwrap()),
                    first_usn: i64::from_le_bytes(out[8..16].try_into().unwrap()),
                    next_usn: i64::from_le_bytes(out[16..24].try_into().unwrap()),
                }),
                Err(e) if e == ERROR_JOURNAL_NOT_ACTIVE || e == ERROR_JOURNAL_DELETE_IN_PROGRESS => Err(JournalErr::Reset(format!("저널 없음(오류 {e})"))),
                Err(e) => Err(JournalErr::Io(io::Error::from_raw_os_error(e as i32))),
            }
        }

        fn read_journal(&self, journal_id: u64, start: i64) -> Result<(i64, Vec<ntfs::UsnRec>), JournalErr> {
            // READ_USN_JOURNAL_DATA_V1: StartUsn i64, ReasonMask u32, ReturnOnlyOnClose u32, Timeout u64,
            // BytesToWaitFor u64, UsnJournalID u64, MinMajorVersion u16, MaxMajorVersion u16
            let mut inp = Vec::with_capacity(44);
            inp.extend_from_slice(&start.to_le_bytes());
            inp.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
            inp.extend_from_slice(&0u32.to_le_bytes());
            inp.extend_from_slice(&0u64.to_le_bytes());
            inp.extend_from_slice(&0u64.to_le_bytes());
            inp.extend_from_slice(&journal_id.to_le_bytes());
            inp.extend_from_slice(&2u16.to_le_bytes());
            inp.extend_from_slice(&3u16.to_le_bytes());
            inp.resize(48, 0);
            let mut out = vec![0u8; 1 << 20];
            match self.ioctl(FSCTL_READ_USN_JOURNAL, &inp[..44], &mut out) {
                Ok(n) => Ok(ntfs::parse_usn_buffer(&out[..n])),
                Err(e) if e == ERROR_JOURNAL_ENTRY_DELETED || e == ERROR_JOURNAL_NOT_ACTIVE || e == ERROR_JOURNAL_DELETE_IN_PROGRESS => {
                    Err(JournalErr::Reset(format!("오류 {e}")))
                }
                Err(e) => Err(JournalErr::Io(io::Error::from_raw_os_error(e as i32))),
            }
        }

        fn record(&self, f: u64) -> Option<FileState> {
            let mut b = self.raw_record(frn_rec(f))?;
            let r = ntfs::parse_cached_record(&mut b).ok()?;
            if !r.in_use || r.base != 0 || r.seq != ntfs::frn_seq(f) {
                return None;
            }
            let mut ext = Vec::new();
            if let Some(al) = &r.attr_list {
                let list = match al {
                    ntfs::AttrListLoc::Resident(v) => v.clone(),
                    ntfs::AttrListLoc::NonResident { runs, size } => {
                        // 클러스터 크기를 몰라도 되게 볼륨 기하를 다시 읽는다(드묾)
                        let boot = ntfs::read_aligned(self, 0, 512, 4096).ok()?;
                        let g = ntfs::parse_boot(&boot)?;
                        ntfs::read_runs(self, runs, *size, g.cluster as u64, (g.bytes_per_sector as u64).max(512)).ok()?
                    }
                };
                let mut segs: Vec<u64> = ntfs::parse_attr_list(&list)
                    .into_iter()
                    .filter(|e| (e.ty == ntfs::ATTR_FILE_NAME || (e.ty == ntfs::ATTR_DATA && e.name_len == 0)) && frn_rec(e.segment) != frn_rec(f))
                    .map(|e| frn_rec(e.segment))
                    .collect();
                segs.sort_unstable();
                segs.dedup();
                for s in segs {
                    if let Some(mut eb) = self.raw_record(s) {
                        if let Ok(er) = ntfs::parse_cached_record(&mut eb) {
                            if er.in_use && frn_rec(er.base) == frn_rec(f) {
                                ext.push(er);
                            }
                        }
                    }
                }
            }
            Some(FileState::from_records(f, &r, &ext))
        }

        fn list_dir(&self, path: &Path) -> io::Result<Vec<(u64, RawEntry)>> {
            let p = super::super::scan::long_path_wide(path, "");
            let h = unsafe {
                CreateFileW(
                    p.as_ptr(),
                    FILE_LIST_DIRECTORY,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS,
                    std::ptr::null_mut(),
                )
            };
            if h == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let mut out = Vec::new();
            let mut buf = vec![0u64; 8192]; // 64 KB, 8바이트 정렬
            let mut class = FileIdBothDirectoryRestartInfo;
            loop {
                let ok = unsafe { GetFileInformationByHandleEx(h, class, buf.as_mut_ptr() as *mut _, (buf.len() * 8) as u32) };
                class = FileIdBothDirectoryInfo;
                if ok == 0 {
                    let e = unsafe { GetLastError() };
                    unsafe { CloseHandle(h) };
                    if e == ERROR_NO_MORE_FILES {
                        break;
                    }
                    return Err(io::Error::from_raw_os_error(e as i32));
                }
                let b: &[u8] = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, buf.len() * 8) };
                let mut o = 0usize;
                loop {
                    let e = &b[o..];
                    let next = u32::from_le_bytes(e[0..4].try_into().unwrap()) as usize;
                    let wt = u64::from_le_bytes(e[24..32].try_into().unwrap());
                    let eof = u64::from_le_bytes(e[40..48].try_into().unwrap());
                    let attr = u32::from_le_bytes(e[56..60].try_into().unwrap());
                    let nl = u32::from_le_bytes(e[60..64].try_into().unwrap()) as usize;
                    let tag = u32::from_le_bytes(e[64..68].try_into().unwrap());
                    let id = u64::from_le_bytes(e[96..104].try_into().unwrap());
                    let w: Vec<u16> = e[104..104 + nl].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                    let name = String::from_utf16_lossy(&w);
                    if name != "." && name != ".." {
                        let is_dir = attr & FILE_ATTRIBUTE_DIRECTORY != 0;
                        let link = attr & FILE_ATTRIBUTE_REPARSE_POINT != 0 && ntfs::is_name_surrogate(tag);
                        out.push((id, RawEntry { name, is_dir, size: if is_dir { 0 } else { eof }, mtime: ntfs::filetime_to_unix(wt), link }));
                    }
                    if next == 0 {
                        break;
                    }
                    o += next;
                }
            }
            Ok(out)
        }
    }
}

/// 루트에 맞는 원본 고르기 결과
pub enum Plan {
    Walk { reason: Option<String> },
    Mft(Arc<Volume>),
}

/// 루트 하나에 쓸 원본. `Mft` 모드인데 안 되면 Err(한국어 이유)
pub fn plan_for(root: &Path, mode: SourceMode) -> Result<Plan, String> {
    match mode {
        SourceMode::Walk => Ok(Plan::Walk { reason: None }),
        SourceMode::Auto => Ok(match open_volume(root) {
            Ok(v) => Plan::Mft(v),
            Err(e) => Plan::Walk { reason: Some(e.to_string()) },
        }),
        SourceMode::Mft => open_volume(root).map(Plan::Mft).map_err(|e| format!("{}: {e}", root.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters() {
        assert_eq!(volume_letter(Path::new(r"C:\")), Some('C'));
        assert_eq!(volume_letter(Path::new(r"d:")), Some('D'));
        assert_eq!(volume_letter(Path::new(r"\\?\E:\")), Some('E'));
        assert_eq!(volume_letter(Path::new(r"C:\Users")), None);
        assert_eq!(volume_letter(Path::new("/home")), None);
    }
}
