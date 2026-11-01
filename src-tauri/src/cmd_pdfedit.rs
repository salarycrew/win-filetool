//! 0.3.0 PDF 편집 Tauri 명령. 계약(이름·인자·반환·이벤트)은 docs/design.md `## 0.3.0 PDF 편집`.
//!
//! - 오류는 `{ code, message }` JSON: code = `password_required` | `wrong_password` | `cancelled` | `error`
//!   (화면은 password_* 이면 암호 칸을 띄우고 같은 명령을 `password`를 넣어 다시 부른다)
//! - 여러 쪽 작업은 `job`(화면이 정한 번호)으로 "job-progress" {job, fraction} 이벤트를 보내고, `cancel_jobs`로 멈춘다.
//! - 결과는 원본 옆(또는 `outDir`)의 새 파일(`이름_접미사.pdf`, 있으면 ` (1)`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use filetool_core::pdfedit::crypt::{self, ProtectOpts, Protection};
use filetool_core::pdfedit::forms::{self, FieldInfo};
use filetool_core::pdfedit::info::{self, DocInfo, InfoRead};
use filetool_core::pdfedit::layout::{self, CropOpts, FitOpts, NupOpts};
use filetool_core::pdfedit::plan::{self, BlankSize, EditPlan};
use filetool_core::pdfedit::stamp::{self, PageNumberOpts, SignatureOpts, TextBox, WatermarkOpts};
use filetool_core::pdfedit::thumbs::{self, DocPages};
use filetool_core::pdfedit::{default_output, EditOut, Job};
use filetool_core::Error;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::{AppState, Progress};

#[derive(Debug, Serialize)]
pub struct CmdError {
    pub code: &'static str,
    pub message: String,
}

impl From<Error> for CmdError {
    fn from(e: Error) -> Self {
        let code = match &e {
            Error::PasswordRequired(_) => "password_required",
            Error::WrongPassword(_) => "wrong_password",
            Error::Cancelled => "cancelled",
            _ => "error",
        };
        CmdError { code, message: e.to_string() }
    }
}

type R<T> = Result<T, CmdError>;

fn pw(p: &Option<String>) -> Option<&str> {
    p.as_deref().filter(|s| !s.is_empty())
}

fn dir(d: &Option<String>) -> Option<PathBuf> {
    d.as_ref().filter(|s| !s.is_empty()).map(PathBuf::from)
}

async fn work<T, F>(f: F) -> R<T>
where
    T: Send + 'static,
    F: FnOnce() -> filetool_core::Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| CmdError { code: "error", message: format!("작업 스레드 오류: {e}") })?
        .map_err(CmdError::from)
}

