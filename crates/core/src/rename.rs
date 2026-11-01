//! 0.3.0 W4 이름 한꺼번에 바꾸기: 미리보기 → 적용 → 되돌리기.
//!
//! - 규칙: 찾아 바꾸기(글자 또는 정규식) → 대소문자 → 번호(바꾸기 위치) → 날짜·번호(앞/뒤, `_`로 잇기) → 앞뒤 글자 → 확장자.
//!   새 이름 = `앞글자 + [날짜_] + [번호_] + 이름 + [_번호] + [_날짜] + 뒷글자 + .확장자`
//! - 적용은 두 단계(모두 임시 이름으로 → 새 이름으로)라 `a→b, b→a` 같은 맞바꾸기·이어 바꾸기도 된다.
//!   바꾸려는 이름이 이미 있으면(바꿀 목록에 없는 파일) 그 항목은 건너뛰고 원래 이름으로 돌려 둔다.
//! - 되돌리기 기록은 앱 데이터 폴더 `rename-undo/<id>.json`에 둔다.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{io_err, Error, Result};
use crate::nfc::to_nfc;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Pos {
    Prefix,
    Suffix,
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Numbering {
    #[serde(default = "one")]
    pub start: u32,
    #[serde(default = "one")]
    pub step: u32,
    #[serde(default = "one_u8")]
    pub digits: u8,
    #[serde(default = "prefix")]
    pub position: Pos,
}

fn one() -> u32 {
    1
}
fn one_u8() -> u8 {
    1
}
fn prefix() -> Pos {
    Pos::Prefix
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DateSource {
    Taken,
    Modified,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DateRule {
    pub source: DateSource,
    /// "YYYY-MM-DD" | "YYYYMMDD" | "YYYY-MM-DD_HHmmss"
    pub format: String,
    #[serde(default = "prefix")]
    pub position: Pos,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Case {
    Lower,
    Upper,
    #[default]
    Keep,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ExtRule {
    #[default]
    Keep,
    Lower,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Rule {
    #[serde(default)]
    pub find: Option<String>,
    #[serde(default)]
    pub replace: Option<String>,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub suffix: Option<String>,
    #[serde(default)]
    pub numbering: Option<Numbering>,
    #[serde(default)]
    pub date: Option<DateRule>,
    #[serde(default)]
    pub case: Option<Case>,
    #[serde(default)]
    pub ext: Option<ExtRule>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SortBy {
    #[default]
    Name,
    Modified,
    Taken,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pair {
    pub from: PathBuf,
    pub to: PathBuf,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PreviewItem {
    pub from: PathBuf,
    pub to: PathBuf,
    /// 적용할 수 없는 까닭(있으면 적용 때 건너뜀)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Failed {
    pub from: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApplyOut {
    pub done: usize,
    pub failed: Vec<Failed>,
    /// 되돌리기 번호(바꾼 것이 없으면 None)
    pub undo_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UndoOut {
    pub done: usize,
    pub failed: Vec<Failed>,
}

/// 사람 순서로 비교("사진2" < "사진10")
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    bi.next();
                }
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)).then_with(|| na.len().cmp(&nb.len()));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                ai.next();
                bi.next();
            }
        }
    }
}

/// 촬영 날짜(EXIF). JPEG는 앞부분만 읽는다.
pub fn taken_date(path: &Path) -> Option<chrono::NaiveDateTime> {
    use std::io::Read;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let exif = if matches!(ext.as_str(), "jpg" | "jpeg" | "jfif") {
        let mut b = Vec::new();
        std::fs::File::open(path).ok()?.take(256 * 1024).read_to_end(&mut b).ok()?;
        crate::exif::from_jpeg(&b)
    } else if matches!(ext.as_str(), "heic" | "heif" | "hif" | "png" | "webp" | "tif" | "tiff") {
        let b = std::fs::read(path).ok()?;
        if crate::heif::is_heif(&b) {
            crate::heif::extract_exif(&b)
        } else {
            use image::ImageDecoder;
            image::ImageReader::new(std::io::Cursor::new(&b)).with_guessed_format().ok()?.into_decoder().ok()?.exif_metadata().ok().flatten()
        }
    } else {
        None
    }?;
    let d = crate::exif::date_taken(&exif)?;
    chrono::NaiveDateTime::parse_from_str(d.trim(), "%Y:%m:%d %H:%M:%S").ok()
}

fn modified_date(path: &Path) -> Option<chrono::NaiveDateTime> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(chrono::DateTime::<chrono::Local>::from(t).naive_local())
}

fn date_fmt(f: &str) -> Result<&'static str> {
    Ok(match f {
        "YYYY-MM-DD" => "%Y-%m-%d",
        "YYYYMMDD" => "%Y%m%d",
        "YYYY-MM-DD_HHmmss" => "%Y-%m-%d_%H%M%S",
        _ => return Err(Error::Invalid(format!("날짜 형식을 알 수 없습니다: {f}"))),
    })
}

/// 윈도에서 파일 이름으로 쓸 수 없는 까닭
pub fn invalid_name_reason(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        return Some("이름이 비어 있습니다");
    }
    if name.chars().any(|c| "\\/:*?\"<>|".contains(c) || (c as u32) < 0x20) {
        return Some("이름에 쓸 수 없는 글자(\\ / : * ? \" < > |)가 있습니다");
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Some("이름 끝에 점이나 빈칸을 둘 수 없습니다");
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit()) {
        return Some("윈도가 쓰는 이름이라 쓸 수 없습니다");
    }
    if name.chars().count() > 240 {
        return Some("이름이 너무 깁니다");
    }
    None
}

/// 같은 파일 이름인지 비교할 열쇠(윈도는 대소문자를 가리지 않음, NFC)
fn key(p: &Path) -> String {
    to_nfc(&p.to_string_lossy()).to_lowercase()
}

fn split_name(p: &Path) -> (String, Option<String>) {
    let name = to_nfc(&p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    if p.is_dir() {
        return (name, None);
    }
    match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), Some(name[i + 1..].to_string())),
        _ => (name, None),
    }
}

