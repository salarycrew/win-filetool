//! 0.3.0 W4 zip 풀기·만들기. 맥 zip 처리([`zipnfc`])를 넓혀:
//! - 이름 인코딩을 고를 수 있다: auto(UTF-8 표시가 없고 UTF-8로 안 읽히는 이름이 있으면 CP949 = 윈도 탐색기 zip) / utf8 / cp949
//! - 풀기: `..`·절대 경로·드라이브 이름은 건너뛰고, 이미 있는 파일은 덮어쓰지 않으며, `__MACOSX`·`._*`·`.DS_Store`는 버린다.
//!   윈도에서 쓸 수 없는 글자(`<>"|?*`)는 `_`로, 끝의 점·공백은 지운다. 이름은 NFC로.
//! - 만들기: 이름은 NFC + UTF-8 표시(bit 11)를 켜서 맥·윈도 어디서나 바르게 보인다. 이미 압축된 형식(jpg·mp4·zip…)은 그대로 담는다.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::{io_err, Error, Result};
use crate::nfc::to_nfc;
use crate::paths::unique_path;
use crate::process::Cancel;
use crate::zipnfc::{is_mac_junk, safe_relative};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NameEnc {
    #[default]
    Auto,
    Utf8,
    Cp949,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ZipEntry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ZipListing {
    pub entries: Vec<ZipEntry>,
    /// "utf8" | "cp949"
    pub detected_encoding: &'static str,
    /// UTF-8 표시 없이 한글 등(ASCII 밖) 이름이 든 zip: 다른 프로그램·다른 OS에서 이름이 깨져 보일 수 있다
    pub garbled_guess: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtractOut {
    pub folder: PathBuf,
    pub count: usize,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateOut {
    pub output: PathBuf,
    pub count: usize,
    pub size: u64,
}

const UTF8_FLAG: u16 = 1 << 11;

fn zip_err(path: &Path) -> impl Fn(zip::result::ZipError) -> Error + '_ {
    move |e| Error::Zip { path: path.to_path_buf(), message: e.to_string() }
}

fn open(path: &Path) -> Result<ZipArchive<BufReader<File>>> {
    let f = File::open(path).map_err(io_err(path))?;
    ZipArchive::new(BufReader::new(f)).map_err(|e| Error::Zip { path: path.into(), message: format!("zip 파일이 아니거나 손상되었습니다 ({e})") })
}

struct RawName {
    raw: Vec<u8>,
    utf8_flag: bool,
}

fn raw_names(a: &mut ZipArchive<BufReader<File>>, path: &Path) -> Result<Vec<RawName>> {
    (0..a.len())
        .map(|i| {
            let f = a.by_index_raw(i).map_err(zip_err(path))?;
            Ok(RawName { raw: f.name_raw().to_vec(), utf8_flag: f.flags().as_u16() & UTF8_FLAG != 0 })
        })
        .collect()
}

/// zip 전체의 이름 인코딩 추정: UTF-8 표시가 없는 이름 중 UTF-8로 안 읽히는 것이 있으면 CP949.
pub fn detect(names: &[(&[u8], bool)]) -> (NameEnc, bool) {
    let unflagged_non_ascii: Vec<&[u8]> = names.iter().filter(|(r, f)| !f && !r.is_ascii()).map(|(r, _)| *r).collect();
    let garbled = !unflagged_non_ascii.is_empty();
    let enc = if unflagged_non_ascii.iter().any(|r| std::str::from_utf8(r).is_err()) { NameEnc::Cp949 } else { NameEnc::Utf8 };
    (enc, garbled)
}

/// 이름 바이트 → NFC 문자열(`/` 구분). UTF-8 표시가 있으면 언제나 UTF-8.
pub fn decode_name(raw: &[u8], utf8_flag: bool, enc: NameEnc) -> String {
    let s = if utf8_flag || raw.is_ascii() {
        String::from_utf8_lossy(raw).into_owned()
    } else {
        match enc {
            NameEnc::Cp949 => encoding_rs::EUC_KR.decode(raw).0.into_owned(),
            NameEnc::Utf8 => String::from_utf8_lossy(raw).into_owned(),
            NameEnc::Auto => match std::str::from_utf8(raw) {
                Ok(s) => s.to_string(),
                Err(_) => encoding_rs::EUC_KR.decode(raw).0.into_owned(),
            },
        }
    };
    to_nfc(&s.replace('\\', "/"))
}

fn effective(enc: NameEnc, detected: NameEnc) -> NameEnc {
    if enc == NameEnc::Auto {
        detected
    } else {
        enc
    }
}

fn enc_name(e: NameEnc) -> &'static str {
    match e {
        NameEnc::Cp949 => "cp949",
        _ => "utf8",
    }
}

pub fn list(path: &Path, enc: NameEnc) -> Result<ZipListing> {
    let mut a = open(path)?;
    let names = raw_names(&mut a, path)?;
    let (detected, garbled) = detect(&names.iter().map(|n| (n.raw.as_slice(), n.utf8_flag)).collect::<Vec<_>>());
    let use_enc = effective(enc, detected);
    let mut entries = Vec::new();
    for (i, n) in names.iter().enumerate() {
        let f = a.by_index_raw(i).map_err(zip_err(path))?;
        let name = decode_name(&n.raw, n.utf8_flag, use_enc);
        let is_dir = f.is_dir() || name.ends_with('/');
        entries.push(ZipEntry { name, size: f.size(), is_dir });
    }
    Ok(ZipListing { entries, detected_encoding: enc_name(detected), garbled_guess: garbled })
}

/// 윈도에서 쓸 수 없는 이름 조각 고치기
pub fn windows_safe_component(c: &str) -> String {
    let mut s: String = c.chars().map(|ch| if "<>\"|?*".contains(ch) || (ch as u32) < 0x20 { '_' } else { ch }).collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    if s.is_empty() {
        s.push('_');
    }
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        s.insert(0, '_');
    }
    s
}

