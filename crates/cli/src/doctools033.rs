//! 0.3.3 명령: 문서를 어느 길로 바꾸는지, Office(COM)로 바꾸기, 문서 도구 받기·넣기·지우기(점검용).
//! CLI는 배포하지 않으므로 시험용 바꾸기(`WIN_FILETOOL_HIDE_COM=1`, `WIN_FILETOOL_DOCTOOLS_URL`)를 읽는다.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use filetool_core::docs::{self, DocTarget, Engines};
use filetool_core::process::Cancel;
use filetool_core::{doctools, msoffice, Error, Result};

use crate::conv::Flags;

pub const USAGE: &str = "  filetool-cli engines                                   한글·Word·Excel·PowerPoint(COM)·문서 도구 상태
  filetool-cli engine <pdf|docx|xlsx|pptx> <파일...>     파일마다 고른 길(hancom|office:Word|libreoffice|none)
  filetool-cli office-convert <pdf|docx|xlsx|pptx> [--out 폴더] <파일...>   설치된 Office로 바꾸기
  filetool-cli doctools <status|download|install-file <파일>|verify <파일>|remove>   문서 도구(LibreOffice) 받기·확인
  filetool-cli pdf-blank <pdf...>                        쪽 수와 빈 쪽(그려서 확인)
  filetool-cli doctools-serve <묶음 파일> [--port 8765] [--cut-after 바이트] [--no-range] [--seconds 600]   시험용 가짜 HTTP 서버(127.0.0.1만)
  (WIN_FILETOOL_HIDE_COM=1: 한글·Office가 없는 것처럼, WIN_FILETOOL_DOCTOOLS_URL: 받을 주소 바꾸기 — CLI·시험용 앱만)";

fn target(s: Option<&String>) -> Result<DocTarget> {
    Ok(match s.map(String::as_str) {
        Some("pdf") => DocTarget::Pdf,
        Some("docx") => DocTarget::Docx,
        Some("xlsx") => DocTarget::Xlsx,
        Some("pptx") => DocTarget::Pptx,
        o => return Err(Error::Invalid(format!("대상은 pdf|docx|xlsx|pptx ({o:?})"))),
    })
}

pub fn engine_label(e: docs::Engine) -> String {
    match e {
        docs::Engine::Hancom => "hancom".into(),
        docs::Engine::Office(a) => format!("office:{}", a.name()),
        docs::Engine::LibreOffice => "libreoffice".into(),
        docs::Engine::None => "none".into(),
    }
}

