//! 0.3.3(보안 강화): 폴더를 **재분석 지점(접합점·심볼릭 링크)을 따라가지 않고** 열기, 열린 핸들로 소유자·DACL 읽고 바꾸기,
//! 만드는 순간부터 보호된 폴더 만들기, "이 사용자만 쓸 수 있는 폴더인가" 판단.
//!
//! - 서비스 자료 폴더(`C:\ProgramData\kr.salarycrew.winfiletool`): 일반 사용자가 먼저 만들어 둔 접합점을 따라가
//!   다른 폴더의 ACL을 바꾸지 않게, 검사와 DACL 설정을 **같은 핸들**로 한다(`filetool-svc`).
//! - exe 옆 `tools\`: LOCALAPPDATA에 쓸 수 없을 때만 쓰는 대체 위치인데, 그 폴더를 다른 사용자가 쓸 수 있으면(공유 폴더·USB)
//!   그 사람이 넣은 실행 파일을 우리가 실행하게 되므로 현재 사용자 전용일 때만 쓴다([`dir_user_only`]).
//!
//! 판단 규칙은 순수 함수([`data_dir_action`], [`acl_user_only`])로 두어 Linux에서도 시험한다.

use std::path::Path;

/// 서비스 자료 폴더를 그대로 쓸지, 지우고 새로 만들지
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDirAction {
    /// 진짜 폴더이고 소유자가 SYSTEM·관리자 → 같은 핸들로 DACL만 다시 설정
    Keep,
    /// 재분석 지점(접합점·링크)이면 **링크만** 지우고(따라가지 않음) 보호된 폴더를 새로 만든다
    RemoveLink,
    /// 진짜 폴더지만 소유자가 일반 사용자(누군가 먼저 만들어 둠) → 지우고 새로 만든다
    Recreate,
}

/// SYSTEM·Administrators(SDDL 약칭 또는 SID 문자열)
pub fn is_trusted_owner(sid: &str) -> bool {
    matches!(sid, "SY" | "BA" | "S-1-5-18" | "S-1-5-32-544")
}

/// 자료 폴더 판단: 재분석 지점이면 무조건 링크만 지움, 아니면 소유자로
pub fn data_dir_action(is_reparse: bool, owner_sid: &str) -> DataDirAction {
    if is_reparse {
        DataDirAction::RemoveLink
    } else if is_trusted_owner(owner_sid) {
        DataDirAction::Keep
    } else {
        DataDirAction::Recreate
    }
}

/// DACL 항목 하나(허용·거부, SID 문자열, 권한, 상속 전용인지)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ace {
    pub allow: bool,
    pub sid: String,
    pub mask: u32,
    pub inherit_only: bool,
}

/// 쓰기로 볼 권한: 파일 쓰기·덧붙이기(=하위 폴더 만들기)·확장 특성 쓰기·하위 지우기·속성 쓰기·지우기·DACL/소유자 바꾸기·GENERIC_WRITE/ALL
pub const WRITE_MASK: u32 = 0x2 | 0x4 | 0x10 | 0x40 | 0x100 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x4000_0000 | 0x1000_0000;

/// 이 사용자(SYSTEM·관리자 포함)만 쓸 수 있는가: 소유자가 그중 하나이고, 쓰기 권한을 주는 허용 항목이 모두 그중 하나(또는 CREATOR OWNER).
/// `aces`가 `None`이면 DACL이 없음(=모두 허용, FAT·exFAT USB 등) → 아니다.
pub fn acl_user_only(owner: &str, aces: Option<&[Ace]>, user: &str) -> bool {
    let ok_sid = |s: &str| s == user || is_trusted_owner(s);
    let Some(aces) = aces else { return false };
    if !ok_sid(owner) {
        return false;
    }
    aces.iter().filter(|a| a.allow && a.mask & WRITE_MASK != 0).all(|a| ok_sid(&a.sid) || a.sid == "S-1-3-0")
}

/// 따라가지 않고 연 결과
pub enum Opened {
    Dir(NoFollowDir),
    /// 접합점·심볼릭 링크 같은 재분석 지점(따라가지 않았음)
    Reparse,
    NotFound,
}

/// 재분석 지점을 따라가지 않고 연 폴더 핸들
pub struct NoFollowDir {
    #[cfg(windows)]
    h: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(not(windows))]
    _f: std::fs::File,
}

#[cfg(windows)]
impl Drop for NoFollowDir {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.h) };
    }
}

