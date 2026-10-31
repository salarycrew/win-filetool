//! 2a 검색: 질의 언어(모든 연산자와 조합), NFC/NFD·초성, 크기·날짜, 저장→읽기, 실제 디스크 스캔,
//! 실시간 갱신(추가·이름 바꾸기·삭제).

use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use chrono::{FixedOffset, TimeZone};
use filetool_core::search::index::Index;
use filetool_core::search::query::{self, Query, QueryCtx, SortKey};
use filetool_core::search::scan::{self, DirWalkSource, Excludes, ScanControl, ScanOpts};
use filetool_core::search::service::{self, SearchConfig, SearchService};
use unicode_normalization::UnicodeNormalization;

fn nfd(s: &str) -> String {
    s.nfd().collect()
}

fn ctx() -> QueryCtx {
    let tz = FixedOffset::east_opt(9 * 3600).unwrap();
    QueryCtx { now: tz.with_ymd_and_hms(2026, 10, 29, 15, 0, 0).unwrap() } // 목요일
}

fn ts(y: i32, m: u32, d: u32, h: u32) -> i64 {
    FixedOffset::east_opt(9 * 3600).unwrap().with_ymd_and_hms(y, m, d, h, 0, 0).unwrap().timestamp()
}

const KB: u64 = 1024;
const MB: u64 = 1024 * 1024;

