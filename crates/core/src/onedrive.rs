//! 0.3.4: OneDrive가 가져간 바탕 화면·문서·사진(·다운로드) — 어디 있는지 찾고, 안전하게 **복사**로 되돌리기.
//!
//! 지키는 것(docs/design.md `## 0.3.4`):
//! - OneDrive 폴더 안은 **읽기만** 한다. 지우거나 옮기지 않는다(되돌리기 = 복사). 쓰는 곳이 OneDrive 안이면 멈춘다.
//! - 폴더 위치 레지스트리(`User Shell Folders`)는 고치지 않는다. 백업 끄기는 OneDrive가 할 일 — 앱은 설정을 열어 주기만.
//! - 덮어쓰지 않는다: 같은 이름이 있으면 크기·SHA-256이 같으면 건너뛰고, 다르면 ` (OneDrive에서)`를 붙여 둘 다 남긴다.
//! - 복사한 파일은 크기·SHA-256을 다시 읽어 확인하고 기록(jsonl)을 남긴다. [되돌리기]는 이번에 복사한 파일 중
//!   그 뒤로 바뀌지 않았고 OneDrive 쪽 원본이 그대로 있는 것만 지운다.
//! - 클라우드에만 있는 파일(자리표시)은 개수·크기를 먼저 보여 주고, 받기로 한 경우에만 읽는다(읽으면 OneDrive가 내려받음).
//!
//! 시험: 실제 PC 대신 [`Env`]를 직접 주거나, CLI·시험용 앱에서만 `WIN_FILETOOL_ONEDRIVE_FAKE=<json 파일>`로 가짜 환경을 읽는다
//! (배포 앱은 읽지 않음 — [`crate::testhooks`]).

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::process::Cancel;

/// 시험용 가짜 환경 파일(JSON, [`Env`] 모양). CLI·시험용 앱만 읽는다.
pub const FAKE_ENV: &str = "WIN_FILETOOL_ONEDRIVE_FAKE";

/// 이름 바꿔 둘 때 붙이는 말
pub const SUFFIX: &str = " (OneDrive에서)";

/// 클라우드에만 있는 파일 표시: RECALL_ON_DATA_ACCESS · RECALL_ON_OPEN · OFFLINE
pub const CLOUD_ATTRS: u32 = 0x0040_0000 | 0x0004_0000 | 0x0000_1000;

/// 세는 파일 수 상한(아주 큰 문서 폴더에서 화면이 오래 멈추지 않게)
const COUNT_CAP: u64 = 300_000;

/// 남겨 둘 여유 공간
const SPACE_MARGIN: u64 = 64 * 1024 * 1024;

/// Windows 기본 경로 길이(이보다 길면 많은 프로그램이 열지 못해 건너뛴다)
const MAX_PATH_CHARS: usize = 259;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Folder {
    Desktop,
    Documents,
    Pictures,
    Downloads,
}

impl Folder {
    pub const ALL: [Folder; 4] = [Folder::Desktop, Folder::Documents, Folder::Pictures, Folder::Downloads];

    pub fn key(self) -> &'static str {
        match self {
            Folder::Desktop => "desktop",
            Folder::Documents => "documents",
            Folder::Pictures => "pictures",
            Folder::Downloads => "downloads",
        }
    }

    pub fn from_key(k: &str) -> Option<Folder> {
        Folder::ALL.into_iter().find(|f| f.key() == k.trim().to_ascii_lowercase())
    }

    /// 화면에 보이는 이름
    pub fn ko(self) -> &'static str {
        match self {
            Folder::Desktop => "바탕 화면",
            Folder::Documents => "문서",
            Folder::Pictures => "사진",
            Folder::Downloads => "다운로드",
        }
    }

    /// `%USERPROFILE%` 아래 기본 폴더 이름
    pub fn default_dir(self) -> &'static str {
        match self {
            Folder::Desktop => "Desktop",
            Folder::Documents => "Documents",
            Folder::Pictures => "Pictures",
            Folder::Downloads => "Downloads",
        }
    }

    /// OneDrive 폴더 안에서 이 폴더가 쓰는 이름들(영어 이름 + 한국어 Windows에서 보이는 이름)
    pub fn onedrive_names(self) -> &'static [&'static str] {
        match self {
            Folder::Desktop => &["Desktop", "바탕 화면"],
            Folder::Documents => &["Documents", "문서"],
            Folder::Pictures => &["Pictures", "사진"],
            Folder::Downloads => &["Downloads", "다운로드"],
        }
    }
}

/// OneDrive 계정(레지스트리 `HKCU\Software\Microsoft\OneDrive\Accounts\*\UserFolder`)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Account {
    pub root: PathBuf,
    #[serde(default)]
    pub business: bool,
    #[serde(default = "yes")]
    pub signed_in: bool,
}

fn yes() -> bool {
    true
}

/// 진단에 쓰는 이 PC의 상태(실제 PC는 [`Env::detect`], 시험은 직접 만들거나 가짜 파일)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Env {
    /// `%USERPROFILE%`
    pub profile: PathBuf,
    /// 지금 Windows가 쓰는 폴더 위치(키: desktop·documents·pictures·downloads)
    #[serde(default)]
    pub folders: BTreeMap<String, PathBuf>,
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// OneDrive.exe가 돌고 있는지
    #[serde(default)]
    pub onedrive_running: bool,
    /// 시험용: 이름이 이것으로 끝나는 파일은 클라우드에만 있는 것으로 본다
    #[serde(default)]
    pub cloud_names: Vec<String>,
    /// 가짜 환경인지(화면에 "시험용" 표시)
    #[serde(default)]
    pub fake: bool,
}

impl Env {
    /// 시험용 가짜(켜져 있을 때만) 또는 이 PC
    pub fn current() -> Result<Env> {
        if let Some(p) = crate::testhooks::var(FAKE_ENV) {
            let s = std::fs::read_to_string(&p).map_err(|e| Error::Invalid(format!("시험용 OneDrive 환경 파일을 읽지 못했습니다({p}): {e}")))?;
            let mut env: Env = serde_json::from_str(&s).map_err(|e| Error::Invalid(format!("시험용 OneDrive 환경 파일 형식 오류: {e}")))?;
            env.fake = true;
            return Ok(env);
        }
        Ok(Env::detect())
    }

    /// 이 PC에서 읽기만 해서 알아낸다(레지스트리·폴더 위치·프로세스 목록)
    pub fn detect() -> Env {
        let mut env = Env { profile: sys::profile(), ..Default::default() };
        for f in Folder::ALL {
            if let Some(p) = sys::known_folder(f) {
                env.folders.insert(f.key().into(), p);
            }
        }
        env.accounts = sys::accounts();
        env.onedrive_running = sys::onedrive_running();
        env
    }

    fn roots(&self) -> impl Iterator<Item = &Path> {
        self.accounts.iter().map(|a| a.root.as_path())
    }