/// 진행률·취소가 있는 작업
async fn job_work<T, F>(app: AppHandle, state: &State<'_, AppState>, job: u64, f: F) -> R<T>
where
    T: Send + 'static,
    F: FnOnce(&mut Job) -> filetool_core::Result<T> + Send + 'static,
{
    let cancel = state.begin_job(job);
    let res = work(move || {
        let mut cb = |i: u32, n: u32| {
            let _ = app.emit("job-progress", Progress { job, fraction: i as f64 / n.max(1) as f64 });
        };
        let mut j = Job::new(Some(&cancel), &mut cb);
        f(&mut j)
    })
    .await;
    state.end_job(job);
    res
}

// ---------- 쪽 미리보기 ----------

#[tauri::command]
pub async fn pdf_pages(path: String, password: Option<String>) -> R<DocPages> {
    work(move || thumbs::page_infos(Path::new(&path), pw(&password))).await
}

#[derive(Serialize)]
pub struct ThumbOut {
    page: u32,
    width: u32,
    height: u32,
    data_url: String,
    ms: f64,
}

/// 쪽 썸네일들(가로 `width` px, PNG data URL). 화면은 보이는 쪽만 조금씩 부른다.
#[tauri::command]
pub async fn pdf_thumbs(path: String, password: Option<String>, pages: Vec<u32>, width: u32) -> R<Vec<ThumbOut>> {
    work(move || {
        let v = thumbs::thumbnails(Path::new(&path), pw(&password), &pages, width, &mut |_| Ok(()))?;
        Ok(v.into_iter().map(|t| ThumbOut { page: t.page, width: t.width, height: t.height, data_url: t.data_url(), ms: t.ms }).collect())
    })
    .await
}

/// 쪽 편집 계획 적용(순서·회전·지우기·뽑기·빈 쪽·다른 PDF·합치기). 결과 이름은 첫 원본 + `_쪽편집`.
#[tauri::command]
pub async fn pdf_apply_plan(app: AppHandle, state: State<'_, AppState>, job: u64, plan: EditPlan, out_dir: Option<String>) -> R<EditOut> {
    let d = dir(&out_dir);
    job_work(app, &state, job, move |j| {
        let first = plan.sources.first().map(|s| s.path.clone()).unwrap_or_default();
        plan::apply(&plan, &default_output(&first, d.as_deref(), "_쪽편집"), j)
    })
    .await
}

#[tauri::command]
pub async fn pdf_extract(app: AppHandle, state: State<'_, AppState>, job: u64, path: String, password: Option<String>, pages: String, out_dir: Option<String>) -> R<EditOut> {
    let d = dir(&out_dir);
    job_work(app, &state, job, move |j| {
        let p = PathBuf::from(&path);
        plan::extract(&p, pw(&password), &pages, &default_output(&p, d.as_deref(), "_쪽뽑기"), j)
    })
    .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pdf_insert_blank(
    app: AppHandle,
    state: State<'_, AppState>,
    job: u64,
    path: String,
    password: Option<String>,
    after: u32,
    count: u32,
    size: BlankSize,
    out_dir: Option<String>,
) -> R<EditOut> {
    let d = dir(&out_dir);
    job_work(app, &state, job, move |j| {
        let p = PathBuf::from(&path);
        plan::insert_blank(&p, pw(&password), after, count, &size, &default_output(&p, d.as_deref(), "_빈쪽추가"), j)
    })
    .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pdf_insert_pdf(
    app: AppHandle,
    state: State<'_, AppState>,
    job: u64,
    path: String,
    password: Option<String>,
    after: u32,
    other: String,
    other_password: Option<String>,
    other_pages: Option<String>,
    out_dir: Option<String>,
) -> R<EditOut> {
    let d = dir(&out_dir);
    job_work(app, &state, job, move |j| {
        let p = PathBuf::from(&path);
        plan::insert_pdf(&p, pw(&password), after, Path::new(&other), pw(&other_password), other_pages.as_deref(), &default_output(&p, d.as_deref(), "_끼워넣기"), j)
    })
    .await
}

// ---------- 얹기 ----------

macro_rules! opts_job {
    ($name:ident, $opts:ty, $func:path, $suffix:literal) => {
        #[tauri::command]
        pub async fn $name(app: AppHandle, state: State<'_, AppState>, job: u64, path: String, password: Option<String>, opts: $opts, out_dir: Option<String>) -> R<EditOut> {
            let d = dir(&out_dir);
            job_work(app, &state, job, move |j| {
                let p = PathBuf::from(&path);
                $func(&p, pw(&password), &opts, &default_output(&p, d.as_deref(), $suffix), j)
            })
            .await
        }
    };
}

opts_job!(pdf_page_numbers, PageNumberOpts, stamp::page_numbers, "_쪽번호");
opts_job!(pdf_watermark, WatermarkOpts, stamp::watermark, "_워터마크");
opts_job!(pdf_place_image, SignatureOpts, stamp::place_image, "_서명");
opts_job!(pdf_crop, CropOpts, layout::crop, "_여백자름");
opts_job!(pdf_fit_a4, FitOpts, layout::fit_a4, "_A4");
opts_job!(pdf_nup, NupOpts, layout::nup, "_모아찍기");

#[tauri::command]
pub async fn pdf_text_boxes(app: AppHandle, state: State<'_, AppState>, job: u64, path: String, password: Option<String>, boxes: Vec<TextBox>, out_dir: Option<String>) -> R<EditOut> {
    let d = dir(&out_dir);
    job_work(app, &state, job, move |j| {
        let p = PathBuf::from(&path);
        stamp::text_boxes(&p, pw(&password), &boxes, &default_output(&p, d.as_deref(), "_글상자"), j)
    })
    .await
}

// ---------- 암호 ----------

#[tauri::command]
pub async fn pdf_protect(path: String, password: Option<String>, opts: ProtectOpts, out_dir: Option<String>) -> R<EditOut> {
    let d = dir(&out_dir);
    work(move || {
        let p = PathBuf::from(&path);
        crypt::protect(&p, pw(&password), &opts, &default_output(&p, d.as_deref(), "_암호"))
    })
    .await
}

#[tauri::command]
pub async fn pdf_unprotect(path: String, password: String, out_dir: Option<String>) -> R<EditOut> {
    let d = dir(&out_dir);
    work(move || {
        let p = PathBuf::from(&path);
        crypt::unprotect(&p, &password, &default_output(&p, d.as_deref(), "_암호풀림"))
    })
    .await
}

#[tauri::command]
pub async fn pdf_protection(path: String, password: Option<String>) -> R<Protection> {
    work(move || crypt::protection(Path::new(&path), pw(&password))).await
}

// ---------- 문서 정보 ----------

#[tauri::command]
pub async fn pdf_info_get(path: String, password: Option<String>) -> R<InfoRead> {
    work(move || info::read(Path::new(&path), pw(&password))).await
}

#[tauri::command]
pub async fn pdf_info_set(path: String, password: Option<String>, info: DocInfo, out_dir: Option<String>) -> R<EditOut> {
    let d = dir(&out_dir);
    work(move || {
        let p = PathBuf::from(&path);
        info::set(&p, pw(&password), &info, &default_output(&p, d.as_deref(), "_정보수정"))
    })
    .await
}

#[tauri::command]
pub async fn pdf_info_clear(path: String, password: Option<String>, out_dir: Option<String>) -> R<EditOut> {
    let d = dir(&out_dir);
    work(move || {
        let p = PathBuf::from(&path);
        info::clear(&p, pw(&password), &default_output(&p, d.as_deref(), "_정보삭제"))
    })
    .await
}

// ---------- 양식 ----------

#[tauri::command]
pub async fn pdf_form_list(path: String, password: Option<String>) -> R<Vec<FieldInfo>> {
    work(move || forms::list(Path::new(&path), pw(&password))).await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn pdf_form_fill(
    app: AppHandle,
    state: State<'_, AppState>,
    job: u64,
    path: String,
    password: Option<String>,
    values: BTreeMap<String, String>,
    flatten: bool,
    out_dir: Option<String>,
) -> R<EditOut> {
    let d = dir(&out_dir);
    job_work(app, &state, job, move |j| {
        let p = PathBuf::from(&path);
        forms::fill(&p, pw(&password), &values, flatten, &default_output(&p, d.as_deref(), "_양식채움"), j)
    })
    .await
}

#[tauri::command]
pub async fn pdf_flatten(app: AppHandle, state: State<'_, AppState>, job: u64, path: String, password: Option<String>, out_dir: Option<String>) -> R<EditOut> {
    let d = dir(&out_dir);
    job_work(app, &state, job, move |j| {
        let p = PathBuf::from(&path);
        forms::flatten(&p, pw(&password), &default_output(&p, d.as_deref(), "_평평하게"), j)
    })
    .await
}
