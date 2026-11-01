//! 쪽 편집 계획을 한 번에 적용한다(쪽 미리보기 화면의 끌어 놓기 결과).
//!
//! ```json
//! { "sources": [ {"path": "a.pdf"}, {"path": "b.pdf", "password": "1234"} ],
//!   "pages": [ {"kind": "page", "src": 0, "index": 2, "rotate": 90},
//!              {"kind": "blank", "size": "a4"},
//!              {"kind": "page", "src": 1, "index": 0} ] }
//! ```
//! 순서 바꾸기·돌리기·지우기(목록에서 빼기)·뽑기·빈 쪽·다른 PDF 끼워 넣기·합치기가 모두 이 한 경로다.
//! 같은 쪽을 두 번 넣어도 된다(복제).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lopdf::{dictionary, Document, Object, ObjectId};
use serde::{Deserialize, Serialize};

use super::{open, page_geom, rect_obj, save, select_pages, EditOut, Job, A4};
use crate::error::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanSource {
    pub path: PathBuf,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BlankSize {
    /// `a4` | `a4-landscape` | `letter` | `letter-landscape` | `same`(앞쪽과 같은 크기)
    Named(String),
    /// pt
    Custom { w: f32, h: f32 },
}

impl Default for BlankSize {
    fn default() -> Self {
        BlankSize::Named("same".into())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanPage {
    Page {
        src: usize,
        /// 0부터
        index: u32,
        /// 지금 회전에 더할 각도(시계 방향 90의 배수)
        #[serde(default)]
        rotate: i64,
    },
    Blank {
        #[serde(default)]
        size: BlankSize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditPlan {
    pub sources: Vec<PlanSource>,
    pub pages: Vec<PlanPage>,
}

fn blank_size(size: &BlankSize, prev: Option<(f32, f32)>) -> Result<(f32, f32)> {
    Ok(match size {
        BlankSize::Custom { w, h } => {
            if !(36.0..=14400.0).contains(w) || !(36.0..=14400.0).contains(h) {
                return Err(Error::Invalid("빈 쪽 크기는 36~14400pt 사이로 정하세요".into()));
            }
            (*w, *h)
        }
        BlankSize::Named(n) => match n.as_str() {
            "a4" => A4,
            "a4-landscape" => (A4.1, A4.0),
            "letter" => (612.0, 792.0),
            "letter-landscape" => (792.0, 612.0),
            "same" | "" => prev.unwrap_or(A4),
            o => return Err(Error::Invalid(format!("빈 쪽 크기를 알 수 없습니다: {o} (a4, a4-landscape, letter, same)"))),
        },
    })
}

/// 계획대로 새 PDF를 만든다.
pub fn apply(plan: &EditPlan, output: &Path, job: &mut Job) -> Result<EditOut> {
    if plan.pages.is_empty() {
        return Err(Error::Invalid("남는 쪽이 없습니다. 쪽을 하나 이상 남겨 주세요".into()));
    }
    if output.exists() {
        return Err(Error::AlreadyExists(output.to_path_buf()));
    }
    for p in &plan.pages {
        if let PlanPage::Page { src, rotate, .. } = p {
            if *src >= plan.sources.len() {
                return Err(Error::Invalid(format!("없는 파일 번호입니다: {src}")));
            }
            if rotate % 90 != 0 {
                return Err(Error::Invalid(format!("회전은 90도 단위로만 됩니다 ({rotate})")));
            }
        }
    }
    let used: Vec<usize> = {
        let mut v: Vec<usize> = plan.pages.iter().filter_map(|p| if let PlanPage::Page { src, .. } = p { Some(*src) } else { None }).collect();
        v.sort();
        v.dedup();
        v
    };
    let mut merged = Document::with_version("1.7");
    let mut max_id = 1;
    // 원본 번호 → 그 원본의 쪽 id 목록(새 번호 기준)
    let mut src_pages: HashMap<usize, Vec<ObjectId>> = HashMap::new();
    let mut first_catalog: Option<lopdf::Dictionary> = None;
    let mut first_info: Option<Object> = None;
    for &si in &used {
        job.check()?;
        let s = &plan.sources[si];
        let mut doc = open(&s.path, s.password.as_deref())?;
        crate::pdf::flatten(&mut doc, &s.path)?;
        doc.renumber_objects_with(max_id);
        max_id = doc.max_id + 1;
        let root = crate::pdf::pages_root(&doc, &s.path)?;
        let catalog_id = doc.trailer.get(b"Root").and_then(Object::as_reference).map_err(super::pdf_err(&s.path))?;
        if first_catalog.is_none() {
            first_catalog = doc.get_dictionary(catalog_id).ok().cloned();
            first_info = doc.trailer.get(b"Info").ok().cloned();
        }
        src_pages.insert(si, doc.get_pages().into_values().collect());
        for (id, obj) in doc.objects {
            if id == root || id == catalog_id {
                continue;
            }
            merged.objects.insert(id, obj);
        }
    }
    merged.max_id = max_id;
    let pages_id = merged.new_object_id();
    let mut kids: Vec<ObjectId> = Vec::new();
    let mut taken: std::collections::HashSet<ObjectId> = std::collections::HashSet::new();
    let mut pristine: HashMap<ObjectId, lopdf::Dictionary> = HashMap::new();
    let total = plan.pages.len() as u32;
    let mut prev: Option<(f32, f32)> = None;
    for (k, p) in plan.pages.iter().enumerate() {
        job.tick(k as u32, total)?;
        match p {
            PlanPage::Page { src, index, rotate } => {
                let list = &src_pages[src];
                let &orig = list.get(*index as usize).ok_or_else(|| {
                    Error::PageSpec(format!("{}쪽은 범위를 벗어났습니다 ({}: 1~{}쪽)", index + 1, plan.sources[*src].path.display(), list.len()))
                })?;
                let id = if taken.insert(orig) {
                    pristine.insert(orig, merged.get_dictionary(orig).map_err(|e| Error::Invalid(e.to_string()))?.clone());
                    orig
                } else {
                    // 같은 쪽을 또 넣으면 (고치기 전) 쪽 사전만 복제(내용·리소스는 함께 씀)
                    let mut d = pristine[&orig].clone();
                    d.remove(b"Annots");
                    merged.add_object(d)
                };
                let page = merged.get_dictionary_mut(id).map_err(|e| Error::Invalid(e.to_string()))?;
                let cur = page.get(b"Rotate").and_then(Object::as_i64).unwrap_or(0);
                let r = (cur + rotate).rem_euclid(360);
                if r == 0 {
                    page.remove(b"Rotate");
                } else {
                    page.set("Rotate", r);
                }
                page.set("Parent", Object::Reference(pages_id));
                prev = Some(page_geom(&merged, id).visual_size());
                kids.push(id);
            }
            PlanPage::Blank { size } => {
                let (w, h) = blank_size(size, prev)?;
                let content = merged.add_object(lopdf::Stream::new(dictionary! {}, Vec::new()));
                let id = merged.add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => pages_id,
                    "MediaBox" => rect_obj([0.0, 0.0, w, h]),
                    "Resources" => dictionary! {},
                    "Contents" => content,
                });
                prev = Some((w, h));
                kids.push(id);
            }
        }
    }
    merged.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids.iter().map(|&id| Object::Reference(id)).collect::<Vec<_>>(),
            "Count" => kids.len() as i64,
        }),
    );
    let mut catalog = dictionary! { "Type" => "Catalog", "Pages" => pages_id };
    if let Some(c) = &first_catalog {
        // 원본이 하나면 양식·언어·보기 설정을 그대로 둔다(책갈피는 지운 쪽을 가리킬 수 있어 뺀다)
        let keys: &[&[u8]] = if used.len() == 1 { &[b"AcroForm", b"Lang", b"ViewerPreferences", b"PageLayout", b"PageMode"] } else { &[b"Lang"] };
        for key in keys {
            if let Ok(v) = c.get(key) {
                if *key == b"PageMode" && v.as_name().map(|n| n == b"UseOutlines").unwrap_or(false) {
                    continue;
                }
                catalog.set(key.to_vec(), v.clone());
            }
        }
    }
    let catalog_id = merged.add_object(catalog);
    merged.trailer.set("Root", catalog_id);
    if let Some(info) = first_info {
        merged.trailer.set("Info", info);
    }
    let count = kids.len() as u32;
    save(merged, output)?;
    job.tick(total, total)?;
    Ok(EditOut { output: output.to_path_buf(), pages: count, note: format!("{count}쪽짜리 PDF를 만들었습니다") })
}

