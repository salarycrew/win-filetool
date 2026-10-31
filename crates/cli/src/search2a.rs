//! 2a 명령: index / search / search-live, selftest 2a

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use filetool_core::search::index::{Index, INDEX_FILE};
use filetool_core::search::query::{self, fmt_time, Query, QueryCtx, SortKey};
use filetool_core::search::scan::{self, default_excludes, DirWalkSource, Excludes, ScanControl, ScanOpts};
use filetool_core::search::service::{default_index_dir, SearchConfig, SearchService};
use filetool_core::{samples::nfd, Error, Result};

const SWITCHES: [&str; 3] = ["--desc", "--no-default-excludes", "--paths"];

struct Args {
    opts: Vec<(String, Option<String>)>,
    rest: Vec<String>,
}

impl Args {
    fn parse(args: &[String]) -> Self {
        let (mut opts, mut rest) = (Vec::new(), Vec::new());
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a.starts_with("--") && a.len() > 2 {
                if SWITCHES.contains(&a.as_str()) {
                    opts.push((a.clone(), None));
                } else {
                    opts.push((a.clone(), args.get(i + 1).cloned()));
                    i += 1;
                }
            } else {
                rest.push(a.clone());
            }
            i += 1;
        }
        Self { opts, rest }
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.opts.iter().rev().find(|(n, _)| n == k).and_then(|(_, v)| v.as_deref())
    }
    fn all(&self, k: &str) -> Vec<String> {
        self.opts.iter().filter(|(n, _)| n == k).filter_map(|(_, v)| v.clone()).collect()
    }
    fn has(&self, k: &str) -> bool {
        self.opts.iter().any(|(n, _)| n == k)
    }
    fn num(&self, k: &str, d: u64) -> Result<u64> {
        self.get(k).map(|v| v.parse().map_err(|_| Error::Invalid(format!("{k} 값이 숫자가 아닙니다: {v}")))).unwrap_or(Ok(d))
    }
    fn index_dir(&self) -> PathBuf {
        self.get("--index-dir").map(PathBuf::from).unwrap_or_else(default_index_dir)
    }
    fn excludes(&self) -> Vec<String> {
        let mut v = if self.has("--no-default-excludes") { Vec::new() } else { default_excludes() };
        v.extend(self.all("--exclude"));
        v
    }
    fn roots(&self) -> Vec<PathBuf> {
        if self.rest.is_empty() {
            scan::fixed_drives()
        } else {
            self.rest.iter().map(PathBuf::from).collect()
        }
    }
}

fn mb(b: u64) -> String {
    format!("{:.1} MB", b as f64 / 1_048_576.0)
}

fn sort_key(s: Option<&str>) -> SortKey {
    match s {
        Some("path") | Some("folder") => SortKey::Path,
        Some("size") => SortKey::Size,
        Some("modified") | Some("dm") => SortKey::Modified,
        _ => SortKey::Name,
    }
}

pub fn run(cmd: &str, args: &[String]) -> Result<Option<ExitCode>> {
    match cmd {
        "index" => index(&Args::parse(&args[1..])).map(Some),
        "search" => search(&Args::parse(&args[1..])).map(Some),
        "search-live" => live(&Args::parse(&args[1..])).map(Some),
        _ => Ok(None),
    }
}

/// `index <루트…>`: 스캔 + 저장, 항목 수·시간·크기 출력
fn index(a: &Args) -> Result<ExitCode> {
    let roots = a.roots();
    let dir = a.index_dir();
    let file = dir.join(INDEX_FILE);
    let opts = ScanOpts { excludes: Excludes::new(&a.excludes()), skip: vec![dir.clone()], threads: a.num("--threads", 0)? as usize };
    println!("제외: {:?}", a.excludes());
    let ix = RwLock::new(Index::new());
    let t0 = Instant::now();
    for r in &roots {
        let ctl = ScanControl::default();
        let st = scan::scan_root_into(&DirWalkSource, r, &ix, &opts, &ctl);
        println!(
            "루트 {}: 폴더 {} / 파일 {} / 건너뜀 {} (권한 없음 {}, 연결 지점 {}) / 제외 {} / {:.2} 초",
            r.display(), st.dirs, st.files, st.denied + st.reparse, st.denied, st.reparse, st.excluded, st.millis as f64 / 1000.0
        );
    }
    let scan_s = t0.elapsed().as_secs_f64();
    let ix = ix.into_inner().unwrap();
    let t1 = Instant::now();
    let size = ix.save(&file)?;
    println!(
        "완료: 항목 {} / 스캔 {:.2} 초 / 저장 {:.0} ms / 색인 파일 {} ({}) / 메모리(색인 배열) {}",
        ix.live_count(), scan_s, t1.elapsed().as_secs_f64() * 1000.0, mb(size), file.display(), mb(ix.heap_bytes() as u64)
    );
    Ok(ExitCode::SUCCESS)
}