fn join(a: &str, b: &str) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_string(),
        (_, true) => a.to_string(),
        _ => format!("{a}_{b}"),
    }
}

/// 미리보기: 정렬한 순서로 번호를 매기고, 겹침·쓸 수 없는 이름·이미 있는 이름을 표시한다.
pub fn preview(files: &[PathBuf], rule: &Rule, sort: SortBy) -> Result<Vec<PreviewItem>> {
    let re = match (&rule.find, rule.regex) {
        (Some(f), true) if !f.is_empty() => Some(regex::Regex::new(f).map_err(|e| Error::Invalid(format!("찾을 글자(정규식)가 올바르지 않습니다: {e}")))?),
        _ => None,
    };
    let fmt = rule.date.as_ref().map(|d| date_fmt(&d.format)).transpose()?;
    // 날짜는 정렬·이름에 함께 쓴다
    let need_taken = sort == SortBy::Taken || rule.date.as_ref().is_some_and(|d| d.source == DateSource::Taken);
    let mut rows: Vec<(PathBuf, Option<chrono::NaiveDateTime>, Option<chrono::NaiveDateTime>)> = files
        .iter()
        .map(|f| (f.clone(), modified_date(f), if need_taken { taken_date(f) } else { None }))
        .collect();
    let name_of = |p: &Path| to_nfc(&p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    match sort {
        SortBy::Name => rows.sort_by(|a, b| natural_cmp(&name_of(&a.0), &name_of(&b.0))),
        SortBy::Modified => rows.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| natural_cmp(&name_of(&a.0), &name_of(&b.0)))),
        SortBy::Taken => rows.sort_by(|a, b| a.2.or(a.1).cmp(&b.2.or(b.1)).then_with(|| natural_cmp(&name_of(&a.0), &name_of(&b.0)))),
    }
    let mut out = Vec::new();
    for (i, (path, modified, taken)) in rows.iter().enumerate() {
        let (mut stem, ext) = split_name(path);
        let mut note = None;
        if let Some(f) = rule.find.as_deref().filter(|f| !f.is_empty()) {
            let rep = rule.replace.as_deref().unwrap_or("");
            stem = match &re {
                Some(re) => re.replace_all(&stem, rep).into_owned(),
                None => stem.replace(f, rep),
            };
        }
        stem = match rule.case.unwrap_or_default() {
            Case::Lower => stem.to_lowercase(),
            Case::Upper => stem.to_uppercase(),
            Case::Keep => stem,
        };
        let num = rule.numbering.as_ref().map(|n| {
            let v = n.start as u64 + i as u64 * n.step as u64;
            (format!("{v:0width$}", width = n.digits.clamp(1, 9) as usize), n.position)
        });
        if let Some((n, Pos::Replace)) = &num {
            stem = n.clone();
        }
        let date = match (&rule.date, fmt) {
            (Some(d), Some(fmt)) => {
                let v = match d.source {
                    DateSource::Taken => taken.or_else(|| {
                        note = Some("촬영 날짜가 없어 수정한 날짜를 썼습니다".to_string());
                        *modified
                    }),
                    DateSource::Modified => *modified,
                };
                v.map(|v| (v.format(fmt).to_string(), d.position))
            }
            _ => None,
        };
        let mut front = String::new();
        let mut back = String::new();
        if let Some((d, p)) = &date {
            if *p == Pos::Suffix { back = join(&back, d) } else { front = join(&front, d) }
        }
        if let Some((n, p)) = &num {
            match p {
                Pos::Prefix => front = join(&front, n),
                Pos::Suffix => back = join(&back, n),
                Pos::Replace => {}
            }
        }
        let mut new_stem = join(&join(&front, &stem), &back);
        new_stem = format!("{}{}{}", rule.prefix.as_deref().unwrap_or(""), new_stem, rule.suffix.as_deref().unwrap_or(""));
        let ext = ext.map(|e| if rule.ext.unwrap_or_default() == ExtRule::Lower { e.to_lowercase() } else { e });
        let new_name = match ext {
            Some(e) => format!("{new_stem}.{e}"),
            None => new_stem,
        };
        let to = path.parent().unwrap_or_else(|| Path::new("")).join(&new_name);
        out.push(PreviewItem { from: path.clone(), to, conflict: invalid_name_reason(&new_name).map(str::to_string), note });
    }
    mark_conflicts(&mut out);
    Ok(out)
}

