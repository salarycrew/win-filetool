//! 0.3.0 W4 영상·소리 고치기: 정보 보기(ffprobe), 화면 한 장 뽑기(JPG), 앞뒤 자르기, 소리 빼기, 돌리기.
//!
//! - 다시 압축하지 않는 길(빠르고 화질 그대로): MP4/MOV/M4V 영상, MP3/M4A 소리.
//!   - 자르기는 시작점 바로 앞의 장면 전환점(키프레임)에서 시작한다(최대 [`COPY_MAX_EARLY`]초 일찍). 더 멀면 다시 압축.
//!   - 돌리기는 회전 정보(display matrix)만 바꾼다(플레이어가 돌려서 보여 줌).
//! - 그 밖의 형식(MKV·AVI·WebM·WMV…)이거나 위 조건이 안 맞으면 앱에 든 LGPL ffmpeg로 다시 압축한다
//!   (H.264는 Windows Media Foundation `h264_mf`, libx264가 있는 ffmpeg면 libx264 — [`media::detect_encoder`]).
//!   돌리기는 픽셀을 돌린다(transpose).
//! - 결과는 `이름_편집.확장자`(겹치면 ` (1)`). 실패·취소하면 만들던 파일을 지운다.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{io_err, Error, Result};
use crate::media::{self, VideoEncoder};
use crate::process::{self, Cancel};
use crate::paths;

pub const SUFFIX: &str = "_편집";
/// 다시 압축하지 않고 자를 때 허용하는, 시작점보다 일찍 시작하는 최대 시간(초)
pub const COPY_MAX_EARLY: f64 = 1.0;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MediaProbe {
    /// 초
    pub duration: f64,
    /// 보이는 크기(회전 반영). 소리만 있으면 0
    pub width: u32,
    pub height: u32,
    /// 시계 방향 0 | 90 | 180 | 270 (플레이어가 돌려서 보여 주는 각도)
    pub rotation: u16,
    pub has_audio: bool,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    /// 처음 시각(초, 보통 0) — 자르기 계산용
    #[serde(skip)]
    pub start_time: f64,
}

fn fferr(tool: &str, out: &process::RunOutput) -> Error {
    Error::Tool { tool: tool.into(), message: process::last_lines(&out.stderr_tail, 3) }
}

fn os(v: &[&str]) -> Vec<OsString> {
    v.iter().map(OsString::from).collect()
}

/// ffprobe JSON → 정보
pub fn parse_probe(json: &str) -> Option<MediaProbe> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let num = |x: Option<&serde_json::Value>| -> Option<f64> {
        let x = x?;
        x.as_f64().or_else(|| x.as_str()?.trim().parse().ok())
    };
    let fmt = v.get("format");
    let mut p = MediaProbe {
        duration: num(fmt.and_then(|f| f.get("duration"))).unwrap_or(0.0),
        width: 0,
        height: 0,
        rotation: 0,
        has_audio: false,
        video_codec: None,
        audio_codec: None,
        start_time: num(fmt.and_then(|f| f.get("start_time"))).unwrap_or(0.0),
    };
    for s in v.get("streams")?.as_array()? {
        let ty = s.get("codec_type").and_then(|t| t.as_str()).unwrap_or("");
        let codec = s.get("codec_name").and_then(|t| t.as_str()).map(str::to_string);
        match ty {
            "video" if p.video_codec.is_none() => {
                let attached = s.get("disposition").and_then(|d| d.get("attached_pic")).and_then(|a| a.as_i64()) == Some(1);
                if attached {
                    continue;
                }
                p.video_codec = codec;
                let w = s.get("width").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                let h = s.get("height").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                // display matrix의 rotation은 반시계 방향, 옛 tags.rotate는 시계 방향
                let ccw = s
                    .get("side_data_list")
                    .and_then(|l| l.as_array())
                    .and_then(|l| l.iter().find_map(|d| num(d.get("rotation"))));
                let cw = match ccw {
                    Some(r) => -r,
                    None => num(s.get("tags").and_then(|t| t.get("rotate"))).unwrap_or(0.0),
                };
                let cw = ((cw.round() as i64).rem_euclid(360) as u16 / 90) * 90;
                p.rotation = cw;
                (p.width, p.height) = if cw % 180 == 90 { (h, w) } else { (w, h) };
                if p.duration <= 0.0 {
                    p.duration = num(s.get("duration")).unwrap_or(0.0);
                }
            }
            "audio" if p.audio_codec.is_none() => {
                p.has_audio = true;
                p.audio_codec = codec;
            }
            _ => {}
        }
    }
    Some(p)
}