/// 폴더를 재분석 지점을 따라가지 않고 연다. `write_dac`면 DACL을 바꿀 수 있게(관리자·SYSTEM).
/// 폴더가 아닌 파일이면 오류.
pub fn open_dir_nofollow(path: &Path, write_dac: bool) -> std::io::Result<Opened> {
    imp::open(path, write_dac)
}

/// 경로 자체가 재분석 지점(접합점·심볼릭 링크 등)인지(따라가지 않고 봄)
pub fn is_reparse_point(p: &Path) -> bool {
    let Ok(m) = std::fs::symlink_metadata(p) else { return false };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    m.file_type().is_symlink()
}

/// 이 사용자만 쓸 수 있는 폴더인지(재분석 지점이면 아니다). 읽지 못하면 아니다.
pub fn dir_user_only(path: &Path) -> bool {
    imp::user_only(path)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{GetLastError, LocalFree, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SetSecurityInfo, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{GetAce, GetSecurityDescriptorDacl, GetTokenInformation, TokenUser, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, PSID, SECURITY_ATTRIBUTES, TOKEN_USER};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    const READ_CONTROL: u32 = 0x2_0000;
    const WRITE_DAC: u32 = 0x4_0000;
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const OWNER_SECURITY_INFORMATION: u32 = 0x1;
    const DACL_SECURITY_INFORMATION: u32 = 0x4;
    const PROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x8000_0000;
    const INHERIT_ONLY_ACE: u8 = 0x8;
    const TOKEN_QUERY: u32 = 0x8;

    fn wide(p: &Path) -> Vec<u16> {
        p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
    }

    fn sid_string(sid: PSID) -> String {
        let mut s: *mut u16 = std::ptr::null_mut();
        if sid.is_null() || unsafe { ConvertSidToStringSidW(sid, &mut s) } == 0 {
            return String::new();
        }
        let mut n = 0;
        let out = unsafe {
            while *s.add(n) != 0 {
                n += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(s, n))
        };
        unsafe { LocalFree(s as _) };
        out
    }

    pub fn open(path: &Path, write_dac: bool) -> std::io::Result<Opened> {
        let w = wide(path);
        let access = READ_CONTROL | FILE_READ_ATTRIBUTES | if write_dac { WRITE_DAC } else { 0 };
        let h: HANDLE = unsafe {
            CreateFileW(
                w.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            let e = unsafe { GetLastError() };
            if e == ERROR_FILE_NOT_FOUND || e == ERROR_PATH_NOT_FOUND {
                return Ok(Opened::NotFound);
            }
            return Err(std::io::Error::from_raw_os_error(e as i32));
        }
        let d = NoFollowDir { h };
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(d.h, &mut info) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Ok(Opened::Reparse);
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("폴더가 아닙니다: {}", path.display())));
        }
        Ok(Opened::Dir(d))
    }

    impl NoFollowDir {
        pub fn raw(&self) -> HANDLE {
            self.h
        }

        /// 소유자 SID 문자열과 DACL 항목(`None` = DACL 없음)
        pub fn owner_and_aces(&self) -> std::io::Result<(String, Option<Vec<Ace>>)> {
            let mut owner: PSID = std::ptr::null_mut();
            let mut dacl: *mut ACL = std::ptr::null_mut();
            let mut sd: *mut core::ffi::c_void = std::ptr::null_mut();
            let r = unsafe {
                GetSecurityInfo(
                    self.h,
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                    &mut owner,
                    std::ptr::null_mut(),
                    &mut dacl,
                    std::ptr::null_mut(),
                    &mut sd,
                )
            };
            if r != 0 {
                return Err(std::io::Error::from_raw_os_error(r as i32));
            }
            let owner_s = sid_string(owner);
            let aces = if dacl.is_null() {
                None
            } else {
                let mut v = Vec::new();
                let count = unsafe { (*dacl).AceCount };
                for i in 0..count as u32 {
                    let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
                    if unsafe { GetAce(dacl, i, &mut p) } == 0 || p.is_null() {
                        continue;
                    }
                    let hdr = unsafe { &*(p as *const ACE_HEADER) };
                    // 0 = ACCESS_ALLOWED, 1 = ACCESS_DENIED(같은 모양). 그 밖(개체·콜백 ACE)은 허용으로 보수적으로 센다
                    let allow = hdr.AceType != 1;
                    let (mask, sid) = if hdr.AceType <= 1 {
                        let a = unsafe { &*(p as *const ACCESS_ALLOWED_ACE) };
                        (a.Mask, sid_string(&a.SidStart as *const u32 as PSID))
                    } else {
                        (u32::MAX, String::from("?"))
                    };
                    v.push(Ace { allow, sid, mask, inherit_only: hdr.AceFlags & INHERIT_ONLY_ACE != 0 });
                }
                Some(v)
            };
            unsafe { LocalFree(sd) };
            Ok((owner_s, aces))
        }

        /// 이 핸들의 폴더에 보호된(상속 끊은) DACL을 SDDL로 설정(아래 항목에도 퍼짐). `write_dac`로 열었어야 한다.
        pub fn set_protected_dacl(&self, sddl: &str) -> std::io::Result<()> {
            with_sd(sddl, |sd| {
                let (mut present, mut defaulted) = (0, 0);
                let mut dacl: *mut ACL = std::ptr::null_mut();
                unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) };
                let r = unsafe {
                    SetSecurityInfo(
                        self.h,
                        SE_FILE_OBJECT,
                        DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        dacl,
                        std::ptr::null(),
                    )
                };
                if r != 0 {
                    return Err(std::io::Error::from_raw_os_error(r as i32));
                }
                Ok(())
            })
        }
    }

    fn with_sd<T>(sddl: &str, f: impl FnOnce(*mut core::ffi::c_void) -> std::io::Result<T>) -> std::io::Result<T> {
        let w: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut sd: *mut core::ffi::c_void = std::ptr::null_mut();
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(w.as_ptr(), 1, &mut sd, std::ptr::null_mut()) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let r = f(sd);
        unsafe { LocalFree(sd) };
        r
    }

    /// 보안 설명자를 넘겨 폴더를 만든다(만드는 순간부터 그 DACL). 이미 있으면 `AlreadyExists` 오류(따라가지 않음).
    pub fn create_dir_with_sddl(path: &Path, sddl: &str) -> std::io::Result<()> {
        with_sd(sddl, |sd| {
            let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: 0 };
            let w = wide(path);
            if unsafe { CreateDirectoryW(w.as_ptr(), &sa) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        })
    }

    /// 현재 프로세스 사용자 SID 문자열
    pub fn current_user_sid() -> Option<String> {
        let mut tok: HANDLE = std::ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) } == 0 {
            return None;
        }
        let mut buf = vec![0u8; 512];
        let mut need = 0u32;
        let ok = unsafe { GetTokenInformation(tok, TokenUser, buf.as_mut_ptr() as _, buf.len() as u32, &mut need) } != 0;
        unsafe { windows_sys::Win32::Foundation::CloseHandle(tok) };
        if !ok {
            return None;
        }
        let tu = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
        Some(sid_string(tu.User.Sid)).filter(|s| !s.is_empty())
    }

    pub fn user_only(path: &Path) -> bool {
        let Ok(Opened::Dir(d)) = open(path, false) else { return false };
        let Some(user) = current_user_sid() else { return false };
        match d.owner_and_aces() {
            Ok((owner, aces)) => acl_user_only(&owner, aces.as_deref(), &user),
            Err(_) => false,
        }
    }
}

