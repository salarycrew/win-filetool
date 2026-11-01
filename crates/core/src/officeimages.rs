//! 이미지만 뽑기: Word·PowerPoint·Excel(docx/pptx/xlsx와 매크로·서식 파일)은 문서 안 그림을 **원본 바이트 그대로**,
//! PDF는 [`crate::pdfrender::extract_images`](JPEG는 다시 인코딩하지 않음)로 꺼낸다. 진입점은 [`extract_images`] 하나.
//!
//! - 순서: 문서에서 처음 쓰인 순서(Word 본문 → 머리글·바닥글, PowerPoint는 슬라이드 순서, Excel은 시트 순서).
//!   어디에도 쓰이지 않은 그림(마스터·레이아웃 등)은 맨 뒤
//! - 같은 바이트의 그림은 한 번만. 이름은 `NN_원래이름`(예: `01_image3.png`)
//! - 결과 폴더: 원본 옆 `이름_이미지\`(있으면 ` (1)`)

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Serialize;

use crate::docx2pptx::{parse_rels, resolve};
use crate::error::{io_err, Error, Result};
use crate::ooxml::pptx::fnv;
use crate::paths;

#[derive(Debug, Clone, Default, Serialize)]
pub struct ImagesOut {
    pub input: PathBuf,
    /// 그림이 하나도 없으면 None(폴더를 만들지 않음)
    pub folder: Option<PathBuf>,
    pub files: Vec<PathBuf>,
    /// 같은 그림이라 건너뛴 수
    pub duplicates: usize,
    /// 형식을 몰라 건너뛴 수(PDF)
    pub skipped: usize,
    pub millis: u64,
    pub note: String,
    /// 건너뛴 항목과 이유(한글: "BIN0003.OLE: OLE 개체(그림이 아님)" 등)
    #[serde(default)]
    pub skipped_items: Vec<String>,
}

pub fn kind(p: &Path) -> Option<&'static str> {
    match p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("docx" | "docm" | "dotx" | "dotm") => Some("word"),
        Some("pptx" | "pptm" | "potx" | "potm" | "ppsx" | "ppsm") => Some("ppt"),
        Some("xlsx" | "xlsm" | "xltx" | "xltm") => Some("xl"),
        Some("pdf") => Some("pdf"),
        // 0.3.0 W6: 한글 5·HWPX(한글 97은 안내 오류)
        Some("hwp" | "hwpx") => Some("hwp"),
        _ => None,
    }
}

