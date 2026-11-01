//! 영상·소리 변환(ffmpeg 프리셋). 진행률은 `-progress pipe:1`의 `out_time_us`를 길이(ffprobe)로 나눠 계산한다.
//!
//! 0.3.0: 앱에 든 ffmpeg는 LGPL 빌드라 libx264가 없고 H.264 인코딩을 Windows Media Foundation(`h264_mf`)에 맡긴다.
//! 쓰는 ffmpeg에 libx264가 있으면(사용자가 지정한 ffmpeg) 예전처럼 libx264를 쓴다([`detect_encoder`]).
//! `h264_mf`는 목표 비트레이트를 넘기 쉬우므로, 메신저용은 만든 뒤 목표 크기를 넘으면 비트레이트를 낮춰 다시 만든다(최대 3번).

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

/// H.264 인코더
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoEncoder {
    /// libx264(GPL ffmpeg 빌드에만 있음)
    X264,
    /// Windows Media Foundation(앱에 든 LGPL ffmpeg)
    Mf,
}

/// `ffmpeg -encoders` 출력 → 쓸 인코더(libx264가 있으면 그것, 없으면 h264_mf)
pub fn encoder_from_list(encoders_stdout: &str) -> VideoEncoder {
    if encoders_stdout.split_whitespace().any(|w| w == "libx264") {
        VideoEncoder::X264
    } else {
        VideoEncoder::Mf
    }
}

static ENCODERS: std::sync::Mutex<Vec<(PathBuf, VideoEncoder)>> = std::sync::Mutex::new(Vec::new());

/// 이 ffmpeg에서 쓸 H.264 인코더(경로별로 한 번만 물어본다)
pub fn detect_encoder(ffmpeg: &Path) -> VideoEncoder {
    if let Some((_, e)) = ENCODERS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(p, _)| p == ffmpeg) {
        return *e;
    }
    let args: Vec<OsString> = vec!["-hide_banner".into(), "-encoders".into()];
    let enc = match process::run(ffmpeg, &args, Duration::from_secs(30), None, &mut |_| {}) {
        Ok(o) if o.success() => encoder_from_list(&o.stdout),
        // 물어보지 못하면 앱에 든 빌드와 같다고 본다
        _ => VideoEncoder::Mf,
    };
    ENCODERS.lock().unwrap_or_else(|e| e.into_inner()).push((ffmpeg.to_path_buf(), enc));
    enc
}

/// 메신저용 재시도 최대 횟수(처음 한 번 포함)
pub const MESSENGER_MAX_TRIES: u32 = 3;

/// 목표 크기를 넘었을 때 다음 비디오 비트레이트(kbps). 소리(96k)는 그대로 두고, 이번 결과에서 본
/// "인코더가 실제로 낸 비디오 비트레이트 / 요청한 비트레이트" 비율만큼 요청을 줄인다(5% 여유).
/// 언제나 이번 값의 90% 이하, 최소 64k.
pub fn next_video_kbps(cur_kbps: u32, got_bytes: u64, target_bytes: u64, duration_s: f64) -> u32 {
    let d = duration_s.max(0.5);
    let to_kbps = |b: u64| b as f64 * 8.0 / 1000.0 / d;
    let got_video = (to_kbps(got_bytes) - 96.0).max(1.0);
    let ratio = (got_video / cur_kbps.max(1) as f64).max(0.5);
    let want_video = (to_kbps(target_bytes) * 0.95 - 96.0).max(1.0);
    let next = (want_video / ratio).min(cur_kbps as f64 * 0.9);
    (next.max(64.0)) as u32
}

/// 목표 크기(바이트)
pub fn messenger_target_bytes(max_mb: u32) -> u64 {
    max_mb as u64 * 1024 * 1024
}

/// ffmpeg 인자(argv). `duration`은 메신저 프리셋의 비트레이트 계산에 쓴다. (libx264용 — 예전 인자 그대로)
pub fn build_args(preset: &MediaPreset, input: &Path, output: &Path, duration: Option<f64>) -> Vec<OsString> {
    build_args_enc(preset, input, output, duration, VideoEncoder::X264, None)
}

