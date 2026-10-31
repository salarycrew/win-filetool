//! filetool-svc.exe — 파일 도구 빠른 검색 서비스(2c, 선택 설치).
//!
//! ```text
//! filetool-svc install [--account system|virtual]   (관리자) Program Files로 복사 → 서비스 등록 → 시작
//! filetool-svc uninstall                             (관리자) 멈춤 → 등록 해제 → Program Files·ProgramData 폴더 지움
//! filetool-svc status                                설치·상태·계정·실행 파일·폴더 ACL·파이프 ACL·색인 상태
//! filetool-svc run                                   (SCM이 부름) 서비스 본체
//! ```
//!
//! 서비스 본체: NTFS 고정 드라이브마다 `MftSource`로 색인(저장한 색인이 있으면 저널 따라잡기), USN 저널을 따라가며,
//! 색인은 `%ProgramData%\kr.salarycrew.winfiletool\index\index-v2.bin`(SYSTEM·관리자만)에 저장한다.
//! 앱·CLI는 이름 있는 파이프로 묻는다(`filetool_core::search::svcproto`).

#[cfg(not(windows))]
fn main() {
    eprintln!("filetool-svc는 Windows 전용입니다");
    std::process::exit(1);
}

#[cfg(windows)]
fn main() {
    std::process::exit(win::main());
}

#[cfg(windows)]
mod win {
    use std::io::Write;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use filetool_core::search::index::now_unix;
    use filetool_core::search::mft::{self, SourceMode};
    use filetool_core::search::scan::{default_excludes, fixed_drives};
    use filetool_core::search::service::{SearchConfig, SearchService};
    use filetool_core::search::svcpipe::{self, PipeServer};
    use filetool_core::search::svcproto::{self, describe_status, service_data_dir, service_index_dir, Handler, SvcStatus, PIPE_NAME, SERVICE_NAME};

    use windows_sys::Win32::Foundation::{GetLastError, LocalFree, ERROR_ACCESS_DENIED, ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_EXISTS};
    use windows_sys::Win32::Security::Authentication::Identity::{
        LsaAddAccountRights, LsaClose, LsaNtStatusToWinError, LsaOpenPolicy, LsaRemoveAccountRights, LSA_OBJECT_ATTRIBUTES, LSA_UNICODE_STRING,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        GetNamedSecurityInfoW, SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{GetSecurityDescriptorDacl, LookupAccountNameW, ACL};
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    use windows_sys::Win32::System::Services::*;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetPriorityClass};