/// 0.3.3(보안 강화): 풀어 쓰는 전체 바이트 상한 — 압축 파일 크기의 이 배수(작은 zip이 수 TB로 풀리는 "zip 폭탄")
pub const MAX_RATIO: u64 = 1000;
/// 아주 작은 zip도 이만큼은 풀 수 있게(1 GiB)
pub const MIN_BUDGET: u64 = 1 << 30;
/// 풀고 나서도 디스크에 남겨 둘 공간(1 GiB)
pub const DISK_RESERVE: u64 = 1 << 30;

/// 이 zip을 풀어 쓸 수 있는 전체 바이트: min(max(zip 크기 × 1000, 1 GiB), 남은 디스크 − 1 GiB)
pub fn extract_budget(zip_len: u64, out_dir: &Path) -> u64 {
    let by_ratio = zip_len.saturating_mul(MAX_RATIO).max(MIN_BUDGET);
    let by_disk = crate::paths::free_space(out_dir).map(|f| f.saturating_sub(DISK_RESERVE)).unwrap_or(u64::MAX);
    by_ratio.min(by_disk)
}

pub fn msg_too_big(done: usize) -> String {
    format!(
        "풀면 너무 커져서(압축 파일의 {MAX_RATIO}배가 넘거나 디스크가 거의 참) 풀기를 멈췄어요. 디스크를 채우지 않으려고요. 지금까지 푼 {done}개는 남겨 뒀어요."
    )
}

/// 상한을 넘으면 쓰기를 멈추는 쓰개
pub(crate) struct Capped<'a, W> {
    pub inner: W,
    pub left: &'a mut u64,
}

#[derive(Debug)]
pub(crate) struct OverBudget;
impl std::fmt::Display for OverBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("풀기 상한을 넘음")
    }
}
impl std::error::Error for OverBudget {}

pub(crate) fn is_over_budget(e: &std::io::Error) -> bool {
    e.get_ref().is_some_and(|x| x.is::<OverBudget>())
}

impl<W: Write> Write for Capped<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() as u64 > *self.left {
            return Err(std::io::Error::other(OverBudget));
        }
        let n = self.inner.write(buf)?;
        *self.left -= n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// `zip_path`를 `out_dir`(없으면 zip 옆) 안의 새 폴더 `이름/`(있으면 `이름 (1)/`)에 푼다.
/// 0.3.3(보안 강화): 다 푼 크기가 [`extract_budget`]을 넘으면 그 파일을 지우고 멈춘다(안내 오류).
pub fn extract(path: &Path, enc: NameEnc, out_dir: Option<&Path>, cancel: Option<&Cancel>, progress: &mut dyn FnMut(usize, usize, &str)) -> Result<ExtractOut> {
    let zip_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let parent = out_dir.map(Path::to_path_buf).or_else(|| path.parent().map(Path::to_path_buf)).unwrap_or_default();
    extract_limited(path, enc, out_dir, cancel, progress, extract_budget(zip_len, &parent))
}

