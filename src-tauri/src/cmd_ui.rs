//! 0.3.0 W5: 화면에만 필요한 작은 명령(파일 크기·폴더 안 파일 목록·그림 미리보기).
//! 실제 작업은 하지 않고 읽기만 한다.

use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use serde::Serialize;

#[derive(Serialize)]
pub struct FileStat {
    pub path: String,
    pub exists: bool,
    pub is_dir: bool,
    /// 파일은 바이트, 폴더는 0
    pub size: u64,
    /// 초(UNIX)
    pub modified: u64,
}

/// 결과 줄의 "전 → 후 크기", 넣은 파일 카드의 크기
#[tauri::command]
pub async fn file_stats(paths: Vec<String>) -> Vec<FileStat> {
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|p| match std::fs::metadata(&p) {
                Ok(m) => FileStat {
                    exists: true,
                    is_dir: m.is_dir(),
                    size: if m.is_dir() { 0 } else { m.len() },
                    modified: m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0),
                    path: p,
                },
                Err(_) => FileStat { path: p, exists: false, is_dir: false, size: 0, modified: 0 },
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// 폴더 바로 안의 파일(이름 순, 숨김·시스템 파일 제외) — 이름 한꺼번에 바꾸기에 폴더를 넣었을 때
#[tauri::command]
pub async fn list_files(folder: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let rd = std::fs::read_dir(&folder).map_err(|e| format!("폴더를 읽지 못했어요: {e}"))?;
        let mut out: Vec<PathBuf> = rd
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                !(n.starts_with('.') || n.eq_ignore_ascii_case("desktop.ini") || n.eq_ignore_ascii_case("thumbs.db"))
            })
            .collect();
        out.sort_by_key(|p| p.file_name().map(|s| s.to_string_lossy().to_lowercase()));
        Ok(out.into_iter().map(|p| p.to_string_lossy().into_owned()).collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 서명·워터마크 그림 미리보기: 그림 파일을 `data:` 주소로(창이 바로 그릴 수 있는 형식만, 20 MB까지)
#[tauri::command]
pub async fn image_data_url(path: String) -> Option<String> {
    tauri::async_runtime::spawn_blocking(move || {
        let ext = std::path::Path::new(&path).extension()?.to_str()?.to_ascii_lowercase();
        let mime = match ext.as_str() {
            "png" => "image/png",
            "jpg" | "jpeg" | "jfif" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "bmp" => "image/bmp",
            _ => return None,
        };
        let meta = std::fs::metadata(&path).ok()?;
        if meta.len() > 20 * 1024 * 1024 {
            return None;
        }
        let bytes = std::fs::read(&path).ok()?;
        Some(format!("data:{mime};base64,{}", base64(&bytes)))
    })
    .await
    .ok()
    .flatten()
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_rfc4648() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64(b"foobar"), "Zm9vYmFy");
    }
}
