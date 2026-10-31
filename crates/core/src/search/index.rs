//! 메모리 색인: 항목마다 `PathBuf`를 두지 않고, 이름은 문자열 창고(arena) 하나에 이어 붙이고
//! 부모 번호(u32)로 트리를 만든다. 열(column)별 배열이라 이름 훑기가 빠르다.
//!
//! - 항목 번호는 0부터. 부모 번호는 언제나 자식보다 작다(스캔·실시간 추가 모두 부모를 먼저 넣음).
//!   그래서 번호 순서대로 한 번 돌면 폴더 경로를 만들 수 있다.
//! - 지운 항목은 표시만 하고(F_DELETED) 다음 정리([`Index::compacted`]) 때 빠진다.
//! - 루트(드라이브·지정 폴더)도 항목이다(F_ROOT). 이름은 경로 그대로(`C:\`), 검색 결과에는 나오지 않는다.
//!
//! 파일 형식(버전 2, 리틀 엔디언): `WFTIDX01` · 버전 u32 · 만든 시각 i64 · 루트 수 u32 · 루트들
//! (경로, 번호, 마지막 스캔, 통계 6개, **v2: 원본 u8 · USN 커서 있음 u8 · 볼륨 일련번호 u64 · 저널 ID u64 · 다음 USN i64**) ·
//! 항목 수 u32 · parent[u32] · flags[u8] · size[u64] · mtime[i64] · name_len[u16] · 이름 바이트 ·
//! **v2: FRN 열 있음 u8 (+ frn[u64])** · FNV-1a 64 검사합. 버전 1 파일도 읽는다(원본 = 폴더 훑기, FRN 없음).
//! 비교 키는 저장하지 않고 읽을 때 다시 만든다(파일을 작게).

use std::io::{BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use super::text::{fold, fold_into};
use crate::error::{io_err, Error, Result};

pub const NONE: u32 = u32::MAX;
pub const F_DIR: u8 = 1;
pub const F_DELETED: u8 = 2;
pub const F_ROOT: u8 = 4;

const MAGIC: &[u8; 8] = b"WFTIDX01";
pub const FORMAT_VERSION: u32 = 2;
pub const INDEX_FILE: &str = "index-v2.bin";
/// 2a가 쓰던 파일. v2 파일이 없으면 이것을 읽고, v2를 저장하면 지운다
pub const INDEX_FILE_V1: &str = "index-v1.bin";

/// 루트를 채운 길
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum SourceKind {
    /// 폴더 훑기(2a, 관리자 권한 불필요)
    #[default]
    Walk,
    /// NTFS MFT 직접 읽기 + USN 저널(2b, 관리자 권한)
    Mft,
}

impl SourceKind {
    pub fn label(self) -> &'static str {
        match self {
            SourceKind::Walk => "일반 색인",
            SourceKind::Mft => "빠른 색인(MFT)",
        }
    }
}

/// USN 저널 위치: 다음 시작 때 여기서부터 바뀐 것만 읽는다
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct UsnCursor {
    pub volume_serial: u64,
    pub journal_id: u64,
    pub next_usn: i64,
}

