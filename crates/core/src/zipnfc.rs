//! 맥에서 만든 zip 처리.
//!
//! 맥 "압축" 기능은 파일명을 UTF-8 **NFD**로 저장하면서 UTF-8 플래그(bit 11)를 켜지 않는 경우가
//! 많다. 그러면 윈도 탐색기·일부 도구가 CP437/CP949로 읽어 이름이 깨진다.
//! 여기서는 이름 바이트를 직접 해석한다:
//! UTF-8로 올바르면 UTF-8, 아니면 CP949(윈도 한국어)로 읽고, 그 뒤 NFC로 정규화한다.
//! `__MACOSX/` 아래 항목과 `._*`(AppleDouble), `.DS_Store`는 버린다.

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use serde::Serialize;
use unicode_normalization::is_nfc;
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::error::{io_err, Error, Result};
use crate::nfc::to_nfc;
use crate::paths::{unique_path, write_new};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum NameEncoding {
    Utf8,
    Cp949,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntryName {
    /// 정규화된 NFC 이름(`/` 구분)
    pub name: String,
    pub encoding: NameEncoding,
    /// 원래 이름이 NFC가 아니었는지
    pub was_nfd: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ExtractReport {
    pub out_dir: PathBuf,
    pub files: Vec<PathBuf>,
    pub dirs: usize,
    /// NFD → NFC로 바뀐 항목 수
    pub normalized: usize,
    /// CP949로 해석한 항목 수
    pub cp949_names: usize,
    /// 버린 맥 전용 항목(`__MACOSX/`, `._*`, `.DS_Store`)
    pub skipped_mac: Vec<String>,
    /// 이미 같은 이름이 있어 건너뛴 경로
    pub collisions: Vec<PathBuf>,
    /// 안전하지 않은 경로(`..`, 절대 경로)라 건너뛴 항목
    pub unsafe_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RezipReport {
    pub output: PathBuf,
    pub entries: usize,
    pub normalized: usize,
    pub skipped_mac: Vec<String>,
    pub duplicates: Vec<String>,
}

/// zip 항목 이름 바이트를 해석해 NFC 문자열로 만든다.
pub fn decode_entry_name(raw: &[u8]) -> EntryName {
    let (decoded, encoding) = match std::str::from_utf8(raw) {
        Ok(s) => (s.to_string(), NameEncoding::Utf8),
        Err(_) => {
            let (cow, _, _) = encoding_rs::EUC_KR.decode(raw);
            (cow.into_owned(), NameEncoding::Cp949)
        }
    };
    let decoded = decoded.replace('\\', "/");
    let was_nfd = !is_nfc(&decoded);
    EntryName { name: to_nfc(&decoded), encoding, was_nfd }
}

/// 맥이 덧붙이는 부산물인지.
pub fn is_mac_junk(name: &str) -> bool {
    let trimmed = name.trim_start_matches('/');
    if trimmed == "__MACOSX" || trimmed.starts_with("__MACOSX/") {
        return true;
    }
    let base = trimmed.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    base.starts_with("._") || base == ".DS_Store"
}

/// `a/b/c.txt` → 안전한 상대 경로. `..`·드라이브·절대 경로가 섞이면 None.
pub(crate) fn safe_relative(name: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for comp in name.split('/') {
        match comp {
            "" | "." => continue,
            ".." => return None,
            c if c.contains(':') => return None,
            c => out.push(c),
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

fn open_archive(zip_path: &Path) -> Result<ZipArchive<BufReader<File>>> {
    let f = File::open(zip_path).map_err(io_err(zip_path))?;
    ZipArchive::new(BufReader::new(f)).map_err(|e| Error::Zip {
        path: zip_path.to_path_buf(),
        message: e.to_string(),
    })
}

fn zip_err(path: &Path) -> impl Fn(zip::result::ZipError) -> Error + '_ {
    move |e| Error::Zip { path: path.to_path_buf(), message: e.to_string() }
}

/// zip 옆에 둘 기본 풀기 폴더: `이름/`(있으면 `이름 (1)/` …).
pub fn default_extract_dir(zip_path: &Path) -> PathBuf {
    let stem = zip_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "압축풀기".into());
    let parent = zip_path.parent().unwrap_or_else(|| Path::new(""));
    unique_path(&parent.join(to_nfc(&stem)))
}

/// zip을 `out_dir`에 NFC 이름으로 푼다. 이미 있는 파일은 덮어쓰지 않고 `collisions`에 넣는다.
pub fn extract_nfc(zip_path: &Path, out_dir: &Path) -> Result<ExtractReport> {
    let mut archive = open_archive(zip_path)?;
    std::fs::create_dir_all(out_dir).map_err(io_err(out_dir))?;
    let mut report = ExtractReport { out_dir: out_dir.to_path_buf(), ..Default::default() };

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(zip_err(zip_path))?;
        let entry = decode_entry_name(file.name_raw());
        if is_mac_junk(&entry.name) {
            report.skipped_mac.push(entry.name);
            continue;
        }
        let Some(rel) = safe_relative(&entry.name) else {
            report.unsafe_names.push(entry.name);
            continue;
        };
        if entry.was_nfd {
            report.normalized += 1;
        }
        if entry.encoding == NameEncoding::Cp949 {
            report.cp949_names += 1;
        }
        let dest = out_dir.join(&rel);
        if file.is_dir() || entry.name.ends_with('/') {
            std::fs::create_dir_all(&dest).map_err(io_err(&dest))?;
            report.dirs += 1;
            continue;
        }
        if dest.exists() {
            report.collisions.push(dest);
            continue;
        }
        let mut buf = Vec::with_capacity(file.size() as usize);
        file.read_to_end(&mut buf).map_err(io_err(zip_path))?;
        match write_new(&dest, &buf) {
            Ok(()) => report.files.push(dest),
            Err(Error::AlreadyExists(p)) => report.collisions.push(p),
            Err(e) => return Err(e),
        }
    }
    Ok(report)
}

/// 맥 부산물을 빼고 모든 이름을 NFC(UTF-8 플래그 포함)로 바꾼 새 zip을 만든다.
/// 압축된 데이터는 다시 압축하지 않고 그대로 복사한다.
pub fn rezip_nfc(zip_path: &Path, output: &Path) -> Result<RezipReport> {
    if output.exists() {
        return Err(Error::AlreadyExists(output.to_path_buf()));
    }
    let mut archive = open_archive(zip_path)?;
    let mut buf = std::io::Cursor::new(Vec::new());
    let mut report = RezipReport { output: output.to_path_buf(), ..Default::default() };
    {
        let mut writer = ZipWriter::new(&mut buf);
        let mut seen = HashSet::new();
        for i in 0..archive.len() {
            let file = archive.by_index_raw(i).map_err(zip_err(zip_path))?;
            let entry = decode_entry_name(file.name_raw());
            if is_mac_junk(&entry.name) {
                report.skipped_mac.push(entry.name);
                continue;
            }
            if safe_relative(&entry.name).is_none() {
                report.skipped_mac.push(entry.name);
                continue;
            }
            if !seen.insert(entry.name.clone()) {
                report.duplicates.push(entry.name);
                continue;
            }
            if entry.was_nfd || entry.encoding == NameEncoding::Cp949 {
                report.normalized += 1;
            }
            if file.is_dir() {
                drop(file);
                writer
                    .add_directory(entry.name.trim_end_matches('/'), SimpleFileOptions::default())
                    .map_err(zip_err(output))?;
            } else {
                writer.raw_copy_file_rename(file, &entry.name).map_err(zip_err(output))?;
            }
            report.entries += 1;
        }
        writer.finish().map_err(zip_err(output))?;
    }
    write_new(output, buf.get_ref())?;
    Ok(report)
}

/// zip 옆 기본 출력 이름: `이름_NFC.zip`.
pub fn default_rezip_path(zip_path: &Path, out_dir: Option<&Path>) -> PathBuf {
    crate::paths::output_path(zip_path, out_dir, "_NFC", "zip")
}
