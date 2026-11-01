//! 0.3.2 탐색기 우클릭 메뉴("파일도구" 하위 메뉴).
//!
//! 관리자 권한·코드 서명 없이 되는 **고전 메뉴**를 쓴다: `HKCU\Software\Classes\SystemFileAssociations\.<확장자>\shell\WinFileTool`
//! (폴더는 `HKCU\Software\Classes\Directory\shell\WinFileTool`)에 `MUIVerb`·`SubCommands=""`를 두고, 그 아래 `shell\<할 일>\command`에
//! `"<exe>" --ctx=<할 일> -- "%1"`을 적는다. 확장자마다 맞는 할 일만 넣으므로 PDF를 고르면 PDF 일만 나온다.
//!
//! 보안: exe 경로와 `%1`은 따옴표로 감싸고, `--` 뒤에 오는 인자는 **모두 파일 경로로만** 본다(옵션으로 읽지 않음).
//! 절대 경로가 아니거나 없는 경로는 버린다. 셸(cmd)을 거치지 않고, 키는 HKCU 아래에만 만든다.
//!
//! Windows 11 첫 메뉴(“더 많은 옵션 표시”를 누르기 전)에 올리려면 `IExplorerCommand`를 구현한 COM 서버를
//! **패키지 ID**(MSIX 또는 서명한 sparse package)와 함께 등록해야 한다. 서명 인증서 없이는 패키지를 설치할 수 없어
//! 이 앱은 고전 메뉴만 쓴다(Windows 11에서는 “더 많은 옵션 표시” 아래, Shift+F10·Shift+우클릭으로 바로 보임).
//!
//! 여러 파일을 골라 누르면 탐색기는 파일마다 exe를 한 번씩 실행한다(`MultiSelectModel=Player`라 15개 제한은 없음).
//! 앱은 한 번만 실행되게 해 두었고(single instance), 뒤에 실행된 것은 인자만 앞의 앱에 넘기고 끝난다.
//! 앞의 앱은 마지막 인자가 온 뒤 잠깐(0.7초) 더 기다렸다가 한 묶음으로 처리한다 — [`Collector`].
//!
//! 상태는 레지스트리 자체다(앱·설치 파일이 같은 값을 봄). 사용자가 끄면 설정에 `context_menu_off`를 적어
//! 설치 파일이 새 판으로 바꿀 때 다시 켜지 않게 한다.

use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::error::{Error, Result};

pub const VERB: &str = "WinFileTool";
pub const MENU_TITLE: &str = "파일도구";
pub const CTX_ARG: &str = "--ctx=";

/// 핵심 6개 할 일(화면 id와 같음)
pub const JOBS: [(&str, &str); 6] = [
    ("merge", "PDF 합치기"),
    ("compress", "PDF 용량 줄이기"),
    ("img2pdf", "사진을 PDF 하나로"),
    ("office2pdf", "PDF로 바꾸기"),
    ("photo", "사진 용량 줄이기·JPG로"),
    ("macname", "맥에서 온 이름 고치기"),
];

pub const PDF_EXT: &[&str] = &["pdf"];
pub const IMAGE_EXT: &[&str] = &["jpg", "jpeg", "png", "heic", "heif", "webp", "bmp", "gif", "tif", "tiff"];
pub const OFFICE_EXT: &[&str] = &["hwp", "hwpx", "doc", "docx", "rtf", "odt", "xls", "xlsx", "ods", "ppt", "pptx", "odp"];
pub const ZIP_EXT: &[&str] = &["zip"];

/// 메뉴 한 곳(확장자 하나 또는 폴더)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuSpot {
    /// `HKCU\` 아래 키(…\shell\WinFileTool 까지)
    pub key: String,
    /// 넣을 할 일(화면 id, 메뉴 글자)
    pub items: Vec<(&'static str, &'static str)>,
}

/// 메뉴 한 줄(할 일 id, 고른 파일에 맞춘 글자 — 사진이면 "PDF로 합치기")
fn item(id: &'static str, label: &'static str) -> (&'static str, &'static str) {
    debug_assert!(JOBS.iter().any(|(j, _)| *j == id));
    (id, label)
}

/// 등록할 곳 전부(확장자별 + 폴더)
pub fn spots() -> Vec<MenuSpot> {
    let mut v = Vec::new();
    let ext_key = |e: &str| format!(r"Software\Classes\SystemFileAssociations\.{e}\shell\{VERB}");
    for e in PDF_EXT {
        v.push(MenuSpot { key: ext_key(e), items: vec![item("merge", "PDF 합치기"), item("compress", "PDF 용량 줄이기")] });
    }
    for e in IMAGE_EXT {
        v.push(MenuSpot { key: ext_key(e), items: vec![item("img2pdf", "PDF로 합치기"), item("photo", "용량 줄이기·JPG로")] });
    }
    for e in OFFICE_EXT {
        v.push(MenuSpot { key: ext_key(e), items: vec![item("office2pdf", "PDF로 바꾸기")] });
    }
    for e in ZIP_EXT {
        v.push(MenuSpot { key: ext_key(e), items: vec![item("macname", "맥에서 온 이름 고치기")] });
    }
    v.push(MenuSpot { key: format!(r"Software\Classes\Directory\shell\{VERB}"), items: vec![item("macname", "맥에서 온 이름 고치기")] });
    v
}

