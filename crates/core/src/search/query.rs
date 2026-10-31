//! 질의 언어(Everything의 일부). 자세한 표는 README.
//!
//! - 띄어쓰기 = 그리고(AND), `|` = 또는(OR, AND보다 먼저 묶임: `a b|c` = a 그리고 (b 또는 c)), `!낱말` = 아님(NOT)
//! - `"따옴표 구절"`(띄어쓰기 포함, 와일드카드 아님), 와일드카드 `*` `?`(이름 전체에 맞춤)
//! - `\`(또는 `/`)가 든 낱말은 전체 경로에 맞춤
//! - `ext:jpg;png`, `file:`, `folder:`, `size:>10mb`(`<` `>` `>=` `<=`, 범위 `1mb..5mb`, 1024 단위),
//!   `dm:today|yesterday|thisweek|thismonth|2024-05-01`(범위 `2024-01..2024-03`, `>`/`<`도)
//! - 대소문자·NFC/NFD 무시. 초성만으로 된 낱말(`ㅂㄱㅅ`)은 음절 첫소리로 맞춤(`보고서`)

use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, TimeZone};
use memchr::memmem::Finder;

use super::index::{DirPaths, Index, F_DELETED, F_DIR, F_ROOT};
use super::text::{choseong_string, fold, glob_chars, is_choseong_query, longest_literal};

/// 날짜 계산 기준(지금 시각과 지역 시간대). 시험에서는 고정 값을 넣는다.
#[derive(Clone, Copy)]
pub struct QueryCtx {
    pub now: DateTime<FixedOffset>,
}