fn mark_conflicts(items: &mut [PreviewItem]) {
    let from_keys: HashSet<String> = items.iter().map(|it| key(&it.from)).collect();
    let mut count: HashMap<String, usize> = HashMap::new();
    for it in items.iter() {
        *count.entry(key(&it.to)).or_default() += 1;
    }
    for it in items.iter_mut() {
        if it.conflict.is_some() {
            continue;
        }
        let k = key(&it.to);
        if count[&k] > 1 {
            it.conflict = Some("다른 파일과 새 이름이 겹칩니다".into());
        } else if it.to.exists() && !from_keys.contains(&k) && key(&it.from) != k {
            it.conflict = Some("같은 이름의 파일이 이미 있습니다".into());
        }
    }
}

fn new_id() -> String {
    let mut r = [0u8; 3];
    let _ = getrandom::fill(&mut r);
    format!("{}-{:02x}{:02x}{:02x}", chrono::Local::now().format("%Y%m%d-%H%M%S"), r[0], r[1], r[2])
}

/// 두 단계로 바꾼다. 실제로 바꾼 쌍을 돌려준다.
fn two_phase(pairs: &[Pair], tag: &str) -> (Vec<Pair>, Vec<Failed>) {
    let mut failed = Vec::new();
    let pairs: Vec<&Pair> = pairs.iter().filter(|p| p.from != p.to).collect();
    let from_keys: HashSet<String> = pairs.iter().map(|p| key(&p.from)).collect();
    let mut to_count: HashMap<String, usize> = HashMap::new();
    for p in &pairs {
        *to_count.entry(key(&p.to)).or_default() += 1;
    }
    let mut ok: Vec<&Pair> = Vec::new();
    for p in pairs {
        let name = p.to.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let reason = if !p.from.exists() {
            Some("원래 파일이 없습니다".to_string())
        } else if let Some(r) = invalid_name_reason(&name) {
            Some(r.to_string())
        } else if to_count[&key(&p.to)] > 1 {
            Some("다른 파일과 새 이름이 겹칩니다".into())
        } else if p.to.exists() && !from_keys.contains(&key(&p.to)) && key(&p.from) != key(&p.to) {
            Some("같은 이름의 파일이 이미 있습니다".into())
        } else {
            None
        };
        match reason {
            Some(reason) => failed.push(Failed { from: p.from.clone(), reason }),
            None => ok.push(p),
        }
    }
    // 1단계: 임시 이름으로
    let mut staged: Vec<(&Pair, PathBuf)> = Vec::new();
    for (i, p) in ok.into_iter().enumerate() {
        let tmp = p.from.parent().unwrap_or_else(|| Path::new("")).join(format!(".ft-rename-{tag}-{i}.tmp"));
        match std::fs::rename(&p.from, &tmp) {
            Ok(()) => staged.push((p, tmp)),
            Err(e) => failed.push(Failed { from: p.from.clone(), reason: format!("바꾸지 못했습니다(사용 중이거나 권한 없음): {e}") }),
        }
    }
    // 2단계: 새 이름으로(이미 있으면 덮어쓰지 않고 원래 이름으로 돌려 둠)
    let mut done = Vec::new();
    for (p, tmp) in staged {
        let res = if p.to.exists() { Err("같은 이름의 파일이 이미 있습니다".to_string()) } else { std::fs::rename(&tmp, &p.to).map_err(|e| format!("바꾸지 못했습니다: {e}")) };
        match res {
            Ok(()) => done.push(p.clone()),
            Err(reason) => {
                let _ = std::fs::rename(&tmp, &p.from);
                failed.push(Failed { from: p.from.clone(), reason });
            }
        }
    }
    (done, failed)
}