    /// 이 경로가 OneDrive 폴더 안인지(계정 폴더, 또는 계정이 지워졌어도 프로필 아래 `OneDrive…` 폴더)
    pub fn onedrive_root_of(&self, p: &Path) -> Option<PathBuf> {
        if let Some(r) = self.roots().find(|r| inside(p, r)) {
            return Some(r.to_path_buf());
        }
        if inside(p, &self.profile) {
            let rel = rel_components(p, &self.profile);
            if let Some(first) = rel.first() {
                if first.to_lowercase().starts_with("onedrive") {
                    return Some(self.profile.join(first));
                }
            }
        }
        None
    }

    fn is_cloud(&self, path: &Path, md: &std::fs::Metadata) -> bool {
        if attrs_cloud(md) {
            return true;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        self.cloud_names.iter().any(|c| !c.is_empty() && name.ends_with(&c.to_lowercase()))
    }
}

#[cfg(windows)]
fn attrs_cloud(md: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    md.file_attributes() & CLOUD_ATTRS != 0
}

#[cfg(not(windows))]
fn attrs_cloud(_md: &std::fs::Metadata) -> bool {
    false
}

fn lc(c: &Component) -> String {
    c.as_os_str().to_string_lossy().to_lowercase()
}

/// `p`가 `root`와 같거나 그 안인지(대소문자 무시 — Windows 경로)
pub fn inside(p: &Path, root: &Path) -> bool {
    if root.as_os_str().is_empty() {
        return false;
    }
    let (a, b): (Vec<_>, Vec<_>) = (p.components().collect(), root.components().collect());
    b.len() <= a.len() && a.iter().zip(&b).all(|(x, y)| lc(x) == lc(y))
}

fn rel_components(p: &Path, root: &Path) -> Vec<String> {
    p.components().skip(root.components().count()).map(|c| c.as_os_str().to_string_lossy().into_owned()).collect()
}

/// 세지도, 옮기지도 않는 시스템 파일(폴더 이름 표시·미리보기 그림 저장)
fn ignored(name: &str) -> bool {
    let n = name.to_lowercase();
    n == "desktop.ini" || n == "thumbs.db"
}

// ---------- 진단 ----------

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Count {
    pub files: u64,
    pub bytes: u64,
    /// 클라우드에만 있는 파일(내려받아야 열림)
    pub cloud_files: u64,
    pub cloud_bytes: u64,
    /// 너무 많아 다 세지 못함
    pub truncated: bool,
}

/// 폴더 안 파일 세기(하위 폴더 포함). 파일 내용은 열지 않는다(클라우드 파일도 내려받지 않음).
pub fn count(env: &Env, dir: &Path) -> Count {
    let mut c = Count::default();
    for e in walkdir::WalkDir::new(dir).follow_links(false).min_depth(1).into_iter().filter_map(|e| e.ok()) {
        if !e.file_type().is_file() || ignored(&e.file_name().to_string_lossy()) {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        c.files += 1;
        c.bytes += md.len();
        if env.is_cloud(e.path(), &md) {
            c.cloud_files += 1;
            c.cloud_bytes += md.len();
        }
        if c.files >= COUNT_CAP {
            c.truncated = true;
            break;
        }
    }
    c
}

/// A 백업 켜짐(OneDrive 안) · B 백업 꺼졌는데 파일이 OneDrive에 남음 · C 정상 · D OneDrive 폴더가 없음/지워짐
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum Case {
    A,
    B,
    C,
    D,
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderReport {
    pub key: &'static str,
    pub name: &'static str,
    pub case: Case,
    /// 한 줄 제목(쉬운 말)
    pub title: String,
    /// 설명
    pub detail: String,
    /// 지금 Windows가 쓰는 위치
    pub current: PathBuf,
    pub current_exists: bool,
    pub in_onedrive: bool,
    /// 지금 위치의 파일
    pub here: Count,
    /// B: OneDrive 안에 남은 같은 폴더
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onedrive_copy: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onedrive_count: Option<Count>,
    /// 이 PC의 기본 위치(`%USERPROFILE%\Desktop` 등)
    pub local_default: PathBuf,
    pub local_default_exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_default_count: Option<Count>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    pub root: PathBuf,
    pub business: bool,
    pub signed_in: bool,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub folders: Vec<FolderReport>,
    pub accounts: Vec<AccountView>,
    pub onedrive_running: bool,
    /// OneDrive 계정이 하나라도 이 PC에 연결돼 있는지
    pub onedrive_set_up: bool,
    /// 한 줄 요약
    pub summary: String,
    pub fake: bool,
    /// 안내 문구(화면·CLI 공용)
    pub backup_off_steps: &'static str,
    pub delete_warning: &'static str,
    pub recycle_help: &'static str,
}

pub fn human(n: u64) -> String {
    let n = n as f64;
    if n < 1024.0 {
        return format!("{n} B");
    }
    let mut v = n / 1024.0;
    for u in ["KB", "MB", "GB"] {
        if v < 1024.0 {
            return format!("{:.1} {u}", v);
        }
        v /= 1024.0;
    }
    format!("{v:.1} TB")
}

fn count_text(c: &Count) -> String {
    let more = if c.truncated { " 이상" } else { "" };
    let mut s = format!("파일 {}개{more}, {}", c.files, human(c.bytes));
    if c.cloud_files > 0 {
        s += &format!(" (그중 {}개 {}는 클라우드에만 있음)", c.cloud_files, human(c.cloud_bytes));
    }
    s
}

/// 받침에 맞춘 조사(바탕 화면은 · 문서는)
fn josa(name: &str, with: &str, without: &str) -> String {
    let last = name.chars().last().map(|c| c as u32).unwrap_or(0);
    let batchim = (0xAC00..=0xD7A3).contains(&last) && (last - 0xAC00) % 28 != 0;
    format!("{name}{}", if batchim { with } else { without })
}

/// 폴더 하나 진단(읽기만)
pub fn diagnose_folder(env: &Env, f: Folder) -> FolderReport {
    let local_default = env.profile.join(f.default_dir());
    let current = env.folders.get(f.key()).cloned().unwrap_or_else(|| local_default.clone());
    let current_exists = current.is_dir();
    let od_root = env.onedrive_root_of(&current);
    let in_onedrive = od_root.is_some();
    let here = if current_exists { count(env, &current) } else { Count::default() };
    let same_as_default = inside(&current, &local_default) && inside(&local_default, &current);
    let local_default_exists = local_default.is_dir();
    let local_default_count = (!same_as_default && local_default_exists).then(|| count(env, &local_default));
    let name = f.ko();
    let (eun, iga, eul) = (josa(name, "은", "는"), josa(name, "이", "가"), josa(name, "을", "를"));
    let mut r = FolderReport {
        key: f.key(),
        name,
        case: Case::C,
        title: String::new(),
        detail: String::new(),
        current: current.clone(),
        current_exists,
        in_onedrive,
        here,
        onedrive_copy: None,
        onedrive_count: None,
        local_default,
        local_default_exists,
        local_default_count,
    };
    if in_onedrive {
        if current_exists {
            r.case = Case::A;
            r.title = format!("백업 켜짐 — {name} 파일은 OneDrive 안에 있어요");
            r.detail = format!(
                "Windows가 {eul} OneDrive 폴더({})로 쓰고 있어요. 파일은 사라지지 않았어요: {}.{}",
                current.display(),
                count_text(&r.here),
                if env.onedrive_running { "" } else { " 지금 OneDrive가 꺼져 있어요." }
            );
        } else {
            r.case = Case::D;
            r.title = format!("{name} 폴더가 가리키는 OneDrive 폴더가 없어요");
            r.detail = format!("Windows는 {eul} {}에서 찾지만 그 폴더가 없어요(OneDrive 폴더가 지워졌거나 옮겨짐).", current.display());
        }
        return r;
    }
    if !current_exists {
        r.case = Case::D;
        r.title = format!("{name} 폴더가 없어요");
        r.detail = format!("{} 폴더가 없어요.", current.display());
        return r;
    }
    // 지금은 이 PC 폴더를 쓰는데 OneDrive 안에 같은 폴더가 남아 있는지
    for root in env.roots().filter(|r| r.is_dir()) {
        for n in f.onedrive_names() {
            let d = root.join(n);
            if !d.is_dir() || inside(&current, &d) {
                continue;
            }
            let c = count(env, &d);
            if c.files > 0 {
                r.case = Case::B;
                r.title = format!("백업은 꺼졌는데 {name} 파일이 OneDrive 폴더에 남아 있어요");
                r.detail = format!(
                    "{iga} 비어 보이는 까닭이에요. 파일은 {}에 있어요: {}. [가져오기]는 복사만 해요 — OneDrive 쪽은 그대로 둬요.",
                    d.display(),
                    count_text(&c)
                );
                r.onedrive_copy = Some(d);
                r.onedrive_count = Some(c);
                return r;
            }
        }
    }
    let missing_root = env.accounts.iter().any(|a| !a.root.is_dir());
    if r.here.files == 0 && missing_root {
        r.case = Case::D;
        r.title = format!("{iga} 비어 있고 OneDrive 폴더도 없어요");
        r.detail = "OneDrive 폴더를 지웠다면 파일은 OneDrive 웹의 휴지통(30일, 회사·학교 계정은 93일)이나 이 PC 휴지통에 있을 수 있어요.".into();
        return r;
    }
    r.case = Case::C;
    r.title = format!("{eun} 이 PC 폴더에 있어요 — 할 일 없어요");
    r.detail = format!("{}: {}.", current.display(), count_text(&r.here));
    r
}

/// 바탕 화면·문서·사진(+ 다운로드는 OneDrive와 얽혔을 때만) 진단
pub fn diagnose(env: &Env) -> Report {
    let mut folders = Vec::new();
    for f in Folder::ALL {
        let r = diagnose_folder(env, f);
        if f == Folder::Downloads && !r.in_onedrive && r.case != Case::B {
            continue;
        }
        folders.push(r);
    }
    let accounts: Vec<AccountView> = env
        .accounts
        .iter()
        .map(|a| AccountView { root: a.root.clone(), business: a.business, signed_in: a.signed_in, exists: a.root.is_dir() })
        .collect();
    let set_up = accounts.iter().any(|a| a.signed_in);
    let n = |c: Case| folders.iter().filter(|f| f.case == c).count();
    let summary = if n(Case::B) > 0 {
        format!("{}개 폴더의 파일이 OneDrive 폴더에 남아 있어요 — [가져오기]로 복사할 수 있어요.", n(Case::B))
    } else if n(Case::A) > 0 {
        format!("{}개 폴더를 OneDrive가 백업하고 있어요. 파일은 OneDrive 안에 있어요.", n(Case::A))
    } else if n(Case::D) > 0 {
        "OneDrive 폴더가 없어요 — 휴지통을 확인해 보세요.".into()
    } else if !set_up {
        "OneDrive가 이 PC에 설정되어 있지 않아요. 바탕 화면·문서·사진은 이 PC 폴더에 있어요 — 할 일 없어요.".into()
    } else {
        "바탕 화면·문서·사진은 이 PC 폴더에 있어요 — 할 일 없어요.".into()
    };
    Report {
        folders,
        accounts,
        onedrive_running: env.onedrive_running,
        onedrive_set_up: set_up,
        summary,
        fake: env.fake,
        backup_off_steps: BACKUP_OFF_STEPS,
        delete_warning: DELETE_WARNING,
        recycle_help: RECYCLE_HELP,
    }
}

// ---------- 가져오기(복사) ----------

#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub path: PathBuf,
    pub reason: String,
}

/// 미리 보기(--dry-run)에서 한 파일에 할 일
#[derive(Debug, Clone, Serialize)]
pub struct PlanItem {
    pub rel: String,
    /// copy · same · rename · cloud(받지 않음) · skip
    pub action: &'static str,
    pub dest: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RestoreOut {
    pub id: String,
    pub from: PathBuf,
    pub to: PathBuf,
    pub dry_run: bool,
    /// 이름 그대로 가져옴
    pub copied: u64,
    /// 같아서(크기·SHA-256) 건너뜀
    pub same: u64,
    /// 이름이 겹쳐 ` (OneDrive에서)`를 붙여 가져옴
    pub renamed: u64,
    pub copied_bytes: u64,
    /// 클라우드에만 있어 받지 않음
    pub cloud_skipped: u64,
    pub cloud_skipped_bytes: u64,
    pub skipped: Vec<Skipped>,
    pub cancelled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<PathBuf>,
    /// 미리 보기 항목(dry-run만, 앞 500개)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub plan: Vec<PlanItem>,
    pub plan_total: u64,
    /// 한 줄 결과
    pub summary: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RestoreOpts {
    /// 클라우드에만 있는 파일도 내려받아 가져온다(사용자가 "받기"를 고른 경우)
    pub allow_download: bool,
    pub dry_run: bool,
}

struct Src {
    path: PathBuf,
    rel: PathBuf,
    size: u64,
    cloud: bool,
}

fn sha_file(path: &Path, cancel: Option<&Cancel>) -> std::io::Result<(u64, [u8; 32])> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut n = 0u64;
    loop {
        if cancel.is_some_and(|c| c.is_cancelled()) {
            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
        }
        let k = f.read(&mut buf)?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((n, h.finalize().into()))
}

fn hex(b: &[u8; 32]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// 읽기·쓰기 실패를 쉬운 말로
fn io_reason(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(32) | Some(33) => "다른 프로그램이 쓰고 있어요(잠김)".into(),
        Some(5) => "권한이 없어요(읽기 전용이거나 막힘)".into(),
        Some(362) => "OneDrive가 꺼져 있어 내려받지 못했어요".into(),
        Some(c) if (358..=400).contains(&c) => format!("OneDrive에서 내려받지 못했어요 — 인터넷 연결과 OneDrive 로그인을 확인해 주세요(오류 {c})"),
        Some(206) | Some(3) => "경로가 너무 길어요".into(),
        Some(112) => "공간이 모자라요".into(),
        _ => match e.kind() {
            std::io::ErrorKind::PermissionDenied => "권한이 없어요(읽기 전용이거나 막힘)".into(),
            std::io::ErrorKind::NotFound => "파일이 없어졌어요".into(),
            _ => e.to_string(),
        },
    }
}

enum CopyErr {
    Cancelled,
    Fail(String),
}

/// 새 파일로만 복사(이미 있으면 실패) → 다시 읽어 크기·SHA-256 확인. 다르면 만든 파일을 지운다.
fn copy_verified(src: &Path, dst: &Path, cancel: &Cancel) -> std::result::Result<(u64, [u8; 32]), CopyErr> {
    let mut input = File::open(src).map_err(|e| CopyErr::Fail(io_reason(&e)))?;
    let src_md = input.metadata().ok();
    let mut out = OpenOptions::new().write(true).create_new(true).open(dst).map_err(|e| CopyErr::Fail(io_reason(&e)))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut n = 0u64;
    let fail = |dst: &Path, e: CopyErr| {
        let _ = std::fs::remove_file(dst);
        e
    };
    loop {
        if cancel.is_cancelled() {
            drop(out);
            return Err(fail(dst, CopyErr::Cancelled));
        }
        let k = match input.read(&mut buf) {
            Ok(k) => k,
            Err(e) => {
                drop(out);
                return Err(fail(dst, CopyErr::Fail(io_reason(&e))));
            }
        };
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        if let Err(e) = out.write_all(&buf[..k]) {
            drop(out);
            return Err(fail(dst, CopyErr::Fail(io_reason(&e))));
        }
        n += k as u64;
    }
    if let Err(e) = out.sync_all() {
        drop(out);
        return Err(fail(dst, CopyErr::Fail(io_reason(&e))));
    }
    if let Some(t) = src_md.as_ref().and_then(|m| m.modified().ok()) {
        let _ = out.set_modified(t);
    }
    drop(out);
    let want: [u8; 32] = h.finalize().into();
    match sha_file(dst, None) {
        Ok((m, got)) if m == n && got == want => Ok((n, want)),
        Ok(_) => Err(fail(dst, CopyErr::Fail("복사한 내용이 원본과 달라 지웠어요".into()))),
        Err(e) => Err(fail(dst, CopyErr::Fail(io_reason(&e)))),
    }
}

/// ` (OneDrive에서)`, ` (OneDrive에서 2)` …
fn suffixed(p: &Path, n: u32) -> PathBuf {
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let tag = if n <= 1 { SUFFIX.to_string() } else { format!(" (OneDrive에서 {n})") };
    p.with_file_name(format!("{stem}{tag}{ext}"))
}

fn same_file(a_size: u64, a_hash: &[u8; 32], b: &Path) -> bool {
    match std::fs::metadata(b) {
        Ok(m) if m.is_file() && m.len() == a_size => sha_file(b, None).is_ok_and(|(_, h)| &h == a_hash),
        _ => false,
    }
}

/// 기록 번호(시각-프로세스-순번). 같은 폴더에 이미 있으면 순번을 올린다
fn run_id(log_dir: &Path) -> String {
    let base = format!("{}-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), std::process::id());
    (1..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !log_dir.join(format!("{id}.jsonl")).exists() && !log_dir.join(format!("{id}.jsonl.undone")).exists())
        .unwrap()
}

/// 이 폴더에 쓰지 않는다: OneDrive 안이거나 원본 폴더 안
fn guard_dest(env: &Env, from: &Path, to: &Path) -> Result<()> {
    if env.onedrive_root_of(to).is_some() || inside(to, from) || inside(from, to) {
        return Err(Error::Invalid(format!("OneDrive 폴더 안에는 쓰지 않아요: {}", to.display())));
    }
    Ok(())
}

/// 로그 한 줄(jsonl)
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t")]
enum LogLine {
    #[serde(rename = "run")]
    Run { id: String, from: PathBuf, to: PathBuf, folder: String },
    #[serde(rename = "dir")]
    Dir { path: PathBuf },
    #[serde(rename = "file")]
    File { dest: PathBuf, src: PathBuf, size: u64, sha256: String },
}

struct Log {
    f: Option<File>,
    path: PathBuf,
}

impl Log {
    fn write(&mut self, l: &LogLine) {
        if let Some(f) = self.f.as_mut() {
            let _ = writeln!(f, "{}", serde_json::to_string(l).unwrap_or_default());
            let _ = f.flush();
        }
    }
}

/// 쓰기 전에 폴더를 만든다(새로 만든 것은 기록 — 되돌리기 때 비었으면 지움)
fn ensure_dirs(dest_root: &Path, rel_parent: &Path, log: &mut Log, dry: bool) -> std::result::Result<(), String> {
    let mut d = dest_root.to_path_buf();
    for c in rel_parent.components() {
        d.push(c);
        match std::fs::symlink_metadata(&d) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(format!("같은 이름의 파일이 있어 폴더를 만들 수 없어요: {}", d.display())),
            Err(_) if dry => {}
            Err(_) => {
                std::fs::create_dir(&d).map_err(|e| io_reason(&e))?;
                log.write(&LogLine::Dir { path: d.clone() });
            }
        }
    }
    Ok(())
}

/// B인 폴더의 파일을 OneDrive 폴더에서 이 PC 폴더로 **복사**한다. `progress(done, total, 지금 파일)`.
pub fn restore(env: &Env, folder: Folder, opts: RestoreOpts, log_dir: &Path, cancel: &Cancel, progress: &mut dyn FnMut(u64, u64, &str)) -> Result<RestoreOut> {
    let rep = diagnose_folder(env, folder);
    let eun = josa(folder.ko(), "은", "는");
    let from = match (&rep.case, &rep.onedrive_copy) {
        (Case::B, Some(d)) => d.clone(),
        (Case::A, _) => return Err(Error::Invalid(format!("{eun} 아직 OneDrive가 백업하고 있어요. 먼저 OneDrive에서 백업을 끄고 [다시 확인]을 눌러 주세요."))),
        _ => return Err(Error::Invalid(format!("{eun} 가져올 것이 없어요."))),
    };
    let to = rep.current.clone();
    guard_dest(env, &from, &to)?;

    // 1) 원본 목록(내용은 아직 읽지 않음)
    let mut srcs = Vec::new();
    for e in walkdir::WalkDir::new(&from).follow_links(false).min_depth(1).sort_by_file_name().into_iter() {
        cancel.check()?;
        let Ok(e) = e else { continue };
        if !e.file_type().is_file() || ignored(&e.file_name().to_string_lossy()) {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        let rel = e.path().strip_prefix(&from).unwrap_or(e.path()).to_path_buf();
        srcs.push(Src { cloud: env.is_cloud(e.path(), &md), path: e.path().to_path_buf(), rel, size: md.len() });
    }
    let total = srcs.len() as u64;
    let cloud_n = srcs.iter().filter(|s| s.cloud).count();
    if !opts.dry_run && opts.allow_download && cloud_n > 0 && !env.onedrive_running {
        return Err(Error::Invalid(format!(
            "클라우드에만 있는 파일 {cloud_n}개를 받으려면 OneDrive가 켜져 있고 로그인되어 있어야 해요. 지금은 OneDrive가 꺼져 있어요. 받지 않고 이 PC에 있는 파일만 가져오려면 [받아서 같이 가져오기]를 끄고 다시 해 주세요."
        )));
    }
    // 2) 남은 공간(이름이 겹쳐도 크기가 같으면 같은 파일일 수 있으니 빼고 셈)
    let need: u64 = srcs
        .iter()
        .filter(|s| !s.cloud || opts.allow_download)
        .filter(|s| std::fs::metadata(to.join(&s.rel)).map(|m| m.len() != s.size).unwrap_or(true))
        .map(|s| s.size)
        .sum();
    if !opts.dry_run {
        if let Some(free) = crate::paths::free_space(&to) {
            if free < need + SPACE_MARGIN {
                return Err(Error::Invalid(format!("공간이 모자라요: {} 필요, 남은 공간 {}. 공간을 비우고 다시 해 주세요.", human(need + SPACE_MARGIN), human(free))));
            }
        }
    }

    let id = run_id(log_dir);
    let mut out = RestoreOut { id: id.clone(), from: from.clone(), to: to.clone(), dry_run: opts.dry_run, plan_total: total, ..Default::default() };
    let mut log = Log { f: None, path: log_dir.join(format!("{id}.jsonl")) };
    if !opts.dry_run && total > 0 {
        std::fs::create_dir_all(log_dir).map_err(crate::error::io_err(log_dir))?;
        let f = OpenOptions::new().create_new(true).append(true).open(&log.path).map_err(crate::error::io_err(&log.path))?;
        log.f = Some(f);
        log.write(&LogLine::Run { id: id.clone(), from: from.clone(), to: to.clone(), folder: folder.key().into() });
        out.log = Some(log.path.clone());
    }
    let plan = |out: &mut RestoreOut, rel: &Path, action: &'static str, dest: &Path, reason: Option<String>| {
        if out.dry_run && out.plan.len() < 500 {
            out.plan.push(PlanItem { rel: rel.to_string_lossy().into_owned(), action, dest: dest.to_path_buf(), reason });
        }
    };

    for (i, s) in srcs.iter().enumerate() {
        if cancel.is_cancelled() {
            out.cancelled = true;
            break;
        }
        progress(i as u64, total, &s.rel.to_string_lossy());
        let dest = to.join(&s.rel);
        let skip = |out: &mut RestoreOut, reason: String| out.skipped.push(Skipped { path: s.path.clone(), reason });
        if dest.to_string_lossy().chars().count() > MAX_PATH_CHARS {
            plan(&mut out, &s.rel, "skip", &dest, Some("경로가 너무 길어요(260자 넘음)".into()));
            skip(&mut out, "경로가 너무 길어요(260자 넘음)".into());
            continue;
        }
        if let Err(r) = ensure_dirs(&to, s.rel.parent().unwrap_or(Path::new("")), &mut log, opts.dry_run) {
            plan(&mut out, &s.rel, "skip", &dest, Some(r.clone()));
            skip(&mut out, r);
            continue;
        }
        if s.cloud && !opts.allow_download {
            out.cloud_skipped += 1;
            out.cloud_skipped_bytes += s.size;
            plan(&mut out, &s.rel, "cloud", &dest, None);
            continue;
        }
        // 같은 이름이 있으면: 같은 내용이면 건너뛰고, 다르면 이름을 바꿔 둔다
        let mut target = dest.clone();
        let mut renamed = false;
        if std::fs::symlink_metadata(&dest).is_ok() {
            if opts.dry_run && s.cloud {
                plan(&mut out, &s.rel, "rename", &suffixed(&dest, 1), Some("같은 이름이 있음(클라우드 파일이라 내용 비교는 가져올 때)".into()));
                out.renamed += 1;
                continue;
            }
            let (size, hash) = match sha_file(&s.path, Some(cancel)) {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                    out.cancelled = true;
                    break;
                }
                Err(e) => {
                    plan(&mut out, &s.rel, "skip", &dest, Some(io_reason(&e)));
                    skip(&mut out, io_reason(&e));
                    continue;
                }
            };
            let mut n = 0u32;
            let mut is_same = same_file(size, &hash, &dest);
            while !is_same {
                n += 1;
                target = suffixed(&dest, n);
                if std::fs::symlink_metadata(&target).is_err() {
                    break;
                }
                is_same = same_file(size, &hash, &target);
            }
            if is_same {
                out.same += 1;
                plan(&mut out, &s.rel, "same", &target, None);
                continue;
            }
            renamed = true;
        }
        if opts.dry_run {
            if renamed {
                out.renamed += 1;
            } else {
                out.copied += 1;
            }
            out.copied_bytes += s.size;
            plan(&mut out, &s.rel, if renamed { "rename" } else { "copy" }, &target, None);
            continue;
        }
        match copy_verified(&s.path, &target, cancel) {
            Ok((size, hash)) => {
                log.write(&LogLine::File { dest: target.clone(), src: s.path.clone(), size, sha256: hex(&hash) });
                if renamed {
                    out.renamed += 1;
                } else {
                    out.copied += 1;
                }
                out.copied_bytes += size;
            }
            Err(CopyErr::Cancelled) => {
                out.cancelled = true;
                break;
            }
            Err(CopyErr::Fail(r)) => skip(&mut out, r),
        }
    }
    progress(total, total, "");
    drop(log);
    out.summary = restore_summary(&out);
    Ok(out)
}

pub fn restore_summary(o: &RestoreOut) -> String {
    let verb = if o.dry_run { "가져올 것" } else { "가져옴" };
    let mut s = format!("{}개 {verb}, {}개 같아서 건너뜀, {}개 이름 바꿔 둠", o.copied, o.same, o.renamed);
    if o.cloud_skipped > 0 {
        s += &format!(", 클라우드에만 있는 {}개({})는 받지 않음", o.cloud_skipped, human(o.cloud_skipped_bytes));
    }
    if !o.skipped.is_empty() {
        s += &format!(", {}개는 못 가져옴", o.skipped.len());
    }
    if o.cancelled {
        s += " — 중간에 멈췄어요";
    }
    s
}

// ---------- 되돌리기 ----------

#[derive(Debug, Clone, Serialize, Default)]
pub struct UndoOut {
    /// 지운 복사본
    pub removed: u64,
    /// 가져온 뒤 바뀌어서 남겨 둠
    pub kept_changed: u64,
    /// OneDrive 쪽 원본이 없어져 남겨 둠(지우면 하나도 안 남음)
    pub kept_no_source: u64,
    /// 이미 없음
    pub gone: u64,
    pub dirs_removed: u64,
    pub failed: Vec<Skipped>,
    pub summary: String,
}

/// 기록 파일 이름(id)만 받는다(경로 문자 거절)
pub fn log_path(log_dir: &Path, id: &str) -> Result<PathBuf> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(Error::Invalid(format!("되돌리기 번호가 올바르지 않습니다: {id}")));
    }
    Ok(log_dir.join(format!("{id}.jsonl")))
}

/// 이번에 복사한 파일 중 바뀌지 않았고 OneDrive 쪽 원본이 그대로인 것만 지운다. OneDrive 안은 건드리지 않는다.
pub fn undo(env: &Env, log_dir: &Path, id: &str) -> Result<UndoOut> {
    let path = log_path(log_dir, id)?;
    let f = File::open(&path).map_err(|_| Error::Invalid("되돌릴 기록이 없어요(이미 되돌렸거나 지워짐).".into()))?;
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for line in BufReader::new(f).lines().map_while(|l| l.ok()) {
        match serde_json::from_str::<LogLine>(&line) {
            Ok(LogLine::File { dest, src, size, sha256 }) => files.push((dest, src, size, sha256)),
            Ok(LogLine::Dir { path }) => dirs.push(path),
            _ => {}
        }
    }
    let mut o = UndoOut::default();
    for (dest, src, size, sha) in files {
        if env.onedrive_root_of(&dest).is_some() {
            o.failed.push(Skipped { path: dest, reason: "OneDrive 폴더 안이라 지우지 않아요".into() });
            continue;
        }
        let Ok(md) = std::fs::symlink_metadata(&dest) else {
            o.gone += 1;
            continue;
        };
        let unchanged = md.is_file() && md.len() == size && sha_file(&dest, None).is_ok_and(|(_, h)| hex(&h) == sha);
        if !unchanged {
            o.kept_changed += 1;
            continue;
        }
        if !std::fs::metadata(&src).is_ok_and(|m| m.is_file() && m.len() == size) {
            o.kept_no_source += 1;
            continue;
        }
        match std::fs::remove_file(&dest) {
            Ok(()) => o.removed += 1,
            Err(e) => o.failed.push(Skipped { path: dest, reason: io_reason(&e) }),
        }
    }
    for d in dirs.iter().rev() {
        if env.onedrive_root_of(d).is_none() && std::fs::remove_dir(d).is_ok() {
            o.dirs_removed += 1;
        }
    }
    let _ = std::fs::rename(&path, path.with_extension("jsonl.undone"));
    let mut s = format!("복사본 {}개를 지웠어요", o.removed);
    if o.kept_changed > 0 {
        s += &format!(", 가져온 뒤 바뀐 {}개는 남겨 뒀어요", o.kept_changed);
    }
    if o.kept_no_source > 0 {
        s += &format!(", OneDrive 쪽 원본이 없어진 {}개는 남겨 뒀어요", o.kept_no_source);
    }
    if !o.failed.is_empty() {
        s += &format!(", {}개는 지우지 못했어요", o.failed.len());
    }
    o.summary = s;
    Ok(o)
}

/// 안내 문구(화면·CLI 공용)
pub const BACKUP_OFF_STEPS: &str = "작업 표시줄 오른쪽의 OneDrive 구름 아이콘 → 오른쪽 위 톱니바퀴(도움말 및 설정) → [설정] → [동기화 및 백업] → [백업 관리] → 끌 폴더의 스위치를 끄고 [백업 중지]. OneDrive가 \"파일은 OneDrive에 남아요\"라고 하면 그대로 두고, 여기로 돌아와 [다시 확인]을 누르세요.";
pub const DELETE_WARNING: &str = "OneDrive 폴더에서 지우면 클라우드(OneDrive 웹)와 같은 계정의 다른 PC에서도 지워져요. 가져온 뒤에도 OneDrive 쪽은 그대로 두는 것이 안전해요.";
pub const RECYCLE_HELP: &str = "OneDrive 폴더를 지웠다면: ① 이 PC의 휴지통 ② OneDrive 웹(onedrive.live.com)에 로그인 → 왼쪽 [휴지통] — 개인 계정은 30일, 회사·학교 계정은 93일 동안 살릴 수 있어요.";
pub const WEB_URL: &str = "https://onedrive.live.com/";
/// Windows 설정 → 계정 → Windows 백업(여기서 [동기화 설정 관리]가 OneDrive의 백업 관리를 연다)
pub const BACKUP_SETTINGS_URI: &str = "ms-settings:backup";

// ---------- Windows: 읽기만 ----------

#[cfg(windows)]
mod sys {
    use super::*;

