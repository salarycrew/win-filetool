//! 0.3.3: (Windows) 설치된 Microsoft Office(Word·Excel·PowerPoint)로 문서 → PDF(그리고 doc → docx 같은 Office 형식).
//!
//! 한글 COM 길([`crate::hwp::hancom`])과 같은 규칙을 따른다.
//! - 별도 프로세스: PowerShell이 [`SCRIPT`]를 실행하고 그 안에서 Office 프로그램을 **새로** 하나 띄운다(앱에 새 의존성 없음).
//!   새 프로세스가 생기지 않으면(사용자가 쓰는 PowerPoint에 붙음) 아무것도 하지 않고 그만둔다 — 사용자 문서를 건드리지 않는다.
//! - 원본은 열지 않는다: 작업 임시 폴더에 ASCII 이름(`in.docx`)으로 복사한 것을 읽기 전용으로 열고, 임시 폴더에 저장한 뒤 옮긴다.
//! - 0.3.3(보안 강화): 그 복사본에서 원격 서식·외부 통합 문서 연결을 먼저 뺀다([`crate::ooxml::extref`]).
//! - 보안(문서를 열기 전에): `AutomationSecurity = 3`(msoAutomationSecurityForceDisable — 매크로·AutoOpen·Workbook_Open 안 돎),
//!   알림 끔, 최근 파일에 넣지 않음, 연결 갱신 안 함(Excel `UpdateLinks:=0`, Word `UpdateLinksAtOpen` 잠시 끔), 원본에 저장하지 않음.
//!   암호 문서는 틀린 암호를 주어 묻지 않고 바로 실패.
//! - 한 번에 하나(앱 전체 잠금 파일), 언제나 문서 닫기 + `Quit`, 끝나지 않으면 우리가 띄운 프로세스만 강제 종료, 시간 제한·취소.
//! - 있는지 보기는 싸다: 레지스트리(ProgID → CLSID → LocalServer32)만 읽는다.
//! - 시험용: `WIN_FILETOOL_HIDE_OFFICE=1` 또는 `WIN_FILETOOL_HIDE_COM=1`이면 Office가 없는 것처럼([`crate::testhooks`], 배포 앱은 읽지 않음).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::docs::DocTarget;
use crate::error::{Error, Result};
use crate::process::Cancel;

/// 한 파일 시간 제한(알림 창이 떠서 멈춘 경우 이 시간 뒤 강제 종료)
pub const TIMEOUT: Duration = Duration::from_secs(180);
pub const TIMEOUT_ENV: &str = "WIN_FILETOOL_OFFICE_TIMEOUT_SECS";
pub const SCRIPT: &str = include_str!("msoffice.ps1");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum App {
    Word,
    Excel,
    PowerPoint,
}

impl App {
    pub const ALL: [App; 3] = [App::Word, App::Excel, App::PowerPoint];
    pub fn name(self) -> &'static str {
        match self {
            App::Word => "Word",
            App::Excel => "Excel",
            App::PowerPoint => "PowerPoint",
        }
    }
    pub fn prog_id(self) -> &'static str {
        match self {
            App::Word => "Word.Application",
            App::Excel => "Excel.Application",
            App::PowerPoint => "PowerPoint.Application",
        }
    }
    /// 작업 관리자에 보이는 실행 파일 이름
    pub fn image(self) -> &'static str {
        match self {
            App::Word => "WINWORD.EXE",
            App::Excel => "EXCEL.EXE",
            App::PowerPoint => "POWERPNT.EXE",
        }
    }
    /// 이 프로그램이 Office 형식으로 저장할 때의 대상
    pub fn office_target(self) -> DocTarget {
        match self {
            App::Word => DocTarget::Docx,
            App::Excel => DocTarget::Xlsx,
            App::PowerPoint => DocTarget::Pptx,
        }
    }
}

/// 확장자 → 그 파일을 여는 Office 프로그램. Office가 직접 여는 형식만(Pages·Keynote·Numbers·fodt 등은 LibreOffice).
pub fn app_for_ext(ext: &str) -> Option<App> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "doc" | "docx" | "docm" | "dot" | "dotx" | "dotm" | "rtf" | "odt" | "txt" => App::Word,
        "xls" | "xlsx" | "xlsm" | "xlsb" | "xlt" | "xltx" | "ods" | "csv" => App::Excel,
        "ppt" | "pptx" | "pptm" | "pps" | "ppsx" | "pot" | "potx" | "odp" => App::PowerPoint,
        _ => return None,
    })
}