/// 하위 메뉴 키 이름(순서를 지키려고 앞에 번호): `0merge`, `1compress`
pub fn sub_key(i: usize, job: &str) -> String {
    format!("{i}{job}")
}

/// 상태를 볼 때 읽는 값: 첫 자리(.pdf)의 첫 하위 명령 `…\.pdf\shell\WinFileTool\shell\0merge\command`
pub fn probe_key() -> String {
    let spot = &spots()[0];
    format!(r"{}\shell\{}\command", spot.key, sub_key(0, spot.items[0].0))
}

/// 하위 명령 줄: `"C:\…\win-filetool.exe" --ctx=merge -- "%1"`
pub fn command_line(exe: &Path, job: &str) -> String {
    format!("\"{}\" {CTX_ARG}{job} -- \"%1\"", exe.display())
}

/// 명령 줄에서 exe 경로 꺼내기
pub fn exe_of_command(v: &str) -> Option<String> {
    crate::autostart::exe_of_value(v)
}

/// 인자에서 우클릭 할 일과 파일들: `--ctx=<할 일>` 다음 `--`, 그 뒤 인자는 모두 경로(옵션으로 읽지 않음).
/// 할 일을 모르거나 `--`가 없으면 None. 경로가 실제로 있는지는 [`existing_paths`]로 따로 거른다.
pub fn parse_args<I: IntoIterator<Item = S>, S: AsRef<str>>(args: I) -> Option<(String, Vec<String>)> {
    let args: Vec<String> = args.into_iter().skip(1).map(|a| a.as_ref().to_string()).collect();
    let ci = args.iter().position(|a| a.starts_with(CTX_ARG))?;
    let job = args[ci].strip_prefix(CTX_ARG)?.to_string();
    if !JOBS.iter().any(|(id, _)| *id == job) {
        return None;
    }
    let sep = args.iter().skip(ci + 1).position(|a| a == "--")? + ci + 1;
    let files = args[sep + 1..].iter().filter(|a| !a.is_empty()).cloned().collect();
    Some((job, files))
}