pub fn run(cmd: &str, args: &[String]) -> Result<Option<ExitCode>> {
    if !matches!(cmd, "engines" | "engine" | "office-convert" | "doctools" | "doctools-serve" | "pdf-blank" | "hwp-script-make" | "office-extref-make") {
        return Ok(None);
    }
    let f = Flags::parse(&args[1.min(args.len())..]);
    if cmd == "office-extref-make" {
        // 0.3.3(보안 강화) 시험용: 원격 서식을 가리키는 docx·외부 통합 문서 연결이 든 xlsx를 만든다(직접 만든 시험 자료)
        let Some(dir) = f.rest.first() else { return Err(Error::Invalid("office-extref-make <폴더>".into())) };
        let dir = std::path::Path::new(dir);
        std::fs::create_dir_all(dir).map_err(|e| Error::Invalid(e.to_string()))?;
        use filetool_core::ooxml::extref::samples;
        samples::docx_with_remote_template(&dir.join("remote-template.docx"));
        samples::xlsx_with_external_link(&dir.join("external-link.xlsx"));
        println!("만듦: {}, {}", dir.join("remote-template.docx").display(), dir.join("external-link.xlsx").display());
        return Ok(Some(ExitCode::SUCCESS));
    }
    if cmd == "hwp-script-make" {
        // 시험용: 한글 5 파일의 문서 스크립트(Scripts/DefaultJScript)에 OnDocument_Open을 넣은 사본을 만든다(매크로가 도는지 보기)
        let (src, dst, js) = (f.rest.first(), f.rest.get(1), f.rest.get(2));
        let (Some(src), Some(dst), Some(js)) = (src, dst, js) else { return Err(Error::Invalid("hwp-script-make <원본.hwp> <출력.hwp> <스크립트.js>".into())) };
        std::fs::copy(src, dst).map_err(|e| Error::Invalid(e.to_string()))?;
        let code = std::fs::read_to_string(js).map_err(|e| Error::Invalid(e.to_string()))?;
        let mut raw = Vec::new();
        let u: Vec<u16> = code.encode_utf16().collect();
        raw.extend_from_slice(&0u32.to_le_bytes()); // 머리 스크립트 길이(0)
        raw.extend_from_slice(&(u.len() as u32).to_le_bytes());
        for c in &u {
            raw.extend_from_slice(&c.to_le_bytes());
        }
        raw.extend_from_slice(&0u32.to_le_bytes());
        raw.extend_from_slice(&0u32.to_le_bytes());
        raw.extend_from_slice(&u32::MAX.to_le_bytes());
        let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut z, &raw).map_err(|e| Error::Invalid(e.to_string()))?;
        let data = z.finish().map_err(|e| Error::Invalid(e.to_string()))?;
        let mut c = cfb::open_rw(dst).map_err(|e| Error::Invalid(e.to_string()))?;
        let mut st = c.create_stream("/Scripts/DefaultJScript").map_err(|e| Error::Invalid(e.to_string()))?;
        std::io::Write::write_all(&mut st, &data).map_err(|e| Error::Invalid(e.to_string()))?;
        drop(st);
        c.flush().map_err(|e| Error::Invalid(e.to_string()))?;
        println!("{dst}: 스크립트 {} 글자 넣음", u.len());
        return Ok(Some(ExitCode::SUCCESS));
    }
    if cmd == "pdf-blank" {
        // 쪽 수 + 빈 쪽(흰색이 아닌 화소 0.1% 이하) — Office·LibreOffice 결과 비교용
        for p in &f.rest {
            match crate::core032::blank_pages(Path::new(p)) {
                Ok((n, b)) => println!("{p}\t쪽 {n}\t빈 쪽 {b:?}"),
                Err(e) => println!("{p}\t오류 {e}"),
            }
        }
        return Ok(Some(ExitCode::SUCCESS));
    }
    if cmd == "doctools-serve" {
        let file = PathBuf::from(f.rest.first().ok_or_else(|| Error::Invalid("묶음 파일을 지정하세요".into()))?);
        let port: u16 = f.get("--port").and_then(|v| v.parse().ok()).unwrap_or(8765);
        let cut: Option<u64> = f.get("--cut-after").and_then(|v| v.parse().ok());
        let secs: u64 = f.get("--seconds").and_then(|v| v.parse().ok()).unwrap_or(600);
        serve(&file, port, cut, f.has("--no-range"), secs)?;
        return Ok(Some(ExitCode::SUCCESS));
    }
    match cmd {
        "engines" => {
            let e = Engines::detect();
            let p = msoffice::presence();
            println!("hancom {} · word {} · excel {} · powerpoint {} · office 판 {} · 64비트 {:?}", e.hancom, e.word, e.excel, e.powerpoint, p.version.unwrap_or_default(), p.x64);
            println!("{}", serde_json::to_string(&doctools::status()).unwrap_or_default());
        }
        "engine" => {
            let t = target(f.rest.first())?;
            let e = Engines::detect();
            for p in &f.rest[1..] {
                println!("{}\t{}", engine_label(docs::engine_for(Path::new(p), t, &e)), p);
            }
        }
        "office-convert" => {
            let t = target(f.rest.first())?;
            let out = f.get("--out").map(PathBuf::from);
            let mut failed = 0;
            for p in &f.rest[1..] {
                match msoffice::convert(Path::new(p), out.as_deref(), t, None, msoffice::TIMEOUT) {
                    Ok(o) => println!("{} -> {}  {} · 쪽/장 {} · {} ms", p, o.output.display(), o.app.name(), o.pages.map(|n| n.to_string()).unwrap_or("?".into()), o.millis),
                    Err(e) => {
                        failed += 1;
                        println!("실패 {p}: {e}");
                    }
                }
            }
            return Ok(Some(if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }));
        }
        _ => {
            let sub = f.rest.first().map(String::as_str).unwrap_or("status");
            let t0 = Instant::now();
            match sub {
                "status" => println!("{}", serde_json::to_string_pretty(&doctools::status()).unwrap_or_default()),
                "download" => {
                    let fetch = doctools::WinHttpFetch { user_agent: format!("win-filetool/{}", env!("CARGO_PKG_VERSION")) };
                    let c = Cancel::new();
                    let mut last = 0u64;
                    let r = doctools::fetch_and_install(&fetch, Some(&c), &mut |a, b| {
                        if a >= last + 10_000_000 || a == b {
                            last = a;
                            println!("받는 중 {a}/{b}");
                        }
                    });
                    match r {
                        Ok(s) => println!("완료 {} · {} ms", s.display(), t0.elapsed().as_millis()),
                        Err(e) => {
                            println!("실패: {e} · 받던 파일 {} B", doctools::status().partial_bytes);
                            return Ok(Some(ExitCode::from(1)));
                        }
                    }
                }
                "install-file" => {
                    let p = f.rest.get(1).ok_or_else(|| Error::Invalid("파일을 지정하세요".into()))?;
                    match doctools::install_file(Path::new(p), None) {
                        Ok(s) => println!("넣음 {} · {} ms", s.display(), t0.elapsed().as_millis()),
                        Err(e) => {
                            println!("거절: {e} · 고른 파일 {}", if Path::new(p).exists() { "그대로" } else { "없음" });
                            return Ok(Some(ExitCode::from(1)));
                        }
                    }
                }
                "verify" => {
                    let p = f.rest.get(1).ok_or_else(|| Error::Invalid("파일을 지정하세요".into()))?;
                    let h = doctools::sha256_file(Path::new(p), None)?;
                    println!("{h}  {}", if h == doctools::SHA256 { "맞음(고정값과 같음)" } else { "다름" });
                }
                "remove" => {
                    doctools::remove(&filetool_core::bundle::tools_root())?;
                    println!("지움 · 준비됨 {}", doctools::status().ready);
                }
                o => return Err(Error::Invalid(format!("doctools <status|download|install-file|verify|remove> ({o})"))),
            }
        }
    }
    Ok(Some(ExitCode::SUCCESS))
}