#[derive(Serialize, Deserialize)]
struct UndoLog {
    id: String,
    created: String,
    pairs: Vec<Pair>,
}

fn undo_path(undo_dir: &Path, id: &str) -> Result<PathBuf> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(Error::Invalid(format!("되돌리기 번호가 올바르지 않습니다: {id}")));
    }
    Ok(undo_dir.join("rename-undo").join(format!("{id}.json")))
}

/// 적용. 되돌리기 기록은 `undo_dir/rename-undo/<id>.json`.
pub fn apply(pairs: &[Pair], undo_dir: &Path) -> Result<ApplyOut> {
    let id = new_id();
    let (done, failed) = two_phase(pairs, &id);
    let undo_id = if done.is_empty() {
        None
    } else {
        let p = undo_path(undo_dir, &id)?;
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(io_err(d))?;
        }
        let log = UndoLog { id: id.clone(), created: chrono::Local::now().to_rfc3339(), pairs: done.clone() };
        std::fs::write(&p, serde_json::to_vec_pretty(&log).unwrap_or_default()).map_err(io_err(&p))?;
        Some(id)
    };
    Ok(ApplyOut { done: done.len(), failed, undo_id })
}

/// 되돌리기: 기록의 새 이름 → 원래 이름. 모두 되돌리면 기록을 지운다(남은 것만 기록에 둔다).
pub fn undo(undo_id: &str, undo_dir: &Path) -> Result<UndoOut> {
    let p = undo_path(undo_dir, undo_id)?;
    let bytes = std::fs::read(&p).map_err(|_| Error::Invalid("되돌릴 기록이 없습니다(이미 되돌렸거나 지워졌습니다)".into()))?;
    let log: UndoLog = serde_json::from_slice(&bytes).map_err(|e| Error::Invalid(format!("되돌리기 기록을 읽지 못했습니다: {e}")))?;
    let back: Vec<Pair> = log.pairs.iter().map(|x| Pair { from: x.to.clone(), to: x.from.clone() }).collect();
    let (done, failed) = two_phase(&back, &format!("u{}", undo_id));
    if failed.is_empty() {
        let _ = std::fs::remove_file(&p);
    } else {
        let failed_keys: HashSet<String> = failed.iter().map(|f| key(&f.from)).collect();
        let left: Vec<Pair> = log.pairs.into_iter().filter(|x| failed_keys.contains(&key(&x.to))).collect();
        let log = UndoLog { id: log.id, created: log.created, pairs: left };
        let _ = std::fs::write(&p, serde_json::to_vec_pretty(&log).unwrap_or_default());
    }
    Ok(UndoOut { done: done.len(), failed })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["사진10.jpg", "사진2.jpg", "사진1.jpg", "a.jpg"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["a.jpg", "사진1.jpg", "사진2.jpg", "사진10.jpg"]);
    }

    #[test]
    fn names() {
        assert!(invalid_name_reason("a:b.txt").is_some());
        assert!(invalid_name_reason("nul.txt").is_some());
        assert!(invalid_name_reason("a.").is_some());
        assert!(invalid_name_reason("보고서 (1).txt").is_none());
        let r: Rule = serde_json::from_str(r#"{"numbering":{"start":1,"step":1,"digits":3,"position":"prefix"},"date":{"source":"taken","format":"YYYY-MM-DD","position":"suffix"},"case":"lower","ext":"lower","regex":false}"#).unwrap();
        assert_eq!(r.numbering.unwrap().digits, 3);
    }
}