    const DISPLAY_NAME: &str = "파일 도구 빠른 검색 서비스";
    const DESCRIPTION: &str =
        "파일 도구 앱의 빠른 검색(NTFS MFT 읽기·USN 저널)을 맡습니다. 관리자 권한 없이 실행한 앱이 이 서비스에 검색을 묻습니다. 지우거나 멈춰도 앱은 일반 색인으로 동작합니다.";
    const INSTALL_DIR_NAME: &str = "파일 도구 서비스";
    const EXE_NAME: &str = "filetool-svc.exe";
    const VIRTUAL_ACCOUNT: &str = r"NT SERVICE\FileToolSvc";
    /// Program Files 아래 서비스 폴더: SYSTEM·관리자 전체, Users 읽기·실행만(상속 끊음)
    const INSTALL_DIR_SDDL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;BU)";
    /// ProgramData 아래 자료 폴더(색인에 모든 이름이 있음): SYSTEM·관리자만
    const DATA_DIR_SDDL: &str = "D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
    const SERVICE_PRIVILEGES: &[&str] = &["SeChangeNotifyPrivilege", "SeBackupPrivilege", "SeManageVolumePrivilege"];
    const VIRTUAL_RIGHTS: &[&str] = &["SeBackupPrivilege", "SeManageVolumePrivilege"];

    const PROCESS_MODE_BACKGROUND_BEGIN: u32 = 0x0010_0000;
    const PROCESS_MODE_BACKGROUND_END: u32 = 0x0020_0000;
    const SE_FILE_OBJECT: i32 = 1;
    const OWNER_SECURITY_INFORMATION: u32 = 0x1;
    const DACL_SECURITY_INFORMATION: u32 = 0x4;
    const PROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x8000_0000;
    const MOVEFILE_DELAY_UNTIL_REBOOT: u32 = 0x4;
    const POLICY_ALL_ACCESS: u32 = 0x000F_0FFF;

    pub fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
    }
    fn wide_path(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }
    fn from_wide_ptr(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0;
        unsafe {
            while *p.add(n) != 0 {
                n += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
        }
    }
    fn last_error() -> u32 {
        unsafe { GetLastError() }
    }

    pub fn main() -> i32 {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let cmd = args.first().map(String::as_str).unwrap_or("");
        let account = match args.iter().position(|a| a == "--account").and_then(|i| args.get(i + 1)).map(String::as_str) {
            None | Some("system") => Account::System,
            Some("virtual") => Account::Virtual,
            Some(o) => {
                eprintln!("--account는 system 또는 virtual입니다(받은 값: {o})");
                return 1;
            }
        };
        let r = match cmd {
            "run" => return run_dispatcher(),
            "install" => install(account),
            "uninstall" => uninstall(),
            "status" => return status(),
            _ => {
                println!("filetool-svc {} — 파일 도구 빠른 검색 서비스", env!("CARGO_PKG_VERSION"));
                println!("  filetool-svc install [--account system|virtual]   (관리자) 설치·시작");
                println!("  filetool-svc uninstall                             (관리자) 멈춤·제거(폴더까지)");
                println!("  filetool-svc status                                상태");
                return if cmd.is_empty() || cmd == "help" || cmd == "--help" { 0 } else { 1 };
            }
        };
        match r {
            Ok(()) => 0,
            Err(Fail::NotAdmin) => {
                eprintln!("관리자 권한이 필요합니다. 관리자 권한으로 실행하세요(앱의 [설정] > 검색 > 빠른 검색 서비스 설치 단추는 UAC 창으로 권한을 얻습니다).");
                2
            }
            Err(Fail::Msg(m)) => {
                eprintln!("{m}");
                1
            }
        }
    }

    enum Fail {
        NotAdmin,
        Msg(String),
    }
    impl From<String> for Fail {
        fn from(s: String) -> Self {
            Fail::Msg(s)
        }
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Account {
        System,
        Virtual,
    }

    pub fn install_dir() -> PathBuf {
        let pf = std::env::var_os("ProgramW6432").or_else(|| std::env::var_os("ProgramFiles")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
        pf.join(INSTALL_DIR_NAME)
    }

    // ---------- SCM ----------

    struct Sc(SC_HANDLE);
    impl Drop for Sc {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CloseServiceHandle(self.0) };
            }
        }
    }

    fn scm(access: u32) -> Result<Sc, Fail> {
        let h = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), access) };
        if h.is_null() {
            let e = last_error();
            return Err(if e == ERROR_ACCESS_DENIED { Fail::NotAdmin } else { Fail::Msg(format!("서비스 관리자를 열 수 없습니다(오류 {e})")) });
        }
        Ok(Sc(h))
    }

    fn open_svc(m: &Sc, access: u32) -> Result<Option<Sc>, Fail> {
        let n = wide(SERVICE_NAME);
        let h = unsafe { OpenServiceW(m.0, n.as_ptr(), access) };
        if h.is_null() {
            let e = last_error();
            return match e {
                ERROR_SERVICE_DOES_NOT_EXIST => Ok(None),
                ERROR_ACCESS_DENIED => Err(Fail::NotAdmin),
                _ => Err(Fail::Msg(format!("서비스를 열 수 없습니다(오류 {e})"))),
            };
        }
        Ok(Some(Sc(h)))
    }

    fn state(s: &Sc) -> u32 {
        let mut st = SERVICE_STATUS_PROCESS::default();
        let mut need = 0u32;
        unsafe { QueryServiceStatusEx(s.0, SC_STATUS_PROCESS_INFO, &mut st as *mut _ as *mut u8, std::mem::size_of_val(&st) as u32, &mut need) };
        st.dwCurrentState
    }

    fn wait_state(s: &Sc, want: u32, limit: Duration) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < limit {
            if state(s) == want {
                return true;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        state(s) == want
    }

    fn stop_service(s: &Sc) -> Result<(), String> {
        if state(s) == SERVICE_STOPPED {
            return Ok(());
        }
        let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        unsafe { ControlService(s.0, SERVICE_CONTROL_STOP, &mut st) };
        if !wait_state(s, SERVICE_STOPPED, Duration::from_secs(60)) {
            return Err("서비스가 60초 안에 멈추지 않았습니다".into());
        }
        Ok(())
    }

    fn multi_sz(items: &[&str]) -> Vec<u16> {
        let mut v = Vec::new();
        for i in items {
            v.extend(i.encode_utf16());
            v.push(0);
        }
        v.push(0);
        v
    }

    // ---------- 보안(ACL) ----------

    /// 폴더 DACL을 SDDL로 바꿈(상속 끊음, 아래 항목에도 퍼짐)
    fn set_dacl(path: &Path, sddl: &str) -> Result<(), String> {
        let w = wide(sddl);
        let mut sd: *mut core::ffi::c_void = std::ptr::null_mut();
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(w.as_ptr(), 1, &mut sd, std::ptr::null_mut()) } == 0 {
            return Err(format!("SDDL 오류 {}", last_error()));
        }
        let (mut present, mut defaulted) = (0, 0);
        let mut dacl: *mut ACL = std::ptr::null_mut();
        unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) };
        let p = wide_path(path);
        let r = unsafe {
            SetNamedSecurityInfoW(p.as_ptr(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, std::ptr::null_mut(), std::ptr::null_mut(), dacl, std::ptr::null())
        };
        unsafe { LocalFree(sd) };
        if r != 0 {
            return Err(format!("{}: 권한을 설정할 수 없습니다(오류 {r})", path.display()));
        }
        Ok(())
    }

    /// 경로의 소유자·DACL을 SDDL 문자열로
    pub fn sddl_of(path: &str) -> Result<String, String> {
        let p = wide(path);
        let mut sd: *mut core::ffi::c_void = std::ptr::null_mut();
        let r = unsafe {
            GetNamedSecurityInfoW(
                p.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut sd,
            )
        };
        if r != 0 {
            return Err(format!("오류 {r}"));
        }
        let mut s: *mut u16 = std::ptr::null_mut();
        let ok = unsafe { ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, 1, OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION, &mut s, std::ptr::null_mut()) };
        unsafe { LocalFree(sd) };
        if ok == 0 {
            return Err(format!("오류 {}", last_error()));
        }
        let out = from_wide_ptr(s);
        unsafe { LocalFree(s as _) };
        Ok(out)
    }

    fn owner_sid(sddl: &str) -> Option<&str> {
        sddl.strip_prefix("O:").map(|r| r.split("D:").next().unwrap_or(r))
    }

    fn account_sid(name: &str) -> Result<(Vec<u8>, String), String> {
        let n = wide(name);
        let mut sid = vec![0u8; 256];
        let mut sl = 256u32;
        let mut dom = [0u16; 256];
        let mut dl = 256u32;
        let mut use_ = 0i32;
        if unsafe { LookupAccountNameW(std::ptr::null(), n.as_ptr(), sid.as_mut_ptr() as _, &mut sl, dom.as_mut_ptr(), &mut dl, &mut use_) } == 0 {
            return Err(format!("{name}: 계정을 찾을 수 없습니다(오류 {})", last_error()));
        }
        let mut s: *mut u16 = std::ptr::null_mut();
        unsafe { ConvertSidToStringSidW(sid.as_mut_ptr() as _, &mut s) };
        let ss = from_wide_ptr(s);
        unsafe { LocalFree(s as _) };
        Ok((sid, ss))
    }

    fn lsa_rights(sid: &mut [u8], add: bool) -> Result<(), String> {
        let attrs: LSA_OBJECT_ATTRIBUTES = unsafe { std::mem::zeroed() };
        let mut pol = 0isize;
        let st = unsafe { LsaOpenPolicy(std::ptr::null(), &attrs, POLICY_ALL_ACCESS, &mut pol as *mut isize as _) };
        if st != 0 {
            return Err(format!("LSA 정책을 열 수 없습니다(오류 {})", unsafe { LsaNtStatusToWinError(st) }));
        }
        let names: Vec<Vec<u16>> = VIRTUAL_RIGHTS.iter().map(|r| r.encode_utf16().collect()).collect();
        let us: Vec<LSA_UNICODE_STRING> =
            names.iter().map(|n| LSA_UNICODE_STRING { Length: (n.len() * 2) as u16, MaximumLength: (n.len() * 2) as u16, Buffer: n.as_ptr() as *mut u16 }).collect();
        let st = unsafe {
            if add {
                LsaAddAccountRights(pol as _, sid.as_mut_ptr() as _, us.as_ptr(), us.len() as u32)
            } else {
                LsaRemoveAccountRights(pol as _, sid.as_mut_ptr() as _, true, std::ptr::null(), 0)
            }
        };
        unsafe { LsaClose(pol as _) };
        if st != 0 {
            let e = unsafe { LsaNtStatusToWinError(st) };
            // 지울 권한이 없었음(2 = 없음)은 괜찮다
            if !add && e == 2 {
                return Ok(());
            }
            return Err(format!("계정 권한을 {} 수 없습니다(오류 {e})", if add { "줄" } else { "거둘" }));
        }
        Ok(())
    }

    // ---------- install / uninstall ----------

    fn install(account: Account) -> Result<(), Fail> {
        let m = scm(SC_MANAGER_ALL_ACCESS)?;
        let src = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = install_dir();
        let dst = dir.join(EXE_NAME);
        println!("[1/6] 실행 파일: {} → {}", src.display(), dst.display());
        let existing = open_svc(&m, SERVICE_ALL_ACCESS)?;
        if let Some(s) = &existing {
            println!("      이미 설치됨 → 멈추고 새로 씀");
            stop_service(s)?;
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        set_dacl(&dir, INSTALL_DIR_SDDL)?;
        let same = src.canonicalize().ok() == dst.canonicalize().ok();
        if !same {
            let tmp = dir.join(format!("{EXE_NAME}.new"));
            std::fs::copy(&src, &tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
            std::fs::rename(&tmp, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
        }
        set_dacl(&dir, INSTALL_DIR_SDDL)?;

        println!("[2/6] 서비스 등록: {SERVICE_NAME} ({DISPLAY_NAME}), 계정 {}", if account == Account::System { "LocalSystem" } else { VIRTUAL_ACCOUNT });
        let bin = wide(&format!("\"{}\" run", dst.display()));
        let disp = wide(DISPLAY_NAME);
        let acct = wide(VIRTUAL_ACCOUNT);
        let acct_ptr = if account == Account::Virtual { acct.as_ptr() } else { std::ptr::null() };
        let s = match existing {
            Some(s) => {
                let ok = unsafe {
                    ChangeServiceConfigW(
                        s.0,
                        SERVICE_WIN32_OWN_PROCESS,
                        SERVICE_AUTO_START,
                        SERVICE_ERROR_NORMAL,
                        bin.as_ptr(),
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        std::ptr::null(),
                        if account == Account::Virtual { acct.as_ptr() } else { wide("LocalSystem").leak().as_ptr() },
                        std::ptr::null(),
                        disp.as_ptr(),
                    )
                };
                if ok == 0 {
                    return Err(format!("서비스 설정을 바꿀 수 없습니다(오류 {})", last_error()).into());
                }
                s
            }
            None => {
                let name = wide(SERVICE_NAME);
                let h = unsafe {
                    CreateServiceW(
                        m.0,
                        name.as_ptr(),
                        disp.as_ptr(),
                        SERVICE_ALL_ACCESS,
                        SERVICE_WIN32_OWN_PROCESS,
                        SERVICE_AUTO_START,
                        SERVICE_ERROR_NORMAL,
                        bin.as_ptr(),
                        std::ptr::null(),
                        std::ptr::null_mut(),
                        std::ptr::null(),
                        acct_ptr,
                        std::ptr::null(),
                    )
                };
                if h.is_null() {
                    let e = last_error();
                    return Err(format!("서비스를 등록할 수 없습니다(오류 {e}{})", if e == ERROR_SERVICE_EXISTS { ", 이미 있음" } else { "" }).into());
                }
                Sc(h)
            }
        };
        // 설명, 지연된 자동 시작, 서비스 SID, 필요한 권한만, 실패 시 다시 시작
        let mut desc = wide(DESCRIPTION);
        let d = SERVICE_DESCRIPTIONW { lpDescription: desc.as_mut_ptr() };
        let delayed = SERVICE_DELAYED_AUTO_START_INFO { fDelayedAutostart: 1 };
        let sidinfo = SERVICE_SID_INFO { dwServiceSidType: SERVICE_SID_TYPE_UNRESTRICTED };
        let mut privs = multi_sz(SERVICE_PRIVILEGES);
        let req = SERVICE_REQUIRED_PRIVILEGES_INFOW { pmszRequiredPrivileges: privs.as_mut_ptr() };
        let mut actions = [SC_ACTION { Type: SC_ACTION_RESTART, Delay: 10_000 }, SC_ACTION { Type: SC_ACTION_RESTART, Delay: 60_000 }, SC_ACTION { Type: SC_ACTION_NONE, Delay: 0 }];
        let fa = SERVICE_FAILURE_ACTIONSW { dwResetPeriod: 86_400, lpRebootMsg: std::ptr::null_mut(), lpCommand: std::ptr::null_mut(), cActions: 3, lpsaActions: actions.as_mut_ptr() };
        for (level, p, what) in [
            (SERVICE_CONFIG_DESCRIPTION, &d as *const _ as *const core::ffi::c_void, "설명"),
            (SERVICE_CONFIG_DELAYED_AUTO_START_INFO, &delayed as *const _ as *const _, "지연된 시작"),
            (SERVICE_CONFIG_SERVICE_SID_INFO, &sidinfo as *const _ as *const _, "서비스 SID"),
            (SERVICE_CONFIG_REQUIRED_PRIVILEGES_INFO, &req as *const _ as *const _, "필요한 권한"),
            (SERVICE_CONFIG_FAILURE_ACTIONS, &fa as *const _ as *const _, "실패 시 동작"),
        ] {
            if unsafe { ChangeServiceConfig2W(s.0, level, p) } == 0 {
                return Err(format!("서비스 {what}을(를) 설정할 수 없습니다(오류 {})", last_error()).into());
            }
        }

        let data = service_data_dir();
        println!("[3/6] 자료 폴더: {} (SYSTEM·관리자만)", data.display());
        let mut data_sddl = DATA_DIR_SDDL.to_string();
        let mut svc_sid = None;
        if account == Account::Virtual {
            let (mut sid, ss) = account_sid(VIRTUAL_ACCOUNT)?;
            println!("      가상 계정 {VIRTUAL_ACCOUNT} = {ss} → 권한 {}", VIRTUAL_RIGHTS.join(", "));
            lsa_rights(&mut sid, true)?;
            data_sddl.push_str(&format!("(A;OICI;FA;;;{ss})"));
            svc_sid = Some(ss);
        }
        prepare_data_dir(&data, &data_sddl)?;
        if let Some(ss) = &svc_sid {
            set_dacl(&dir, &format!("{INSTALL_DIR_SDDL}(A;OICI;0x1200a9;;;{ss})"))?;
        }

        println!("[4/6] 시작");
        if unsafe { StartServiceW(s.0, 0, std::ptr::null()) } == 0 {
            let e = last_error();
            if e != 1056 {
                return Err(format!("서비스를 시작할 수 없습니다(오류 {e})").into());
            }
        }
        if !wait_state(&s, SERVICE_RUNNING, Duration::from_secs(30)) {
            return Err(format!("서비스가 30초 안에 실행되지 않았습니다(상태 {})", svcpipe::state_label(state(&s))).into());
        }
        println!("[5/6] 파이프 확인");
        let t0 = Instant::now();
        let mut last = String::new();
        while t0.elapsed() < Duration::from_secs(20) {
            match svcpipe::connect("filetool-svc install").and_then(|mut c| c.status()) {
                Ok(st) => {
                    for l in describe_status(&st) {
                        println!("      {l}");
                    }
                    println!("[6/6] 설치 끝({:.1} 초 뒤 파이프 응답)", t0.elapsed().as_secs_f64());
                    return Ok(());
                }
                Err(e) => last = e,
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        Err(format!("서비스는 실행 중이지만 파이프가 답하지 않습니다: {last}").into())
    }

    /// 자료 폴더 준비. 일반 사용자가 먼저 만들어 둔 폴더(소유자가 SYSTEM·관리자가 아님)는 지우고 새로 만든다
    fn prepare_data_dir(data: &Path, sddl: &str) -> Result<(), String> {
        if data.exists() {
            let cur = sddl_of(&data.to_string_lossy()).unwrap_or_default();
            let owner = owner_sid(&cur).unwrap_or("");
            if owner != "SY" && owner != "BA" && owner != "S-1-5-18" && owner != "S-1-5-32-544" {
                println!("      소유자가 SYSTEM·관리자가 아닌 폴더({owner}) → 지우고 새로 만듦");
                std::fs::remove_dir_all(data).map_err(|e| format!("{}: {e}", data.display()))?;
            }
        }
        std::fs::create_dir_all(data).map_err(|e| format!("{}: {e}", data.display()))?;
        set_dacl(data, sddl)?;
        std::fs::create_dir_all(service_index_dir(data)).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn uninstall() -> Result<(), Fail> {
        let m = scm(SC_MANAGER_ALL_ACCESS)?;
        match open_svc(&m, SERVICE_ALL_ACCESS)? {
            Some(s) => {
                let virt = svcpipe::service_info().account.eq_ignore_ascii_case(VIRTUAL_ACCOUNT);
                println!("[1/4] 서비스 멈춤");
                stop_service(&s)?;
                println!("[2/4] 등록 해제");
                if unsafe { DeleteService(s.0) } == 0 {
                    let e = last_error();
                    if e != 1072 {
                        return Err(format!("서비스를 지울 수 없습니다(오류 {e})").into());
                    }
                }
                drop(s);
                if virt {
                    if let Ok((mut sid, ss)) = account_sid(VIRTUAL_ACCOUNT) {
                        println!("      가상 계정 {ss} 권한 거둠");
                        lsa_rights(&mut sid, false)?;
                    }
                }
                let t0 = Instant::now();
                while t0.elapsed() < Duration::from_secs(10) && open_svc(&m, SERVICE_QUERY_STATUS)?.is_some() {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
            None => println!("[1/4] 서비스 없음(이미 제거됨)"),
        }
        let data = service_data_dir();
        println!("[3/4] 자료 폴더 지움: {}", data.display());
        if data.exists() {
            std::fs::remove_dir_all(&data).map_err(|e| format!("{}: {e}", data.display()))?;
        }
        let dir = install_dir();
        println!("[4/4] 서비스 폴더 지움: {}", dir.display());
        if dir.exists() {
            let me = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok());
            let inside = me.as_ref().map(|p| dir.canonicalize().map(|d| p.starts_with(d)).unwrap_or(false)).unwrap_or(false);
            if inside {
                // 실행 중인 자신은 지울 수 없음 → 다시 시작할 때 지우도록 예약
                for e in std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
                    let p = e.path();
                    if Some(p.canonicalize().unwrap_or(p.clone())) != me {
                        let _ = std::fs::remove_file(&p);
                    }
                }
                for p in [me.clone().unwrap(), dir.clone()] {
                    let w = wide_path(&p);
                    unsafe { MoveFileExW(w.as_ptr(), std::ptr::null(), MOVEFILE_DELAY_UNTIL_REBOOT) };
                }
                println!("      실행 중인 {EXE_NAME}는 다음 재부팅 때 지워집니다(폴더째). 앱 폴더의 filetool-svc.exe로 제거하면 바로 지워집니다");
            } else {
                std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
        }
        println!("제거 끝");
        Ok(())
    }

    fn status() -> i32 {
        let info = svcpipe::service_info();
        println!("서비스 {SERVICE_NAME}: {}", if info.installed { "설치됨" } else { "설치 안 됨" });
        if info.installed {
            println!("  상태 {} · pid {} · 시작 {} · 계정 {}", info.state, info.pid, info.start_type, if info.account.is_empty() { "?" } else { &info.account });
            println!("  실행 파일 {}", info.binary_path);
        }
        let dir = install_dir();
        let data = service_data_dir();
        for (what, p) in [("서비스 폴더", dir.to_string_lossy().into_owned()), ("자료 폴더", data.to_string_lossy().into_owned()), ("파이프", PIPE_NAME.to_string())] {
            if what != "파이프" && !Path::new(&p).exists() {
                println!("  {what} {p}: 없음");
                continue;
            }
            match sddl_of(&p) {
                Ok(s) => println!("  {what} {p}: {s}"),
                Err(e) => println!("  {what} {p}: ACL 읽기 실패({e})"),
            }
        }
        if !info.installed {
            return 3;
        }
        match svcpipe::connect("filetool-svc status").and_then(|mut c| {
            let me = c.me.clone();
            c.status().map(|s| (me, s))
        }) {
            Ok((me, st)) => {
                println!("  호출자 {} ({}) 관리자 {} · {}", me.user, me.sid, me.admin, me.visibility);
                for l in describe_status(&st) {
                    println!("  {l}");
                }
                0
            }
            Err(e) => {
                println!("  파이프: {e}");
                4
            }
        }
    }

    // ---------- 서비스 본체 ----------

    static STATUS: OnceLock<usize> = OnceLock::new();
    static STOP_TX: OnceLock<Mutex<Sender<()>>> = OnceLock::new();
    static LOG: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

    fn log(msg: String) {
        let line = format!("{} {msg}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"));
        if let Some(m) = LOG.get() {
            if let Some(f) = m.lock().unwrap().as_mut() {
                let _ = f.write_all(line.as_bytes());
            }
        }
    }

    fn open_log(data: &Path) {
        let p = data.join("service.log");
        // 1 MB가 넘으면 새로
        if std::fs::metadata(&p).map(|m| m.len() > 1 << 20).unwrap_or(false) {
            let _ = std::fs::rename(&p, data.join("service.old.log"));
        }
        let f = std::fs::OpenOptions::new().create(true).append(true).open(&p).ok();
        let _ = LOG.set(Mutex::new(f));
    }

    fn run_dispatcher() -> i32 {
        let mut name = wide(SERVICE_NAME);
        let table = [SERVICE_TABLE_ENTRYW { lpServiceName: name.as_mut_ptr(), lpServiceProc: Some(service_main) }, SERVICE_TABLE_ENTRYW { lpServiceName: std::ptr::null_mut(), lpServiceProc: None }];
        if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
            eprintln!("서비스 관리자에서 실행해야 합니다(오류 {}). 설치는 filetool-svc install", last_error());
            return 1;
        }
        0
    }

    fn report(state: u32, wait_hint: u32) {
        let Some(&h) = STATUS.get() else { return };
        let st = SERVICE_STATUS {
            dwServiceType: SERVICE_WIN32_OWN_PROCESS,
            dwCurrentState: state,
            dwControlsAccepted: if state == SERVICE_RUNNING { SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN } else { 0 },
            dwWin32ExitCode: 0,
            dwServiceSpecificExitCode: 0,
            dwCheckPoint: if state == SERVICE_RUNNING || state == SERVICE_STOPPED { 0 } else { 1 },
            dwWaitHint: wait_hint,
        };
        unsafe { SetServiceStatus(h as SERVICE_STATUS_HANDLE, &st) };
    }

    unsafe extern "system" fn control(ctrl: u32, _t: u32, _d: *mut core::ffi::c_void, _c: *mut core::ffi::c_void) -> u32 {
        match ctrl {
            SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
                report(SERVICE_STOP_PENDING, 30_000);
                if let Some(tx) = STOP_TX.get() {
                    let _ = tx.lock().unwrap().send(());
                }
                0
            }
            SERVICE_CONTROL_INTERROGATE => 0,
            _ => 120, // ERROR_CALL_NOT_IMPLEMENTED
        }
    }

    unsafe extern "system" fn service_main(_argc: u32, _argv: *mut windows_sys::core::PWSTR) {
        let name = wide(SERVICE_NAME);
        let h = unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control), std::ptr::null()) };
        if h.is_null() {
            return;
        }
        let _ = STATUS.set(h as usize);
        let (tx, rx) = channel();
        let _ = STOP_TX.set(Mutex::new(tx));
        report(SERVICE_START_PENDING, 10_000);
        let host = match Host::start() {
            Ok(x) => x,
            Err(e) => {
                log(format!("시작 실패: {e}"));
                report(SERVICE_STOPPED, 0);
                return;
            }
        };
        report(SERVICE_RUNNING, 0);
        let _ = rx.recv();
        log("멈춤 요청".into());
        host.stop();
        log("멈춤".into());
        report(SERVICE_STOPPED, 0);
    }

    struct Host {
        search: Arc<SearchService>,
        pipe: PipeServer,
    }

    impl Host {
        fn start() -> Result<Host, String> {
            let t0 = Instant::now();
            let started = now_unix();
            let data = service_data_dir();
            let idx = service_index_dir(&data);
            std::fs::create_dir_all(&idx).map_err(|e| format!("{}: {e}", idx.display()))?;
            open_log(&data);
            log(format!("시작: {} pid {} · 자료 폴더 {}", svcproto::server_name(), std::process::id(), data.display()));
            // 처음 색인하는 동안은 낮은 I/O·CPU 우선순위(배경 모드). 준비되면 끈다
            let bg = unsafe { SetPriorityClass(GetCurrentProcess(), PROCESS_MODE_BACKGROUND_BEGIN) } != 0;
            log(format!("배경 모드(낮은 I/O 우선순위): {}", if bg { "켬" } else { "실패" }));
            // NTFS 고정 드라이브 중 볼륨을 열 수 있는 것만(그 밖은 앱이 스스로 색인)
            let mut roots = Vec::new();
            for r in fixed_drives() {
                match mft::open_volume(&r) {
                    Ok(_) => roots.push(r),
                    Err(e) => log(format!("{}: 빼고 시작({e})", r.display())),
                }
            }
            log(format!("루트: {}", roots.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(", ")));
            let mut cfg = SearchConfig::new(roots, default_excludes(), &idx);
            cfg.source = SourceMode::Auto;
            let search = SearchService::start(cfg);
            let clients = Arc::new(AtomicUsize::new(0));
            let ready_ms: Arc<OnceLock<u64>> = Arc::new(OnceLock::new());
            let (s2, c2, r2) = (search.clone(), clients.clone(), ready_ms.clone());
            let handler = Arc::new(Handler {
                index: search.index.clone(),
                status: Box::new(move || {
                    let mut st = SvcStatus::from_search(&s2.status());
                    st.started = started;
                    st.ready_ms = r2.get().copied();
                    st.clients = c2.load(Ordering::SeqCst);
                    st
                }),
            });
            let pipe = PipeServer::start(handler, clients, Arc::new(log))?;
            // 처음 색인이 끝나면 기록하고 배경 모드를 끈다
            let s3 = search.clone();
            std::thread::Builder::new()
                .name("ready-watch".into())
                .spawn(move || loop {
                    std::thread::sleep(Duration::from_millis(250));
                    let st = s3.status();
                    if st.ready && st.roots.iter().all(|r| !r.scanning) {
                        let ms = t0.elapsed().as_millis() as u64;
                        let _ = ready_ms.set(ms);
                        if bg {
                            unsafe { SetPriorityClass(GetCurrentProcess(), PROCESS_MODE_BACKGROUND_END) };
                        }
                        log(format!("준비: {:.2} 초, 항목 {}개, 불러오기 {:?} ms", ms as f64 / 1000.0, st.entries, st.loaded_ms));
                        for r in &st.roots {
                            log(format!(
                                "  {} [{}] 폴더 {} 파일 {} 스캔 {} ms {}",
                                r.path,
                                r.source,
                                r.stats.dirs,
                                r.stats.files,
                                r.stats.millis,
                                r.catchup.clone().or(r.source_note.clone()).unwrap_or_default()
                            ));
                        }
                        break;
                    }
                })
                .ok();
            Ok(Host { search, pipe })
        }

        fn stop(self) {
            self.pipe.stop();
            match self.search.save_now() {
                Ok(n) => log(format!("색인 저장 {n} 바이트")),
                Err(e) => log(format!("색인 저장 실패: {e}")),
            }
            self.search.stop();
        }
    }
}
