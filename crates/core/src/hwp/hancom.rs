//! (Windows) 설치된 한글 프로그램으로 PDF·DOCX 만들기 — 한컴오피스의 COM 자동화(`HWPFrame.HwpObject`).
//!
//! - 별도 프로세스: 앱이 PowerShell(한글과 같은 32/64비트)을 띄워 [`SCRIPT`]를 실행하고, 그 안에서 한글(Hwp.exe,
//!   out-of-process COM 서버)을 하나 새로 띄운다. 앱에는 새 의존성이 없다(GPL 없음).
//! - 원본은 열지 않는다: 작업 임시 폴더에 ASCII 이름(`in.hwp`)으로 복사한 것을 열고, 임시 폴더에 저장한 뒤 옮긴다.
//! - 대화상자 없음: 한글은 자동화로 파일을 열 때 "파일 접근 허용" 보안 창을 띄운다. 한컴이 안내하는 방법은
//!   보안 모듈 DLL(`IsAccessiblePath`를 내보냄)을 `HKCU\Software\HNC\HwpAutomation\Modules`에 등록하고
//!   `RegisterModule("FilePathCheckDLL", 이름)`을 부르는 것이다. 한컴 예제 DLL은 재배포하지 않고, 같은 일을 하는
//!   아주 작은 32비트 DLL(항상 허용)을 [`path_check_dll`]이 그 자리에서 만든다(소스 = 이 파일). 등록 이름은 앱 전용
//!   (`WinFileToolPathCheck`)이라 다른 프로그램이 일부러 부르지 않는 한 쓰이지 않는다. 같은 사용자 권한의 프로그램은
//!   이 레지스트리를 원래 직접 쓸 수 있으므로 새로 열리는 권한은 없다. 메시지 상자는 `SetMessageBoxMode`로 자동 응답,
//!   창은 숨김, 암호 문서는 `suspendpassword`로 묻지 않고 실패.
//! - 0.3.3 매크로: 여는 복사본에서 문서 스크립트(`Scripts`)를 먼저 뺀다([`super::scrub`]) — 자동 응답이 "실행할까요?"에 예라고 해도 돌 것이 없다.
//! - 한 번에 하나(앱 전체 잠금 파일), 언제나 `Quit` + 끝나지 않으면 그 한글 프로세스만 강제 종료, 시간 제한·취소.
//! - 시험용: `WIN_FILETOOL_HIDE_HANCOM=1`(또는 `WIN_FILETOOL_HIDE_COM=1`)이면 한글이 없는 것처럼 동작한다.
//!   0.3.3부터 배포 앱은 이 값을 읽지 않는다([`crate::testhooks`] — CLI·시험용 앱만).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::docs::DocTarget;
use crate::error::{Error, Result};
use crate::process::Cancel;

/// 한글 한 파일 변환 시간 제한(창이 떠서 멈춘 경우 이 시간 뒤 강제 종료)
pub const TIMEOUT: Duration = Duration::from_secs(180);
/// 이 값이 있으면(0 제외) 한글이 없는 것처럼
pub const HIDE_ENV: &str = "WIN_FILETOOL_HIDE_HANCOM";
/// 점검용: 시간 제한(초)을 줄여 멈춤 → 강제 종료 길을 시험
pub const TIMEOUT_ENV: &str = "WIN_FILETOOL_HANCOM_TIMEOUT_SECS";
/// `HKCU\Software\HNC\HwpAutomation\Modules`의 값 이름
pub const MODULE_NAME: &str = "WinFileToolPathCheck";
pub const MODULE_KEY: &str = r"Software\HNC\HwpAutomation\Modules";
pub const PROG_ID: &str = "HWPFrame.HwpObject";
/// PowerShell에서 실행하는 자동화 스크립트
pub const SCRIPT: &str = include_str!("hancom.ps1");

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Install {
    /// Hwp.exe 경로(LocalServer32)
    pub exe: PathBuf,
    /// 32비트 한글(64비트 Windows의 WOW6432Node에 등록)
    pub wow64: bool,
}

fn hidden() -> bool {
    crate::testhooks::flag(HIDE_ENV) || crate::testhooks::flag(crate::testhooks::HIDE_COM)
}

/// 한글이 설치되어 자동화를 쓸 수 있는지(숨김 설정 반영)
pub fn available() -> bool {
    !hidden() && installed().is_some()
}