pub fn probe(ffprobe: &Path, input: &Path) -> Result<MediaProbe> {
    if !input.is_file() {
        return Err(Error::Invalid(format!("파일이 없습니다: {}", input.display())));
    }
    let mut args = os(&["-v", "error", "-show_format", "-show_streams", "-of", "json"]);
    args.push(input.into());
    let out = process::run(ffprobe, &args, Duration::from_secs(60), None, &mut |_| {})?;
    if !out.success() {
        return Err(Error::Unsupported(format!("영상·소리 파일로 읽지 못했습니다: {}", input.display())));
    }
    parse_probe(&out.stdout).filter(|p| p.video_codec.is_some() || p.has_audio).ok_or_else(|| Error::Unsupported(format!("영상·소리 파일로 읽지 못했습니다: {}", input.display())))
}

/// 시각 `at`(초)의 화면 한 장을 JPG 바이트로. `width`를 주면 그 가로로 줄인다(세로는 비율대로).
pub fn frame_jpeg(ffmpeg: &Path, input: &Path, at: f64, width: Option<u32>, duration: Option<f64>, cancel: Option<&Cancel>) -> Result<Vec<u8>> {
    let tmp = crate::tempdir::JobDir::new("frame")?;
    let out = tmp.path().join("f.jpg");
    let mut at = at.max(0.0);
    if let Some(d) = duration.filter(|d| *d > 0.0) {
        at = at.min((d - 0.05).max(0.0));
    }
    for t in [at, (at - 1.0).max(0.0), 0.0] {
        let mut a = os(&["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-ss"]);
        a.push(format!("{t:.3}").into());
        a.push("-i".into());
        a.push(input.into());
        a.extend(os(&["-map", "0:v:0", "-frames:v", "1", "-an"]));
        if let Some(w) = width.filter(|w| *w > 0) {
            a.extend(os(&["-vf", &format!("scale={w}:-2")]));
        }
        a.extend(os(&["-c:v", "mjpeg", "-q:v", "3", "-f", "image2", "-update", "1"]));
        a.push(out.clone().into());
        let r = process::run(ffmpeg, &a, Duration::from_secs(60), cancel, &mut |_| {})?;
        if r.success() && out.is_file() && std::fs::metadata(&out).map(|m| m.len() > 0).unwrap_or(false) {
            return std::fs::read(&out).map_err(io_err(&out));
        }
        if t == 0.0 {
            return Err(fferr("ffmpeg", &r));
        }
    }
    unreachable!()
}

/// `이름_장면_mm-ss.jpg`
pub fn frame_name_suffix(at: f64) -> String {
    let s = at.max(0.0).floor() as u64;
    format!("_장면_{:02}-{:02}", s / 60, s % 60)
}

