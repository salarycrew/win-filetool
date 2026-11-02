//! 감시 폴더: 규칙(폴더 + 변환 + 출력 하위 폴더)마다 폴더 맨 위 파일을 주기적으로 살펴
//! 새 파일을 변환한다(앱이 열려 있는 동안).
//!
//! - 복사 중인 파일: 크기·수정 시각이 `stable_for` 동안 바뀌지 않고, (Windows) 다른 프로그램이
//!   쓰기로 열고 있지 않을 때만 처리한다.
//! - 다시 처리하지 않기: 처리한 파일(이름+크기+수정 시각)과 만든 출력 이름을 상태 파일에 적는다.
//!   출력은 하위 폴더에 쓰고 하위 폴더는 살피지 않는다. 같은 이름이라도 내용이 바뀌면(크기/시각) 다시 처리한다.
//! - 파일 시스템 알림(notify) 대신 2초 간격 폴링을 쓴다: 네트워크 드라이브·USB에서도 같게 동작하고 시험하기 쉽다.
//! - 0.3.3(보안 강화): 인터넷에서 받은 파일(Mark-of-the-Web `Zone.Identifier`의 `ZoneId`가 3 이상)은 Office·한글·LibreOffice·ffmpeg로
//!   **자동으로 열지 않는다**(문서·영상 파서 취약점, Office가 스스로 받는 원격 서식이 사람 확인 없이 돌지 않게). 그런 파일은
//!   "직접 골라서 해 주세요"로 남기고, 사용자가 화면에서 직접 고른 작업은 그대로 한다.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::convert::{self, Op};
use crate::error::Result;
use crate::process::Cancel;
use crate::settings::{load_json, save_json};
use crate::tools::Toolset;