/// LocalServer32 값(`"C:\…\Hwp.exe" -Automation`) → exe 경로
pub fn exe_from_command(s: &str) -> PathBuf {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('"') {
        return PathBuf::from(rest.split('"').next().unwrap_or(""));
    }
    let lower = s.to_ascii_lowercase();
    match lower.find(".exe") {
        Some(i) => PathBuf::from(&s[..i + 4]),
        None => PathBuf::from(s.split_whitespace().next().unwrap_or("")),
    }
}

#[cfg(windows)]
pub fn installed() -> Option<Install> {
    static CACHE: std::sync::OnceLock<Option<Install>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            use winreg::enums::*;
            use winreg::RegKey;
            let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
            let clsid: String = hkcr.open_subkey(format!(r"{PROG_ID}\CLSID")).ok()?.get_value("").ok()?;
            let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
            for (flag, wow64) in [(KEY_WOW64_64KEY, false), (KEY_WOW64_32KEY, true)] {
                let Ok(k) = hklm.open_subkey_with_flags(format!(r"SOFTWARE\Classes\CLSID\{clsid}\LocalServer32"), KEY_READ | flag) else { continue };
                let Ok(cmd) = k.get_value::<String, _>("") else { continue };
                let exe = exe_from_command(&cmd);
                if exe.is_file() {
                    return Some(Install { exe, wow64 });
                }
            }
            None
        })
        .clone()
}

#[cfg(not(windows))]
pub fn installed() -> Option<Install> {
    None
}

/// 보안 모듈 DLL(PE32, x86): `IsAccessiblePath`(stdcall, 인자 16바이트)가 언제나 1(허용)을 돌려준다.
/// 한컴 예제 모듈과 같은 내보내기·호출 규약이다. DllMain은 1을 돌려줄 뿐 아무것도 하지 않는다. 가져오는 함수·재배치 없음.
pub fn path_check_dll() -> Vec<u8> {
    const SEC_RVA: u32 = 0x1000;
    const FA: u32 = 0x200;
    const BASE: u32 = 0x6F3C_0000;
    let le16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
    let le32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    // .text: DllMain(mov eax,1; ret 0Ch) + IsAccessiblePath(mov eax,1; ret 10h) + 내보내기 표
    let mut sec: Vec<u8> = vec![0xB8, 1, 0, 0, 0, 0xC2, 0x0C, 0x00, 0xB8, 1, 0, 0, 0, 0xC2, 0x10, 0x00];
    let exp = sec.len() as u32;
    let eat = exp + 40;
    let names = eat + 4;
    let ords = names + 4;
    let fname = ords + 4;
    let func = b"IsAccessiblePath\0";
    let dname = fname + func.len() as u32;
    let dll = b"WinFileToolHwpPath.dll\0";
    let end = dname + dll.len() as u32;
    le32(&mut sec, 0); // Characteristics
    le32(&mut sec, 0); // TimeDateStamp
    le16(&mut sec, 0);
    le16(&mut sec, 0);
    le32(&mut sec, SEC_RVA + dname);
    le32(&mut sec, 1); // Base
    le32(&mut sec, 1); // NumberOfFunctions
    le32(&mut sec, 1); // NumberOfNames
    le32(&mut sec, SEC_RVA + eat);
    le32(&mut sec, SEC_RVA + names);
    le32(&mut sec, SEC_RVA + ords);
    le32(&mut sec, SEC_RVA + 8); // IsAccessiblePath
    le32(&mut sec, SEC_RVA + fname);
    le16(&mut sec, 0);
    le16(&mut sec, 0);
    sec.extend_from_slice(func);
    sec.extend_from_slice(dll);
    debug_assert_eq!(sec.len() as u32, end);
    sec.resize(FA as usize, 0);

    let mut f = vec![0u8; 0x40];
    f[0] = b'M';
    f[1] = b'Z';
    f[0x3c] = 0x40; // e_lfanew
    f.extend_from_slice(b"PE\0\0");
    // COFF: i386, 구역 1, 선택 머리 0xE0, EXECUTABLE|32BIT|DLL
    le16(&mut f, 0x14c);
    le16(&mut f, 1);
    le32(&mut f, 0);
    le32(&mut f, 0);
    le32(&mut f, 0);
    le16(&mut f, 0xE0);
    le16(&mut f, 0x2102);
    // 선택 머리(PE32)
    le16(&mut f, 0x10b);
    f.extend_from_slice(&[0, 0]);
    for v in [FA, 0, 0, SEC_RVA /* 진입점 = DllMain */, SEC_RVA, SEC_RVA, BASE, 0x1000, FA] {
        le32(&mut f, v);
    }
    for v in [6u16, 0, 0, 0, 6, 0] {
        le16(&mut f, v);
    }
    for v in [0u32, 0x2000 /* SizeOfImage */, FA /* SizeOfHeaders */, 0] {
        le32(&mut f, v);
    }
    le16(&mut f, 2); // GUI
    le16(&mut f, 0x0500); // NX_COMPAT | NO_SEH
    for v in [0x10_0000u32, 0x1000, 0x10_0000, 0x1000, 0, 16] {
        le32(&mut f, v);
    }
    for i in 0..16 {
        if i == 0 {
            le32(&mut f, SEC_RVA + exp);
            le32(&mut f, end - exp);
        } else {
            le32(&mut f, 0);
            le32(&mut f, 0);
        }
    }
    // 구역 머리 .text
    f.extend_from_slice(b".text\0\0\0");
    for v in [FA, SEC_RVA, FA, FA, 0, 0] {
        le32(&mut f, v);
    }
    le16(&mut f, 0);
    le16(&mut f, 0);
    le32(&mut f, 0x6000_0020); // CODE | EXECUTE | READ
    f.resize(FA as usize, 0);
    f.extend_from_slice(&sec);
    f
}

