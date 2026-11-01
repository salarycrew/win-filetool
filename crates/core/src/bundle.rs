//! 0.3.0 W1: 앱에 든 도구(ffmpeg·PDFium·LibreOffice) 묶음을 읽고, 처음 필요할 때 도구별로 푼다.
//!
//! ## 묶음 형식(`tools.pack`)
//! 도구마다 `tar` → `zstd -19 --long=27`로 압축한 덩어리를 이어 붙이고, 끝에 목록(JSON)과 끝 표식을 둔다.
//! ```text
//! [덩어리 ffmpeg.tar.zst][덩어리 pdfium.tar.zst][덩어리 lo.tar.zst][목록 JSON][목록 길이 u64 LE][묶음 전체 길이 u64 LE]["WFTTOOL1"]
//! ```
//! 덩어리 위치는 묶음 시작 기준이라, 묶음은 따로 둔 파일(`tools.pack`, 설치본)로도, exe 뒤에 덧붙인 것(휴대용 exe)으로도 그대로 읽힌다.
//! Windows는 exe 뒤에 붙은 데이터(overlay)를 무시하고 실행한다. 만들기: `scripts/make-tools-pack.py`.
//!
//! ## 푸는 곳
//! `%LOCALAPPDATA%\kr.salarycrew.winfiletool\tools\<앱 버전>-<내용 해시>\<도구>\`.
//! - `<도구>.partial`에 다 푼 뒤 이름을 바꾼다 → 폴더가 있으면 "다 풀렸음".
//! - 같은 도구를 여러 프로그램(앱 두 개, 앱 + CLI)이 동시에 풀지 않도록 `.<도구>.lock` 파일 잠금. 기다린 쪽은 다시 확인만 한다.
//! - LOCALAPPDATA에 쓸 수 없을 때만 exe 옆 `tools\`. 시험용으로 `WIN_FILETOOL_TOOLS_DIR`로 바꿀 수 있다.
//! - 새 버전 폴더에서 도구가 풀리면 지난 버전 폴더는 지운다(쓰는 중이라 이름을 바꿀 수 없는 폴더는 남겨 두고 다음에).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::error::{io_err, Error, Result};

pub const MAGIC: &[u8; 8] = b"WFTTOOL1";
pub const TRAILER_LEN: u64 = 24;
pub const APP_DIR: &str = "kr.salarycrew.winfiletool";
/// 묶음 안 도구 이름
pub const FFMPEG: &str = "ffmpeg";
pub const PDFIUM: &str = "pdfium";
pub const LIBREOFFICE: &str = "lo";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub name: String,
    /// 묶음 시작 기준 위치
    pub offset: u64,
    pub len: u64,
    /// 풀었을 때 파일 수·바이트(진행 표시·점검용)
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Index {
    pub version: u32,
    /// 덩어리 내용의 sha256 앞 16자(버전 폴더 이름에 씀)
    pub hash: String,
    pub tools: Vec<Entry>,
}

/// 열어 둔 묶음.
#[derive(Debug, Clone)]
pub struct Bundle {
    pub file: PathBuf,
    pub pack_start: u64,
    pub index: Index,
    /// 도구 폴더들의 부모(`…\tools`)
    pub root: PathBuf,
    pub app_version: String,
}

/// 마지막으로 푼 기록(앱 정보·점검용)
#[derive(Debug, Clone, Serialize)]
pub struct UnpackInfo {
    pub tool: String,
    pub dir: PathBuf,
    pub secs: f64,
    pub files: u64,
    pub bytes: u64,
}

