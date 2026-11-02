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
    // 0.3.3(보안 강화): DLL은 System32에서만 찾는다 — 맨 먼저
    filetool_core::dllsafe::harden();
    std::process::exit(win::main());
}

/// 0.3.3(보안 강화): 설치할 때 복사하는 실행 파일이 **지금 돌고 있는 이 빌드와 같은지** 빌드 때 넣은 SHA-256으로 확인한다.
///
/// [설치·다시 설치(갱신)]은 사용자가 쓸 수 있는 앱 폴더의 `filetool-svc.exe`를 UAC로 실행하고, 관리자 권한 프로세스가 그 **경로**를
/// 다시 열어 Program Files로 복사한다. 그 사이 같은 사용자의 다른 프로그램이 실행 중인 exe 이름을 바꾸고 같은 경로에 다른 파일을
/// 두면, 그 파일이 SYSTEM 서비스가 된다. 그래서:
/// - 빌드가 끝난 뒤 `scripts/patch-svc-hash.py`가 아래 [`SELF_HASH`]의 32바이트 자리에 "그 자리를 0으로 둔 파일 전체의 SHA-256"을 써 넣는다
/// - 설치는 경로를 **쓰기·지우기 공유 없이** 한 번 열어 다 읽고, 같은 계산으로 메모리의(=실행 중인 이미지의) 값과 비교한 뒤,
///   읽은 그 바이트를 복사한다(다시 열지 않음)
mod selfhash {
    use sha2::{Digest, Sha256};

    /// `[표식 16][SHA-256 32][끝 표식 16]`. 표식 문자열은 이 정적 값에만 있다(찾을 때도 이 값을 씀 → 파일에 한 번만 나옴)
    #[used]
    pub static SELF_HASH: [u8; 64] = *b"WFT-SVC-SHA256::\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0::WFT-SVC-END:::";

    fn embedded() -> [u8; 64] {
        // 컴파일러가 상수로 접지 않게(빌드 뒤 파일에서 바뀐 값을 읽어야 함)
        unsafe { std::ptr::read_volatile(&SELF_HASH) }
    }

    /// 실행 중인 이미지에 든 값(아직 넣지 않았으면 None)
    pub fn expected() -> Option<[u8; 32]> {
        let e = embedded();
        let h: [u8; 32] = e[16..48].try_into().ok()?;
        (h != [0u8; 32]).then_some(h)
    }

    /// 파일 바이트에서 표식을 찾아(정확히 한 번) 해시 자리를 0으로 둔 SHA-256과 그 자리에 적힌 값
    pub fn compute(bytes: &[u8]) -> Result<([u8; 32], [u8; 32]), String> {
        let e = embedded();
        let (head, tail) = (&e[..16], &e[48..]);
        let mut at = None;
        let mut i = 0;
        while let Some(off) = bytes[i..].windows(16).position(|w| w == head) {
            let pos = i + off;
            if bytes.len() >= pos + 64 && &bytes[pos + 48..pos + 64] == tail {
                if at.is_some() {
                    return Err("확인값 자리가 두 번 나옵니다".into());
                }
                at = Some(pos);
            }
            i = pos + 1;
        }
        let pos = at.ok_or("확인값 자리를 찾지 못했습니다")?;
        let written: [u8; 32] = bytes[pos + 16..pos + 48].try_into().unwrap();
        let mut h = Sha256::new();
        h.update(&bytes[..pos + 16]);
        h.update([0u8; 32]);
        h.update(&bytes[pos + 48..]);
        Ok((h.finalize().into(), written))
    }