/// 스크립트 출력 해석 결과
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub pids: Vec<u32>,
    pub nopid: bool,
    pub registered: Option<bool>,
    pub opened: Option<bool>,
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
        "REG" => o.registered = b(),
        "OPEN" => o.opened = b(),
        "PAGES" => o.pages = v.trim().parse().ok(),
        "SAVE" => o.saved = b(),
        "ERROR" => o.error = Some(v.trim().to_string()),
        "DONE" => o.done = true,
        _ => {}
    }
}

/// 한글로 만드는 방법
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 다른 이름으로 저장(PDF·DOCX)
    Save,
    /// 0.3.1: PDF로 인쇄(배포용 문서 — 저장은 막혀 있음)
    PrintPdf,
}

/// 실패 사유 → 사용자 안내(없으면 성공)
pub fn outcome_error(o: &Outcome, target: DocTarget) -> Option<String> {
    outcome_error_mode(o, target, Mode::Save)
}

pub fn outcome_error_mode(o: &Outcome, target: DocTarget, mode: Mode) -> Option<String> {
    if mode == Mode::PrintPdf && o.opened == Some(true) && o.saved == Some(false) {
        return Some(super::MSG_DISTRIBUTION_NOPRINT.into());
    }
    let what = if target == DocTarget::Pdf { "PDF" } else { "Word(DOCX)" };
    if o.nopid {
        return Some("이미 실행 중인 한글과 겹쳐 바꾸지 못했어요. 열려 있는 한글 창을 모두 닫고 다시 해 주세요.".into());
    }
    if let Some(e) = &o.error {
        return Some(format!("한글 자동화 중 오류가 났어요: {e}"));
    }
    if o.registered == Some(false) {
        return Some("한글 자동화 보안 모듈을 등록하지 못했어요.".into());
    }
    if o.opened == Some(false) {
        return Some("한글이 이 파일을 열지 못했어요. 손상된 파일이거나 암호가 걸린 한글 파일일 수 있어요. 암호가 있으면 한글에서 풀고 다시 넣어 주세요.".into());
    }
    if o.saved == Some(false) {
        return Some(format!("한글에서 {what}로 저장하지 못했어요."));
    }
    if o.saved != Some(true) {
        return Some("한글이 작업을 끝내지 못했어요.".into());
    }
    None
}

/// PowerShell 인자
pub fn script_args(script: &Path, input: &Path, output: &Path, target: DocTarget) -> Vec<OsString> {
    script_args_mode(script, input, output, target, Mode::Save)
}

