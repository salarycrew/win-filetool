//! 영상·소리 변환(ffmpeg 프리셋). 진행률은 `-progress pipe:1`의 `out_time_us`를 길이(ffprobe)로 나눠 계산한다.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::process::{self, Cancel};
use crate::paths;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "preset", rename_all = "snake_case")]
pub enum MediaPreset {
    /// MP4(H.264/AAC)로 바꾸기
    Mp4,
    /// 메신저용으로 줄이기: 목표 크기(MB) 이하, 세로 해상도 최대 `max_height`
    Messenger { max_mb: u32, max_height: u32 },
    /// 소리만 MP3로
    Mp3,
    /// 소리만 M4A(AAC)로
    M4a,
    /// 짧은 GIF: 시작(초), 길이(초), 가로(px), 초당 장면 수
    Gif { start: f64, duration: f64, width: u32, fps: u32 },
}

impl MediaPreset {
    pub fn label(&self) -> String {
        match self {
            MediaPreset::Mp4 => "MP4로 바꾸기 (H.264/AAC)".into(),
            MediaPreset::Messenger { max_mb, max_height } => format!("메신저용으로 줄이기 ({max_mb}MB 이하, {max_height}p)"),
            MediaPreset::Mp3 => "소리만 MP3로".into(),
            MediaPreset::M4a => "소리만 M4A로".into(),
            MediaPreset::Gif { duration, .. } => format!("짧은 GIF로 ({duration}초)"),
        }
    }
    pub fn ext(&self) -> &'static str {
        match self {
            MediaPreset::Mp4 | MediaPreset::Messenger { .. } => "mp4",
            MediaPreset::Mp3 => "mp3",
            MediaPreset::M4a => "m4a",
            MediaPreset::Gif { .. } => "gif",
        }
    }
    fn suffix(&self) -> &'static str {
        match self {
            MediaPreset::Messenger { .. } => "_메신저",
            _ => "",
        }
    }
    pub fn needs_video(&self) -> bool {
        matches!(self, MediaPreset::Mp4 | MediaPreset::Messenger { .. } | MediaPreset::Gif { .. })
    }
}

pub const VIDEO_EXTS: [&str; 12] = ["mov", "mkv", "avi", "webm", "mp4", "m4v", "wmv", "flv", "3gp", "mts", "m2ts", "ts"];
pub const AUDIO_EXTS: [&str; 8] = ["mp3", "m4a", "wav", "flac", "ogg", "aac", "wma", "opus"];

/// 메신저용 비디오 비트레이트(kbps). 목표 크기의 92%를 쓰고 소리 96k를 뺀다.
pub fn messenger_video_kbps(max_mb: u32, duration_s: f64) -> u32 {
    let total_kbps = (max_mb as f64 * 1024.0 * 1024.0 * 8.0 * 0.92) / 1000.0 / duration_s.max(0.5);
    ((total_kbps - 96.0).max(100.0)) as u32
}