/// 끝 표식을 읽어 묶음을 찾는다. 표식이 없으면 `Ok(None)`.
pub fn read_index(file: &Path) -> Result<Option<(u64, Index)>> {
    let mut f = File::open(file).map_err(io_err(file))?;
    let len = f.metadata().map_err(io_err(file))?.len();
    if len < TRAILER_LEN {
        return Ok(None);
    }
    let mut t = [0u8; TRAILER_LEN as usize];
    f.seek(SeekFrom::Start(len - TRAILER_LEN)).map_err(io_err(file))?;
    f.read_exact(&mut t).map_err(io_err(file))?;
    if &t[16..24] != MAGIC {
        return Ok(None);
    }
    let index_len = u64::from_le_bytes(t[0..8].try_into().unwrap());
    let pack_len = u64::from_le_bytes(t[8..16].try_into().unwrap());
    if pack_len > len || index_len + TRAILER_LEN > pack_len || index_len > 16 * 1024 * 1024 {
        return Err(Error::Invalid(format!("도구 묶음 끝 표식이 맞지 않습니다: {}", file.display())));
    }
    let pack_start = len - pack_len;
    let mut buf = vec![0u8; index_len as usize];
    f.seek(SeekFrom::Start(len - TRAILER_LEN - index_len)).map_err(io_err(file))?;
    f.read_exact(&mut buf).map_err(io_err(file))?;
    let index: Index = serde_json::from_slice(&buf).map_err(|e| Error::Invalid(format!("도구 묶음 목록을 읽지 못했습니다: {e}")))?;
    let data_len = pack_len - TRAILER_LEN - index_len;
    for e in &index.tools {
        if e.offset.checked_add(e.len).is_none_or(|end| end > data_len) {
            return Err(Error::Invalid(format!("도구 묶음이 잘렸습니다({}): {}", e.name, file.display())));
        }
    }
    Ok(Some((pack_start, index)))
}

/// 덩어리들(이름, tar.zst 바이트)을 묶음 바이트로 만든다(시험·도구용. 실제 빌드는 make-tools-pack.py가 같은 형식으로 만든다).
pub fn build_pack(blobs: &[(&str, &[u8], u64, u64)], hash: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut tools = Vec::new();
    for (name, data, files, bytes) in blobs {
        tools.push(Entry { name: name.to_string(), offset: out.len() as u64, len: data.len() as u64, files: *files, bytes: *bytes });
        out.extend_from_slice(data);
    }
    let index = serde_json::to_vec(&Index { version: 1, hash: hash.into(), tools }).unwrap();
    out.extend_from_slice(&index);
    let pack_len = out.len() as u64 + TRAILER_LEN;
    out.extend_from_slice(&(index.len() as u64).to_le_bytes());
    out.extend_from_slice(&pack_len.to_le_bytes());
    out.extend_from_slice(MAGIC);
    out
}

/// 폴더에 쓸 수 있는지(만들어 보고 시험 파일을 써 본다)
pub fn writable_dir(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".write-probe-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"x").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// PC별 앱 데이터 폴더(`%LOCALAPPDATA%\kr.salarycrew.winfiletool`). Linux(시험)는 XDG_DATA_HOME 또는 ~/.local/share.
pub fn local_app_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_DATA_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(std::env::temp_dir)
        .join(APP_DIR)
}

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf))
}

/// 도구를 풀 곳: `WIN_FILETOOL_TOOLS_DIR` → LOCALAPPDATA\…\tools(쓸 수 있으면) → exe 옆 `tools\`
pub fn tools_root() -> PathBuf {
    if let Some(d) = std::env::var_os("WIN_FILETOOL_TOOLS_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(d);
    }
    choose_root(&local_app_dir().join("tools"), exe_dir().map(|d| d.join("tools")).as_deref())
}

/// LOCALAPPDATA 쪽에 쓸 수 있으면 그곳, 아니면 exe 옆(그것도 안 되면 그래도 LOCALAPPDATA 쪽을 돌려줘 오류 문구에 쓰게 한다)
pub fn choose_root(local: &Path, beside_exe: Option<&Path>) -> PathBuf {
    if writable_dir(local) {
        return local.to_path_buf();
    }
    if let Some(b) = beside_exe {
        if writable_dir(b) {
            return b.to_path_buf();
        }
    }
    local.to_path_buf()
}