/// 루트 하나를 스캔한 결과 요약
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize, PartialEq)]
pub struct RootStats {
    pub dirs: u64,
    pub files: u64,
    /// 열 수 없어 건너뛴 폴더(권한 없음 등)
    pub denied: u64,
    /// 따라가지 않은 연결 지점(정션·심볼릭 링크)
    pub reparse: u64,
    /// 제외 규칙에 걸린 폴더
    pub excluded: u64,
    pub millis: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RootInfo {
    pub path: PathBuf,
    pub id: u32,
    /// 마지막으로 끝까지 스캔한 시각(유닉스 초). 0 = 아직
    pub last_scan: i64,
    pub stats: RootStats,
    pub source: SourceKind,
    pub usn: Option<UsnCursor>,
}

/// 폴더 경로 키 캐시(경로 낱말 검색·폴더 정렬용). 번호 → 창고 위치
pub struct DirPaths {
    pub version: u64,
    slot: Vec<u32>,
    off: Vec<u32>,
    len: Vec<u32>,
    arena: Vec<u8>,
}

impl DirPaths {
    /// 폴더 `dir`의 전체 경로 키(소문자 NFC, 구분자 `\`)
    pub fn get(&self, dir: u32) -> &str {
        let s = self.slot.get(dir as usize).copied().unwrap_or(NONE);
        if s == NONE {
            return "";
        }
        let (o, l) = (self.off[s as usize] as usize, self.len[s as usize] as usize);
        // 창고에는 UTF-8만 넣는다
        unsafe { std::str::from_utf8_unchecked(&self.arena[o..o + l]) }
    }
}

#[derive(Default)]
pub struct Index {
    names: Vec<u8>,
    keys: Vec<u8>,
    name_off: Vec<u32>,
    name_len: Vec<u16>,
    key_off: Vec<u32>,
    key_len: Vec<u16>,
    parent: Vec<u32>,
    first_child: Vec<u32>,
    next_sib: Vec<u32>,
    size: Vec<u64>,
    mtime: Vec<i64>,
    flags: Vec<u8>,
    /// NTFS 파일 참조 번호(MFT로 채운 항목만, 아니면 0). USN 저널의 변경을 항목에 잇는 데 쓴다
    frn: Vec<u64>,
    roots: Vec<RootInfo>,
    deleted: usize,
    /// 번호가 바뀌면(정리·이어 붙이기) 올라간다. 화면은 이 값이 바뀌면 다시 검색한다
    pub generation: u64,
    /// 내용이 바뀔 때마다 올라간다
    pub version: u64,
    dir_cache: Mutex<Option<Arc<DirPaths>>>,
    /// 이름 순위(정렬용). 만든 뒤 새로 들어온 항목은 순위가 없어 뒤로 가고 그들끼리 이름으로 비교한다
    name_rank: Mutex<Option<Arc<Vec<u32>>>>,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Index({} 항목, 루트 {}, 지움 {})", self.len(), self.roots.len(), self.deleted)
    }
}

/// 루트 경로를 키로: 소문자, 구분자 `\`, 끝 구분자 없음 (`C:\` → `c:`, `/home/a` → `\home\a`)
pub fn root_key(path: &Path) -> String {
    let s = fold(&path.to_string_lossy()).replace('/', "\\");
    s.trim_end_matches('\\').to_string()
}