impl QueryCtx {
    pub fn local_now() -> Self {
        Self { now: chrono::Local::now().fixed_offset() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

#[derive(Debug, Clone)]
pub enum Node {
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
    /// 이름에 들어 있음
    Substr(String),
    /// 이름 전체가 와일드카드에 맞음
    Glob(Vec<char>),
    /// 전체 경로에 들어 있음
    PathSubstr(String),
    PathGlob(Vec<char>),
    /// 초성
    Choseong(String),
    Ext(Vec<String>),
    IsFile,
    IsDir,
    /// 크기 [lo, hi] (포함). 파일만
    Size(u64, u64),
    /// 수정 시각 [lo, hi) 유닉스 초
    Dm(i64, i64),
    True,
}

/// 해석한 질의
pub struct Query {
    pub root: Node,
    pub uses_path: bool,
    compiled: Compiled,
}

impl std::fmt::Debug for Query {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Query({:?})", self.root)
    }
}

enum Compiled {
    And(Vec<Compiled>),
    Or(Vec<Compiled>),
    Not(Box<Compiled>),
    Substr(Finder<'static>),
    Glob { pat: Vec<char>, lit: Option<Finder<'static>> },
    PathSubstr(Finder<'static>),
    PathGlob { pat: Vec<char>, lit: Option<Finder<'static>> },
    Choseong(Finder<'static>),
    Ext(Vec<Vec<u8>>),
    IsFile,
    IsDir,
    Size(u64, u64),
    Dm(i64, i64),
    True,
}

// ---------- 낱말 나누기 ----------

#[derive(Debug, PartialEq)]
enum Tok {
    Word { text: String, quoted: bool, not: bool },
    Or,
}

fn tokenize(q: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut in_q = false;
    let mut not = false;
    let mut started = false;
    let flush = |out: &mut Vec<Tok>, cur: &mut String, quoted: &mut bool, not: &mut bool, started: &mut bool| {
        if *started && (!cur.is_empty() || *quoted) {
            out.push(Tok::Word { text: std::mem::take(cur), quoted: *quoted, not: *not });
        }
        cur.clear();
        *quoted = false;
        *not = false;
        *started = false;
    };
    for c in q.chars() {
        if in_q {
            if c == '"' {
                in_q = false;
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '"' => {
                in_q = true;
                quoted = true;
                started = true;
            }
            '|' => {
                flush(&mut out, &mut cur, &mut quoted, &mut not, &mut started);
                out.push(Tok::Or);
            }
            c if c.is_whitespace() => flush(&mut out, &mut cur, &mut quoted, &mut not, &mut started),
            '!' if !started => {
                not = !not;
                started = true;
            }
            c => {
                cur.push(c);
                started = true;
            }
        }
    }
    flush(&mut out, &mut cur, &mut quoted, &mut not, &mut started);
    out
}

// ---------- 해석 ----------

pub fn parse(q: &str, ctx: &QueryCtx) -> Result<Node, String> {
    let toks = tokenize(q);
    // 띄어쓰기로 나뉜 묶음 = AND, 묶음 안의 `|` = OR
    let mut and_items: Vec<Node> = Vec::new();
    let mut or_items: Vec<Node> = Vec::new();
    let mut pending_or = false;
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Or => {
                pending_or = true;
            }
            Tok::Word { text, quoted, not } => {
                let mut n = word(text, *quoted, ctx)?;
                if *not {
                    n = Node::Not(Box::new(n));
                }
                if pending_or && !or_items.is_empty() {
                    or_items.push(n);
                } else {
                    if !or_items.is_empty() {
                        and_items.push(collapse_or(std::mem::take(&mut or_items)));
                    }
                    or_items.push(n);
                }
                pending_or = false;
            }
        }
        i += 1;
    }
    if !or_items.is_empty() {
        and_items.push(collapse_or(or_items));
    }
    Ok(match and_items.len() {
        0 => Node::True,
        1 => and_items.pop().unwrap(),
        _ => Node::And(and_items),
    })
}

fn collapse_or(mut v: Vec<Node>) -> Node {
    if v.len() == 1 {
        v.pop().unwrap()
    } else {
        Node::Or(v)
    }
}

fn word(text: &str, quoted: bool, ctx: &QueryCtx) -> Result<Node, String> {
    if quoted {
        let k = fold(text);
        return Ok(if k.contains('\\') || k.contains('/') { Node::PathSubstr(k.replace('/', "\\")) } else { Node::Substr(k) });
    }
    let lower = text.to_lowercase();
    if let Some(rest) = lower.strip_prefix("ext:") {
        let exts: Vec<String> = rest.split(';').map(|e| fold(e.trim_start_matches('.'))).filter(|e| !e.is_empty()).collect();
        return Ok(Node::Ext(exts));
    }
    for (pre, n) in [("file:", Node::IsFile), ("folder:", Node::IsDir)] {
        if let Some(rest) = lower.strip_prefix(pre) {
            let rest_orig = &text[pre.len()..];
            return Ok(if rest.is_empty() { n } else { Node::And(vec![n, word(rest_orig, false, ctx)?]) });
        }
    }
    if let Some(rest) = lower.strip_prefix("size:") {
        let (lo, hi) = parse_size_filter(rest)?;
        return Ok(Node::Size(lo, hi));
    }
    if let Some(rest) = lower.strip_prefix("dm:").or_else(|| lower.strip_prefix("datemodified:")) {
        let (lo, hi) = parse_date_filter(rest, ctx)?;
        return Ok(Node::Dm(lo, hi));
    }
    let k = fold(text);
    let wild = k.contains('*') || k.contains('?');
    if k.contains('\\') || k.contains('/') {
        let k = k.replace('/', "\\");
        return Ok(if wild { Node::PathGlob(k.chars().collect()) } else { Node::PathSubstr(k) });
    }
    if wild {
        return Ok(Node::Glob(k.chars().collect()));
    }
    if is_choseong_query(&k) {
        return Ok(Node::Choseong(k));
    }
    Ok(Node::Substr(k))
}

fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: f64 = num.parse().map_err(|_| format!("크기를 알 수 없습니다: {s} (예: size:>10mb)"))?;
    let mul: u64 = match unit.trim() {
        "" | "b" => 1,
        "k" | "kb" => 1 << 10,
        "m" | "mb" => 1 << 20,
        "g" | "gb" => 1 << 30,
        "t" | "tb" => 1 << 40,
        u => return Err(format!("크기 단위를 알 수 없습니다: {u} (b, kb, mb, gb, tb)")),
    };
    Ok((n * mul as f64).round() as u64)
}

fn split_cmp(s: &str) -> (Cmp, &str) {
    for (p, c) in [(">=", Cmp::Ge), ("<=", Cmp::Le), (">", Cmp::Gt), ("<", Cmp::Lt), ("=", Cmp::Eq)] {
        if let Some(r) = s.strip_prefix(p) {
            return (c, r);
        }
    }
    (Cmp::Eq, s)
}

pub fn parse_size_filter(s: &str) -> Result<(u64, u64), String> {
    if let Some((a, b)) = s.split_once("..") {
        let lo = if a.is_empty() { 0 } else { parse_size(a)? };
        let hi = if b.is_empty() { u64::MAX } else { parse_size(b)? };
        return Ok((lo, hi));
    }
    let (c, v) = split_cmp(s);
    let v = parse_size(v)?;
    Ok(match c {
        Cmp::Lt => (0, v.saturating_sub(1)),
        Cmp::Le => (0, v),
        Cmp::Gt => (v.saturating_add(1), u64::MAX),
        Cmp::Ge => (v, u64::MAX),
        Cmp::Eq => (v, v),
    })
}

/// 날짜 값 하나 → [시작, 끝) 유닉스 초(지역 시간)
fn date_range(s: &str, ctx: &QueryCtx) -> Result<(i64, i64), String> {
    let tz = ctx.now.offset();
    let day_start = |d: NaiveDate| tz.from_local_datetime(&d.and_hms_opt(0, 0, 0).unwrap()).single().map(|t| t.timestamp()).unwrap_or(0);
    let today = ctx.now.date_naive();
    let month_start = |y: i32, m: u32| NaiveDate::from_ymd_opt(y, m, 1).unwrap();
    let next_month = |y: i32, m: u32| if m == 12 { month_start(y + 1, 1) } else { month_start(y, m + 1) };
    let (a, b) = match s {
        "today" | "오늘" => (today, today + Duration::days(1)),
        "yesterday" | "어제" => (today - Duration::days(1), today),
        "thisweek" | "이번주" => {
            let mon = today - Duration::days(today.weekday().num_days_from_monday() as i64);
            (mon, mon + Duration::days(7))
        }
        "lastweek" | "지난주" => {
            let mon = today - Duration::days(today.weekday().num_days_from_monday() as i64);
            (mon - Duration::days(7), mon)
        }
        "thismonth" | "이번달" => (month_start(today.year(), today.month()), next_month(today.year(), today.month())),
        "lastmonth" | "지난달" => {
            let (y, m) = if today.month() == 1 { (today.year() - 1, 12) } else { (today.year(), today.month() - 1) };
            (month_start(y, m), month_start(today.year(), today.month()))
        }
        "thisyear" | "올해" => (month_start(today.year(), 1), month_start(today.year() + 1, 1)),
        _ => {
            let parts: Vec<&str> = s.split(['-', '.', '/']).filter(|p| !p.is_empty()).collect();
            let num = |i: usize| parts.get(i).and_then(|p| p.parse::<u32>().ok());
            let bad = || format!("날짜를 알 수 없습니다: {s} (예: dm:today, dm:2024-05-01, dm:2024-01..2024-03)");
            match parts.len() {
                1 => {
                    let y = num(0).ok_or_else(bad)? as i32;
                    (month_start(y, 1), month_start(y + 1, 1))
                }
                2 => {
                    let (y, m) = (num(0).ok_or_else(bad)? as i32, num(1).ok_or_else(bad)?);
                    if !(1..=12).contains(&m) {
                        return Err(bad());
                    }
                    (month_start(y, m), next_month(y, m))
                }
                3 => {
                    let d = NaiveDate::from_ymd_opt(num(0).ok_or_else(bad)? as i32, num(1).ok_or_else(bad)?, num(2).ok_or_else(bad)?).ok_or_else(bad)?;
                    (d, d + Duration::days(1))
                }
                _ => return Err(bad()),
            }
        }
    };
    Ok((day_start(a), day_start(b)))
}

pub fn parse_date_filter(s: &str, ctx: &QueryCtx) -> Result<(i64, i64), String> {
    if let Some((a, b)) = s.split_once("..") {
        let lo = if a.is_empty() { i64::MIN } else { date_range(a, ctx)?.0 };
        let hi = if b.is_empty() { i64::MAX } else { date_range(b, ctx)?.1 };
        return Ok((lo, hi));
    }
    let (c, v) = split_cmp(s);
    let (a, b) = date_range(v, ctx)?;
    Ok(match c {
        Cmp::Eq => (a, b),
        Cmp::Gt => (b, i64::MAX),
        Cmp::Ge => (a, i64::MAX),
        Cmp::Lt => (i64::MIN, a),
        Cmp::Le => (i64::MIN, b),
    })
}

fn compile(n: &Node) -> Compiled {
    let finder = |s: &str| Finder::new(s.as_bytes()).into_owned();
    let lit = |p: &[char]| {
        let l = longest_literal(p);
        (!l.is_empty()).then(|| finder(&l))
    };
    match n {
        Node::And(v) => Compiled::And(order_cheap_first(v.iter().map(compile).collect())),
        Node::Or(v) => Compiled::Or(v.iter().map(compile).collect()),
        Node::Not(x) => Compiled::Not(Box::new(compile(x))),
        Node::Substr(s) => Compiled::Substr(finder(s)),
        Node::Glob(p) => Compiled::Glob { pat: p.clone(), lit: lit(p) },
        Node::PathSubstr(s) => Compiled::PathSubstr(finder(s)),
        Node::PathGlob(p) => Compiled::PathGlob { pat: p.clone(), lit: lit(p) },
        Node::Choseong(s) => Compiled::Choseong(finder(s)),
        Node::Ext(v) => Compiled::Ext(v.iter().map(|e| e.as_bytes().to_vec()).collect()),
        Node::IsFile => Compiled::IsFile,
        Node::IsDir => Compiled::IsDir,
        Node::Size(a, b) => Compiled::Size(*a, *b),
        Node::Dm(a, b) => Compiled::Dm(*a, *b),
        Node::True => Compiled::True,
    }
}

fn cost(c: &Compiled) -> u8 {
    match c {
        Compiled::IsFile | Compiled::IsDir | Compiled::Size(..) | Compiled::Dm(..) | Compiled::True => 0,
        Compiled::Ext(_) => 1,
        Compiled::Substr(_) => 2,
        Compiled::Glob { .. } | Compiled::Choseong(_) => 3,
        Compiled::Not(x) => cost(x),
        Compiled::And(_) | Compiled::Or(_) => 4,
        Compiled::PathSubstr(_) | Compiled::PathGlob { .. } => 5,
    }
}

fn order_cheap_first(mut v: Vec<Compiled>) -> Vec<Compiled> {
    v.sort_by_key(cost);
    v
}

fn uses_path(n: &Node) -> bool {
    match n {
        Node::And(v) | Node::Or(v) => v.iter().any(uses_path),
        Node::Not(x) => uses_path(x),
        Node::PathSubstr(_) | Node::PathGlob(_) => true,
        _ => false,
    }
}

impl Query {
    pub fn parse(q: &str, ctx: &QueryCtx) -> Result<Query, String> {
        let root = parse(q, ctx)?;
        let compiled = compile(&root);
        Ok(Query { uses_path: uses_path(&root), root, compiled })
    }

    /// 빈 질의(아무 조건 없음)
    pub fn is_empty(&self) -> bool {
        matches!(self.root, Node::True)
    }
}

/// 항목마다 다시 쓰는 버퍼
#[derive(Default)]
struct Scratch {
    chars: Vec<char>,
    path: String,
    cho: String,
}

struct Env<'a> {
    ix: &'a Index,
    dirs: Option<&'a DirPaths>,
}

impl Env<'_> {
    fn full_path_key<'s>(&self, id: u32, buf: &'s mut String) -> &'s str {
        buf.clear();
        if let Some(d) = self.dirs {
            buf.push_str(d.get(self.ix.parent(id)));
            buf.push('\\');
        }
        buf.push_str(self.ix.key_str(id));
        buf
    }
}

fn eval(c: &Compiled, env: &Env, id: u32, s: &mut Scratch) -> bool {
    let ix = env.ix;
    match c {
        Compiled::True => true,
        Compiled::And(v) => v.iter().all(|x| eval(x, env, id, s)),
        Compiled::Or(v) => v.iter().any(|x| eval(x, env, id, s)),
        Compiled::Not(x) => !eval(x, env, id, s),
        Compiled::Substr(f) => f.find(ix.key(id)).is_some(),
        Compiled::Glob { pat, lit } => {
            let key = ix.key(id);
            if let Some(l) = lit {
                if l.find(key).is_none() {
                    return false;
                }
            }
            s.chars.clear();
            s.chars.extend(ix.key_str(id).chars());
            glob_chars(pat, &s.chars)
        }
        Compiled::PathSubstr(f) => {
            let mut buf = std::mem::take(&mut s.path);
            let ok = f.find(env.full_path_key(id, &mut buf).as_bytes()).is_some();
            s.path = buf;
            ok
        }
        Compiled::PathGlob { pat, lit } => {
            let mut buf = std::mem::take(&mut s.path);
            let p = env.full_path_key(id, &mut buf);
            let ok = lit.as_ref().map(|l| l.find(p.as_bytes()).is_some()).unwrap_or(true) && {
                s.chars.clear();
                s.chars.extend(p.chars());
                glob_chars(pat, &s.chars)
            };
            s.path = buf;
            ok
        }
        Compiled::Choseong(f) => {
            let key = ix.key(id);
            if key.is_ascii() {
                return false;
            }
            let mut cho = std::mem::take(&mut s.cho);
            choseong_string(ix.key_str(id), &mut cho);
            let ok = f.find(cho.as_bytes()).is_some();
            s.cho = cho;
            ok
        }
        Compiled::Ext(exts) => {
            let key = ix.key(id);
            if ix.is_dir(id) {
                return false;
            }
            match memchr::memrchr(b'.', key) {
                Some(p) => {
                    let e = &key[p + 1..];
                    exts.iter().any(|x| x.as_slice() == e)
                }
                None => false,
            }
        }
        Compiled::IsFile => !ix.is_dir(id),
        Compiled::IsDir => ix.is_dir(id),
        Compiled::Size(lo, hi) => !ix.is_dir(id) && (*lo..=*hi).contains(&ix.size(id)),
        Compiled::Dm(lo, hi) => {
            let t = ix.mtime(id);
            t >= *lo && t < *hi
        }
    }
}

/// 질의에 맞는 항목 번호(번호 순서). 항목이 많으면 여러 스레드로 나눠 훑는다.
pub fn run(ix: &Index, q: &Query) -> Vec<u32> {
    let dirs: Option<Arc<DirPaths>> = q.uses_path.then(|| ix.dir_paths());
    let env = Env { ix, dirs: dirs.as_deref() };
    let n = ix.len();
    let scan = |lo: usize, hi: usize| {
        let mut s = Scratch::default();
        let mut out = Vec::new();
        for id in lo..hi {
            if ix.flags(id as u32) & (F_DELETED | F_ROOT) != 0 {
                continue;
            }
            if eval(&q.compiled, &env, id as u32, &mut s) {
                out.push(id as u32);
            }
        }
        out
    };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).min(8);
    if n < 100_000 || threads < 2 {
        return scan(0, n);
    }
    let chunk = n.div_ceil(threads);
    let parts: Vec<Vec<u32>> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..threads).map(|t| {
            let (lo, hi) = (t * chunk, ((t + 1) * chunk).min(n));
            let scan = &scan;
            sc.spawn(move || scan(lo, hi))
        }).collect();
        hs.into_iter().map(|h| h.join().unwrap_or_default()).collect()
    });
    let mut out = Vec::with_capacity(parts.iter().map(Vec::len).sum());
    for p in parts {
        out.extend(p);
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
    #[default]
    Name,
    Path,
    Size,
    Modified,
}

