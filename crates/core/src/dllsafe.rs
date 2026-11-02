//! 0.3.3(보안 강화): DLL을 찾는 곳을 System32(와 앱이 직접 더한 폴더)로만 좁힌다.
//!
//! 휴대용 exe는 `다운로드` 같은 폴더에서 실행되는데, Windows는 KnownDLLs에 없는 DLL(`dwmapi.dll`, `winhttp.dll` 등)을
//! **exe가 있는 폴더에서 먼저** 찾는다. 그 폴더에 먼저 받아져 있던 같은 이름의 DLL이 앱 권한으로 실행되지 않게:
//! - 링크할 때 `/DEPENDENTLOADFLAG:0x800`(정적 가져오기를 System32에서만, 각 exe의 `build.rs`)
//! - `main` 첫 줄에서 [`harden`] → `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_USER_DIRS)`
//!   (이후 `LoadLibrary`·지연 로드(`dwmapi`·`winhttp`)도 exe 폴더·현재 폴더·PATH를 보지 않는다)
//!
//! 우리가 부르는 도구 DLL(PDFium)은 언제나 전체 경로로 불러오므로 영향이 없다.

/// System32에서만(+ `AddDllDirectory`로 더한 폴더) DLL을 찾게 한다. 실패해도 계속 실행한다(Windows 7 KB2533623 없는 PC 등).
#[cfg(windows)]
pub fn harden() -> bool {
    use windows_sys::Win32::System::LibraryLoader::{SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_SYSTEM32, LOAD_LIBRARY_SEARCH_USER_DIRS};
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_USER_DIRS) != 0 }
}

#[cfg(not(windows))]
pub fn harden() -> bool {
    true
}

