//! ux3: 검색 결과의 파일 아이콘. 확장자마다 한 번만 만들어 캐시한다(폴더는 폴더 아이콘 하나).
//! Windows: `SHGetFileInfoW` + `SHGFI_USEFILEATTRIBUTES` — 실제 파일을 열지 않고 확장자·속성만으로 셸 아이콘을 받는다
//! (줄마다 디스크를 읽지 않음). 받은 HICON을 32비트 PNG로 바꿔 `data:` URL로 넘긴다.
//! 그 밖(Linux 빌드·시험)은 아이콘 없음(None) → 화면은 글자 아이콘으로 대신한다.

use std::collections::HashMap;
use std::sync::Mutex;

/// 폴더 아이콘의 키(확장자와 겹치지 않게 `/`로 시작)
pub const DIR_KEY: &str = "/dir";
/// 한 번에 받는 키 수·키 길이 한도
const MAX_KEYS: usize = 256;
const MAX_KEY_LEN: usize = 24;

static CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);

/// 키(소문자 확장자, 확장자 없음 = "", 폴더 = [`DIR_KEY`]) → 아이콘 `data:image/png;base64,…`(없으면 None)
pub fn icons(keys: Vec<String>) -> HashMap<String, Option<String>> {
    let mut out = HashMap::new();
    let mut todo = Vec::new();
    {
        let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let c = c.get_or_insert_with(HashMap::new);
        for k in keys.into_iter().take(MAX_KEYS) {
            if k.len() > MAX_KEY_LEN || (k != DIR_KEY && !k.chars().all(|ch| ch.is_alphanumeric() || "_-+~$#".contains(ch))) {
                out.insert(k, None);
            } else if let Some(v) = c.get(&k) {
                out.insert(k, v.clone());
            } else {
                todo.push(k);
            }
        }
    }
    if todo.is_empty() {
        return out;
    }
    let made: Vec<(String, Option<String>)> = platform::with_com(|| {
        todo.into_iter().map(|k| {
            let v = platform::icon_png(&k).map(|png| format!("data:image/png;base64,{}", base64(&png)));
            (k, v)
        }).collect()
    });
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let c = c.get_or_insert_with(HashMap::new);
    for (k, v) in made {
        c.insert(k.clone(), v.clone());
        out.insert(k, v);
    }
    out
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for ch in b.chunks(3) {
        let n = (ch[0] as u32) << 16 | (*ch.get(1).unwrap_or(&0) as u32) << 8 | *ch.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if ch.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if ch.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};

    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
    use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON, SHGFI_USEFILEATTRIBUTES};
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

    /// SHGetFileInfo는 COM이 초기화된 스레드에서 부른다(이 스레드에서 처음이면 끝나고 풀기)
    pub fn with_com<T>(f: impl FnOnce() -> T) -> T {
        let hr = unsafe { CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as _) };
        let r = f();
        if hr >= 0 {
            unsafe { CoUninitialize() };
        }
        r
    }

    pub fn icon_png(key: &str) -> Option<Vec<u8>> {
        let (name, attr) = if key == super::DIR_KEY {
            ("folder".to_string(), FILE_ATTRIBUTE_DIRECTORY)
        } else if key.is_empty() {
            ("file".to_string(), FILE_ATTRIBUTE_NORMAL)
        } else {
            (format!("file.{key}"), FILE_ATTRIBUTE_NORMAL)
        };
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let mut info: SHFILEINFOW = zeroed();
            let r = SHGetFileInfoW(
                wide.as_ptr(),
                attr,
                &mut info,
                size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES,
            );
            if r == 0 || info.hIcon.is_null() {
                return None;
            }
            let png = hicon_to_png(info.hIcon);
            DestroyIcon(info.hIcon);
            png
        }
    }

    unsafe fn hicon_to_png(h: HICON) -> Option<Vec<u8>> {
        let mut ii: ICONINFO = zeroed();
        if GetIconInfo(h, &mut ii) == 0 {
            return None;
        }
        let res = (|| {
            if ii.hbmColor.is_null() {
                return None; // 흑백 아이콘은 쓰지 않음
            }
            let mut bm: BITMAP = zeroed();
            if GetObjectW(ii.hbmColor as _, size_of::<BITMAP>() as i32, &mut bm as *mut _ as *mut c_void) == 0 {
                return None;
            }
            let (w, hgt) = (bm.bmWidth, bm.bmHeight.abs());
            if w <= 0 || hgt <= 0 || w > 256 || hgt > 256 {
                return None;
            }
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            let header = || {
                let mut bi: BITMAPINFO = zeroed();
                bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
                bi.bmiHeader.biWidth = w;
                bi.bmiHeader.biHeight = -hgt; // 위에서 아래로
                bi.bmiHeader.biPlanes = 1;
                bi.bmiHeader.biBitCount = 32;
                bi.bmiHeader.biCompression = BI_RGB as _;
                bi
            };
            let n = (w * hgt) as usize;
            let mut px = vec![0u8; n * 4];
            let mut bi = header();
            let ok = GetDIBits(dc, ii.hbmColor, 0, hgt as u32, px.as_mut_ptr() as *mut c_void, &mut bi, DIB_RGB_COLORS) != 0;
            if ok && !px.chunks_exact(4).any(|p| p[3] != 0) {
                // 알파가 없는 옛 아이콘: 마스크(검정 = 보임)로 투명도
                let mut mask = vec![0u8; n * 4];
                let mut bi = header();
                if !ii.hbmMask.is_null() && GetDIBits(dc, ii.hbmMask, 0, hgt as u32, mask.as_mut_ptr() as *mut c_void, &mut bi, DIB_RGB_COLORS) != 0 {
                    for i in 0..n {
                        px[i * 4 + 3] = if mask[i * 4] == 0 { 255 } else { 0 };
                    }
                } else {
                    for i in 0..n {
                        px[i * 4 + 3] = 255;
                    }
                }
            }
            DeleteDC(dc);
            if !ok {
                return None;
            }
            for p in px.chunks_exact_mut(4) {
                p.swap(0, 2); // BGRA → RGBA
            }
            let img = image::RgbaImage::from_raw(w as u32, hgt as u32, px)?;
            let mut out = std::io::Cursor::new(Vec::new());
            img.write_to(&mut out, image::ImageFormat::Png).ok()?;
            Some(out.into_inner())
        })();
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor as _);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask as _);
        }
        res
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn with_com<T>(f: impl FnOnce() -> T) -> T {
        f()
    }
    pub fn icon_png(_key: &str) -> Option<Vec<u8>> {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_rfc4648() {
        for (i, o) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(super::base64(i.as_bytes()), o);
        }
    }

    #[test]
    fn bad_keys_get_no_icon_and_no_shell_call() {
        let m = super::icons(vec!["../x".into(), "a".repeat(40), "pdf".into()]);
        assert_eq!(m.get("../x"), Some(&None));
        assert_eq!(m.len(), 3);
    }
}
