//! 0.3.3 "문서 도구"(줄인 LibreOffice) — 앱에 넣지 않고, 필요할 때 한 번만 받는다.
//!
//! 문서 → PDF는 먼저 이 PC의 한글·Microsoft Office(COM)로 한다([`crate::docs::engine_for`]). 그것으로 안 되는 일
//! (Pages·Keynote·Numbers, Office가 없는 PC의 Word·Excel·PPT, 한글 없는 PC의 한글 97 등)만 LibreOffice가 필요하다.
//!
//! - 묶음: `win-filetool-doctools-<판>.zst` = 줄인 LibreOffice 폴더의 `tar` → `zstd -19 --long=27`(0.3.0~0.3.2 앱에 들어 있던
//!   `lo` 덩어리와 같은 바이트). 받는 곳은 공개 저장소 릴리스의 고정 주소([`URL`]), **SHA-256과 크기는 exe에 고정**([`SHA256`]).
//!   주소는 바꿀 수 있어도(시험용 앱만) 해시는 바꿀 수 없다.
//! - 받기: 이어받기(Range) — 서버가 이어주지 않으면(200) 처음부터. 멈추거나 끊기면 받던 파일을 남겨 다음에 이어받는다.
//!   다 받으면 SHA-256을 확인하고, 다르면 **지우고 거절**한다. 풀기 전에 확인한 파일만 푼다.
//! - 푸는 곳: `[`crate::bundle::tools_root`]\doctools\lo\`(0.3.x와 같은 LOCALAPPDATA 도구 폴더). `.partial`에 다 푼 뒤 이름을 바꾼다.
//! - 0.3.0~0.3.2가 이미 풀어 둔 `tools\<판>-<해시>\lo\`가 있으면 받지 않고 그것을 옮겨 쓴다([`migrate`]).
//!   0.3.3(보안 강화): 옮겨 쓰기 전에 한 번, 폴더 내용 해시([`LO_TREE_SHA256`], 빌드 때 묶음에서 계산)와 같은지 본다 —
//!   빠지거나 바뀌거나 **더 들어간** 파일(예: 심어 둔 DLL)이 있으면 쓰지 않는다.
//! - 0.3.3(보안 강화): 해시 확인과 풀기를 **한 번 연 핸들**에서(쓰기 공유 거부) 한다 — 읽으며 해시를 내고 임시 폴더에 풀어,
//!   해시가 맞을 때만 이름을 바꾼다(확인한 뒤 다시 열어 푸는 사이에 파일이 바뀌는 틈을 없앰).
//! - 인터넷이 없는 PC: [설정] → 문서 도구 → [파일로 넣기]로 같은 파일을 고르면 같은 확인을 거쳐 푼다(고른 파일은 지우지 않음).
//!
//! 이 모듈은 네트워크를 직접 쓰지 않는다: HTTP는 [`RangeFetch`]로 받아서(앱은 WinHTTP, 시험은 가짜) Linux에서 시험한다.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{io_err, Error, Result};
use crate::process::Cancel;

/// 묶음 판(묶음 내용이 바뀌면 올린다)
pub const PACK_VERSION: u32 = 1;
/// 릴리스에 올리는 파일 이름
pub const ASSET: &str = "win-filetool-doctools-1.zst";
/// 받는 곳(공개 저장소 v0.3.3 릴리스). 묶음이 바뀌지 않는 한 다음 판도 이 주소를 쓴다.
pub const URL: &str = "https://github.com/salarycrew/win-filetool/releases/download/v0.3.3/win-filetool-doctools-1.zst";
/// 묶음 전체의 SHA-256(고정)
pub const SHA256: &str = "355752ca307325cc7885cbd7515be24827b24d3554ae3214622a24b7592850d0";
/// 묶음 크기(바이트)
pub const SIZE: u64 = 105_443_108;
/// 묶음에 든 LibreOffice 판
pub const LO_VERSION: &str = "26.2.6.3";
/// 풀었을 때(파일 2,426개)
pub const UNPACKED_BYTES: u64 = 444_364_426;
/// 0.3.3(보안 강화): 묶음을 풀었을 때 폴더 내용 해시(`scripts/doctools-tree-hash.py`, [`tree_sha256`]과 같은 계산)
pub const LO_TREE_SHA256: &str = "3eed7615558b5a9b551b71791919c78465753eee800c118a8c2efc70dbcbecc4";

/// 확인에 쓰는 고정값(배포 앱은 언제나 [`Pin::RELEASE`] — 시험만 작은 가짜 묶음으로 바꾼다)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
    pub sha256: &'static str,
    pub size: u64,
}

impl Pin {
    pub const RELEASE: Pin = Pin { sha256: SHA256, size: SIZE };
}

/// 화면에 보일 받기 크기(MB, 올림)
pub fn download_mb() -> u64 {
    SIZE.div_ceil(1_000_000)
}

/// 받을 주소(시험용 앱·CLI만 `WIN_FILETOOL_DOCTOOLS_URL`로 바꿀 수 있음)
pub fn url() -> String {
    crate::testhooks::var(crate::testhooks::DOCTOOLS_URL).unwrap_or_else(|| URL.to_string())
}

/// `…\tools\doctools`
pub fn dir() -> PathBuf {
    dir_in(&crate::bundle::tools_root())
}

pub fn dir_in(root: &Path) -> PathBuf {
    root.join("doctools")
}

fn soffice_rel(lo: &Path) -> PathBuf {
    lo.join("program").join(if cfg!(windows) { "soffice.exe" } else { "soffice" })
}

