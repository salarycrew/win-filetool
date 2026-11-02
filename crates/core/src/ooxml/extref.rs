//! 0.3.3(보안 강화): Office에 넘기기 전에 **임시 복사본**에서 바깥을 가리키는 참조를 뺀다(한글 스크립트 빼기와 같은 방식).
//!
//! Office는 문서를 열 때 문서가 가리키는 원격 서식 파일(`attachedTemplate`)을 스스로 받으려 하고, 엑셀은 외부 통합 문서
//! 연결(`externalLink`)을 들고 있다. 매크로는 꺼져 있어도 이 요청 자체가 인터넷·공유 폴더로 나가므로(NTLM 인증 시도 포함),
//! 변환에 필요 없는 이 참조를 연 파일에서 지운다.
//! - Word(`docx`·`docm`·`dotx`·`dotm`): `word/_rels/settings.xml.rels`의 `attachedTemplate` 관계 중 `TargetMode="External"`인 것과
//!   `word/settings.xml`의 그 `<w:attachedTemplate r:id=…/>`
//! - Excel(`xlsx`·`xlsm`·`xltx`·`xltm`): `xl/externalLinks/` 아래 부분 전체(+ 그 관계), `xl/_rels/workbook.xml.rels`의
//!   `externalLink` 관계, `xl/workbook.xml`의 `<externalReference r:id=…/>`, `[Content_Types].xml`의 그 `Override`
//!
//! 셀에 저장된 값은 그대로라 PDF에는 마지막으로 저장된 값이 나온다. 원본은 건드리지 않는다.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::Path;

use crate::error::{io_err, Error, Result};

const SETTINGS_RELS: &str = "word/_rels/settings.xml.rels";
const SETTINGS: &str = "word/settings.xml";
const WB_RELS: &str = "xl/_rels/workbook.xml.rels";
const WB: &str = "xl/workbook.xml";
const CT: &str = "[Content_Types].xml";
const EXT_LINKS: &str = "xl/externalLinks/";
const CALC_CHAIN: &str = "xl/calcChain.xml";

fn rel_re() -> regex::Regex {
    regex::Regex::new(r#"(?s)<Relationship\b[^>]*?(?:/>|>\s*</Relationship>)"#).expect("정규식")
}

fn attr<'a>(el: &'a str, name: &str) -> Option<&'a str> {
    let re = regex::Regex::new(&format!(r#"\b{}\s*=\s*["']([^"']*)["']"#, regex::escape(name))).expect("정규식");
    re.captures(el).and_then(|c| c.get(1)).map(|m| m.as_str())
}

/// 관계 목록 XML에서 `keep`이 거짓인 `<Relationship>`을 지우고 지운 Id를 돌려준다
fn drop_rels(xml: &str, drop: impl Fn(&str) -> bool) -> (String, Vec<String>) {
    let mut ids = Vec::new();
    let out = rel_re()
        .replace_all(xml, |c: &regex::Captures| {
            let el = &c[0];
            if drop(el) {
                ids.push(attr(el, "Id").unwrap_or("").to_string());
                String::new()
            } else {
                el.to_string()
            }
        })
        .into_owned();
    (out, ids)
}

fn type_ends(el: &str, suffix: &str) -> bool {
    attr(el, "Type").is_some_and(|t| t.ends_with(suffix))
}

fn is_external(el: &str) -> bool {
    attr(el, "TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External"))
}

/// `word/_rels/settings.xml.rels`에서 바깥 서식 관계를 지운다(지운 Id)
pub fn drop_remote_template_rels(xml: &str) -> (String, Vec<String>) {
    drop_rels(xml, |el| type_ends(el, "/attachedTemplate") && is_external(el))
}

/// `xl/_rels/workbook.xml.rels`에서 외부 통합 문서 연결 관계를 지운다(지운 Id)
pub fn drop_external_link_rels(xml: &str) -> (String, Vec<String>) {
    drop_rels(xml, |el| type_ends(el, "/externalLink"))
}