pub fn script_args_mode(script: &Path, input: &Path, output: &Path, target: DocTarget, mode: Mode) -> Vec<OsString> {
    let fmt = match (mode, target) {
        (Mode::PrintPdf, _) => "PRINTPDF",
        (Mode::Save, DocTarget::Pdf) => "PDF",
        _ => "OOXML",
    };
    let mut a: Vec<OsString> = ["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"].iter().map(OsString::from).collect();
    a.push(script.into());
    for (k, v) in [("-In", input.as_os_str()), ("-Out", output.as_os_str()), ("-Fmt", fmt.as_ref()), ("-Module", MODULE_NAME.as_ref())] {
        a.push(k.into());
        a.push(v.into());
    }
    a
}

#[derive(Debug, Clone, Serialize)]
pub struct HancomOut {
    pub output: PathBuf,
    /// 한글이 센 쪽수
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

    fn hwp_dir() -> PathBuf {
        crate::bundle::local_app_dir().join("hwp")
    }

    /// 변환하는 동안만 HKCU에 보안 모듈을 등록해 둔다. 끝나면(성공·실패·취소 모두) 값을 지운다(0.3.0 출시 전 수정).
    /// 값 이름은 이 앱 전용이라 지워도 다른 프로그램에 영향이 없다. 키(`…\Modules`)는 다른 프로그램도 쓰므로 남긴다.
    struct ModuleReg;

    impl Drop for ModuleReg {
        fn drop(&mut self) {
            remove_module_value();
        }
    }

    /// 앱이 남긴 등록값 지우기(없으면 아무것도 안 함)
    pub fn remove_module_value() {
        use winreg::enums::*;
        use winreg::RegKey;
        if let Ok(k) = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(MODULE_KEY, KEY_READ | KEY_SET_VALUE) {
            if k.get_raw_value(MODULE_NAME).is_ok() {
                let _ = k.delete_value(MODULE_NAME);
            }
        }
    }

    /// 등록돼 있는지(시험·점검용)
    pub fn module_registered() -> bool {
        use winreg::enums::*;
        use winreg::RegKey;
        RegKey::predef(HKEY_CURRENT_USER).open_subkey(MODULE_KEY).and_then(|k| k.get_raw_value(MODULE_NAME)).is_ok()
    }

    /// 보안 모듈 DLL을 쓰고(내용별 이름이라 쓰는 중인 파일을 덮지 않음) HKCU에 등록. 돌려준 값을 버리면 등록을 지운다.
    fn ensure_module() -> Result<(PathBuf, ModuleReg)> {
        use winreg::enums::*;
        use winreg::RegKey;
        let bytes = path_check_dll();
        let dir = hwp_dir();
        std::fs::create_dir_all(&dir).map_err(io_err(&dir))?;
        let dll = dir.join(format!("WinFileToolHwpPath-{:016x}.dll", crate::ooxml::pptx::fnv(&bytes)));
        if std::fs::read(&dll).map_or(true, |b| b != bytes) {
            let tmp = dir.join(format!("dll-{}.tmp", std::process::id()));
            std::fs::write(&tmp, &bytes).map_err(io_err(&tmp))?;
            let _ = std::fs::remove_file(&dll);
            std::fs::rename(&tmp, &dll).map_err(io_err(&dll))?;
        }
        let (k, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(MODULE_KEY).map_err(|e| Error::Tool { tool: "한글".into(), message: format!("보안 모듈을 등록하지 못했어요: {e}") })?;
        let want = dll.to_string_lossy().into_owned();
        if k.get_value::<String, _>(MODULE_NAME).ok().as_deref() != Some(want.as_str()) {
            k.set_value(MODULE_NAME, &want).map_err(|e| Error::Tool { tool: "한글".into(), message: format!("보안 모듈을 등록하지 못했어요: {e}") })?;
        }
        Ok((dll, ModuleReg))
    }

    /// 지금 떠 있는 Hwp.exe 프로세스 번호
    pub fn hwp_pids() -> Vec<u32> {
        let out = process::command(Path::new("tasklist")).args(["/FI", "IMAGENAME eq Hwp.exe", "/FO", "CSV", "/NH"]).stdin(Stdio::null()).stderr(Stdio::null()).output();
        let Ok(out) = out else { return vec![] };
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.to_ascii_lowercase().starts_with("\"hwp.exe\""))
            .filter_map(|l| l.split(',').nth(1).and_then(|p| p.trim_matches('"').parse().ok()))
            .collect()
    }

