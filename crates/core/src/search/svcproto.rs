//! 2c: 빠른 검색 서비스(`filetool-svc`)와 앱·CLI 사이의 이름 있는 파이프 프로토콜 — 플랫폼 무관 부분.
//!
//! - 틀: `u32`(리틀 엔디언) 길이 + JSON 한 덩어리. 요청은 [`MAX_REQUEST_BYTES`]를 넘으면 읽지 않고 끊는다.
//! - 봉투: `{"v": 1, "op": "...", ...}`. 버전이 다르면 `version` 오류. 첫 요청은 반드시 `hello`.
//! - 할 수 있는 것: `hello`, `status`, `query`(같은 질의 언어, 정렬, 첫 줄들), `rows`(마지막 결과의 다음 줄들).
//!   쓰기·지우기·실행은 없다. 색인이 바뀌었는지는 `status`의 `version`/`generation`으로 알아낸다(폴링).
//! - 가시성([`Visibility`]): 서비스는 볼륨의 모든 이름을 보므로, 관리자가 아닌 호출자에게는
//!   **다른 사용자의 프로필 폴더**(`C:\Users\<다른 사람>`, 프로필 목록의 다른 경로) 아래를 돌려주지 않는다.
//!   관리자(권한 상승된 토큰)는 모두 본다. 2a 기본 제외 목록은 서비스가 색인할 때 이미 적용한다.
//!
//! Windows 쪽(파이프 만들기·ACL·호출자 확인·연결)은 [`super::svcpipe`].

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::index::{root_key, Index, RootStats, F_DELETED, F_ROOT, NONE};
use super::query::{self, Query, QueryCtx, Row, SortKey};
use super::scan::{default_excludes, Excludes};
use super::service::SearchStatus;
use super::text::fold;

/// 프로토콜 버전(봉투의 `v`)
pub const PROTOCOL_VERSION: u32 = 1;
/// 파이프 이름
pub const PIPE_NAME: &str = r"\\.\pipe\kr.salarycrew.winfiletool.search";
/// 서비스 이름(SCM)
pub const SERVICE_NAME: &str = "FileToolSvc";
/// 서비스가 받는 요청 한 덩어리의 최대 크기
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// 클라이언트가 받는 응답 한 덩어리의 최대 크기
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
/// 질의 문자열 최대 길이(바이트)
pub const MAX_QUERY_BYTES: usize = 4096;
/// 한 번에 돌려주는 줄 수 최대(넘으면 줄임)
pub const MAX_ROWS: usize = 1000;
/// 사용자 제외 패턴 수·길이 최대
pub const MAX_EXCLUDES: usize = 64;
pub const MAX_EXCLUDE_BYTES: usize = 520;
/// 동시에 붙을 수 있는 클라이언트 수(파이프 인스턴스 수)
pub const MAX_CLIENTS: usize = 16;
/// 0.3.3(보안 강화): 같은 사용자(클라이언트 프로세스의 SID)가 동시에 붙잡을 수 있는 연결 수 — 한 사람이 16개를 다 붙잡아
/// 다른 사용자의 빠른 검색을 막지 못하게
pub const MAX_CLIENTS_PER_USER: usize = 4;
/// 0.3.3: 연결한 뒤 첫 요청이 다 올 때까지(이 안에 안 오면 끊음)
pub const FIRST_FRAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// 0.3.3: 요청 사이에 아무것도 오지 않으면 끊는 시간(읽기·쓰기 하나마다)
pub const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// 0.3.3: 사용자별 연결 수 세기(서버가 연결을 받자마자 확인, 끝나면 돌려줌)
#[derive(Debug, Default)]
pub struct ConnLimiter {
    per: std::sync::Mutex<std::collections::HashMap<String, usize>>,
}

impl ConnLimiter {
    /// `key`(사용자 SID, 모르면 "?")의 연결을 하나 더 받을 수 있으면 세고 true
    pub fn try_acquire(&self, key: &str, max: usize) -> bool {
        let mut m = self.per.lock().unwrap_or_else(|e| e.into_inner());
        let n = m.entry(key.to_string()).or_insert(0);
        if *n >= max {
            return false;
        }
        *n += 1;
        true
    }
    pub fn release(&self, key: &str) {
        let mut m = self.per.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = m.get_mut(key) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                m.remove(key);
            }
        }
    }
    pub fn count(&self, key: &str) -> usize {
        self.per.lock().unwrap_or_else(|e| e.into_inner()).get(key).copied().unwrap_or(0)
    }
}

