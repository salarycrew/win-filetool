//! 문서 변환(LibreOffice headless `--convert-to`). LibreOffice는 동봉하지 않는다(사용자 설치·지정).
//!
//! 작업마다 개인 프로필(`-env:UserInstallation=file:///…`)을 임시 폴더에 만들어
//! 사용자의 LibreOffice 설정을 건드리지 않고, 동시에 여러 작업을 돌려도 충돌하지 않게 한다.
//! LibreOffice는 출력 이름을 원본 이름으로 정하고 덮어쓰므로, 임시 출력 폴더에 받은 뒤
//! 최종 위치에 겹치지 않는 이름(`이름 (1)`)으로 옮긴다.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{io_err, Error, Result};
use crate::process::{self, Cancel};
use crate::{paths, tempdir::JobDir};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocTarget {
    Pdf,
    Docx,
    Xlsx,
    Pptx,
}

impl DocTarget {
    pub fn ext(self) -> &'static str {
        match self {
            DocTarget::Pdf => "pdf",
            DocTarget::Docx => "docx",
            DocTarget::Xlsx => "xlsx",
            DocTarget::Pptx => "pptx",
        }
    }
    /// `--convert-to` 값(확장자:필터 이름)
    pub fn convert_to(self) -> &'static str {
        match self {
            DocTarget::Pdf => "pdf",
            DocTarget::Docx => "docx:MS Word 2007 XML",
            DocTarget::Xlsx => "xlsx:Calc MS Excel 2007 XML",
            DocTarget::Pptx => "pptx:Impress MS PowerPoint 2007 XML",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DocFamily {
    Text,
    Sheet,
    Slides,
}

/// 확장자 → 문서 종류. `hwp`는 LibreOffice의 한글 97 가져오기 필터(오래된 HWP)만 해당한다.
pub fn family(ext: &str) -> Option<DocFamily> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "docx" | "doc" | "odt" | "rtf" | "txt" | "pages" | "hwp" | "docm" | "dotx" | "fodt" | "wpd" => DocFamily::Text,
        "xlsx" | "xls" | "ods" | "csv" | "numbers" | "xlsm" | "fods" => DocFamily::Sheet,
        "pptx" | "ppt" | "odp" | "key" | "pps" | "ppsx" | "fodp" => DocFamily::Slides,
        _ => return None,
    })
}

pub fn family_of(p: &Path) -> Option<DocFamily> {
    p.extension().and_then(|e| e.to_str()).and_then(family)
}

/// 이 파일에서 바꿀 수 있는 형식: 언제나 PDF, 그리고 같은 종류의 MS Office 형식(원본과 같은 확장자 제외).
pub fn allowed_targets(p: &Path) -> Vec<DocTarget> {
    let Some(f) = family_of(p) else { return vec![] };
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let office = match f {
        DocFamily::Text => DocTarget::Docx,
        DocFamily::Sheet => DocTarget::Xlsx,
        DocFamily::Slides => DocTarget::Pptx,
    };
    let mut v = vec![DocTarget::Pdf];
    if office.ext() != ext {
        v.push(office);
    }
    v
}

/// 경로 → `file:///C:/…` URL(공백·한글은 퍼센트 인코딩).
pub fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// soffice 인자(argv).
pub fn build_args(profile_dir: &Path, out_dir: &Path, input: &Path, target: DocTarget) -> Vec<OsString> {
    vec![
        format!("-env:UserInstallation={}", file_url(profile_dir)).into(),
        "--headless".into(),
        "--invisible".into(),
        "--norestore".into(),
        "--nologo".into(),
        "--nodefault".into(),
        "--nolockcheck".into(),
        "--convert-to".into(),
        target.convert_to().into(),
        "--outdir".into(),
        out_dir.into(),
        input.into(),
    ]
}