/// 시험용 색인(디스크 없이)
fn sample() -> Index {
    let mut ix = Index::new();
    let root = ix.add_root(Path::new(r"C:\"));
    let users = ix.push_child(root, "Users", true, 0, ts(2026, 1, 1, 9));
    let user = ix.push_child(users, "user", true, 0, ts(2026, 1, 1, 9));
    let docs = ix.push_child(user, "Documents", true, 0, ts(2026, 10, 29, 9));
    let pics = ix.push_child(user, "Pictures", true, 0, ts(2026, 10, 1, 9));
    let proj = ix.push_child(docs, "보고서 모음", true, 0, ts(2026, 10, 28, 9));
    ix.push_child(docs, "주간보고서_2024.hwp", false, 30 * KB, ts(2026, 10, 29, 10)); // 오늘
    ix.push_child(docs, &nfd("회의록.docx"), false, 12 * KB, ts(2026, 10, 28, 10)); // 어제, 맥에서 온 NFD 이름
    ix.push_child(docs, "Annual Report.PDF", false, 2 * MB, ts(2026, 10, 5, 10)); // 이번 달
    ix.push_child(docs, "budget.xlsx", false, 500 * KB, ts(2025, 12, 31, 23));
    ix.push_child(proj, "보고서.pdf", false, 11 * MB, ts(2026, 10, 27, 10)); // 이번 주 월요일
    ix.push_child(proj, &nfd("보고서_최종.pdf"), false, 3 * MB, ts(2026, 9, 30, 10));
    ix.push_child(pics, "IMG_0001.JPG", false, 4 * MB, ts(2024, 5, 1, 12));
    ix.push_child(pics, "IMG_0002.png", false, 1 * MB, ts(2024, 5, 2, 12));
    ix.push_child(pics, "holiday photo.jpg", false, 6 * MB, ts(2024, 7, 1, 12));
    ix.push_child(pics, "report.jpg.txt", false, 100, ts(2024, 7, 1, 12));
    ix
}

fn names(ix: &Index, q: &str) -> Vec<String> {
    let query = Query::parse(q, &ctx()).unwrap_or_else(|e| panic!("{q}: {e}"));
    let mut ids = query::run(ix, &query);
    query::sort(ix, &mut ids, SortKey::Name, false);
    ids.iter().map(|&i| ix.name(i).nfc().collect::<String>()).collect()
}

fn s(v: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = v.iter().map(|x| x.to_string()).collect();
    v.sort_by_key(|x| x.to_lowercase());
    v
}

fn assert_q(ix: &Index, q: &str, want: &[&str]) {
    let mut got = names(ix, q);
    got.sort_by_key(|x| x.to_lowercase());
    assert_eq!(got, s(want), "질의: {q}");
}

#[test]
fn query_operators() {
    let ix = sample();
    // 낱말 하나(대소문자 무시)
    assert_q(&ix, "report", &["Annual Report.PDF", "report.jpg.txt"]);
    assert_q(&ix, "REPORT", &["Annual Report.PDF", "report.jpg.txt"]);
    // AND(띄어쓰기)
    assert_q(&ix, "img 0001", &["IMG_0001.JPG"]);
    // OR
    assert_q(&ix, "budget|holiday", &["budget.xlsx", "holiday photo.jpg"]);
    // OR가 AND보다 먼저 묶임: img (0001 또는 0002)
    assert_q(&ix, "img 0001|0002", &["IMG_0001.JPG", "IMG_0002.png"]);
    assert_q(&ix, "img 0001 | 0002", &["IMG_0001.JPG", "IMG_0002.png"]);
    // NOT
    assert_q(&ix, "img !png", &["IMG_0001.JPG"]);
    assert_q(&ix, "보고서 !최종 !주간 file:", &["보고서.pdf"]);
    // 따옴표 구절(띄어쓰기 포함)
    assert_q(&ix, "\"holiday photo\"", &["holiday photo.jpg"]);
    assert_q(&ix, "\"annual report\"", &["Annual Report.PDF"]);
    assert_q(&ix, "!\"보고서\" ext:pdf", &["Annual Report.PDF"]);
    // 와일드카드: 이름 전체
    assert_q(&ix, "*.jpg", &["IMG_0001.JPG", "holiday photo.jpg"]);
    assert_q(&ix, "img_000?.*", &["IMG_0001.JPG", "IMG_0002.png"]);
    assert_q(&ix, "report*", &["report.jpg.txt"]);
    // ext:
    assert_q(&ix, "ext:jpg;png", &["IMG_0001.JPG", "IMG_0002.png", "holiday photo.jpg"]);
    assert_q(&ix, "ext:pdf", &["Annual Report.PDF", "보고서.pdf", "보고서_최종.pdf"]);
    // file: / folder:
    assert_q(&ix, "folder:", &["Users", "user", "Documents", "Pictures", "보고서 모음"]);
    assert_q(&ix, "folder:보고서", &["보고서 모음"]);
    assert_q(&ix, "file:보고서", &["주간보고서_2024.hwp", "보고서.pdf", "보고서_최종.pdf"]);
    // 경로 낱말(\ 포함)
    assert_q(&ix, r"pictures\img", &["IMG_0001.JPG", "IMG_0002.png"]);
    // 경로에 "들어 있음"이므로 더 깊은 항목도 맞음
    assert_q(&ix, r"user\documents\", &["보고서 모음", "주간보고서_2024.hwp", "회의록.docx", "Annual Report.PDF", "budget.xlsx", "보고서.pdf", "보고서_최종.pdf"]);
    assert_q(&ix, r"c:\users\user\documents\*", &["보고서 모음", "주간보고서_2024.hwp", "회의록.docx", "Annual Report.PDF", "budget.xlsx", "보고서.pdf", "보고서_최종.pdf"]);
    assert_q(&ix, r"user\documents\ !모음\", &["보고서 모음", "주간보고서_2024.hwp", "회의록.docx", "Annual Report.PDF", "budget.xlsx"]);
    assert_q(&ix, r"모음\보고서", &["보고서.pdf", "보고서_최종.pdf"]);
    assert_q(&ix, r"c:\users\*\pictures\*.png", &["IMG_0002.png"]);
    assert_q(&ix, "users/user/pictures/holiday", &["holiday photo.jpg"]);
    // 조합
    assert_q(&ix, r"ext:pdf size:>2mb documents\ !최종", &["보고서.pdf"]);
    assert_q(&ix, "ext:jpg|ext:png size:<5mb", &["IMG_0001.JPG", "IMG_0002.png"]);
    assert_q(&ix, "file: dm:thismonth !ext:pdf", &["주간보고서_2024.hwp", "회의록.docx"]);
    // 빈 질의는 조건 없음
    assert!(Query::parse("   ", &ctx()).unwrap().is_empty());
    // 잘못된 값은 한국어 오류
    assert!(Query::parse("size:>10zb", &ctx()).unwrap_err().contains("단위"));
    assert!(Query::parse("dm:2024-13-01", &ctx()).unwrap_err().contains("날짜"));
}

#[test]
fn size_and_date_filters() {
    let ix = sample();
    assert_q(&ix, "size:>10mb", &["보고서.pdf"]);
    assert_q(&ix, "size:>=6mb", &["holiday photo.jpg", "보고서.pdf"]);
    assert_q(&ix, "size:<1kb", &["report.jpg.txt"]);
    assert_q(&ix, "size:1mb..4mb", &["IMG_0002.png", "IMG_0001.JPG", "Annual Report.PDF", "보고서_최종.pdf"]);
    assert_q(&ix, "size:500kb", &["budget.xlsx"]);
    // 폴더는 크기 조건에 들지 않음
    assert!(names(&ix, "size:<=1mb folder:").is_empty());
    // 날짜(기준: 2026-10-29 목요일 15시, +09:00)
    assert_q(&ix, "dm:today file:", &["주간보고서_2024.hwp"]);
    assert_q(&ix, "dm:yesterday file:", &["회의록.docx"]);
    assert_q(&ix, "dm:thisweek file:", &["주간보고서_2024.hwp", "회의록.docx", "보고서.pdf"]);
    assert_q(&ix, "dm:thismonth file:", &["주간보고서_2024.hwp", "회의록.docx", "보고서.pdf", "Annual Report.PDF"]);
    assert_q(&ix, "dm:today|dm:yesterday file:", &["주간보고서_2024.hwp", "회의록.docx"]);
    assert_q(&ix, "dm:2024-05-01", &["IMG_0001.JPG"]);
    assert_q(&ix, "dm:2024-05", &["IMG_0001.JPG", "IMG_0002.png"]);
    assert_q(&ix, "dm:2024", &["IMG_0001.JPG", "IMG_0002.png", "holiday photo.jpg", "report.jpg.txt"]);
    assert_q(&ix, "dm:2024-05..2024-07 ext:jpg", &["IMG_0001.JPG", "holiday photo.jpg"]);
    assert_q(&ix, "dm:<2025 ext:png", &["IMG_0002.png"]);
    assert_q(&ix, "dm:>2025-12-30 dm:<2026-01-01", &["budget.xlsx"]);
}

#[test]
fn nfc_nfd_and_choseong() {
    let ix = sample();
    // NFD로 저장된 이름(맥)을 NFC 질의로
    assert_q(&ix, "회의록", &["회의록.docx"]);
    assert_q(&ix, "최종", &["보고서_최종.pdf"]);
    // NFC 이름을 NFD 질의로
    assert_q(&ix, &nfd("주간보고서"), &["주간보고서_2024.hwp"]);
    assert_q(&ix, &format!("\"{}\"", nfd("보고서 모음")), &["보고서 모음"]);
    // 경로 낱말도 NFD/NFC 무시
    assert_q(&ix, &nfd(r"모음\보고서_최"), &["보고서_최종.pdf"]);
    // 초성
    assert_q(&ix, "ㅂㄱㅅ", &["주간보고서_2024.hwp", "보고서 모음", "보고서.pdf", "보고서_최종.pdf"]);
    assert_q(&ix, "ㅎㅇㄹ", &["회의록.docx"]); // NFD 이름도 초성으로
    assert_q(&ix, "ㅈㄱㅂㄱㅅ", &["주간보고서_2024.hwp"]);
    assert_q(&ix, "ㅂㄱㅅ ext:pdf !ㅊㅈ", &["보고서.pdf"]);
    assert!(names(&ix, "ㅂㄱㅎ").is_empty());
}

#[test]
fn sorting() {
    let ix = sample();
    let q = Query::parse("ext:jpg;png;pdf", &ctx()).unwrap();
    let mut ids = query::run(&ix, &q);
    query::sort(&ix, &mut ids, SortKey::Size, true);
    assert_eq!(ix.name(ids[0]), "보고서.pdf");
    query::sort(&ix, &mut ids, SortKey::Modified, false);
    assert_eq!(ix.name(ids[0]), "IMG_0001.JPG");
    query::sort(&ix, &mut ids, SortKey::Path, false);
    let first = query::row(&ix, ids[0]);
    assert!(first.folder.ends_with("Documents"), "{first:?}");
    let r = query::row(&ix, ids[0]);
    assert_eq!(Path::new(&r.path).file_name().unwrap().to_string_lossy(), r.name);
}

#[test]
fn save_load_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let mut ix = sample();
    // 하나 지우고(정리되어 저장됨) 저장
    let id = ix.lookup(Path::new(r"C:\").join("Users").join("user").join("Pictures").join("report.jpg.txt").as_path());
    assert!(id.is_some(), "경로 찾기");
    ix.remove_subtree(id.unwrap());
    ix.roots_mut()[0].stats.denied = 7;
    ix.roots_mut()[0].last_scan = 12345;
    let f = dir.path().join("index-v1.bin");
    let bytes = ix.save(&f).unwrap();
    assert!(bytes > 0);
    let back = Index::load(&f).unwrap();
    assert_eq!(back.live_count(), ix.live_count());
    assert_eq!(back.roots()[0].stats.denied, 7);
    assert_eq!(back.roots()[0].last_scan, 12345);
    for q in ["report", "ㅂㄱㅅ", "회의록", r"pictures\img", "size:>1mb", "dm:thisweek", "*.jpg", "folder:"] {
        assert_eq!(names(&back, q), names(&ix, q), "질의 {q}");
    }
    // 경로도 같다
    let q = Query::parse("ext:pdf", &ctx()).unwrap();
    let mut a: Vec<String> = query::run(&ix, &q).iter().map(|&i| query::row(&ix, i).path).collect();
    let mut b: Vec<String> = query::run(&back, &q).iter().map(|&i| query::row(&back, i).path).collect();
    a.sort();
    b.sort();
    assert_eq!(a, b);
    // 깨진 파일 / 다른 형식은 거절
    let mut raw = std::fs::read(&f).unwrap();
    let mid = raw.len() / 2;
    raw[mid] ^= 0xff;
    std::fs::write(&f, &raw).unwrap();
    assert!(Index::load(&f).unwrap_err().to_string().contains("검사합"));
    std::fs::write(&f, b"not an index").unwrap();
    assert!(Index::load(&f).is_err());
    // 임시 파일이 남지 않음
    assert!(!dir.path().join("index-v1.bin.tmp").exists());
}

fn touch(p: &Path, bytes: usize) {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(p, vec![b'x'; bytes]).unwrap();
}

/// 실제 디스크 트리(NFD·한글 이름, 제외 폴더, 링크 고리)
fn make_tree(root: &Path) {
    touch(&root.join("문서").join(nfd("보고서.hwp")), 10);
    touch(&root.join("문서").join("주간 보고서.docx"), 20);
    touch(&root.join("photos").join("IMG_1.jpg"), 2048);
    touch(&root.join("photos").join("deep").join("a").join("b").join("c.png"), 5);
    touch(&root.join("node_modules").join("pkg").join("index.js"), 5);
    touch(&root.join(nfd("맥에서 온 폴더")).join(nfd("사진.jpg")), 7);
    #[cfg(unix)]
    std::os::unix::fs::symlink(root, root.join("photos").join("loop")).unwrap();
}

#[test]
fn disk_scan_excludes_and_links() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    make_tree(&root);
    let ix = RwLock::new(Index::new());
    let opts = ScanOpts { excludes: Excludes::new(&["node_modules".to_string()]), skip: vec![], threads: 4 };
    let st = scan::scan_root_into(&DirWalkSource, &root, &ix, &opts, &ScanControl::default());
    let ix = ix.into_inner().unwrap();
    assert_eq!(st.excluded, 1, "{st:?}");
    #[cfg(unix)]
    assert_eq!(st.reparse, 1, "링크는 따라가지 않고 셈: {st:?}");
    assert_eq!(st.files, 5, "{st:?}");
    assert_eq!(st.dirs, 6 + cfg!(unix) as u64, "{st:?}"); // 폴더를 가리키는 링크는 폴더 항목(따라가지 않음)
    assert_eq!(st.denied, 0);
    assert!(ix.roots()[0].last_scan > 0);
    assert_q(&ix, "보고서", &["보고서.hwp", "주간 보고서.docx"]);
    assert_q(&ix, "ㅂㄱㅅ", &["보고서.hwp", "주간 보고서.docx"]);
    assert_q(&ix, "사진", &["사진.jpg"]);
    assert_q(&ix, "index.js", &[]);
    assert_q(&ix, r"deep\a\b\", &["c.png"]);
    let p = ix.lookup(&root.join("photos").join("deep").join("a").join("b").join("c.png")).unwrap();
    assert_eq!(ix.full_path(p), root.join("photos").join("deep").join("a").join("b").join("c.png"));
    assert_eq!(ix.size(ix.lookup(&root.join("photos").join("IMG_1.jpg")).unwrap()), 2048);
}

#[test]
fn incremental_add_rename_delete() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    make_tree(&root);
    let ix = RwLock::new(Index::new());
    let opts = ScanOpts::default();
    scan::scan_root_into(&DirWalkSource, &root, &ix, &opts, &ScanControl::default());
    let roots = vec![root.clone()];
    let apply = |p: &Path| service::apply_path(&DirWalkSource, &ix, &roots, p, &opts);
    // 추가
    let newf = root.join("문서").join("새 파일.txt");
    touch(&newf, 3);
    assert!(apply(&newf));
    assert_q(&ix.read().unwrap(), "새 파일", &["새 파일.txt"]);
    // 크기 바뀜
    touch(&newf, 5000);
    apply(&newf);
    assert_q(&ix.read().unwrap(), "새 size:>4kb", &["새 파일.txt"]);
    // 이름 바꾸기(옛 경로 사라짐 + 새 경로 생김)
    let renamed = root.join("문서").join("이름바꿈.txt");
    std::fs::rename(&newf, &renamed).unwrap();
    apply(&newf);
    apply(&renamed);
    assert_q(&ix.read().unwrap(), "새 파일", &[]);
    assert_q(&ix.read().unwrap(), "이름바꿈", &["이름바꿈.txt"]);
    // 새 폴더(안에 파일 있음) — 부모를 모르는 깊은 경로 알림만 와도 부모부터 채움
    touch(&root.join("새폴더").join("안쪽").join("깊은.txt"), 1);
    assert!(apply(&root.join("새폴더").join("안쪽").join("깊은.txt")));
    assert_q(&ix.read().unwrap(), "깊은", &["깊은.txt"]);
    assert_q(&ix.read().unwrap(), r"새폴더\안쪽\", &["깊은.txt"]);
    // 폴더 이름 바꾸기 → 아래 항목 경로도 바뀜
    std::fs::rename(root.join("새폴더"), root.join("바뀐폴더")).unwrap();
    apply(&root.join("새폴더"));
    apply(&root.join("바뀐폴더"));
    assert_q(&ix.read().unwrap(), r"새폴더\", &[]);
    assert_q(&ix.read().unwrap(), r"바뀐폴더\안쪽\", &["깊은.txt"]);
    // 삭제(폴더째)
    std::fs::remove_dir_all(root.join("photos")).unwrap();
    assert!(apply(&root.join("photos")));
    assert_q(&ix.read().unwrap(), "img_1", &[]);
    assert_q(&ix.read().unwrap(), "c.png", &[]);
    // 루트 밖은 무시
    assert!(!apply(&dir.path().join("밖.txt")));
    // 버퍼 넘침 뒤 하위 폴더 다시 훑기
    touch(&root.join("문서").join("몰래 생김.txt"), 1);
    service::rescan_dir(&DirWalkSource, &ix, &root.join("문서"), &opts);
    assert_q(&ix.read().unwrap(), "몰래", &["몰래 생김.txt"]);
    assert_q(&ix.read().unwrap(), "이름바꿈", &["이름바꿈.txt"]);
    // 정리 후에도 같은 결과
    let c = ix.read().unwrap().compacted(None);
    assert_q(&c, "이름바꿈|깊은|몰래", &["이름바꿈.txt", "깊은.txt", "몰래 생김.txt"]);
    assert_eq!(c.deleted_count(), 0);
}

fn wait_for(svc: &SearchService, q: &str, want: usize, secs: u64) -> (bool, Duration) {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(secs) {
        if let Ok((ids, _)) = svc.query(q, SortKey::Name, false) {
            if ids.len() == want {
                return (true, t0.elapsed());
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    (false, t0.elapsed())
}

#[test]
fn service_live_updates_and_reload() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    make_tree(&root);
    let idx_dir = dir.path().join("index");
    let cfg = SearchConfig::new(vec![root.clone()], vec!["node_modules".into()], &idx_dir);
    let svc = SearchService::start(cfg.clone());
    let t0 = Instant::now();
    while !svc.status().ready && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(svc.status().ready);
    assert!(wait_for(&svc, "보고서", 2, 5).0);
    // 실시간: 만들기 → 이름 바꾸기 → 지우기
    let f = root.join("문서").join("live 시험.txt");
    touch(&f, 1);
    let (ok, t) = wait_for(&svc, "live", 1, 5);
    assert!(ok, "새 파일이 5초 안에 보여야 함");
    eprintln!("추가 반영 {t:?}");
    let g = root.join("문서").join(nfd("바뀐 이름.txt"));
    std::fs::rename(&f, &g).unwrap();
    assert!(wait_for(&svc, "바뀐 이름", 1, 5).0, "이름 바꾸기");
    assert!(wait_for(&svc, "live", 0, 5).0);
    std::fs::remove_file(&g).unwrap();
    assert!(wait_for(&svc, "바뀐", 0, 5).0, "삭제");
    // 새 폴더 트리
    touch(&root.join("new_dir").join("sub").join("x.txt"), 1);
    assert!(wait_for(&svc, r"new_dir\sub\x", 1, 5).0, "새 폴더 아래");
    let st = svc.status();
    assert!(st.live_updates > 0);
    // 저장 → 새 서비스가 불러옴
    let n = svc.save_now().unwrap();
    assert!(n > 0 && cfg.index_file.exists());
    svc.stop();
    let svc2 = SearchService::start(cfg.clone());
    let t0 = Instant::now();
    while svc2.status().loaded_ms.is_none() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let st2 = svc2.status();
    assert!(st2.loaded_ms.is_some(), "저장한 색인을 불러와야 함: {st2:?}");
    assert!(wait_for(&svc2, r"new_dir\sub\x", 1, 2).0);
    // 배경 다시 스캔이 끝나도 결과 같음
    let t0 = Instant::now();
    while svc2.status().roots.iter().any(|r| r.scanning) && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(wait_for(&svc2, "보고서|x.txt", 3, 3).0);
    svc2.stop();
}

/// 루트 바꿔 끼우기(배경 다시 스캔 결과 반영)
#[test]
fn root_replace_keeps_other_roots() {
    let mut a = Index::new();
    let r1 = a.add_root(Path::new("/r1"));
    a.push_child(r1, "old.txt", false, 1, 0);
    let r2 = a.add_root(Path::new("/r2"));
    a.push_child(r2, "keep.txt", false, 1, 0);
    let mut fresh = Index::new();
    let f1 = fresh.add_root(Path::new("/r1"));
    fresh.push_child(f1, "new.txt", false, 1, 0);
    let b = a.with_root_replaced(&fresh);
    assert_eq!(b.generation, a.generation + 1);
    assert_q(&b, "txt", &["new.txt", "keep.txt"]);
    assert_eq!(b.roots().len(), 2);
    assert_eq!(b.full_path(b.lookup(Path::new("/r2/keep.txt")).unwrap()), PathBuf::from("/r2/keep.txt"));
}