// ---------- 메시지 ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Hello {
        client: String,
    },
    Status,
    Query {
        q: String,
        #[serde(default)]
        sort: SortKey,
        #[serde(default)]
        desc: bool,
        /// 함께 돌려받을 첫 줄 수(0이면 수만)
        #[serde(default)]
        limit: usize,
        /// 사용자 설정의 제외 패턴(서비스 기본 목록에 없는 것만 결과에서 거른다)
        #[serde(default)]
        excludes: Vec<String>,
    },
    Rows {
        offset: usize,
        limit: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Response {
    Hello { server: String, protocol: u32, client: ClientView },
    Status(SvcStatus),
    Query(QueryReply),
    Rows { rows: Vec<Row>, stale: bool },
    Error { code: String, message: String },
}

impl Response {
    pub fn error(code: &str, message: impl Into<String>) -> Response {
        Response::Error { code: code.into(), message: message.into() }
    }
}

/// 서비스가 본 호출자(hello 응답)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ClientView {
    pub user: String,
    pub sid: String,
    pub admin: bool,
    /// 사람이 읽는 가시성 설명
    pub visibility: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SvcRoot {
    pub path: String,
    pub source: String,
    pub scanning: bool,
    pub scanned_so_far: u64,
    pub last_scan: i64,
    pub stats: RootStats,
    pub note: Option<String>,
    pub catchup: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SvcStatus {
    pub server: String,
    pub protocol: u32,
    pub pid: u32,
    /// 서비스가 시작한 시각(유닉스 초)
    pub started: i64,
    /// 시작부터 처음 색인(불러오기 + 따라잡기 또는 전체 읽기)이 끝날 때까지(ms)
    pub ready_ms: Option<u64>,
    pub ready: bool,
    pub entries: usize,
    pub version: u64,
    pub generation: u64,
    pub live_updates: u64,
    pub loaded_ms: Option<u64>,
    pub last_save: i64,
    pub index_bytes: u64,
    pub roots: Vec<SvcRoot>,
    /// 지금 붙어 있는 클라이언트 수
    pub clients: usize,
}

impl SvcStatus {
    /// 검색 서비스 상태 → 파이프 상태
    pub fn from_search(st: &SearchStatus) -> SvcStatus {
        SvcStatus {
            server: server_name(),
            protocol: PROTOCOL_VERSION,
            pid: std::process::id(),
            ready: st.ready,
            entries: st.entries,
            version: st.version,
            generation: st.generation,
            live_updates: st.live_updates,
            loaded_ms: st.loaded_ms,
            last_save: st.last_save,
            index_bytes: st.index_bytes,
            roots: st
                .roots
                .iter()
                .map(|r| SvcRoot {
                    path: r.path.clone(),
                    source: r.source.clone(),
                    scanning: r.scanning,
                    scanned_so_far: r.scanned_so_far,
                    last_scan: r.last_scan,
                    stats: r.stats.clone(),
                    note: r.source_note.clone(),
                    catchup: r.catchup.clone(),
                })
                .collect(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct QueryReply {
    pub total: usize,
    pub millis: f64,
    pub generation: u64,
    pub version: u64,
    pub rows: Vec<Row>,
}

pub fn server_name() -> String {
    format!("filetool-svc {}", env!("CARGO_PKG_VERSION"))
}

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    v: u32,
    #[serde(flatten)]
    body: T,
}

pub fn encode_request(r: &Request) -> Vec<u8> {
    serde_json::to_vec(&Envelope { v: PROTOCOL_VERSION, body: r }).unwrap_or_default()
}

pub fn encode_response(r: &Response) -> Vec<u8> {
    serde_json::to_vec(&Envelope { v: PROTOCOL_VERSION, body: r }).unwrap_or_default()
}

#[derive(Deserialize)]
struct VersionOnly {
    v: Option<u32>,
}

/// 요청 풀기. 실패하면 돌려보낼 오류 응답
pub fn decode_request(b: &[u8]) -> Result<Request, Response> {
    let v: VersionOnly = serde_json::from_slice(b).map_err(|e| Response::error("bad_request", format!("요청을 읽을 수 없습니다: {e}")))?;
    match v.v {
        Some(PROTOCOL_VERSION) => {}
        Some(other) => {
            return Err(Response::error("version", format!("프로토콜 버전이 다릅니다(서비스 {PROTOCOL_VERSION}, 요청 {other})")));
        }
        None => return Err(Response::error("bad_request", "프로토콜 버전(v)이 없습니다")),
    }
    let e: Envelope<Request> = serde_json::from_slice(b).map_err(|e| Response::error("bad_request", format!("알 수 없는 요청입니다: {e}")))?;
    Ok(e.body)
}

pub fn decode_response(b: &[u8]) -> Result<Response, String> {
    let v: VersionOnly = serde_json::from_slice(b).map_err(|e| format!("서비스 응답을 읽을 수 없습니다: {e}"))?;
    if v.v != Some(PROTOCOL_VERSION) {
        return Err(format!("서비스 프로토콜 버전이 다릅니다(앱 {PROTOCOL_VERSION}, 서비스 {:?})", v.v));
    }
    let e: Envelope<Response> = serde_json::from_slice(b).map_err(|e| format!("서비스 응답을 읽을 수 없습니다: {e}"))?;
    Ok(e.body)
}

// ---------- 틀(길이 + 본문) ----------

#[derive(Debug)]
pub enum FrameErr {
    /// 길이가 한도를 넘음(본문은 읽지 않음 → 연결을 끊어야 함)
    TooLarge(usize),
    Io(std::io::Error),
}

impl std::fmt::Display for FrameErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameErr::TooLarge(n) => write!(f, "메시지가 너무 큽니다({n} 바이트)"),
            FrameErr::Io(e) => write!(f, "{e}"),
        }
    }
}

pub fn write_frame<W: Write>(w: &mut W, body: &[u8]) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    buf.extend_from_slice(body);
    w.write_all(&buf)?;
    w.flush()
}

/// 틀 하나 읽기. 상대가 깨끗이 끊었으면 `Ok(None)`
pub fn read_frame<R: Read>(r: &mut R, max: usize) -> Result<Option<Vec<u8>>, FrameErr> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(FrameErr::Io(std::io::ErrorKind::UnexpectedEof.into())),
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe && got == 0 => return Ok(None),
            Err(e) => return Err(FrameErr::Io(e)),
        }
    }
    let n = u32::from_le_bytes(len) as usize;
    if n > max {
        return Err(FrameErr::TooLarge(n));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body).map_err(FrameErr::Io)?;
    Ok(Some(body))
}

