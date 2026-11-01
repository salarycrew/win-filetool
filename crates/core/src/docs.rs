//! 문서 변환(LibreOffice headless `--convert-to`). 0.3.0부터 줄인 LibreOffice가 앱에 들어 있다([`crate::bundle`]).
//!
//! 앱 전용 프로필(`-env:UserInstallation=file:///…`)을 써서 사용자의 LibreOffice 설정을 건드리지 않는다.
//! 0.3.0: 프로필을 작업마다 새로 만들지 않고 작은 묶음(`lo-profile-1..N`)을 돌려 쓴다(첫 변환 1.9초 → 다음부터 0.5초).
//! 같은 프로필을 두 soffice가 동시에 쓸 수 없으므로 프로필마다 잠금 파일을 잡고, 모두 쓰는 중이면 빌 때까지 기다린다.
//! 재사용한 프로필로 실패하면 프로필이 망가졌을 수 있어 지우고 새 프로필로 한 번 더 해 본다.
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

/// 동시에 돌릴 수 있는 변환 수(프로필 수)
pub const PROFILE_POOL: usize = 3;

/// 프로필 묶음을 둘 폴더: 앱에 든 LibreOffice면 그 버전 폴더(`tools\<버전>-<해시>\`) 안,
/// 아니면 `%LOCALAPPDATA%\kr.salarycrew.winfiletool\lo-profiles\<soffice 경로 해시>\`.
/// 시험용으로 `WIN_FILETOOL_LO_PROFILES`로 바꿀 수 있다.
pub fn profile_base(soffice: &Path) -> PathBuf {
    if let Some(d) = std::env::var_os("WIN_FILETOOL_LO_PROFILES").filter(|v| !v.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(b) = crate::bundle::current() {
        let vd = b.version_dir();
        if soffice.starts_with(&vd) {
            return vd;
        }
    }
    // FNV-1a: 다른 LibreOffice(버전이 다를 수 있음)끼리 프로필을 나눠 쓰지 않게
    let mut h: u64 = 0xcbf29ce484222325;
    for b in soffice.to_string_lossy().to_lowercase().bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    crate::bundle::local_app_dir().join("lo-profiles").join(format!("{h:016x}"))
}

/// 빌려 쓴 프로필. 놓으면(drop) 잠금이 풀린다.
#[derive(Debug)]
pub struct ProfileLease {
    pub dir: PathBuf,
    /// 이미 쓰던 프로필인지(실패하면 새로 만들어 다시 해 볼지 정할 때)
    pub reused: bool,
    _lock: std::fs::File,
}

/// 빈 프로필 하나를 잡는다. 모두 쓰는 중이면 0.2초마다 다시 본다(취소·시간 제한 지킴).
pub fn acquire_profile(base: &Path, pool: usize, cancel: Option<&Cancel>, timeout: Duration) -> Result<ProfileLease> {
    std::fs::create_dir_all(base).map_err(io_err(base))?;
    let t0 = std::time::Instant::now();
    loop {
        for i in 1..=pool.max(1) {
            let lock_path = base.join(format!("lo-profile-{i}.lock"));
            let f = std::fs::File::options().create(true).truncate(false).write(true).open(&lock_path).map_err(io_err(&lock_path))?;
            if f.try_lock().is_ok() {
                let dir = base.join(format!("lo-profile-{i}"));
                let reused = dir.join("user").is_dir();
                return Ok(ProfileLease { dir, reused, _lock: f });
            }
        }
        if let Some(c) = cancel {
            c.check()?;
        }
        if t0.elapsed() > timeout {
            return Err(Error::Tool { tool: "LibreOffice".into(), message: "다른 문서 변환이 끝나지 않아 기다리다 멈췄습니다. 잠시 뒤 다시 해 주세요.".into() });
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// 문서 한 개 변환.
pub fn convert(soffice: &Path, input: &Path, out_dir: Option<&Path>, target: DocTarget, cancel: Option<&Cancel>, timeout: Duration) -> Result<PathBuf> {
    if !input.is_file() {
        return Err(Error::Invalid(format!("파일이 없습니다: {}", input.display())));
    }
    if !allowed_targets(input).contains(&target) {
        return Err(Error::Unsupported(format!("{} → {}", input.display(), target.ext())));
    }
    let job = JobDir::new("lo")?;
    let tmp_out = job.path().join("out");
    std::fs::create_dir_all(&tmp_out).map_err(io_err(&tmp_out))?;
    let lease = acquire_profile(&profile_base(soffice), PROFILE_POOL, cancel, timeout)?;
    // Windows: soffice.exe는 실행기라 먼저 끝날 수 있어, 끝까지 기다리는 soffice.com(콘솔용)으로 실행한다
    let program = crate::tools::soffice_console(soffice);
    let produced = |dir: &Path| -> Option<PathBuf> {
        std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.is_file())
    };
    let mut res = process::run(&program, &build_args(&lease.dir, &tmp_out, input, target), timeout, cancel, &mut |_| {})?;
    if produced(&tmp_out).is_none() && lease.reused {
        // 재사용한 프로필이 망가졌을 수 있다(강제 종료 등): 지우고 새로 한 번 더
        if let Some(c) = cancel {
            c.check()?;
        }
        let _ = std::fs::remove_dir_all(&lease.dir);
        res = process::run(&program, &build_args(&lease.dir, &tmp_out, input, target), timeout, cancel, &mut |_| {})?;
    }
    drop(lease);
    let Some(made) = produced(&tmp_out) else {
        let detail = process::last_lines(&format!("{}\n{}", res.stdout, res.stderr_tail), 3);
        return Err(Error::Tool {
            tool: "LibreOffice".into(),
            message: format!("결과 파일이 생기지 않았습니다(이 형식을 읽지 못했거나 파일이 손상됐을 수 있음). {detail}"),
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
    fn profile_pool_locks() {
        let t = tempfile::tempdir().unwrap();
        let a = acquire_profile(t.path(), 2, None, Duration::from_secs(1)).unwrap();
        let b = acquire_profile(t.path(), 2, None, Duration::from_secs(1)).unwrap();
        assert_ne!(a.dir, b.dir);
        assert!(!a.reused);
        // 둘 다 쓰는 중이면 기다리다 시간 제한
        let t0 = std::time::Instant::now();
        assert!(acquire_profile(t.path(), 2, None, Duration::from_millis(300)).is_err());
        assert!(t0.elapsed() >= Duration::from_millis(300));
        // 하나를 놓으면 그것을 다시 쓴다(쓰던 흔적이 있으면 reused)
        std::fs::create_dir_all(a.dir.join("user")).unwrap();
        let a_dir = a.dir.clone();
        drop(a);
        let c = acquire_profile(t.path(), 2, None, Duration::from_secs(1)).unwrap();
        assert_eq!(c.dir, a_dir);
        assert!(c.reused);
        drop(b);
        // 기다리는 쪽은 풀리는 즉시 잡는다
        let base = t.path().to_path_buf();
        let h = std::thread::spawn(move || {
            let x = acquire_profile(&base, 1, None, Duration::from_secs(5)).unwrap();
            x.dir
        });
        std::thread::sleep(Duration::from_millis(300));
        drop(c);
        assert_eq!(h.join().unwrap(), a_dir);
    }

    #[test]
    fn profile_base_separates_installs() {
        let a = profile_base(Path::new(r"C:\Program Files\LibreOffice\program\soffice.exe"));
        let b = profile_base(Path::new(r"D:\LO\program\soffice.exe"));
        assert_ne!(a, b);
        assert!(a.to_string_lossy().contains("lo-profiles"));
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