/// 화면 한 장을 원본 옆(또는 `out_dir`)에 `이름_장면_mm-ss.jpg`로 저장
pub fn frame_save(ffmpeg: &Path, ffprobe: Option<&Path>, input: &Path, at: f64, out_dir: Option<&Path>, cancel: Option<&Cancel>) -> Result<PathBuf> {
    let duration = ffprobe.and_then(|p| probe(p, input).ok()).map(|p| p.duration).filter(|d| *d > 0.0);
    // 길이보다 뒤를 고르면 끝 장면(이름도 그 시각으로)
    let at = match duration {
        Some(d) => at.max(0.0).min((d - 0.05).max(0.0)),
        None => at.max(0.0),
    };
    let bytes = frame_jpeg(ffmpeg, input, at, None, duration, cancel)?;
    let output = paths::output_path(input, out_dir, &frame_name_suffix(at), "jpg");
    paths::write_new(&output, &bytes)?;
    Ok(output)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Trim {
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MediaOps {
    #[serde(default)]
    pub trim: Option<Trim>,
    #[serde(default)]
    pub mute: Option<bool>,
    /// 시계 방향 90 | 180 | 270
    #[serde(default)]
    pub rotate: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaEditOut {
    pub output: PathBuf,
    /// 바이트
    pub before: u64,
    pub after: u64,
    /// 결과 길이(초)
    pub duration: f64,
    /// 다시 압축했는지
    pub reencoded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// 계획: 다시 압축하지 않고(copy) / 다시 압축(reencode)
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub copy: bool,
    pub ext: &'static str,
    /// copy: 실제 시작(키프레임, 파일 처음 기준 초). reencode: 요청한 시작
    pub start: f64,
    pub end: Option<f64>,
    pub mute: bool,
    /// copy: 새 회전(시계 방향 합). reencode: 픽셀을 더 돌릴 각도
    pub rotate: Option<u16>,
    pub note: Option<String>,
}

fn ext_of(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// 키프레임 시각 목록(파일 처음 기준 초)을 받아 계획을 세운다.
pub fn plan(input: &Path, info: &MediaProbe, ops: &MediaOps, keyframes: &[f64]) -> Result<Plan> {
    let ext = ext_of(input);
    let has_video = info.video_codec.is_some();
    let mute = ops.mute.unwrap_or(false);
    let rotate = ops.rotate.filter(|r| *r % 360 != 0);
    if let Some(r) = rotate {
        if !matches!(r, 90 | 180 | 270) {
            return Err(Error::Invalid(format!("돌리기는 90·180·270도만 됩니다: {r}")));
        }
        if !has_video {
            return Err(Error::Invalid("소리만 있는 파일은 돌릴 수 없습니다".into()));
        }
    }
    if mute && !has_video {
        return Err(Error::Invalid("소리만 있는 파일에서는 소리를 뺄 수 없습니다".into()));
    }
    if mute && !info.has_audio {
        return Err(Error::Invalid("이 영상에는 소리가 없습니다".into()));
    }
    let trim = match ops.trim {
        Some(t) => {
            let end = if info.duration > 0.0 { t.end.min(info.duration) } else { t.end };
            if !(t.start >= 0.0 && end - t.start >= 0.1) {
                return Err(Error::Invalid(format!("자를 구간이 올바르지 않습니다 ({:.1}초 ~ {:.1}초)", t.start, t.end)));
            }
            Some((t.start, end))
        }
        None => None,
    };
    if trim.is_none() && !mute && rotate.is_none() {
        return Err(Error::Invalid("바꿀 내용이 없습니다(자르기·소리 빼기·돌리기 중 하나를 고르세요)".into()));
    }
    let copy_ext: Option<&'static str> = match (has_video, ext.as_str()) {
        (true, "mp4") => Some("mp4"),
        (true, "mov") => Some("mov"),
        (true, "m4v") => Some("m4v"),
        (false, "mp3") => Some("mp3"),
        (false, "m4a") => Some("m4a"),
        _ => None,
    };
    let note;
    if let Some(cext) = copy_ext {
        let start = match trim {
            None => Some(0.0),
            Some((s, _)) if s <= 0.0 || !has_video => Some(s),
            Some((s, _)) => keyframes.iter().copied().filter(|k| *k <= s + 0.01).fold(None, |a: Option<f64>, k| Some(a.map_or(k, |a| a.max(k)))).filter(|k| s - k <= COPY_MAX_EARLY),
        };
        if let Some(start) = start {
            let note = if let Some((s, _)) = trim {
                if s - start > 0.05 {
                    Some(format!(
                        "다시 압축하지 않으려고 가까운 장면 전환점에 맞춰 {:.1}초 일찍({}) 시작합니다",
                        s - start,
                        clock(start)
                    ))
                } else {
                    Some("다시 압축하지 않아 화질이 그대로입니다".into())
                }
            } else if rotate.is_some() {
                Some("회전 정보만 바꿔 화질이 그대로입니다".into())
            } else {
                Some("다시 압축하지 않아 화질이 그대로입니다".into())
            };
            return Ok(Plan {
                copy: true,
                ext: cext,
                start,
                end: trim.map(|t| t.1),
                mute,
                rotate: rotate.map(|r| (info.rotation + r) % 360),
                note,
            });
        }
        note = Some("시작점 근처에 장면 전환점이 없어 정확히 자르려고 다시 압축했습니다".into());
    } else {
        note = Some(if has_video {
            "이 형식은 그대로 고칠 수 없어 MP4로 다시 만들었습니다".into()
        } else {
            "이 형식은 그대로 고칠 수 없어 M4A로 다시 만들었습니다".into()
        });
    }
    Ok(Plan {
        copy: false,
        ext: if has_video { "mp4" } else { "m4a" },
        start: trim.map(|t| t.0).unwrap_or(0.0),
        end: trim.map(|t| t.1),
        mute,
        rotate,
        note,
    })
}

fn clock(t: f64) -> String {
    let s = t.max(0.0);
    format!("{}:{:04.1}", (s / 60.0).floor() as u64, s % 60.0)
}

/// ffmpeg 인자. `enc`는 다시 압축할 때만 쓴다.
pub fn build_args(input: &Path, output: &Path, p: &Plan, has_video: bool, enc: VideoEncoder) -> Vec<OsString> {
    let mut a = os(&["-hide_banner", "-nostdin", "-n", "-loglevel", "error", "-progress", "pipe:1", "-nostats"]);
    if p.copy {
        if let Some(cw) = p.rotate {
            // -display_rotation은 반시계 방향
            a.extend(os(&["-display_rotation:v:0", &format!("{}", (360 - cw as i32) % 360)]));
        }
    }
    if p.start > 0.0 {
        a.extend(os(&["-ss", &format!("{:.3}", p.start)]));
    }
    a.push("-i".into());
    a.push(input.into());
    if let Some(e) = p.end {
        a.extend(os(&["-t", &format!("{:.3}", (e - p.start).max(0.1))]));
    }
    if has_video {
        a.extend(os(&["-map", "0:v:0"]));
        if !p.mute {
            a.extend(os(&["-map", "0:a:0?"]));
        }
    } else {
        a.extend(os(&["-map", "0:a:0"]));
    }
    if p.copy {
        a.extend(os(&["-c", "copy"]));
        if p.mute {
            a.push("-an".into());
        }
        a.extend(os(&["-avoid_negative_ts", "make_zero"]));
    } else {
        if has_video {
            let vf = match p.rotate {
                Some(90) => Some("transpose=clock"),
                Some(180) => Some("hflip,vflip"),
                Some(270) => Some("transpose=cclock"),
                _ => None,
            };
            if let Some(vf) = vf {
                a.extend(os(&["-vf", vf]));
            }
            match enc {
                VideoEncoder::X264 => a.extend(os(&["-c:v", "libx264", "-preset", "medium", "-crf", "20"])),
                VideoEncoder::Mf => a.extend(os(&["-c:v", "h264_mf", "-rate_control", "quality", "-quality", "75"])),
            }
            a.extend(os(&["-pix_fmt", "yuv420p"]));
        }
        if p.mute {
            a.push("-an".into());
        } else {
            a.extend(os(&["-c:a", "aac", "-b:a", "160k"]));
        }
    }
    if matches!(p.ext, "mp4" | "mov" | "m4v" | "m4a") {
        a.extend(os(&["-movflags", "+faststart"]));
    }
    a.push(output.into());
    a
}

/// 시작점 근처 키프레임 시각(파일 처음 기준 초)
pub fn keyframes_near(ffprobe: &Path, input: &Path, at: f64, start_time: f64) -> Vec<f64> {
    let from = (at - 10.0).max(0.0) + start_time;
    let to = at + start_time + 0.05;
    let mut args = os(&["-v", "error", "-select_streams", "v:0", "-skip_frame", "nokey", "-show_entries", "frame=pts_time,best_effort_timestamp_time", "-of", "csv=p=0", "-read_intervals"]);
    args.push(format!("{from:.3}%{to:.3}").into());
    args.push(input.into());
    let Ok(out) = process::run(ffprobe, &args, Duration::from_secs(60), None, &mut |_| {}) else { return vec![] };
    out.stdout
        .lines()
        .filter_map(|l| l.split(',').find_map(|x| x.trim().parse::<f64>().ok()))
        .map(|t| t - start_time)
        .collect()
}

/// 영상 하나 고치기. `progress(0..=1)`
pub fn edit(
    ffmpeg: &Path,
    ffprobe: &Path,
    input: &Path,
    ops: &MediaOps,
    out_dir: Option<&Path>,
    cancel: Option<&Cancel>,
    progress: &mut dyn FnMut(f64),
) -> Result<MediaEditOut> {
    let info = probe(ffprobe, input)?;
    let has_video = info.video_codec.is_some();
    let kf = match ops.trim {
        Some(t) if t.start > 0.0 && has_video => keyframes_near(ffprobe, input, t.start, info.start_time),
        _ => vec![],
    };
    let p = plan(input, &info, ops, &kf)?;
    let enc = if p.copy || !has_video { VideoEncoder::Mf } else { media::detect_encoder(ffmpeg) };
    let output = paths::output_path(input, out_dir, SUFFIX, p.ext);
    if let Some(d) = output.parent() {
        std::fs::create_dir_all(d).map_err(io_err(d))?;
    }
    let args = build_args(input, &output, &p, has_video, enc);
    let total = p.end.unwrap_or(info.duration) - p.start;
    let mut on_line = |line: &str| {
        if let Some(t) = media::parse_progress_secs(line) {
            if total > 0.0 {
                progress((t / total).clamp(0.0, 1.0));
            }
        }
    };
    let res = process::run(ffmpeg, &args, Duration::from_secs(6 * 3600), cancel, &mut on_line);
    let ok = matches!(&res, Ok(o) if o.success() && output.is_file());
    if !ok {
        let _ = std::fs::remove_file(&output);
        return Err(match res {
            Err(e) => e,
            Ok(o) => fferr("ffmpeg", &o),
        });
    }
    progress(1.0);
    let before = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let after = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    let duration = probe(ffprobe, &output).map(|i| i.duration).unwrap_or(total);
    Ok(MediaEditOut { output, before, after, duration, reencoded: !p.copy, note: p.note })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(rot: u16, audio: bool) -> MediaProbe {
        MediaProbe { duration: 10.0, width: 320, height: 240, rotation: rot, has_audio: audio, video_codec: Some("h264".into()), audio_codec: audio.then(|| "aac".into()), start_time: 0.0 }
    }

    #[test]
    fn probe_json() {
        let j = r#"{"streams":[{"codec_type":"video","codec_name":"hevc","width":1920,"height":1080,
            "side_data_list":[{"side_data_type":"Display Matrix","rotation":-90}]},
            {"codec_type":"audio","codec_name":"aac"},{"codec_type":"data","codec_name":"none"}],
            "format":{"duration":"12.345000","start_time":"0.000000"}}"#;
        let p = parse_probe(j).unwrap();
        assert_eq!((p.width, p.height, p.rotation, p.has_audio), (1080, 1920, 90, true));
        assert_eq!(p.video_codec.as_deref(), Some("hevc"));
        assert!((p.duration - 12.345).abs() < 1e-9);
        // 옛 rotate 태그(시계 방향), 표지 그림은 영상이 아님
        let j = r#"{"streams":[{"codec_type":"video","codec_name":"mjpeg","width":10,"height":10,"disposition":{"attached_pic":1}},
            {"codec_type":"audio","codec_name":"mp3"}],"format":{"duration":"3.0"}}"#;
        let p = parse_probe(j).unwrap();
        assert!(p.video_codec.is_none() && p.has_audio);
        let j = r#"{"streams":[{"codec_type":"video","codec_name":"h264","width":640,"height":480,"tags":{"rotate":"270"}}],"format":{"duration":"1"}}"#;
        assert_eq!(parse_probe(j).unwrap().rotation, 270);
    }

    #[test]
    fn plans() {
        let mp4 = Path::new("a.mp4");
        let t = |s, e| MediaOps { trim: Some(Trim { start: s, end: e }), ..Default::default() };
        // 키프레임 2.0이 시작 2.4보다 0.4초 앞: 그대로 자름
        let p = plan(mp4, &info(0, true), &t(2.4, 6.0), &[0.0, 2.0, 4.0]).unwrap();
        assert!(p.copy && p.start == 2.0 && p.end == Some(6.0));
        assert!(p.note.unwrap().contains("0.4초 일찍"));
        // 가까운 키프레임이 없으면 다시 압축
        let p = plan(mp4, &info(0, true), &t(5.5, 8.0), &[0.0, 2.0]).unwrap();
        assert!(!p.copy && p.start == 5.5 && p.ext == "mp4");
        // MKV는 다시 압축해 MP4로
        let p = plan(Path::new("a.mkv"), &info(0, true), &MediaOps { mute: Some(true), ..Default::default() }, &[]).unwrap();
        assert!(!p.copy && p.ext == "mp4" && p.mute);
        // 돌리기: 기존 90 + 90 = 180(회전 정보만)
        let p = plan(Path::new("a.MOV"), &info(90, true), &MediaOps { rotate: Some(90), ..Default::default() }, &[]).unwrap();
        assert!(p.copy && p.rotate == Some(180) && p.ext == "mov");
        let a = build_args(Path::new("a.mov"), Path::new("b.mov"), &p, true, VideoEncoder::Mf);
        let a: Vec<String> = a.iter().map(|x| x.to_string_lossy().into_owned()).collect();
        assert!(a.windows(2).any(|w| w == ["-display_rotation:v:0", "180"]));
        // 소리만 있는 파일
        let mut s = info(0, true);
        s.video_codec = None;
        assert!(plan(Path::new("a.mp3"), &s, &MediaOps { rotate: Some(90), ..Default::default() }, &[]).is_err());
        assert!(plan(Path::new("a.mp3"), &s, &MediaOps { mute: Some(true), ..Default::default() }, &[]).is_err());
        let p = plan(Path::new("a.mp3"), &s, &t(1.0, 2.0), &[]).unwrap();
        assert!(p.copy && p.start == 1.0 && p.ext == "mp3");
        let p = plan(Path::new("a.wav"), &s, &t(1.0, 2.0), &[]).unwrap();
        assert!(!p.copy && p.ext == "m4a");
        // 잘못된 값
        assert!(plan(mp4, &info(0, true), &t(5.0, 5.0), &[]).is_err());
        assert!(plan(mp4, &info(0, true), &MediaOps::default(), &[]).is_err());
        assert!(plan(mp4, &info(0, false), &MediaOps { mute: Some(true), ..Default::default() }, &[]).is_err());
        assert_eq!(frame_name_suffix(75.9), "_장면_01-15");
    }
}