/// `search "<질의>"`: 저장한 색인을 읽어 찾기
fn search(a: &Args) -> Result<ExitCode> {
    let q = a.rest.join(" ");
    let file = a.index_dir().join(INDEX_FILE);
    let t0 = Instant::now();
    let ix = Index::load(&file)?;
    let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let query = Query::parse(&q, &QueryCtx::local_now()).map_err(Error::Invalid)?;
    let t1 = Instant::now();
    let mut ids = query::run(&ix, &query);
    let match_ms = t1.elapsed().as_secs_f64() * 1000.0;
    query::sort(&ix, &mut ids, sort_key(a.get("--sort")), a.has("--desc"));
    let total_ms = t1.elapsed().as_secs_f64() * 1000.0;
    let limit = a.num("--limit", 20)? as usize;
    for &id in ids.iter().take(limit) {
        let r = query::row(&ix, id);
        if a.has("--paths") {
            println!("{}", r.path);
        } else {
            let size = if r.is_dir { "<폴더>".to_string() } else { r.size.to_string() };
            println!("{:>12}  {}  {}", size, fmt_time(r.mtime), r.path);
        }
    }
    println!(
        "질의 {q:?}: {}개 / 찾기 {match_ms:.1} ms / 정렬 포함 {total_ms:.1} ms / 색인 읽기 {load_ms:.0} ms (항목 {})",
        ids.len(), ix.live_count()
    );
    Ok(ExitCode::SUCCESS)
}