// ---------- 호출자·가시성 ----------

/// 파이프 호출자(서비스가 토큰에서 읽음)
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Identity {
    /// `DOMAIN\user`
    pub user: String,
    /// `S-1-5-21-…`
    pub sid: String,
    /// 관리자 그룹이 켜진(권한 상승된) 토큰
    pub admin: bool,
    /// 이 사용자의 프로필 폴더(`C:\Users\user`), 프로필 목록에서 찾음
    pub profile: Option<String>,
}

/// 결과에서 숨길 범위
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Visibility {
    /// 모두 보임(관리자)
    pub all: bool,
    /// 프로필 폴더들의 부모(`C:\Users`). 그 바로 아래 폴더는 `keep`에 없으면 숨김
    pub profiles_dirs: Vec<String>,
    /// 숨기지 않을 프로필 아래 폴더(자기 프로필, `C:\Users\Public`)
    pub keep: Vec<String>,
    /// 그 밖에 숨길 경로(프로필 목록의 다른 사용자·서비스 계정 프로필, 예: `C:\Windows\ServiceProfiles\LocalService`)
    pub hidden: Vec<String>,
}

/// 모든 사용자가 같이 쓰는 프로필 폴더(숨기지 않음)
pub const SHARED_PROFILE_NAMES: &[&str] = &["Public"];

