//! 0.3.0 W6 한글 명령: hwp-info / hwp-convert / hwp-images (점검용, 결과를 한 줄씩)
//! 한글이 없는 것처럼 시험하려면 `WIN_FILETOOL_HIDE_HANCOM=1`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use filetool_core::docs::{self, DocTarget};
use filetool_core::hwp::{self, hancom, Route};
use filetool_core::tools::Tool;
use filetool_core::{Error, Result};

use crate::conv::{toolset, Flags};

pub const USAGE: &str = "  filetool-cli hwp-info <hwp|hwpx...>                                  한글 형식·할 수 있는 것(JSON 한 줄)
  filetool-cli hwp-convert <pdf|docx> [--out 폴더] <hwp|hwpx...>       한글 → PDF/DOCX(설치된 한글, 한글 97은 LibreOffice)
  filetool-cli hwp-images [--out 폴더] <hwp|hwpx...>                   한글 안 그림 원본 꺼내기(건너뛴 이유 표시)
  (WIN_FILETOOL_HIDE_HANCOM=1 이면 한글이 없는 것처럼 동작)";

pub fn run(cmd: &str, args: &[String]) -> Result<Option<ExitCode>> {
    if !matches!(cmd, "hwp-info" | "hwp-convert" | "hwp-images") {
        return Ok(None);
    }
    let f = Flags::parse(&args[1.min(args.len())..]);
    let out = f.get("--out").map(PathBuf::from);
    let out = out.as_deref();
    let ts = toolset(&f);
    let soffice = ts.get(Tool::Soffice).ok().map(Path::to_path_buf);
    let mut files: &[String] = &f.rest;
    let target = if cmd == "hwp-convert" {
        let t = match files.first().map(String::as_str) {
            Some("pdf") => DocTarget::Pdf,
            Some("docx") => DocTarget::Docx,
            o => return Err(Error::Invalid(format!("hwp-convert <pdf|docx> 파일… ({o:?})"))),
        };
        files = &files[1..];
        Some(t)
    } else {
        None
    };
    if files.is_empty() {
        println!("파일을 지정하세요\n{USAGE}");
        return Ok(Some(ExitCode::from(2)));
    }
    println!("한글 설치: {}", match hancom::installed() {
        Some(i) if hancom::available() => format!("있음 ({}, {}비트)", i.exe.display(), if i.wow64 { 32 } else { 64 }),
        Some(_) => "있음(숨김 설정으로 없는 것처럼)".into(),
        None => "없음".into(),
    });
    let mut failed = 0;
    for p in files {
        let p = Path::new(p);
        let r: Result<String> = (|| match cmd {
            "hwp-info" => Ok(hwp::capabilities(p, soffice.is_some())?.to_json()),
            "hwp-convert" => {
                let t = target.unwrap_or(DocTarget::Pdf);
                let caps = hwp::capabilities(p, soffice.is_some())?;
                let t0 = Instant::now();
                match caps.route {
                    Route::Hancom => {
                        // 0.3.1: 배포용은 PDF로 인쇄(앱과 같은 길)
                        let mode = if caps.distribution { hancom::Mode::PrintPdf } else { hancom::Mode::Save };
                        let r = hancom::convert_mode(p, out, t, mode, None, hancom::TIMEOUT)?;
                        Ok(format!("{} -> {}  경로 hancom{} · 한글 쪽수 {} · {} ms", p.display(), r.output.display(), if caps.distribution { "(PDF로 인쇄)" } else { "" }, r.pages.map(|n| n.to_string()).unwrap_or("?".into()), r.millis))
                    }
                    Route::Libreoffice => {
                        let o = docs::convert_lo(soffice.as_deref().expect("route"), p, out, t, None, docs::DOC_TIMEOUT)?;
                        Ok(format!("{} -> {}  경로 libreoffice · {} ms", p.display(), o.display(), t0.elapsed().as_millis()))
                    }
                    Route::None => Err(Error::Invalid(caps.message.unwrap_or_default())),
                }
            }
            _ => {
                let r = filetool_core::officeimages::extract_images(p, out)?;
                let mut s = format!(
                    "{} -> {}  그림 {} · 같은 그림 건너뜀 {} · 그림 아님/못 읽음 {} · {} ms",
                    p.display(),
                    r.folder.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "(그림 없음)".into()),
                    r.files.len(),
                    r.duplicates,
                    r.skipped,
                    r.millis
                );
                for k in &r.skipped_items {
                    s.push_str(&format!("\n      건너뜀 {k}"));
                }
                Ok(s)
            }
        })();
        match r {
            Ok(line) => println!("완료  {line}"),
            Err(e) => {
                failed += 1;
                println!("실패  {}: {e}", p.display());
            }
        }
    }
    Ok(Some(if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }))
}
