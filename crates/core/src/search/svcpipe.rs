//! 2c: 빠른 검색 서비스의 이름 있는 파이프 — Windows 쪽(서버·클라이언트·호출자 확인·서비스 조회).
//!
//! - 파이프 DACL(SDDL [`PIPE_SDDL`]): SYSTEM·관리자 전체, **대화형 사용자(IU)는 읽기·쓰기만**
//!   (`FILE_CREATE_PIPE_INSTANCE`는 주지 않음 → 일반 사용자가 같은 이름의 인스턴스를 더 만들 수 없음).
//!   `PIPE_REJECT_REMOTE_CLIENTS`로 원격 연결은 받지 않는다. 첫 인스턴스는 `FILE_FLAG_FIRST_PIPE_INSTANCE`(이름을 먼저 차지한 다른 프로세스가 있으면 실패).
//! - 호출자: 첫 요청을 읽은 뒤 `ImpersonateNamedPipeClient` → 스레드 토큰(사용자 SID·그룹) → `RevertToSelf`.
//!   클라이언트는 `SECURITY_IDENTIFICATION`으로 열어 서비스가 호출자 **확인만** 할 수 있게 한다(대신 행동 불가).
//!   관리자 = Administrators 그룹이 켜져 있음(권한 상승 안 된 관리자 계정의 토큰은 그 그룹이 deny-only라 일반 사용자로 본다).
//! - 클라이언트는 연결한 뒤 `GetNamedPipeServerProcessId`가 SCM이 아는 서비스 프로세스 번호와 같은지 확인한다(가짜 서버 방지).
//! - 클라이언트 읽기·쓰기는 겹친 I/O + 시간 제한([`CLIENT_TIMEOUT`]) — 서비스가 멈춰도 앱이 멈추지 않는다.

use std::time::Duration;

#[cfg(not(windows))]
use super::svcproto::Client;

/// 파이프 보안: SYSTEM·관리자 전체, 대화형 사용자(IU) 읽기/쓰기(인스턴스 만들기 제외). 그 밖(네트워크·일괄 로그온의 일반 사용자 등)은 없음.
/// 원격(SMB) 연결은 `PIPE_REJECT_REMOTE_CLIENTS`로 막는다. 네트워크 로그온 SID(NU) 거부는 두지 않았다 — 로컬 ssh 세션이
/// 네트워크 로그온이라 관리자 ssh도 막히기 때문(관리자는 어차피 볼륨을 직접 읽을 수 있음). 일반 사용자의 ssh 세션에는 IU가 없어 막힌다.
pub const PIPE_SDDL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019b;;;IU)";
/// 클라이언트 요청 하나의 시간 제한
pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

/// 서비스(SCM) 정보
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ServiceInfo {
    pub installed: bool,
    /// "실행 중" / "멈춤" / …
    pub state: String,
    pub running: bool,
    pub pid: u32,
    /// "자동(지연된 시작)" / "자동" / "수동" / "사용 안 함"
    pub start_type: String,
    pub account: String,
    pub binary_path: String,
}

#[cfg(windows)]
pub use win::*;

/// 서비스 클라이언트(파이프)
#[cfg(not(windows))]
pub type SvcClient = Client<std::fs::File>;

#[cfg(not(windows))]
pub fn connect(_client: &str) -> Result<SvcClient, String> {
    Err("빠른 검색 서비스는 Windows에서만 씁니다".into())
}

#[cfg(not(windows))]
pub fn service_info() -> ServiceInfo {
    ServiceInfo::default()
}