/// XML 안 관계 id(r:embed, r:id, r:link, r:pict)를 나온 순서대로
fn rel_ids_in_order(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut r = Reader::from_str(xml);
    loop {
        match r.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                for a in e.attributes().flatten() {
                    let k = a.key.as_ref();
                    if matches!(k, "r:embed" | "r:id" | "r:link" | "r:pict" | "o:relid") {
                        out.push(a.value.to_string());
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

fn natural_key(s: &str) -> (String, u64) {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    (s.chars().filter(|c| !c.is_ascii_digit()).collect(), digits.parse().unwrap_or(0))
}

struct Pkg {
    z: zip::ZipArchive<std::fs::File>,
    names: Vec<String>,
}

impl Pkg {
    fn read(&mut self, name: &str) -> Option<String> {
        let mut f = self.z.by_name(name).ok()?;
        let mut s = String::new();
        f.read_to_string(&mut s).ok()?;
        Some(s)
    }
    fn rels_of(&mut self, part: &str) -> HashMap<String, String> {
        let (dir, file) = part.rsplit_once('/').unwrap_or(("", part));
        let rp = if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") };
        self.read(&rp).map(|s| parse_rels(&s)).unwrap_or_default().into_iter().map(|(k, v)| (k, resolve(dir, &v))).collect()
    }
    /// 부분(part) 하나를 따라가며 쓰인 그림을 순서대로 모은다(차트·드로잉·다이어그램 안쪽도 한 단계 더)
    fn walk(&mut self, part: &str, media: &HashSet<String>, order: &mut Vec<String>, depth: u32) {
        let Some(xml) = self.read(part) else { return };
        let rels = self.rels_of(part);
        for id in rel_ids_in_order(&xml) {
            let Some(t) = rels.get(&id) else { continue };
            if media.contains(t) {
                if !order.contains(t) {
                    order.push(t.clone());
                }
            } else if depth < 2 && t.ends_with(".xml") && self.names.contains(t) {
                self.walk(&t.clone(), media, order, depth + 1);
            }
        }
    }
}

/// zip 안 그림 경로를 문서 순서로
fn office_order(pkg: &mut Pkg, kind: &str) -> (Vec<String>, usize) {
    let media: HashSet<String> = pkg.names.iter().filter(|n| n.starts_with(&format!("{kind}/media/")) && !n.ends_with('/')).cloned().collect();
    let mut order = Vec::new();
    match kind {
        "word" => {
            let main = pkg.names.iter().find(|n| n.starts_with("word/document") && n.ends_with(".xml") && !n.contains("_rels")).cloned().unwrap_or_else(|| "word/document.xml".into());
            pkg.walk(&main, &media, &mut order, 0);
            let mut hf: Vec<String> = pkg.names.iter().filter(|n| (n.starts_with("word/header") || n.starts_with("word/footer")) && n.ends_with(".xml")).cloned().collect();
            hf.sort_by_key(|n| natural_key(n));
            for p in hf {
                pkg.walk(&p, &media, &mut order, 0);
            }
        }
        "ppt" => {
            // presentation.xml의 슬라이드 순서(파일 이름 순서가 아님)
            let pres = "ppt/presentation.xml";
            let rels = pkg.rels_of(pres);
            let ids = pkg.read(pres).map(|x| {
                let mut v = Vec::new();
                let mut r = Reader::from_str(&x);
                loop {
                    match r.read_event() {
                        Ok(Event::Start(e)) | Ok(Event::Empty(e)) if e.name().as_ref() == "p:sldId" => {
                            if let Some(a) = e.attributes().flatten().find(|a| a.key.as_ref() == "r:id") {
                                v.push(a.value.to_string());
                            }
                        }
                        Ok(Event::Eof) | Err(_) => break,
                        _ => {}
                    }
                }
                v
            });
            let slides: Vec<String> = match ids {
                Some(ids) if !ids.is_empty() => ids.iter().filter_map(|i| rels.get(i).cloned()).collect(),
                _ => {
                    let mut s: Vec<String> = pkg.names.iter().filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml")).cloned().collect();
                    s.sort_by_key(|n| natural_key(n));
                    s
                }
            };
            for s in slides {
                pkg.walk(&s, &media, &mut order, 0);
            }
        }
        _ => {
            // 시트 순서 → 시트의 drawing → 그림
            let wb = "xl/workbook.xml";
            let rels = pkg.rels_of(wb);
            let sheet_ids = pkg.read(wb).map(|x| rel_ids_in_order(&x)).unwrap_or_default();
            let mut sheets: Vec<String> = sheet_ids.iter().filter_map(|i| rels.get(i).cloned()).filter(|t| t.contains("worksheets/")).collect();
            if sheets.is_empty() {
                sheets = pkg.names.iter().filter(|n| n.starts_with("xl/worksheets/sheet") && n.ends_with(".xml")).cloned().collect();
                sheets.sort_by_key(|n| natural_key(n));
            }
            for s in sheets {
                pkg.walk(&s, &media, &mut order, 0);
            }
        }
    }
    let used = order.len();
    let mut rest: Vec<String> = media.into_iter().filter(|m| !order.contains(m)).collect();
    rest.sort_by_key(|n| natural_key(n));
    order.extend(rest);
    (order, used)
}

fn extract_office(input: &Path, kind: &str, out_dir: Option<&Path>) -> Result<ImagesOut> {
    let t0 = Instant::now();
    let f = std::fs::File::open(input).map_err(io_err(input))?;
    let z = zip::ZipArchive::new(f).map_err(|e| Error::Invalid(format!("문서를 열지 못했습니다. 파일이 손상되었거나 암호가 걸려 있을 수 있습니다({}): {e}", input.display())))?;
    let names: Vec<String> = z.file_names().filter_map(|n| n.ok().map(|n| n.into_owned())).collect();
    let mut pkg = Pkg { z, names };
    let (order, _used) = office_order(&mut pkg, kind);
    let mut seen: HashMap<(u64, usize), Vec<Vec<u8>>> = HashMap::new();
    let mut files = Vec::new();
    let mut dups = 0;
    let mut folder: Option<PathBuf> = None;
    let width = order.len().to_string().len().max(2);
    for m in &order {
        let mut b = Vec::new();
        if pkg.z.by_name(m).ok().and_then(|mut f| f.read_to_end(&mut b).ok()).is_none() || b.is_empty() {
            continue;
        }
        let e = seen.entry((fnv(&b), b.len())).or_default();
        if e.iter().any(|x| *x == b) {
            dups += 1;
            continue;
        }
        e.push(b.clone());
        let dir = match &folder {
            Some(d) => d.clone(),
            None => {
                let d = crate::pdfrender::output_folder(input, out_dir, "_이미지");
                std::fs::create_dir_all(&d).map_err(io_err(&d))?;
                folder = Some(d.clone());
                d
            }
        };
        let orig = m.rsplit('/').next().unwrap_or(m);
        let path = paths::unique_path(&dir.join(format!("{:0width$}_{orig}", files.len() + 1)));
        paths::write_new(&path, &b)?;
        files.push(path);
    }
    let note = if files.is_empty() {
        "문서에 그림이 없습니다".to_string()
    } else {
        format!("그림 {}개를 원본 그대로 꺼냈습니다{}", files.len(), if dups > 0 { format!(" (같은 그림 {dups}개는 한 번만)") } else { String::new() })
    };
    Ok(ImagesOut { input: input.into(), folder, files, duplicates: dups, skipped: 0, millis: t0.elapsed().as_millis() as u64, note, skipped_items: vec![] })
}

/// 문서·PDF 안 그림을 원본 그대로 `이름_이미지\`에 꺼낸다.
pub fn extract_images(input: &Path, out_dir: Option<&Path>) -> Result<ImagesOut> {
    if !input.is_file() {
        return Err(Error::Invalid(format!("파일이 없습니다: {}", input.display())));
    }
    match kind(input) {
        Some("pdf") => {
            let t0 = Instant::now();
            // 암호 PDF 안내가 "압축할 수 없습니다"(1c 공용 함수 문구)로 나오지 않게
            let r = crate::pdfrender::extract_images_named(input, out_dir, "_이미지").map_err(|e| match e {
                Error::Pdf { path, message } if message.contains("암호") => Error::Pdf { path, message: "암호가 걸린 PDF에서는 그림을 꺼낼 수 없습니다. PDF 프로그램에서 암호를 풀어 저장한 뒤 다시 시도하세요".into() },
                e => e,
            })?;
            Ok(ImagesOut { input: r.input, folder: r.folder, files: r.files, duplicates: 0, skipped: r.skipped, millis: t0.elapsed().as_millis() as u64, note: r.note, skipped_items: vec![] })
        }
        Some("hwp") => crate::hwp::images::extract(input, out_dir),
        Some(k) => extract_office(input, k, out_dir),
        None => Err(Error::Unsupported(format!("그림 꺼내기는 Word·PowerPoint·Excel·PDF·한글 파일만 됩니다: {}", input.display()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_order_and_kinds() {
        let xml = r#"<w:document><w:p><a:blip r:embed="rId7"/></w:p><w:p><v:imagedata r:id="rId3"/><a:blip r:embed="rId7"/></w:p></w:document>"#;
        assert_eq!(rel_ids_in_order(xml), vec!["rId7", "rId3", "rId7"]);
        assert_eq!(kind(Path::new("a.PPTX")), Some("ppt"));
        assert_eq!(kind(Path::new("a.xlsm")), Some("xl"));
        assert_eq!(kind(Path::new("a.hwp")), Some("hwp"));
        assert_eq!(kind(Path::new("a.HWPX")), Some("hwp"));
        assert_eq!(kind(Path::new("a.hwt")), None);
        assert!(natural_key("slide10.xml") > natural_key("slide9.xml"));
    }
}