/// 받던 파일·받은 파일을 두는 곳
pub fn download_dir(root: &Path) -> PathBuf {
    root.join("doctools-download")
}

pub fn partial_path(root: &Path) -> PathBuf {
    download_dir(root).join(format!("{ASSET}.part"))
}

/// 푼 폴더 옆에 적어 두는 기록
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Marker {
    pub pack_version: u32,
    pub sha256: String,
    /// "download" | "file" | "0.3.x"
    pub source: String,
    pub lo_version: String,
    /// 0.3.3(보안 강화): 폴더 내용을 확인했는지(받은 것·넣은 것은 해시를 확인한 묶음에서 풀었으므로 true, 옮긴 것은 내용 해시 확인 뒤)
    #[serde(default)]
    pub tree_verified: bool,
}

fn marker_path(root: &Path) -> PathBuf {
    dir_in(root).join("doctools.json")
}

fn read_marker(root: &Path) -> Option<Marker> {
    serde_json::from_slice(&std::fs::read(marker_path(root)).ok()?).ok()
}

fn write_marker(root: &Path, source: &str) {
    let m = Marker { pack_version: PACK_VERSION, sha256: SHA256.into(), source: source.into(), lo_version: LO_VERSION.into(), tree_verified: true };
    let _ = std::fs::write(marker_path(root), serde_json::to_vec_pretty(&m).unwrap_or_default());
}

/// 잠금(앱 두 개·앱 + CLI가 같이 풀거나 옮기지 않게)
fn lock(root: &Path) -> Result<File> {
    std::fs::create_dir_all(root).map_err(io_err(root))?;
    let p = root.join(".doctools.lock");
    let f = File::options().create(true).truncate(false).write(true).open(&p).map_err(io_err(&p))?;
    f.lock().map_err(io_err(&p))?;
    Ok(f)
}

/// 폴더 내용 해시: 모든 파일의 `<상대 경로(/)>\t<크기>\t<SHA-256>\n`을 경로(UTF-8 바이트) 순으로 이은 것의 SHA-256.
/// 링크·접합점이 하나라도 있거나 읽지 못하면 None.
pub fn tree_sha256(dir: &Path) -> Option<String> {
    let mut lines: Vec<(String, String)> = Vec::new();
    for e in walkdir::WalkDir::new(dir).follow_links(false) {
        let e = e.ok()?;
        let ft = e.file_type();
        if ft.is_symlink() || crate::secdir::is_reparse_point(e.path()) {
            return None;
        }
        if ft.is_dir() {
            continue;
        }
        let rel = e.path().strip_prefix(dir).ok()?.to_string_lossy().replace('\\', "/");
        let len = e.metadata().ok()?.len();
        let h = sha256_file(e.path(), None).ok()?;
        lines.push((rel.clone(), format!("{rel}\t{len}\t{h}\n")));
    }
    lines.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut h = Sha256::new();
    for (_, l) in &lines {
        h.update(l.as_bytes());
    }
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// 옮겨 쓸 예전 `lo` 폴더의 내용이 묶음과 같은지. 다르면 그 폴더를 치운다(다음에 다시 계산하지 않게, 쓰는 중이면 남김).
fn legacy_ok(lo: &Path, root: &Path) -> bool {
    if tree_sha256(lo).is_some_and(|h| h == expected_tree(root)) {
        return true;
    }
    if let Some(parent) = lo.parent() {
        let trash = parent.join(format!(".trash-lo-{}", std::process::id()));
        if std::fs::rename(lo, &trash).is_ok() {
            let _ = std::fs::remove_dir_all(&trash);
        }
    }
    false
}

/// 기대하는 폴더 내용 해시(단위 시험만 작은 가짜 폴더의 값을 `<root>/.test-tree-sha256`에 둘 수 있다)
fn expected_tree(_root: &Path) -> String {
    #[cfg(test)]
    if let Ok(s) = std::fs::read_to_string(_root.join(".test-tree-sha256")) {
        return s.trim().to_string();
    }
    LO_TREE_SHA256.to_string()
}

/// 0.3.0~0.3.2가 풀어 둔 LibreOffice(`<root>\<판>-<16자 해시>\lo`) 찾기
pub fn legacy_lo_dirs(root: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    crate::bundle::looks_like_version_dir(&n)
                })
                .map(|e| e.path().join("lo"))
                .filter(|lo| soffice_rel(lo).is_file())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v.reverse(); // 새 판 먼저(이름 순으로 대강)
    v
}

/// 예전 판이 풀어 둔 LibreOffice를 `doctools\lo`로 옮긴다(같은 드라이브라 이름 바꾸기만). 옮긴 soffice 경로.
/// 쓰는 중이라 옮기지 못하면 그 자리의 경로를 돌려준다(다음에 다시 옮긴다).
/// 0.3.3(보안 강화): 내용 해시가 묶음과 같은 폴더만 옮겨 쓴다(다르면 그 폴더는 쓰지 않음 → 필요하면 받기).
pub fn migrate(root: &Path) -> Option<PathBuf> {
    let target = dir_in(root).join("lo");
    if soffice_rel(&target).is_file() {
        return Some(soffice_rel(&target));
    }
    let legacy: Vec<PathBuf> = legacy_lo_dirs(root).into_iter().filter(|lo| legacy_ok(lo, root)).collect();
    let first = legacy.first()?.clone();
    let _l = lock(root).ok()?;
    if soffice_rel(&target).is_file() {
        return Some(soffice_rel(&target));
    }
    let _ = std::fs::create_dir_all(dir_in(root));
    for lo in &legacy {
        if std::fs::rename(lo, &target).is_ok() {
            write_marker(root, "0.3.x");
            return Some(soffice_rel(&target));
        }
    }
    Some(soffice_rel(&first))
}