/// 절대 경로이고 실제로 있는 것만(없는 경로·상대 경로·이상한 인자는 버림)
pub fn existing_paths(files: Vec<String>) -> Vec<String> {
    files.into_iter().filter(|f| { let p = Path::new(f); p.is_absolute() && p.exists() }).collect()
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CtxStatus {
    /// 메뉴가 등록돼 있는지(PDF 자리 기준)
    pub enabled: bool,
    /// 등록된 exe가 지금 실행 중인 exe와 같은지(휴대용 exe를 옮겼으면 false → "다시 연결")
    pub matches_exe: bool,
    /// 등록된 exe 경로
    pub registered_exe: Option<String>,
    pub supported: bool,
}

pub fn status_from(cmd: Option<String>, exe: &Path) -> CtxStatus {
    let registered_exe = cmd.as_deref().and_then(exe_of_command);
    let matches_exe = registered_exe.as_deref().map(|p| p.eq_ignore_ascii_case(&exe.display().to_string())).unwrap_or(false);
    CtxStatus { enabled: cmd.is_some(), matches_exe, registered_exe, supported: cfg!(windows) }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    use winreg::RegKey;

    fn reg_err(e: std::io::Error) -> Error {
        Error::Invalid(format!("레지스트리(HKCU\\Software\\Classes)를 바꾸지 못했습니다: {e}"))
    }

    pub fn read_command() -> Option<String> {
        RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(probe_key(), KEY_READ).ok()?.get_value::<String, _>("").ok()
    }

    pub fn install(exe: &Path) -> Result<()> {
        let hk = RegKey::predef(HKEY_CURRENT_USER);
        let icon = format!("\"{}\",0", exe.display());
        for spot in spots() {
            // 예전 항목이 남아 있으면 지우고 새로(할 일 목록이 바뀌었을 때)
            let _ = hk.delete_subkey_all(&spot.key);
            let (k, _) = hk.create_subkey(&spot.key).map_err(reg_err)?;
            k.set_value("MUIVerb", &MENU_TITLE).map_err(reg_err)?;
            k.set_value("SubCommands", &"").map_err(reg_err)?;
            k.set_value("Icon", &icon).map_err(reg_err)?;
            for (i, (job, label)) in spot.items.iter().enumerate() {
                let (s, _) = k.create_subkey(format!(r"shell\{}", sub_key(i, job))).map_err(reg_err)?;
                s.set_value("MUIVerb", label).map_err(reg_err)?;
                s.set_value("MultiSelectModel", &"Player").map_err(reg_err)?;
                let (c, _) = s.create_subkey("command").map_err(reg_err)?;
                c.set_value("", &command_line(exe, job)).map_err(reg_err)?;
            }
        }
        notify();
        Ok(())
    }

    pub fn remove() -> Result<()> {
        let hk = RegKey::predef(HKEY_CURRENT_USER);
        let mut first_err = None;
        for spot in spots() {
            match hk.delete_subkey_all(&spot.key) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        notify();
        match first_err {
            Some(e) => Err(reg_err(e)),
            None => Ok(()),
        }
    }

    fn notify() {
        use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, std::ptr::null(), std::ptr::null()) };
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub fn read_command() -> Option<String> {
        None
    }
    pub fn install(_exe: &Path) -> Result<()> {
        Err(Error::Unsupported("우클릭 메뉴는 Windows에서만 됩니다".into()))
    }
    pub fn remove() -> Result<()> {
        Ok(())
    }
}

/// 등록된 첫 하위 명령(`…\.pdf\shell\WinFileTool\shell\0merge\command`)의 값
fn read_command() -> Option<String> {
    imp::read_command()
}

pub fn status(exe: &Path) -> CtxStatus {
    status_from(read_command(), exe)
}

/// 켜기(지금 exe로 다시 적음 — "다시 연결"도 이것) / 끄기
pub fn set(enabled: bool, exe: &Path) -> Result<CtxStatus> {
    if enabled {
        imp::install(exe)?;
    } else {
        imp::remove()?;
    }
    Ok(status(exe))
}

// ---------- 여러 파일을 한 묶음으로 ----------

/// 우클릭으로 온 인자 모으기: 마지막으로 온 뒤 `quiet`만큼 조용하면 한 묶음.
#[derive(Debug, Default)]
pub struct Collector {
    pending: Vec<(String, Vec<String>)>,
    last: Option<Instant>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CtxBatch {
    pub job: String,
    pub paths: Vec<String>,
}

pub const QUIET: Duration = Duration::from_millis(700);

impl Collector {
    pub fn push(&mut self, job: String, files: Vec<String>, now: Instant) {
        self.pending.push((job, files));
        self.last = Some(now);
    }
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
    /// 모인 것이 있고 마지막으로 온 뒤 `quiet`가 지났는지(꺼내지는 않음)
    pub fn is_quiet(&self, now: Instant, quiet: Duration) -> bool {
        !self.pending.is_empty() && self.last.map(|l| now.duration_since(l) >= quiet).unwrap_or(false)
    }
    /// 조용해졌으면 할 일별로 묶어 꺼낸다(온 순서 유지, 같은 파일은 한 번). 아직이면 None.
    pub fn take_if_quiet(&mut self, now: Instant, quiet: Duration) -> Option<Vec<CtxBatch>> {
        let last = self.last?;
        if now.duration_since(last) < quiet {
            return None;
        }
        Some(self.take())
    }
    pub fn take(&mut self) -> Vec<CtxBatch> {
        let mut out: Vec<CtxBatch> = Vec::new();
        for (job, files) in self.pending.drain(..) {
            let b = match out.iter_mut().position(|b| b.job == job) {
                Some(i) => &mut out[i],
                None => {
                    out.push(CtxBatch { job, paths: Vec::new() });
                    out.last_mut().unwrap()
                }
            };
            for f in files {
                if !b.paths.contains(&f) {
                    b.paths.push(f);
                }
            }
        }
        self.last = None;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_cover_core_jobs_per_type() {
        let s = spots();
        let pdf = s.iter().find(|x| x.key.contains(r"\.pdf\")).unwrap();
        assert_eq!(pdf.items.iter().map(|i| i.0).collect::<Vec<_>>(), ["merge", "compress"]);
        let heic = s.iter().find(|x| x.key.contains(r"\.heic\")).unwrap();
        assert_eq!(heic.items.iter().map(|i| i.0).collect::<Vec<_>>(), ["img2pdf", "photo"]);
        let hwp = s.iter().find(|x| x.key.contains(r"\.hwp\")).unwrap();
        assert_eq!(hwp.items[0].0, "office2pdf");
        let dir = s.iter().find(|x| x.key.contains(r"Directory\shell")).unwrap();
        assert_eq!(dir.items[0].0, "macname");
        assert!(s.iter().all(|x| x.key.starts_with(r"Software\Classes\") && x.key.ends_with(r"\shell\WinFileTool")));
        // 상태를 읽는 키 = 등록하는 키(0.3.2 시험에서 어긋났던 것)
        assert_eq!(probe_key(), r"Software\Classes\SystemFileAssociations\.pdf\shell\WinFileTool\shell\0merge\command");
        // 모든 할 일이 어딘가에 있음
        for (j, _) in JOBS {
            assert!(s.iter().any(|x| x.items.iter().any(|i| i.0 == j)), "{j}");
        }
    }

    #[test]
    fn installer_removes_every_spot() {
        // 설치 파일(NSIS)이 지우는 키 목록이 앱이 만드는 곳과 같아야 지운 뒤 메뉴가 남지 않는다
        let nsh = include_str!("../../../src-tauri/windows/installer-hooks.nsh");
        for spot in spots() {
            assert!(nsh.contains(&format!("DeleteRegKey HKCU \"{}\"", spot.key)), "{}", spot.key);
        }
        assert!(nsh.contains("--ctx-menu=install") && nsh.contains("--ctx-menu=remove"));
    }

    #[test]
    fn command_and_args_roundtrip() {
        let exe = Path::new(r"C:\Users\u\AppData\Local\파일 도구\win-filetool.exe");
        let c = command_line(exe, "merge");
        assert_eq!(c, "\"C:\\Users\\u\\AppData\\Local\\파일 도구\\win-filetool.exe\" --ctx=merge -- \"%1\"");
        assert_eq!(exe_of_command(&c).as_deref(), Some(r"C:\Users\u\AppData\Local\파일 도구\win-filetool.exe"));
        let st = status_from(Some(c.clone()), exe);
        assert!(st.enabled && st.matches_exe);
        let moved = status_from(Some(c), Path::new(r"D:\usb\win-filetool.exe"));
        assert!(moved.enabled && !moved.matches_exe);
        assert!(!status_from(None, exe).enabled);
        assert_eq!(
            parse_args(["app.exe", "--ctx=merge", "--", r"C:\a b\1.pdf"]),
            Some(("merge".to_string(), vec![r"C:\a b\1.pdf".to_string()]))
        );
        // `--` 뒤는 옵션처럼 보여도 경로로만
        assert_eq!(
            parse_args(["app.exe", "--ctx=photo", "--", "--autostart", "-x.jpg", "--tab=search"]),
            Some(("photo".to_string(), vec!["--autostart".to_string(), "-x.jpg".to_string(), "--tab=search".to_string()]))
        );
        assert_eq!(parse_args(["app.exe", "--ctx=merge", r"C:\1.pdf"]), None, "구분자 없으면 받지 않음");
        assert!(!crate::autostart::is_autostart_launch(["app.exe", "--ctx=photo", "--", "--autostart"]), "경로를 옵션으로 읽지 않음");
        assert!(crate::autostart::is_autostart_launch(["app.exe", "--autostart"]));
        assert_eq!(parse_args(["app.exe", "--ctx=nope", "--", "x"]), None);
        assert_eq!(parse_args(["app.exe", "--autostart"]), None);
    }

    #[test]
    fn only_existing_absolute_paths() {
        let t = tempfile::tempdir().unwrap();
        let odd = t.path().join("-가 & 나 '다' 100% ^라! .pdf");
        std::fs::write(&odd, b"%PDF").unwrap();
        let ok = odd.to_string_lossy().into_owned();
        let got = existing_paths(vec![ok.clone(), "--autostart".into(), "relative.pdf".into(), t.path().join("없음.pdf").to_string_lossy().into_owned(), String::new()]);
        assert_eq!(got, vec![ok]);
    }

    #[test]
    fn collector_waits_for_quiet_and_groups() {
        let t0 = Instant::now();
        let mut c = Collector::default();
        assert!(c.take_if_quiet(t0, QUIET).is_none());
        c.push("merge".into(), vec!["a.pdf".into()], t0);
        c.push("merge".into(), vec!["b.pdf".into()], t0 + Duration::from_millis(300));
        c.push("merge".into(), vec!["a.pdf".into()], t0 + Duration::from_millis(400));
        c.push("photo".into(), vec!["c.jpg".into()], t0 + Duration::from_millis(500));
        assert!(c.take_if_quiet(t0 + Duration::from_millis(900), QUIET).is_none());
        assert!(!c.is_quiet(t0 + Duration::from_millis(900), QUIET));
        assert!(c.is_quiet(t0 + Duration::from_millis(1300), QUIET));
        let b = c.take_if_quiet(t0 + Duration::from_millis(1300), QUIET).unwrap();
        assert_eq!(b, vec![
            CtxBatch { job: "merge".into(), paths: vec!["a.pdf".into(), "b.pdf".into()] },
            CtxBatch { job: "photo".into(), paths: vec!["c.jpg".into()] },
        ]);
        assert!(c.is_empty());
    }
}