#[cfg(windows)]
pub use imp::{create_dir_with_sddl, current_user_sid};

#[cfg(not(windows))]
mod imp {
    use super::*;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    pub fn open(path: &Path, _write_dac: bool) -> std::io::Result<Opened> {
        match std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY).open(path) {
            Ok(f) => Ok(Opened::Dir(NoFollowDir { _f: f })),
            Err(e) if e.raw_os_error() == Some(libc::ELOOP) => Ok(Opened::Reparse),
            // O_DIRECTORY + O_NOFOLLOW에서 링크는 ENOTDIR로 오는 경우도 있다
            Err(e) if e.raw_os_error() == Some(libc::ENOTDIR) && std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) => {
                Ok(Opened::Reparse)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Opened::NotFound),
            Err(e) => Err(e),
        }
    }

    /// Unix(시험용): 링크가 아니고, 내 소유이고, 그룹·다른 사용자 쓰기 권한이 없으면
    pub fn user_only(path: &Path) -> bool {
        let Ok(Opened::Dir(_)) = open(path, false) else { return false };
        let Ok(m) = std::fs::symlink_metadata(path) else { return false };
        m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o022 == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ace(sid: &str, mask: u32) -> Ace {
        Ace { allow: true, sid: sid.into(), mask, inherit_only: false }
    }

    #[test]
    fn data_dir_decisions() {
        // 접합점·링크는 소유자가 누구든(대상 폴더의 소유자를 읽게 되는 것이 바로 문제였다) 링크만 지운다
        assert_eq!(data_dir_action(true, "BA"), DataDirAction::RemoveLink);
        assert_eq!(data_dir_action(true, "S-1-5-18"), DataDirAction::RemoveLink);
        assert_eq!(data_dir_action(true, "S-1-5-21-1-2-3-1001"), DataDirAction::RemoveLink);
        assert_eq!(data_dir_action(false, "SY"), DataDirAction::Keep);
        assert_eq!(data_dir_action(false, "S-1-5-32-544"), DataDirAction::Keep);
        assert_eq!(data_dir_action(false, "S-1-5-21-1-2-3-1001"), DataDirAction::Recreate);
        assert_eq!(data_dir_action(false, ""), DataDirAction::Recreate);
    }

    #[test]
    fn user_only_acl_rules() {
        let me = "S-1-5-21-1-2-3-1001";
        let other = "S-1-5-21-1-2-3-1002";
        let users = "S-1-5-32-545";
        let auth = "S-1-5-11";
        let full = 0x1F_01FF;
        let rx = 0x1200a9;
        // 보통의 내 프로필 폴더: SYSTEM·관리자·나 전체
        let typical = [ace("S-1-5-18", full), ace("S-1-5-32-544", full), ace(me, full)];
        assert!(acl_user_only(me, Some(&typical), me));
        // Users·Authenticated Users 읽기·실행만 더해도 괜찮음
        let mut v = typical.to_vec();
        v.push(ace(users, rx));
        assert!(acl_user_only(me, Some(&v), me));
        // Authenticated Users "수정"(C:\ 아래 새 폴더, 공유 폴더) → 아님
        v.push(ace(auth, 0x1301bf));
        assert!(!acl_user_only(me, Some(&v), me));
        // 다른 사용자 쓰기 → 아님
        assert!(!acl_user_only(me, Some(&[ace(me, full), ace(other, 0x2)]), me));
        // 거부 항목은 쓰기를 주지 않음
        let deny = Ace { allow: false, sid: other.into(), mask: full, inherit_only: false };
        assert!(acl_user_only(me, Some(&[ace(me, full), deny]), me));
        // CREATOR OWNER(상속 전용)는 만든 사람 = 나
        let co = Ace { allow: true, sid: "S-1-3-0".into(), mask: full, inherit_only: true };
        assert!(acl_user_only(me, Some(&[ace(me, full), co]), me));
        // 소유자가 다른 사용자(그 사람이 DACL을 언제든 바꿀 수 있음) → 아님
        assert!(!acl_user_only(other, Some(&typical), me));
        // DACL 없음(FAT·exFAT USB) → 아님
        assert!(!acl_user_only(me, None, me));
        // 알 수 없는 ACE 모양은 허용으로 센다 → 아님
        assert!(!acl_user_only(me, Some(&[ace(me, full), ace("?", u32::MAX)]), me));
    }

    #[test]
    fn open_nofollow_refuses_links() {
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        std::fs::create_dir(&real).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(matches!(open_dir_nofollow(&real, false).unwrap(), Opened::Dir(_)));
        assert!(matches!(open_dir_nofollow(&t.path().join("none"), false).unwrap(), Opened::NotFound));
        let link = t.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        // Windows: 시험 임시 폴더 안에 디렉터리 접합점을 만든다(mklink /J는 관리자 권한이 필요 없다)
        #[cfg(windows)]
        {
            let st = std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&link).arg(&real).output().unwrap();
            assert!(st.status.success(), "mklink /J 실패: {}", String::from_utf8_lossy(&st.stderr));
        }
        assert!(link.is_dir(), "링크를 따라가면 폴더로 보임");
        assert!(matches!(open_dir_nofollow(&link, false).unwrap(), Opened::Reparse));
        assert!(!dir_user_only(&link));
        // 진짜 폴더(내가 만든 임시 폴더)는 나만 쓸 수 있음(Unix: 0755 → 그룹·기타 쓰기 없음, Windows: 프로필 TEMP 상속)
        assert!(dir_user_only(&real));
        // 링크를 지워도 대상은 그대로
        #[cfg(unix)]
        std::fs::remove_file(&link).unwrap();
        #[cfg(windows)]
        std::fs::remove_dir(&link).unwrap();
        assert!(real.is_dir());
    }
}
