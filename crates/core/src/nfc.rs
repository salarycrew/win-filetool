//! 폴더 안 파일·폴더 이름을 NFD(맥 방식, 자모 분리) → NFC(윈도 방식, 완성형)로 바꾼다.
//!
//! 1. [`plan_folder`]로 미리보기(dry run) 목록을 만든다. 디스크는 건드리지 않는다.
//! 2. [`apply_plan`]으로 실제 이름을 바꾼다. 같은 NFC 이름이 이미 있으면 건너뛰고 보고한다.
//!
//! 하위 항목을 먼저 바꾸고 상위 폴더를 나중에 바꾸므로(깊이 우선, 자식 먼저)
//! 계획에 적힌 경로가 적용 중에도 유효하다.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;
use unicode_normalization::{is_nfc, UnicodeNormalization};
use walkdir::WalkDir;

use crate::error::{io_err, Error, Result};

pub fn to_nfc(s: &str) -> String {
    s.nfc().collect()
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Rename {
    pub from: PathBuf,
    pub to: PathBuf,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Collision {
    /// 바꾸려던 NFD 경로
    pub path: PathBuf,
    /// 이미 있는(또는 다른 항목이 차지할) NFC 경로
    pub existing: PathBuf,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct FolderPlan {
    pub root: PathBuf,
    pub renames: Vec<Rename>,
    pub collisions: Vec<Collision>,
    /// UTF-8로 읽을 수 없는 이름(건너뜀)
    pub unreadable: Vec<PathBuf>,
    pub scanned: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ApplyReport {
    pub renamed: Vec<Rename>,
    pub collisions: Vec<Collision>,
    pub failed: Vec<(PathBuf, String)>,
}

/// `root` 아래(자기 자신 제외)를 훑어 NFC로 바꿀 항목과 충돌 항목을 모은다.
pub fn plan_folder(root: &Path) -> Result<FolderPlan> {
    let meta = std::fs::metadata(root).map_err(io_err(root))?;
    if !meta.is_dir() {
        return Err(Error::Invalid(format!("폴더가 아닙니다: {}", root.display())));
    }
    let mut plan = FolderPlan { root: root.to_path_buf(), ..Default::default() };
    // 폴더별로 "이 이름은 이미 쓰였다" 집합. 기존 이름 + 이번 계획이 차지할 NFC 이름.
    let mut taken: HashMap<PathBuf, HashSet<String>> = HashMap::new();

    for entry in WalkDir::new(root).min_depth(1).contents_first(true).follow_links(false) {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                let path = e.path().map(Path::to_path_buf).unwrap_or_else(|| root.to_path_buf());
                return Err(Error::Invalid(format!("폴더를 읽을 수 없습니다 ({}): {e}", path.display())));
            }
        };
        plan.scanned += 1;
        let Some(name) = entry.file_name().to_str() else {
            plan.unreadable.push(entry.path().to_path_buf());
            continue;
        };
        if is_nfc(name) {
            continue;
        }
        let nfc_name = to_nfc(name);
        let parent = entry.path().parent().unwrap_or(root).to_path_buf();
        let names = match taken.entry(parent.clone()) {
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => v.insert(list_names(&parent)?),
        };
        let target = parent.join(&nfc_name);
        if names.contains(&nfc_name) {
            plan.collisions.push(Collision { path: entry.path().to_path_buf(), existing: target });
            continue;
        }
        names.insert(nfc_name);
        plan.renames.push(Rename {
            from: entry.path().to_path_buf(),
            to: target,
            is_dir: entry.file_type().is_dir(),
        });
    }
    Ok(plan)
}

fn list_names(dir: &Path) -> Result<HashSet<String>> {
    let mut set = HashSet::new();
    for e in std::fs::read_dir(dir).map_err(io_err(dir))? {
        let e = e.map_err(io_err(dir))?;
        if let Some(n) = e.file_name().to_str() {
            set.insert(n.to_string());
        }
    }
    Ok(set)
}

/// 계획대로 이름을 바꾼다. 적용 직전에 다시 확인해, 그사이 생긴 같은 이름은 덮어쓰지 않는다.
pub fn apply_plan(plan: &FolderPlan) -> ApplyReport {
    let mut report = ApplyReport { collisions: plan.collisions.clone(), ..Default::default() };
    for r in &plan.renames {
        let exists_exact = r
            .to
            .parent()
            .and_then(|p| list_names(p).ok())
            .zip(r.to.file_name().and_then(|n| n.to_str()))
            .map(|(names, n)| names.contains(n))
            .unwrap_or(false);
        if exists_exact {
            report.collisions.push(Collision { path: r.from.clone(), existing: r.to.clone() });
            continue;
        }
        match std::fs::rename(&r.from, &r.to) {
            Ok(()) => report.renamed.push(r.clone()),
            Err(e) => report.failed.push((r.from.clone(), e.to_string())),
        }
    }
    report
}

/// 미리보기 없이 바로 적용(CLI용).
pub fn normalize_folder(root: &Path) -> Result<ApplyReport> {
    Ok(apply_plan(&plan_folder(root)?))
}
