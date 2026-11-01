//! 2c: 빠른 검색 서비스 — 파이프 프로토콜(봉투·틀·버전·한도), 요청 처리(hello 먼저, 질의·줄, 오래된 결과),
//! 가시성(다른 사용자 프로필 제외, 관리자는 모두, 사용자 제외 패턴), 서비스 색인 파일의 저널 따라잡기 재사용.

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use filetool_core::search::index::{Index, SourceKind, UsnCursor, INDEX_FILE};
use filetool_core::search::mft::{FileState, JournalErr, JournalInfo, VolumeView};
use filetool_core::search::ntfs::{frn, UsnRec};
use filetool_core::search::query::SortKey;
use filetool_core::search::scan::{self, Excludes, RawEntry, ScanOpts};
use filetool_core::search::service::try_catch_up;
use filetool_core::search::svcproto::{
    self, decode_request, decode_response, encode_request, encode_response, read_frame, serve, visible_paths, write_frame, Client, FrameErr, Handler,
    Identity, QueryReply, Request, Response, Session, SvcStatus, Visibility, MAX_EXCLUDES, MAX_QUERY_BYTES, MAX_REQUEST_BYTES, MAX_ROWS,
    PROTOCOL_VERSION,
};

// ---------- 봉투·틀 ----------

#[test]
fn requests_and_responses_round_trip() {
    let reqs = vec![
        Request::Hello { client: "win-filetool 0.2.1".into() },
        Request::Status,
        Request::Query { q: "보고서 ext:pdf".into(), sort: SortKey::Modified, desc: true, limit: 50, excludes: vec!["node_modules".into()] },
        Request::Rows { offset: 100, limit: 100 },
    ];
    for r in &reqs {
        let b = encode_request(r);
        let v: serde_json::Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(v["v"], PROTOCOL_VERSION, "봉투에 버전: {v}");
        assert!(v["op"].is_string(), "op 꼬리표: {v}");
        assert_eq!(&decode_request(&b).unwrap(), r);
    }
    // 빠진 선택 항목은 기본값
    let q = decode_request(br#"{"v":1,"op":"query","q":"abc"}"#).unwrap();
    assert_eq!(q, Request::Query { q: "abc".into(), sort: SortKey::Name, desc: false, limit: 0, excludes: vec![] });

    let resps = vec![
        Response::Status(SvcStatus { entries: 5, ready: true, version: 3, ..Default::default() }),
        Response::Query(QueryReply { total: 2, millis: 1.5, generation: 1, version: 3, rows: vec![] }),
        Response::Rows { rows: vec![], stale: true },
        Response::error("limit", "너무 큼"),
    ];
    for r in &resps {
        assert_eq!(&decode_response(&encode_response(r)).unwrap(), r);
    }
}

#[test]
fn version_and_garbage_rejected() {
    let code = |b: &[u8]| match decode_request(b) {
        Err(Response::Error { code, .. }) => code,
        other => panic!("{other:?}"),
    };
    assert_eq!(code(br#"{"v":2,"op":"status"}"#), "version");
    assert_eq!(code(br#"{"op":"status"}"#), "bad_request");
    assert_eq!(code(br#"{"v":1,"op":"delete","path":"C:\\x"}"#), "bad_request", "쓰기·지우기 요청은 없음");
    assert_eq!(code(br#"{"v":1,"op":"query"}"#), "bad_request", "q 없음");
    assert_eq!(code(b"\xff\xfe not json"), "bad_request");
    // 클라이언트도 버전이 다른 응답은 거절
    assert!(decode_response(br#"{"v":9,"op":"status"}"#).is_err());
}

#[test]
fn frame_limits() {
    let mut buf = Vec::new();
    write_frame(&mut buf, b"hello").unwrap();
    assert_eq!(&buf[..4], &5u32.to_le_bytes());
    assert_eq!(read_frame(&mut Cursor::new(&buf), 16).unwrap().unwrap(), b"hello");
    // 한도를 넘는 길이는 본문을 읽지 않고(할당도 없이) 거절
    let mut big = Vec::new();
    big.extend_from_slice(&(MAX_REQUEST_BYTES as u32 + 1).to_le_bytes());
    assert!(matches!(read_frame(&mut Cursor::new(&big), MAX_REQUEST_BYTES), Err(FrameErr::TooLarge(n)) if n == MAX_REQUEST_BYTES + 1));
    let mut huge = Vec::new();
    huge.extend_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(read_frame(&mut Cursor::new(&huge), MAX_REQUEST_BYTES), Err(FrameErr::TooLarge(_))));
    // 빈 입력 = 깨끗이 끊김, 길이 중간에 끊김 = 오류
    assert!(read_frame(&mut Cursor::new(Vec::<u8>::new()), 16).unwrap().is_none());
    assert!(read_frame(&mut Cursor::new(vec![1u8, 0]), 16).is_err());
    assert!(read_frame(&mut Cursor::new(vec![9u8, 0, 0, 0, 1, 2]), 16).is_err(), "본문이 모자람");
}

// ---------- 시험용 색인 ----------

const T: i64 = 1_760_000_000;

/// C:\Users\{user, other, Public, Default}, 서비스 계정 프로필, 휴지통 밖 일반 폴더
fn sample() -> Index {
    let mut ix = Index::new();
    let c = ix.add_root(Path::new(r"C:\"));
    let users = ix.push_child(c, "Users", true, 0, T);
    let user = ix.push_child(users, "user", true, 0, T);
    let docs = ix.push_child(user, "Documents", true, 0, T);
    ix.push_child(docs, "secret-user.txt", false, 10, T);
    let nm = ix.push_child(docs, "node_modules", true, 0, T);
    ix.push_child(nm, "secret-dep.js", false, 1, T);
    let other = ix.push_child(users, "Other", true, 0, T);
    let od = ix.push_child(other, "Desktop", true, 0, T);
    ix.push_child(od, "secret-other.txt", false, 20, T);
    let dummy = ix.push_child(users, "wft2c-other", true, 0, T);
    ix.push_child(dummy, "secret-dummy.txt", false, 30, T);
    let public = ix.push_child(users, "Public", true, 0, T);
    ix.push_child(public, "secret-public.txt", false, 40, T);
    let def = ix.push_child(users, "Default", true, 0, T);
    ix.push_child(def, "secret-default.txt", false, 50, T);
    let win = ix.push_child(c, "Windows", true, 0, T);
    let sp = ix.push_child(win, "ServiceProfiles", true, 0, T);
    let ls = ix.push_child(sp, "LocalService", true, 0, T);
    ix.push_child(ls, "secret-service.txt", false, 60, T);
    let data = ix.push_child(c, "Data", true, 0, T);
    ix.push_child(data, "secret-shared.txt", false, 70, T);
    ix
}

fn me() -> Identity {
    Identity { user: r"PC\user".into(), sid: "S-1-5-21-1-2-3-1001".into(), admin: false, profile: Some(r"C:\Users\user".into()) }
}

fn profile_list() -> Vec<String> {
    vec![
        r"C:\Windows\system32\config\systemprofile".into(),
        r"C:\Windows\ServiceProfiles\LocalService".into(),
        r"C:\Windows\ServiceProfiles\NetworkService".into(),
        r"C:\Users\user".into(),
        r"C:\Users\Other".into(),
    ]
}

fn names(paths: &[String]) -> Vec<String> {
    let mut v: Vec<String> = paths.iter().map(|p| p.rsplit(['\\', '/']).next().unwrap().to_string()).collect();
    v.sort();
    v
}

// ---------- 가시성 ----------

#[test]
fn visibility_hides_other_profiles_admin_sees_all() {
    let ix = sample();
    let vis = Visibility::for_identity(&me(), r"C:\Users", &profile_list());
    assert!(!vis.all);
    let got = names(&visible_paths(&ix, "secret", &vis, &[]).unwrap());
    assert_eq!(
        got,
        vec!["secret-dep.js", "secret-user.txt", "secret-public.txt", "secret-shared.txt"],
        "자기 프로필·Public·프로필 밖만 — 다른 사용자(Other), 프로필 목록에 없는 C:\\Users 아래 폴더(wft2c-other, Default), 서비스 계정 프로필은 숨김"
    );
    // 숨긴 폴더 이름 자체도 안 보임
    assert!(visible_paths(&ix, "wft2c-other", &vis, &[]).unwrap().is_empty());
    assert!(visible_paths(&ix, "folder:other", &vis, &[]).unwrap().is_empty());
    assert_eq!(names(&visible_paths(&ix, "folder:user", &vis, &[]).unwrap()), vec!["user"]);
    // 경로 낱말로 찔러 봐도 마찬가지
    assert!(visible_paths(&ix, r"users\other\", &vis, &[]).unwrap().is_empty());

    // 관리자(권한 상승된 토큰)는 모두
    let admin = Identity { admin: true, ..me() };
    let va = Visibility::for_identity(&admin, r"C:\Users", &profile_list());
    assert!(va.all);
    assert_eq!(visible_paths(&ix, "secret", &va, &[]).unwrap().len(), 8);

    // 대소문자·구분자가 달라도 같은 규칙(프로필 경로가 소문자로 등록돼 있어도)
    let lower = Identity { profile: Some("c:/users/USER/".into()), ..me() };
    let vl = Visibility::for_identity(&lower, r"C:\Users\", &profile_list());
    assert_eq!(names(&visible_paths(&ix, "secret", &vl, &[]).unwrap()), vec!["secret-dep.js", "secret-user.txt", "secret-public.txt", "secret-shared.txt"]);

    // 프로필을 모르는 사용자(프로필 목록에 없음)는 C:\Users 아래에서 Public만
    let nobody = Identity { profile: None, ..me() };
    let vn = Visibility::for_identity(&nobody, r"C:\Users", &profile_list());
    assert_eq!(names(&visible_paths(&ix, "secret", &vn, &[]).unwrap()), vec!["secret-public.txt", "secret-shared.txt"]);
}

#[test]
fn visibility_follows_renames_and_client_excludes() {
    let mut ix = sample();
    let vis = Visibility::for_identity(&me(), r"C:\Users", &profile_list());
    // 사용자 제외 패턴(서비스 기본 목록에 없는 것만): node_modules 아래 빠짐. 기본 목록은 그대로 둬도 결과 같음
    let mut ex = scan::default_excludes();
    ex.push("node_modules".into());
    assert_eq!(names(&visible_paths(&ix, "secret", &vis, &ex).unwrap()), vec!["secret-user.txt", "secret-public.txt", "secret-shared.txt"]);
    assert_eq!(names(&visible_paths(&ix, "secret", &vis, &["*\\Data".into()]).unwrap()), vec!["secret-dep.js", "secret-user.txt", "secret-public.txt"]);
    // 숨긴 폴더 안으로 옮긴 파일은 숨고, 그 폴더 이름을 바꿔도(질의 때마다 다시 구함) 규칙을 따른다
    let users = ix.lookup_test(r"C:\Users");
    let pubf = ix.find_child(users, "Public").unwrap();
    ix.update_entry(pubf, "Public2", 0, T);
    assert_eq!(names(&visible_paths(&ix, "secret", &vis, &[]).unwrap()), vec!["secret-dep.js", "secret-user.txt", "secret-shared.txt"], "Public2는 공유 폴더가 아님");
}

/// Linux에서 `C:\` 루트를 경로로 찾기 위한 도우미
trait LookupTest {
    fn lookup_test(&self, p: &str) -> u32;
}
impl LookupTest for Index {
    fn lookup_test(&self, p: &str) -> u32 {
        svcproto::lookup_win(self, p).unwrap()
    }
}

// ---------- 요청 처리 ----------

fn handler(ix: Index) -> Handler {
    let index = Arc::new(RwLock::new(ix));
    let i2 = index.clone();
    Handler {
        index,
        status: Box::new(move || {
            let ix = i2.read().unwrap();
            SvcStatus { entries: ix.live_count(), ready: true, version: ix.version, generation: ix.generation, ..Default::default() }
        }),
    }
}

fn session(id: Identity) -> Session {
    let vis = Visibility::for_identity(&id, r"C:\Users", &profile_list());
    Session::new(id, vis)
}

#[test]
fn handler_hello_first_query_rows_and_limits() {
    let h = handler(sample());
    let mut s = session(me());
    let code = |r: Response| match r {
        Response::Error { code, .. } => code,
        o => panic!("{o:?}"),
    };
    assert_eq!(code(h.handle(&mut s, Request::Status)), "hello_first");
    match h.handle(&mut s, Request::Hello { client: "test".into() }) {
        Response::Hello { protocol, client, .. } => {
            assert_eq!(protocol, PROTOCOL_VERSION);
            assert!(!client.admin);
            assert!(client.visibility.contains("다른 사용자"));
        }
        o => panic!("{o:?}"),
    }
    // 질의: 거른 뒤의 수, 이름순, 첫 2줄
    let r = match h.handle(&mut s, Request::Query { q: "secret".into(), sort: SortKey::Name, desc: false, limit: 2, excludes: vec![] }) {
        Response::Query(r) => r,
        o => panic!("{o:?}"),
    };
    assert_eq!(r.total, 4);
    assert_eq!(r.rows.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), vec!["secret-dep.js", "secret-user.txt"]);
    // 다음 줄들
    match h.handle(&mut s, Request::Rows { offset: 2, limit: 10 }) {
        Response::Rows { rows, stale } => {
            assert!(!stale);
            assert_eq!(rows.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), vec!["secret-public.txt", "secret-shared.txt"]);
        }
        o => panic!("{o:?}"),
    }
    // 범위 밖·큰 수도 안전하게
    match h.handle(&mut s, Request::Rows { offset: usize::MAX, limit: usize::MAX }) {
        Response::Rows { rows, stale } => assert!(rows.is_empty() && !stale),
        o => panic!("{o:?}"),
    }
    // 한도
    assert_eq!(code(h.handle(&mut s, Request::Query { q: "a".repeat(MAX_QUERY_BYTES + 1), sort: SortKey::Name, desc: false, limit: 0, excludes: vec![] })), "limit");
    assert_eq!(
        code(h.handle(&mut s, Request::Query { q: "a".into(), sort: SortKey::Name, desc: false, limit: 0, excludes: vec!["x".into(); MAX_EXCLUDES + 1] })),
        "limit"
    );
    assert_eq!(code(h.handle(&mut s, Request::Query { q: "size:>10zb".into(), sort: SortKey::Name, desc: false, limit: 0, excludes: vec![] })), "query");
    // 줄 수는 MAX_ROWS로 줄임
    let mut big = Index::new();
    let r0 = big.add_root(Path::new(r"D:\"));
    for i in 0..(MAX_ROWS + 500) {
        big.push_child(r0, &format!("f{i:05}.txt"), false, 1, T);
    }
    let hb = handler(big);
    let mut sb = session(me());
    hb.handle(&mut sb, Request::Hello { client: "t".into() });
    match hb.handle(&mut sb, Request::Query { q: "*.txt".into(), sort: SortKey::Name, desc: false, limit: usize::MAX, excludes: vec![] }) {
        Response::Query(r) => {
            assert_eq!(r.total, MAX_ROWS + 500);
            assert_eq!(r.rows.len(), MAX_ROWS);
        }
        o => panic!("{o:?}"),
    }
    match hb.handle(&mut sb, Request::Rows { offset: 0, limit: 5000 }) {
        Response::Rows { rows, .. } => assert_eq!(rows.len(), MAX_ROWS),
        o => panic!("{o:?}"),
    }
    // 색인이 통째로 바뀌면(세대) 줄 요청은 stale → 다시 질의
    {
        let mut g = hb.index.write().unwrap();
        let fresh = g.compacted(None);
        *g = fresh;
        g.generation += 1;
    }
    match hb.handle(&mut sb, Request::Rows { offset: 0, limit: 5 }) {
        Response::Rows { stale, .. } => assert!(stale),
        o => panic!("{o:?}"),
    }
    // 관리자 세션은 모두
    let mut sa = session(Identity { admin: true, ..me() });
    h.handle(&mut sa, Request::Hello { client: "t".into() });
    match h.handle(&mut sa, Request::Query { q: "secret".into(), sort: SortKey::Name, desc: false, limit: 0, excludes: vec![] }) {
        Response::Query(r) => assert_eq!(r.total, 8),
        o => panic!("{o:?}"),
    }
}

/// 요청 바이트를 읽고 응답 바이트를 모으는 가짜 연결
struct Duplex {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
}
impl Read for Duplex {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.input.read(b)
    }
}
impl Write for Duplex {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.output.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn frames(b: &[u8]) -> Vec<Response> {
    let mut c = Cursor::new(b.to_vec());
    let mut out = Vec::new();
    while let Some(f) = read_frame(&mut c, usize::MAX).unwrap() {
        out.push(decode_response(&f).unwrap());
    }
    out
}

#[test]
fn serve_handles_stream_and_cuts_oversized() {
    let h = handler(sample());
    let mut input = Vec::new();
    for r in [Request::Hello { client: "t".into() }, Request::Status, Request::Query { q: "secret".into(), sort: SortKey::Size, desc: true, limit: 1, excludes: vec![] }] {
        write_frame(&mut input, &encode_request(&r)).unwrap();
    }
    write_frame(&mut input, br#"{"v":1,"op":"exec","cmd":"calc"}"#).unwrap();
    // 한도를 넘는 틀 → 오류 응답 후 끊음(뒤의 요청은 처리 안 함)
    input.extend_from_slice(&((MAX_REQUEST_BYTES + 10) as u32).to_le_bytes());
    input.extend_from_slice(&vec![b' '; 64]);
    write_frame(&mut input, &encode_request(&Request::Status)).unwrap();
    let mut d = Duplex { input: Cursor::new(input), output: Vec::new() };
    let mut s = session(me());
    serve(&mut d, &h, &mut s).unwrap();
    let out = frames(&d.output);
    assert_eq!(out.len(), 5, "{out:?}");
    assert!(matches!(out[0], Response::Hello { .. }));
    assert!(matches!(&out[1], Response::Status(st) if st.entries == h.index.read().unwrap().live_count()));
    match &out[2] {
        Response::Query(r) => {
            assert_eq!(r.total, 4);
            assert_eq!(r.rows[0].name, "secret-shared.txt", "크기 큰 순(보이는 것 중 70 B)");
        }
        o => panic!("{o:?}"),
    }
    assert!(matches!(&out[3], Response::Error { code, .. } if code == "bad_request"));
    assert!(matches!(&out[4], Response::Error { code, .. } if code == "limit"));
}

#[cfg(unix)]
#[test]
fn client_over_socket() {
    use std::os::unix::net::UnixStream;
    let (a, mut b) = UnixStream::pair().unwrap();
    let h = Arc::new(handler(sample()));
    let h2 = h.clone();
    let t = std::thread::spawn(move || {
        let mut s = session(me());
        serve(&mut b, &h2, &mut s).unwrap();
        s.requests
    });
    let mut c = Client::handshake(a, "test-client").unwrap();
    assert!(!c.me.admin && c.me.sid.starts_with("S-1-5-21"));
    assert!(c.status().unwrap().ready);
    let r = c.query("secret", SortKey::Name, false, 3, &["node_modules".into()]).unwrap();
    assert_eq!(r.total, 3);
    assert_eq!(c.rows(3, 10).unwrap().unwrap().len(), 0);
    assert_eq!(c.rows(1, 10).unwrap().unwrap().len(), 2);
    let e = c.query("size:>1zb", SortKey::Name, false, 3, &[]).unwrap_err();
    assert!(e.contains("[query]"), "{e}");
    drop(c);
    assert_eq!(t.join().unwrap(), 6, "hello + status + query + rows×2 + 오류 질의");
}

// ---------- 서비스 색인: 저장 → 다시 시작 → 저널 따라잡기 ----------

struct View {
    journal: JournalInfo,
    log: Vec<UsnRec>,
    files: HashMap<u64, FileState>,
}
struct FakeView(Mutex<View>);
impl VolumeView for FakeView {
    fn serial(&self) -> u64 {
        9
    }
    fn query_journal(&self) -> Result<JournalInfo, JournalErr> {
        Ok(self.0.lock().unwrap().journal)
    }
    fn read_journal(&self, id: u64, start: i64) -> Result<(i64, Vec<UsnRec>), JournalErr> {
        let v = self.0.lock().unwrap();
        if id != v.journal.journal_id {
            return Err(JournalErr::Reset("id".into()));
        }
        let recs: Vec<UsnRec> = v.log.iter().filter(|r| r.usn >= start).cloned().collect();
        Ok((recs.last().map(|r| r.usn + 1).unwrap_or(v.journal.next_usn), recs))
    }
    fn record(&self, f: u64) -> Option<FileState> {
        self.0.lock().unwrap().files.get(&(f & 0xffff_ffff_ffff)).filter(|s| s.frn == f).cloned()
    }
    fn list_dir(&self, _p: &Path) -> std::io::Result<Vec<(u64, RawEntry)>> {
        Ok(vec![])
    }
}

#[test]
fn service_index_file_catch_up_reuse() {
    let tmp = tempfile::tempdir().unwrap();
    // 서비스 자료 폴더 구조: <ProgramData>\kr.salarycrew.winfiletool\index\index-v2.bin
    let data = tmp.path().join("kr.salarycrew.winfiletool");
    let idx_dir = svcproto::service_index_dir(&data);
    assert!(idx_dir.ends_with("index"));
    let file = idx_dir.join(INDEX_FILE);

    // MFT로 채운 루트(FRN 있음) + 저널 위치 → 서비스가 끝날 때 저장
    let root = frn(5, 5);
    let (docs, a) = (frn(20, 1), frn(30, 1));
    let mut ix = Index::new();
    let rid = ix.add_root(Path::new("/vol"));
    let d = ix.push_child_frn(rid, "docs", true, 0, T, docs);
    ix.push_child_frn(d, "a.txt", false, 1, T, a);
    {
        let r = ix.roots_mut().iter_mut().find(|r| r.id == rid).unwrap();
        r.source = SourceKind::Mft;
        r.usn = Some(UsnCursor { volume_serial: 9, journal_id: 77, next_usn: 100 });
    }
    ix.save(&file).unwrap();

    // 다시 시작: 같은 파일을 불러와 같은 저널이면 전체 읽기 대신 따라잡기(서비스와 앱이 같은 함수)
    let loaded = Index::load_dir_file(&file).unwrap();
    assert_eq!(loaded.roots()[0].source, SourceKind::Mft);
    let index = RwLock::new(loaded);
    let new_file = frn(31, 1);
    let mut files = HashMap::new();
    files.insert(20, FileState { frn: docs, is_dir: true, link: false, size: 0, mtime: T, names: vec![(root, "docs".into())] });
    files.insert(31, FileState { frn: new_file, is_dir: false, link: false, size: 7, mtime: T, names: vec![(docs, "new-while-stopped.txt".into())] });
    let view = FakeView(Mutex::new(View {
        journal: JournalInfo { journal_id: 77, first_usn: 10, next_usn: 150 },
        log: vec![UsnRec { frn: new_file, parent: docs, usn: 120, reason: 0, attrs: 0, name: String::new(), major: 2 }, UsnRec { frn: a, parent: docs, usn: 130, reason: 0, attrs: 0, name: String::new(), major: 2 }],
        files,
    }));
    let opts = ScanOpts { excludes: Excludes::new(&scan::default_excludes()), skip: vec![idx_dir.clone()], threads: 0 };
    let msg = try_catch_up(&index, &view, Path::new("/vol"), &opts).expect("같은 저널 → 따라잡기");
    assert!(msg.contains("기록 2건"), "{msg}");
    {
        let ix = index.read().unwrap();
        assert!(ix.lookup(&PathBuf::from("/vol/docs/new-while-stopped.txt")).is_some(), "멈춘 동안 생긴 파일");
        assert!(ix.lookup(&PathBuf::from("/vol/docs/a.txt")).is_none(), "멈춘 동안 지운 파일");
        assert_eq!(ix.roots()[0].usn.unwrap().next_usn, 150, "다음 위치 갱신");
    }
    // 저장 → 다시 불러오면 갱신된 위치가 남음
    index.read().unwrap().save(&file).unwrap();
    assert_eq!(Index::load_dir_file(&file).unwrap().roots()[0].usn.unwrap().next_usn, 150);

    // 저널이 다시 만들어졌으면(ID 다름) 따라잡지 않음 → 서비스가 MFT 전체 읽기
    view.0.lock().unwrap().journal.journal_id = 78;
    assert!(try_catch_up(&index, &view, Path::new("/vol"), &opts).is_none());
    // 폴더 훑기로 채운 루트(2a 색인)는 따라잡기 대상 아님
    let mut walk = Index::new();
    walk.add_root(Path::new("/vol"));
    assert!(try_catch_up(&RwLock::new(walk), &view, Path::new("/vol"), &opts).is_none());
}