pub fn app_for(p: &Path) -> Option<App> {
    p.extension().and_then(|e| e.to_str()).and_then(app_for_ext)
}

/// 이 Office 프로그램으로 `target`을 만들 수 있는지(PDF는 언제나, 그 밖에는 자기 형식만)
pub fn can_make(app: App, target: DocTarget) -> bool {
    target == DocTarget::Pdf || target == app.office_target()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Install {
    pub app: App,
    /// WINWORD.EXE 등(LocalServer32)
    pub exe: PathBuf,
    /// 32비트 Office(64비트 Windows의 WOW6432Node에 등록)
    pub wow64: bool,
    /// `Word.Application.16` → "16"
    pub version: Option<String>,
}

/// 설정 화면·점검용 요약
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct Presence {
    pub word: bool,
    pub excel: bool,
    pub powerpoint: bool,
    /// 예: "16"
    pub version: Option<String>,
    /// 64비트 Office인지(찾은 것 기준)
    pub x64: Option<bool>,
}

fn hidden() -> bool {
    crate::testhooks::flag(crate::testhooks::HIDE_OFFICE) || crate::testhooks::flag(crate::testhooks::HIDE_COM)
}

/// 이 프로그램을 자동화로 쓸 수 있는지(숨김 설정 반영)
pub fn available(app: App) -> bool {
    !hidden() && installed(app).is_some()
}

pub fn presence() -> Presence {
    let get = |a| if hidden() { None } else { installed(a) };
    let (w, x, p) = (get(App::Word), get(App::Excel), get(App::PowerPoint));
    let first = w.as_ref().or(x.as_ref()).or(p.as_ref());
    Presence { word: w.is_some(), excel: x.is_some(), powerpoint: p.is_some(), version: first.and_then(|i| i.version.clone()), x64: first.map(|i| !i.wow64) }
}

/// `Word.Application.16` → "16"
pub fn version_from_curver(s: &str) -> Option<String> {
    let v = s.trim().rsplit('.').next()?;
    (!v.is_empty() && v.chars().all(|c| c.is_ascii_digit())).then(|| v.to_string())
}

#[cfg(windows)]
pub fn installed(app: App) -> Option<Install> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<App, Option<Install>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&app) {
        return v.clone();
    }
    let found = (|| {
        use winreg::enums::*;
        use winreg::RegKey;
        let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
        let clsid: String = hkcr.open_subkey(format!(r"{}\CLSID", app.prog_id())).ok()?.get_value("").ok()?;
        let version = hkcr.open_subkey(format!(r"{}\CurVer", app.prog_id())).ok().and_then(|k| k.get_value::<String, _>("").ok()).and_then(|v| version_from_curver(&v));
        for (flag, wow64) in [(KEY_WOW64_64KEY, false), (KEY_WOW64_32KEY, true)] {
            let Ok(k) = hkcr.open_subkey_with_flags(format!(r"CLSID\{clsid}\LocalServer32"), KEY_READ | flag) else { continue };
            let Ok(cmd) = k.get_value::<String, _>("") else { continue };
            let exe = crate::hwp::hancom::exe_from_command(&cmd);
            if exe.is_file() {
                // 64비트 창에서 찾은 exe가 Program Files (x86)이면 32비트 Office
                let x86 = exe.to_string_lossy().to_ascii_lowercase().contains("program files (x86)");
                return Some(Install { app, exe, wow64: wow64 || x86, version });
            }
        }
        None
    })();
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(app, found.clone());
    found
}

#[cfg(not(windows))]
pub fn installed(_app: App) -> Option<Install> {
    None
}

/// 스크립트 출력 해석 결과
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub pids: Vec<u32>,
    pub nopid: bool,
    /// 열기 전에 확인한 AutomationSecurity 값(3이어야 함)
    pub security: Option<u32>,
    pub opened: Option<bool>,
    pub open_error: Option<String>,
    pub pages: Option<u32>,
    pub saved: Option<bool>,
    pub error: Option<String>,
    pub done: bool,
}

