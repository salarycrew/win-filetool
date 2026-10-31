//! 2a 합성 벤치마크: 항목 1,000,000개(현실적인 깊이, 한글·영어 이름) → 만드는 시간, 메모리, 색인 파일 크기,
//! 저장·읽기 시간, 질의 지연 p50/p95.
//!
//! `cargo test --release -p filetool-core --test search_bench -- --ignored --nocapture`

use std::path::Path;
use std::time::Instant;

use filetool_core::search::index::Index;
use filetool_core::search::query::{self, Query, QueryCtx, SortKey};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a>(&mut self, v: &[&'a str]) -> &'a str {
        v[(self.next() % v.len() as u64) as usize]
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const EN: &[&str] = &[
    "report", "budget", "invoice", "photo", "img", "scan", "draft", "final", "meeting", "notes", "project", "design", "backup",
    "data", "summary", "plan", "contract", "resume", "presentation", "video", "music", "setup", "readme", "config", "log",
    "test", "build", "release", "archive", "screenshot", "letter", "form", "manual", "guide", "chart", "table", "list",
];
const KO: &[&str] = &[
    "보고서", "회의록", "견적서", "계약서", "사진", "자료", "기획안", "발표", "예산", "정산", "영수증", "주간", "월간", "최종",
    "수정", "초안", "업무", "일정", "명단", "강의", "과제", "논문", "설계", "매뉴얼", "결과", "요약", "신청서", "증명서",
];
const EXT: &[&str] = &[
    "pdf", "docx", "xlsx", "pptx", "hwp", "jpg", "png", "heic", "mp4", "txt", "zip", "dll", "exe", "json", "js", "md", "csv", "mov",
];
const DIRS_EN: &[&str] = &[
    "Documents", "Pictures", "Downloads", "Desktop", "Projects", "src", "assets", "node_modules", "lib", "bin", "cache", "2023",
    "2024", "2025", "Work", "Personal", "Archive", "Backup", "Music", "Videos",
];
const DIRS_KO: &[&str] = &["업무", "개인", "사진", "자료", "프로젝트", "보관", "공유", "학교", "회사", "가족"];

fn file_name(r: &mut Rng) -> String {
    let n = r.below(10);
    let ext = r.pick(EXT);
    match n {
        0..=3 => format!("{}_{}.{}", r.pick(EN), r.below(100_000), ext),
        4..=6 => format!("{} {}_{}.{}", r.pick(KO), r.pick(KO), 2015 + r.below(12), ext),
        7 => format!("IMG_{:04}.{}", r.below(10_000), if r.below(2) == 0 { "JPG" } else { "HEIC" }),
        8 => format!("{}-{}-{}.{}", r.pick(EN), r.pick(EN), r.below(1000), ext),
        _ => format!("{}{}.{}", r.pick(KO), r.below(1000), ext),
    }
}

fn dir_name(r: &mut Rng) -> String {
    match r.below(4) {
        0 => r.pick(DIRS_KO).to_string(),
        1 => format!("{} {}", r.pick(KO), r.below(100)),
        2 => format!("{}_{}", r.pick(EN), r.below(1000)),
        _ => r.pick(DIRS_EN).to_string(),
    }
}

/// 항목 `total`개: 폴더 약 8%, 깊이 최대 10(파일 기준), 폴더당 파일 수는 고르지 않게
pub fn generate(total: usize) -> Index {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    let mut ix = Index::new();
    let root = ix.add_root(Path::new(r"C:\"));
    let mut dirs: Vec<(u32, u32)> = vec![(root, 0)]; // (번호, 깊이)
    let mut n = 0usize;
    let now = 1_793_000_000i64;
    while n < total {
        // 부모 고르기: 최근 폴더 쪽으로 치우침(깊이 생김)
        let k = dirs.len() as u64;
        let pi = if r.below(4) != 0 { r.below(k) } else { k - 1 - r.below(k.min(20)) };
        let (parent, depth) = dirs[pi as usize];
        if depth < 9 && r.below(100) < 8 {
            let name = dir_name(&mut r);
            let id = ix.push_child(parent, &name, true, 0, now - r.below(3 * 365 * 86400) as i64);
            dirs.push((id, depth + 1));
        } else {
            let name = file_name(&mut r);
            let size = match r.below(10) {
                0 => r.below(1 << 30),
                1..=3 => r.below(10 << 20),
                _ => r.below(200 << 10),
            };
            ix.push_child(parent, &name, false, size, now - r.below(3 * 365 * 86400) as i64);
        }
        n += 1;
    }
    ix
}

fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).map(|l| l.split_whitespace().nth(1).unwrap_or("0").parse().unwrap_or(0)))
        .unwrap_or(0)
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((v.len() as f64 - 1.0) * p).round() as usize;
    v[i]
}

#[test]
#[ignore]
fn bench_1m() {
    let rss0 = rss_kb();
    let t0 = Instant::now();
    let ix = generate(1_000_000);
    let build = t0.elapsed();
    let rss1 = rss_kb();
    let depth_max = (0..ix.len() as u32).map(|id| {
        let mut d = 0;
        let mut c = id;
        while ix.parent(c) != u32::MAX {
            c = ix.parent(c);
            d += 1;
        }
        d
    });
    let (mut dsum, mut dmax) = (0u64, 0u64);
    for d in depth_max {
        dsum += d;
        dmax = dmax.max(d);
    }
    println!("항목 {} (폴더 {}), 평균 깊이 {:.1}, 최대 깊이 {}", ix.live_count(), (0..ix.len() as u32).filter(|&i| ix.is_dir(i)).count() - 1, dsum as f64 / ix.len() as f64, dmax);
    println!("만들기 {:.0} ms", build.as_secs_f64() * 1000.0);
    println!("메모리: 색인 배열 {:.1} MB, RSS 증가 {:.1} MB", ix.heap_bytes() as f64 / 1e6, (rss1 - rss0) as f64 / 1024.0);

    let t = Instant::now();
    ix.build_name_rank();
    println!("이름 순위 만들기(배경) {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);

    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("index-v1.bin");
    let t = Instant::now();
    let bytes = ix.save(&f).unwrap();
    println!("저장 {:.0} ms, 파일 {:.1} MB", t.elapsed().as_secs_f64() * 1000.0, bytes as f64 / 1e6);
    let t = Instant::now();
    let back = Index::load(&f).unwrap();
    println!("읽기(다시 시작) {:.0} ms, 항목 {}", t.elapsed().as_secs_f64() * 1000.0, back.live_count());
    assert_eq!(back.live_count(), ix.live_count());
    drop(back);

    let ctx = QueryCtx::local_now();
    let queries = [
        ("낱말 하나", "report"),
        ("낱말 하나(한글)", "회의록"),
        ("두 낱말 AND", "보고서 2024"),
        ("OR", "invoice|영수증"),
        ("ext:", "ext:jpg"),
        ("와일드카드", "img_1*"),
        ("와일드카드 확장자", "*.pdf"),
        ("경로 낱말", r"documents\project"),
        ("초성", "ㅂㄱㅅ"),
        ("크기", "size:>500mb"),
        ("조합", "ext:pdf;docx 계약 !초안"),
        ("NOT 많은 결과", "!a"),
    ];
    println!("| 질의 | 결과 수 | 찾기 p50 | 찾기 p95 | +이름 정렬 p50 | +이름 정렬 p95 |");
    println!("|---|---:|---:|---:|---:|---:|");
    for (label, q) in queries {
        let query = Query::parse(q, &ctx).unwrap();
        let mut m = Vec::new();
        let mut ms = Vec::new();
        let mut count = 0;
        // 처음 한 번은 캐시(폴더 경로) 만들기 포함해서 따로
        let t = Instant::now();
        let _ = query::run(&ix, &query);
        let first = t.elapsed().as_secs_f64() * 1000.0;
        for _ in 0..30 {
            let t = Instant::now();
            let mut ids = query::run(&ix, &query);
            m.push(t.elapsed().as_secs_f64() * 1000.0);
            query::sort(&ix, &mut ids, SortKey::Name, false);
            ms.push(t.elapsed().as_secs_f64() * 1000.0);
            count = ids.len();
        }
        println!(
            "| {label} `{q}` | {count} | {:.1} ms | {:.1} ms | {:.1} ms | {:.1} ms |{}",
            pct(&mut m, 0.5),
            pct(&mut m, 0.95),
            pct(&mut ms, 0.5),
            pct(&mut ms, 0.95),
            if query.uses_path { format!(" (첫 실행 {first:.0} ms: 폴더 경로 캐시)") } else { String::new() }
        );
    }
}
