//! PDF 페이지 작업(순수 Rust, lopdf). 결과는 항상 새 파일로 쓴다.
//!
//! 페이지 트리를 한 단계로 평평하게 만든 뒤(상속 속성은 각 페이지로 내려 복사)
//! 루트 `/Pages`의 `Kids`를 원하는 페이지 목록으로 바꾸는 방식으로
//! 삭제·나누기·순서 변경을 한 가지 경로로 처리한다.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lopdf::{dictionary, Document, Object, ObjectId};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::pages::{parse_groups, parse_list};
use crate::paths::{output_path, write_new};

const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

fn pdf_err(path: &Path) -> impl Fn(lopdf::Error) -> Error + '_ {
    move |e| Error::Pdf { path: path.to_path_buf(), message: e.to_string() }
}

pub fn load(path: &Path) -> Result<Document> {
    let doc = Document::load(path).map_err(pdf_err(path))?;
    if doc.is_encrypted() {
        return Err(Error::Pdf {
            path: path.to_path_buf(),
            message: "암호가 걸린 PDF는 아직 지원하지 않습니다".into(),
        });
    }
    if doc.get_pages().is_empty() {
        return Err(Error::Pdf { path: path.to_path_buf(), message: "페이지가 없습니다".into() });
    }
    Ok(doc)
}

pub fn page_count(path: &Path) -> Result<u32> {
    Ok(load(path)?.get_pages().len() as u32)
}

fn pages_root(doc: &Document, path: &Path) -> Result<ObjectId> {
    doc.catalog()
        .and_then(|c| c.get(b"Pages"))
        .and_then(Object::as_reference)
        .map_err(pdf_err(path))
}

/// 상속 속성을 페이지로 내려 복사하고, 모든 페이지를 루트 `/Pages` 바로 아래에 둔다.
/// 페이지 id 목록(원래 순서)을 돌려준다.
fn flatten(doc: &mut Document, path: &Path) -> Result<(ObjectId, Vec<ObjectId>)> {
    let root = pages_root(doc, path)?;
    let page_ids: Vec<ObjectId> = doc.get_pages().into_values().collect();
    for &pid in &page_ids {
        let mut inherited: Vec<(Vec<u8>, Object)> = Vec::new();
        {
            let page = doc.get_dictionary(pid).map_err(pdf_err(path))?;
            let mut parent = page.get(b"Parent").and_then(Object::as_reference).ok();
            let mut guard = 0;
            while let Some(pid2) = parent {
                guard += 1;
                if guard > 64 {
                    break;
                }
                let Ok(node) = doc.get_dictionary(pid2) else { break };
                for key in INHERITABLE {
                    if !page.has(key) && !inherited.iter().any(|(k, _)| k == key) {
                        if let Ok(v) = node.get(key) {
                            inherited.push((key.to_vec(), v.clone()));
                        }
                    }
                }
                parent = node.get(b"Parent").and_then(Object::as_reference).ok();
            }
        }
        let page = doc.get_dictionary_mut(pid).map_err(pdf_err(path))?;
        for (k, v) in inherited {
            page.set(k, v);
        }
        page.set("Parent", Object::Reference(root));
    }
    set_kids(doc, root, &page_ids, path)?;
    Ok((root, page_ids))
}

fn set_kids(doc: &mut Document, root: ObjectId, kids: &[ObjectId], path: &Path) -> Result<()> {
    let node = doc.get_dictionary_mut(root).map_err(pdf_err(path))?;
    node.set("Kids", Object::Array(kids.iter().map(|&id| Object::Reference(id)).collect()));
    node.set("Count", Object::Integer(kids.len() as i64));
    node.remove(b"Parent");
    for key in INHERITABLE {
        node.remove(key);
    }
    Ok(())
}

fn save_new(mut doc: Document, output: &Path) -> Result<()> {
    doc.prune_objects();
    doc.compress();
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(crate::error::io_err(output))?;
    write_new(output, &bytes)
}

/// 원본에서 `pages`(1부터, 이 순서대로, 중복 없이)만 남긴 새 문서를 `output`에 쓴다.
fn write_selection(input: &Path, pages: &[u32], output: &Path) -> Result<()> {
    let mut doc = load(input)?;
    let (root, ids) = flatten(&mut doc, input)?;
    let mut seen = std::collections::HashSet::new();
    let mut kids = Vec::with_capacity(pages.len());
    for &p in pages {
        if !seen.insert(p) {
            return Err(Error::PageSpec(format!("{p}쪽이 두 번 지정되었습니다")));
        }
        let id = ids
            .get(p as usize - 1)
            .ok_or_else(|| Error::PageSpec(format!("{p}쪽은 범위를 벗어났습니다")))?;
        kids.push(*id);
    }
    if kids.is_empty() {
        return Err(Error::PageSpec("남는 페이지가 없습니다".into()));
    }
    set_kids(&mut doc, root, &kids, input)?;
    save_new(doc, output)
}