pub(crate) fn extract_limited(
    path: &Path,
    enc: NameEnc,
    out_dir: Option<&Path>,
    cancel: Option<&Cancel>,
    progress: &mut dyn FnMut(usize, usize, &str),
    budget: u64,
) -> Result<ExtractOut> {
    let mut left = budget;
    let mut a = open(path)?;
    let names = raw_names(&mut a, path)?;
    let (detected, _) = detect(&names.iter().map(|n| (n.raw.as_slice(), n.utf8_flag)).collect::<Vec<_>>());
    let use_enc = effective(enc, detected);
    let stem = path.file_stem().map(|s| to_nfc(&s.to_string_lossy())).unwrap_or_else(|| "압축풀기".into());
    let parent = out_dir.map(Path::to_path_buf).or_else(|| path.parent().map(Path::to_path_buf)).unwrap_or_default();
    let folder = unique_path(&parent.join(windows_safe_component(&stem)));
    std::fs::create_dir_all(&folder).map_err(io_err(&folder))?;
    let mut out = ExtractOut { folder: folder.clone(), count: 0, skipped: vec![] };
    let total = names.len();
    for (i, n) in names.iter().enumerate() {
        if let Some(c) = cancel {
            c.check()?;
        }
        let name = decode_name(&n.raw, n.utf8_flag, use_enc);
        progress(i, total, &name);
        let skip = |out: &mut ExtractOut, reason: &str| out.skipped.push(Skipped { name: name.clone(), reason: reason.into() });
        if is_mac_junk(&name) {
            skip(&mut out, "맥이 만든 부속 파일이라 뺐습니다");
            continue;
        }
        let Some(rel) = safe_relative(&name) else {
            skip(&mut out, "폴더 밖을 가리키는 이름이라 뺐습니다");
            continue;
        };
        let rel: PathBuf = rel.components().map(|c| windows_safe_component(&c.as_os_str().to_string_lossy())).collect();
        let dest = folder.join(&rel);
        let mut f = match a.by_index(i) {
            Ok(f) => f,
            Err(e) => {
                let msg = e.to_string();
                let reason = if msg.to_ascii_lowercase().contains("password") {
                    "암호가 걸린 항목이라 풀지 못했습니다".to_string()
                } else {
                    format!("읽지 못했습니다: {msg}")
                };
                skip(&mut out, &reason);
                continue;
            }
        };
        if f.encrypted() {
            skip(&mut out, "암호가 걸린 항목이라 풀지 못했습니다");
            continue;
        }
        if f.is_dir() || name.ends_with('/') {
            std::fs::create_dir_all(&dest).map_err(io_err(&dest))?;
            continue;
        }
        if f.is_symlink() {
            skip(&mut out, "바로 가기(링크)라 뺐습니다");
            continue;
        }
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p).map_err(io_err(p))?;
        }
        let file = match OpenOptions::new().write(true).create_new(true).open(&dest) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                skip(&mut out, "같은 이름이 이미 있어 덮어쓰지 않았습니다");
                continue;
            }
            Err(e) => return Err(Error::Io { path: dest, source: e }),
        };
        let mut w = BufWriter::new(Capped { inner: file, left: &mut left });
        let res = std::io::copy(&mut f, &mut w).and_then(|_| w.flush());
        drop(w);
        if let Err(e) = res {
            let _ = std::fs::remove_file(&dest);
            if is_over_budget(&e) || e.get_ref().is_some_and(|x| x.to_string().contains("풀기 상한")) {
                return Err(Error::Invalid(msg_too_big(out.count)));
            }
            skip(&mut out, &format!("풀다가 오류가 났습니다: {e}"));
            continue;
        }
        if let Some(t) = f.last_modified().and_then(zip_time_to_system) {
            if let Ok(h) = OpenOptions::new().write(true).open(&dest) {
                let _ = h.set_modified(t);
            }
        }
        out.count += 1;
    }
    progress(total, total, "");
    Ok(out)
}

fn zip_time_to_system(t: zip::DateTime) -> Option<std::time::SystemTime> {
    use chrono::{Local, NaiveDate, TimeZone};
    let d = NaiveDate::from_ymd_opt(t.year() as i32, t.month() as u32, t.day() as u32)?.and_hms_opt(t.hour() as u32, t.minute() as u32, t.second() as u32)?;
    let local = Local.from_local_datetime(&d).earliest()?;
    Some(std::time::SystemTime::from(local))
}