/// `prefix:local`(예: `w:attachedTemplate`) 요소 중 `r:id`가 `ids`에 든 것을 지운다
fn drop_elements_with_rid(xml: &str, tag: &str, ids: &[String]) -> String {
    if ids.is_empty() {
        return xml.to_string();
    }
    let re = regex::Regex::new(&format!(r#"(?s)<{}\b[^>]*?(?:/>|>.*?</{}>)"#, regex::escape(tag), regex::escape(tag))).expect("정규식");
    re.replace_all(xml, |c: &regex::Captures| {
        let el = &c[0];
        let rid = attr(el, "r:id").unwrap_or("");
        if ids.iter().any(|i| i == rid) {
            String::new()
        } else {
            el.to_string()
        }
    })
    .into_owned()
}

/// `word/settings.xml`에서 지운 관계를 가리키던 `<w:attachedTemplate>`을 지운다
pub fn drop_attached_template(xml: &str, ids: &[String]) -> String {
    drop_elements_with_rid(xml, "w:attachedTemplate", ids)
}

/// `xl/workbook.xml`에서 `<externalReference>`를 지우고, 비면 `<externalReferences>`도
pub fn drop_external_references(xml: &str, ids: &[String]) -> String {
    let s = drop_elements_with_rid(xml, "externalReference", ids);
    let empty = regex::Regex::new(r#"(?s)<externalReferences\b[^>]*>\s*</externalReferences>|<externalReferences\b[^>]*/>"#).expect("정규식");
    empty.replace_all(&s, "").into_owned()
}

/// `[Content_Types].xml`에서 `/xl/externalLinks/…` Override를 지운다
pub fn drop_external_link_types(xml: &str) -> String {
    let re = regex::Regex::new(r#"(?is)<Override\b[^>]*\bPartName\s*=\s*["']/xl/externalLinks/[^"']*["'][^>]*?(?:/>|>\s*</Override>)"#).expect("정규식");
    re.replace_all(xml, "").into_owned()
}

/// 수식이 외부 통합 문서(`[1]Sheet1!A1`, `'[2]표'!B2`)를 가리키는지
fn is_external_formula(f: &str) -> bool {
    regex::Regex::new(r"\[\d+\]").expect("정규식").is_match(f)
}

/// 시트 XML에서 외부 통합 문서를 가리키는 `<f>`를 지운다(저장된 값 `<v>`는 남김). 그 수식이 공유 수식의 머리였으면
/// 같은 `si`를 쓰는 다른 칸의 `<f t="shared" si=…/>`도 지운다.
pub fn drop_external_formulas(xml: &str) -> String {
    let re = regex::Regex::new(r#"(?s)<f\b([^>]*?)(?:/>|>(.*?)</f>)"#).expect("정규식");
    let mut shared: Vec<String> = Vec::new();
    for c in re.captures_iter(xml) {
        let body = c.get(2).map(|m| m.as_str()).unwrap_or("");
        if is_external_formula(body) {
            if let Some(si) = attr(&c[0], "si") {
                shared.push(si.to_string());
            }
        }
    }
    re.replace_all(xml, |c: &regex::Captures| {
        let body = c.get(2).map(|m| m.as_str()).unwrap_or("");
        let si = attr(&c[0], "si");
        if is_external_formula(body) || si.is_some_and(|x| shared.iter().any(|s| s == x)) {
            String::new()
        } else {
            c[0].to_string()
        }
    })
    .into_owned()
}

/// `xl/workbook.xml`의 이름 정의 중 외부 통합 문서를 가리키는 것을 지운다
pub fn drop_external_defined_names(xml: &str) -> String {
    let re = regex::Regex::new(r#"(?s)<definedName\b[^>]*?(?:/>|>(.*?)</definedName>)"#).expect("정규식");
    let s = re
        .replace_all(xml, |c: &regex::Captures| if c.get(1).is_some_and(|m| is_external_formula(m.as_str())) { String::new() } else { c[0].to_string() })
        .into_owned();
    let empty = regex::Regex::new(r#"(?s)<definedNames\b[^>]*>\s*</definedNames>"#).expect("정규식");
    empty.replace_all(&s, "").into_owned()
}

/// `[Content_Types].xml`에서 `/xl/calcChain.xml` Override를 지운다
pub fn drop_calc_chain_type(xml: &str) -> String {
    let re = regex::Regex::new(r#"(?is)<Override\b[^>]*\bPartName\s*=\s*["']/xl/calcChain\.xml["'][^>]*?(?:/>|>\s*</Override>)"#).expect("정규식");
    re.replace_all(xml, "").into_owned()
}

/// zip 기반 OOXML의 바깥 참조를 임시 복사본에서 지운다. 바꿨으면 true(없었거나 zip이 아니면 false).
pub fn strip_external_refs(p: &Path) -> Result<bool> {
    let mut head = [0u8; 4];
    let n = std::fs::File::open(p).and_then(|mut f| f.read(&mut head)).map_err(io_err(p))?;
    if n < 4 || &head != b"PK\x03\x04" {
        return Ok(false);
    }
    let zerr = |e: zip::result::ZipError| Error::Zip { path: p.to_path_buf(), message: format!("Office에 넘길 준비를 하지 못했어요(파일이 손상되었을 수 있어요): {e}") };
    let f = std::fs::File::open(p).map_err(io_err(p))?;
    let mut src = zip::ZipArchive::new(std::io::BufReader::new(f)).map_err(zerr)?;
    let names: Vec<String> = (0..src.len()).map(|i| src.name_for_index(i).and_then(|r| r.ok()).map(|c| c.into_owned()).unwrap_or_default()).collect();
    let find = |want: &str| names.iter().position(|n| n.eq_ignore_ascii_case(want));
    let mut read = |i: usize| -> Result<String> {
        let mut s = String::new();
        // 이 XML들은 작다. 잘못 만든 파일이 큰 크기를 적어도 16 MiB까지만 읽는다
        src.by_index(i).map_err(zerr)?.take(16 << 20).read_to_string(&mut s).map_err(io_err(p))?;
        Ok(s)
    };
    // 바꿀 내용(이름 → 새 XML)과 뺄 항목
    let mut replace: Vec<(usize, String)> = Vec::new();
    let mut drop: BTreeSet<usize> = BTreeSet::new();

    // Word: 원격 서식
    if let Some(ri) = find(SETTINGS_RELS) {
        let (xml, ids) = drop_remote_template_rels(&read(ri)?);
        if !ids.is_empty() {
            replace.push((ri, xml));
            if let Some(si) = find(SETTINGS) {
                let s = read(si)?;
                replace.push((si, drop_attached_template(&s, &ids)));
            }
        }
    }
    // Excel: 외부 통합 문서 연결
    let ext_parts: Vec<usize> = names.iter().enumerate().filter(|(_, n)| n.len() >= EXT_LINKS.len() && n[..EXT_LINKS.len()].eq_ignore_ascii_case(EXT_LINKS)).map(|(i, _)| i).collect();
    let wb_rels = match find(WB_RELS) {
        Some(ri) => Some((ri, read(ri)?)),
        None => None,
    };
    let has_link_rel = wb_rels.as_ref().is_some_and(|(_, x)| !drop_external_link_rels(x).1.is_empty());
    if !ext_parts.is_empty() || has_link_rel {
        drop.extend(ext_parts);
        // 그 연결을 가리키는 수식은 값만 남기고 지운다(가리킬 곳이 없는 `[1]Sheet1!A1`이 남으면 Excel이 파일을 열지 못함).
        // 계산 순서 목록(calcChain)도 지운다 — 수식이 없어진 칸을 가리키면 Excel이 "복구"하려 하므로(열 때 다시 만듦)
        let mut wb_ids = Vec::new();
        if let Some((ri, x)) = &wb_rels {
            let (xml, ids) = drop_rels(x, |el| type_ends(el, "/externalLink") || type_ends(el, "/calcChain"));
            replace.push((*ri, xml));
            wb_ids = ids;
        }
        if let Some(ci) = find(CALC_CHAIN) {
            drop.insert(ci);
        }
        if let Some(wi) = find(WB) {
            let s = read(wi)?;
            replace.push((wi, drop_external_defined_names(&drop_external_references(&s, &wb_ids))));
        }
        if let Some(ci) = find(CT) {
            let s = read(ci)?;
            replace.push((ci, drop_calc_chain_type(&drop_external_link_types(&s))));
        }
        let sheets: Vec<usize> = names
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                let l = n.to_ascii_lowercase();
                l.starts_with("xl/worksheets/") && !l.contains("/_rels/") && l.ends_with(".xml")
            })
            .map(|(i, _)| i)
            .collect();
        for si in sheets {
            let x = read(si)?;
            let y = drop_external_formulas(&x);
            if y != x {
                replace.push((si, y));
            }
        }
    }
    if replace.is_empty() && drop.is_empty() {
        return Ok(false);
    }
    let tmp = p.with_extension("extref.tmp");
    {
        let out = std::fs::File::create(&tmp).map_err(io_err(&tmp))?;
        let mut w = zip::ZipWriter::new(out);
        for (i, name) in names.iter().enumerate() {
            if drop.contains(&i) {
                continue;
            }
            if let Some((_, xml)) = replace.iter().find(|(j, _)| *j == i) {
                let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
                w.start_file(name.as_str(), opts).map_err(zerr)?;
                w.write_all(xml.as_bytes()).map_err(io_err(&tmp))?;
                continue;
            }
            w.raw_copy_file(src.by_index_raw(i).map_err(zerr)?).map_err(zerr)?;
        }
        w.finish().map_err(zerr)?;
    }
    drop_src(src);
    std::fs::rename(&tmp, p).map_err(io_err(p))?;
    Ok(true)
}

fn drop_src<T>(_: T) {}

/// 시험 자료(직접 만든 것): 원격 서식을 가리키는 docx, 외부 통합 문서 연결이 든 xlsx. 점검용 CLI(`office-extref-make`)와 단위 시험이 쓴다.
/// 원격 주소는 `127.0.0.1:9`(discard 포트, 아무것도 받지 않음)·없는 공유 폴더라 실제로 나가는 곳은 없다.
pub mod samples {
    use std::io::Write;
    use std::path::Path;

    pub const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

    pub fn make_zip(p: &Path, files: &[(&str, &str)]) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(p).expect("시험 자료 만들기"));
        let o = zip::write::SimpleFileOptions::default();
        for (n, c) in files {
            w.start_file(*n, o).expect("zip");
            w.write_all(c.as_bytes()).expect("zip");
        }
        w.finish().expect("zip");
    }

    pub fn docx_with_remote_template(p: &Path) {
        make_zip(
            p,
            &[
                ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/></Types>"#),
                ("_rels/.rels", &format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/officeDocument" Target="word/document.xml"/></Relationships>"#)),
                ("word/document.xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>원격 서식 시험</w:t></w:r></w:p></w:body></w:document>"#),
                ("word/_rels/document.xml.rels", &format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/settings" Target="settings.xml"/></Relationships>"#)),
                ("word/settings.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="{R}"><w:attachedTemplate r:id="rId9"/><w:defaultTabStop w:val="800"/></w:settings>"#)),
                ("word/_rels/settings.xml.rels", &format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId9" Type="{R}/attachedTemplate" Target="http://127.0.0.1:9/template.dotm" TargetMode="External"/></Relationships>"#)),
            ],
        );
    }

    /// 외부 통합 문서(`\\127.0.0.1\nonexistent\book.xlsx`)의 A1을 가리키는 수식(저장된 값 42)이 든 xlsx
    pub fn xlsx_with_external_link(p: &Path) {
        let ns = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
        make_zip(
            p,
            &[
                ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/externalLinks/externalLink1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml"/></Types>"#),
                ("_rels/.rels", &format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/officeDocument" Target="xl/workbook.xml"/></Relationships>"#)),
                ("xl/workbook.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="{ns}" xmlns:r="{R}"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets><externalReferences><externalReference r:id="rId5"/></externalReferences></workbook>"#)),
                ("xl/_rels/workbook.xml.rels", &format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId5" Type="{R}/externalLink" Target="externalLinks/externalLink1.xml"/></Relationships>"#)),
                ("xl/worksheets/sheet1.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="{ns}"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>외부 연결 시험</t></is></c><c r="B1"><f>[1]Sheet1!A1</f><v>42</v></c></row></sheetData></worksheet>"#)),
                ("xl/externalLinks/externalLink1.xml", &format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><externalLink xmlns="{ns}" xmlns:r="{R}"><externalBook r:id="rId1"><sheetNames><sheetName val="Sheet1"/></sheetNames><sheetDataSet><sheetData sheetId="0"><row r="1"><cell r="A1"><v>42</v></cell></row></sheetData></sheetDataSet></externalBook></externalLink>"#)),
                ("xl/externalLinks/_rels/externalLink1.xml.rels", &format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/externalLinkPath" Target="file:///\127.0.0.1
onexistentook.xlsx" TargetMode="External"/></Relationships>"#)),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::samples::{docx_with_remote_template, make_zip, R};

    fn read_zip(p: &Path) -> Vec<(String, String)> {
        let mut z = zip::ZipArchive::new(std::fs::File::open(p).unwrap()).unwrap();
        (0..z.len())
            .map(|i| {
                let mut f = z.by_index(i).unwrap();
                let mut s = String::new();
                f.read_to_string(&mut s).unwrap();
                (f.name().unwrap().to_string(), s)
            })
            .collect()
    }

    #[test]
    fn docx_remote_template_removed() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("in.docx");
        docx_with_remote_template(&p);
        assert!(strip_external_refs(&p).unwrap());
        let files = read_zip(&p);
        let get = |n: &str| files.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone()).unwrap();
        assert!(!get(SETTINGS_RELS).contains("attachedTemplate") && !get(SETTINGS_RELS).contains("127.0.0.1"));
        assert!(!get(SETTINGS).contains("attachedTemplate") && get(SETTINGS).contains("defaultTabStop"));
        assert!(get("word/document.xml").contains("원격 서식 시험"), "본문은 그대로");
        assert_eq!(files.len(), 6);
        assert!(!strip_external_refs(&p).unwrap(), "두 번째는 바꿀 것 없음");
    }

    #[test]
    fn local_template_kept() {
        // 바깥(External)이 아닌 서식 관계는 그대로(문서 안 부분)
        let xml = format!(r#"<Relationships><Relationship Id="rId1" Type="{R}/attachedTemplate" Target="file.dotx"/><Relationship Id="rId2" Type="{R}/attachedTemplate" Target="\\server\share\t.dotm" TargetMode="External"></Relationship></Relationships>"#);
        let (out, ids) = drop_remote_template_rels(&xml);
        assert_eq!(ids, ["rId2"]);
        assert!(out.contains("rId1") && !out.contains("server"));
    }

    #[test]
    fn xlsx_external_links_removed() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("in.xlsx");
        make_zip(
            &p,
            &[
                ("[Content_Types].xml", r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/externalLinks/externalLink1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.externalLink+xml"/></Types>"#),
                ("xl/workbook.xml", &format!(r#"<workbook xmlns:r="{R}"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets><externalReferences><externalReference r:id="rId5"/></externalReferences></workbook>"#)),
                ("xl/_rels/workbook.xml.rels", &format!(r#"<Relationships><Relationship Id="rId1" Type="{R}/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId5" Type="{R}/externalLink" Target="externalLinks/externalLink1.xml"/></Relationships>"#)),
                ("xl/worksheets/sheet1.xml", "<worksheet><sheetData><row r=\"1\"><c r=\"A1\"><f>[1]Sheet1!A1</f><v>42</v></c></row></sheetData></worksheet>"),
                ("xl/externalLinks/externalLink1.xml", "<externalLink/>"),
                ("xl/externalLinks/_rels/externalLink1.xml.rels", &format!(r#"<Relationships><Relationship Id="rId1" Type="{R}/externalLinkPath" Target="file:///\\attacker\share\book.xlsx" TargetMode="External"/></Relationships>"#)),
            ],
        );
        assert!(strip_external_refs(&p).unwrap());
        let files = read_zip(&p);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["[Content_Types].xml", "xl/workbook.xml", "xl/_rels/workbook.xml.rels", "xl/worksheets/sheet1.xml"]);
        let all: String = files.iter().map(|(_, v)| v.as_str()).collect();
        assert!(!all.contains("externalLink") && !all.contains("externalReference") && !all.contains("attacker"), "{all}");
        assert!(all.contains("<v>42</v>") && all.contains("Sheet1"), "셀 값·시트는 그대로");
        assert!(!all.contains("<f>"), "외부를 가리키던 수식은 지움(값만 남김): {all}");
        assert!(!strip_external_refs(&p).unwrap());
    }

    #[test]
    fn external_formulas_and_names() {
        let x = r#"<sheetData><row r="1"><c r="A1"><f>SUM(B1:B2)</f><v>3</v></c><c r="B1"><f t="shared" ref="B1:B3" si="0">[1]Sheet1!A1</f><v>1</v></c></row><row r="2"><c r="B2"><f t="shared" si="0"/><v>2</v></c><c r="C2"><f t="shared" ref="C2:C3" si="1">A1*2</f><v>6</v></c></row><row r="3"><c r="C3"><f t="shared" si="1"/><v>6</v></c><c r="D3"><f>'[2]표 1'!B2+1</f><v>9</v></c></row></sheetData>"#;
        let y = drop_external_formulas(x);
        assert!(y.contains("<f>SUM(B1:B2)</f>") && y.contains(r#"si="1">A1*2</f>"#) && y.contains(r#"<f t="shared" si="1"/>"#), "{y}");
        assert!(!y.contains("[1]") && !y.contains("[2]") && !y.contains(r#"si="0""#), "{y}");
        assert!(y.contains("<v>1</v>") && y.contains("<v>2</v>") && y.contains("<v>9</v>"));
        let w = r#"<workbook><definedNames><definedName name="ext">[1]Sheet1!$A$1</definedName><definedName name="mine">Sheet1!$A$1</definedName></definedNames></workbook>"#;
        let z = drop_external_defined_names(w);
        assert!(z.contains("mine") && !z.contains("ext"), "{z}");
        assert_eq!(drop_external_defined_names(r#"<workbook><definedNames><definedName name="e">[1]S!A1</definedName></definedNames></workbook>"#), "<workbook></workbook>");
    }

    #[test]
    fn non_zip_untouched() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.doc");
        std::fs::write(&p, [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0, 0]).unwrap();
        assert!(!strip_external_refs(&p).unwrap());
        // 바깥 참조 없는 docx도 그대로
        let d = t.path().join("plain.docx");
        make_zip(&d, &[("word/document.xml", "<w:document/>")]);
        let before = std::fs::read(&d).unwrap();
        assert!(!strip_external_refs(&d).unwrap());
        assert_eq!(std::fs::read(&d).unwrap(), before);
    }
}