/// 여러 PDF를 주어진 순서대로 합친다. 합친 쪽수를 돌려준다.
pub fn merge(inputs: &[PathBuf], output: &Path) -> Result<u32> {
    if inputs.len() < 2 {
        return Err(Error::Invalid("합칠 PDF를 두 개 이상 고르세요".into()));
    }
    if output.exists() {
        return Err(Error::AlreadyExists(output.to_path_buf()));
    }
    let mut merged = Document::with_version("1.7");
    let mut all_pages: Vec<ObjectId> = Vec::new();
    let mut max_id = 1;
    for input in inputs {
        let mut doc = load(input)?;
        flatten(&mut doc, input)?;
        doc.renumber_objects_with(max_id);
        max_id = doc.max_id + 1;
        let root = pages_root(&doc, input)?;
        let catalog = doc.trailer.get(b"Root").and_then(Object::as_reference).map_err(pdf_err(input))?;
        let pages: Vec<ObjectId> = doc.get_pages().into_values().collect();
        all_pages.extend(&pages);
        for (id, obj) in doc.objects {
            if id == root || id == catalog {
                continue;
            }
            merged.objects.insert(id, obj);
        }
    }
    merged.max_id = max_id;
    let pages_id = merged.new_object_id();
    for &pid in &all_pages {
        if let Ok(page) = merged.get_dictionary_mut(pid) {
            page.set("Parent", Object::Reference(pages_id));
        }
    }
    merged.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => all_pages.iter().map(|&id| Object::Reference(id)).collect::<Vec<_>>(),
            "Count" => all_pages.len() as i64,
        }),
    );
    let catalog_id = merged.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    merged.trailer.set("Root", catalog_id);
    merged.renumber_objects();
    let count = all_pages.len() as u32;
    save_new(merged, output)?;
    Ok(count)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", content = "spec", rename_all = "snake_case")]
pub enum SplitMode {
    /// 한 쪽씩 각각의 파일로
    EveryPage,
    /// `1-3,5` → `1-3`쪽 파일 하나, `5`쪽 파일 하나
    Ranges(String),
}

/// PDF를 나눈다. 만든 파일 경로를 순서대로 돌려준다.
pub fn split(input: &Path, mode: &SplitMode, out_dir: Option<&Path>) -> Result<Vec<PathBuf>> {
    let n = page_count(input)?;
    let groups: Vec<Vec<u32>> = match mode {
        SplitMode::EveryPage => (1..=n).map(|p| vec![p]).collect(),
        SplitMode::Ranges(spec) => parse_groups(spec, n)?,
    };
    let width = n.to_string().len();
    let mut outputs = Vec::new();
    for g in &groups {
        let label = if g.len() == 1 {
            format!("{:0width$}", g[0])
        } else {
            format!("{:0width$}-{:0width$}", g[0], g[g.len() - 1])
        };
        let out = output_path(input, out_dir, &format!("_p{label}"), "pdf");
        write_selection(input, g, &out)?;
        outputs.push(out);
    }
    Ok(outputs)
}

/// 지정한 페이지(`all` 가능)를 시계 방향으로 90/180/270도 돌린다.
pub fn rotate(input: &Path, spec: &str, degrees: i64, output: &Path) -> Result<()> {
    if ![90, 180, 270, -90].contains(&degrees) {
        return Err(Error::Invalid(format!("회전 각도는 90, 180, 270 중 하나여야 합니다 ({degrees})")));
    }
    let mut doc = load(input)?;
    let (_, ids) = flatten(&mut doc, input)?;
    let targets = parse_list(spec, ids.len() as u32)?;
    for p in targets {
        let page = doc.get_dictionary_mut(ids[p as usize - 1]).map_err(pdf_err(input))?;
        let cur = page.get(b"Rotate").and_then(Object::as_i64).unwrap_or(0);
        page.set("Rotate", Object::Integer((cur + degrees).rem_euclid(360)));
    }
    save_new(doc, output)
}

/// 지정한 페이지를 지운 새 파일을 만든다.
pub fn delete_pages(input: &Path, spec: &str, output: &Path) -> Result<u32> {
    let n = page_count(input)?;
    let remove: std::collections::HashSet<u32> = parse_list(spec, n)?.into_iter().collect();
    let keep: Vec<u32> = (1..=n).filter(|p| !remove.contains(p)).collect();
    if keep.is_empty() {
        return Err(Error::PageSpec("모든 페이지를 지울 수는 없습니다".into()));
    }
    write_selection(input, &keep, output)?;
    Ok(keep.len() as u32)
}

/// 새 순서(`3,1,2` 또는 `5-1` 등)로 다시 배열한다. 모든 페이지가 정확히 한 번씩 있어야 한다.
pub fn reorder(input: &Path, order_spec: &str, output: &Path) -> Result<()> {
    let n = page_count(input)?;
    let order = parse_list(order_spec, n)?;
    let mut count: BTreeMap<u32, u32> = BTreeMap::new();
    for &p in &order {
        *count.entry(p).or_default() += 1;
    }
    let missing: Vec<String> = (1..=n).filter(|p| !count.contains_key(p)).map(|p| p.to_string()).collect();
    if !missing.is_empty() {
        return Err(Error::PageSpec(format!(
            "순서에 빠진 페이지가 있습니다: {} (전체 {n}쪽을 모두 적어야 합니다)",
            missing.join(", ")
        )));
    }
    write_selection(input, &order, output)
}