impl Index {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.flags.len()
    }

    pub fn is_empty(&self) -> bool {
        self.flags.is_empty()
    }

    /// 지우지 않은 항목 수(루트 제외)
    pub fn live_count(&self) -> usize {
        self.flags.len() - self.deleted - self.roots.len()
    }

    pub fn deleted_count(&self) -> usize {
        self.deleted
    }

    pub fn roots(&self) -> &[RootInfo] {
        &self.roots
    }

    pub fn roots_mut(&mut self) -> &mut [RootInfo] {
        &mut self.roots
    }

    pub fn root_by_path(&self, path: &Path) -> Option<&RootInfo> {
        let k = root_key(path);
        self.roots.iter().find(|r| root_key(&r.path) == k)
    }

    fn push_raw(&mut self, parent: u32, name: &str, flags: u8, size: u64, mtime: i64, key: Option<&str>) -> u32 {
        self.push_raw_frn(parent, name, flags, size, mtime, key, 0)
    }

    #[allow(clippy::too_many_arguments)]
    fn push_raw_frn(&mut self, parent: u32, name: &str, flags: u8, size: u64, mtime: i64, key: Option<&str>, frn: u64) -> u32 {
        let id = self.flags.len() as u32;
        self.name_off.push(self.names.len() as u32);
        let nl = name.len().min(u16::MAX as usize);
        self.names.extend_from_slice(&name.as_bytes()[..nl]);
        self.name_len.push(nl as u16);
        let ko = self.keys.len();
        match key {
            Some(k) => self.keys.extend_from_slice(k.as_bytes()),
            None => {
                let mut k = String::new();
                fold_into(name, &mut k);
                self.keys.extend_from_slice(k.as_bytes());
            }
        }
        self.key_off.push(ko as u32);
        self.key_len.push((self.keys.len() - ko).min(u16::MAX as usize) as u16);
        self.parent.push(parent);
        self.first_child.push(NONE);
        self.next_sib.push(NONE);
        self.size.push(size);
        self.mtime.push(mtime);
        self.flags.push(flags);
        self.frn.push(frn);
        if parent != NONE {
            self.next_sib[id as usize] = self.first_child[parent as usize];
            self.first_child[parent as usize] = id;
        }
        self.version += 1;
        id
    }

    /// 루트 추가(이미 있으면 그 번호)
    pub fn add_root(&mut self, path: &Path) -> u32 {
        if let Some(r) = self.root_by_path(path) {
            return r.id;
        }
        let name = path.to_string_lossy().into_owned();
        let key = root_key(path);
        let id = self.push_raw(NONE, &name, F_DIR | F_ROOT, 0, 0, Some(&key));
        self.roots.push(RootInfo {
            path: path.to_path_buf(),
            id,
            last_scan: 0,
            stats: RootStats::default(),
            source: SourceKind::Walk,
            usn: None,
        });
        id
    }

    /// 자식 추가(중복 확인 없음 — 스캔용)
    pub fn push_child(&mut self, parent: u32, name: &str, is_dir: bool, size: u64, mtime: i64) -> u32 {
        self.push_raw(parent, name, if is_dir { F_DIR } else { 0 }, size, mtime, None)
    }

    /// 자식 추가 + NTFS 파일 참조 번호(MFT 원본용)
    pub fn push_child_frn(&mut self, parent: u32, name: &str, is_dir: bool, size: u64, mtime: i64, frn: u64) -> u32 {
        self.push_raw_frn(parent, name, if is_dir { F_DIR } else { 0 }, if is_dir { 0 } else { size }, mtime, None, frn)
    }

    #[inline]
    pub fn frn(&self, id: u32) -> u64 {
        self.frn[id as usize]
    }

    /// FRN이 하나라도 있는지(저장할 때 열을 쓸지)
    pub fn has_frn(&self) -> bool {
        self.frn.iter().any(|&f| f != 0)
    }

    /// 항목의 이름(대소문자만 바뀐 경우)·크기·시각 고치기
    pub fn update_entry(&mut self, id: u32, name: &str, size: u64, mtime: i64) {
        let i = id as usize;
        if self.name(id) != name {
            self.name_off[i] = self.names.len() as u32;
            self.names.extend_from_slice(name.as_bytes());
            self.name_len[i] = name.len().min(u16::MAX as usize) as u16;
            let ko = self.keys.len();
            let mut k = String::new();
            fold_into(name, &mut k);
            self.keys.extend_from_slice(k.as_bytes());
            self.key_off[i] = ko as u32;
            self.key_len[i] = (self.keys.len() - ko).min(u16::MAX as usize) as u16;
        }
        self.size[i] = if self.is_dir(id) { 0 } else { size };
        self.mtime[i] = mtime;
        self.version += 1;
    }

    /// 항목(과 아래)을 다른 부모 밑으로 옮긴다(이름도 바꿈). "부모 번호 < 자식 번호"를 지키려고 끝에 새로 복사하고
    /// 옛 항목은 지움 표시. (옛 번호, 새 번호) 목록을 돌려준다(첫 쌍이 옮긴 항목).
    pub fn relocate(&mut self, id: u32, new_parent: u32, new_name: &str) -> Vec<(u32, u32)> {
        if !self.is_live(id) || !self.is_live(new_parent) || self.flags[id as usize] & F_ROOT != 0 {
            return Vec::new();
        }
        let mut order = vec![id];
        let mut i = 0;
        while i < order.len() {
            let x = order[i];
            order.extend(self.children(x));
            i += 1;
        }
        let mut map: std::collections::HashMap<u32, u32> = std::collections::HashMap::with_capacity(order.len());
        let mut pairs = Vec::with_capacity(order.len());
        for (k, &old) in order.iter().enumerate() {
            let o = old as usize;
            let (np, name) = if k == 0 { (new_parent, new_name.to_string()) } else { (map[&self.parent[o]], self.name(old).to_string()) };
            let f = self.flags[o] & !F_DELETED;
            let nid = self.push_raw_frn(np, &name, f, self.size[o], self.mtime[o], None, self.frn[o]);
            map.insert(old, nid);
            pairs.push((old, nid));
        }
        self.remove_subtree(id);
        pairs
    }

    #[inline]
    pub fn flags(&self, id: u32) -> u8 {
        self.flags[id as usize]
    }
    #[inline]
    pub fn is_dir(&self, id: u32) -> bool {
        self.flags[id as usize] & F_DIR != 0
    }
    #[inline]
    pub fn is_live(&self, id: u32) -> bool {
        (id as usize) < self.flags.len() && self.flags[id as usize] & F_DELETED == 0
    }
    #[inline]
    pub fn parent(&self, id: u32) -> u32 {
        self.parent[id as usize]
    }
    #[inline]
    pub fn size(&self, id: u32) -> u64 {
        self.size[id as usize]
    }
    #[inline]
    pub fn mtime(&self, id: u32) -> i64 {
        self.mtime[id as usize]
    }
    #[inline]
    pub fn name(&self, id: u32) -> &str {
        let o = self.name_off[id as usize] as usize;
        let l = self.name_len[id as usize] as usize;
        std::str::from_utf8(&self.names[o..o + l]).unwrap_or("")
    }
    #[inline]
    pub fn key(&self, id: u32) -> &[u8] {
        let o = self.key_off[id as usize] as usize;
        let l = self.key_len[id as usize] as usize;
        &self.keys[o..o + l]
    }
    #[inline]
    pub fn key_str(&self, id: u32) -> &str {
        // 키는 fold 결과(UTF-8)만 넣는다
        unsafe { std::str::from_utf8_unchecked(self.key(id)) }
    }

    pub fn children(&self, id: u32) -> impl Iterator<Item = u32> + '_ {
        let mut c = self.first_child[id as usize];
        std::iter::from_fn(move || {
            while c != NONE {
                let cur = c;
                c = self.next_sib[cur as usize];
                if self.flags[cur as usize] & F_DELETED == 0 {
                    return Some(cur);
                }
            }
            None
        })
    }

    /// 이름(키 비교: 대소문자·NFC/NFD 무시)으로 자식 찾기
    pub fn find_child(&self, parent: u32, name: &str) -> Option<u32> {
        let k = fold(name);
        self.children(parent).find(|&c| self.key(c) == k.as_bytes())
    }

    /// 항목의 실제 경로
    pub fn full_path(&self, id: u32) -> PathBuf {
        let mut chain = Vec::new();
        let mut cur = id;
        while cur != NONE {
            chain.push(cur);
            cur = self.parent[cur as usize];
        }
        let mut p = PathBuf::new();
        for (i, c) in chain.iter().rev().enumerate() {
            if i == 0 {
                p = PathBuf::from(self.name(*c));
            } else {
                p.push(self.name(*c));
            }
        }
        p
    }

    /// 경로 → 항목 번호(루트 아래에 있을 때). 가장 긴 루트를 고른다.
    pub fn lookup(&self, path: &Path) -> Option<u32> {
        let mut best: Option<(&RootInfo, usize)> = None;
        for r in &self.roots {
            if let Ok(rest) = path.strip_prefix(&r.path) {
                let depth = r.path.components().count();
                if best.map(|(_, d)| depth > d).unwrap_or(true) {
                    best = Some((r, depth));
                    let _ = rest;
                }
            }
        }
        let (root, _) = best?;
        let rest = path.strip_prefix(&root.path).ok()?;
        let mut cur = root.id;
        for c in rest.components() {
            if let Component::Normal(n) = c {
                cur = self.find_child(cur, &n.to_string_lossy())?;
            }
        }
        Some(cur)
    }

    /// 항목과 그 아래를 모두 지운다(표시만). 지운 수
    pub fn remove_subtree(&mut self, id: u32) -> usize {
        if !self.is_live(id) || self.flags[id as usize] & F_ROOT != 0 {
            return 0;
        }
        // 부모의 자식 목록에서 떼기
        let p = self.parent[id as usize];
        if p != NONE {
            let mut prev = NONE;
            let mut c = self.first_child[p as usize];
            while c != NONE && c != id {
                prev = c;
                c = self.next_sib[c as usize];
            }
            if c == id {
                let next = self.next_sib[id as usize];
                if prev == NONE {
                    self.first_child[p as usize] = next;
                } else {
                    self.next_sib[prev as usize] = next;
                }
            }
        }
        let mut n = 0;
        let mut stack = vec![id];
        while let Some(x) = stack.pop() {
            if self.flags[x as usize] & F_DELETED != 0 {
                continue;
            }
            self.flags[x as usize] |= F_DELETED;
            n += 1;
            let mut c = self.first_child[x as usize];
            while c != NONE {
                stack.push(c);
                c = self.next_sib[c as usize];
            }
        }
        self.deleted += n;
        self.version += 1;
        n
    }

    /// 루트 아래 내용 모두 지우기(루트 자체는 남김)
    pub fn clear_root(&mut self, root: u32) {
        let kids: Vec<u32> = self.children(root).collect();
        for k in kids {
            self.remove_subtree(k);
        }
    }

    /// 자식 하나 갱신·추가. (번호, 새로 만들었는지)
    /// 종류(파일↔폴더)가 바뀌었으면 옛 항목을 지우고 새로 만든다.
    pub fn upsert_child(&mut self, parent: u32, name: &str, is_dir: bool, size: u64, mtime: i64) -> (u32, bool) {
        if let Some(id) = self.find_child(parent, name) {
            if self.is_dir(id) == is_dir {
                let i = id as usize;
                if self.name(id) != name {
                    // 대소문자만 바뀐 이름 바꾸기: 새 이름을 창고 끝에 붙인다
                    self.name_off[i] = self.names.len() as u32;
                    self.names.extend_from_slice(name.as_bytes());
                    self.name_len[i] = name.len() as u16;
                }
                self.size[i] = if is_dir { 0 } else { size };
                self.mtime[i] = mtime;
                self.version += 1;
                return (id, false);
            }
            self.remove_subtree(id);
        }
        (self.push_child(parent, name, is_dir, if is_dir { 0 } else { size }, mtime), true)
    }

    /// 폴더 경로 키 캐시(내용이 바뀌었으면 다시 만듦)
    pub fn dir_paths(&self) -> Arc<DirPaths> {
        let mut g = self.dir_cache.lock().unwrap();
        if let Some(c) = g.as_ref() {
            if c.version == self.version {
                return c.clone();
            }
        }
        let n = self.flags.len();
        let mut d = DirPaths { version: self.version, slot: vec![NONE; n], off: Vec::new(), len: Vec::new(), arena: Vec::new() };
        for id in 0..n {
            let f = self.flags[id];
            if f & F_DIR == 0 || f & F_DELETED != 0 {
                continue;
            }
            let off = d.arena.len();
            if f & F_ROOT != 0 {
                d.arena.extend_from_slice(self.key(id as u32));
            } else {
                let p = self.parent[id];
                let ps = d.slot[p as usize];
                if ps == NONE {
                    continue;
                }
                let (po, pl) = (d.off[ps as usize] as usize, d.len[ps as usize] as usize);
                d.arena.extend_from_within(po..po + pl);
                d.arena.push(b'\\');
                d.arena.extend_from_slice(self.key(id as u32));
            }
            d.slot[id] = d.off.len() as u32;
            d.off.push(off as u32);
            d.len.push((d.arena.len() - off) as u32);
        }
        let arc = Arc::new(d);
        *g = Some(arc.clone());
        arc
    }

    /// 이름 순위 만들기(큰 결과를 이름순으로 빨리 정렬하려고). 배경 스레드에서 부른다
    pub fn build_name_rank(&self) {
        let n = self.flags.len();
        let mut ids: Vec<u32> = (0..n as u32).collect();
        ids.sort_unstable_by(|a, b| self.key(*a).cmp(self.key(*b)).then_with(|| self.name(*a).cmp(self.name(*b))));
        let mut rank = vec![0u32; n];
        for (i, id) in ids.into_iter().enumerate() {
            rank[id as usize] = i as u32;
        }
        *self.name_rank.lock().unwrap() = Some(Arc::new(rank));
    }

    pub fn name_rank(&self) -> Option<Arc<Vec<u32>>> {
        self.name_rank.lock().unwrap().clone()
    }

    /// 메모리 사용량(바이트, 대략: 배열 용량 합)
    pub fn heap_bytes(&self) -> usize {
        self.names.capacity()
            + self.keys.capacity()
            + self.name_off.capacity() * 4
            + self.name_len.capacity() * 2
            + self.key_off.capacity() * 4
            + self.key_len.capacity() * 2
            + self.parent.capacity() * 4
            + self.first_child.capacity() * 4
            + self.next_sib.capacity() * 4
            + self.size.capacity() * 8
            + self.mtime.capacity() * 8
            + self.flags.capacity()
            + self.frn.capacity() * 8
            + self.name_rank.lock().unwrap().as_ref().map(|r| r.capacity() * 4).unwrap_or(0)
    }

    /// 지운 항목을 빼고 번호를 다시 매긴 새 색인. `skip_root`가 있으면 그 루트(와 아래)도 뺀다.
    pub fn compacted(&self, skip_root: Option<u32>) -> Index {
        let n = self.flags.len();
        let mut out = Index { generation: self.generation + 1, ..Default::default() };
        out.names.reserve(self.names.len());
        out.keys.reserve(self.keys.len());
        let mut map = vec![NONE; n];
        for id in 0..n {
            let f = self.flags[id];
            if f & F_DELETED != 0 || Some(id as u32) == skip_root {
                continue;
            }
            let p = self.parent[id];
            let np = if p == NONE { NONE } else { map[p as usize] };
            if p != NONE && np == NONE {
                continue; // 뺀 루트 아래
            }
            let key = self.key_str(id as u32).to_string();
            let nid = out.push_raw_frn(np, self.name(id as u32), f, self.size[id], self.mtime[id], Some(&key), self.frn[id]);
            map[id] = nid;
        }
        for r in &self.roots {
            if Some(r.id) == skip_root {
                continue;
            }
            let mut r2 = r.clone();
            r2.id = map[r.id as usize];
            out.roots.push(r2);
        }
        out
    }

    /// 다른 색인(루트 하나 이상)을 뒤에 이어 붙인다. 같은 경로의 루트가 이미 있으면 먼저 빼야 한다.
    pub fn append(&mut self, other: &Index) {
        let mut map = vec![NONE; other.flags.len()];
        for id in 0..other.flags.len() {
            let f = other.flags[id];
            if f & F_DELETED != 0 {
                continue;
            }
            let p = other.parent[id];
            let np = if p == NONE { NONE } else { map[p as usize] };
            if p != NONE && np == NONE {
                continue;
            }
            let key = other.key_str(id as u32).to_string();
            map[id] = self.push_raw_frn(np, other.name(id as u32), f, other.size[id], other.mtime[id], Some(&key), other.frn[id]);
        }
        for r in &other.roots {
            let mut r2 = r.clone();
            r2.id = map[r.id as usize];
            self.roots.push(r2);
        }
    }

    /// 루트 하나를 새로 스캔한 결과(`fresh`, 그 루트만 들어 있음)로 바꾼 새 색인
    pub fn with_root_replaced(&self, fresh: &Index) -> Index {
        let mut out = match fresh.roots.first().and_then(|r| self.root_by_path(&r.path)) {
            Some(old) => self.compacted(Some(old.id)),
            None => self.compacted(None),
        };
        out.append(fresh);
        out
    }

    // ---------- 저장·읽기 ----------

    /// 파일에 저장: 임시 파일에 쓰고 바꿔치기(중간에 끊겨도 옛 파일이 남음)
    pub fn save(&self, path: &Path) -> Result<u64> {
        let compact;
        let ix = if self.deleted > 0 {
            compact = self.compacted(None);
            &compact
        } else {
            self
        };
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(io_err(d))?;
        }
        let tmp = path.with_extension("bin.tmp");
        {
            let f = std::fs::File::create(&tmp).map_err(io_err(&tmp))?;
            let mut w = HashWriter { inner: BufWriter::with_capacity(1 << 20, f), hash: FNV_OFFSET };
            ix.write_to(&mut w).map_err(io_err(&tmp))?;
            let h = w.hash;
            w.inner.write_all(&h.to_le_bytes()).map_err(io_err(&tmp))?;
            let f = w.inner.into_inner().map_err(|e| Error::Invalid(e.to_string()))?;
            f.sync_all().map_err(io_err(&tmp))?;
        }
        std::fs::rename(&tmp, path).map_err(io_err(path))?;
        if path.file_name().map(|n| n == INDEX_FILE).unwrap_or(false) {
            if let Some(d) = path.parent() {
                let _ = std::fs::remove_file(d.join(INDEX_FILE_V1));
            }
        }
        Ok(std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
    }

    fn write_to(&self, w: &mut impl Write) -> std::io::Result<()> {
        w.write_all(MAGIC)?;
        w.write_all(&FORMAT_VERSION.to_le_bytes())?;
        w.write_all(&now_unix().to_le_bytes())?;
        w.write_all(&(self.roots.len() as u32).to_le_bytes())?;
        for r in &self.roots {
            let p = r.path.to_string_lossy();
            w.write_all(&(p.len() as u32).to_le_bytes())?;
            w.write_all(p.as_bytes())?;
            w.write_all(&r.id.to_le_bytes())?;
            w.write_all(&r.last_scan.to_le_bytes())?;
            let s = &r.stats;
            for v in [s.dirs, s.files, s.denied, s.reparse, s.excluded, s.millis] {
                w.write_all(&v.to_le_bytes())?;
            }
            w.write_all(&[r.source as u8, r.usn.is_some() as u8])?;
            let c = r.usn.unwrap_or_default();
            w.write_all(&c.volume_serial.to_le_bytes())?;
            w.write_all(&c.journal_id.to_le_bytes())?;
            w.write_all(&c.next_usn.to_le_bytes())?;
        }
        let n = self.flags.len();
        w.write_all(&(n as u32).to_le_bytes())?;
        let mut buf = Vec::with_capacity(n * 8);
        for v in &self.parent {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        w.write_all(&buf)?;
        w.write_all(&self.flags)?;
        buf.clear();
        for v in &self.size {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        w.write_all(&buf)?;
        buf.clear();
        for v in &self.mtime {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        w.write_all(&buf)?;
        buf.clear();
        for v in &self.name_len {
            buf.extend_from_slice(&v.to_le_bytes());
        }
        w.write_all(&buf)?;
        // 이름: 번호 순서대로(창고에 남은 옛 이름은 빼고)
        for id in 0..n {
            let o = self.name_off[id] as usize;
            w.write_all(&self.names[o..o + self.name_len[id] as usize])?;
        }
        let has = self.has_frn();
        w.write_all(&[has as u8])?;
        if has {
            buf.clear();
            for v in &self.frn {
                buf.extend_from_slice(&v.to_le_bytes());
            }
            w.write_all(&buf)?;
        }
        Ok(())
    }

    /// 색인 폴더에서 읽기: `index-v2.bin`, 없으면 2a의 `index-v1.bin`
    pub fn load_dir_file(path: &Path) -> Result<Index> {
        if !path.exists() {
            if let Some(d) = path.parent() {
                let v1 = d.join(INDEX_FILE_V1);
                if v1.exists() {
                    return Index::load(&v1);
                }
            }
        }
        Index::load(path)
    }

    /// 저장한 파일 읽기(버전 1·2). 형식·버전·검사합이 다르면 오류(→ 새로 스캔)
    pub fn load(path: &Path) -> Result<Index> {
        let mut bytes = Vec::new();
        std::fs::File::open(path).and_then(|mut f| f.read_to_end(&mut bytes)).map_err(io_err(path))?;
        let bad = |m: &str| Error::Invalid(format!("검색 색인 파일을 읽을 수 없습니다({m}): {}", path.display()));
        if bytes.len() < 8 + 4 + 8 + 4 + 4 + 8 || &bytes[..8] != MAGIC {
            return Err(bad("형식"));
        }
        let (body, tail) = bytes.split_at(bytes.len() - 8);
        let mut h = FNV_OFFSET;
        fnv(&mut h, body);
        if h.to_le_bytes() != tail {
            return Err(bad("검사합"));
        }
        let mut r = Reader { b: body, i: 8 };
        let ver = r.u32().ok_or_else(|| bad("버전"))?;
        if ver != 1 && ver != FORMAT_VERSION {
            return Err(bad("버전"));
        }
        let _created = r.i64();
        let nr = r.u32().ok_or_else(|| bad("루트"))? as usize;
        let mut roots = Vec::with_capacity(nr);
        for _ in 0..nr {
            let l = r.u32().ok_or_else(|| bad("루트"))? as usize;
            let p = String::from_utf8_lossy(r.take(l).ok_or_else(|| bad("루트"))?).into_owned();
            let id = r.u32().ok_or_else(|| bad("루트"))?;
            let last_scan = r.i64().ok_or_else(|| bad("루트"))?;
            let mut v = [0u64; 6];
            for x in v.iter_mut() {
                *x = r.u64().ok_or_else(|| bad("루트"))?;
            }
            let stats = RootStats { dirs: v[0], files: v[1], denied: v[2], reparse: v[3], excluded: v[4], millis: v[5] };
            let (mut source, mut usn) = (SourceKind::Walk, None);
            if ver >= 2 {
                let sb = r.take(2).ok_or_else(|| bad("루트"))?;
                source = if sb[0] == 1 { SourceKind::Mft } else { SourceKind::Walk };
                let c = UsnCursor {
                    volume_serial: r.u64().ok_or_else(|| bad("루트"))?,
                    journal_id: r.u64().ok_or_else(|| bad("루트"))?,
                    next_usn: r.i64().ok_or_else(|| bad("루트"))?,
                };
                if sb[1] == 1 {
                    usn = Some(c);
                }
            }
            roots.push(RootInfo { path: PathBuf::from(p), id, last_scan, stats, source, usn });
        }
        let n = r.u32().ok_or_else(|| bad("항목 수"))? as usize;
        let parent: Vec<u32> = r.take(n * 4).ok_or_else(|| bad("부모"))?.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        let flags = r.take(n).ok_or_else(|| bad("종류"))?.to_vec();
        let size: Vec<u64> = r.take(n * 8).ok_or_else(|| bad("크기"))?.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect();
        let mtime: Vec<i64> = r.take(n * 8).ok_or_else(|| bad("시각"))?.chunks_exact(8).map(|c| i64::from_le_bytes(c.try_into().unwrap())).collect();
        let name_len: Vec<u16> = r.take(n * 2).ok_or_else(|| bad("이름"))?.chunks_exact(2).map(|c| u16::from_le_bytes(c.try_into().unwrap())).collect();
        let total: usize = name_len.iter().map(|&l| l as usize).sum();
        let names = r.take(total).ok_or_else(|| bad("이름"))?.to_vec();
        if std::str::from_utf8(&names).is_err() {
            return Err(bad("이름"));
        }
        let frn: Vec<u64> = if ver >= 2 && r.take(1).ok_or_else(|| bad("FRN"))?[0] == 1 {
            r.take(n * 8).ok_or_else(|| bad("FRN"))?.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect()
        } else {
            vec![0; n]
        };
        let mut ix = Index { names, roots, ..Default::default() };
        ix.name_off.reserve(n);
        ix.keys.reserve(total);
        let mut off = 0u32;
        let mut key = String::new();
        for (id, &l) in name_len.iter().enumerate() {
            ix.name_off.push(off);
            let name = std::str::from_utf8(&ix.names[off as usize..off as usize + l as usize]).map_err(|_| bad("이름"))?;
            off += l as u32;
            let ko = ix.keys.len();
            if flags[id] & F_ROOT != 0 {
                ix.keys.extend_from_slice(root_key(Path::new(name)).as_bytes());
            } else {
                key.clear();
                fold_into(name, &mut key);
                ix.keys.extend_from_slice(key.as_bytes());
            }
            ix.key_off.push(ko as u32);
            ix.key_len.push((ix.keys.len() - ko) as u16);
            let p = parent[id];
            if p != NONE && p as usize >= id {
                return Err(bad("부모 순서"));
            }
        }
        ix.name_len = name_len;
        ix.first_child = vec![NONE; n];
        ix.next_sib = vec![NONE; n];
        for id in 0..n {
            let p = parent[id];
            if p != NONE {
                ix.next_sib[id] = ix.first_child[p as usize];
                ix.first_child[p as usize] = id as u32;
            }
        }
        ix.parent = parent;
        ix.flags = flags;
        ix.size = size;
        ix.mtime = mtime;
        ix.frn = frn;
        ix.deleted = ix.flags.iter().filter(|f| **f & F_DELETED != 0).count();
        ix.version = 1;
        Ok(ix)
    }
}

const FNV_OFFSET: u64 = 0xcbf29ce484222325;

fn fnv(h: &mut u64, data: &[u8]) {
    for b in data {
        *h ^= *b as u64;
        *h = h.wrapping_mul(0x100000001b3);
    }
}

struct HashWriter<W: Write> {
    inner: W,
    hash: u64,
}

impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        fnv(&mut self.hash, &buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.i..self.i.checked_add(n)?)?;
        self.i += n;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
    }
    fn i64(&mut self) -> Option<i64> {
        self.take(8).map(|s| i64::from_le_bytes(s.try_into().unwrap()))
    }
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