/// 시험용 가짜 서버: GET 하나에 파일을 준다. `Range: bytes=N-`면 206(`--no-range`면 무시하고 200).
/// `--cut-after`: 첫 요청만 그 바이트까지 보내고 끊는다(끊김 → 이어받기 시험). 127.0.0.1에만 열고 `secs`초 뒤 끝난다.
fn serve(file: &Path, port: u16, cut: Option<u64>, no_range: bool, secs: u64) -> Result<()> {
    use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
    let len = std::fs::metadata(file).map_err(|e| Error::Invalid(format!("{}: {e}", file.display())))?.len();
    let l = std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|e| Error::Invalid(format!("포트 {port}: {e}")))?;
    l.set_nonblocking(true).ok();
    println!("serving {} ({len} B) on http://127.0.0.1:{port}/ for {secs}s cut={cut:?} no_range={no_range}", file.display());
    let t0 = Instant::now();
    let mut first = true;
    while t0.elapsed().as_secs() < secs {
        let (mut sock, _) = match l.accept() {
            Ok(x) => x,
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
        };
        sock.set_nonblocking(false).ok();
        let mut rd = BufReader::new(sock.try_clone().map_err(|e| Error::Invalid(e.to_string()))?);
        let mut line = String::new();
        let _ = rd.read_line(&mut line);
        let mut from = 0u64;
        loop {
            let mut h = String::new();
            if rd.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                break;
            }
            let lower = h.to_ascii_lowercase();
            if let Some(v) = lower.strip_prefix("range: bytes=") {
                from = v.trim().trim_end_matches('-').parse().unwrap_or(0);
            }
        }
        let partial = from > 0 && !no_range && from < len;
        let start = if partial { from } else { 0 };
        let head = if partial {
            format!("HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{len}\r\nConnection: close\r\n\r\n", len - start, len - 1)
        } else {
            format!("HTTP/1.1 200 OK\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n")
        };
        let limit = if first { cut } else { None };
        first = false;
        println!("{} {} range_from={from} -> {} send_from={start}{}", t0.elapsed().as_millis(), line.trim(), if partial { 206 } else { 200 }, limit.map(|c| format!(" cut_after={c}")).unwrap_or_default());
        let _ = sock.write_all(head.as_bytes());
        let mut fh = std::fs::File::open(file).map_err(|e| Error::Invalid(e.to_string()))?;
        fh.seek(SeekFrom::Start(start)).ok();
        let mut sent = 0u64;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = fh.read(&mut buf).unwrap_or(0);
            if n == 0 {
                break;
            }
            let n = match limit {
                Some(c) if sent + n as u64 > c => (c - sent) as usize,
                _ => n,
            };
            if sock.write_all(&buf[..n]).is_err() {
                break;
            }
            sent += n as u64;
            if limit.is_some_and(|c| sent >= c) {
                break;
            }
        }
        let _ = sock.flush();
        let _ = sock.shutdown(std::net::Shutdown::Both);
        println!("{} sent {sent} B", t0.elapsed().as_millis());
    }
    Ok(())
}