/// ffmpeg 인자(argv). `duration`은 메신저 프리셋의 비트레이트 계산에 쓴다.
pub fn build_args(preset: &MediaPreset, input: &Path, output: &Path, duration: Option<f64>) -> Vec<OsString> {
    let mut a: Vec<OsString> = ["-hide_banner", "-nostdin", "-n", "-loglevel", "error", "-progress", "pipe:1", "-nostats"]
        .iter()
        .map(OsString::from)
        .collect();
    let push = |a: &mut Vec<OsString>, xs: &[&str]| a.extend(xs.iter().map(OsString::from));
    if let MediaPreset::Gif { start, duration, .. } = preset {
        push(&mut a, &["-ss", &format!("{start}"), "-t", &format!("{duration}")]);
    }
    a.push("-i".into());
    a.push(input.into());
    match preset {
        MediaPreset::Mp4 => push(
            &mut a,
            &["-map", "0:v:0", "-map", "0:a:0?", "-c:v", "libx264", "-preset", "medium", "-crf", "23", "-pix_fmt", "yuv420p",
              "-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart"],
        ),
        MediaPreset::Messenger { max_mb, max_height } => {
            push(&mut a, &["-map", "0:v:0", "-map", "0:a:0?", "-vf", &format!("scale=-2:'min({max_height},ih)'"), "-c:v", "libx264", "-preset", "medium"]);
            match duration {
                Some(d) if d > 0.0 => {
                    let v = messenger_video_kbps(*max_mb, d);
                    push(&mut a, &["-b:v", &format!("{v}k"), "-maxrate", &format!("{v}k"), "-bufsize", &format!("{}k", v * 2)]);
                }
                _ => push(&mut a, &["-crf", "28"]),
            }
            push(&mut a, &["-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "96k", "-ac", "2", "-movflags", "+faststart"]);
        }
        MediaPreset::Mp3 => push(&mut a, &["-vn", "-map", "0:a:0", "-c:a", "libmp3lame", "-q:a", "2"]),
        MediaPreset::M4a => push(&mut a, &["-vn", "-map", "0:a:0", "-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart"]),
        MediaPreset::Gif { width, fps, .. } => push(
            &mut a,
            &["-vf", &format!("fps={fps},scale={width}:-2:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse"), "-loop", "0"],
        ),
    }
    a.push(output.into());
    a
}

/// `-progress` 한 줄 → 처리한 시간(초).
pub fn parse_progress_secs(line: &str) -> Option<f64> {
    let (k, v) = line.trim().split_once('=')?;
    match k {
        // out_time_ms도 실제로는 마이크로초다(ffmpeg의 오래된 이름)
        "out_time_us" | "out_time_ms" => v.trim().parse::<i64>().ok().filter(|x| *x >= 0).map(|x| x as f64 / 1e6),
        "out_time" => {
            let mut parts = v.trim().split(':');
            let h: f64 = parts.next()?.parse().ok()?;
            let m: f64 = parts.next()?.parse().ok()?;
            let s: f64 = parts.next()?.parse().ok()?;
            Some(h * 3600.0 + m * 60.0 + s)
        }
        _ => None,
    }
}

/// ffprobe로 길이(초)를 얻는다.
pub fn probe_duration(ffprobe: &Path, input: &Path) -> Option<f64> {
    let args: Vec<OsString> = vec![
        "-v".into(), "error".into(), "-show_entries".into(), "format=duration".into(),
        "-of".into(), "default=noprint_wrappers=1:nokey=1".into(), input.into(),
    ];
    let out = process::run(ffprobe, &args, Duration::from_secs(60), None, &mut |_| {}).ok()?;
    out.stdout.lines().find_map(|l| l.trim().parse::<f64>().ok()).filter(|d| d.is_finite() && *d > 0.0)
}

/// 스트림 정보(점검용): ffprobe `-show_entries stream=codec_type,codec_name,width,height` 결과 줄들.
pub fn probe_streams(ffprobe: &Path, input: &Path) -> Vec<String> {
    let args: Vec<OsString> = vec![
        "-v".into(), "error".into(), "-show_entries".into(), "stream=codec_type,codec_name,width,height".into(),
        "-of".into(), "compact=p=0:nk=0".into(), input.into(),
    ];
    process::run(ffprobe, &args, Duration::from_secs(60), None, &mut |_| {})
        .map(|o| o.stdout.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaOut {
    pub output: PathBuf,
    pub bytes: u64,
    pub duration: Option<f64>,
}

/// 영상·소리 변환. `progress(0.0..=1.0)`. 실패·취소하면 만들던 출력 파일을 지운다.
pub fn convert(
    ffmpeg: &Path,
    ffprobe: Option<&Path>,
    input: &Path,
    out_dir: Option<&Path>,
    preset: &MediaPreset,
    cancel: Option<&Cancel>,
    timeout: Duration,
    progress: &mut dyn FnMut(f64),
) -> Result<MediaOut> {
    if !input.is_file() {
        return Err(Error::Invalid(format!("파일이 없습니다: {}", input.display())));
    }
    let duration = ffprobe.and_then(|p| probe_duration(p, input));
    let total = match preset {
        MediaPreset::Gif { duration: d, .. } => Some(duration.map(|x| x.min(*d)).unwrap_or(*d)),
        _ => duration,
    };
    let output = paths::output_path(input, out_dir, preset.suffix(), preset.ext());
    if let Some(p) = output.parent() {
        std::fs::create_dir_all(p).map_err(crate::error::io_err(p))?;
    }
    let args = build_args(preset, input, &output, duration);
    let mut on_line = |line: &str| {
        if let (Some(t), Some(total)) = (parse_progress_secs(line), total) {
            progress((t / total).clamp(0.0, 1.0));
        }
        if line.trim() == "progress=end" {
            progress(1.0);
        }
    };
    let res = process::run(ffmpeg, &args, timeout, cancel, &mut on_line);
    let ok = match &res {
        Ok(o) if o.success() && output.is_file() => true,
        _ => false,
    };
    if !ok {
        let _ = std::fs::remove_file(&output);
        return Err(match res {
            Err(e) => e,
            Ok(o) => Error::Tool { tool: "ffmpeg".into(), message: process::last_lines(&o.stderr_tail, 3) },
        });
    }
    crate::imageconv::copy_mtime(input, &output);
    let bytes = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    Ok(MediaOut { output, bytes, duration })
}
