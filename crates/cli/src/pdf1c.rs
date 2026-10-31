//! 1c 명령: pdf-compress / img2pdf / pdf2img / pdf-images / autostart

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use filetool_core::img2pdf::{self, Img2PdfOpts, PageSize};
use filetool_core::pdfcompress::{self, CompressPreset};
use filetool_core::pdfrender::{self, PageImageFormat, RenderOpts};
use filetool_core::{autostart, Error, Result};

use crate::conv::{toolset, Flags};

/// 앱 exe: `--exe` 또는 이 CLI 옆의 win-filetool.exe
pub fn app_exe(f: &Flags) -> PathBuf {
    if let Some(e) = f.get("--exe") {
        return PathBuf::from(e);
    }
    let me = std::env::current_exe().unwrap_or_default();
    me.with_file_name(if cfg!(windows) { "win-filetool.exe" } else { "win-filetool" })
}

pub fn run(cmd: &str, args: &[String]) -> Result<Option<ExitCode>> {
    let f = Flags::parse(&args[1.min(args.len())..]);
    let out = f.get("--out").map(PathBuf::from);
    match cmd {
        "pdf-compress" => {
            let preset = CompressPreset::parse(f.rest.first().map(String::as_str).unwrap_or(""))
                .ok_or_else(|| Error::Invalid("프리셋은 screen|normal|print (화면용|보통|인쇄용)".into()))?;
            let mut failed = 0;
            for p in &f.rest[1..] {
                match pdfcompress::compress(Path::new(p), out.as_deref(), preset) {
                    Ok(r) => println!(
                        "{}  {} -> {}  {}",
                        if r.output.is_some() { "완료" } else { "그대로" },
                        p,
                        r.output.as_ref().map(|o| o.display().to_string()).unwrap_or_else(|| "(새 파일 없음)".into()),
                        r.note
                    ),
                    Err(e) => {
                        failed += 1;
                        println!("실패  {p}: {e}");
                    }
                }
            }
            Ok(Some(if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }))
        }
        "img2pdf" => {
            let page = match f.get("--page").unwrap_or("image") {
                "image" => PageSize::Image,
                "a4" | "a4-portrait" => PageSize::A4Portrait,
                "a4-landscape" => PageSize::A4Landscape,
                o => return Err(Error::Invalid(format!("--page는 image|a4|a4-landscape ({o})"))),
            };
            let inputs: Vec<PathBuf> = f.rest.iter().map(PathBuf::from).collect();
            let output = f.get("--output").map(PathBuf::from).unwrap_or_else(|| img2pdf::default_output(&inputs, out.as_deref()));
            let ts = toolset(&f);
            let r = img2pdf::images_to_pdf(ts.ffmpeg.as_deref(), &inputs, &output, &Img2PdfOpts { page, margin_mm: f.num("--margin", 0.0)? }, None, &mut |_| {})?;
            for (i, p) in r.pages.iter().enumerate() {
                println!(
                    "{}쪽  {}  {}×{}px → 쪽 {:.1}×{:.1}pt{}{}",
                    i + 1,
                    p.source.display(),
                    p.px_w,
                    p.px_h,
                    p.page_w,
                    p.page_h,
                    if p.jpeg_passthrough { ", JPEG 그대로" } else { "" },
                    if p.orientation != 1 { format!(", EXIF 회전 {}", p.orientation) } else { String::new() }
                );
            }
            println!("완료: {} -> {}", r.note, r.output.display());
            Ok(Some(ExitCode::SUCCESS))
        }
        "pdf2img" => {
            let format = match f.rest.first().map(String::as_str) {
                Some("png") => PageImageFormat::Png,
                Some("jpg") | Some("jpeg") => PageImageFormat::Jpg,
                _ => return Err(Error::Invalid("형식은 png|jpg".into())),
            };
            let opts = RenderOpts { format, dpi: f.num("--dpi", 150)?, quality: f.num("--quality", 90)?, pages: f.get("--pages").map(str::to_string) };
            let mut failed = 0;
            for p in &f.rest[1..] {
                match pdfrender::render_pages(Path::new(p), out.as_deref(), &opts, None, &mut |_, _| {}) {
                    Ok(r) => println!("완료  {} -> {}  {}", p, r.folder.display(), r.note),
                    Err(e) => {
                        failed += 1;
                        println!("실패  {p}: {e}");
                    }
                }
            }
            Ok(Some(if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }))
        }
        "pdf-images" => {
            let mut failed = 0;
            for p in &f.rest {
                match pdfrender::extract_images(Path::new(p), out.as_deref()) {
                    Ok(r) => println!("완료  {} -> {}  {}", p, r.folder.map(|d| d.display().to_string()).unwrap_or_default(), r.note),
                    Err(e) => {
                        failed += 1;
                        println!("실패  {p}: {e}");
                    }
                }
            }
            Ok(Some(if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }))
        }
        "autostart" => {
            let exe = app_exe(&f);
            let st = match f.rest.first().map(String::as_str) {
                Some("on") => {
                    if !exe.is_file() {
                        return Err(Error::Invalid(format!("앱 exe가 없습니다: {} (--exe로 지정)", exe.display())));
                    }
                    autostart::set(true, &exe)?
                }
                Some("off") => autostart::set(false, &exe)?,
                Some("status") | None => autostart::status(&exe),
                Some(o) => return Err(Error::Invalid(format!("autostart on|off|status ({o})"))),
            };
            println!(
                "윈도 시작 시 실행: {}  값 {}  (HKCU\\{}\\{}){}",
                if st.enabled { "켜짐" } else { "꺼짐" },
                st.value.as_deref().unwrap_or("(없음)"),
                autostart::RUN_KEY,
                autostart::VALUE_NAME,
                if st.enabled && !st.matches_exe { format!("  ※ {}와 경로가 다름", exe.display()) } else { String::new() }
            );
            Ok(Some(ExitCode::SUCCESS))
        }
        _ => Ok(None),
    }
}