type Hook = Box<dyn Fn(&str, f64) + Send + Sync>;
static HOOK: RwLock<Option<Hook>> = RwLock::new(None);
static LAST: Mutex<Vec<UnpackInfo>> = Mutex::new(Vec::new());
static CLEANED: OnceLock<()> = OnceLock::new();

/// 푸는 중 진행률(도구 이름, 0.0..=1.0)을 받을 함수(앱이 `tools_unpack_progress` 이벤트로 넘긴다)
pub fn set_progress_hook(f: impl Fn(&str, f64) + Send + Sync + 'static) {
    *HOOK.write().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
}

fn report(tool: &str, frac: f64) {
    if let Some(h) = HOOK.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        h(tool, frac);
    }
}

/// 이번 실행에서 푼 기록
pub fn unpack_log() -> Vec<UnpackInfo> {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

struct Counting<R> {
    inner: R,
    read: u64,
    total: u64,
    last: f64,
    tool: String,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        let f = if self.total == 0 { 1.0 } else { self.read as f64 / self.total as f64 };
        if f - self.last >= 0.02 || (n == 0 && self.last < 1.0) {
            self.last = f;
            report(&self.tool, f.min(1.0));
        }
        Ok(n)
    }
}

impl Bundle {
    pub fn open(file: &Path, root: &Path, app_version: &str) -> Result<Option<Bundle>> {
        Ok(read_index(file)?.map(|(pack_start, index)| Bundle {
            file: file.to_path_buf(),
            pack_start,
            index,
            root: root.to_path_buf(),
            app_version: app_version.to_string(),
        }))
    }