fn move_into(from: &Path, to: &Path) -> Result<()> {
    if let Some(p) = to.parent() {
        std::fs::create_dir_all(p).map_err(io_err(p))?;
    }
    if to.exists() {
        return Err(Error::AlreadyExists(to.to_path_buf()));
    }
    if std::fs::rename(from, to).is_err() {
        // 다른 드라이브면 복사
        let bytes = std::fs::read(from).map_err(io_err(from))?;
        paths::write_new(to, &bytes)?;
    }
    Ok(())
}

pub const DOC_TIMEOUT: Duration = Duration::from_secs(300);

/// 문서 한 개 변환.
pub fn convert(soffice: &Path, input: &Path, out_dir: Option<&Path>, target: DocTarget, cancel: Option<&Cancel>, timeout: Duration) -> Result<PathBuf> {
    if !input.is_file() {
        return Err(Error::Invalid(format!("파일이 없습니다: {}", input.display())));
    }
    if !allowed_targets(input).contains(&target) {
        return Err(Error::Unsupported(format!("{} → {}", input.display(), target.ext())));
    }
    let job = JobDir::new("lo")?;
    let profile = job.path().join("profile");
    let tmp_out = job.path().join("out");
    std::fs::create_dir_all(&tmp_out).map_err(io_err(&tmp_out))?;
    let args = build_args(&profile, &tmp_out, input, target);
    // Windows: soffice.exe는 실행기라 먼저 끝날 수 있어, 끝까지 기다리는 soffice.com(콘솔용)으로 실행한다
    let program = crate::tools::soffice_console(soffice);
    let res = process::run(&program, &args, timeout, cancel, &mut |_| {})?;
    let produced: Vec<PathBuf> = std::fs::read_dir(&tmp_out)
        .map(|it| it.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect())
        .unwrap_or_default();
    let Some(made) = produced.into_iter().next() else {
        let detail = process::last_lines(&format!("{}\n{}", res.stdout, res.stderr_tail), 3);
        return Err(Error::Tool {
            tool: "LibreOffice".into(),
            message: format!("결과 파일이 생기지 않았습니다(이 형식을 읽지 못했을 수 있음). {detail}"),
        });
    };
    let output = paths::output_path(input, out_dir, "", target.ext());
    move_into(&made, &output)?;
    crate::imageconv::copy_mtime(input, &output);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_and_args() {
        assert_eq!(file_url(Path::new(r"C:\Users\user\AppData\Local\Temp\win-filetool\lo-1\profile")), "file:///C:/Users/user/AppData/Local/Temp/win-filetool/lo-1/profile");
        assert_eq!(file_url(Path::new("/tmp/a b/프로필")), "file:///tmp/a%20b/%ED%94%84%EB%A1%9C%ED%95%84");
        let a = build_args(Path::new("/tmp/p"), Path::new("/tmp/o"), Path::new("/x/발표.key"), DocTarget::Pptx);
        let a: Vec<String> = a.iter().map(|s| s.to_string_lossy().into_owned()).collect();
        assert_eq!(a[0], "-env:UserInstallation=file:///tmp/p");
        assert!(a.contains(&"--headless".to_string()));
        let i = a.iter().position(|x| x == "--convert-to").unwrap();
        assert_eq!(a[i + 1], "pptx:Impress MS PowerPoint 2007 XML");
        assert_eq!(a.last().unwrap(), "/x/발표.key");
    }

    #[test]
    fn targets() {
        assert_eq!(allowed_targets(Path::new("a.pages")), vec![DocTarget::Pdf, DocTarget::Docx]);
        assert_eq!(allowed_targets(Path::new("a.numbers")), vec![DocTarget::Pdf, DocTarget::Xlsx]);
        assert_eq!(allowed_targets(Path::new("a.KEY")), vec![DocTarget::Pdf, DocTarget::Pptx]);
        assert_eq!(allowed_targets(Path::new("a.docx")), vec![DocTarget::Pdf]);
        assert!(allowed_targets(Path::new("a.jpg")).is_empty());
    }
}