    pub fn profile() -> PathBuf {
        known(&windows_sys::Win32::UI::Shell::FOLDERID_Profile)
            .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
            .unwrap_or_default()
    }

    fn known(id: &windows_sys::core::GUID) -> Option<PathBuf> {
        use std::os::windows::ffi::OsStringExt;
        use windows_sys::Win32::System::Com::CoTaskMemFree;
        use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;
        let mut p: windows_sys::core::PWSTR = std::ptr::null_mut();
        // KF_FLAG_DONT_VERIFY(0x4000): 폴더가 없어도 만들지 않고 경로만
        let hr = unsafe { SHGetKnownFolderPath(id, 0x4000, std::ptr::null_mut(), &mut p) };
        let out = if hr >= 0 && !p.is_null() {
            let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
            let s = unsafe { std::slice::from_raw_parts(p, len) };
            Some(PathBuf::from(std::ffi::OsString::from_wide(s)))
        } else {
            None
        };
        if !p.is_null() {
            unsafe { CoTaskMemFree(p as *const _) };
        }
        out
    }

    pub fn known_folder(f: Folder) -> Option<PathBuf> {
        use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Pictures};
        let id = match f {
            Folder::Desktop => &FOLDERID_Desktop,
            Folder::Documents => &FOLDERID_Documents,
            Folder::Pictures => &FOLDERID_Pictures,
            Folder::Downloads => &FOLDERID_Downloads,
        };
        known(id)
    }