/// 쓸 수 있는 문서 도구의 soffice 경로(받은 것, 또는 예전 판에서 옮긴 것). 없으면 None.
pub fn soffice() -> Option<PathBuf> {
    soffice_in(&crate::bundle::tools_root())
}

pub fn soffice_in(root: &Path) -> Option<PathBuf> {
    let lo = dir_in(root).join("lo");
    let s = soffice_rel(&lo);
    if s.is_file() {
        // 0.3.3(보안 강화): 확인 기록이 없는 폴더(0.3.3 첫 판이 확인 없이 옮긴 0.3.x 폴더 등)는 한 번 내용 해시를 본다
        match read_marker(root) {
            Some(m) if m.tree_verified => return Some(s),
            m => {
                if legacy_ok(&lo, root) {
                    write_marker(root, m.as_ref().map(|m| m.source.as_str()).unwrap_or("0.3.x"));
                    return Some(s);
                }
                // 내용이 다르면 쓰지 않는다(legacy_ok가 그 폴더를 치움). 기록도 지운다
                let _ = std::fs::remove_file(marker_path(root));
                return None;
            }
        }
    }
    migrate(root)
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub ready: bool,
    pub soffice: Option<PathBuf>,
    pub dir: PathBuf,
    /// "download" | "file" | "0.3.x" | null
    pub source: Option<String>,
    pub lo_version: &'static str,
    /// 받을 크기(MB)
    pub download_mb: u64,
    /// 풀었을 때 크기(MB)
    pub unpacked_mb: u64,
    /// 받다 멈춘 바이트(이어받기)
    pub partial_bytes: u64,
    pub url: String,
    pub asset: &'static str,
    pub sha256: &'static str,
}

pub fn status() -> Status {
    status_in(&crate::bundle::tools_root())
}

pub fn status_in(root: &Path) -> Status {
    let soffice = soffice_in(root);
    Status {
        ready: soffice.is_some(),
        source: soffice.as_ref().map(|_| read_marker(root).map(|m| m.source).unwrap_or_else(|| "0.3.x".into())),
        soffice,
        dir: dir_in(root),
        lo_version: LO_VERSION,
        download_mb: download_mb(),
        unpacked_mb: UNPACKED_BYTES.div_ceil(1_000_000),
        partial_bytes: std::fs::metadata(partial_path(root)).map(|m| m.len()).unwrap_or(0),
        url: url(),
        asset: ASSET,
        sha256: SHA256,
    }
}