fn system_to_zip_time(t: std::time::SystemTime) -> Option<zip::DateTime> {
    use chrono::{DateTime, Datelike, Local, Timelike};
    let d: DateTime<Local> = t.into();
    zip::DateTime::from_date_and_time(d.year().try_into().ok()?, d.month() as u8, d.day() as u8, d.hour() as u8, d.minute() as u8, d.second() as u8).ok()
}

const STORED_EXTS: [&str; 22] = [
    "jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "mp4", "mov", "m4v", "mkv", "webm", "mp3", "m4a", "aac", "zip", "7z", "rar", "gz", "docx", "xlsx", "pptx",
];

fn is_junk_file(name: &str) -> bool {
    name == ".DS_Store" || name.starts_with("._") || name.eq_ignore_ascii_case("Thumbs.db") || name.eq_ignore_ascii_case("desktop.ini")
}

/// 기본 출력 이름: 첫 항목 이름 + `.zip`(파일이면 확장자 뺀 이름), 겹치면 ` (1)`.
pub fn default_output(inputs: &[PathBuf]) -> PathBuf {
    let first = &inputs[0];
    let parent = first.parent().unwrap_or_else(|| Path::new(""));
    let stem = if first.is_dir() { first.file_name() } else { first.file_stem() }
        .map(|s| to_nfc(&s.to_string_lossy()))
        .unwrap_or_else(|| "압축".into());
    unique_path(&parent.join(format!("{stem}.zip")))
}

