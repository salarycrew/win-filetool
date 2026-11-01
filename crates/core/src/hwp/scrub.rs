//! 0.3.3: 한글에 넘기기 전에 **임시 복사본**에서 문서 스크립트(매크로)를 뺀다 — 스크립트가 없으면 돌 수 없다.
//!
//! 한글 자동화에는 Office의 `AutomationSecurity` 같은 "매크로 끄기"가 없고, 앱은 알림 창이 멈추지 않게 자동 응답
//! (`SetMessageBoxMode`)을 쓴다. 그래서 "스크립트를 실행할까요?" 같은 물음에 기대지 않고, 열 파일에서 스크립트 자체를 지운다.
//! - HWP 5(복합 파일): `Scripts` 저장소(`DefaultJScript`·`JScriptVersion`)를 지운다. 문서 본문·그림과는 상관없다.
//! - HWPX(zip): `Scripts/`로 시작하는 항목과, `Contents/content.hpf` 목록의 그 항목(`href="Scripts/…"`)을 뺀다.
//! - 한글 97은 스크립트가 없는 형식이다. 원본은 건드리지 않는다(언제나 작업 임시 폴더의 복사본에만).

use std::io::{Read, Write};
use std::path::Path;

use crate::error::{io_err, Error, Result};

/// 스크립트를 뺐으면 true(없었으면 false)
pub fn strip_scripts(p: &Path) -> Result<bool> {
    let mut head = [0u8; 8];
    let n = std::fs::File::open(p).and_then(|mut f| f.read(&mut head)).map_err(io_err(p))?;
    if n >= 8 && head == [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1] {
        return strip_cfb(p);
    }
    if n >= 4 && &head[..4] == b"PK\x03\x04" {
        return strip_zip(p);
    }
    Ok(false)
}

fn strip_cfb(p: &Path) -> Result<bool> {
    let err = |e: std::io::Error| Error::Invalid(format!("한글 파일의 스크립트를 빼지 못했어요: {e}"));
    let mut c = cfb::open_rw(p).map_err(err)?;
    let found: Vec<String> = c.walk().map(|e| e.path().to_string_lossy().replace('\\', "/")).filter(|s| s.eq_ignore_ascii_case("/Scripts")).collect();
    if found.is_empty() {
        return Ok(false);
    }
    for s in found {
        c.remove_storage_all(&s).map_err(err)?;
    }
    c.flush().map_err(err)?;
    Ok(true)
}

fn is_script_entry(name: &str) -> bool {
    name.len() >= 8 && name[..8].eq_ignore_ascii_case("Scripts/")
}

