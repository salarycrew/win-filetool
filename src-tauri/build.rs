fn main() {
    tauri_build::build();
    // 0.3.3(보안 강화): 휴대용 exe를 다운로드 폴더에서 실행해도 그 폴더의 DLL을 불러오지 않게.
    // 정적 가져오기는 System32에서만(/DEPENDENTLOADFLAG:0x800), KnownDLLs에 없는 dwmapi·winhttp는 지연 로드로 바꿔
    // main 첫 줄의 SetDefaultDllDirectories(System32) 뒤에 불리게 한다(filetool_core::dllsafe).
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:dwmapi.dll");
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:winhttp.dll");
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
}
