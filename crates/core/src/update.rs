//! 0.3.1: 새 버전 알림.
//!
//! 하루에 한 번까지 GitHub의 "최신 릴리스" 정보(판 이름 `tag_name`과 릴리스 쪽 주소 `html_url`만)를 읽어
//! 지금 판보다 새 판이 있으면 화면에 알린다. 보내는 것은 보통 HTTP 요청 하나(User-Agent: `win-filetool/<판>`)뿐이고
//! 쿠키·식별자·사용 통계는 없다. 받기·설치는 하지 않는다(서명 안 된 exe라 사용자가 브라우저에서 직접 받는다).
//!
//! 이 모듈은 네트워크를 직접 쓰지 않는다: HTTP는 [`Fetch`]로 받아서(앱은 WinHTTP, 시험은 가짜) Linux에서 시험할 수 있다.

use serde::{Deserialize, Serialize};

/// 최신 릴리스를 묻는 주소(GitHub REST API, 로그인 불필요)
pub const LATEST_URL: &str = "https://api.github.com/repos/salarycrew/win-filetool/releases/latest";
/// 브라우저로 열어도 되는 주소(이 앞부분으로 시작해야 함)
pub const RELEASES_PREFIX: &str = "https://github.com/salarycrew/win-filetool/releases/";
/// 받는 페이지가 없거나 이상할 때 여는 주소
pub const RELEASES_LATEST_PAGE: &str = "https://github.com/salarycrew/win-filetool/releases/latest";
/// 자동 확인 간격(초): 24시간
pub const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
/// 응답 본문 상한(GitHub 응답은 보통 수십 KB)
pub const MAX_BODY: usize = 2 * 1024 * 1024;

/// 설정 파일에 같이 저장하는 상태(휴대용 모드면 exe 옆 설정 폴더에)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSettings {
    /// "새 버전 나오면 알려 주기"(기본 켬)
    #[serde(default = "yes")]
    pub enabled: bool,
    /// 마지막으로 물어본 시각(유닉스 초). 실패해도 적는다(오프라인에서 켤 때마다 묻지 않게)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check: Option<u64>,
    /// 마지막으로 알아낸 최신 판(예: `0.3.2`)과 릴리스 쪽 주소
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_url: Option<String>,
    /// [이 버전 넘기기]로 넘긴 판
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

fn yes() -> bool {
    true
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self { enabled: true, last_check: None, latest: None, latest_url: None, skipped: None }
    }
}

/// `0.3.1`, `v0.3.1`, `0.3.1-beta.2` → 비교용 판. 숫자 세 개가 아니면 None
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// `-` 뒤(시험판). 없으면 정식판(시험판보다 새것)
    pub pre: Option<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim();
        let s = s.strip_prefix('v').or_else(|| s.strip_prefix('V')).unwrap_or(s);
        // 빌드 정보(+…)는 비교하지 않는다
        let s = s.split('+').next().unwrap_or(s);
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, Some(p.to_string())),
            Some(_) => return None,
            None => (s, None),
        };
        let mut it = core.split('.');
        let num = |p: Option<&str>| -> Option<u64> {
            let p = p?;
            if p.is_empty() || p.len() > 9 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            p.parse().ok()
        };
        let (major, minor, patch) = (num(it.next())?, num(it.next())?, num(it.next())?);
        if it.next().is_some() {
            return None;
        }
        Some(Version { major, minor, patch, pre })
    }

    /// 사람에게 보이는 판(`0.3.2`, 시험판이면 `0.3.2-beta`)
    pub fn display(&self) -> String {
        match &self.pre {
            Some(p) => format!("{}.{}.{}-{p}", self.major, self.minor, self.patch),
            None => format!("{}.{}.{}", self.major, self.minor, self.patch),
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| match (&self.pre, &other.pre) {
            (None, None) => Equal,
            (None, Some(_)) => Greater,
            (Some(_), None) => Less,
            (Some(a), Some(b)) => cmp_pre(a, b),
        })
    }
}

