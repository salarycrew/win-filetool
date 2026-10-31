//! 페이지 지정 문자열 해석: `1-3,5`, `7-`, `all`. 페이지 번호는 1부터.

use crate::error::{Error, Result};

/// 쉼표로 나뉜 각 조각을 하나의 그룹으로 돌려준다. `1-3,5` → `[[1,2,3],[5]]`.
/// `n-`은 끝 페이지까지, `-n`은 처음부터, `all`/`전체`/빈 문자열은 전체 한 그룹.
pub fn parse_groups(spec: &str, page_count: u32) -> Result<Vec<Vec<u32>>> {
    let spec = spec.trim();
    if page_count == 0 {
        return Err(Error::PageSpec("페이지가 없는 PDF입니다".into()));
    }
    if spec.is_empty() || spec.eq_ignore_ascii_case("all") || spec == "전체" {
        return Ok(vec![(1..=page_count).collect()]);
    }
    let mut groups = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let num = |s: &str| -> Result<u32> {
            let v: u32 = s
                .trim()
                .parse()
                .map_err(|_| Error::PageSpec(format!("'{part}'에서 숫자를 읽을 수 없습니다")))?;
            if v == 0 || v > page_count {
                return Err(Error::PageSpec(format!(
                    "{v}쪽은 범위를 벗어났습니다 (1~{page_count}쪽)"
                )));
            }
            Ok(v)
        };
        let group: Vec<u32> = if let Some((a, b)) = part.split_once('-') {
            let start = if a.trim().is_empty() { 1 } else { num(a)? };
            let end = if b.trim().is_empty() { page_count } else { num(b)? };
            if start <= end {
                (start..=end).collect()
            } else {
                // 역순 범위(5-3)는 거꾸로 된 순서로 받아들인다 — 순서 변경에 쓸모 있다.
                (end..=start).rev().collect()
            }
        } else {
            vec![num(part)?]
        };
        groups.push(group);
    }
    if groups.is_empty() {
        return Err(Error::PageSpec("페이지를 하나 이상 지정하세요".into()));
    }
    Ok(groups)
}

/// 모든 그룹을 이어 붙인 페이지 목록. `1-3,5` → `[1,2,3,5]`.
pub fn parse_list(spec: &str, page_count: u32) -> Result<Vec<u32>> {
    Ok(parse_groups(spec, page_count)?.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups() {
        assert_eq!(parse_groups("1-3,5", 6).unwrap(), vec![vec![1, 2, 3], vec![5]]);
        assert_eq!(parse_groups("5-", 6).unwrap(), vec![vec![5, 6]]);
        assert_eq!(parse_groups("-2", 6).unwrap(), vec![vec![1, 2]]);
        assert_eq!(parse_groups("all", 3).unwrap(), vec![vec![1, 2, 3]]);
        assert_eq!(parse_list("3-1", 3).unwrap(), vec![3, 2, 1]);
        assert!(parse_groups("0", 3).is_err());
        assert!(parse_groups("4", 3).is_err());
        assert!(parse_groups("a-b", 3).is_err());
    }
}
