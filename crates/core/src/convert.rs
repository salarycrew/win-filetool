//! 변환 작업 하나(파일 하나)를 종류별 모듈로 보낸다. CLI·앱·감시 폴더가 같이 쓴다.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::docs::{self, DocTarget};
use crate::error::{Error, Result};
use crate::imageconv::{self, ImageOpts};
use crate::media::{self, MediaPreset};
use crate::process::Cancel;
use crate::tools::{Tool, Toolset};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    Image,
    Heic,
    Video,
    Audio,
    Document,
    Other,
}

pub fn classify(p: &Path) -> FileKind {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if imageconv::HEIF_EXTS.contains(&ext.as_str()) {
        FileKind::Heic
    } else if imageconv::RASTER_EXTS.contains(&ext.as_str()) {
        FileKind::Image
    } else if media::VIDEO_EXTS.contains(&ext.as_str()) {
        FileKind::Video
    } else if media::AUDIO_EXTS.contains(&ext.as_str()) {
        FileKind::Audio
    } else if docs::family(&ext).is_some() {
        FileKind::Document
    } else {
        FileKind::Other
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Op {
    Image(ImageOpts),
    Media(MediaPreset),
    Doc { target: DocTarget },
}

impl Op {
    /// 이 작업이 받을 수 있는 파일인지.
    pub fn accepts(&self, p: &Path) -> bool {
        match (self, classify(p)) {
            (Op::Image(_), FileKind::Image | FileKind::Heic) => true,
            (Op::Media(_), FileKind::Video) => true,
            (Op::Media(m), FileKind::Audio) => !m.needs_video(),
            (Op::Doc { target }, FileKind::Document) => docs::allowed_targets(p).contains(target),
            _ => false,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Op::Image(o) => format!("이미지 → {}", o.format.ext().to_uppercase()),
            Op::Media(m) => m.label(),
            Op::Doc { target } => format!("문서 → {}", target.ext().to_uppercase()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConvertOut {
    pub input: PathBuf,
    pub output: PathBuf,
    /// 사람이 읽을 요약(크기, 해상도, EXIF 날짜 등)
    pub note: String,
}

fn human_bytes(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / 1048576.0)
    } else {
        format!("{:.0} KB", (n as f64 / 1024.0).max(1.0))
    }
}

pub const MEDIA_TIMEOUT: Duration = Duration::from_secs(4 * 3600);

/// 파일 하나 변환. 원본은 바꾸지 않고, 결과는 `out_dir`(없으면 원본 옆)에 새 이름으로 쓴다.
pub fn convert_file(
    tools: &Toolset,
    input: &Path,
    out_dir: Option<&Path>,
    op: &Op,
    cancel: Option<&Cancel>,
    progress: &mut dyn FnMut(f64),
) -> Result<ConvertOut> {
    if let Some(c) = cancel {
        c.check()?;
    }
    if !input.is_file() {
        return Err(Error::Invalid(format!("파일이 없습니다: {}", input.display())));
    }
    if !op.accepts(input) {
        return Err(Error::Unsupported(format!("{} ({})", input.display(), op.label())));
    }
    let (output, note) = match op {
        Op::Image(o) => {
            let r = if imageconv::is_heif_path(input) || imageconv::sniff_heif(input) {
                imageconv::convert_heif(tools.get(Tool::Ffmpeg)?, input, out_dir, o, cancel)?
            } else {
                imageconv::convert_raster(input, out_dir, o)?
            };
            let mut note = format!("{}×{}", r.width, r.height);
            if let Some(d) = &r.exif_date {
                note.push_str(&format!(", 촬영 날짜 유지 {d}"));
            }
            (r.output, note)
        }
        Op::Media(m) => {
            let r = media::convert(tools.get(Tool::Ffmpeg)?, tools.ffprobe.as_deref(), input, out_dir, m, cancel, MEDIA_TIMEOUT, progress)?;
            let mut note = human_bytes(r.bytes);
            if let Some(d) = r.duration {
                note.push_str(&format!(", 원본 길이 {d:.1}초"));
            }
            (r.output, note)
        }
        Op::Doc { target } => {
            let out = docs::convert(tools.get(Tool::Soffice)?, input, out_dir, *target, cancel, docs::DOC_TIMEOUT)?;
            let n = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
            (out, human_bytes(n))
        }
    };
    progress(1.0);
    Ok(ConvertOut { input: input.to_path_buf(), output, note })
}