#[cfg(windows)]
mod win {
    use super::*;
    use crate::search::svcproto::{self, Client, Handler, Identity, Session, Visibility, PIPE_NAME};
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, LocalFree, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW};
    use windows_sys::Win32::Security::{GetTokenInformation, LookupAccountSidW, RevertToSelf, SECURITY_ATTRIBUTES, TOKEN_GROUPS, TOKEN_USER};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FlushFileBuffers, ReadFile, WriteFile, OPEN_EXISTING};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId, ImpersonateNamedPipeClient,
        WaitNamedPipeW,
    };
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceConfig2W, QueryServiceConfigW, QueryServiceStatusEx, QUERY_SERVICE_CONFIGW,
        SERVICE_STATUS_PROCESS,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, GetCurrentThread, OpenProcess, OpenProcessToken, OpenThreadToken, WaitForSingleObject};
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

    const PIPE_ACCESS_DUPLEX: u32 = 0x3;
    const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
    const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
    const PIPE_TYPE_BYTE: u32 = 0;
    const PIPE_READMODE_BYTE: u32 = 0;
    const PIPE_WAIT: u32 = 0;
    const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x8;
    const SECURITY_SQOS_PRESENT: u32 = 0x0010_0000;
    const SECURITY_IDENTIFICATION: u32 = 1 << 16;
    const FILE_READ_DATA: u32 = 0x1;
    const FILE_WRITE_DATA: u32 = 0x2;
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const TOKEN_QUERY: u32 = 0x8;
    const TOKEN_USER_CLASS: i32 = 1;
    const TOKEN_GROUPS_CLASS: i32 = 2;
    const SE_GROUP_ENABLED: u32 = 0x4;
    const SE_GROUP_USE_FOR_DENY_ONLY: u32 = 0x10;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const ERROR_PIPE_CONNECTED: u32 = 535;
    const ERROR_PIPE_BUSY: u32 = 231;
    const ERROR_BROKEN_PIPE: u32 = 109;
    const ERROR_NO_DATA: u32 = 232;
    const ERROR_IO_PENDING: u32 = 997;
    const ERROR_FILE_NOT_FOUND: u32 = 2;
    const ERROR_SERVICE_DOES_NOT_EXIST: u32 = 1060;
    const WAIT_OBJECT_0: u32 = 0;
    const SC_MANAGER_CONNECT: u32 = 0x1;
    const SERVICE_QUERY_CONFIG: u32 = 0x1;
    const SERVICE_QUERY_STATUS: u32 = 0x4;
    const SC_STATUS_PROCESS_INFO: i32 = 0;
    const SERVICE_CONFIG_DELAYED_AUTO_START_INFO: u32 = 3;
    const SERVICE_RUNNING: u32 = 4;

    pub fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
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

    // ---------- 서비스 정보(SCM) ----------

    struct Sc(windows_sys::Win32::System::Services::SC_HANDLE);
    impl Drop for Sc {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CloseServiceHandle(self.0) };
            }
        }
    }

    fn open_service(access: u32) -> Result<(Sc, Sc), u32> {
        let m = Sc(unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT) });
        if m.0.is_null() {
            return Err(unsafe { GetLastError() });
        }
        let n = wide(svcproto::SERVICE_NAME);
        let s = Sc(unsafe { OpenServiceW(m.0, n.as_ptr(), access) });
        if s.0.is_null() {
            return Err(unsafe { GetLastError() });
        }
        Ok((m, s))
    }

    fn query_status(s: &Sc) -> Option<SERVICE_STATUS_PROCESS> {
        let mut st = SERVICE_STATUS_PROCESS::default();
        let mut need = 0u32;
        let ok = unsafe {
            QueryServiceStatusEx(s.0, SC_STATUS_PROCESS_INFO, &mut st as *mut _ as *mut u8, std::mem::size_of::<SERVICE_STATUS_PROCESS>() as u32, &mut need)
        };
        (ok != 0).then_some(st)
    }

    /// 서비스 프로세스 번호(실행 중일 때)
    pub fn service_pid() -> Option<u32> {
        let (_m, s) = open_service(SERVICE_QUERY_STATUS).ok()?;
        let st = query_status(&s)?;
        (st.dwCurrentState == SERVICE_RUNNING && st.dwProcessId != 0).then_some(st.dwProcessId)
    }

    pub fn state_label(state: u32) -> &'static str {
        match state {
            1 => "멈춤",
            2 => "시작 중",
            3 => "멈추는 중",
            4 => "실행 중",
            5 => "계속하는 중",
            6 => "일시 정지 중",
            7 => "일시 정지됨",
            _ => "알 수 없음",
        }
    }

    /// 설치 여부·상태·시작 유형·계정·실행 파일(일반 사용자도 읽을 수 있음)
    pub fn service_info() -> ServiceInfo {
        let (_m, s) = match open_service(SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS) {
            Ok(x) => x,
            Err(ERROR_SERVICE_DOES_NOT_EXIST) => return ServiceInfo { state: "설치 안 됨".into(), ..Default::default() },
            Err(e) => return ServiceInfo { state: format!("알 수 없음(오류 {e})"), ..Default::default() },
        };
        let mut info = ServiceInfo { installed: true, ..Default::default() };
        if let Some(st) = query_status(&s) {
            info.state = state_label(st.dwCurrentState).into();
            info.running = st.dwCurrentState == SERVICE_RUNNING;
            info.pid = st.dwProcessId;
        }
        let mut need = 0u32;
        unsafe { QueryServiceConfigW(s.0, std::ptr::null_mut(), 0, &mut need) };
        if need > 0 {
            let mut buf = vec![0u64; (need as usize).div_ceil(8)];
            let p = buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
            if unsafe { QueryServiceConfigW(s.0, p, need, &mut need) } != 0 {
                let c = unsafe { &*p };
                info.binary_path = from_wide_ptr(c.lpBinaryPathName);
                info.account = from_wide_ptr(c.lpServiceStartName);
                info.start_type = match c.dwStartType {
                    2 => "자동",
                    3 => "수동",
                    4 => "사용 안 함",
                    _ => "기타",
                }
                .into();
            }
        }
        if info.start_type == "자동" {
            let mut d = [0u32; 1];
            let mut need = 0u32;
            if unsafe { QueryServiceConfig2W(s.0, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, d.as_mut_ptr() as *mut u8, 4, &mut need) } != 0 && d[0] != 0 {
                info.start_type = "자동(지연된 시작)".into();
            }
        }
        info
    }

    // ---------- 클라이언트 ----------

    /// 겹친 I/O 파이프(시간 제한)
    pub struct PipeStream {
        h: HANDLE,
        ev: HANDLE,
        timeout: Duration,
    }
    unsafe impl Send for PipeStream {}

    impl Drop for PipeStream {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.ev);
                CloseHandle(self.h);
            }
        }
    }

    impl PipeStream {
        fn io(&mut self, read: bool, buf: *mut u8, len: u32) -> std::io::Result<usize> {
            let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
            ov.hEvent = self.ev;
            let ok = unsafe { if read { ReadFile(self.h, buf, len, std::ptr::null_mut(), &mut ov) } else { WriteFile(self.h, buf, len, std::ptr::null_mut(), &mut ov) } };
            if ok == 0 {
                let e = unsafe { GetLastError() };
                if e == ERROR_BROKEN_PIPE || e == ERROR_NO_DATA {
                    return if read { Ok(0) } else { Err(std::io::ErrorKind::BrokenPipe.into()) };
                }
                if e != ERROR_IO_PENDING {
                    return Err(std::io::Error::from_raw_os_error(e as i32));
                }
                let w = unsafe { WaitForSingleObject(self.ev, self.timeout.as_millis() as u32) };
                if w != WAIT_OBJECT_0 {
                    let mut n = 0u32;
                    unsafe {
                        CancelIoEx(self.h, &ov);
                        GetOverlappedResult(self.h, &ov, &mut n, 1);
                    }
                    return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "서비스가 제때 답하지 않았습니다"));
                }
            }
            let mut n = 0u32;
            if unsafe { GetOverlappedResult(self.h, &ov, &mut n, 1) } == 0 {
                let e = unsafe { GetLastError() };
                if e == ERROR_BROKEN_PIPE || e == ERROR_NO_DATA {
                    return if read { Ok(0) } else { Err(std::io::ErrorKind::BrokenPipe.into()) };
                }
                return Err(std::io::Error::from_raw_os_error(e as i32));
            }
            Ok(n as usize)
        }
    }

    impl Read for PipeStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let len = buf.len().min(1 << 20) as u32;
            self.io(true, buf.as_mut_ptr(), len)
        }
    }

    impl Write for PipeStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let len = buf.len().min(1 << 20) as u32;
            self.io(false, buf.as_ptr() as *mut u8, len)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// 서비스 클라이언트(파이프)
    pub type SvcClient = Client<PipeStream>;

    /// 서비스에 연결(파이프가 없으면 바로 실패). 연결한 상대가 SCM의 서비스 프로세스인지 확인한다.
    pub fn connect(client: &str) -> Result<SvcClient, String> {
        let name = wide(PIPE_NAME);
        let access = FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE;
        let flags = FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION;
        let mut h = INVALID_HANDLE_VALUE;
        for attempt in 0..2 {
            h = unsafe { CreateFileW(name.as_ptr(), access, 0, std::ptr::null(), OPEN_EXISTING, flags, std::ptr::null_mut()) };
            if h != INVALID_HANDLE_VALUE {
                break;
            }
            let e = unsafe { GetLastError() };
            match e {
                ERROR_FILE_NOT_FOUND => return Err("빠른 검색 서비스가 실행 중이 아닙니다(파이프 없음)".into()),
                ERROR_PIPE_BUSY if attempt == 0 => {
                    unsafe { WaitNamedPipeW(name.as_ptr(), 2000) };
                }
                _ => return Err(format!("서비스 파이프를 열 수 없습니다(오류 {e})")),
            }
        }
        if h == INVALID_HANDLE_VALUE {
            return Err("서비스가 바쁩니다(연결 수 한도)".into());
        }
        let ev = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        let stream = PipeStream { h, ev, timeout: CLIENT_TIMEOUT };
        // 가짜 서버(같은 이름을 먼저 차지한 프로세스)에 붙지 않도록: 파이프 서버 = SCM이 아는 서비스 프로세스
        let mut server_pid = 0u32;
        if unsafe { GetNamedPipeServerProcessId(h, &mut server_pid) } == 0 {
            return Err(format!("파이프 서버를 확인할 수 없습니다(오류 {})", unsafe { GetLastError() }));
        }
        match service_pid() {
            Some(p) if p == server_pid => {}
            Some(p) => return Err(format!("파이프를 연 프로세스({server_pid})가 서비스({p})가 아닙니다 — 연결하지 않음")),
            None => return Err(format!("서비스가 실행 중이 아닌데 파이프가 있습니다(pid {server_pid}) — 연결하지 않음")),
        }
        Client::handshake(stream, client)
    }

    // ---------- 서버 ----------

    struct ServerPipe(HANDLE);
    unsafe impl Send for ServerPipe {}

    impl Read for ServerPipe {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut n = 0u32;
            let ok = unsafe { ReadFile(self.0, buf.as_mut_ptr(), buf.len().min(1 << 20) as u32, &mut n, std::ptr::null_mut()) };
            if ok == 0 {
                let e = unsafe { GetLastError() };
                if e == ERROR_BROKEN_PIPE || e == ERROR_NO_DATA {
                    return Ok(0);
                }
                return Err(std::io::Error::from_raw_os_error(e as i32));
            }
            Ok(n as usize)
        }
    }

    impl Write for ServerPipe {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let mut n = 0u32;
            let ok = unsafe { WriteFile(self.0, buf.as_ptr(), buf.len().min(1 << 20) as u32, &mut n, std::ptr::null_mut()) };
            if ok == 0 {
                return Err(std::io::Error::from_raw_os_error(unsafe { GetLastError() } as i32));
            }
            Ok(n as usize)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct SecDesc(*mut core::ffi::c_void);
    unsafe impl Send for SecDesc {}
    unsafe impl Sync for SecDesc {}
    impl Drop for SecDesc {
        fn drop(&mut self) {
            unsafe { LocalFree(self.0) };
        }
    }

    fn security_descriptor(sddl: &str) -> Result<SecDesc, String> {
        let w = wide(sddl);
        let mut sd: *mut core::ffi::c_void = std::ptr::null_mut();
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(w.as_ptr(), 1, &mut sd, std::ptr::null_mut()) } == 0 {
            return Err(format!("보안 설명자를 만들 수 없습니다(오류 {})", unsafe { GetLastError() }));
        }
        Ok(SecDesc(sd))
    }

    /// 파이프 서버. 멈추려면 [`PipeServer::stop`]
    pub struct PipeServer {
        stop: Arc<AtomicBool>,
        pub clients: Arc<AtomicUsize>,
    }

    /// 서버 기록(서비스 로그로)
    pub type Log = Arc<dyn Fn(String) + Send + Sync>;

    impl PipeServer {
        /// `clients` = 지금 붙어 있는 연결 수(상태 응답에 씀)
        pub fn start(handler: Arc<Handler>, clients: Arc<AtomicUsize>, log: Log) -> Result<PipeServer, String> {
            let sd = Arc::new(security_descriptor(PIPE_SDDL)?);
            let stop = Arc::new(AtomicBool::new(false));
            let (s2, c2) = (stop.clone(), clients.clone());
            std::thread::Builder::new()
                .name("pipe-accept".into())
                .spawn(move || accept_loop(handler, sd, s2, c2, log))
                .map_err(|e| e.to_string())?;
            Ok(PipeServer { stop, clients })
        }

        /// 새 연결을 그만 받는다(붙어 있는 연결은 프로세스가 끝날 때 끊긴다)
        pub fn stop(&self) {
            self.stop.store(true, Ordering::SeqCst);
            // ConnectNamedPipe에서 기다리는 accept 스레드를 깨운다
            let name = wide(PIPE_NAME);
            let h = unsafe { CreateFileW(name.as_ptr(), FILE_READ_DATA | FILE_WRITE_DATA, 0, std::ptr::null(), OPEN_EXISTING, SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION, std::ptr::null_mut()) };
            if h != INVALID_HANDLE_VALUE {
                unsafe { CloseHandle(h) };
            }
        }
    }

    fn accept_loop(handler: Arc<Handler>, sd: Arc<SecDesc>, stop: Arc<AtomicBool>, clients: Arc<AtomicUsize>, log: Log) {
        let name = wide(PIPE_NAME);
        let mut first = true;
        while !stop.load(Ordering::SeqCst) {
            let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: 0 };
            let open = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
            let h = unsafe {
                CreateNamedPipeW(
                    name.as_ptr(),
                    open,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    svcproto::MAX_CLIENTS as u32,
                    64 * 1024,
                    64 * 1024,
                    0,
                    &sa,
                )
            };
            if h == INVALID_HANDLE_VALUE {
                let e = unsafe { GetLastError() };
                if first {
                    log(format!("파이프를 만들 수 없습니다(오류 {e}) — 같은 이름을 다른 프로세스가 차지했을 수 있음, 5초 뒤 다시"));
                    std::thread::sleep(Duration::from_secs(5));
                } else {
                    // 인스턴스 한도(ERROR_PIPE_BUSY) 등: 잠깐 뒤 다시
                    std::thread::sleep(Duration::from_millis(200));
                }
                continue;
            }
            if first {
                log(format!("파이프 대기: {PIPE_NAME} ({})", PIPE_SDDL));
            }
            first = false;
            let ok = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if !ok || stop.load(Ordering::SeqCst) {
                unsafe {
                    DisconnectNamedPipe(h);
                    CloseHandle(h);
                }
                continue;
            }
            let (handler, clients, log) = (handler.clone(), clients.clone(), log.clone());
            let pipe = ServerPipe(h);
            let _ = std::thread::Builder::new().name("pipe-conn".into()).spawn(move || {
                clients.fetch_add(1, Ordering::SeqCst);
                serve_conn(pipe, &handler, &log);
                clients.fetch_sub(1, Ordering::SeqCst);
            });
        }
    }

    fn serve_conn(mut pipe: ServerPipe, handler: &Handler, log: &Log) {
        let h = pipe.0;
        // 첫 요청을 읽은 뒤(그래야 흉내 내기가 됨) 호출자를 확인
        let first = svcproto::read_frame(&mut pipe, svcproto::MAX_REQUEST_BYTES);
        let mut pid = 0u32;
        unsafe { GetNamedPipeClientProcessId(h, &mut pid) };
        let ident = identify(h, pid);
        match (first, ident) {
            (Ok(Some(frame)), Ok(id)) => {
                let (profiles_dir, list) = profile_list();
                let vis = Visibility::for_identity(&id, &profiles_dir, &list);
                log(format!("연결: pid {pid} {} ({}) 관리자 {} · {}", id.user, id.sid, id.admin, vis.describe()));
                let mut s = Session::new(id, vis);
                let resp = match svcproto::decode_request(&frame) {
                    Ok(req) => handler.handle(&mut s, req),
                    Err(e) => e,
                };
                if svcproto::write_frame(&mut pipe, &svcproto::encode_response(&resp)).is_ok() {
                    let _ = svcproto::serve(&mut pipe, handler, &mut s);
                }
                log(format!("연결 끝: pid {pid} 요청 {}", s.requests));
            }
            (Ok(Some(_)), Err(e)) => {
                log(format!("호출자 확인 실패(pid {pid}): {e}"));
                let r = svcproto::Response::error("identity", format!("호출자를 확인할 수 없습니다: {e}"));
                let _ = svcproto::write_frame(&mut pipe, &svcproto::encode_response(&r));
            }
            (Err(svcproto::FrameErr::TooLarge(n)), _) => {
                let r = svcproto::Response::error("limit", format!("요청이 너무 큽니다({n} 바이트)"));
                let _ = svcproto::write_frame(&mut pipe, &svcproto::encode_response(&r));
            }
            _ => {}
        }
        unsafe {
            FlushFileBuffers(h);
            DisconnectNamedPipe(h);
            CloseHandle(h);
        }
    }

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    fn token_info(tok: HANDLE, class: i32) -> Result<Vec<u64>, String> {
        let mut need = 0u32;
        unsafe { GetTokenInformation(tok, class, std::ptr::null_mut(), 0, &mut need) };
        if need == 0 {
            return Err(format!("토큰 정보 크기(오류 {})", unsafe { GetLastError() }));
        }
        let mut buf = vec![0u64; (need as usize).div_ceil(8)];
        if unsafe { GetTokenInformation(tok, class, buf.as_mut_ptr() as *mut _, need, &mut need) } == 0 {
            return Err(format!("토큰 정보(오류 {})", unsafe { GetLastError() }));
        }
        Ok(buf)
    }

    fn sid_string(sid: *mut core::ffi::c_void) -> String {
        let mut p: *mut u16 = std::ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(sid, &mut p) } == 0 {
            return String::new();
        }
        let s = from_wide_ptr(p);
        unsafe { LocalFree(p as _) };
        s
    }

    fn account_name(sid: *mut core::ffi::c_void) -> String {
        let mut name = [0u16; 256];
        let mut dom = [0u16; 256];
        let (mut nl, mut dl, mut use_) = (256u32, 256u32, 0i32);
        if unsafe { LookupAccountSidW(std::ptr::null(), sid, name.as_mut_ptr(), &mut nl, dom.as_mut_ptr(), &mut dl, &mut use_) } == 0 {
            return String::new();
        }
        format!("{}\\{}", String::from_utf16_lossy(&dom[..dl as usize]), String::from_utf16_lossy(&name[..nl as usize]))
    }

    /// 토큰에서 사용자·관리자 여부
    fn identity_from_token(tok: HANDLE) -> Result<Identity, String> {
        let u = token_info(tok, TOKEN_USER_CLASS)?;
        let tu = unsafe { &*(u.as_ptr() as *const TOKEN_USER) };
        let sid = sid_string(tu.User.Sid);
        let user = account_name(tu.User.Sid);
        let g = token_info(tok, TOKEN_GROUPS_CLASS)?;
        let tg = unsafe { &*(g.as_ptr() as *const TOKEN_GROUPS) };
        let groups = unsafe { std::slice::from_raw_parts(tg.Groups.as_ptr(), tg.GroupCount as usize) };
        let admin_group = groups.iter().any(|ga| {
            ga.Attributes & SE_GROUP_ENABLED != 0 && ga.Attributes & SE_GROUP_USE_FOR_DENY_ONLY == 0 && sid_string(ga.Sid) == "S-1-5-32-544"
        });
        let admin = admin_group || sid == "S-1-5-18";
        let profile = profile_of(&sid);
        Ok(Identity { user, sid, admin, profile })
    }

    /// 호출자 확인: 흉내 내기 토큰, 안 되면 클라이언트 프로세스 토큰
    fn identify(h: HANDLE, pid: u32) -> Result<Identity, String> {
        if unsafe { ImpersonateNamedPipeClient(h) } != 0 {
            let mut tok: HANDLE = std::ptr::null_mut();
            let ok = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut tok) } != 0;
            unsafe { RevertToSelf() };
            if ok {
                let tok = Handle(tok);
                return identity_from_token(tok.0);
            }
        }
        if pid == 0 {
            return Err("클라이언트 프로세스를 알 수 없습니다".into());
        }
        let p = Handle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) });
        if p.0.is_null() {
            return Err(format!("클라이언트 프로세스를 열 수 없습니다(오류 {})", unsafe { GetLastError() }));
        }
        let mut tok: HANDLE = std::ptr::null_mut();
        if unsafe { OpenProcessToken(p.0, TOKEN_QUERY, &mut tok) } == 0 {
            return Err(format!("클라이언트 토큰을 열 수 없습니다(오류 {})", unsafe { GetLastError() }));
        }
        let tok = Handle(tok);
        identity_from_token(tok.0)
    }

    const PROFILE_LIST: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList";

    /// `%SystemDrive%` 같은 환경 변수 풀기(레지스트리 REG_EXPAND_SZ)
    fn expand(s: &str) -> String {
        let mut out = String::new();
        let mut rest = s;
        while let Some(a) = rest.find('%') {
            out.push_str(&rest[..a]);
            let after = &rest[a + 1..];
            match after.find('%') {
                Some(b) => {
                    let var = &after[..b];
                    match std::env::var(var) {
                        Ok(v) => out.push_str(&v),
                        Err(_) => {
                            out.push('%');
                            out.push_str(var);
                            out.push('%');
                        }
                    }
                    rest = &after[b + 1..];
                }
                None => {
                    out.push('%');
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }

    fn profile_of(sid: &str) -> Option<String> {
        let k = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE).open_subkey(format!(r"{PROFILE_LIST}\{sid}")).ok()?;
        let p: String = k.get_value("ProfileImagePath").ok()?;
        Some(expand(&p))
    }

    /// (프로필 폴더들의 부모, 등록된 모든 프로필 경로)
    pub fn profile_list() -> (String, Vec<String>) {
        let mut dir = String::from(r"C:\Users");
        let mut list = Vec::new();
        if let Ok(k) = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE).open_subkey(PROFILE_LIST) {
            if let Ok(d) = k.get_value::<String, _>("ProfilesDirectory") {
                dir = expand(&d);
            }
            for name in k.enum_keys().flatten() {
                if let Ok(sk) = k.open_subkey(&name) {
                    if let Ok(p) = sk.get_value::<String, _>("ProfileImagePath") {
                        list.push(expand(&p));
                    }
                }
            }
        }
        (dir, list)
    }
}