    fn kill(pid: u32) {
        let pid = pid.to_string();
        let _ = process::command(Path::new("taskkill"))
            .args(["/F", "/T", "/PID", pid.as_str(), "/FI", "IMAGENAME eq Hwp.exe"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    /// 앱 전체에서 한글 변환은 하나씩(다른 앱 창·감시 폴더와도)
    fn lock(cancel: Option<&Cancel>, timeout: Duration) -> Result<std::fs::File> {
        let dir = hwp_dir();
        std::fs::create_dir_all(&dir).map_err(io_err(&dir))?;
        let p = dir.join("hancom.lock");
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
                return Err(Error::Tool { tool: "한글".into(), message: "다른 한글 변환이 끝나지 않아 기다리다 멈췄어요. 잠시 뒤 다시 해 주세요.".into() });
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    pub fn convert(inst: &Install, input: &Path, out_dir: Option<&Path>, target: DocTarget, mode: Mode, cancel: Option<&Cancel>, timeout: Duration) -> Result<HancomOut> {
        let t0 = Instant::now();
        let _lock = lock(cancel, timeout)?;
        // 잠금보다 먼저 풀리도록 뒤에 선언(변환이 끝나면 등록값을 지운 다음 잠금을 놓음)
        let (_dll, _reg) = ensure_module()?;
        let job = JobDir::new("hwp")?;
        let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("hwp").to_ascii_lowercase();
        let tmp_in = job.path().join(format!("in.{ext}"));
        std::fs::copy(input, &tmp_in).map_err(io_err(input))?;
        // 0.3.3: 한글에는 매크로 끄기 설정이 없어, 여는 복사본에서 문서 스크립트를 뺀다(원본은 그대로)
        crate::hwp::scrub::strip_scripts(&tmp_in)?;
        let tmp_out = job.path().join(format!("out.{}", target.ext()));
        let script = job.path().join("hancom.ps1");
        std::fs::write(&script, format!("\u{feff}{SCRIPT}")).map_err(io_err(&script))?;
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let ps = root.join(if inst.wow64 { "SysWOW64" } else { "System32" }).join(r"WindowsPowerShell\v1.0\powershell.exe");
        let before = hwp_pids();
        let mut o = Outcome::default();
        let res = process::run(&ps, &script_args_mode(&script, &tmp_in, &tmp_out, target, mode), timeout, cancel, &mut |l| parse_line(&mut o, l));
        // 우리가 띄운 한글만 정리: 스크립트가 알려 준 번호, 못 알려 줬으면(만들다 멈춤) 시작 뒤 새로 생긴 것
        let mut ours = o.pids.clone();
        if ours.is_empty() && !o.nopid && res.is_err() {
            ours = hwp_pids().into_iter().filter(|p| !before.contains(p)).collect();
        }
        if !ours.is_empty() {
            let wait_until = Instant::now() + if res.is_ok() { Duration::from_secs(5) } else { Duration::ZERO };
            loop {
                let alive: Vec<u32> = hwp_pids().into_iter().filter(|p| ours.contains(p)).collect();
                if alive.is_empty() {
                    break;
                }
                if Instant::now() >= wait_until {
                    alive.into_iter().for_each(kill);
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        match res {
            Err(Error::Timeout(_)) => return Err(Error::Tool { tool: "한글".into(), message: "한글이 응답하지 않아 멈췄어요(시간 초과). 한글 창이나 알림이 떠 있으면 닫고 다시 해 주세요.".into() }),
            Err(e) => return Err(e),
            Ok(_) => {}
        }
        if let Some(m) = outcome_error_mode(&o, target, mode) {
            return Err(Error::Tool { tool: "한글".into(), message: m });
        }
        if std::fs::metadata(&tmp_out).map(|m| m.len()).unwrap_or(0) == 0 {
            return Err(Error::Tool { tool: "한글".into(), message: "결과 파일이 생기지 않았어요.".into() });
        }
        let output = crate::paths::output_path(input, out_dir, "", target.ext());
        crate::docs::move_into(&tmp_out, &output)?;
        crate::imageconv::copy_mtime(input, &output);
        Ok(HancomOut { output, pages: o.pages, millis: t0.elapsed().as_millis() as u64 })
    }
}

#[cfg(windows)]
pub use win::{hwp_pids, module_registered, remove_module_value};

/// 설치된 한글로 PDF(`target` Pdf) 또는 DOCX를 만든다. 결과는 `이름.pdf`/`이름.docx`(겹치면 ` (1)`).
pub fn convert(input: &Path, out_dir: Option<&Path>, target: DocTarget, cancel: Option<&Cancel>, timeout: Duration) -> Result<HancomOut> {
    convert_mode(input, out_dir, target, Mode::Save, cancel, timeout)
}

/// [`convert`] + 만드는 방법(배포용 문서는 [`Mode::PrintPdf`], PDF만)
pub fn convert_mode(input: &Path, out_dir: Option<&Path>, target: DocTarget, mode: Mode, cancel: Option<&Cancel>, timeout: Duration) -> Result<HancomOut> {
    if mode == Mode::PrintPdf && target != DocTarget::Pdf {
        return Err(Error::Invalid(super::MSG_DISTRIBUTION_PDF_ONLY.into()));
    }
    if !matches!(target, DocTarget::Pdf | DocTarget::Docx) {
        return Err(Error::Unsupported(format!("{} → {}", input.display(), target.ext())));
    }
    let inst = if hidden() { None } else { installed() };
    let Some(_inst) = inst else {
        return Err(Error::Invalid(super::MSG_NEED_HANCOM.into()));
    };
    let timeout = crate::testhooks::var(TIMEOUT_ENV).and_then(|v| v.trim().parse().ok()).map(Duration::from_secs).unwrap_or(timeout);
    #[cfg(windows)]
    {
        win::convert(&_inst, input, out_dir, target, mode, cancel, timeout)
    }
    #[cfg(not(windows))]
    {
        let _ = (out_dir, cancel, timeout, mode);
        unreachable!("installed()는 Windows에서만 Some")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u16at(b: &[u8], i: usize) -> u16 {
        u16::from_le_bytes([b[i], b[i + 1]])
    }
    fn u32at(b: &[u8], i: usize) -> u32 {
        u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
    }

    #[test]
    fn dll_structure() {
        let d = path_check_dll();
        assert_eq!(d.len(), 0x400);
        assert_eq!(&d[..2], b"MZ");
        let pe = u32at(&d, 0x3c) as usize;
        assert_eq!(&d[pe..pe + 4], b"PE\0\0");
        assert_eq!(u16at(&d, pe + 4), 0x14c); // i386
        assert_eq!(u16at(&d, pe + 22) & 0x2000, 0x2000); // DLL
        let opt = pe + 24;
        assert_eq!(u16at(&d, opt), 0x10b); // PE32
        assert_eq!(u16at(&d, opt + 68), 2); // Subsystem GUI
        // 내보내기 표 → 이름·함수 코드
        let exp_rva = u32at(&d, opt + 96) as usize;
        let rva = |r: usize| r - 0x1000 + 0x200;
        let e = rva(exp_rva);
        assert_eq!(u32at(&d, e + 20), 1);
        let name_ptr = rva(u32at(&d, rva(u32at(&d, e + 32) as usize)) as usize);
        assert_eq!(&d[name_ptr..name_ptr + 17], b"IsAccessiblePath\0");
        let func = rva(u32at(&d, rva(u32at(&d, e + 28) as usize)) as usize);
        assert_eq!(&d[func..func + 8], &[0xB8, 1, 0, 0, 0, 0xC2, 0x10, 0x00]); // mov eax,1; ret 16
        let entry = rva(u32at(&d, opt + 16) as usize);
        assert_eq!(&d[entry..entry + 8], &[0xB8, 1, 0, 0, 0, 0xC2, 0x0C, 0x00]); // DllMain: TRUE
        // 같은 입력이면 같은 바이트(내용 해시로 파일 이름을 정함)
        assert_eq!(d, path_check_dll());
    }

    #[test]
    fn outcome_parsing() {
        let mut o = Outcome::default();
        for l in ["\u{feff}PID 1234", "REG True", "OPEN True", "PAGES 65", "SAVE True", "DONE"] {
            parse_line(&mut o, l);
        }
        assert_eq!(o.pids, vec![1234]);
        assert_eq!(o.pages, Some(65));
        assert_eq!(outcome_error(&o, DocTarget::Pdf), None);
        let mut o = Outcome::default();
        for l in ["PID 7", "REG True", "OPEN False", "DONE"] {
            parse_line(&mut o, l);
        }
        assert!(outcome_error(&o, DocTarget::Pdf).unwrap().contains("열지 못했어요"));
        let mut o = Outcome::default();
        parse_line(&mut o, "PID 7");
        parse_line(&mut o, "SAVE False");
        assert!(outcome_error(&o, DocTarget::Docx).unwrap().contains("Word(DOCX)"));
        let mut o = Outcome::default();
        parse_line(&mut o, "NOPID");
        assert!(outcome_error(&o, DocTarget::Pdf).unwrap().contains("한글 창을 모두 닫고"));
        let mut o = Outcome::default();
        parse_line(&mut o, "ERROR 클래스가 등록되지 않았습니다");
        assert!(outcome_error(&o, DocTarget::Pdf).unwrap().contains("클래스가"));
        // 아무 말 없이 끝나면 실패
        assert!(outcome_error(&Outcome::default(), DocTarget::Pdf).is_some());
        // 0.3.1 배포용: 열렸는데 인쇄가 거절되면(만든 사람이 인쇄를 막음) 그 안내
        let mut o = Outcome::default();
        for l in ["PID 7", "REG True", "OPEN True", "PAGES 1", "SAVE False", "DONE"] {
            parse_line(&mut o, l);
        }
        assert_eq!(outcome_error_mode(&o, DocTarget::Pdf, Mode::PrintPdf).as_deref(), Some(super::super::MSG_DISTRIBUTION_NOPRINT));
        assert!(outcome_error_mode(&o, DocTarget::Pdf, Mode::Save).unwrap().contains("PDF로 저장하지 못했어요"));
        let a = script_args_mode(Path::new("s.ps1"), Path::new("in.hwp"), Path::new("out.pdf"), DocTarget::Pdf, Mode::PrintPdf);
        assert!(a.iter().any(|x| x == "PRINTPDF"));
        // 배포용은 PDF만
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.hwp");
        std::fs::write(&p, b"x").unwrap();
        assert_eq!(convert_mode(&p, None, DocTarget::Docx, Mode::PrintPdf, None, TIMEOUT).unwrap_err().to_string(), super::super::MSG_DISTRIBUTION_PDF_ONLY);
    }

    #[test]
    fn args_and_paths() {
        let a = script_args(Path::new(r"C:\t\hancom.ps1"), Path::new(r"C:\t\in.hwp"), Path::new(r"C:\t\out.docx"), DocTarget::Docx);
        let a: Vec<String> = a.iter().map(|s| s.to_string_lossy().into_owned()).collect();
        let i = a.iter().position(|x| x == "-Fmt").unwrap();
        assert_eq!(a[i + 1], "OOXML");
        assert!(a.contains(&"-NonInteractive".to_string()) && a.contains(&MODULE_NAME.to_string()));
        assert_eq!(exe_from_command(r#""C:\Program Files (x86)\HNC\Office 2020\HOffice110\bin\hwp.exe" -Automation"#), PathBuf::from(r"C:\Program Files (x86)\HNC\Office 2020\HOffice110\bin\hwp.exe"));
        assert_eq!(exe_from_command(r"C:\Program Files (x86)\HNC\Office 2020\HOffice110\bin\hwp.exe -Automation"), PathBuf::from(r"C:\Program Files (x86)\HNC\Office 2020\HOffice110\bin\hwp.exe"));
        // 스크립트는 상태 줄을 내보내고 사용자 한글에는 Quit하지 않는다
        for k in ["PID $p", "NOPID", "RegisterModule('FilePathCheckDLL', $Module)", "SetMessageBoxMode", "suspendpassword:true", "$hwp.Quit()", "if ($ours)"] {
            assert!(SCRIPT.contains(k), "{k}");
        }
    }

    #[test]
    fn hidden_env() {
        // 이 시험은 환경 변수를 바꾸지 않는다(병렬 시험). 숨김 판단 함수만 확인
        if std::env::var_os(HIDE_ENV).is_some() {
            assert!(!available());
        }
        #[cfg(not(windows))]
        assert!(!available() && installed().is_none());
    }
}