/// semver 시험판 비교: 점으로 나눠 숫자는 숫자로, 글자는 글자로, 숫자 < 글자, 짧은 쪽이 먼저
fn cmp_pre(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.split('.'), b.split('.'));
    loop {
        match (x.next(), y.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(p), Some(q)) => {
                let o = match (p.parse::<u64>(), q.parse::<u64>()) {
                    (Ok(m), Ok(n)) => m.cmp(&n),
                    (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                    (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                    (Err(_), Err(_)) => p.cmp(q),
                };
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

/// `latest`가 `current`보다 새 판인가(어느 하나라도 읽을 수 없으면 아니라고 본다)
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (Version::parse(latest), Version::parse(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// 자동 확인을 할 때인가: 한 번도 안 했거나 24시간이 지났거나, 시계가 뒤로 갔을 때(마지막 시각이 지금보다 뒤)
pub fn due(last_check: Option<u64>, now: u64) -> bool {
    match last_check {
        None => true,
        Some(t) if t > now => true,
        Some(t) => now - t >= CHECK_INTERVAL_SECS,
    }
}

/// 최신 릴리스 응답에서 읽는 것(그 밖의 필드는 버린다)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// 정리한 판(`v` 뗀 것)
    pub version: String,
    /// 브라우저로 열 주소(검사한 것)
    pub url: String,
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: Option<String>,
    html_url: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// GitHub 응답(JSON) → 판·주소. 초안·시험판·판 이름이 이상하면 None.
/// 주소는 이 저장소의 릴리스 쪽만 쓴다(아니면 최신 릴리스 쪽)
pub fn parse_release(body: &str) -> Option<Release> {
    let r: RawRelease = serde_json::from_str(body).ok()?;
    if r.draft || r.prerelease {
        return None;
    }
    let v = Version::parse(r.tag_name.as_deref()?)?;
    let url = r.html_url.filter(|u| safe_release_url(u)).unwrap_or_else(|| RELEASES_LATEST_PAGE.to_string());
    Some(Release { version: v.display(), url })
}

/// 브라우저로 열어도 되는 주소인가: 이 저장소의 릴리스 쪽, 따옴표·공백·제어 문자 없음
pub fn safe_release_url(u: &str) -> bool {
    u.starts_with(RELEASES_PREFIX) && u.len() < 300 && u.bytes().all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\'' && b != b'<' && b != b'>' && b != b'\\')
}

/// HTTP GET 한 번(본문 문자열). 실패는 짧은 이유 글
pub trait Fetch {
    fn get(&self, url: &str) -> Result<String, String>;
}

/// 확인 결과(화면에 보이는 것)
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Outcome {
    /// 새 판이 있음(넘긴 판이면 자동 알림에서는 숨김)
    Newer { version: String, url: String },
    /// 지금 판이 최신
    Latest,
    /// 묻지 못했음(오프라인·시간 초과·요청 제한·이상한 응답)
    Failed,
    /// 자동 확인을 꺼 두었거나 아직 24시간이 안 됨 — 묻지 않음
    NotChecked,
}

/// 확인하기. `force`면 켜짐·24시간과 상관없이 묻는다([지금 확인]). 상태(`st`)를 고친다(저장은 부르는 쪽).
pub fn check(fetch: &dyn Fetch, url: &str, current: &str, st: &mut UpdateSettings, now: u64, force: bool) -> Outcome {
    if !force && (!st.enabled || !due(st.last_check, now)) {
        return Outcome::NotChecked;
    }
    st.last_check = Some(now);
    let rel = match fetch.get(url).ok().and_then(|b| parse_release(&b)) {
        Some(r) => r,
        None => return Outcome::Failed,
    };
    st.latest = Some(rel.version.clone());
    st.latest_url = Some(rel.url.clone());
    if is_newer(&rel.version, current) {
        Outcome::Newer { version: rel.version, url: rel.url }
    } else {
        Outcome::Latest
    }
}

/// 알림 띠에 보일 판: 켜져 있고, 알아 둔 최신 판이 지금보다 새것이고, 넘긴 판이 아닐 때
pub fn banner(st: &UpdateSettings, current: &str) -> Option<(String, String)> {
    if !st.enabled {
        return None;
    }
    let v = st.latest.as_deref()?;
    if !is_newer(v, current) || st.skipped.as_deref() == Some(v) {
        return None;
    }
    let url = st.latest_url.clone().filter(|u| safe_release_url(u)).unwrap_or_else(|| RELEASES_LATEST_PAGE.to_string());
    Some((v.to_string(), url))
}

/// `https://host[:port]/path` 또는 `http://…` → (https인가, 호스트, 포트, 경로). 사용자·쿼리 없는 단순 주소만
pub fn split_url(url: &str) -> Option<(bool, String, u16, String)> {
    let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return None;
    };
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if hostport.is_empty() || hostport.contains('@') {
        return None;
    }
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()?),
        None => (hostport, if secure { 443 } else { 80 }),
    };
    if host.is_empty() {
        return None;
    }
    Some((secure, host.to_string(), port, path.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct Mock {
        reply: RefCell<Result<String, String>>,
        calls: Cell<u32>,
    }
    impl Mock {
        fn new(r: Result<&str, &str>) -> Self {
            Mock { reply: RefCell::new(r.map(String::from).map_err(String::from)), calls: Cell::new(0) }
        }
    }
    impl Fetch for Mock {
        fn get(&self, url: &str) -> Result<String, String> {
            assert_eq!(url, LATEST_URL);
            self.calls.set(self.calls.get() + 1);
            self.reply.borrow().clone()
        }
    }

    fn rel(tag: &str) -> String {
        format!(r#"{{"tag_name":"{tag}","html_url":"https://github.com/salarycrew/win-filetool/releases/tag/{tag}","name":"x","draft":false,"prerelease":false,"assets":[]}}"#)
    }

    #[test]
    fn version_compare() {
        assert!(is_newer("0.3.2", "0.3.1"));
        assert!(is_newer("v0.3.2", "0.3.1"));
        assert!(is_newer("0.4.0", "0.3.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("0.3.10", "0.3.9")); // 글자 비교가 아님
        assert!(!is_newer("0.3.1", "0.3.1"));
        assert!(!is_newer("v0.3.1", "0.3.1"));
        assert!(!is_newer("0.3.0", "0.3.1"));
        assert!(!is_newer("0.2.9", "0.3.1"));
        // 시험판은 같은 숫자의 정식판보다 앞
        assert!(!is_newer("0.3.2-beta.1", "0.3.2"));
        assert!(is_newer("0.3.2", "0.3.2-beta.1"));
        assert!(is_newer("0.3.2-beta.2", "0.3.2-beta.1"));
        assert!(is_newer("0.3.2-beta.10", "0.3.2-beta.2"));
        assert!(is_newer("0.3.2-rc.1", "0.3.2-beta.9"));
        assert!(is_newer("0.3.2-alpha.1", "0.3.2-alpha"));
        // 빌드 정보는 무시
        assert!(!is_newer("0.3.1+abc", "0.3.1"));
        // 읽을 수 없는 판은 새것이 아님
        for bad in ["", "latest", "0.3", "0.3.1.2", "0.3.x", "-1.0.0", "0.3.1-", "01234567890.0.0", " v "] {
            assert!(!is_newer(bad, "0.3.1"), "{bad}");
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
        assert_eq!(Version::parse("v0.3.2").unwrap().display(), "0.3.2");
        assert_eq!(Version::parse(" 0.3.2-beta ").unwrap().display(), "0.3.2-beta");
    }

    #[test]
    fn throttle_24h() {
        let now = 1_800_000_000;
        assert!(due(None, now));
        assert!(!due(Some(now), now));
        assert!(!due(Some(now - CHECK_INTERVAL_SECS + 1), now));
        assert!(due(Some(now - CHECK_INTERVAL_SECS), now));
        // 시계가 뒤로 감 → 다시 물어 봄
        assert!(due(Some(now + 3600), now));

        // 같은 날 두 번 켜도 한 번만 묻는다(실패해도)
        let m = Mock::new(Err("offline"));
        let mut st = UpdateSettings::default();
        assert_eq!(check(&m, LATEST_URL, "0.3.1", &mut st, now, false), Outcome::Failed);
        assert_eq!(st.last_check, Some(now));
        assert_eq!(check(&m, LATEST_URL, "0.3.1", &mut st, now + 60, false), Outcome::NotChecked);
        assert_eq!(check(&m, LATEST_URL, "0.3.1", &mut st, now + CHECK_INTERVAL_SECS - 1, false), Outcome::NotChecked);
        assert_eq!(m.calls.get(), 1);
        // 24시간 뒤에는 다시
        *m.reply.borrow_mut() = Ok(rel("v0.3.2"));
        assert!(matches!(check(&m, LATEST_URL, "0.3.1", &mut st, now + CHECK_INTERVAL_SECS, false), Outcome::Newer { .. }));
        assert_eq!(m.calls.get(), 2);
        // [지금 확인]은 시간과 상관없이
        assert!(matches!(check(&m, LATEST_URL, "0.3.1", &mut st, now + CHECK_INTERVAL_SECS + 5, true), Outcome::Newer { .. }));
        assert_eq!(m.calls.get(), 3);
        // 꺼 두면 자동으로는 묻지 않지만 [지금 확인]은 됨
        st.enabled = false;
        st.last_check = None;
        assert_eq!(check(&m, LATEST_URL, "0.3.1", &mut st, now, false), Outcome::NotChecked);
        assert_eq!(m.calls.get(), 3);
        assert!(matches!(check(&m, LATEST_URL, "0.3.1", &mut st, now, true), Outcome::Newer { .. }));
        assert_eq!(m.calls.get(), 4);
    }

    #[test]
    fn check_results_and_banner() {
        let now = 1_800_000_000;
        let mut st = UpdateSettings::default();
        let m = Mock::new(Ok(&rel("v0.3.2")));
        let o = check(&m, LATEST_URL, "0.3.1", &mut st, now, false);
        assert_eq!(o, Outcome::Newer { version: "0.3.2".into(), url: "https://github.com/salarycrew/win-filetool/releases/tag/v0.3.2".into() });
        assert_eq!(banner(&st, "0.3.1").unwrap().0, "0.3.2");
        // 그 판으로 올린 뒤에는 띠가 없음
        assert_eq!(banner(&st, "0.3.2"), None);
        // [이 버전 넘기기]
        st.skipped = Some("0.3.2".into());
        assert_eq!(banner(&st, "0.3.1"), None);
        // 더 새 판이 나오면 다시 보임
        st.latest = Some("0.3.3".into());
        assert_eq!(banner(&st, "0.3.1").unwrap().0, "0.3.3");
        // 알림을 끄면 숨김
        st.enabled = false;
        assert_eq!(banner(&st, "0.3.1"), None);

        // 최신
        let mut st = UpdateSettings::default();
        assert_eq!(check(&Mock::new(Ok(&rel("v0.3.1"))), LATEST_URL, "0.3.1", &mut st, now, true), Outcome::Latest);
        assert_eq!(check(&Mock::new(Ok(&rel("v0.3.0"))), LATEST_URL, "0.3.1", &mut st, now, true), Outcome::Latest);
        // 실패: 요청 제한(본문이 오류 JSON), 이상한 응답, 시험판, 오프라인
        for body in [r#"{"message":"API rate limit exceeded","documentation_url":"https://docs.github.com"}"#, "<html>", "", r#"{"tag_name":"nightly"}"#,
            r#"{"tag_name":"v9.9.9","prerelease":true}"#, r#"{"tag_name":"v9.9.9","draft":true}"#] {
            let mut st = UpdateSettings::default();
            assert_eq!(check(&Mock::new(Ok(body)), LATEST_URL, "0.3.1", &mut st, now, true), Outcome::Failed, "{body}");
            assert_eq!(st.latest, None);
        }
        let mut st = UpdateSettings::default();
        assert_eq!(check(&Mock::new(Err("timeout")), LATEST_URL, "0.3.1", &mut st, now, true), Outcome::Failed);
        // 실패해도 예전에 알아 둔 새 판 띠는 그대로
        st.latest = Some("0.3.2".into());
        assert_eq!(check(&Mock::new(Err("timeout")), LATEST_URL, "0.3.1", &mut st, now, true), Outcome::Failed);
        assert_eq!(banner(&st, "0.3.1").unwrap().0, "0.3.2");
    }

    #[test]
    fn release_url_is_restricted() {
        // 다른 곳 주소는 쓰지 않고 최신 릴리스 쪽으로
        let r = parse_release(r#"{"tag_name":"v0.3.2","html_url":"https://evil.example/x"}"#).unwrap();
        assert_eq!(r.url, RELEASES_LATEST_PAGE);
        let r = parse_release(r#"{"tag_name":"v0.3.2","html_url":"https://github.com/salarycrew/win-filetool/releases/tag/v0.3.2\" & calc"}"#).unwrap();
        assert_eq!(r.url, RELEASES_LATEST_PAGE);
        let r = parse_release(r#"{"tag_name":"v0.3.2"}"#).unwrap();
        assert_eq!(r.url, RELEASES_LATEST_PAGE);
        assert!(safe_release_url("https://github.com/salarycrew/win-filetool/releases/tag/v0.3.2"));
        assert!(!safe_release_url("https://github.com/salarycrew/win-filetool-evil/releases/tag/v1"));
        assert!(!safe_release_url("http://github.com/salarycrew/win-filetool/releases/tag/v1"));
        // 저장된 주소도 검사
        let st = UpdateSettings { latest: Some("0.3.2".into()), latest_url: Some("file:///C:/x.exe".into()), ..Default::default() };
        assert_eq!(banner(&st, "0.3.1").unwrap().1, RELEASES_LATEST_PAGE);
    }

    #[test]
    fn settings_defaults_and_round_trip() {
        // 예전 설정 파일(update 없음) → 켜짐
        let s: crate::settings::Settings = serde_json::from_str(r#"{ "search": { "roots": [] } }"#).unwrap();
        assert_eq!(s.update, UpdateSettings::default());
        assert!(s.update.enabled);
        let s: crate::settings::Settings = serde_json::from_str(r#"{ "update": { "last_check": 5 } }"#).unwrap();
        assert!(s.update.enabled);
        assert_eq!(s.update.last_check, Some(5));
        let d = tempfile::tempdir().unwrap();
        let mut s = crate::settings::Settings::default();
        s.update = UpdateSettings { enabled: false, last_check: Some(7), latest: Some("0.3.2".into()), latest_url: Some(RELEASES_LATEST_PAGE.into()), skipped: Some("0.3.2".into()) };
        s.save(d.path()).unwrap();
        assert_eq!(crate::settings::Settings::load(d.path()).update, s.update);
    }

    #[test]
    fn url_split() {
        assert_eq!(split_url(LATEST_URL), Some((true, "api.github.com".into(), 443, "/repos/salarycrew/win-filetool/releases/latest".into())));
        assert_eq!(split_url("http://127.0.0.1:8765/latest"), Some((false, "127.0.0.1".into(), 8765, "/latest".into())));
        assert_eq!(split_url("http://localhost"), Some((false, "localhost".into(), 80, "/".into())));
        assert_eq!(split_url("ftp://x/y"), None);
        assert_eq!(split_url("https://u@x/y"), None);
        assert_eq!(split_url("https://:8/y"), None);
        assert_eq!(split_url("https://x:99999/y"), None);
    }
}