/// 파일의 SHA-256(소문자 16진)
pub fn sha256_file(p: &Path, cancel: Option<&Cancel>) -> Result<String> {
    let mut f = File::open(p).map_err(io_err(p))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if let Some(c) = cancel {
            c.check()?;
        }
        let n = f.read(&mut buf).map_err(io_err(p))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub const MSG_HASH: &str = "받은 문서 도구 파일이 원래 파일과 달라서(SHA-256 불일치) 쓰지 않았어요. 다시 받아 주세요.";
pub const MSG_HASH_FILE: &str = "고른 파일이 이 앱의 문서 도구 파일과 달라요(SHA-256 불일치). 릴리스 쪽에서 받은 win-filetool-doctools 파일을 골라 주세요.";
pub const MSG_OFFLINE: &str = "인터넷 연결을 확인하고 다시 해 주세요. 인터넷이 없는 PC라면 [설정] → 문서 도구 → [파일로 넣기]로 넣을 수 있어요.";

/// 묶음 파일이 고정한 해시·크기와 같은지
pub fn verify(p: &Path, pin: Pin, cancel: Option<&Cancel>) -> Result<bool> {
    let len = std::fs::metadata(p).map_err(io_err(p))?.len();
    if len != pin.size {
        return Ok(false);
    }
    Ok(sha256_file(p, cancel)? == pin.sha256)
}

/// HTTP 받기(앱은 WinHTTP, 시험은 가짜). `from` > 0이면 `Range: bytes=<from>-`를 보낸다.
/// 돌려주는 값: (상태 코드, 본문 읽개). 연결 실패는 `Err(설명)`.
pub trait RangeFetch {
    fn open(&self, url: &str, from: u64) -> std::result::Result<(u16, Box<dyn Read + '_>), String>;
}

/// 받기 진행(받은 바이트, 전체)
pub type Progress<'a> = &'a mut dyn FnMut(u64, u64);

/// 묶음을 `partial_path(root)`로 받고(이어받기), 다 받으면 해시를 확인해 받은 파일 경로를 돌려준다.
/// - 멈춤: `Err(Cancelled)`, 받던 파일은 남김(다음에 이어받기)
/// - 끊김·서버 오류: [`MSG_OFFLINE`] 안내, 받던 파일은 남김
/// - 해시·크기 불일치: 받은 파일을 **지우고** [`MSG_HASH`]
pub fn download(fetch: &dyn RangeFetch, url: &str, root: &Path, pin: Pin, cancel: Option<&Cancel>, progress: Progress) -> Result<PathBuf> {
    let size = pin.size;
    let part = partial_path(root);
    let dd = download_dir(root);
    std::fs::create_dir_all(&dd).map_err(io_err(&dd))?;
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > size {
        let _ = std::fs::remove_file(&part);
        have = 0;
    }
    let net = |detail: String| Error::Tool { tool: "문서 도구 받기".into(), message: format!("{MSG_OFFLINE} ({detail})") };
    if have < size {
        if let Some(c) = cancel {
            c.check()?;
        }
        let (status, mut body) = fetch.open(url, have).map_err(net)?;
        let mut f = match status {
            206 if have > 0 => File::options().append(true).open(&part).map_err(io_err(&part))?,
            200 => {
                // 이어주지 않는 서버: 처음부터
                have = 0;
                File::create(&part).map_err(io_err(&part))?
            }
            416 => {
                // 이미 다 받았는데 크기가 맞지 않음 → 처음부터 다시(다음 시도)
                let _ = std::fs::remove_file(&part);
                return Err(net("HTTP 416".into()));
            }
            s => return Err(net(format!("HTTP {s}"))),
        };
        progress(have, size);
        let mut buf = vec![0u8; 256 * 1024];
        let mut last = 0u64;
        loop {
            if let Some(c) = cancel {
                if c.is_cancelled() {
                    let _ = f.flush();
                    return Err(Error::Cancelled);
                }
            }
            let n = match body.read(&mut buf) {
                Ok(n) => n,
                Err(e) => {
                    let _ = f.flush();
                    return Err(net(e.to_string()));
                }
            };
            if n == 0 {
                break;
            }
            have += n as u64;
            if have > size {
                drop(f);
                let _ = std::fs::remove_file(&part);
                return Err(Error::Tool { tool: "문서 도구 받기".into(), message: MSG_HASH.into() });
            }
            f.write_all(&buf[..n]).map_err(io_err(&part))?;
            if have - last >= 1 << 20 || have == size {
                last = have;
                progress(have, size);
            }
        }
        f.flush().map_err(io_err(&part))?;
        drop(f);
        if have < size {
            // 서버가 일찍 끊음: 받은 데까지 남겨 두고 다음에 이어받기
            return Err(net(format!("{have}/{size} 바이트에서 끊김")));
        }
    }
    progress(size, size);
    if !verify(&part, pin, cancel)? {
        let _ = std::fs::remove_file(&part);
        return Err(Error::Tool { tool: "문서 도구 받기".into(), message: MSG_HASH.into() });
    }
    Ok(part)
}

/// 확인된 묶음 파일을 `doctools\lo`로 푼다. `delete_bad`: 해시가 다르면 그 파일을 지울지(받은 파일만 — 사용자가 고른 파일은 지우지 않음).
/// 풀기 전에 언제나 해시를 다시 확인한다. 돌려주는 값: soffice 경로.
pub fn install(pack: &Path, root: &Path, pin: Pin, source: &str, delete_bad: bool, cancel: Option<&Cancel>) -> Result<PathBuf> {
    let _l = lock(root)?;
    // 사용자가 고른 파일은 확인하는 사이에 바뀌지 않게 우리 폴더로 먼저 복사한다
    let dd = download_dir(root);
    std::fs::create_dir_all(&dd).map_err(io_err(&dd))?;
    let own = if pack.starts_with(&dd) {
        pack.to_path_buf()
    } else {
        let c = dd.join(format!("{ASSET}.copy"));
        if std::fs::metadata(pack).map(|m| m.len()).unwrap_or(0) != pin.size {
            return Err(Error::Invalid(MSG_HASH_FILE.into()));
        }
        std::fs::copy(pack, &c).map_err(io_err(pack))?;
        c
    };
    let cleanup_own = |p: &Path| {
        if p != pack || delete_bad {
            let _ = std::fs::remove_file(p);
        }
    };
    let target = dir_in(root).join("lo");
    if soffice_rel(&target).is_file() && read_marker(root).is_some_and(|m| m.tree_verified) {
        cleanup_own(&own);
        return Ok(soffice_rel(&target));
    }
    let partial = dir_in(root).join("lo.partial");
    if partial.exists() {
        std::fs::remove_dir_all(&partial).map_err(io_err(&partial))?;
    }
    // 0.3.3(보안 강화): 한 번 연 핸들(쓰기 공유 거부)에서 읽으며 해시를 내고 임시 폴더에 푼다 → 해시가 맞을 때만 이름 바꾸기
    let f = open_deny_write(&own)?;
    let len = f.metadata().map_err(io_err(&own))?.len();
    let bad = |own: &Path| -> Error {
        cleanup_own(own);
        Error::Invalid(if delete_bad { MSG_HASH.into() } else { MSG_HASH_FILE.into() })
    };
    if len != pin.size {
        drop(f);
        return Err(bad(&own));
    }
    let mut tee = HashTee { inner: std::io::BufReader::new(f), h: Sha256::new(), n: 0, cancel };
    let unpacked = crate::bundle::unpack_tar_zst(&mut tee, pin.size, crate::bundle::LIBREOFFICE, &partial);
    // tar 끝 뒤에 남은 바이트도 해시에 넣는다(묶음 전체의 해시와 비교하므로)
    let drained = std::io::copy(&mut tee, &mut std::io::sink());
    // 핸들을 먼저 닫는다(Windows는 쓰기·지우기 공유 없이 연 파일을 열린 채로 지울 수 없음)
    let HashTee { inner, h, n: read_n, .. } = tee;
    drop(inner);
    let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if matches!(&unpacked, Err(Error::Cancelled)) || cancel.is_some_and(Cancel::is_cancelled) {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(Error::Cancelled);
    }
    if drained.is_err() || read_n != pin.size || got != pin.sha256 {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(bad(&own));
    }
    if let Err(e) = unpacked {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(e);
    }
    if target.exists() {
        // 반쯤 남은 폴더(soffice 없음)나 확인 기록 없는 폴더
        let trash = root.join(format!(".trash-doctools-lo-{}", std::process::id()));
        std::fs::rename(&target, &trash).map_err(io_err(&target))?;
        let _ = std::fs::remove_dir_all(&trash);
    }
    std::fs::rename(&partial, &target).map_err(io_err(&target))?;
    write_marker(root, source);
    // 다 풀었으면 받은 파일(105 MB)은 지운다
    if own != pack || delete_bad {
        let _ = std::fs::remove_file(&own);
    }
    Ok(soffice_rel(&target))
}

/// 읽으며 SHA-256을 내는 읽개(멈추기 확인 포함)
struct HashTee<'a, R> {
    inner: R,
    h: Sha256,
    n: u64,
    cancel: Option<&'a Cancel>,
}

impl<R: Read> Read for HashTee<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.is_some_and(Cancel::is_cancelled) {
            // (Interrupted는 읽는 쪽이 다시 시도하므로 쓰지 않는다)
            return Err(std::io::Error::other("멈춤"));
        }
        let n = self.inner.read(buf)?;
        self.h.update(&buf[..n]);
        self.n += n as u64;
        Ok(n)
    }
}

