//! 2c 명령: svc-status, search --via service, svc-watch (빠른 검색 서비스에 파이프로 묻기)

use std::process::ExitCode;
use std::time::{Duration, Instant};

use filetool_core::search::query::{fmt_time, SortKey};
use filetool_core::search::svcpipe;
use filetool_core::search::svcproto::describe_status;
use filetool_core::{Error, Result};

fn client() -> Result<svcpipe::SvcClient> {
    svcpipe::connect(&format!("filetool-cli {}", env!("CARGO_PKG_VERSION"))).map_err(Error::Invalid)
}

/// `svc-status`: 설치·상태(SCM) + 파이프로 받은 색인 상태. 종료 코드 0 = 답함, 3 = 설치 안 됨, 4 = 답하지 않음
pub fn status() -> Result<ExitCode> {
    let info = svcpipe::service_info();
    println!("서비스: {}", if info.installed { "설치됨" } else { "설치 안 됨" });
    if info.installed {
        println!("  상태 {} · pid {} · 시작 {} · 계정 {}", info.state, info.pid, info.start_type, info.account);
        println!("  실행 파일 {}", info.binary_path);
    }
    let t0 = Instant::now();
    match client().and_then(|mut c| {
        let me = c.me.clone();
        c.status().map(|s| (me, s)).map_err(Error::Invalid)
    }) {
        Ok((me, st)) => {
            println!("  파이프 연결 + 상태 {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
            println!("  나: {} ({}) 관리자 {} · {}", me.user, me.sid, me.admin, me.visibility);
            for l in describe_status(&st) {
                println!("  {l}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => {
            println!("  파이프: {e}");
            Ok(ExitCode::from(if info.installed { 4 } else { 3 }))
        }
    }
}

/// `search <질의> --via service [--limit N] [--sort …] [--desc] [--paths] [--repeat N] [--exclude 패턴]…`
pub fn search(q: &str, limit: usize, sort: SortKey, desc: bool, paths: bool, repeat: usize, excludes: &[String]) -> Result<ExitCode> {
    let t0 = Instant::now();
    let mut c = client()?;
    let connect_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let mut rtts = Vec::new();
    let mut last = None;
    for _ in 0..repeat.max(1) {
        let t = Instant::now();
        let r = c.query(q, sort, desc, limit, excludes).map_err(Error::Invalid)?;
        rtts.push(t.elapsed().as_secs_f64() * 1000.0);
        last = Some(r);
    }
    let r = last.unwrap();
    for row in &r.rows {
        if paths {
            println!("{}", row.path);
        } else {
            let size = if row.is_dir { "<폴더>".to_string() } else { row.size.to_string() };
            println!("{:>12}  {}  {}", size, fmt_time(row.mtime), row.path);
        }
    }
    rtts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = rtts[rtts.len() / 2];
    println!(
        "질의 {q:?} (서비스, {} · {}): {}개 / 서비스 안 {:.1} ms / 왕복 {:.1} ms{} / 연결 {connect_ms:.1} ms",
        c.me.user,
        if c.me.admin { "관리자" } else { "일반 사용자" },
        r.total,
        r.millis,
        rtts[0],
        if rtts.len() > 1 { format!(" (최소 {:.1} · 중간 {med:.1} · 최대 {:.1}, {}회)", rtts[0], rtts[rtts.len() - 1], rtts.len()) } else { String::new() },
    );
    Ok(ExitCode::SUCCESS)
}

/// `svc-watch <질의> [--seconds 30] [--interval-ms 100]`: 서비스 결과 수가 바뀔 때마다 시각과 함께 출력(실시간 반영 확인용)
pub fn watch(q: &str, seconds: u64, interval_ms: u64) -> Result<ExitCode> {
    let mut c = client()?;
    let t0 = Instant::now();
    let mut prev: Option<(usize, Vec<String>)> = None;
    while t0.elapsed() < Duration::from_secs(seconds) {
        let r = c.query(q, SortKey::Path, false, 50, &[]).map_err(Error::Invalid)?;
        let paths: Vec<String> = r.rows.iter().map(|x| x.path.clone()).collect();
        if prev.as_ref().map(|(n, p)| *n != r.total || *p != paths).unwrap_or(true) {
            println!(
                "{} {:>8.3} s  {}개  (서비스 안 {:.1} ms, 버전 {})",
                chrono::Local::now().format("%H:%M:%S%.3f"),
                t0.elapsed().as_secs_f64(),
                r.total,
                r.millis,
                r.version
            );
            for p in &paths {
                println!("            {p}");
            }
            prev = Some((r.total, paths));
        }
        std::thread::sleep(Duration::from_millis(interval_ms));
    }
    Ok(ExitCode::SUCCESS)
}