/// content.hpf에서 `href="Scripts/…"`인 `<opf:item …/>`을 지운다
pub fn drop_script_items(hpf: &str) -> String {
    let re = regex::Regex::new(r#"(?i)<opf:item\b[^>]*\bhref\s*=\s*"Scripts/[^"]*"[^>]*/>"#).expect("정규식");
    re.replace_all(hpf, "").into_owned()
}

fn strip_zip(p: &Path) -> Result<bool> {
    let zerr = |e: zip::result::ZipError| Error::Zip { path: p.to_path_buf(), message: e.to_string() };
    let f = std::fs::File::open(p).map_err(io_err(p))?;
    let mut src = zip::ZipArchive::new(std::io::BufReader::new(f)).map_err(zerr)?;
    let name_at = |z: &zip::ZipArchive<std::io::BufReader<std::fs::File>>, i: usize| z.name_for_index(i).and_then(|r| r.ok()).map(|c| c.into_owned()).unwrap_or_default();
    let has = (0..src.len()).any(|i| is_script_entry(&name_at(&src, i)));
    if !has {
        return Ok(false);
    }
    let tmp = p.with_extension("scrub.tmp");
    {
        let out = std::fs::File::create(&tmp).map_err(io_err(&tmp))?;
        let mut w = zip::ZipWriter::new(out);
        for i in 0..src.len() {
            let name = name_at(&src, i);
            if is_script_entry(&name) {
                continue;
            }
            if name.eq_ignore_ascii_case("Contents/content.hpf") {
                let mut s = String::new();
                src.by_index(i).map_err(zerr)?.read_to_string(&mut s).map_err(io_err(p))?;
                let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
                w.start_file(name, opts).map_err(zerr)?;
                w.write_all(drop_script_items(&s).as_bytes()).map_err(io_err(&tmp))?;
                continue;
            }
            // mimetype(처음·압축 안 함) 등은 그대로 복사
            w.raw_copy_file(src.by_index_raw(i).map_err(zerr)?).map_err(zerr)?;
        }
        w.finish().map_err(zerr)?;
    }
    drop(src);
    std::fs::rename(&tmp, p).map_err(io_err(p))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hwp5_scripts_removed_body_kept() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("in.hwp");
        {
            let mut c = cfb::create(&p).unwrap();
            c.create_stream("/FileHeader").unwrap().write_all(b"HWP Document File").unwrap();
            c.create_storage("/BodyText").unwrap();
            c.create_stream("/BodyText/Section0").unwrap().write_all(b"body").unwrap();
            c.create_storage("/Scripts").unwrap();
            c.create_stream("/Scripts/DefaultJScript").unwrap().write_all(b"function OnDocument_Open(){}").unwrap();
            c.create_stream("/Scripts/JScriptVersion").unwrap().write_all(b"v").unwrap();
            c.flush().unwrap();
        }
        assert!(strip_scripts(&p).unwrap());
        let mut c = cfb::open(&p).unwrap();
        assert!(!c.exists("/Scripts") && !c.exists("/Scripts/DefaultJScript"));
        let mut s = String::new();
        c.open_stream("/BodyText/Section0").unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "body");
        assert!(!strip_scripts(&p).unwrap(), "두 번째는 바꿀 것 없음");
    }

    #[test]
    fn hwpx_scripts_removed_manifest_fixed() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("in.hwpx");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
            let st = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            let df = zip::write::SimpleFileOptions::default();
            w.start_file("mimetype", st).unwrap();
            w.write_all(b"application/hwp+zip").unwrap();
            w.start_file("Contents/section0.xml", df).unwrap();
            w.write_all(b"<sec/>").unwrap();
            w.start_file("Contents/content.hpf", df).unwrap();
            w.write_all(br#"<opf:manifest><opf:item id="section0" href="Contents/section0.xml" media-type="application/xml"/><opf:item id="headersc" href="Scripts/headerScripts" media-type="application/x-javascript ;charset=utf-16"/><opf:item id="sourcesc" href="Scripts/sourceScripts" media-type="application/x-javascript ;charset=utf-16"/></opf:manifest>"#).unwrap();
            w.start_file("Scripts/sourceScripts", df).unwrap();
            w.write_all(b"function OnDocument_Open(){ new ActiveXObject('WScript.Shell'); }").unwrap();
            w.start_file("Scripts/headerScripts", df).unwrap();
            w.write_all(b"var x;").unwrap();
            w.finish().unwrap();
        }
        assert!(strip_scripts(&p).unwrap());
        let mut z = zip::ZipArchive::new(std::fs::File::open(&p).unwrap()).unwrap();
        let names: Vec<String> = (0..z.len()).map(|i| z.by_index(i).unwrap().name().unwrap().to_string()).collect();
        assert_eq!(names, ["mimetype", "Contents/section0.xml", "Contents/content.hpf"]);
        assert_eq!(z.by_index(0).unwrap().compression(), zip::CompressionMethod::Stored, "mimetype는 그대로");
        let mut hpf = String::new();
        z.by_name("Contents/content.hpf").unwrap().read_to_string(&mut hpf).unwrap();
        assert!(hpf.contains("section0") && !hpf.contains("Scripts/"), "{hpf}");
        // 스크립트 없는 hwpx는 손대지 않음
        assert!(!strip_scripts(&p).unwrap());
        let other = t.path().join("a.txt");
        std::fs::write(&other, b"plain").unwrap();
        assert!(!strip_scripts(&other).unwrap());
    }
}
