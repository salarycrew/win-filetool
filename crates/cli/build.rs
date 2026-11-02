//! 0.3.0 W4: Windows(MSVC)에서 VC 런타임(VCRUNTIME140.dll)을 exe 안에 넣는다 — VC++ 재배포 패키지 없는 PC에서도 뜨게.
//! UCRT는 Windows 10부터 OS에 들어 있으므로 동적으로 둔다(앱 win-filetool.exe와 같은 방식: tauri-build의 static_vcruntime).
//! 0.3.3(보안 강화): 정적 가져오기 DLL은 System32에서만, winhttp는 지연 로드(main 첫 줄의 SetDefaultDllDirectories 뒤에 불림).
fn main() {
    static_vcruntime::metabuild();
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:winhttp.dll");
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    }
}
