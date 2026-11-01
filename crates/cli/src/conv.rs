//! 1b 명령: tools / image / media / doc / watch

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use filetool_core::convert::{self, Op};
use filetool_core::docs::DocTarget;
use filetool_core::imageconv::{ImageOpts, ImgFormat};
use filetool_core::media::MediaPreset;
use filetool_core::settings::Settings;
use filetool_core::tools::{self, SearchEnv, ToolPaths, Toolset};
use filetool_core::watch::{WatchRule, Watcher};
use filetool_core::{Error, Result};

/// 앱(`win-filetool.exe`)과 같은 설정 폴더: %APPDATA%\kr.salarycrew.winfiletool
pub fn app_settings_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("kr.salarycrew.winfiletool"))
}

/// `--이름 값` 옵션과 나머지(위치 인자)를 나눈다.
pub struct Flags {
    pub opts: Vec<(String, Option<String>)>,
    pub rest: Vec<String>,
}

const SWITCHES: [&str; 1] = ["--no-exif"];

impl Flags {
    pub fn parse(args: &[String]) -> Self {
        let mut opts = Vec::new();
        let mut rest = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a.starts_with("--") {
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
    pub fn get(&self, k: &str) -> Option<&str> {
        self.opts.iter().rev().find(|(n, _)| n == k).and_then(|(_, v)| v.as_deref())
    }
    pub fn has(&self, k: &str) -> bool {
        self.opts.iter().any(|(n, _)| n == k)
    }
    pub fn num<T: std::str::FromStr>(&self, k: &str, default: T) -> Result<T> {
        match self.get(k) {
            None => Ok(default),
            Some(v) => v.parse().map_err(|_| Error::Invalid(format!("{k} 값이 숫자가 아닙니다: {v}"))),
        }
    }
}

/// 사용자 경로: 명령줄 → 앱 설정.
pub fn tool_paths(f: &Flags) -> ToolPaths {
    let mut p = app_settings_dir().map(|d| Settings::load(&d).tools).unwrap_or_default();
    if let Some(v) = f.get("--ffmpeg") {
        p.ffmpeg = Some(v.into());
    }
    if let Some(v) = f.get("--soffice") {
        p.soffice = Some(v.into());
    }
    // 명령줄로 준 경로는 앱에 든 도구보다 먼저
    if f.get("--ffmpeg").is_some() || f.get("--soffice").is_some() {
        p.prefer_custom = true;
    }
    p
}

pub fn toolset(f: &Flags) -> Toolset {
    Toolset::discover(&tool_paths(f), &SearchEnv::from_system())
}

pub fn parse_op(kind: &str, sub: &str, f: &Flags) -> Result<Op> {
    Ok(match kind {
        "image" => {
            let format = match sub {
                "jpg" | "jpeg" => ImgFormat::Jpg,
                "png" => ImgFormat::Png,
                "webp" => ImgFormat::Webp,
                "bmp" => ImgFormat::Bmp,
                "tif" | "tiff" => ImgFormat::Tiff,
                "gif" => ImgFormat::Gif,
                _ => return Err(Error::Invalid(format!("이미지 형식을 모릅니다: {sub}"))),
            };
            let max = f.num("--max-side", 0u32)?;
            Op::Image(ImageOpts { format, quality: f.num("--quality", 90u8)?, max_side: (max > 0).then_some(max), keep_exif: !f.has("--no-exif") })
        }
        "media" => Op::Media(match sub {
            "mp4" => MediaPreset::Mp4,
            "messenger" => MediaPreset::Messenger { max_mb: f.num("--max-mb", 25)?, max_height: f.num("--height", 720)? },
            "mp3" => MediaPreset::Mp3,
            "m4a" => MediaPreset::M4a,
            "gif" => MediaPreset::Gif { start: f.num("--start", 0.0)?, duration: f.num("--dur", 5.0)?, width: f.num("--width", 480)?, fps: f.num("--fps", 12)? },
            _ => return Err(Error::Invalid(format!("영상 프리셋을 모릅니다: {sub}"))),
        }),
        "doc" => Op::Doc {
            target: match sub {
                "pdf" => DocTarget::Pdf,
                "docx" => DocTarget::Docx,
                "xlsx" => DocTarget::Xlsx,
                "pptx" => DocTarget::Pptx,
                _ => return Err(Error::Invalid(format!("문서 형식을 모릅니다: {sub}"))),
            },
        },
        _ => return Err(Error::Invalid(format!("작업 종류를 모릅니다: {kind}"))),
    })
}

/// 처리했으면 Some(종료 코드), 이 모듈 명령이 아니면 None.
pub fn run(cmd: &str, args: &[String]) -> Result<Option<ExitCode>> {
    let f = Flags::parse(&args[1.min(args.len())..]);
    match cmd {
        "tools" => {
            for st in tools::status_all(&tool_paths(&f), &SearchEnv::from_system()) {
                match &st.path {
                    Some(p) => println!("있음  {:?}  {}  ({}, 버전 {})", st.tool, p.display(), st.source.unwrap_or(""), st.version.as_deref().unwrap_or("확인 실패")),
                    None => println!("없음  {:?}  {}\n      {}\n      내려받기: {}", st.tool, st.label, st.guidance.unwrap_or(""), st.download_url),
                }
            }
            match filetool_core::pdfrender::pdfium_status() {
                Ok(p) => println!("있음  PDFium  {}", p.display()),
                Err(e) => println!("없음  PDFium  {e}"),
            }
            Ok(Some(ExitCode::SUCCESS))
        }
        "image" | "media" | "doc" => {
            let sub = f.rest.first().ok_or_else(|| Error::Invalid("형식을 적어 주세요".into()))?;
            let op = parse_op(cmd, sub, &f)?;
            let ts = toolset(&f);
            let out = f.get("--out").map(PathBuf::from);
            let mut failed = 0;
            for input in &f.rest[1..] {
                let t0 = Instant::now();
                let mut last = -1i32;
                let r = convert::convert_file(&ts, Path::new(input), out.as_deref(), &op, None, &mut |x| {
                    let pct = (x * 100.0) as i32;
                    if pct / 25 != last / 25 {
                        last = pct;
                        eprintln!("  진행 {pct}%");
                    }
                });
                match r {
                    Ok(o) => println!("완료  {} -> {} ({}, {:.1}초)", input, o.output.display(), o.note, t0.elapsed().as_secs_f64()),
                    Err(e) => {
                        failed += 1;
                        println!("실패  {input}: {e}");
                    }
                }
            }
            Ok(Some(if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }))
        }
        "watch" => {
            let folder = PathBuf::from(f.rest.first().ok_or_else(|| Error::Invalid("감시할 폴더를 적어 주세요".into()))?);
            let spec = f.rest.get(1).map(String::as_str).unwrap_or("image:jpg");
            let (kind, sub) = spec.split_once(':').ok_or_else(|| Error::Invalid("작업은 image:jpg 처럼 적어 주세요".into()))?;
            let rule = WatchRule {
                id: "cli".into(),
                folder: folder.clone(),
                op: parse_op(kind, sub, &f)?,
                out_subfolder: f.get("--sub").unwrap_or("변환됨").into(),
                enabled: true,
            };
            rule.validate().map_err(Error::Invalid)?;
            let ts = toolset(&f);
            let mut w = Watcher::new(folder.join(".win-filetool-watch.json"), Duration::from_secs(2));
            let until = Instant::now() + Duration::from_secs(f.num("--seconds", 60u64)?);
            println!("감시 시작: {} ({}) → {}", folder.display(), rule.op.label(), rule.out_dir().display());
            while Instant::now() < until {
                for ev in w.run_once(std::slice::from_ref(&rule), &ts, None, Instant::now()) {
                    println!("{}  {} -> {} {}", if ev.ok { "완료" } else { "실패" }, ev.input.display(), ev.output.map(|p| p.display().to_string()).unwrap_or_default(), ev.message);
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            Ok(Some(ExitCode::SUCCESS))
        }
        _ => Ok(None),
    }
}
