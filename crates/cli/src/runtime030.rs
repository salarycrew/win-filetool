//! 0.3.0 W1 점검 명령: 앱에 든 도구 묶음, 앱 정보·고지, WebView2.

use std::process::ExitCode;
use std::time::Instant;

use filetool_core::{bundle, notices, runtime, Result};

pub const USAGE: &str = "  filetool-cli bundle-status                       앱에 든 도구 묶음(찾은 파일, 버전 폴더, 풀린 도구)
  filetool-cli bundle-unpack [ffmpeg|pdfium|lo|notices ...]   도구를 (필요하면) 풀고 걸린 시간 출력(없으면 모두)
  filetool-cli app-info [--notices]                앱 버전·고지 파일 위치(·전문)·WebView2 버전
  filetool-cli webview2                            WebView2 런타임 버전(없으면 종료 코드 3)";

pub fn run(cmd: &str, args: &[String]) -> Result<Option<ExitCode>> {
    match cmd {
        "bundle-status" => {
            let s = bundle::status();
            println!("{s:#?}");
            Ok(Some(if s.present { ExitCode::SUCCESS } else { ExitCode::from(3) }))
        }
        "bundle-unpack" => {
            let Some(b) = bundle::current() else {
                println!("도구 묶음 없음 (후보: {:?})", bundle::candidates());
                return Ok(Some(ExitCode::from(3)));
            };
            println!("묶음 {} → {}", b.file.display(), b.version_dir().display());
            let names: Vec<String> = if args.len() > 1 { args[1..].to_vec() } else { b.index.tools.iter().map(|e| e.name.clone()).collect() };
            let mut failed = 0;
            for n in names {
                let was = b.is_ready(&n);
                let t0 = Instant::now();
                match b.ensure(&n) {
                    Ok(d) => {
                        // 이 프로그램이 직접 풀었는지(다른 프로그램이 푸는 동안 기다렸으면 "기다림")
                        let mine = bundle::unpack_log().iter().any(|u| u.tool == n);
                        let how = if was { "이미 있음" } else if mine { "풀었음" } else { "기다림(다른 프로그램이 풂)" };
                        println!("{how}  {n}  {:.2}초  {}", t0.elapsed().as_secs_f64(), d.display())
                    }
                    Err(e) => {
                        failed += 1;
                        println!("실패  {n}: {e}");
                    }
                }
            }
            Ok(Some(if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }))
        }
        "app-info" => {
            let i = notices::app_info();
            println!("{} {}", i.name, i.version);
            println!("고지: {} ({} 글자)", i.notices_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "(짧은 안내문)".into()), i.notices.chars().count());
            println!("WebView2: {}", runtime::webview2_version().unwrap_or_else(|| "없음".into()));
            if args.iter().any(|a| a == "--notices") {
                println!("{}", i.notices);
            }
            Ok(Some(ExitCode::SUCCESS))
        }
        "webview2" => match runtime::webview2_version() {
            Some(v) => {
                println!("WebView2 {v}");
                Ok(Some(ExitCode::SUCCESS))
            }
            None => {
                println!("WebView2 없음\n{}", runtime::WEBVIEW2_MISSING_TEXT);
                Ok(Some(ExitCode::from(3)))
            }
        },
        _ => Ok(None),
    }
}