    pub fn version_name(&self) -> String {
        format!("{}-{}", self.app_version, self.index.hash)
    }
    pub fn version_dir(&self) -> PathBuf {
        self.root.join(self.version_name())
    }
    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.index.tools.iter().find(|e| e.name == name)
    }
    pub fn has(&self, name: &str) -> bool {
        self.entry(name).is_some()
    }
    /// 풀린 뒤의 도구 폴더(아직 없을 수 있음)
    pub fn tool_dir(&self, name: &str) -> PathBuf {
        self.version_dir().join(name)
    }
    pub fn is_ready(&self, name: &str) -> bool {
        self.tool_dir(name).is_dir()
    }

    /// 도구를 (필요하면) 풀고 폴더를 돌려준다. 여러 프로그램이 동시에 불러도 한 번만 푼다.
    pub fn ensure(&self, name: &str) -> Result<PathBuf> {
        let entry = self.entry(name).ok_or_else(|| Error::Invalid(format!("앱에 든 도구가 아닙니다: {name}")))?.clone();
        let dir = self.tool_dir(name);
        if dir.is_dir() {
            return Ok(dir);
        }
        let vdir = self.version_dir();
        std::fs::create_dir_all(&vdir).map_err(|e| prep_err(&vdir, e))?;
        let lock_path = vdir.join(format!(".{name}.lock"));
        let lock = File::options().create(true).truncate(false).write(true).open(&lock_path).map_err(|e| prep_err(&lock_path, e))?;
        lock.lock().map_err(|e| prep_err(&lock_path, e))?;
        // 기다리는 동안 다른 프로그램이 다 풀었을 수 있다
        if dir.is_dir() {
            return Ok(dir);
        }
        let t0 = Instant::now();
        let partial = vdir.join(format!("{name}.partial"));
        if partial.exists() {
            std::fs::remove_dir_all(&partial).map_err(|e| prep_err(&partial, e))?;
        }
        report(name, 0.0);
        let res = self.extract(&entry, &partial);
        if let Err(e) = res {
            let _ = std::fs::remove_dir_all(&partial);
            return Err(e);
        }
        std::fs::rename(&partial, &dir).map_err(|e| prep_err(&dir, e))?;
        report(name, 1.0);
        LAST.lock().unwrap_or_else(|e| e.into_inner()).push(UnpackInfo {
            tool: name.to_string(),
            dir: dir.clone(),
            secs: t0.elapsed().as_secs_f64(),
            files: entry.files,
            bytes: entry.bytes,
        });
        // 잠금 파일은 지우지 않는다(지우는 사이 다른 프로그램이 같은 이름으로 새로 잠그는 경쟁을 피함)
        drop(lock);
        if CLEANED.set(()).is_ok() {
            self.cleanup_old();
        }
        Ok(dir)
    }

    fn extract(&self, e: &Entry, to: &Path) -> Result<()> {
        let mut f = File::open(&self.file).map_err(io_err(&self.file))?;
        f.seek(SeekFrom::Start(self.pack_start + e.offset)).map_err(io_err(&self.file))?;
        let counting = Counting { inner: std::io::BufReader::new(f).take(e.len), read: 0, total: e.len, last: 0.0, tool: e.name.clone() };
        let mut dec = zstd::stream::read::Decoder::new(counting).map_err(|err| prep_err(&self.file, err))?;
        // --long=27 로 압축했으므로 큰 창을 허락한다
        dec.window_log_max(31).map_err(|err| prep_err(&self.file, err))?;
        std::fs::create_dir_all(to).map_err(|err| prep_err(to, err))?;
        let mut ar = tar::Archive::new(dec);
        ar.set_preserve_permissions(false);
        ar.set_overwrite(true);
        ar.unpack(to).map_err(|err| prep_err(to, err))?;
        Ok(())
    }

    /// 지난 버전 폴더 지우기: 이름을 바꿀 수 있는(아무도 쓰지 않는) 폴더만 `.trash-…`로 옮긴 뒤 지운다. 지운 폴더 목록.
    pub fn cleanup_old(&self) -> Vec<PathBuf> {
        let keep = self.version_name();
        let mut removed = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.root) else { return removed };
        for ent in rd.flatten() {
            let p = ent.path();
            let name = ent.file_name().to_string_lossy().into_owned();
            if !p.is_dir() || name == keep {
                continue;
            }
            if name.starts_with(".trash-") {
                if std::fs::remove_dir_all(&p).is_ok() {
                    removed.push(p);
                }
                continue;
            }
            // `<버전>-<16자 해시>` 모양만(사용자가 둔 다른 폴더는 건드리지 않는다)
            let looks_like_version = name.rsplit_once('-').is_some_and(|(_, h)| h.len() == 16 && h.chars().all(|c| c.is_ascii_hexdigit()));
            if !looks_like_version {
                continue;
            }
            let trash = self.root.join(format!(".trash-{name}-{}", std::process::id()));
            if std::fs::rename(&p, &trash).is_ok() {
                let _ = std::fs::remove_dir_all(&trash);
                removed.push(p);
            }
        }
        removed
    }
}

fn prep_err(p: &Path, e: std::io::Error) -> Error {
    Error::Tool {
        tool: "도구 준비".into(),
        message: format!(
            "앱에 든 도구를 준비하지 못했습니다. 디스크 공간(약 0.5GB)과 폴더 권한을 확인해 주세요. ({}: {e})",
            p.display()
        ),
    }
}

static CURRENT: OnceLock<Option<Bundle>> = OnceLock::new();

/// 묶음을 찾을 파일 후보: `WIN_FILETOOL_BUNDLE` → 실행 중인 exe(휴대용 exe) → exe 옆 `tools.pack`(설치본) → exe 옆 휴대용 exe들(CLI용)
pub fn candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = std::env::var_os("WIN_FILETOOL_BUNDLE").filter(|x| !x.is_empty()) {
        v.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        v.push(exe.clone());
        if let Some(d) = exe.parent() {
            v.push(d.join("tools.pack"));
            let mut others: Vec<PathBuf> = std::fs::read_dir(d)
                .map(|it| {
                    it.flatten()
                        .map(|e| e.path())
                        .filter(|p| {
                            let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                            n.ends_with("_portable.exe") || n == "win-filetool.exe"
                        })
                        .filter(|p| *p != exe)
                        .collect()
                })
                .unwrap_or_default();
            others.sort();
            v.extend(others);
        }
    }
    v
}