    /// 복사할 바이트가 실행 중인 빌드와 같은지
    pub fn verify(bytes: &[u8]) -> Result<(), String> {
        let want = expected().ok_or("이 실행 파일에는 빌드 확인값이 없습니다(설치용으로 만든 파일이 아님)")?;
        let (got, written) = compute(bytes)?;
        if got != want || written != want {
            return Err("복사하려는 실행 파일이 지금 실행 중인 것과 다릅니다(UAC 창이 뜬 뒤 바뀌었을 수 있음) — 설치하지 않음".into());
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn patched(prefix: &[u8], suffix: &[u8]) -> Vec<u8> {
            let mut v = prefix.to_vec();
            v.extend_from_slice(&SELF_HASH);
            v.extend_from_slice(suffix);
            let (h, _) = compute(&v).unwrap();
            let pos = prefix.len();
            v[pos + 16..pos + 48].copy_from_slice(&h);
            v
        }

        #[test]
        fn hash_slot_rules() {
            let v = patched(b"MZ....code....", b"...more data...");
            let (h, written) = compute(&v).unwrap();
            assert_eq!(h, written, "써 넣은 값 = 자리를 0으로 둔 해시");
            // 한 바이트라도 바뀌면 다름
            let mut bad = v.clone();
            bad[3] ^= 1;
            let (h2, w2) = compute(&bad).unwrap();
            assert_ne!(h2, w2);
            // 표식이 없거나 두 번이면 거절
            assert!(compute(b"no marker here").is_err());
            let mut twice = v.clone();
            twice.extend_from_slice(&SELF_HASH);
            assert!(compute(&twice).is_err());
            // 시험 빌드(값을 넣지 않음)는 설치 거절
            assert!(expected().is_none());
            assert!(verify(&v).is_err());
        }
    }
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
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LUID};
    use windows_sys::Win32::Security::{
        AdjustTokenPrivileges, GetSecurityDescriptorDacl, LookupAccountNameW, LookupPrivilegeValueW, ACL, LUID_AND_ATTRIBUTES, TOKEN_PRIVILEGES,
    };
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    use windows_sys::Win32::System::Services::*;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, SetPriorityClass, WaitForSingleObject};

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

    /// `--report <파일>`: 화면에 찍는 줄을 파일에도(앱의 설치 단추가 UAC로 띄운 창 없는 실행의 결과를 읽으려고)
    static REPORT: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();

    macro_rules! say {
        ($($t:tt)*) => {{
            let s = format!($($t)*);
            println!("{s}");
            if let Some(m) = REPORT.get() {
                if let Some(f) = m.lock().unwrap().as_mut() {
                    let _ = writeln!(f, "{s}");
                }
            }
        }};
    }

    /// 0.3.3(보안 강화): `--report` 파일은 관리자 권한으로 만든다. 사용자 TEMP 아래라 같은 사용자의 프로그램이 그 경로에 링크나
    /// 접합점을 미리 둘 수 있으므로: 이미 있는 이름은 열지 않고(`CREATE_NEW` — 링크도 이름이 있으므로 실패), 마지막 항목의 재분석 지점을
    /// 따라가지 않으며(`FILE_FLAG_OPEN_REPARSE_POINT`), 만든 파일의 실제 위치(`GetFinalPathNameByHandleW`)가 준 경로와 다르면(경로 중간의
    /// 접합점) 그 핸들로 지우고 쓰지 않는다. 결과 파일이 없어도 설치는 계속한다(앱은 종료 코드로 성공·실패를 안다).
    fn open_report(p: &str) -> Option<std::fs::File> {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, GetLongPathNameW, SetFileInformationByHandle, FileDispositionInfo, FILE_DISPOSITION_INFO};
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const DELETE: u32 = 0x1_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        let path = Path::new(p);
        let f = std::fs::OpenOptions::new().access_mode(GENERIC_WRITE | DELETE).create_new(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(path).ok()?;
        let h = f.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
        let mut buf = vec![0u16; 1024];
        let n = unsafe { GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, 0) } as usize;
        let actual = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
        let actual = actual.strip_prefix(r"\\?\").unwrap_or(&actual).to_string();
        // 준 경로의 폴더를 긴 이름으로(8.3 짧은 이름 차이는 같은 곳)
        let want = path.parent().map(|d| {
            let w = wide_path(d);
            let mut lb = vec![0u16; 1024];
            let m = unsafe { GetLongPathNameW(w.as_ptr(), lb.as_mut_ptr(), lb.len() as u32) } as usize;
            let dir = if m > 0 && m < lb.len() { String::from_utf16_lossy(&lb[..m]) } else { d.display().to_string() };
            format!("{}\\{}", dir.trim_end_matches('\\'), path.file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default())
        });
        if n == 0 || want.as_deref().is_none_or(|w| !w.eq_ignore_ascii_case(&actual)) {
            let di = FILE_DISPOSITION_INFO { DeleteFile: true };
            unsafe { SetFileInformationByHandle(h, FileDispositionInfo, &di as *const _ as *const _, std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32) };
            eprintln!("--report 경로가 다른 곳을 가리킵니다({actual}) — 결과 파일을 쓰지 않음");
            return None;
        }
        Some(f)
    }

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
        if let Some(p) = args.iter().position(|a| a == "--report").and_then(|i| args.get(i + 1)) {
            let _ = REPORT.set(Mutex::new(open_report(p)));
        }
        let r = match cmd {
            "run" => return run_dispatcher(),
            "install" => install(account),
            "uninstall" => uninstall(),
            "status" => return status(),
            _ => {
                say!("filetool-svc {} — 파일 도구 빠른 검색 서비스", env!("CARGO_PKG_VERSION"));
                say!("  filetool-svc install [--account system|virtual] [--report 파일]   (관리자) 설치·시작. virtual은 시험용(볼륨을 열 수 없어 빠른 색인이 안 됨)");
                say!("  filetool-svc uninstall [--report 파일]             (관리자) 멈춤·제거(폴더까지)");
                say!("  filetool-svc status                                상태");
                return if cmd.is_empty() || cmd == "help" || cmd == "--help" { 0 } else { 1 };
            }
        };
        match r {
            Ok(()) => 0,
            Err(Fail::NotAdmin) => {
                say!("관리자 권한이 필요합니다. 관리자 권한으로 실행하세요(앱의 [설정] > 검색 > 빠른 검색 서비스 설치 단추는 UAC 창으로 권한을 얻습니다).");
                2
            }
            Err(Fail::Msg(m)) => {
                say!("오류: {m}");
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

    fn service_pid(s: &Sc) -> u32 {
        let mut st = SERVICE_STATUS_PROCESS::default();
        let mut need = 0u32;
        unsafe { QueryServiceStatusEx(s.0, SC_STATUS_PROCESS_INFO, &mut st as *mut _ as *mut u8, std::mem::size_of_val(&st) as u32, &mut need) };
        st.dwProcessId
    }

    fn stop_service(s: &Sc) -> Result<(), String> {
        if state(s) == SERVICE_STOPPED {
            return Ok(());
        }
        let pid = service_pid(s);
        let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        unsafe { ControlService(s.0, SERVICE_CONTROL_STOP, &mut st) };
        if !wait_state(s, SERVICE_STOPPED, Duration::from_secs(60)) {
            return Err("서비스가 60초 안에 멈추지 않았습니다".into());
        }
        // 0.2.1: SCM이 "멈춤"이라 해도 프로세스는 잠깐 더 살아 있다(실행 파일이 잠긴 채) → 끝날 때까지 기다린다.
        // 안 기다리면 다시 설치(갱신)에서 새 exe로 바꾸다 "액세스가 거부되었습니다(os error 5)"
        if pid != 0 {
            const SYNCHRONIZE: u32 = 0x0010_0000;
            let h = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
            if !h.is_null() {
                unsafe { WaitForSingleObject(h, 30_000) };
                unsafe { CloseHandle(h) };
            }
        }
        Ok(())
    }

    /// 0.3.3(보안 강화): 자기 실행 파일을 **쓰기·지우기 공유 없이** 한 번 열어 다 읽고, 빌드 때 넣은 확인값으로 실행 중인 것과 같은지 본다.
    /// (이 바이트를 그대로 복사한다 — 경로를 다시 열지 않음)
    fn read_own_image(src: &Path) -> Result<Vec<u8>, String> {
        use std::io::Read;
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        let mut f = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(src).map_err(|e| format!("{}: {e}", src.display()))?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", src.display()))?;
        crate::selfhash::verify(&bytes)?;
        Ok(bytes)
    }

    /// 새 exe를 옆에 써 두고 바꿔 넣는다. 막 멈춘 서비스·백신 검사가 잠깐 잡고 있을 수 있어 10초까지 다시 해 본다
    fn replace_exe(bytes: &[u8], dir: &Path, dst: &Path) -> Result<(), String> {
        let tmp = dir.join(format!("{EXE_NAME}.new"));
        std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
        let t0 = Instant::now();
        loop {
            match std::fs::rename(&tmp, dst) {
                Ok(()) => return Ok(()),
                Err(e) if t0.elapsed() < Duration::from_secs(10) && matches!(e.raw_os_error(), Some(5 | 32)) => std::thread::sleep(Duration::from_millis(250)),
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(format!("{}: {e}", dst.display()));
                }
            }
        }
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

    /// 토큰의 권한 켜기(백업·볼륨 관리 — 계정에 있어도 처음에는 꺼져 있음). 켠 이름들
    fn enable_privileges(names: &[&str]) -> Vec<String> {
        let mut tok: HANDLE = std::ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), 0x20 | 0x8, &mut tok) } == 0 {
            return Vec::new();
        }
        let mut on = Vec::new();
        for n in names {
            let w = wide(n);
            let mut luid = LUID { LowPart: 0, HighPart: 0 };
            if unsafe { LookupPrivilegeValueW(std::ptr::null(), w.as_ptr(), &mut luid) } == 0 {
                continue;
            }
            let tp = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: 0x2 }] };
            let ok = unsafe { AdjustTokenPrivileges(tok, 0, &tp, 0, std::ptr::null_mut(), std::ptr::null_mut()) } != 0;
            // ERROR_NOT_ALL_ASSIGNED(1300) = 계정에 그 권한이 없음
            if ok && last_error() == 0 {
                on.push(n.to_string());
            }
        }
        unsafe { CloseHandle(tok) };
        on
    }

    // ---------- install / uninstall ----------

    fn install(account: Account) -> Result<(), Fail> {
        let m = scm(SC_MANAGER_ALL_ACCESS)?;
        let src = std::env::current_exe().map_err(|e| e.to_string())?;
        let dir = install_dir();
        let dst = dir.join(EXE_NAME);
        say!("[1/6] 실행 파일: {} → {}", src.display(), dst.display());
        let existing = open_svc(&m, SERVICE_ALL_ACCESS)?;
        if let Some(s) = &existing {
            say!("      이미 설치됨 → 멈추고 새로 씀");
            stop_service(s)?;
        }
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        set_dacl(&dir, INSTALL_DIR_SDDL)?;
        let same = src.canonicalize().ok() == dst.canonicalize().ok();
        if !same {
            // 0.3.3: 복사할 내용을 서비스를 멈추기 전에 확인할 수 있으면 좋지만, 멈춘 뒤 실패하면 아래에서 다시 켠다
            let bytes = match read_own_image(&src) {
                Ok(b) => b,
                Err(e) => {
                    if let Some(s) = &existing {
                        let back = unsafe { StartServiceW(s.0, 0, std::ptr::null()) } != 0;
                        say!("      확인 실패 → 이전 서비스 {}", if back { "다시 시작함" } else { "다시 시작 못 함" });
                    }
                    return Err(e.into());
                }
            };
            if let Err(e) = replace_exe(&bytes, &dir, &dst) {
                // 갱신하다 실패하면 멈춰 둔 이전 서비스를 다시 켜 둔다(멈춘 채로 두지 않음)
                if let Some(s) = &existing {
                    let back = unsafe { StartServiceW(s.0, 0, std::ptr::null()) } != 0;
                    say!("      바꾸지 못함 → 이전 서비스 {}", if back { "다시 시작함" } else { "다시 시작 못 함" });
                }
                return Err(e.into());
            }
        }
        set_dacl(&dir, INSTALL_DIR_SDDL)?;

        say!("[2/6] 서비스 등록: {SERVICE_NAME} ({DISPLAY_NAME}), 계정 {}", if account == Account::System { "LocalSystem" } else { VIRTUAL_ACCOUNT });
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
        say!("[3/6] 자료 폴더: {} (SYSTEM·관리자만)", data.display());
        let mut data_sddl = DATA_DIR_SDDL.to_string();
        let mut svc_sid = None;
        if account == Account::Virtual {
            let (mut sid, ss) = account_sid(VIRTUAL_ACCOUNT)?;
            say!("      가상 계정 {VIRTUAL_ACCOUNT} = {ss} → 권한 {}", VIRTUAL_RIGHTS.join(", "));
            lsa_rights(&mut sid, true)?;
            data_sddl.push_str(&format!("(A;OICI;FA;;;{ss})"));
            svc_sid = Some(ss);
        }
        prepare_data_dir(&data, &data_sddl)?;
        if let Some(ss) = &svc_sid {
            set_dacl(&dir, &format!("{INSTALL_DIR_SDDL}(A;OICI;0x1200a9;;;{ss})"))?;
        }

        say!("[4/6] 시작");
        if unsafe { StartServiceW(s.0, 0, std::ptr::null()) } == 0 {
            let e = last_error();
            if e != 1056 {
                return Err(format!("서비스를 시작할 수 없습니다(오류 {e})").into());
            }
        }
        if !wait_state(&s, SERVICE_RUNNING, Duration::from_secs(30)) {
            return Err(format!("서비스가 30초 안에 실행되지 않았습니다(상태 {})", svcpipe::state_label(state(&s))).into());
        }
        say!("[5/6] 파이프 확인");
        let t0 = Instant::now();
        let mut last = String::new();
        while t0.elapsed() < Duration::from_secs(20) {
            match svcpipe::connect("filetool-svc install").and_then(|mut c| c.status()) {
                Ok(st) => {
                    for l in describe_status(&st) {
                        say!("      {l}");
                    }
                    say!("[6/6] 설치 끝({:.1} 초 뒤 파이프 응답)", t0.elapsed().as_secs_f64());
                    return Ok(());
                }
                Err(e) => last = e,
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        Err(format!("서비스는 실행 중이지만 파이프가 답하지 않습니다: {last}").into())
    }

    /// 자료 폴더 준비(0.3.3 보안 강화).
    ///
    /// 서비스를 처음 설치하기 전에는 일반 사용자도 `C:\ProgramData\kr.salarycrew.winfiletool`을 만들 수 있다. 그것이 다른 폴더를
    /// 가리키는 접합점이면 경로로 소유자를 읽고(`GetNamedSecurityInfoW`) 경로로 DACL을 바꾸는(`SetNamedSecurityInfoW`) 방식은
    /// **대상** 폴더를 검사·변경한다. 그래서:
    /// - 재분석 지점을 따라가지 않고 연다(`FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS`), 접합점·링크면 링크만 지운다
    /// - 소유자 검사와 DACL 설정을 **같은 핸들**로(`GetSecurityInfo`/`SetSecurityInfo`)
    /// - 새로 만들 때는 `CreateDirectoryW`에 보안 설명자를 넘겨 만드는 순간부터 SYSTEM·관리자만
    fn prepare_data_dir(data: &Path, sddl: &str) -> Result<(), String> {
        use filetool_core::secdir::{create_dir_with_sddl, data_dir_action, open_dir_nofollow, DataDirAction, Opened};
        let err = |e: std::io::Error| format!("{}: {e}", data.display());
        // 지우고 새로 만드는 사이에 누군가 다시 끼워 넣으면(경쟁) 몇 번만 다시 해 보고 멈춘다
        for _ in 0..4 {
            match open_dir_nofollow(data, true).map_err(err)? {
                Opened::NotFound => match create_dir_with_sddl(data, sddl) {
                    Ok(()) => say!("      새로 만듦(만들 때부터 SYSTEM·관리자만)"),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(err(e)),
                },
                Opened::Reparse => {
                    say!("      접합점·링크로 된 폴더 → 따라가지 않고 링크만 지움");
                    // 디렉터리 접합점·디렉터리 링크는 RemoveDirectoryW로 링크만 지워진다(대상은 그대로)
                    if let Err(e) = std::fs::remove_dir(data) {
                        // 파일 링크면
                        std::fs::remove_file(data).map_err(|_| err(e))?;
                    }
                    continue;
                }
                Opened::Dir(h) => {
                    let (owner, _) = h.owner_and_aces().map_err(err)?;
                    match data_dir_action(false, &owner) {
                        DataDirAction::Keep => {
                            h.set_protected_dacl(sddl).map_err(|e| format!("{}: 권한을 설정할 수 없습니다({e})", data.display()))?;
                            std::fs::create_dir_all(service_index_dir(data)).map_err(|e| e.to_string())?;
                            return Ok(());
                        }
                        _ => {
                            say!("      소유자가 SYSTEM·관리자가 아닌 폴더({owner}) → 지우고 새로 만듦");
                            drop(h);
                            // remove_dir_all은 안의 접합점·링크를 따라가지 않고 링크만 지운다(Rust 1.58+)
                            std::fs::remove_dir_all(data).map_err(err)?;
                            continue;
                        }
                    }
                }
            }
        }
        Err(format!("{}: 자료 폴더를 안전하게 준비하지 못했습니다(다른 프로그램이 계속 바꿈)", data.display()))
    }

    fn uninstall() -> Result<(), Fail> {
        let m = scm(SC_MANAGER_ALL_ACCESS)?;
        match open_svc(&m, SERVICE_ALL_ACCESS)? {
            Some(s) => {
                let virt = svcpipe::service_info().account.eq_ignore_ascii_case(VIRTUAL_ACCOUNT);
                say!("[1/4] 서비스 멈춤");
                stop_service(&s)?;
                say!("[2/4] 등록 해제");
                if unsafe { DeleteService(s.0) } == 0 {
                    let e = last_error();
                    if e != 1072 {
                        return Err(format!("서비스를 지울 수 없습니다(오류 {e})").into());
                    }
                }
                drop(s);
                if virt {
                    if let Ok((mut sid, ss)) = account_sid(VIRTUAL_ACCOUNT) {
                        say!("      가상 계정 {ss} 권한 거둠");
                        lsa_rights(&mut sid, false)?;
                    }
                }
                let t0 = Instant::now();
                while t0.elapsed() < Duration::from_secs(10) && open_svc(&m, SERVICE_QUERY_STATUS)?.is_some() {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
            None => say!("[1/4] 서비스 없음(이미 제거됨)"),
        }
        let data = service_data_dir();
        say!("[3/4] 자료 폴더 지움: {}", data.display());
        if data.exists() {
            std::fs::remove_dir_all(&data).map_err(|e| format!("{}: {e}", data.display()))?;
        }
        let dir = install_dir();
        say!("[4/4] 서비스 폴더 지움: {}", dir.display());
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
                say!("      실행 중인 {EXE_NAME}는 다음 재부팅 때 지워집니다(폴더째). 앱 폴더의 filetool-svc.exe로 제거하면 바로 지워집니다");
            } else {
                std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            }
        }
        say!("제거 끝");
        Ok(())
    }

    fn status() -> i32 {
        let info = svcpipe::service_info();
        say!("서비스 {SERVICE_NAME}: {}", if info.installed { "설치됨" } else { "설치 안 됨" });
        if info.installed {
            say!("  상태 {} · pid {} · 시작 {} · 계정 {}", info.state, info.pid, info.start_type, if info.account.is_empty() { "?" } else { &info.account });
            say!("  실행 파일 {}", info.binary_path);
        }
        let dir = install_dir();
        let data = service_data_dir();
        for (what, p) in [("서비스 폴더", dir.to_string_lossy().into_owned()), ("자료 폴더", data.to_string_lossy().into_owned()), ("파이프", PIPE_NAME.to_string())] {
            if what != "파이프" && !Path::new(&p).exists() {
                say!("  {what} {p}: 없음");
                continue;
            }
            match sddl_of(&p) {
                Ok(s) => say!("  {what} {p}: {s}"),
                Err(e) => say!("  {what} {p}: ACL 읽기 실패({e})"),
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
                say!("  호출자 {} ({}) 관리자 {} · {}", me.user, me.sid, me.admin, me.visibility);
                for l in describe_status(&st) {
                    say!("  {l}");
                }
                0
            }
            Err(e) => {
                say!("  파이프: {e}");
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
            // 처음 전체 읽기(저장한 색인이 없을 때)는 낮은 I/O·CPU 우선순위(배경 모드). 준비되면 끈다.
            // 저장한 색인이 있으면(다시 시작) 불러오기 + 저널 따라잡기만이라 보통 우선순위(배경 모드에서는 111 MB 불러오기가 13 초 걸렸음)
            let first = !idx.join(filetool_core::search::index::INDEX_FILE).exists();
            let bg = first && unsafe { SetPriorityClass(GetCurrentProcess(), PROCESS_MODE_BACKGROUND_BEGIN) } != 0;
            log(format!("배경 모드(낮은 I/O 우선순위): {}", if bg { "켬(처음 전체 읽기)" } else if first { "실패" } else { "끔(저장한 색인 불러오기)" }));
            log(format!("켠 권한: {:?}", enable_privileges(&["SeBackupPrivilege", "SeManageVolumePrivilege"])));
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