/// 파일·폴더들을 zip 하나로. 폴더는 그 이름부터 담는다. 출력은 새 파일로만 쓴다.
pub fn create(inputs: &[PathBuf], output: Option<&Path>, cancel: Option<&Cancel>, progress: &mut dyn FnMut(usize, usize, &str)) -> Result<CreateOut> {
    if inputs.is_empty() {
        return Err(Error::Invalid("압축할 파일을 고르세요".into()));
    }
    for p in inputs {
        if !p.exists() {
            return Err(Error::Invalid(format!("파일이 없습니다: {}", p.display())));
        }
    }
    let output = match output {
        Some(o) => {
            if o.exists() {
                return Err(Error::AlreadyExists(o.into()));
            }
            o.to_path_buf()
        }
        None => default_output(inputs),
    };
    // (디스크 경로, zip 안 이름, 폴더인지)
    let mut items: Vec<(PathBuf, String, bool)> = Vec::new();
    for p in inputs {
        let base = to_nfc(&p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        if p.is_dir() {
            items.push((p.clone(), format!("{base}/"), true));
            let mut entries: Vec<_> = walkdir::WalkDir::new(p).min_depth(1).follow_links(false).sort_by_file_name().into_iter().filter_map(|e| e.ok()).collect();
            entries.retain(|e| !e.file_type().is_symlink());
            for e in entries {
                let rel = e.path().strip_prefix(p).unwrap_or(e.path());
                let rel: Vec<String> = rel.components().map(|c| to_nfc(&c.as_os_str().to_string_lossy())).collect();
                if rel.last().is_some_and(|n| is_junk_file(n)) {
                    continue;
                }
                let name = format!("{base}/{}", rel.join("/"));
                if e.file_type().is_dir() {
                    items.push((e.path().to_path_buf(), format!("{name}/"), true));
                } else {
                    items.push((e.path().to_path_buf(), name, false));
                }
            }
        } else {
            items.push((p.clone(), base, false));
        }
    }
    // 만드는 zip 자신은 넣지 않는다
    let out_canon = output.parent().and_then(|d| d.canonicalize().ok()).map(|d| d.join(output.file_name().unwrap_or_default()));
    items.retain(|(p, _, _)| p.canonicalize().ok() != out_canon);
    let mut seen = std::collections::HashSet::new();
    items.retain(|(_, n, _)| seen.insert(n.to_lowercase()));

    let file = OpenOptions::new().write(true).create_new(true).open(&output).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            Error::AlreadyExists(output.clone())
        } else {
            Error::Io { path: output.clone(), source: e }
        }
    })?;
    let res = (|| -> Result<usize> {
        let mut zw = ZipWriter::new(BufWriter::new(file));
        let total = items.len();
        let mut count = 0;
        for (i, (disk, name, is_dir)) in items.iter().enumerate() {
            if let Some(c) = cancel {
                c.check()?;
            }
            progress(i, total, name);
            let mut opts = SimpleFileOptions::default().large_file(false);
            if let Some(t) = std::fs::metadata(disk).ok().and_then(|m| m.modified().ok()).and_then(system_to_zip_time) {
                opts = opts.last_modified_time(t);
            }
            if *is_dir {
                zw.add_directory(name.trim_end_matches('/'), opts).map_err(zip_err(&output))?;
                continue;
            }
            let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            let method = if STORED_EXTS.contains(&ext.as_str()) { CompressionMethod::Stored } else { CompressionMethod::Deflated };
            let len = std::fs::metadata(disk).map(|m| m.len()).unwrap_or(0);
            opts = opts.compression_method(method).large_file(len >= 0xFFFF_0000);
            zw.start_file(name.as_str(), opts).map_err(zip_err(&output))?;
            let mut src = File::open(disk).map_err(io_err(disk))?;
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                let n = src.read(&mut buf).map_err(io_err(disk))?;
                if n == 0 {
                    break;
                }
                zw.write_all(&buf[..n]).map_err(io_err(&output))?;
            }
            count += 1;
        }
        let mut w = zw.finish().map_err(zip_err(&output))?;
        w.flush().map_err(io_err(&output))?;
        progress(total, total, "");
        Ok(count)
    })();
    match res {
        Ok(count) => {
            let size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
            Ok(CreateOut { output, count, size })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&output);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn zip_bomb_stops_before_filling_disk() {
        use std::io::Write;
        let t = tempfile::tempdir().unwrap();
        let z = t.path().join("bomb.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&z).unwrap());
            let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            w.start_file("a.txt", o).unwrap();
            w.write_all(b"small").unwrap();
            for name in ["zeros1.bin", "zeros2.bin"] {
                w.start_file(name, o).unwrap();
                let block = vec![0u8; 1 << 20];
                for _ in 0..8 {
                    w.write_all(&block).unwrap();
                }
            }
            w.finish().unwrap();
        }
        let zip_len = std::fs::metadata(&z).unwrap().len();
        assert!(zip_len < 64 * 1024, "16 MiB의 0이 작게 압축됨: {zip_len}");
        // 상한 10 MiB: 첫 8 MiB는 풀리고 두 번째에서 멈춤
        let e = super::extract_limited(&z, super::NameEnc::Auto, Some(t.path()), None, &mut |_, _, _| {}, 10 << 20).unwrap_err();
        assert!(e.to_string().contains("풀기를 멈췄어요") && e.to_string().contains("2개"), "{e}");
        let folder = t.path().join("bomb");
        assert!(folder.join("zeros1.bin").exists() && !folder.join("zeros2.bin").exists(), "넘친 파일은 지움");
        // 상한 안이면 그대로
        let ok = super::extract_limited(&z, super::NameEnc::Auto, Some(t.path()), None, &mut |_, _, _| {}, 64 << 20).unwrap();
        assert_eq!(ok.count, 3);
        // 기본 상한: 작은 zip도 1 GiB, 압축률 1000배
        let free = crate::paths::free_space(t.path()).unwrap().saturating_sub(super::DISK_RESERVE);
        assert_eq!(super::extract_budget(1, t.path()), super::MIN_BUDGET.min(free));
        assert_eq!(super::extract_budget(10 << 30, t.path()), ((10u64 << 30) * super::MAX_RATIO).min(free));
    }

    use super::*;

    #[test]
    fn detection_and_decoding() {
        let (cp, _, _) = encoding_rs::EUC_KR.encode("보고서.txt");
        let names: Vec<(&[u8], bool)> = vec![(b"a.txt", false), (&cp, false)];
        assert_eq!(detect(&names), (NameEnc::Cp949, true));
        assert_eq!(decode_name(&cp, false, NameEnc::Cp949), "보고서.txt");
        assert_eq!(decode_name(&cp, false, NameEnc::Auto), "보고서.txt");
        let u = "보고서.txt".as_bytes();
        assert_eq!(detect(&[(u, true)]), (NameEnc::Utf8, false));
        assert_eq!(detect(&[(u, false)]), (NameEnc::Utf8, true));
        // UTF-8 표시가 있으면 cp949를 골라도 UTF-8
        assert_eq!(decode_name(u, true, NameEnc::Cp949), "보고서.txt");
        assert_eq!(windows_safe_component("a?b*. "), "a_b_");
        assert_eq!(windows_safe_component("con.txt"), "_con.txt");
        assert_eq!(windows_safe_component("COM1"), "_COM1");
        assert_eq!(windows_safe_component("COMMA"), "COMMA");
    }
}