/// 이 앱의 묶음(없으면 None). 처음 부를 때 한 번 찾는다.
pub fn current() -> Option<&'static Bundle> {
    CURRENT
        .get_or_init(|| {
            let root = tools_root();
            candidates().into_iter().find_map(|c| Bundle::open(&c, &root, env!("CARGO_PKG_VERSION")).ok().flatten())
        })
        .as_ref()
}

/// 묶음에 든 도구 실행 파일 경로(필요하면 푼다). 묶음이 없거나 그 도구가 없으면 None, 풀다 실패하면 Err.
pub fn ensure_file(tool: &str, rel: &[&str]) -> Option<Result<PathBuf>> {
    let b = current()?;
    if !b.has(tool) {
        return None;
    }
    Some(b.ensure(tool).map(|d| rel.iter().fold(d, |p, s| p.join(s))))
}

/// 앱 정보 화면용 상태
#[derive(Debug, Clone, Serialize)]
pub struct BundleStatus {
    pub present: bool,
    pub file: Option<PathBuf>,
    pub dir: Option<PathBuf>,
    pub tools: Vec<ToolState>,
    pub unpacked: Vec<UnpackInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolState {
    pub name: String,
    pub ready: bool,
    pub packed_bytes: u64,
    pub bytes: u64,
}

pub fn status() -> BundleStatus {
    match current() {
        None => BundleStatus { present: false, file: None, dir: None, tools: vec![], unpacked: unpack_log() },
        Some(b) => BundleStatus {
            present: true,
            file: Some(b.file.clone()),
            dir: Some(b.version_dir()),
            tools: b.index.tools.iter().map(|e| ToolState { name: e.name.clone(), ready: b.is_ready(&e.name), packed_bytes: e.len, bytes: e.bytes }).collect(),
            unpacked: unpack_log(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_zst(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tarb = tar::Builder::new(Vec::new());
        for (name, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tarb.append_data(&mut h, name, *data).unwrap();
        }
        let raw = tarb.into_inner().unwrap();
        let mut enc = zstd::stream::write::Encoder::new(Vec::new(), 19).unwrap();
        enc.long_distance_matching(true).unwrap();
        enc.window_log(27).unwrap();
        std::io::Write::write_all(&mut enc, &raw).unwrap();
        enc.finish().unwrap()
    }

    fn sample_pack() -> Vec<u8> {
        let ff = tar_zst(&[("ffmpeg.exe", b"FFMPEG"), ("ffprobe.exe", b"FFPROBE")]);
        let lo = tar_zst(&[("program/soffice.exe", b"SOFFICE"), ("program/a.dll", &[7u8; 5000][..])]);
        build_pack(&[(FFMPEG, &ff, 2, 13), (LIBREOFFICE, &lo, 2, 5007)], "0123456789abcdef")
    }

    #[test]
    fn overlay_round_trip_and_unpack() {
        let t = tempfile::tempdir().unwrap();
        // exe 뒤에 덧붙인 경우
        let exe = t.path().join("app.exe");
        let mut bytes = b"MZ fake exe body".repeat(100);
        bytes.extend(sample_pack());
        std::fs::write(&exe, &bytes).unwrap();
        // 표식 없는 파일은 None
        let plain = t.path().join("plain.exe");
        std::fs::write(&plain, b"MZ no overlay at all, just bytes").unwrap();
        assert!(read_index(&plain).unwrap().is_none());

        let root = t.path().join("tools");
        let b = Bundle::open(&exe, &root, "0.3.0-dev").unwrap().unwrap();
        assert_eq!(b.pack_start, 1600);
        assert_eq!(b.version_name(), "0.3.0-dev-0123456789abcdef");
        assert!(!b.is_ready(FFMPEG));
        let d = b.ensure(FFMPEG).unwrap();
        assert_eq!(std::fs::read(d.join("ffprobe.exe")).unwrap(), b"FFPROBE");
        assert!(!b.is_ready(LIBREOFFICE), "도구별로 따로 푼다");
        let lo = b.ensure(LIBREOFFICE).unwrap();
        assert_eq!(std::fs::read(lo.join("program").join("a.dll")).unwrap().len(), 5000);
        assert!(!b.version_dir().join("lo.partial").exists());
        // 다시 부르면 그대로
        assert_eq!(b.ensure(LIBREOFFICE).unwrap(), lo);
        assert!(b.ensure("nope").is_err());

        // 따로 둔 tools.pack(설치본)도 같은 형식
        let pack = t.path().join("tools.pack");
        std::fs::write(&pack, sample_pack()).unwrap();
        let b2 = Bundle::open(&pack, &root, "0.3.0-dev").unwrap().unwrap();
        assert_eq!(b2.pack_start, 0);
        assert_eq!(b2.index, b.index);
    }

    #[test]
    fn truncated_pack_is_an_error() {
        let t = tempfile::tempdir().unwrap();
        let mut p = sample_pack();
        // 덩어리 일부를 잘라 낸 뒤 표식만 남긴 경우
        let tail = p.split_off(p.len() - 200);
        p.truncate(50);
        p.extend(tail);
        let f = t.path().join("bad.exe");
        std::fs::write(&f, &p).unwrap();
        assert!(read_index(&f).is_err());
    }

    #[test]
    fn stale_partial_is_replaced_and_old_versions_removed() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("tools");
        let old = root.join("0.2.9-aaaaaaaaaaaaaaaa").join("lo");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("x"), b"old").unwrap();
        let mine = root.join("my-notes");
        std::fs::create_dir_all(&mine).unwrap();
        let pack = t.path().join("tools.pack");
        std::fs::write(&pack, sample_pack()).unwrap();
        let b = Bundle::open(&pack, &root, "0.3.0-dev").unwrap().unwrap();
        // 지난번에 풀다 끊긴 흔적
        let partial = b.version_dir().join("ffmpeg.partial");
        std::fs::create_dir_all(&partial).unwrap();
        std::fs::write(partial.join("junk"), b"half").unwrap();
        let d = b.ensure(FFMPEG).unwrap();
        assert!(!d.join("junk").exists());
        assert!(!partial.exists());
        let removed = b.cleanup_old();
        assert!(!root.join("0.2.9-aaaaaaaaaaaaaaaa").exists(), "{removed:?}");
        assert!(mine.exists(), "버전 폴더 모양이 아니면 남긴다");
        assert!(b.version_dir().exists());
    }

    #[test]
    fn concurrent_ensure_unpacks_once() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("tools");
        let pack = t.path().join("tools.pack");
        std::fs::write(&pack, sample_pack()).unwrap();
        let hs: Vec<_> = (0..6)
            .map(|_| {
                let (pack, root) = (pack.clone(), root.clone());
                std::thread::spawn(move || {
                    let b = Bundle::open(&pack, &root, "0.3.0-dev").unwrap().unwrap();
                    b.ensure(LIBREOFFICE).unwrap()
                })
            })
            .collect();
        let dirs: Vec<PathBuf> = hs.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(dirs.windows(2).all(|w| w[0] == w[1]));
        assert_eq!(std::fs::read(dirs[0].join("program").join("soffice.exe")).unwrap(), b"SOFFICE");
        let left: Vec<_> = std::fs::read_dir(root.join("0.3.0-dev-0123456789abcdef"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.'))
            .collect();
        assert_eq!(left, vec!["lo".to_string()], "partial이 남지 않음");
    }

    #[test]
    fn root_falls_back_beside_exe() {
        let t = tempfile::tempdir().unwrap();
        let ok = t.path().join("local").join("tools");
        assert_eq!(choose_root(&ok, None), ok);
        // 파일이 가로막아 만들 수 없는 경로 → exe 옆
        let blocker = t.path().join("blocker");
        std::fs::write(&blocker, b"").unwrap();
        let bad = blocker.join("tools");
        let beside = t.path().join("exe").join("tools");
        assert_eq!(choose_root(&bad, Some(&beside)), beside);
    }
}