/// 결과 정렬. 폴더 정렬은 폴더 경로 → 이름 순.
pub fn sort(ix: &Index, ids: &mut [u32], key: SortKey, desc: bool) {
    use std::cmp::Ordering;
    let by_name = |a: &u32, b: &u32| ix.key(*a).cmp(ix.key(*b)).then_with(|| ix.name(*a).cmp(ix.name(*b)));
    match key {
        SortKey::Name => match ix.name_rank() {
            // 순위가 있으면 정수 비교(순위 뒤에 들어온 항목만 이름 비교)
            Some(rank) => {
                let r = |id: u32| rank.get(id as usize).copied().unwrap_or(u32::MAX);
                ids.sort_unstable_by(|a, b| r(*a).cmp(&r(*b)).then_with(|| by_name(a, b)))
            }
            None => ids.sort_unstable_by(by_name),
        },
        SortKey::Size => ids.sort_unstable_by(|a, b| {
            // 폴더(크기 모름)는 파일보다 앞
            (ix.is_dir(*b), ix.size(*a)).cmp(&(ix.is_dir(*a), ix.size(*b))).then_with(|| by_name(a, b))
        }),
        SortKey::Modified => ids.sort_unstable_by(|a, b| ix.mtime(*a).cmp(&ix.mtime(*b)).then_with(|| by_name(a, b))),
        SortKey::Path => {
            let d = ix.dir_paths();
            ids.sort_unstable_by(|a, b| {
                let o: Ordering = d.get(ix.parent(*a)).cmp(d.get(ix.parent(*b)));
                o.then_with(|| by_name(a, b))
            })
        }
    }
    if desc {
        ids.reverse();
    }
}