fn default_sub() -> String {
    "변환됨".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchRule {
    pub id: String,
    pub folder: PathBuf,
    pub op: Op,
    #[serde(default = "default_sub")]
    pub out_subfolder: String,
    #[serde(default)]
    pub enabled: bool,
}

impl WatchRule {
    pub fn out_dir(&self) -> PathBuf {
        self.folder.join(self.out_subfolder.trim())
    }
    /// 규칙 검사(출력 하위 폴더가 비어 있으면 출력이 다시 입력이 되므로 막는다).
    pub fn validate(&self) -> std::result::Result<(), String> {
        let sub = self.out_subfolder.trim();
        if sub.is_empty() || sub == "." || sub.contains("..") || Path::new(sub).is_absolute() {
            return Err("출력 하위 폴더 이름을 적어 주세요 (예: 변환됨)".into());
        }
        if !self.folder.is_dir() {
            return Err(format!("감시할 폴더가 없습니다: {}", self.folder.display()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSig {
    pub size: u64,
    pub mtime_ms: i64,
}

fn sig_of(meta: &std::fs::Metadata) -> FileSig {
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    FileSig { size: meta.len(), mtime_ms }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoneEntry {
    pub sig: FileSig,
    pub ok: bool,
    pub output: Option<PathBuf>,
    pub message: String,
    pub at_unix: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RuleState {
    /// 파일 이름 → 처리 기록
    #[serde(default)]
    pub done: BTreeMap<String, DoneEntry>,
    /// 이 규칙이 만든 출력 파일 이름(혹시 같은 폴더에 생겨도 입력으로 받지 않음)
    #[serde(default)]
    pub outputs: BTreeSet<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WatchState {
    #[serde(default)]
    pub rules: BTreeMap<String, RuleState>,
}

/// 임시·숨김·부분 다운로드 파일은 보지 않는다.
pub fn is_ignored_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    name.starts_with('.')
        || name.starts_with("~$")
        || lower == "desktop.ini"
        || lower == "thumbs.db"
        || [".tmp", ".part", ".crdownload", ".download", ".partial", ".!ut", ".opdownload"].iter().any(|e| lower.ends_with(e))
}

/// 다른 프로그램이 아직 쓰고 있는지(Windows: 읽기만 공유로 열어 본다).
pub fn is_unlocked(p: &Path) -> bool {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(1); // FILE_SHARE_READ: 누군가 쓰기로 열고 있으면 실패
    }
    o.open(p).is_ok()
}

#[derive(Debug, Clone, Serialize)]
pub struct WatchEvent {
    pub rule_id: String,
    pub input: PathBuf,
    pub ok: bool,
    pub output: Option<PathBuf>,
    pub message: String,
}

pub struct Watcher {
    state_path: PathBuf,
    pub state: WatchState,
    /// 규칙 → 파일 → (마지막으로 본 sig, 그 sig를 처음 본 시각)
    pending: HashMap<String, HashMap<PathBuf, (FileSig, Instant)>>,
    pub stable_for: Duration,
}

impl Watcher {
    pub fn new(state_path: PathBuf, stable_for: Duration) -> Self {
        let state = load_json(&state_path);
        Self { state_path, state, pending: HashMap::new(), stable_for }
    }

    /// 처리할 준비가 된 파일들(크기가 안정되었고 아직 처리하지 않은 것).
    pub fn ready(&mut self, rule: &WatchRule, now: Instant) -> Vec<PathBuf> {
        let rs = self.state.rules.get(&rule.id);
        let pend = self.pending.entry(rule.id.clone()).or_default();
        let mut seen = Vec::new();
        let mut ready = Vec::new();
        let mut entries: Vec<_> = std::fs::read_dir(&rule.folder).map(|it| it.filter_map(|e| e.ok()).collect()).unwrap_or_default();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let path = e.path();
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if is_ignored_name(&name) || !rule.op.accepts(&path) {
                continue;
            }
            if let Some(rs) = rs {
                if rs.outputs.contains(&name) {
                    continue;
                }
            }
            let sig = sig_of(&meta);
            if rs.and_then(|r| r.done.get(&name)).map(|d| d.sig == sig).unwrap_or(false) {
                continue;
            }
            seen.push(path.clone());
            if sig.size == 0 {
                pend.insert(path, (sig, now));
                continue;
            }
            match pend.get(&path) {
                Some((prev, since)) if *prev == sig => {
                    if now.duration_since(*since) >= self.stable_for && is_unlocked(&path) {
                        ready.push(path.clone());
                    }
                }
                _ => {
                    pend.insert(path, (sig, now));
                }
            }
        }
        pend.retain(|p, _| seen.contains(p));
        for r in &ready {
            pend.remove(r);
        }
        ready
    }

    /// 처리 결과를 상태 파일에 적는다.
    pub fn record(&mut self, rule: &WatchRule, input: &Path, res: &Result<convert::ConvertOut>) -> WatchEvent {
        let name = input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let sig = std::fs::metadata(input).map(|m| sig_of(&m)).unwrap_or(FileSig { size: 0, mtime_ms: 0 });
        let rs = self.state.rules.entry(rule.id.clone()).or_default();
        let at_unix = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let ev = match res {
            Ok(o) => {
                if let Some(n) = o.output.file_name() {
                    rs.outputs.insert(n.to_string_lossy().into_owned());
                }
                WatchEvent { rule_id: rule.id.clone(), input: input.into(), ok: true, output: Some(o.output.clone()), message: o.note.clone() }
            }
            Err(e) => WatchEvent { rule_id: rule.id.clone(), input: input.into(), ok: false, output: None, message: e.to_string() },
        };
        rs.done.insert(name, DoneEntry { sig, ok: ev.ok, output: ev.output.clone(), message: ev.message.clone(), at_unix });
        let _ = save_json(&self.state_path, &self.state);
        ev
    }

    /// 0.3.3: 자동으로 열지 않은 파일을 기록한다(같은 파일이 바뀌지 않으면 다시 묻지 않음).
    fn record_skip(&mut self, rule: &WatchRule, input: &Path, message: &str) -> WatchEvent {
        let name = input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let sig = std::fs::metadata(input).map(|m| sig_of(&m)).unwrap_or(FileSig { size: 0, mtime_ms: 0 });
        let at_unix = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let rs = self.state.rules.entry(rule.id.clone()).or_default();
        rs.done.insert(name, DoneEntry { sig, ok: false, output: None, message: message.to_string(), at_unix });
        let _ = save_json(&self.state_path, &self.state);
        WatchEvent { rule_id: rule.id.clone(), input: input.into(), ok: false, output: None, message: message.to_string() }
    }

    /// 켜진 규칙마다 한 번 살펴보고 준비된 파일을 변환한다.
    pub fn run_once(&mut self, rules: &[WatchRule], tools: &Toolset, cancel: Option<&Cancel>, now: Instant) -> Vec<WatchEvent> {
        let mut events = Vec::new();
        for rule in rules.iter().filter(|r| r.enabled) {
            if rule.validate().is_err() {
                continue;
            }
            for input in self.ready(rule, now) {
                if cancel.map(Cancel::is_cancelled).unwrap_or(false) {
                    return events;
                }
                // 0.3.3: 인터넷에서 받은 파일은 바깥 프로그램(Office·한글·LibreOffice·ffmpeg)으로 자동으로 열지 않는다
                if uses_external_program(&rule.op, &input) && from_internet(&input) {
                    events.push(self.record_skip(rule, &input, INTERNET_SKIP_NOTE));
                    continue;
                }
                let res = convert::convert_file(tools, &input, Some(&rule.out_dir()), &rule.op, cancel, &mut |_| {});
                // 취소로 끝난 것은 기록하지 않는다(다음에 다시 처리)
                if matches!(res, Err(crate::Error::Cancelled)) {
                    return events;
                }
                events.push(self.record(rule, &input, &res));
            }
        }
        events
    }

    /// 규칙을 지우면 그 기록도 지운다.
    pub fn forget_rule(&mut self, rule_id: &str) {
        self.state.rules.remove(rule_id);
        self.pending.remove(rule_id);
        let _ = save_json(&self.state_path, &self.state);
    }
}

/// 0.3.3: 감시 폴더가 인터넷에서 받은 파일을 건너뛸 때 보이는 말
pub const INTERNET_SKIP_NOTE: &str = "인터넷에서 받은 파일은 자동으로 열지 않아요. 직접 골라서 해 주세요.";

/// 이 작업이 파일을 바깥 프로그램으로 여는지: 문서(Office·한글·LibreOffice), 영상·소리(ffmpeg), HEIC 사진(ffmpeg로 풂).
/// 보통 그림 형식 바꾸기는 앱 안(Rust)에서 하므로 아니다.
pub fn uses_external_program(op: &Op, input: &Path) -> bool {
    match op {
        Op::Doc { .. } | Op::Media(_) => true,
        Op::Image(_) => convert::classify(input) == convert::FileKind::Heic,
    }
}

/// `Zone.Identifier` 내용에서 `ZoneId` 값(`[ZoneTransfer]` 절, 대소문자·공백 무시)
pub fn parse_zone_id(text: &str) -> Option<u32> {
    let text = text.trim_start_matches('\u{feff}');
    let mut in_zt = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_zt = l.eq_ignore_ascii_case("[ZoneTransfer]");
            continue;
        }
        if !in_zt {
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            if k.trim().eq_ignore_ascii_case("ZoneId") {
                return v.trim().parse().ok();
            }
        }
    }
    None
}

/// 파일의 Mark-of-the-Web `ZoneId`(없으면 None). Windows는 NTFS 대체 데이터 스트림 `<파일>:Zone.Identifier`를 읽는다
/// (다른 OS에서는 같은 이름의 옆 파일 — 시험용).
pub fn zone_id(path: &Path) -> Option<u32> {
    let mut s = path.as_os_str().to_os_string();
    s.push(":Zone.Identifier");
    let mut buf = Vec::new();
    use std::io::Read;
    std::fs::File::open(PathBuf::from(s)).ok()?.take(64 * 1024).read_to_end(&mut buf).ok()?;
    // 보통 ANSI/UTF-8, 가끔 UTF-16LE
    let text = if buf.len() >= 2 && buf[0] == 0xFF && buf[1] == 0xFE {
        let w: Vec<u16> = buf[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&w)
    } else {
        String::from_utf8_lossy(&buf).into_owned()
    };
    parse_zone_id(&text)
}

/// 인터넷(3)·제한된 사이트(4)에서 온 파일인지
pub fn from_internet(path: &Path) -> bool {
    zone_id(path).is_some_and(|z| z >= 3)
}

/// 감시는 한 프로세스만: 설정 폴더의 잠금 파일을 잡은 앱만 규칙을 돌린다
/// (창을 두 개 열어도 같은 파일을 두 번 변환하지 않게). 프로세스가 끝나면 자동으로 풀린다.
///
/// Windows: `LockFileEx`(std `try_lock`). 핸들은 상속되지 않으므로 자식 프로세스가 잠금을 붙잡지 않는다.
/// Unix: `flock`은 열린 파일 설명에 붙어서, 다른 스레드가 자식 프로세스를 띄우는 순간(fork~exec 사이)
/// 자식이 fd를 복사해 가면 놓은 뒤에도 잠금이 잠깐 남는다(ux3: 1b `watch_lock_single_holder`가 가끔 실패한 원인).
/// 그래서 Unix는 프로세스 단위 `fcntl(F_SETLK)` 잠금(fork로 물려지지 않음)을 쓰고, 같은 프로세스 안의
/// 두 번째 요청은 잡은 경로 목록으로 막는다(POSIX 잠금은 같은 프로세스끼리 충돌하지 않으므로).
pub struct WatchLock {
    file: Option<std::fs::File>,
    #[cfg(unix)]
    key: std::path::PathBuf,
}

#[cfg(unix)]
static HELD: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());

impl WatchLock {
    pub fn try_acquire(path: &Path) -> Option<Self> {
        if let Some(p) = path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let key = match (path.parent(), path.file_name()) {
                (Some(d), Some(n)) => std::fs::canonicalize(if d.as_os_str().is_empty() { Path::new(".") } else { d }).ok()?.join(n),
                _ => path.to_path_buf(),
            };
            // 목록을 잡은 채로 열고 잠근다: 같은 프로세스가 이미 잡은 파일을 다시 열었다 닫으면
            // POSIX 잠금이 풀려 버리므로, 잡혀 있으면 열기 전에 돌아간다.
            let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
            if held.contains(&key) {
                return None;
            }
            let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path).ok()?;
            let mut fl: libc::flock = unsafe { std::mem::zeroed() };
            fl.l_type = libc::F_WRLCK as _;
            fl.l_whence = libc::SEEK_SET as _;
            if unsafe { libc::fcntl(f.as_raw_fd(), libc::F_SETLK, &fl) } != 0 {
                return None;
            }
            held.push(key.clone());
            Some(Self { file: Some(f), key })
        }
        #[cfg(not(unix))]
        {
            let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path).ok()?;
            f.try_lock().ok()?;
            Some(Self { file: Some(f) })
        }
    }
}

