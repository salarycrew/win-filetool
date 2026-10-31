//! 검색용 글자 다루기: 비교 키(NFC + 소문자), 한글 초성, 와일드카드(`*` `?`).

use unicode_normalization::{is_nfc_quick, IsNormalized, UnicodeNormalization};

/// 이름·질의를 비교 키로 바꾼다: NFC로 합치고(맥의 NFD 이름도 같은 키가 됨) 소문자로.
/// 대부분인 ASCII 이름은 빠른 길로 처리한다.
pub fn fold(s: &str) -> String {
    if s.is_ascii() {
        return s.to_ascii_lowercase();
    }
    let nfc: String = match is_nfc_quick(s.chars()) {
        IsNormalized::Yes => s.to_string(),
        _ => s.nfc().collect(),
    };
    nfc.to_lowercase()
}

/// 키를 `out`에 이어 쓴다(할당을 줄이려고).
pub fn fold_into(s: &str, out: &mut String) {
    if s.is_ascii() {
        out.extend(s.bytes().map(|b| b.to_ascii_lowercase() as char));
    } else {
        out.push_str(&fold(s));
    }
}

/// 호환 자모 초성 19개(ㄱ ㄲ ㄴ ㄷ ㄸ ㄹ ㅁ ㅂ ㅃ ㅅ ㅆ ㅇ ㅈ ㅉ ㅊ ㅋ ㅌ ㅍ ㅎ)
const CHOSEONG: [char; 19] = [
    'ㄱ', 'ㄲ', 'ㄴ', 'ㄷ', 'ㄸ', 'ㄹ', 'ㅁ', 'ㅂ', 'ㅃ', 'ㅅ', 'ㅆ', 'ㅇ', 'ㅈ', 'ㅉ', 'ㅊ', 'ㅋ', 'ㅌ', 'ㅍ', 'ㅎ',
];

/// 한글 음절의 초성(호환 자모). 음절이 아니면 None.
pub fn choseong_of(c: char) -> Option<char> {
    let u = c as u32;
    if (0xAC00..=0xD7A3).contains(&u) {
        Some(CHOSEONG[((u - 0xAC00) / 588) as usize])
    } else {
        None
    }
}

/// 질의 낱말이 초성(자음 호환 자모)으로만 이루어졌는지. 예: `ㅂㄱㅅ`
pub fn is_choseong_query(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| CHOSEONG.contains(&c))
}

/// 이름 키를 초성 문자열로: 한글 음절은 초성으로, 나머지 글자는 그대로.
/// `보고서_2024.pdf` → `ㅂㄱㅅ_2024.pdf`
pub fn choseong_string(key: &str, out: &mut String) {
    out.clear();
    for c in key.chars() {
        out.push(choseong_of(c).unwrap_or(c));
    }
}

/// `*`(0자 이상), `?`(1자) 와일드카드. 이름 전체에 맞춘다(Everything과 같음).
/// 문자 단위(UTF-8 바이트가 아님)로 비교한다.
pub fn glob_match(pattern: &[char], text: &str) -> bool {
    let t: Vec<char> = text.chars().collect();
    glob_chars(pattern, &t)
}

pub fn glob_chars(p: &[char], t: &[char]) -> bool {
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// 와일드카드 패턴에서 `*`/`?`가 아닌 가장 긴 조각(미리 거르기용)
pub fn longest_literal(p: &[char]) -> String {
    p.split(|c| *c == '*' || *c == '?')
        .max_by_key(|s| s.len())
        .map(|s| s.iter().collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_nfd_nfc_same() {
        let nfd: String = "보고서.PDF".nfd().collect();
        assert_ne!(nfd, "보고서.PDF");
        assert_eq!(fold(&nfd), "보고서.pdf");
        assert_eq!(fold("보고서.PDF"), "보고서.pdf");
    }

    #[test]
    fn choseong() {
        let mut s = String::new();
        choseong_string("주간보고서_2024.pdf", &mut s);
        assert_eq!(s, "ㅈㄱㅂㄱㅅ_2024.pdf");
        assert!(is_choseong_query("ㅂㄱㅅ"));
        assert!(!is_choseong_query("ㅂㄱa"));
        assert!(!is_choseong_query("보"));
        assert!(!is_choseong_query("ㅏ")); // 모음은 아님
    }

    #[test]
    fn globbing() {
        let p = |s: &str| s.chars().collect::<Vec<_>>();
        assert!(glob_match(&p("*.jpg"), "a.jpg"));
        assert!(!glob_match(&p("*.jpg"), "a.jpg.txt"));
        assert!(glob_match(&p("보고?.hwp"), "보고서.hwp"));
        assert!(!glob_match(&p("보고?.hwp"), "보고.hwp"));
        assert!(glob_match(&p("a*b*c"), "aXXbYYc"));
        assert!(glob_match(&p("*"), ""));
        assert_eq!(longest_literal(&p("*report*.d?c")), "report");
    }
}