impl Visibility {
    /// 기본 규칙: 관리자 = 모두, 아니면 다른 사용자의 프로필 폴더를 숨김.
    /// `profiles_dir` = 프로필 목록의 `ProfilesDirectory`(보통 `C:\Users`), `profile_list` = 등록된 모든 프로필 경로
    pub fn for_identity(id: &Identity, profiles_dir: &str, profile_list: &[String]) -> Visibility {
        if id.admin {
            return Visibility { all: true, ..Default::default() };
        }
        let own = id.profile.as_deref().map(fold_win);
        let mut keep: Vec<String> = SHARED_PROFILE_NAMES.iter().map(|n| join_win(profiles_dir, n)).collect();
        if let Some(p) = &id.profile {
            keep.push(p.clone());
        }
        let hidden = profile_list.iter().filter(|p| Some(fold_win(p)) != own).cloned().collect();
        Visibility { all: false, profiles_dirs: vec![profiles_dir.to_string()], keep, hidden }
    }

    pub fn describe(&self) -> String {
        if self.all {
            "관리자: 모든 이름".into()
        } else {
            format!("다른 사용자의 프로필 폴더 제외({})", self.profiles_dirs.join(", "))
        }
    }

    /// 색인에서 숨길 폴더 번호(질의할 때마다 다시 구함 — 이름이 바뀌어도 맞음)
    pub fn hidden_ids(&self, ix: &Index) -> HashSet<u32> {
        let mut out = HashSet::new();
        if self.all {
            return out;
        }
        let keep: HashSet<String> = self.keep.iter().map(|k| fold_win(k)).collect();
        for pd in &self.profiles_dirs {
            if let Some(id) = lookup_win(ix, pd) {
                let base = fold_win(pd);
                for c in ix.children(id) {
                    let p = format!("{base}\\{}", ix.key_str(c));
                    if !keep.contains(&p) {
                        out.insert(c);
                    }
                }
            }
        }
        for h in &self.hidden {
            if keep.contains(&fold_win(h)) {
                continue;
            }
            if let Some(id) = lookup_win(ix, h) {
                out.insert(id);
            }
        }
        out
    }
}

/// Windows 경로 비교 키(소문자 NFC, 구분자 `\`, 끝 구분자 없음)
pub fn fold_win(p: &str) -> String {
    fold(p).replace('/', "\\").trim_end_matches('\\').to_string()
}

fn join_win(a: &str, b: &str) -> String {
    format!("{}\\{b}", a.trim_end_matches(['\\', '/']))
}

/// Windows 경로(`C:\Users\user`) → 색인 번호. 어느 플랫폼에서든 같게 동작(시험용)
pub fn lookup_win(ix: &Index, p: &str) -> Option<u32> {
    let k = fold_win(p);
    let mut parts = k.split('\\').filter(|s| !s.is_empty());
    let drive = parts.next()?;
    let root = ix.roots().iter().find(|r| root_key(&r.path) == drive)?;
    let mut cur = root.id;
    for part in parts {
        cur = ix.children(cur).find(|&c| ix.key_str(c) == part)?;
    }
    Some(cur)
}

/// 결과 거르기: 가시성 + 사용자 제외 패턴(폴더와 그 아래)
pub struct ResultFilter<'a> {
    ix: &'a Index,
    hidden: HashSet<u32>,
    extra: Option<(Excludes, Arc<super::index::DirPaths>)>,
    memo: Vec<u8>,
}