impl Drop for WatchLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // 닫기(= 잠금 풀기)와 목록에서 빼기를 한 번에: 그 사이에 같은 프로세스가 새로 잡지 못하게
            let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
            drop(self.file.take());
            held.retain(|k| k != &self.key);
        }
        #[cfg(not(unix))]
        drop(self.file.take());
    }
}

#[cfg(test)]
mod motw_tests {
    use super::*;
    use crate::docs::DocTarget;
    use crate::imageconv::ImageOpts;

    #[test]
    fn zone_id_parsing() {
        assert_eq!(parse_zone_id("[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://example.com/\r\n"), Some(3));
        assert_eq!(parse_zone_id("\u{feff}[zonetransfer]\n zoneid = 4 \n"), Some(4));
        assert_eq!(parse_zone_id("[ZoneTransfer]\nZoneId=2\n"), Some(2));
        assert_eq!(parse_zone_id("[Other]\nZoneId=3\n"), None);
        assert_eq!(parse_zone_id("[ZoneTransfer]\nZoneId=x\n"), None);
        assert_eq!(parse_zone_id(""), None);
    }

    #[test]
    fn external_programs_only() {
        let doc = Op::Doc { target: DocTarget::Pdf };
        assert!(uses_external_program(&doc, Path::new("a.docx")));
        let img = Op::Image(ImageOpts { format: crate::imageconv::ImgFormat::Png, quality: 90, max_side: None, keep_exif: true });
        assert!(!uses_external_program(&img, Path::new("a.jpg")));
        assert!(uses_external_program(&img, Path::new("a.heic")));
    }