    pub fn accounts() -> Vec<Account> {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
        let Ok(k) = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(r"Software\Microsoft\OneDrive\Accounts", KEY_READ) else {
            return Vec::new();
        };
        let mut v = Vec::new();
        for name in k.enum_keys().filter_map(|n| n.ok()) {
            let Ok(a) = k.open_subkey_with_flags(&name, KEY_READ) else { continue };
            let folder: String = a.get_value("UserFolder").unwrap_or_default();
            if folder.trim().is_empty() {
                continue;
            }
            let email: String = a.get_value("UserEmail").unwrap_or_default();
            let cid: String = a.get_value("cid").unwrap_or_default();
            v.push(Account {
                root: PathBuf::from(folder.trim()),
                business: name.to_lowercase().starts_with("business"),
                signed_in: !email.trim().is_empty() || !cid.trim().is_empty(),
            });
        }
        v
    }

    pub fn onedrive_running() -> bool {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut ok = unsafe { Process32FirstW(snap, &mut e) } != 0;
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            if String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case("OneDrive.exe") {
                found = true;
                break;
            }
            ok = unsafe { Process32NextW(snap, &mut e) } != 0;
        }
        unsafe { CloseHandle(snap) };
        found
    }
}

#[cfg(not(windows))]
mod sys {
    use super::*;