impl<'a> ResultFilter<'a> {
    /// 거를 것이 없으면 None
    pub fn new(ix: &'a Index, vis: &Visibility, client_excludes: &[String]) -> Option<ResultFilter<'a>> {
        let hidden = vis.hidden_ids(ix);
        let defaults: HashSet<String> = default_excludes().iter().map(|d| fold(d.trim())).collect();
        let extra: Vec<String> = client_excludes.iter().filter(|e| !e.trim().is_empty() && !defaults.contains(&fold(e.trim()))).cloned().collect();
        let extra = (!extra.is_empty()).then(|| (Excludes::new(&extra), ix.dir_paths()));
        if hidden.is_empty() && extra.is_none() {
            return None;
        }
        Some(ResultFilter { ix, hidden, extra, memo: vec![0; ix.len()] })
    }

    /// 보여 줄 항목인지(자신이나 위 폴더가 숨김/제외면 false)
    pub fn keep(&mut self, id: u32) -> bool {
        const VISIBLE: u8 = 1;
        const HIDDEN: u8 = 2;
        let mut chain: Vec<u32> = Vec::new();
        let mut cur = id;
        let mut verdict = VISIBLE;
        while cur != NONE {
            let m = self.memo.get(cur as usize).copied().unwrap_or(0);
            if m != 0 {
                verdict = m;
                break;
            }
            chain.push(cur);
            if self.hidden.contains(&cur) || self.excluded(cur) {
                verdict = HIDDEN;
                break;
            }
            cur = self.ix.parent(cur);
        }
        for c in chain {
            if let Some(m) = self.memo.get_mut(c as usize) {
                *m = verdict;
            }
        }
        verdict == VISIBLE
    }

    fn excluded(&self, id: u32) -> bool {
        let Some((ex, dirs)) = &self.extra else { return false };
        if !self.ix.is_dir(id) || self.ix.flags(id) & F_ROOT != 0 {
            return false;
        }
        ex.matches_key(self.ix.key_str(id), dirs.get(id))
    }
}

// ---------- 서비스 쪽 처리 ----------

/// 연결 하나의 상태
pub struct Session {
    pub identity: Identity,
    pub vis: Visibility,
    hello: bool,
    ids: Vec<u32>,
    generation: u64,
    pub requests: u64,
}

impl Session {
    pub fn new(identity: Identity, vis: Visibility) -> Session {
        Session { identity, vis, hello: false, ids: Vec::new(), generation: u64::MAX, requests: 0 }
    }
}

/// 요청 처리기(서비스의 색인 + 상태)
pub struct Handler {
    pub index: Arc<RwLock<Index>>,
    pub status: Box<dyn Fn() -> SvcStatus + Send + Sync>,
}

impl Handler {
    pub fn handle(&self, s: &mut Session, req: Request) -> Response {
        s.requests += 1;
        if !s.hello && !matches!(req, Request::Hello { .. }) {
            return Response::error("hello_first", "먼저 hello를 보내야 합니다");
        }
        match req {
            Request::Hello { .. } => {
                s.hello = true;
                Response::Hello {
                    server: server_name(),
                    protocol: PROTOCOL_VERSION,
                    client: ClientView { user: s.identity.user.clone(), sid: s.identity.sid.clone(), admin: s.identity.admin, visibility: s.vis.describe() },
                }
            }
            Request::Status => Response::Status((self.status)()),
            Request::Query { q, sort, desc, limit, excludes } => {
                if q.len() > MAX_QUERY_BYTES {
                    return Response::error("limit", format!("질의가 너무 깁니다({} 바이트, 최대 {MAX_QUERY_BYTES})", q.len()));
                }
                if excludes.len() > MAX_EXCLUDES || excludes.iter().any(|e| e.len() > MAX_EXCLUDE_BYTES) {
                    return Response::error("limit", format!("제외 패턴이 너무 많거나 깁니다(최대 {MAX_EXCLUDES}개, {MAX_EXCLUDE_BYTES} 바이트)"));
                }
                let query = match Query::parse(&q, &QueryCtx::local_now()) {
                    Ok(x) => x,
                    Err(e) => return Response::error("query", e),
                };
                let t0 = Instant::now();
                let ix = self.index.read().unwrap();
                let mut ids = if query.is_empty() { Vec::new() } else { query::run(&ix, &query) };
                if let Some(mut f) = ResultFilter::new(&ix, &s.vis, &excludes) {
                    ids.retain(|&id| f.keep(id));
                }
                query::sort(&ix, &mut ids, sort, desc);
                let rows = ids.iter().take(limit.min(MAX_ROWS)).map(|&id| query::row(&ix, id)).collect();
                let reply = QueryReply { total: ids.len(), millis: t0.elapsed().as_secs_f64() * 1000.0, generation: ix.generation, version: ix.version, rows };
                s.generation = ix.generation;
                s.ids = ids;
                Response::Query(reply)
            }
            Request::Rows { offset, limit } => {
                let ix = self.index.read().unwrap();
                if ix.generation != s.generation {
                    return Response::Rows { rows: Vec::new(), stale: true };
                }
                let end = offset.saturating_add(limit.min(MAX_ROWS)).min(s.ids.len());
                let start = offset.min(end);
                let rows = s.ids[start..end].iter().filter(|&&id| ix.is_live(id) && ix.flags(id) & F_DELETED == 0).map(|&id| query::row(&ix, id)).collect();
                Response::Rows { rows, stale: false }
            }
        }
    }
}

/// 연결 하나를 끝까지 처리(틀 읽기 → 처리 → 응답). 한도를 넘는 요청이 오면 오류를 보내고 끊는다.
pub fn serve<S: Read + Write>(stream: &mut S, h: &Handler, s: &mut Session) -> std::io::Result<()> {
    loop {
        let frame = match read_frame(stream, MAX_REQUEST_BYTES) {
            Ok(Some(f)) => f,
            Ok(None) => return Ok(()),
            Err(FrameErr::TooLarge(n)) => {
                let r = Response::error("limit", format!("요청이 너무 큽니다({n} 바이트, 최대 {MAX_REQUEST_BYTES})"));
                let _ = write_frame(stream, &encode_response(&r));
                return Ok(());
            }
            Err(FrameErr::Io(e)) => return Err(e),
        };
        let resp = match decode_request(&frame) {
            Ok(req) => h.handle(s, req),
            Err(e) => e,
        };
        write_frame(stream, &encode_response(&resp))?;
    }
}

// ---------- 클라이언트 ----------

/// 파이프(또는 시험용 스트림) 위의 클라이언트
pub struct Client<S: Read + Write> {
    s: S,
    pub server: String,
    pub me: ClientView,
}

impl<S: Read + Write> Client<S> {
    pub fn handshake(s: S, client: &str) -> Result<Client<S>, String> {
        let mut c = Client { s, server: String::new(), me: ClientView::default() };
        match c.call(&Request::Hello { client: client.into() })? {
            Response::Hello { server, protocol, client } if protocol == PROTOCOL_VERSION => {
                c.server = server;
                c.me = client;
                Ok(c)
            }
            other => Err(format!("서비스가 예상하지 못한 응답을 보냈습니다: {other:?}")),
        }
    }