    #[test]
    fn watch_skips_internet_files_for_external_jobs() {
        let t = tempfile::tempdir().unwrap();
        let folder = t.path().join("다운로드");
        std::fs::create_dir_all(&folder).unwrap();
        let doc = folder.join("받은 문서.docx");
        std::fs::write(&doc, b"PK not really a docx").unwrap();
        // 인터넷 표시(Windows는 대체 데이터 스트림, 여기서는 같은 이름의 옆 파일)
        let mut zi = doc.as_os_str().to_os_string();
        zi.push(":Zone.Identifier");
        std::fs::write(PathBuf::from(&zi), "[ZoneTransfer]\r\nZoneId=3\r\n").unwrap();
        assert!(from_internet(&doc));
        let rule = WatchRule { id: "r1".into(), folder: folder.clone(), op: Op::Doc { target: DocTarget::Pdf }, out_subfolder: "변환됨".into(), enabled: true };
        let mut w = Watcher::new(t.path().join("state.json"), Duration::from_millis(0));
        let tools = Toolset::default();
        let t0 = Instant::now();
        let _ = w.run_once(std::slice::from_ref(&rule), &tools, None, t0);
        let ev = w.run_once(std::slice::from_ref(&rule), &tools, None, t0 + Duration::from_secs(5));
        assert_eq!(ev.len(), 1);
        assert!(!ev[0].ok);
        assert_eq!(ev[0].message, INTERNET_SKIP_NOTE);
        assert!(!rule.out_dir().exists(), "열지 않았으니 출력도 없음");
        // 같은 파일은 다시 묻지 않는다
        assert!(w.run_once(std::slice::from_ref(&rule), &tools, None, t0 + Duration::from_secs(10)).is_empty());
        // 로컬 영역(ZoneId=1·2)이면 평소대로 처리(여기서는 가짜 docx라 변환 실패로 끝나지만, 건너뜀 문구는 아님)
        let doc2 = folder.join("회사 문서.docx");
        std::fs::write(&doc2, b"PK local").unwrap();
        let mut zi2 = doc2.as_os_str().to_os_string();
        zi2.push(":Zone.Identifier");
        std::fs::write(PathBuf::from(&zi2), "[ZoneTransfer]\r\nZoneId=2\r\n").unwrap();
        assert!(!from_internet(&doc2));
    }
}