/// 고른 쪽만 새 PDF로(`1-3,5`, 이 순서대로)
pub fn extract(input: &Path, password: Option<&str>, spec: &str, output: &Path, job: &mut Job) -> Result<EditOut> {
    let n = super::thumbs::page_count(input, password)?;
    let list = crate::pages::parse_list(spec, n)?;
    if list.is_empty() {
        return Err(Error::PageSpec("뽑을 쪽을 고르세요".into()));
    }
    let plan = EditPlan {
        sources: vec![PlanSource { path: input.to_path_buf(), password: password.map(String::from) }],
        pages: list.into_iter().map(|p| PlanPage::Page { src: 0, index: p - 1, rotate: 0 }).collect(),
    };
    let mut out = apply(&plan, output, job)?;
    out.note = format!("{}쪽 가운데 {}쪽을 뽑았습니다", n, out.pages);
    Ok(out)
}

/// `after`쪽 뒤에(0 = 맨 앞) 빈 쪽 `count`장을 넣는다.
pub fn insert_blank(input: &Path, password: Option<&str>, after: u32, count: u32, size: &BlankSize, output: &Path, job: &mut Job) -> Result<EditOut> {
    let n = super::thumbs::page_count(input, password)?;
    if after > n {
        return Err(Error::PageSpec(format!("{after}쪽 뒤에는 넣을 수 없습니다 (0~{n})")));
    }
    if count == 0 || count > 1000 {
        return Err(Error::Invalid("빈 쪽은 1~1000장 사이로 정하세요".into()));
    }
    let mut pages: Vec<PlanPage> = (0..n).map(|i| PlanPage::Page { src: 0, index: i, rotate: 0 }).collect();
    let blanks = (0..count).map(|_| PlanPage::Blank { size: size.clone() });
    pages.splice(after as usize..after as usize, blanks);
    let plan = EditPlan { sources: vec![PlanSource { path: input.to_path_buf(), password: password.map(String::from) }], pages };
    let mut out = apply(&plan, output, job)?;
    out.note = format!("{after}쪽 뒤에 빈 쪽 {count}장을 넣었습니다");
    Ok(out)
}