    pub fn profile() -> PathBuf {
        std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
    }
    pub fn known_folder(_f: Folder) -> Option<PathBuf> {
        None
    }
    pub fn accounts() -> Vec<Account> {
        Vec::new()
    }
    pub fn onedrive_running() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fx {
        _t: tempfile::TempDir,
        env: Env,
        od: PathBuf,
        local: PathBuf,
        logs: PathBuf,
    }

    /// 프로필 + OneDrive 폴더(바탕 화면에 파일) + 비어 있는 이 PC 바탕 화면 = B
    fn fx() -> Fx {
        let t = tempfile::tempdir().unwrap();
        let profile = t.path().join("Users").join("kim");
        let od = profile.join("OneDrive");
        let odd = od.join("바탕 화면");
        std::fs::create_dir_all(odd.join("보고서")).unwrap();
        std::fs::write(odd.join("메모.txt"), "OneDrive 메모").unwrap();
        std::fs::write(odd.join("같은.txt"), "같은 내용").unwrap();
        std::fs::write(odd.join("보고서").join("1분기.docx"), "보고서 내용").unwrap();
        std::fs::write(odd.join("desktop.ini"), "[.ShellClassInfo]").unwrap();
        let local = profile.join("Desktop");
        std::fs::create_dir_all(&local).unwrap();
        for d in ["Documents", "Pictures", "Downloads"] {
            std::fs::create_dir_all(profile.join(d)).unwrap();
        }
        let mut folders = BTreeMap::new();
        for f in Folder::ALL {
            folders.insert(f.key().to_string(), profile.join(f.default_dir()));
        }
        let env = Env { profile, folders, accounts: vec![Account { root: od.clone(), business: false, signed_in: true }], onedrive_running: true, cloud_names: vec![], fake: true };
        let logs = t.path().join("logs");
        Fx { _t: t, env, od, local, logs }
    }