/// 다른 프로그램이 쓰거나 지우지 못하게 열기(Windows: 읽기 공유만)
fn open_deny_write(p: &Path) -> Result<File> {
    let mut o = File::options();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(0x1); // FILE_SHARE_READ
    }
    o.open(p).map_err(io_err(p))
}

/// [지우기]: 문서 도구 폴더와 받던 파일을 지운다. 쓰는 중이면(문서 변환 중) 안내 오류.
pub fn remove(root: &Path) -> Result<()> {
    let _l = lock(root)?;
    let d = dir_in(root);
    if d.exists() {
        let trash = root.join(format!(".trash-doctools-{}", std::process::id()));
        std::fs::rename(&d, &trash).map_err(|_| Error::Invalid("문서 도구가 지금 쓰이는 중이에요. 문서 변환이 끝난 뒤(또는 앱을 다시 켠 뒤) 지워 주세요.".into()))?;
        let _ = std::fs::remove_dir_all(&trash);
    }
    let _ = std::fs::remove_dir_all(download_dir(root));
    // 예전 판 폴더에 남은 LibreOffice도(옮기지 못하고 남은 것)
    for lo in legacy_lo_dirs(root) {
        let _ = std::fs::remove_dir_all(&lo);
    }
    Ok(())
}

/// Windows: WinHTTP로 받기(시스템 프록시 설정을 따름, GitHub의 다른 주소로 넘기기(302)는 WinHTTP가 따라감).
/// 배포 앱은 https만 쓴다(시험용 주소 바꾸기가 켜졌을 때만 http 허용 — 로컬 가짜 서버).
pub struct WinHttpFetch {
    pub user_agent: String,
}

#[cfg(windows)]
mod winhttp {
    use super::*;
    use std::ffi::c_void;
    use windows_sys::Win32::Networking::WinHttp::*;

    struct H(*mut c_void);
    impl Drop for H {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { WinHttpCloseHandle(self.0) };
            }
        }
    }
    // 핸들은 한 스레드에서만 순서대로 쓴다
    unsafe impl Send for H {}

    pub struct Body {
        // 닫는 순서: 요청 → 연결 → 세션
        req: H,
        _con: H,
        _ses: H,
    }

    impl Read for Body {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut n: u32 = 0;
            let ok = unsafe { WinHttpReadData(self.req.0, buf.as_mut_ptr() as *mut c_void, buf.len().min(u32::MAX as usize) as u32, &mut n) };
            if ok == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::ConnectionAborted, format!("WinHttpReadData {}", std::io::Error::last_os_error())));
            }
            Ok(n as usize)
        }
    }

    impl RangeFetch for WinHttpFetch {
        fn open(&self, url: &str, from: u64) -> std::result::Result<(u16, Box<dyn Read + '_>), String> {
            let (secure, host, port, path) = crate::update::split_url(url).ok_or("주소가 올바르지 않습니다")?;
            if !secure && !crate::testhooks::enabled() {
                return Err("https 주소만 씁니다".into());
            }
            let w = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
            let agent = w(&self.user_agent);
            unsafe {
                let ses = H(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0));
                if ses.0.is_null() {
                    return Err(format!("WinHttpOpen {}", std::io::Error::last_os_error()));
                }
                // 이름 찾기·연결·보내기 15초, 받기(조각 사이) 60초
                WinHttpSetTimeouts(ses.0, 15000, 15000, 15000, 60000);
                let hw = w(&host);
                let con = H(WinHttpConnect(ses.0, hw.as_ptr(), port, 0));
                if con.0.is_null() {
                    return Err(format!("WinHttpConnect {}", std::io::Error::last_os_error()));
                }
                let (verb, pw) = (w("GET"), w(&path));
                let req = H(WinHttpOpenRequest(con.0, verb.as_ptr(), pw.as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null(), if secure { WINHTTP_FLAG_SECURE } else { 0 }));
                if req.0.is_null() {
                    return Err(format!("WinHttpOpenRequest {}", std::io::Error::last_os_error()));
                }
                let hdr = if from > 0 { format!("Range: bytes={from}-\r\n") } else { String::new() };
                let hw = w(&hdr);
                let (hp, hl) = if hdr.is_empty() { (std::ptr::null(), 0) } else { (hw.as_ptr(), u32::MAX) };
                if WinHttpSendRequest(req.0, hp, hl, std::ptr::null(), 0, 0, 0) == 0 || WinHttpReceiveResponse(req.0, std::ptr::null_mut()) == 0 {
                    return Err(format!("연결하지 못했습니다 {}", std::io::Error::last_os_error()));
                }
                let mut code: u32 = 0;
                let mut len = std::mem::size_of::<u32>() as u32;
                if WinHttpQueryHeaders(req.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, std::ptr::null(), &mut code as *mut u32 as *mut c_void, &mut len, std::ptr::null_mut()) == 0 {
                    return Err("응답 상태를 읽지 못했습니다".into());
                }
                Ok((code as u16, Box::new(Body { req, _con: con, _ses: ses })))
            }
        }
    }
}