/// `search-live <루트…> --query q --seconds N`: 앱과 같은 서비스를 돌리며(불러오기·배경 스캔·실시간 갱신)
/// 질의 결과가 바뀔 때마다 시각과 함께 출력
fn live(a: &Args) -> Result<ExitCode> {
    let q = a.get("--query").unwrap_or("").to_string();
    let secs = a.num("--seconds", 60)?;
    let cfg = SearchConfig::new(a.roots(), a.excludes(), &a.index_dir());
    let svc = SearchService::start(cfg);
    let t0 = Instant::now();
    let mut last: Option<Vec<String>> = None;
    let mut was_ready = false;
    while t0.elapsed() < Duration::from_secs(secs) {
        let st = svc.status();
        if st.ready && !st.roots.iter().any(|r| r.scanning) && !was_ready {
            was_ready = true;
            println!(
                "[{:6.2}s] 준비됨: 항목 {} / 저장한 색인 불러오기 {} / 건너뜀 {}",
                t0.elapsed().as_secs_f64(), st.entries, st.loaded_ms.map(|m| format!("{m} ms")).unwrap_or("없음".into()), st.skipped
            );
        }
        if let Ok((ids, r)) = svc.query(&q, SortKey::Name, false) {
            let rows = svc.rows(&ids[..ids.len().min(10)], r.generation).unwrap_or_default();
            let names: Vec<String> = rows.iter().map(|r| r.path.clone()).collect();
            if last.as_ref() != Some(&names) {
                println!("[{:6.2}s] {}개 ({:.1} ms): {:?}", t0.elapsed().as_secs_f64(), r.total, r.millis, names);
                last = Some(names);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = svc.save_now();
    svc.stop();
    Ok(ExitCode::SUCCESS)
}

// ---------- selftest 2a ----------

struct Checker {
    failed: usize,
    total: usize,
}

impl Checker {
    fn check(&mut self, name: &str, ok: bool, detail: String) {
        self.total += 1;
        if !ok {
            self.failed += 1;
        }
        println!("{} {name}: {detail}", if ok { "PASS" } else { "FAIL" });
    }
}

fn io(e: std::io::Error) -> Error {
    Error::Invalid(e.to_string())
}

fn touch(p: &Path, n: usize) -> Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(io)?;
    }
    std::fs::write(p, vec![b'x'; n]).map_err(io)
}

fn names_for(ix: &Index, q: &str) -> Vec<String> {
    let query = match Query::parse(q, &QueryCtx::local_now()) {
        Ok(x) => x,
        Err(e) => return vec![format!("오류: {e}")],
    };
    let mut ids = query::run(ix, &query);
    query::sort(ix, &mut ids, SortKey::Name, false);
    let mut v: Vec<String> = ids.iter().map(|&i| filetool_core::nfc::to_nfc(ix.name(i))).collect();
    v.sort();
    v
}

fn want(v: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = v.iter().map(|s| s.to_string()).collect();
    v.sort();
    v
}

/// 생성한 트리(NFD 이름·한글 이름)를 실제 디스크에서 스캔 → 질의 결과가 정확히 맞는지, 저장→읽기, 실시간 갱신
pub fn selftest(dir: &Path) -> Result<ExitCode> {
    std::fs::create_dir_all(dir).map_err(io)?;
    println!("--- 2a ---");
    let mut c = Checker { failed: 0, total: 0 };
    let root = dir.join("tree");
    let idx_dir = dir.join("index");
    // 트리
    touch(&root.join("문서").join(nfd("보고서.hwp")), 100)?; // 맥에서 온 NFD 이름
    touch(&root.join("문서").join("주간 보고서_2024.docx"), 2000)?;
    touch(&root.join("문서").join(nfd("회의록")).join("10월 회의록.txt"), 10)?; // NFD 폴더 안
    touch(&root.join("Photos").join("IMG_0001.JPG"), 3 * 1024 * 1024)?;
    touch(&root.join("Photos").join("IMG_0002.png"), 50)?;
    touch(&root.join("Photos").join("holiday photo.jpg"), 2 * 1024 * 1024)?;
    touch(&root.join("Projects").join("app").join("src").join("main.rs"), 10)?;
    touch(&root.join("Projects").join("app").join("node_modules").join("x").join("index.js"), 10)?;
    touch(&root.join(nfd("맥 자료")).join(nfd("사진 모음.zip")), 10)?;

    let opts = ScanOpts { excludes: Excludes::new(&["node_modules".to_string()]), skip: vec![idx_dir.clone()], threads: 0 };
    let ix = RwLock::new(Index::new());
    let st = scan::scan_root_into(&DirWalkSource, &root, &ix, &opts, &ScanControl::default());
    let ix = ix.into_inner().unwrap();
    c.check("스캔", st.files == 8 && st.dirs == 7 && st.excluded == 1 && st.denied == 0,
        format!("폴더 {} / 파일 {} / 제외 {} / 권한 없음 {} / 연결 지점 {}", st.dirs, st.files, st.excluded, st.denied, st.reparse));
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("보고서", vec!["보고서.hwp", "주간 보고서_2024.docx"]),
        ("ㅂㄱㅅ", vec!["보고서.hwp", "주간 보고서_2024.docx"]),
        ("ㅎㅇㄹ", vec!["10월 회의록.txt", "회의록"]),
        ("회의록", vec!["10월 회의록.txt", "회의록"]),
        ("사진", vec!["사진 모음.zip"]),
        ("ext:jpg;png", vec!["IMG_0001.JPG", "IMG_0002.png", "holiday photo.jpg"]),
        ("*.jpg", vec!["IMG_0001.JPG", "holiday photo.jpg"]),
        ("img_000?.*", vec!["IMG_0001.JPG", "IMG_0002.png"]),
        ("\"holiday photo\"", vec!["holiday photo.jpg"]),
        ("img !png", vec!["IMG_0001.JPG"]),
        ("보고서|holiday", vec!["보고서.hwp", "주간 보고서_2024.docx", "holiday photo.jpg"]),
        ("size:>1mb", vec!["IMG_0001.JPG", "holiday photo.jpg"]),
        ("size:1kb..3kb", vec!["주간 보고서_2024.docx"]),
        ("folder:회의", vec!["회의록"]),
        ("file: 회의", vec!["10월 회의록.txt"]),
        (r"app\src\", vec!["main.rs"]),
        (r"문서\회의록\", vec!["10월 회의록.txt"]),
        ("dm:today file: ext:rs", vec!["main.rs"]),
        ("index.js", vec![]),
    ];
    let nfd_q = nfd("보고서");
    let mut all_cases = cases.clone();
    all_cases.push((nfd_q.as_str(), vec!["보고서.hwp", "주간 보고서_2024.docx"]));
    let mut bad = Vec::new();
    for (q, w) in &all_cases {
        let got = names_for(&ix, q);
        if got != want(w) {
            bad.push(format!("{q:?} → {got:?} (기대 {w:?})"));
        }
    }
    c.check("질의 결과 정확히", bad.is_empty(), if bad.is_empty() { format!("{}개 질의 모두 기대값", all_cases.len()) } else { bad.join(" / ") });

    // 저장 → 읽기
    let file = idx_dir.join(INDEX_FILE);
    let t = Instant::now();
    let size = ix.save(&file)?;
    let save_ms = t.elapsed().as_millis();
    let t = Instant::now();
    let back = Index::load(&file);
    let load_ms = t.elapsed().as_millis();
    match back {
        Ok(b) => {
            let same = all_cases.iter().all(|(q, _)| names_for(&b, q) == names_for(&ix, q));
            c.check("저장 → 읽기", same && b.live_count() == ix.live_count(),
                format!("{} B, 저장 {save_ms} ms, 읽기 {load_ms} ms, 항목 {} = {}", size, b.live_count(), ix.live_count()));
        }
        Err(e) => c.check("저장 → 읽기", false, e.to_string()),
    }

    // 실시간 갱신(앱과 같은 서비스)
    let svc = SearchService::start(SearchConfig::new(vec![root.clone()], vec!["node_modules".into()], &idx_dir));
    let t0 = Instant::now();
    while (svc.status().roots.iter().any(|r| r.scanning) || !svc.status().ready) && t0.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let st = svc.status();
    c.check("서비스 시작(저장한 색인 불러오기 + 다시 스캔)", st.ready && st.loaded_ms.is_some() && st.entries == ix.live_count(),
        format!("불러오기 {:?} ms, 항목 {}", st.loaded_ms, st.entries));
    let wait = |q: &str, n: usize| -> (bool, f64) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(10) {
            if svc.query(q, SortKey::Name, false).map(|(v, _)| v.len() == n).unwrap_or(false) {
                return (true, t.elapsed().as_secs_f64());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        (false, t.elapsed().as_secs_f64())
    };
    let f = root.join("문서").join("실시간 시험.txt");
    touch(&f, 1)?;
    let (ok, s) = wait("실시간 시험", 1);
    c.check("실시간: 새 파일", ok, format!("{s:.2} 초"));
    let g = root.join("문서").join(nfd("이름 바꾼 파일.txt"));
    std::fs::rename(&f, &g).map_err(io)?;
    let (ok1, s1) = wait("이름 바꾼", 1);
    let (ok2, _) = wait("실시간 시험", 0);
    c.check("실시간: 이름 바꾸기(NFD 새 이름)", ok1 && ok2, format!("{s1:.2} 초"));
    std::fs::remove_file(&g).map_err(io)?;
    let (ok, s) = wait("이름 바꾼", 0);
    c.check("실시간: 삭제", ok, format!("{s:.2} 초"));
    touch(&root.join("새 폴더").join("안쪽").join("깊은 파일.txt"), 1)?;
    let (ok, s) = wait(r#""새 폴더\안쪽\깊은""#, 1);
    c.check("실시간: 새 폴더 트리", ok, format!("{s:.2} 초"));
    std::fs::rename(root.join("새 폴더"), root.join("옮긴 폴더")).map_err(io)?;
    let (ok, s) = wait(r#""옮긴 폴더\안쪽\깊은""#, 1);
    let (ok2, _) = wait(r#""새 폴더\""#, 0);
    c.check("실시간: 폴더 이름 바꾸기", ok && ok2, format!("{s:.2} 초"));
    svc.stop();
    println!("결과: {}/{} 통과", c.total - c.failed, c.total);
    Ok(if c.failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) })
}