    /// OneDrive 폴더 전체(경로·크기·내용 해시·수정 시각)
    fn snapshot(root: &Path) -> Vec<(PathBuf, u64, String, Option<std::time::SystemTime>)> {
        let mut v: Vec<_> = walkdir::WalkDir::new(root)
            .sort_by_file_name()
            .into_iter()
            .filter_map(|e| e.ok())
            .map(|e| {
                let md = e.metadata().unwrap();
                let h = if md.is_file() { hex(&sha_file(e.path(), None).unwrap().1) } else { "dir".into() };
                (e.path().to_path_buf(), md.len(), h, md.modified().ok())
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    fn run(f: &Fx, opts: RestoreOpts) -> RestoreOut {
        restore(&f.env, Folder::Desktop, opts, &f.logs, &Cancel::new(), &mut |_, _, _| {}).unwrap()
    }

    #[test]
    fn cases_a_b_c_d() {
        let f = fx();
        let r = diagnose(&f.env);
        let d = r.folders.iter().find(|x| x.key == "desktop").unwrap();
        assert_eq!(d.case, Case::B);
        assert_eq!(d.onedrive_count.as_ref().unwrap().files, 3, "desktop.ini는 세지 않음");
        assert!(r.folders.iter().filter(|x| x.key != "desktop").all(|x| x.case == Case::C));
        assert!(!r.folders.iter().any(|x| x.key == "downloads"), "다운로드는 OneDrive와 얽혔을 때만");

        // A: 바탕 화면 위치가 OneDrive 안
        let mut a = f.env.clone();
        a.folders.insert("desktop".into(), f.od.join("바탕 화면"));
        assert_eq!(diagnose_folder(&a, Folder::Desktop).case, Case::A);
        // 계정이 레지스트리에 없어도 프로필 아래 OneDrive… 폴더면 OneDrive 안
        a.accounts.clear();
        assert_eq!(diagnose_folder(&a, Folder::Desktop).case, Case::A);

        // D: 가리키는 OneDrive 폴더가 없음
        let mut dd = f.env.clone();
        dd.folders.insert("documents".into(), f.od.join("문서"));
        assert_eq!(diagnose_folder(&dd, Folder::Documents).case, Case::D);
        // D: OneDrive 폴더를 지웠고 이 PC 바탕 화면은 비어 있음
        std::fs::remove_dir_all(&f.od).unwrap();
        assert_eq!(diagnose_folder(&f.env, Folder::Desktop).case, Case::D);
        // OneDrive가 없는 PC → C, "설정되어 있지 않아요"
        let mut none = f.env.clone();
        none.accounts.clear();
        let r = diagnose(&none);
        assert!(r.folders.iter().all(|x| x.case == Case::C));
        assert!(!r.onedrive_set_up && r.summary.contains("설정되어 있지 않아요"));
    }

    #[test]
    fn restore_copies_and_never_writes_under_onedrive() {
        let f = fx();
        // 이 PC 바탕 화면에 같은 내용 하나, 이름만 같고 내용이 다른 하나
        std::fs::write(f.local.join("같은.txt"), "같은 내용").unwrap();
        std::fs::write(f.local.join("메모.txt"), "이 PC 메모").unwrap();
        let before = snapshot(&f.od);

        let dry = run(&f, RestoreOpts { dry_run: true, ..Default::default() });
        assert_eq!((dry.copied, dry.same, dry.renamed), (1, 1, 1), "{:?}", dry.plan);
        assert!(dry.log.is_none() && !f.logs.exists(), "미리 보기는 아무것도 쓰지 않음");
        assert!(!f.local.join("보고서").exists());

        let o = run(&f, RestoreOpts::default());
        assert_eq!((o.copied, o.same, o.renamed), (1, 1, 1), "{}", o.summary);
        assert_eq!(o.summary, "1개 가져옴, 1개 같아서 건너뜀, 1개 이름 바꿔 둠");
        assert_eq!(std::fs::read_to_string(f.local.join("메모.txt")).unwrap(), "이 PC 메모", "덮어쓰지 않음");
        assert_eq!(std::fs::read_to_string(f.local.join("메모 (OneDrive에서).txt")).unwrap(), "OneDrive 메모");
        assert_eq!(std::fs::read_to_string(f.local.join("보고서").join("1분기.docx")).unwrap(), "보고서 내용");
        assert!(!f.local.join("desktop.ini").exists());
        assert_eq!(snapshot(&f.od), before, "OneDrive 폴더는 그대로");

        // 다시 하면 모두 같아서 건너뜀(이름 바꿔 둔 것도 같은 것으로 알아봄)
        let again = run(&f, RestoreOpts::default());
        assert_eq!((again.copied, again.same, again.renamed), (0, 3, 0), "{}", again.summary);
        assert_eq!(snapshot(&f.od), before);
    }

    #[test]
    fn refuses_to_write_inside_onedrive() {
        let f = fx();
        // 바탕 화면이 OneDrive 안(A) → 가져오기 거절
        let mut a = f.env.clone();
        a.folders.insert("desktop".into(), f.od.join("바탕 화면"));
        let e = restore(&a, Folder::Desktop, RestoreOpts::default(), &f.logs, &Cancel::new(), &mut |_, _, _| {}).unwrap_err();
        assert!(e.to_string().contains("백업을 끄고"), "{e}");
        // 쓰는 곳이 OneDrive 안이면 막는 검사
        assert!(guard_dest(&f.env, &f.od.join("바탕 화면"), &f.od.join("다른 곳")).is_err());
        assert!(guard_dest(&f.env, &f.od.join("바탕 화면"), &f.local).is_ok());
        // 대소문자가 달라도 OneDrive 안
        let upper = PathBuf::from(f.od.to_string_lossy().to_uppercase()).join("x");
        assert!(f.env.onedrive_root_of(&upper).is_some());
    }

    #[test]
    fn cloud_only_files_counted_and_skipped_unless_allowed() {
        let mut f = fx();
        f.env.cloud_names = vec![".docx".into()];
        let d = diagnose_folder(&f.env, Folder::Desktop);
        let c = d.onedrive_count.unwrap();
        assert_eq!((c.cloud_files, c.cloud_bytes), (1, "보고서 내용".len() as u64));
        let o = run(&f, RestoreOpts::default());
        assert_eq!((o.copied, o.cloud_skipped), (2, 1));
        assert!(o.summary.contains("클라우드에만 있는 1개"), "{}", o.summary);
        assert!(!f.local.join("보고서").join("1분기.docx").exists());
        // 받기로 했는데 OneDrive가 꺼져 있음 → 멈추지 않고 바로 알림
        f.env.onedrive_running = false;
        let e = restore(&f.env, Folder::Desktop, RestoreOpts { allow_download: true, dry_run: false }, &f.logs, &Cancel::new(), &mut |_, _, _| {}).unwrap_err();
        assert!(e.to_string().contains("OneDrive가 꺼져 있어요"), "{e}");
        f.env.onedrive_running = true;
        let o = run(&f, RestoreOpts { allow_download: true, dry_run: false });
        assert_eq!((o.copied, o.same, o.cloud_skipped), (1, 2, 0), "{}", o.summary);
    }

    #[test]
    fn undo_removes_only_unchanged_copies() {
        let f = fx();
        std::fs::write(f.local.join("이미 있던.txt"), "원래 파일").unwrap();
        std::fs::write(f.local.join("같은.txt"), "같은 내용").unwrap();
        let before = snapshot(&f.od);
        let o = run(&f, RestoreOpts::default());
        assert_eq!(o.copied, 2);
        // 가져온 뒤 사용자가 고친 파일은 남긴다
        std::fs::write(f.local.join("메모.txt"), "고쳤음").unwrap();
        let u = undo(&f.env, &f.logs, &o.id).unwrap();
        assert_eq!((u.removed, u.kept_changed), (1, 1), "{}", u.summary);
        assert_eq!(u.dirs_removed, 1, "새로 만든 빈 폴더 지움");
        assert!(!f.local.join("보고서").exists());
        assert_eq!(std::fs::read_to_string(f.local.join("메모.txt")).unwrap(), "고쳤음");
        assert_eq!(std::fs::read_to_string(f.local.join("같은.txt")).unwrap(), "같은 내용", "건너뛴(원래 있던) 파일은 그대로");
        assert!(f.local.join("이미 있던.txt").exists());
        assert_eq!(snapshot(&f.od), before, "되돌리기도 OneDrive는 건드리지 않음");
        // 두 번은 안 됨, 경로 문자 거절
        assert!(undo(&f.env, &f.logs, &o.id).is_err());
        assert!(undo(&f.env, &f.logs, "../x").is_err());
    }

    #[test]
    fn undo_keeps_copy_when_source_gone() {
        let f = fx();
        let o = run(&f, RestoreOpts::default());
        std::fs::remove_file(f.od.join("바탕 화면").join("메모.txt")).unwrap();
        let u = undo(&f.env, &f.logs, &o.id).unwrap();
        assert_eq!((u.removed, u.kept_no_source), (2, 1), "{}", u.summary);
        assert!(f.local.join("메모.txt").exists(), "하나뿐인 사본은 지우지 않음");
    }

    #[test]
    fn long_paths_and_name_clash_with_folder_skip_and_continue() {
        let f = fx();
        let odd = f.od.join("바탕 화면");
        let long = "가".repeat(80);
        let deep = odd.join(&long).join(&long).join(&long);
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join(format!("{long}.txt")), "x").unwrap();
        // 이 PC에 '보고서'라는 파일이 있어 폴더를 만들 수 없음
        std::fs::write(f.local.join("보고서"), "파일").unwrap();
        let o = run(&f, RestoreOpts::default());
        assert_eq!(o.skipped.len(), 2, "{:?}", o.skipped);
        assert!(o.skipped.iter().any(|s| s.reason.contains("너무 길어요")));
        assert!(o.skipped.iter().any(|s| s.reason.contains("폴더를 만들 수 없어요")));
        assert_eq!(o.copied, 2, "나머지는 계속");
    }

    #[test]
    fn cancel_stops_and_leaves_no_partial_file() {
        let f = fx();
        let c = Cancel::new();
        c.cancel();
        let o = restore(&f.env, Folder::Desktop, RestoreOpts::default(), &f.logs, &c, &mut |_, _, _| {});
        // 목록을 만들 때 바로 멈춤(취소)
        assert!(matches!(o, Err(Error::Cancelled)));
        assert_eq!(std::fs::read_dir(&f.local).unwrap().count(), 0);
    }

    #[test]
    fn fake_env_only_with_testhooks() {
        // 단위 시험에서는 켜져 있음(cfg(test)) — 배포 앱은 testhooks::enable()을 부르지 않으므로 읽지 않는다
        let f = fx();
        let p = f.logs.with_file_name("fake.json");
        std::fs::write(&p, serde_json::to_string(&f.env).unwrap()).unwrap();
        std::env::set_var(FAKE_ENV, &p);
        let e = Env::current().unwrap();
        std::env::remove_var(FAKE_ENV);
        assert!(e.fake && e.accounts.len() == 1);
    }
}