#[cfg(not(windows))]
impl RangeFetch for WinHttpFetch {
    fn open(&self, _url: &str, _from: u64) -> std::result::Result<(u16, Box<dyn Read + '_>), String> {
        Err("Windows에서만 받습니다".into())
    }
}

/// 받기 + 확인 + 풀기(앱 [받기]·CLI가 같이 쓰는 한 번에 하기). 돌려주는 값: soffice 경로.
pub fn fetch_and_install(fetch: &dyn RangeFetch, cancel: Option<&Cancel>, progress: Progress) -> Result<PathBuf> {
    let root = crate::bundle::tools_root();
    if let Some(s) = soffice_in(&root) {
        return Ok(s);
    }
    let got = download(fetch, &url(), &root, Pin::RELEASE, cancel, progress)?;
    install(&got, &root, Pin::RELEASE, "download", true, cancel)
}

/// [파일로 넣기](인터넷 없는 PC). 고른 파일은 지우지 않는다.
pub fn install_file(file: &Path, cancel: Option<&Cancel>) -> Result<PathBuf> {
    install(file, &crate::bundle::tools_root(), Pin::RELEASE, "file", false, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// 가짜 lo 폴더(program/soffice)를 tar.zst로
    fn fake_pack() -> Vec<u8> {
        let mut tarb = tar::Builder::new(Vec::new());
        let exe = if cfg!(windows) { "program/soffice.exe" } else { "program/soffice" };
        for (name, data) in [(exe, &b"SOFFICE"[..]), ("NOTICE", &b"LibreOffice notice"[..]), ("share/x.dat", &[9u8; 30000][..])] {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tarb.append_data(&mut h, name, data).unwrap();
        }
        zstd::stream::encode_all(&tarb.into_inner().unwrap()[..], 3).unwrap()
    }

    fn pin_of(data: &[u8]) -> Pin {
        let h: String = Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect();
        Pin { sha256: Box::leak(h.into_boxed_str()), size: data.len() as u64 }
    }

    /// 가짜 서버: range를 지킬지, 몇 바이트 뒤 끊을지, 상태 코드
    struct Mock {
        data: Vec<u8>,
        honor_range: bool,
        cut_after: RefCell<Option<usize>>,
        status: u16,
        seen_from: RefCell<Vec<u64>>,
    }

    impl Mock {
        fn new(data: Vec<u8>) -> Self {
            Mock { data, honor_range: true, cut_after: RefCell::new(None), status: 0, seen_from: RefCell::new(vec![]) }
        }
    }

    struct Cut<'a> {
        data: &'a [u8],
        pos: usize,
        cut: Option<usize>,
    }

    impl Read for Cut<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if let Some(c) = self.cut {
                if self.pos >= c {
                    return Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "끊김"));
                }
            }
            let end = self.cut.unwrap_or(usize::MAX).min(self.data.len()).min(self.pos + buf.len().min(7000));
            let n = end - self.pos;
            buf[..n].copy_from_slice(&self.data[self.pos..end]);
            self.pos = end;
            Ok(n)
        }
    }

    impl RangeFetch for Mock {
        fn open(&self, _url: &str, from: u64) -> std::result::Result<(u16, Box<dyn Read + '_>), String> {
            self.seen_from.borrow_mut().push(from);
            if self.status != 0 {
                return Ok((self.status, Box::new(std::io::empty())));
            }
            let cut = self.cut_after.borrow_mut().take();
            if self.honor_range && from > 0 {
                let rest = &self.data[from as usize..];
                Ok((206, Box::new(Cut { data: rest, pos: 0, cut })))
            } else {
                Ok((200, Box::new(Cut { data: &self.data, pos: 0, cut })))
            }
        }
    }

    #[test]
    fn download_verify_install() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let pack = fake_pack();
        let pin = pin_of(&pack);
        let m = Mock::new(pack.clone());
        let mut seen = vec![];
        let got = download(&m, "http://x/doctools.zst", root, pin, None, &mut |a, b| seen.push((a, b))).unwrap();
        assert_eq!(std::fs::read(&got).unwrap(), pack);
        assert_eq!(seen.last(), Some(&(pin.size, pin.size)));
        assert!(soffice_in(root).is_none(), "받기만 하고 아직 풀지 않음");
        let so = install(&got, root, pin, "download", true, None).unwrap();
        assert_eq!(std::fs::read(&so).unwrap(), b"SOFFICE");
        assert!(dir_in(root).join("lo").join("NOTICE").is_file(), "LibreOffice 고지는 묶음 안에");
        assert!(!got.exists(), "다 풀면 받은 파일은 지움");
        let st = status_in(root);
        assert!(st.ready && st.source.as_deref() == Some("download") && st.partial_bytes == 0);
        // 지우기
        remove(root).unwrap();
        assert!(soffice_in(root).is_none() && !dir_in(root).exists());
    }

    #[test]
    fn interrupted_download_resumes_or_restarts() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let pack = fake_pack();
        let pin = pin_of(&pack);
        let cut = pack.len() / 3;
        let m = Mock::new(pack.clone());
        *m.cut_after.borrow_mut() = Some(cut);
        let e = download(&m, "u", root, pin, None, &mut |_, _| {}).unwrap_err().to_string();
        assert!(e.contains("인터넷 연결을 확인하고 다시 해 주세요"), "{e}");
        assert_eq!(std::fs::metadata(partial_path(root)).unwrap().len(), cut as u64, "받은 데까지 남김");
        assert_eq!(status_in(root).partial_bytes, cut as u64);
        // 다시: 이어받기(Range: bytes=cut-)
        let got = download(&m, "u", root, pin, None, &mut |_, _| {}).unwrap();
        assert_eq!(*m.seen_from.borrow(), vec![0, cut as u64]);
        assert_eq!(std::fs::read(&got).unwrap(), pack);

        // 이어주지 않는 서버(200): 처음부터 받아도 맞게
        let t2 = tempfile::tempdir().unwrap();
        let mut m2 = Mock::new(pack.clone());
        m2.honor_range = false;
        *m2.cut_after.borrow_mut() = Some(cut);
        assert!(download(&m2, "u", t2.path(), pin, None, &mut |_, _| {}).is_err());
        let got = download(&m2, "u", t2.path(), pin, None, &mut |_, _| {}).unwrap();
        assert_eq!(std::fs::read(&got).unwrap(), pack);

        // 서버 오류(404): 안내, 받던 파일 그대로
        let t3 = tempfile::tempdir().unwrap();
        let mut m3 = Mock::new(pack.clone());
        m3.status = 404;
        let e = download(&m3, "u", t3.path(), pin, None, &mut |_, _| {}).unwrap_err().to_string();
        assert!(e.contains("HTTP 404") && e.contains("파일로 넣기"), "{e}");

        // 멈추기: Cancelled, 받던 파일은 남김
        let t4 = tempfile::tempdir().unwrap();
        let c = Cancel::new();
        let m4 = Mock::new(pack.clone());
        let mut calls = 0;
        let r = download(&m4, "u", t4.path(), pin, Some(&c), &mut |_, _| {
            calls += 1;
            c.cancel();
        });
        assert!(matches!(r, Err(Error::Cancelled)));
        assert!(calls >= 1 && partial_path(t4.path()).exists());
    }

    #[test]
    fn hash_mismatch_refused_and_deleted() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let pack = fake_pack();
        let pin = pin_of(&pack);
        // 같은 크기, 한 바이트 바뀐 묶음(가짜 서버가 바꿔치기)
        let mut bad = pack.clone();
        let mid = bad.len() / 2;
        bad[mid] ^= 0x55;
        let m = Mock::new(bad.clone());
        let e = download(&m, "u", root, pin, None, &mut |_, _| {}).unwrap_err().to_string();
        assert!(e.contains("SHA-256 불일치"), "{e}");
        assert!(!partial_path(root).exists(), "다른 파일은 지움");
        assert!(soffice_in(root).is_none(), "풀지 않음");
        // 더 긴 파일
        let mut long = pack.clone();
        long.extend_from_slice(b"extra");
        let m = Mock::new(long);
        assert!(download(&m, "u", root, pin, None, &mut |_, _| {}).unwrap_err().to_string().contains("SHA-256"));
        assert!(!partial_path(root).exists());
        // [파일로 넣기]에 바뀐 파일: 거절하지만 사용자 파일은 지우지 않음
        let user = t.path().join("내가 받은 파일.zst");
        std::fs::write(&user, &bad).unwrap();
        let e = install(&user, root, pin, "file", false, None).unwrap_err().to_string();
        assert!(e.contains("SHA-256 불일치"), "{e}");
        assert!(user.exists() && soffice_in(root).is_none());
        assert!(!download_dir(root).join(format!("{ASSET}.copy")).exists());
        // 맞는 파일은 넣을 수 있고, 고른 원본은 그대로
        std::fs::write(&user, &pack).unwrap();
        install(&user, root, pin, "file", false, None).unwrap();
        assert!(user.exists() && soffice_in(root).is_some());
        assert_eq!(status_in(root).source.as_deref(), Some("file"));
        // 받은 파일(우리 것)이 바뀌었으면 지운다
        let t2 = tempfile::tempdir().unwrap();
        let dd = download_dir(t2.path());
        std::fs::create_dir_all(&dd).unwrap();
        let ours = dd.join(ASSET);
        std::fs::write(&ours, &bad).unwrap();
        assert!(install(&ours, t2.path(), pin, "download", true, None).is_err());
        assert!(!ours.exists());
    }

    #[test]
    fn reuse_unpacked_from_030_to_032() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let exe = if cfg!(windows) { "soffice.exe" } else { "soffice" };
        let old = root.join("0.3.2-27b0aadb98e3db02").join("lo").join("program");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join(exe), b"OLD").unwrap();
        std::fs::create_dir_all(root.join("0.3.2-27b0aadb98e3db02").join("ffmpeg")).unwrap();
        // 0.3.3(보안 강화): 이 가짜 폴더의 내용 해시를 기대값으로(실제 앱은 묶음에서 계산한 LO_TREE_SHA256)
        let tree = tree_sha256(&root.join("0.3.2-27b0aadb98e3db02").join("lo")).unwrap();
        std::fs::write(root.join(".test-tree-sha256"), &tree).unwrap();
        // 사용자 폴더는 판 폴더로 보지 않음
        std::fs::create_dir_all(root.join("my-folder").join("lo").join("program")).unwrap();
        let so = soffice_in(root).unwrap();
        assert_eq!(so, dir_in(root).join("lo").join("program").join(exe));
        assert_eq!(std::fs::read(&so).unwrap(), b"OLD");
        assert!(!root.join("0.3.2-27b0aadb98e3db02").join("lo").exists(), "옮김(복사 아님)");
        assert_eq!(status_in(root).source.as_deref(), Some("0.3.x"));
        // 새 판 묶음이 지난 판 폴더를 지워도 문서 도구는 남는다
        let b = crate::bundle::Bundle { file: root.join("x"), pack_start: 0, index: crate::bundle::Index { version: 1, hash: "aaaaaaaaaaaaaaaa".into(), tools: vec![] }, root: root.to_path_buf(), app_version: "0.3.3".into() };
        b.cleanup_old();
        assert!(!root.join("0.3.2-27b0aadb98e3db02").exists());
        assert!(soffice_in(root).is_some() && root.join("my-folder").exists());
    }

    #[test]
    fn cleanup_old_moves_legacy_lo_first() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let exe = if cfg!(windows) { "soffice.exe" } else { "soffice" };
        let old = root.join("0.3.1-1111111111111111").join("lo").join("program");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join(exe), b"OLD").unwrap();
        std::fs::write(root.join(".test-tree-sha256"), tree_sha256(&root.join("0.3.1-1111111111111111").join("lo")).unwrap()).unwrap();
        let b = crate::bundle::Bundle { file: root.join("x"), pack_start: 0, index: crate::bundle::Index { version: 1, hash: "aaaaaaaaaaaaaaaa".into(), tools: vec![] }, root: root.to_path_buf(), app_version: "0.3.3".into() };
        b.cleanup_old();
        assert!(!root.join("0.3.1-1111111111111111").exists());
        assert_eq!(std::fs::read(dir_in(root).join("lo").join("program").join(exe)).unwrap(), b"OLD");
    }

    #[test]
    fn legacy_folder_with_changed_or_extra_files_not_reused() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let exe = if cfg!(windows) { "soffice.exe" } else { "soffice" };
        let lo = root.join("0.3.2-27b0aadb98e3db02").join("lo");
        std::fs::create_dir_all(lo.join("program")).unwrap();
        std::fs::write(lo.join("program").join(exe), b"OLD").unwrap();
        std::fs::write(root.join(".test-tree-sha256"), tree_sha256(&lo).unwrap()).unwrap();
        // 더 들어간 파일(예: program 폴더에 심어 둔 DLL) → 옮겨 쓰지 않음
        std::fs::write(lo.join("program").join("version.dll"), b"MZ planted").unwrap();
        assert!(soffice_in(root).is_none());
        assert!(!lo.exists() && !dir_in(root).join("lo").exists(), "옮기지 않고 치움");
        // 바뀐 파일 → 옮겨 쓰지 않음
        std::fs::create_dir_all(lo.join("program")).unwrap();
        std::fs::write(lo.join("program").join(exe), b"NEW").unwrap();
        assert!(soffice_in(root).is_none());
        // 원래대로면 옮겨 씀
        std::fs::create_dir_all(lo.join("program")).unwrap();
        std::fs::write(lo.join("program").join(exe), b"OLD").unwrap();
        assert!(soffice_in(root).is_some());
    }

    #[test]
    fn migrated_without_check_is_verified_once() {
        // 0.3.3 첫 판이 확인 없이 옮긴 폴더(기록에 tree_verified 없음)
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let exe = if cfg!(windows) { "soffice.exe" } else { "soffice" };
        let lo = dir_in(root).join("lo");
        std::fs::create_dir_all(lo.join("program")).unwrap();
        std::fs::write(lo.join("program").join(exe), b"OLD").unwrap();
        std::fs::write(root.join(".test-tree-sha256"), tree_sha256(&lo).unwrap()).unwrap();
        std::fs::write(marker_path(root), br#"{"pack_version":1,"sha256":"x","source":"0.3.x","lo_version":"26.2.6.3"}"#).unwrap();
        assert!(soffice_in(root).is_some());
        assert!(read_marker(root).unwrap().tree_verified, "한 번 확인하고 기록");
        // 확인 기록이 없는데 내용이 다르면 쓰지 않고 치움
        std::fs::write(marker_path(root), br#"{"pack_version":1,"sha256":"x","source":"0.3.x","lo_version":"26.2.6.3"}"#).unwrap();
        std::fs::write(lo.join("program").join("extra.dll"), b"MZ").unwrap();
        assert!(soffice_in(root).is_none());
        assert!(!lo.exists() && read_marker(root).is_none());
    }

    #[test]
    fn constants_agree() {
        assert_eq!(SHA256.len(), 64);
        assert!(SHA256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert!(URL.starts_with("https://github.com/salarycrew/win-filetool/releases/download/") && URL.ends_with(ASSET));
        assert!(ASSET.contains(&format!("-{PACK_VERSION}.")));
        assert_eq!(download_mb(), 106);
        // 시험에서는 바꾼 주소를 읽지만 해시는 그대로
        assert!(url().starts_with("http"));
    }
}