pub fn parse_line(o: &mut Outcome, line: &str) {
    let line = line.trim_start_matches('\u{feff}').trim();
    let (k, v) = line.split_once(' ').unwrap_or((line, ""));
    let b = || Some(v.trim().eq_ignore_ascii_case("true"));
    match k {
        "PID" => o.pids.extend(v.split(',').filter_map(|x| x.trim().parse::<u32>().ok())),
        "NOPID" => o.nopid = true,
        "SEC" => o.security = v.trim().parse().ok(),
        "OPEN" => o.opened = b(),
        "OPENERR" => o.open_error = Some(v.trim().to_string()),
        "PAGES" => o.pages = v.trim().parse().ok(),
        "SAVE" => o.saved = b(),
        "ERROR" => o.error = Some(v.trim().to_string()),
        "DONE" => o.done = true,
        _ => {}
    }
}

pub const MSG_ENCRYPTED: &str = "암호가 걸린 문서예요. 원래 프로그램에서 암호를 풀고 다시 넣어 주세요.";

/// Office가 열려 있는 PowerPoint에 붙은 경우의 안내(앱 이름)
pub fn msg_busy(app: App) -> String {
    format!("열려 있는 {}과(와) 겹쳐 바꾸지 못했어요. {} 창을 모두 닫고 다시 해 주세요.", app.name(), app.name())
}

/// 실패 사유 → 사용자 안내(없으면 성공)
pub fn outcome_error(o: &Outcome, app: App, target: DocTarget) -> Option<String> {
    let what = if target == DocTarget::Pdf { "PDF" } else { target.ext() };
    let name = app.name();
    if o.nopid {
        return Some(msg_busy(app));
    }
    if o.security.is_some_and(|s| s != 3) {
        return Some(format!("{name}의 매크로를 끌 수 없어 열지 않았어요."));
    }
    if o.opened == Some(false) {
        let e = o.open_error.as_deref().unwrap_or("").to_lowercase();
        if e.contains("password") || e.contains("암호") {
            return Some(MSG_ENCRYPTED.into());
        }
        let detail = o.open_error.as_deref().map(|d| format!(" ({d})")).unwrap_or_default();
        return Some(format!("{name}이(가) 이 파일을 열지 못했어요. 손상된 파일이거나 {name}이(가) 읽을 수 없는 형식일 수 있어요.{detail}"));
    }
    if let Some(e) = &o.error {
        return Some(format!("{name} 자동화 중 오류가 났어요: {e}"));
    }
    if o.saved != Some(true) {
        return Some(format!("{name}에서 {what}로 저장하지 못했어요."));
    }
    None
}

/// PowerShell 인자
pub fn script_args(script: &Path, input: &Path, output: &Path, app: App, target: DocTarget) -> Vec<OsString> {
    let fmt = if target == DocTarget::Pdf { "PDF" } else { "OOXML" };
    let mut a: Vec<OsString> = ["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"].iter().map(OsString::from).collect();
    a.push(script.into());
    for (k, v) in [("-In", input.as_os_str()), ("-Out", output.as_os_str()), ("-App", app.name().as_ref()), ("-Fmt", fmt.as_ref())] {
        a.push(k.into());
        a.push(v.into());
    }
    a
}

/// OOXML 확장자인데 내용이 OLE 복합 파일이면 암호가 걸린 문서(Office는 암호 문서를 이렇게 싼다)
pub fn looks_encrypted_ooxml(p: &Path) -> bool {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !matches!(ext.as_str(), "docx" | "docm" | "dotx" | "xlsx" | "xlsm" | "xltx" | "pptx" | "pptm" | "ppsx" | "potx") {
        return false;
    }
    let mut head = [0u8; 8];
    use std::io::Read;
    std::fs::File::open(p).and_then(|mut f| f.read_exact(&mut head)).is_ok() && head == [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]
}

#[derive(Debug, Clone, Serialize)]
pub struct OfficeOut {
    pub output: PathBuf,
    pub app: App,
    /// Word는 쪽 수, PowerPoint는 슬라이드 수, Excel은 시트 수
    pub pages: Option<u32>,
    pub millis: u64,
}