/// 인코더를 골라 인자를 만든다. `video_kbps`가 있으면 메신저용 비디오 비트레이트를 그 값으로(재시도).
pub fn build_args_enc(preset: &MediaPreset, input: &Path, output: &Path, duration: Option<f64>, enc: VideoEncoder, video_kbps: Option<u32>) -> Vec<OsString> {
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
        MediaPreset::Mp4 => {
            push(&mut a, &["-map", "0:v:0", "-map", "0:a:0?"]);
            match enc {
                VideoEncoder::X264 => push(&mut a, &["-c:v", "libx264", "-preset", "medium", "-crf", "23"]),
                VideoEncoder::Mf => push(&mut a, &["-c:v", "h264_mf", "-rate_control", "quality", "-quality", "70"]),
            }
            push(&mut a, &["-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart"]);
        }
        MediaPreset::Messenger { max_mb, max_height } => {
            push(&mut a, &["-map", "0:v:0", "-map", "0:a:0?", "-vf", &format!("scale=-2:'min({max_height},ih)'")]);
            match enc {
                VideoEncoder::X264 => push(&mut a, &["-c:v", "libx264", "-preset", "medium"]),
                VideoEncoder::Mf => push(&mut a, &["-c:v", "h264_mf"]),
            }
            let v = video_kbps.or(match duration {
                Some(d) if d > 0.0 => Some(messenger_video_kbps(*max_mb, d)),
                _ => None,
            });
            match (v, enc) {
                (Some(v), VideoEncoder::X264) => {
                    push(&mut a, &["-b:v", &format!("{v}k"), "-maxrate", &format!("{v}k"), "-bufsize", &format!("{}k", v * 2)])
                }
                // h264_mf: cbr가 목표에 가장 가까웠다(스파이크 측정)
                (Some(v), VideoEncoder::Mf) => push(&mut a, &["-rate_control", "cbr", "-b:v", &format!("{v}k")]),
                (None, VideoEncoder::X264) => push(&mut a, &["-crf", "28"]),
                (None, VideoEncoder::Mf) => push(&mut a, &["-rate_control", "quality", "-quality", "50"]),
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
    /// 만든 횟수(메신저용 크기 맞춤 재시도 포함, 보통 1)
    pub tries: u32,
    /// 메신저용 목표 크기(바이트)
    pub target_bytes: Option<u64>,
    /// 마지막 비디오 비트레이트(kbps, 메신저용)
    pub video_kbps: Option<u32>,
    pub encoder: VideoEncoder,
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
    crate::error::ensure_nonempty(input)?;
    let duration = ffprobe.and_then(|p| probe_duration(p, input));
    let total = match preset {
        MediaPreset::Gif { duration: d, .. } => Some(duration.map(|x| x.min(*d)).unwrap_or(*d)),
        _ => duration,
    };
    let output = paths::output_path(input, out_dir, preset.suffix(), preset.ext());
    if let Some(p) = output.parent() {
        std::fs::create_dir_all(p).map_err(crate::error::io_err(p))?;
    }
    let enc = if preset.needs_video() && !matches!(preset, MediaPreset::Gif { .. }) { detect_encoder(ffmpeg) } else { VideoEncoder::Mf };
    let target = match preset {
        MediaPreset::Messenger { max_mb, .. } => Some(messenger_target_bytes(*max_mb)),
        _ => None,
    };
    let mut kbps = match (preset, duration) {
        (MediaPreset::Messenger { max_mb, .. }, Some(d)) if d > 0.0 => Some(messenger_video_kbps(*max_mb, d)),
        _ => None,
    };
    let max_tries = if target.is_some() && kbps.is_some() { MESSENGER_MAX_TRIES } else { 1 };
    let mut tries = 0;
    loop {
        tries += 1;
        let args = build_args_enc(preset, input, &output, duration, enc, kbps);
        // 재시도는 진행률을 (지난 시도까지 몫) + (이번 몫)으로 나눠 보여 준다
        let (base, span) = ((tries - 1) as f64 / max_tries as f64, 1.0 / max_tries as f64);
        let scale = |f: f64| if max_tries == 1 { f } else { base + f * span };
        let mut on_line = |line: &str| {
            if let (Some(t), Some(total)) = (parse_progress_secs(line), total) {
                progress(scale((t / total).clamp(0.0, 1.0)));
            }
            if line.trim() == "progress=end" {
                progress(scale(1.0));
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
        let bytes = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
        if let (Some(t), Some(cur), Some(d)) = (target, kbps, duration) {
            if bytes > t && tries < max_tries {
                let _ = std::fs::remove_file(&output);
                kbps = Some(next_video_kbps(cur, bytes, t, d));
                continue;
            }
        }
        progress(1.0);
        crate::imageconv::copy_mtime(input, &output);
        return Ok(MediaOut { output, bytes, duration, tries, target_bytes: target, video_kbps: kbps, encoder: enc });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(a: &[OsString]) -> Vec<String> {
        a.iter().map(|x| x.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn encoder_detection() {
        let lgpl = " V....D h264_mf              H264 via MediaFoundation (codec h264)\n A....D aac                  AAC (Advanced Audio Coding)\n";
        assert_eq!(encoder_from_list(lgpl), VideoEncoder::Mf);
        let gpl = format!("{lgpl} V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC (codec h264)\n");
        assert_eq!(encoder_from_list(&gpl), VideoEncoder::X264);
    }

    #[test]
    fn mf_args() {
        let (i, o) = (Path::new("/v/a.mov"), Path::new("/v/a.mp4"));
        let a = s(&build_args_enc(&MediaPreset::Mp4, i, o, None, VideoEncoder::Mf, None));
        assert!(a.windows(2).any(|w| w == ["-c:v", "h264_mf"]));
        assert!(!a.iter().any(|x| x.contains("x264") || x == "-crf"));
        let m = MediaPreset::Messenger { max_mb: 25, max_height: 720 };
        let a = s(&build_args_enc(&m, i, o, Some(60.0), VideoEncoder::Mf, None));
        let v = messenger_video_kbps(25, 60.0);
        assert!(a.windows(2).any(|w| w == ["-rate_control", "cbr"]));
        assert!(a.windows(2).any(|w| w[0] == "-b:v" && w[1] == format!("{v}k")));
        // 재시도 값이 있으면 그 값을 쓴다
        let a = s(&build_args_enc(&m, i, o, Some(60.0), VideoEncoder::Mf, Some(512)));
        assert!(a.windows(2).any(|w| w == ["-b:v", "512k"]));
        // x264용은 예전 그대로
        assert_eq!(build_args(&m, i, o, Some(60.0)), build_args_enc(&m, i, o, Some(60.0), VideoEncoder::X264, None));
    }

    #[test]
    fn bitrate_steps_converge_under_target() {
        // 6초, 1MB 목표: 첫 요청 ≈1190k. 인코더가 요청의 1.6배를 낸다고 치면(스파이크: 1.27–1.66배)
        let (d, t) = (6.0, messenger_target_bytes(1));
        let first = messenger_video_kbps(1, d);
        assert!((1150..1250).contains(&first), "{first}");
        let encode = |kbps: u32, overshoot: f64| -> u64 { ((kbps as f64 * overshoot + 96.0) * 1000.0 / 8.0 * d) as u64 };
        let got1 = encode(first, 1.6);
        assert!(got1 > t);
        let second = next_video_kbps(first, got1, t, d);
        assert!(second < first * 9 / 10 + 1);
        let got2 = encode(second, 1.6);
        assert!(got2 <= t, "두 번째에 목표 안: {got2} > {t}");
        // 조금만 넘으면 적어도 10%는 내린다
        let third = next_video_kbps(1000, t + 1000, t, d);
        assert!(third <= 900);
        // 아주 짧은·아주 작은 목표라도 64k 아래로는 안 간다
        assert_eq!(next_video_kbps(100, 50_000_000, 1000, 1.0), 64);
        // 이상한 값(0)에도 멈추지 않는다
        assert!(next_video_kbps(0, 1, 1, 0.0) >= 64);
    }

    #[test]
    fn overshoot_three_tries_bound() {
        // 인코더가 요청을 2.2배까지 넘겨도 세 번 안에 목표 아래로
        let (d, t) = (6.0, messenger_target_bytes(1));
        let encode = |kbps: u32| -> u64 { ((kbps as f64 * 2.2 + 96.0) * 1000.0 / 8.0 * d) as u64 };
        let mut k = messenger_video_kbps(1, d);
        let mut tries = 1;
        let mut got = encode(k);
        while got > t && tries < MESSENGER_MAX_TRIES {
            k = next_video_kbps(k, got, t, d);
            got = encode(k);
            tries += 1;
        }
        assert!(got <= t && tries <= 3, "tries={tries} got={got}");
    }
}