/// `after`쪽 뒤에(0 = 맨 앞) 다른 PDF의 쪽(`other_pages`, 빈 값 = 전체)을 끼워 넣는다.
#[allow(clippy::too_many_arguments)]
pub fn insert_pdf(
    input: &Path,
    password: Option<&str>,
    after: u32,
    other: &Path,
    other_password: Option<&str>,
    other_pages: Option<&str>,
    output: &Path,
    job: &mut Job,
) -> Result<EditOut> {
    let n = super::thumbs::page_count(input, password)?;
    let m = super::thumbs::page_count(other, other_password)?;
    if after > n {
        return Err(Error::PageSpec(format!("{after}쪽 뒤에는 넣을 수 없습니다 (0~{n})")));
    }
    let sel = select_pages(other_pages, m)?;
    let mut pages: Vec<PlanPage> = (0..n).map(|i| PlanPage::Page { src: 0, index: i, rotate: 0 }).collect();
    let ins: Vec<PlanPage> = sel.iter().map(|&i| PlanPage::Page { src: 1, index: i as u32, rotate: 0 }).collect();
    let k = ins.len();
    pages.splice(after as usize..after as usize, ins);
    let plan = EditPlan {
        sources: vec![
            PlanSource { path: input.to_path_buf(), password: password.map(String::from) },
            PlanSource { path: other.to_path_buf(), password: other_password.map(String::from) },
        ],
        pages,
    };
    let mut out = apply(&plan, output, job)?;
    out.note = format!("{after}쪽 뒤에 {}의 {k}쪽을 끼워 넣었습니다", other.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    Ok(out)
}
