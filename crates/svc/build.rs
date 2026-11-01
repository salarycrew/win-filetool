//! 0.3.0 W4: Windows(MSVC)에서 VC 런타임(VCRUNTIME140.dll)을 exe 안에 넣는다 — VC++ 재배포 패키지 없는 PC에서도 뜨게.
//! UCRT는 Windows 10부터 OS에 들어 있으므로 동적으로 둔다(앱 win-filetool.exe와 같은 방식: tauri-build의 static_vcruntime).
fn main() {
    static_vcruntime::metabuild();
}