    pub fn call(&mut self, req: &Request) -> Result<Response, String> {
        write_frame(&mut self.s, &encode_request(req)).map_err(|e| format!("서비스에 보내지 못했습니다: {e}"))?;
        let b = read_frame(&mut self.s, MAX_RESPONSE_BYTES).map_err(|e| format!("서비스 응답을 받지 못했습니다: {e}"))?.ok_or("서비스가 연결을 끊었습니다")?;
        match decode_response(&b)? {
            Response::Error { code, message } => Err(format!("{message} [{code}]")),
            r => Ok(r),
        }
    }

    pub fn status(&mut self) -> Result<SvcStatus, String> {
        match self.call(&Request::Status)? {
            Response::Status(s) => Ok(s),
            o => Err(format!("예상하지 못한 응답: {o:?}")),
        }
    }

    pub fn query(&mut self, q: &str, sort: SortKey, desc: bool, limit: usize, excludes: &[String]) -> Result<QueryReply, String> {
        match self.call(&Request::Query { q: q.into(), sort, desc, limit, excludes: excludes.to_vec() })? {
            Response::Query(r) => Ok(r),
            o => Err(format!("예상하지 못한 응답: {o:?}")),
        }
    }

    /// 마지막 결과의 [offset, offset+limit). 색인이 통째로 바뀌었으면 `None`(다시 질의)
    pub fn rows(&mut self, offset: usize, limit: usize) -> Result<Option<Vec<Row>>, String> {
        match self.call(&Request::Rows { offset, limit })? {
            Response::Rows { stale: true, .. } => Ok(None),
            Response::Rows { rows, .. } => Ok(Some(rows)),
            o => Err(format!("예상하지 못한 응답: {o:?}")),
        }
    }
}

/// 시험·점검용: 같은 질의를 색인에 직접(파이프 없이) — 가시성 규칙을 적용한 경로 목록
pub fn visible_paths(ix: &Index, q: &str, vis: &Visibility, excludes: &[String]) -> Result<Vec<String>, String> {
    let query = Query::parse(q, &QueryCtx::local_now())?;
    let mut ids = query::run(ix, &query);
    if let Some(mut f) = ResultFilter::new(ix, vis, excludes) {
        ids.retain(|&id| f.keep(id));
    }
    query::sort(ix, &mut ids, SortKey::Path, false);
    Ok(ids.iter().map(|&id| query::row(ix, id).path).collect())
}

/// 서비스 상태를 사람이 읽는 줄들로(CLI `svc-status`, 서비스 `status`)
pub fn describe_status(s: &SvcStatus) -> Vec<String> {
    let mut v = vec![format!(
        "{} (프로토콜 {}, pid {}) · 항목 {}개 · {} · 실시간 반영 {}건 · 클라이언트 {}",
        s.server,
        s.protocol,
        s.pid,
        s.entries,
        if s.ready { "준비됨" } else { "색인 중" },
        s.live_updates,
        s.clients
    )];
    if let Some(ms) = s.ready_ms {
        v.push(format!("시작 → 준비 {:.2} 초", ms as f64 / 1000.0));
    }
    if let Some(ms) = s.loaded_ms {
        v.push(format!("저장한 색인 불러오기 {ms} ms, 색인 파일 {} 바이트", s.index_bytes));
    }
    let mut by: HashMap<&str, usize> = HashMap::new();
    for r in &s.roots {
        *by.entry(r.source.as_str()).or_default() += 1;
        v.push(format!(
            "  {} [{}]{} 폴더 {} / 파일 {} / 스캔 {:.2} 초{}{}",
            r.path,
            r.source,
            if r.scanning { format!(" 스캔 중({})", r.scanned_so_far) } else { String::new() },
            r.stats.dirs,
            r.stats.files,
            r.stats.millis as f64 / 1000.0,
            r.catchup.as_ref().map(|c| format!(" · {c}")).unwrap_or_default(),
            r.note.as_ref().map(|c| format!(" · {c}")).unwrap_or_default(),
        ));
    }
    v
}

/// 서비스 자료 폴더: `%ProgramData%\kr.salarycrew.winfiletool` (SYSTEM·관리자만 쓰고 읽음 — 색인에는 모든 이름이 있음)
pub fn service_data_dir() -> std::path::PathBuf {
    let base = std::env::var_os("ProgramData").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(r"C:\ProgramData"));
    base.join("kr.salarycrew.winfiletool")
}