#[cfg(windows)]
mod win {
    use super::*;
    use crate::error::io_err;
    use crate::process;
    use crate::tempdir::JobDir;
    use std::process::Stdio;
    use std::time::Instant;

    pub fn pids(app: App) -> Vec<u32> {
        let filter = format!("IMAGENAME eq {}", app.image());
        let out = process::command(Path::new("tasklist")).args(["/FI", filter.as_str(), "/FO", "CSV", "/NH"]).stdin(Stdio::null()).stderr(Stdio::null()).output();
        let Ok(out) = out else { return vec![] };
        let want = format!("\"{}\"", app.image().to_ascii_lowercase());
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.to_ascii_lowercase().starts_with(&want))
            .filter_map(|l| l.split(',').nth(1).and_then(|p| p.trim_matches('"').parse().ok()))
            .collect()
    }

    fn kill(app: App, pid: u32) {
        let pid = pid.to_string();
        let filter = format!("IMAGENAME eq {}", app.image());
        let _ = process::command(Path::new("taskkill"))
            .args(["/F", "/T", "/PID", pid.as_str(), "/FI", filter.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    /// 앱 전체에서 Office 변환은 하나씩(다른 앱 창·감시 폴더와도)
    fn lock(cancel: Option<&Cancel>, timeout: Duration) -> Result<std::fs::File> {
        let dir = crate::bundle::local_app_dir().join("office");
        std::fs::create_dir_all(&dir).map_err(io_err(&dir))?;
        let p = dir.join("office.lock");
        let f = std::fs::File::options().create(true).truncate(false).write(true).open(&p).map_err(io_err(&p))?;
        let t0 = Instant::now();
        loop {
            if f.try_lock().is_ok() {
                return Ok(f);
            }
            if let Some(c) = cancel {
                c.check()?;
            }
            if t0.elapsed() > timeout {
                return Err(Error::Tool { tool: "Office".into(), message: "다른 문서 변환이 끝나지 않아 기다리다 멈췄어요. 잠시 뒤 다시 해 주세요.".into() });
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    pub fn convert(inst: &Install, input: &Path, out_dir: Option<&Path>, target: DocTarget, cancel: Option<&Cancel>, timeout: Duration) -> Result<OfficeOut> {
        let t0 = Instant::now();
        let app = inst.app;
        let _lock = lock(cancel, timeout)?;
        let job = JobDir::new("office")?;
        let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("bin").to_ascii_lowercase();
        let tmp_in = job.path().join(format!("in.{ext}"));
        std::fs::copy(input, &tmp_in).map_err(io_err(input))?;
        // 0.3.3(보안 강화): 원격 서식(attachedTemplate)·외부 통합 문서 연결(externalLink)을 임시 복사본에서 뺀다
        crate::ooxml::extref::strip_external_refs(&tmp_in)?;
        let tmp_out = job.path().join(format!("out.{}", target.ext()));
        let script = job.path().join("msoffice.ps1");
        std::fs::write(&script, format!("\u{feff}{SCRIPT}")).map_err(io_err(&script))?;
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let ps = root.join(if inst.wow64 { "SysWOW64" } else { "System32" }).join(r"WindowsPowerShell\v1.0\powershell.exe");
        let before = pids(app);
        let mut o = Outcome::default();
        let res = process::run(&ps, &script_args(&script, &tmp_in, &tmp_out, app, target), timeout, cancel, &mut |l| parse_line(&mut o, l));
        // 우리가 띄운 것만 정리: 스크립트가 알려 준 번호, 못 알려 줬으면(띄우다 멈춤) 시작 뒤 새로 생긴 것
        let mut ours = o.pids.clone();
        if ours.is_empty() && !o.nopid && res.is_err() {
            ours = pids(app).into_iter().filter(|p| !before.contains(p)).collect();
        }
        if !ours.is_empty() {
            let wait_until = Instant::now() + if res.is_ok() { Duration::from_secs(8) } else { Duration::ZERO };
            loop {
                let alive: Vec<u32> = pids(app).into_iter().filter(|p| ours.contains(p)).collect();
                if alive.is_empty() {
                    break;
                }
                if Instant::now() >= wait_until {
                    alive.into_iter().for_each(|p| kill(app, p));
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        match res {
            Err(Error::Timeout(_)) => {
                return Err(Error::Tool { tool: app.name().into(), message: format!("{}이(가) 응답하지 않아 멈췄어요(시간 초과). 알림 창이 떠 있었을 수 있어요. 다시 해 주세요.", app.name()) })
            }
            Err(e) => return Err(e),
            Ok(_) => {}
        }
        if let Some(m) = outcome_error(&o, app, target) {
            return Err(Error::Tool { tool: app.name().into(), message: m });
        }
        if std::fs::metadata(&tmp_out).map(|m| m.len()).unwrap_or(0) == 0 {
            return Err(Error::Tool { tool: app.name().into(), message: "결과 파일이 생기지 않았어요.".into() });
        }
        let output = crate::paths::output_path(input, out_dir, "", target.ext());
        crate::docs::move_into(&tmp_out, &output)?;
        crate::imageconv::copy_mtime(input, &output);
        Ok(OfficeOut { output, app, pages: o.pages, millis: t0.elapsed().as_millis() as u64 })
    }
}

#[cfg(windows)]
pub use win::pids;

/// Office 변환 중 "다른 Office와 겹침" 오류인지(앱이 문서 도구가 있으면 그쪽으로 넘길 때)
pub fn is_busy_error(e: &Error) -> bool {
    matches!(e, Error::Tool { message, .. } if message.contains("겹쳐 바꾸지 못했어요"))
}

/// 설치된 Office로 한 파일 변환. 결과는 `이름.pdf` 등(겹치면 ` (1)`).
pub fn convert(input: &Path, out_dir: Option<&Path>, target: DocTarget, cancel: Option<&Cancel>, timeout: Duration) -> Result<OfficeOut> {
    if !input.is_file() {
        return Err(Error::Invalid(format!("파일이 없습니다: {}", input.display())));
    }
    crate::error::ensure_nonempty(input)?;
    let app = app_for(input).ok_or_else(|| Error::Unsupported(format!("{} (Office)", input.display())))?;
    if !can_make(app, target) {
        return Err(Error::Unsupported(format!("{} → {}", input.display(), target.ext())));
    }
    if looks_encrypted_ooxml(input) {
        return Err(Error::Tool { tool: app.name().into(), message: MSG_ENCRYPTED.into() });
    }
    let inst = if hidden() { None } else { installed(app) };
    let Some(_inst) = inst else {
        return Err(Error::Invalid(format!("이 PC에서 {}을(를) 찾지 못했어요.", app.name())));
    };
    let timeout = crate::testhooks::var(TIMEOUT_ENV).and_then(|v| v.trim().parse().ok()).map(Duration::from_secs).unwrap_or(timeout.min(TIMEOUT));
    #[cfg(windows)]
    {
        win::convert(&_inst, input, out_dir, target, cancel, timeout)
    }
    #[cfg(not(windows))]
    {
        let _ = (out_dir, cancel, timeout);
        unreachable!("installed()는 Windows에서만 Some")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_by_extension() {
        assert_eq!(app_for(Path::new("a.DOCX")), Some(App::Word));
        assert_eq!(app_for(Path::new("a.doc")), Some(App::Word));
        assert_eq!(app_for(Path::new("a.xls")), Some(App::Excel));
        assert_eq!(app_for(Path::new("a.ppt")), Some(App::PowerPoint));
        assert_eq!(app_for(Path::new("a.odp")), Some(App::PowerPoint));
        for e in ["pages", "key", "numbers", "hwp", "hwpx", "fodt", "wpd", "pdf"] {
            assert_eq!(app_for_ext(e), None, "{e}");
        }
        assert!(can_make(App::Word, DocTarget::Pdf) && can_make(App::Word, DocTarget::Docx) && !can_make(App::Word, DocTarget::Pptx));
        assert_eq!(version_from_curver("Word.Application.16"), Some("16".into()));
        assert_eq!(version_from_curver("Word.Application"), None);
    }

    #[test]
    fn outcome_parsing() {
        let mut o = Outcome::default();
        for l in ["\u{feff}PID 4321", "SEC 3", "OPEN True", "PAGES 7", "SAVE True", "DONE"] {
            parse_line(&mut o, l);
        }
        assert_eq!((o.pids.clone(), o.security, o.pages), (vec![4321], Some(3), Some(7)));
        assert_eq!(outcome_error(&o, App::Word, DocTarget::Pdf), None);
        let mut o = Outcome::default();
        for l in ["PID 1", "SEC 3", "OPENERR 암호가 올바르지 않습니다. 문서를 열 수 없습니다.", "OPEN False", "DONE"] {
            parse_line(&mut o, l);
        }
        assert_eq!(outcome_error(&o, App::Word, DocTarget::Pdf).as_deref(), Some(MSG_ENCRYPTED));
        let mut o = Outcome::default();
        for l in ["PID 1", "SEC 3", "OPENERR The password you supplied is not correct.", "OPEN False"] {
            parse_line(&mut o, l);
        }
        assert_eq!(outcome_error(&o, App::Excel, DocTarget::Pdf).as_deref(), Some(MSG_ENCRYPTED));
        let mut o = Outcome::default();
        parse_line(&mut o, "NOPID");
        let e = outcome_error(&o, App::PowerPoint, DocTarget::Pdf).unwrap();
        assert!(e.contains("PowerPoint 창을 모두 닫고"));
        assert!(is_busy_error(&Error::Tool { tool: "PowerPoint".into(), message: e }));
        // 매크로를 끄지 못했으면 열지 않은 것으로
        let mut o = Outcome::default();
        for l in ["PID 1", "SEC 1"] {
            parse_line(&mut o, l);
        }
        assert!(outcome_error(&o, App::Word, DocTarget::Pdf).unwrap().contains("매크로"));
        // 아무 말 없이 끝나면 실패
        assert!(outcome_error(&Outcome::default(), App::Word, DocTarget::Pdf).is_some());
    }

    #[test]
    fn script_security_rules() {
        // 문서를 열기 전에 매크로를 끄고(AutomationSecurity 3), 읽기 전용·최근 파일 안 넣음·연결 갱신 안 함·사용자 Office는 Quit하지 않음
        let sec = SCRIPT.find("$o.AutomationSecurity = 3").unwrap();
        for open in ["$o.Documents.Open(", "$o.Workbooks.Open(", "$o.Presentations.Open("] {
            assert!(SCRIPT.find(open).unwrap() > sec, "{open}");
        }
        for k in ["$o.Documents.Open($In, $false, $true, $false, $nopass, $nopass, $false)", "$o.Workbooks.Open($In, 0, $true", "$o.Presentations.Open($In, -1, 0, 0)", "'NOPID'", "if ($ours)", "$o.Quit()", "DisplayAlerts", "EnableEvents = $false", "UpdateLinksAtOpen"] {
            assert!(SCRIPT.contains(k), "{k}");
        }
        // 원본 경로에 저장하는 말이 없다(-Out으로만 저장)
        for l in SCRIPT.lines().filter(|l| l.contains("SaveAs") || l.contains("ExportAsFixedFormat")) {
            assert!(l.contains("$Out"), "{l}");
        }
        let a = script_args(Path::new("s.ps1"), Path::new("in.docx"), Path::new("out.pdf"), App::Word, DocTarget::Pdf);
        let a: Vec<String> = a.iter().map(|s| s.to_string_lossy().into_owned()).collect();
        assert!(a.contains(&"-NonInteractive".to_string()) && a.contains(&"Word".to_string()) && a.contains(&"PDF".to_string()));
    }

    #[test]
    fn encrypted_ooxml_sniff() {
        let t = tempfile::tempdir().unwrap();
        let enc = t.path().join("a.docx");
        std::fs::write(&enc, [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0]).unwrap();
        assert!(looks_encrypted_ooxml(&enc));
        let doc = t.path().join("a.doc");
        std::fs::write(&doc, [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0]).unwrap();
        assert!(!looks_encrypted_ooxml(&doc), "옛 .doc는 원래 복합 파일");
        let zip = t.path().join("b.xlsx");
        std::fs::write(&zip, b"PK\x03\x04rest").unwrap();
        assert!(!looks_encrypted_ooxml(&zip));
        #[cfg(not(windows))]
        assert!(!available(App::Word) && presence() == Presence::default());
    }
}