/// 결과 줄 하나(화면·CLI용)
#[derive(Debug, Clone, serde::Serialize)]
pub struct Row {
    pub id: u32,
    pub name: String,
    pub folder: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: i64,
}

/// 유닉스 초 → 지역 시각 `2026-10-31 14:05`
pub fn fmt_time(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(ts, 0).single() {
        Some(t) if ts > 0 => t.format("%Y-%m-%d %H:%M").to_string(),
        _ => String::new(),
    }
}

pub fn row(ix: &Index, id: u32) -> Row {
    let path = ix.full_path(id);
    let folder = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    Row {
        id,
        name: ix.name(id).to_string(),
        folder,
        path: path.to_string_lossy().into_owned(),
        is_dir: ix.flags(id) & F_DIR != 0,
        size: ix.size(id),
        mtime: ix.mtime(id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> QueryCtx {
        let tz = FixedOffset::east_opt(9 * 3600).unwrap();
        QueryCtx { now: tz.with_ymd_and_hms(2026, 10, 29, 15, 0, 0).unwrap() } // 목요일
    }

    #[test]
    fn tokenizing() {
        assert_eq!(
            tokenize(r#"a "b c" !d e|f"#),
            vec![
                Tok::Word { text: "a".into(), quoted: false, not: false },
                Tok::Word { text: "b c".into(), quoted: true, not: false },
                Tok::Word { text: "d".into(), quoted: false, not: true },
                Tok::Word { text: "e".into(), quoted: false, not: false },
                Tok::Or,
                Tok::Word { text: "f".into(), quoted: false, not: false },
            ]
        );
    }

    #[test]
    fn precedence_or_binds_tighter() {
        let n = parse("a b|c", &ctx()).unwrap();
        let s = format!("{n:?}");
        assert_eq!(s, r#"And([Substr("a"), Or([Substr("b"), Substr("c")])])"#);
    }

    #[test]
    fn dates() {
        let c = ctx();
        let (a, b) = parse_date_filter("today", &c).unwrap();
        assert_eq!(b - a, 86400);
        assert_eq!(a, c.now.offset().with_ymd_and_hms(2026, 10, 29, 0, 0, 0).unwrap().timestamp());
        let (wa, wb) = parse_date_filter("thisweek", &c).unwrap();
        assert_eq!(wa, c.now.offset().with_ymd_and_hms(2026, 10, 26, 0, 0, 0).unwrap().timestamp());
        assert_eq!(wb - wa, 7 * 86400);
        let (ma, mb) = parse_date_filter("thismonth", &c).unwrap();
        assert_eq!(mb - ma, 31 * 86400);
        let (ra, rb) = parse_date_filter("2024-01..2024-02", &c).unwrap();
        assert_eq!(rb - ra, (31 + 29) * 86400);
        assert!(parse_date_filter("2024-13", &c).is_err());
        assert!(parse_date_filter("어제", &c).is_ok());
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size_filter(">10mb").unwrap(), (10 * 1024 * 1024 + 1, u64::MAX));
        assert_eq!(parse_size_filter("1kb..2kb").unwrap(), (1024, 2048));
        assert_eq!(parse_size_filter("<=1.5k").unwrap(), (0, 1536));
        assert_eq!(parse_size_filter("100").unwrap(), (100, 100));
        assert!(parse_size_filter(">10zb").is_err());
    }
}