/// 서비스 색인 폴더(그 안에 `index-v2.bin`)
pub fn service_index_dir(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("index")
}

/// 앱에서 쓰는 표시 이름
pub const SERVICE_SOURCE_LABEL: &str = "빠른 색인(서비스)";

#[cfg(test)]
mod tests {
    #[test]
    fn per_user_connection_cap() {
        let l = super::ConnLimiter::default();
        let (a, b) = ("S-1-5-21-1-1001", "S-1-5-21-1-1002");
        for _ in 0..super::MAX_CLIENTS_PER_USER {
            assert!(l.try_acquire(a, super::MAX_CLIENTS_PER_USER));
        }
        // 한 사용자가 한도를 채워도 다른 사용자는 들어온다
        assert!(!l.try_acquire(a, super::MAX_CLIENTS_PER_USER));
        assert!(l.try_acquire(b, super::MAX_CLIENTS_PER_USER));
        assert!(super::MAX_CLIENTS_PER_USER * 2 < super::MAX_CLIENTS, "한 사람이 전체를 못 채움");
        l.release(a);
        assert_eq!(l.count(a), super::MAX_CLIENTS_PER_USER - 1);
        assert!(l.try_acquire(a, super::MAX_CLIENTS_PER_USER));
        for _ in 0..10 {
            l.release(b);
        }
        assert_eq!(l.count(b), 0);
    }

    use super::*;

    #[test]
    fn fold_and_join() {
        assert_eq!(fold_win(r"C:\Users\User\"), r"c:\users\user");
        assert_eq!(join_win(r"C:\Users\", "Public"), r"C:\Users\Public");
    }
}
