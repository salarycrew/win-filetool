//! 0.3.2 핵심 6개 할 일 화면과 우클릭 메뉴용 Tauri 명령.
//!
//! - `pdf_compress_target`: 목표 크기(바이트) 이하가 될 때까지 사진 해상도·품질을 낮춰 본다(취소·진행 "job-progress").
//! - `pdf_merge_plan`: 쪽 편집 계획(빼 둔 쪽 제외)으로 합치기. 결과 이름 `첫파일_합침.pdf`.
//! - `photo_shrink`: 사진 여러 장을 JPG로(긴 변 제한). 결과가 원본보다 크면 버리고 "그대로"라고 알림. 4장씩 동시에.
//! - `nfc_fix_paths`: 고른 파일·폴더 이름 자체를 NFC로(맥 자모 분리). 같은 이름이 있으면 건너뜀.
//! - `ctx_menu_get` / `ctx_menu_set`: 우클릭 메뉴 상태·켜기·끄기(다시 연결 = 켜기).
//! - `ctx_take`: 우클릭으로 온 파일 묶음 꺼내기(마지막 인자가 온 뒤 0.7초 조용해질 때까지 기다림).
//! - `ui_ready`: 첫 화면이 그려진 시각(시작 시간 재기, `WIN_FILETOOL_STARTUP_LOG`가 있을 때만 파일에 적음).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use filetool_core::convert::{self, Op};
use filetool_core::ctxmenu::{self, Collector, CtxBatch, CtxStatus};
use filetool_core::imageconv::{ImageOpts, ImgFormat};
use filetool_core::nfc;
use filetool_core::pdfcompress::{self, CompressOut};
use filetool_core::pdfedit::plan::{self, EditPlan};
use filetool_core::pdfedit::{EditOut, Job};
use filetool_core::Error;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::cmd_pdfedit::CmdError;
use crate::cmd_tools::{err, job_work};
use crate::AppState;

type R<T> = Result<T, CmdError>;

fn dir(d: &Option<String>) -> Option<PathBuf> {
    d.as_ref().filter(|s| !s.is_empty()).map(PathBuf::from)
}

#[derive(Clone, Serialize)]
struct Progress {
    job: u64,
    fraction: f64,
}

// ---------- PDF 용량 줄이기(목표 크기) ----------

#[tauri::command]
pub async fn pdf_compress_target(app: AppHandle, state: State<'_, AppState>, job: u64, path: String, target: u64, out_dir: Option<String>) -> R<CompressOut> {
    let d = dir(&out_dir);
    job_work(app.clone(), &state, Some(job), move |cancel, _emit| {
        pdfcompress::compress_to_target(Path::new(&path), d.as_deref(), target.max(10_000), Some(cancel), &mut |i, n| {
            let _ = app.emit("job-progress", Progress { job, fraction: i as f64 / n.max(1) as f64 });
        })
    })
    .await
}

// ---------- PDF 합치기(쪽 빼기 포함) ----------

#[tauri::command]
pub async fn pdf_merge_plan(app: AppHandle, state: State<'_, AppState>, job: u64, plan: EditPlan, out_dir: Option<String>) -> R<EditOut> {
    let d = dir(&out_dir);
    let cancel = state.begin_job(job);
    let app2 = app.clone();
    let res = tauri::async_runtime::spawn_blocking(move || {
        let mut cb = |i: u32, n: u32| {
            let _ = app2.emit("job-progress", Progress { job, fraction: i as f64 / n.max(1) as f64 });
        };
        let mut j = Job::new(Some(&cancel), &mut cb);
        let first = plan.sources.first().map(|s| s.path.clone()).unwrap_or_default();
        let out = filetool_core::paths::output_path(&first, d.as_deref(), "_합침", "pdf");
        plan::apply(&plan, &out, &mut j)
    })
    .await
    .map_err(|e| CmdError { code: "error", message: format!("작업 스레드 오류: {e}") })?
    .map_err(CmdError::from);
    state.end_job(job);
    res
}

// ---------- 사진 용량 줄이기·JPG로 ----------

#[derive(Debug, Clone, Serialize)]
pub struct PhotoItem {
    input: PathBuf,
    output: Option<PathBuf>,
    before: u64,
    after: u64,
    note: Option<String>,
    error: Option<String>,
    /// 원본보다 커져서 새 파일을 만들지 않음
    unchanged: bool,
}

fn len(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

fn shrink_one(ts: &filetool_core::tools::Toolset, src: &Path, opts: &ImageOpts, out_dir: Option<&Path>, cancel: &filetool_core::process::Cancel) -> PhotoItem {
    let before = len(src);
    let op = Op::Image(opts.clone());
    match convert::convert_file(ts, src, out_dir, &op, Some(cancel), &mut |_| {}) {
        Ok(o) => {
            let after = len(&o.output);
            let is_jpg = src.extension().and_then(|e| e.to_str()).map(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "jfif")).unwrap_or(false);
            if is_jpg && after >= before {
                let _ = std::fs::remove_file(&o.output);
                return PhotoItem { input: src.into(), output: None, before, after: before, note: Some("이미 작아서 그대로 두었어요".into()), error: None, unchanged: true };
            }
            PhotoItem { input: src.into(), output: Some(o.output), before, after, note: None, error: None, unchanged: false }
        }
        Err(e) => PhotoItem { input: src.into(), output: None, before, after: 0, note: None, error: Some(e.to_string()), unchanged: false },
    }
}

#[tauri::command]
pub async fn photo_shrink(app: AppHandle, state: State<'_, AppState>, job: u64, paths: Vec<String>, max_side: Option<u32>, quality: Option<u8>, out_dir: Option<String>) -> R<Vec<PhotoItem>> {
    let d = dir(&out_dir);
    let ts = state.toolset();
    job_work(app, &state, Some(job), move |cancel, emit| {
        let opts = ImageOpts { format: ImgFormat::Jpg, quality: quality.unwrap_or(85).clamp(40, 95), max_side: max_side.filter(|m| *m >= 320), keep_exif: true };
        let n = paths.len();
        let results: Vec<Mutex<Option<PhotoItem>>> = (0..n).map(|_| Mutex::new(None)).collect();
        let next = AtomicUsize::new(0);
        let done = AtomicUsize::new(0);
        let workers = std::thread::available_parallelism().map(|v| v.get()).unwrap_or(2).clamp(1, 4).min(n.max(1));
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::scope(|s| {
            for _ in 0..workers {
                let tx = tx.clone();
                let (ts, opts, d, paths, results, next, done) = (&ts, &opts, &d, &paths, &results, &next, &done);
                s.spawn(move || loop {
                    if cancel.is_cancelled() {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= n {
                        break;
                    }
                    let it = shrink_one(ts, Path::new(&paths[i]), opts, d.as_deref(), cancel);
                    *results[i].lock().unwrap() = Some(it);
                    done.fetch_add(1, Ordering::SeqCst);
                    let _ = tx.send(paths[i].clone());
                });
            }
            drop(tx);
            for cur in rx {
                emit.send(done.load(Ordering::SeqCst) as f64, n as f64, &cur);
            }
        });
        if cancel.is_cancelled() {
            // 멈추면 이번에 만든 파일은 지운다(원본은 건드리지 않음)
            for r in &results {
                if let Some(PhotoItem { output: Some(o), .. }) = r.lock().unwrap().as_ref() {
                    let _ = std::fs::remove_file(o);
                }
            }
            return Err(Error::Cancelled);
        }
        Ok(results.into_iter().filter_map(|m| m.into_inner().unwrap()).collect())
    })
    .await
}

// ---------- 맥에서 온 이름 고치기(고른 파일·폴더 이름 자체) ----------

#[derive(Debug, Clone, Serialize)]
pub struct NfcFix {
    from: PathBuf,
    to: Option<PathBuf>,
    note: Option<String>,
}

#[tauri::command]
pub async fn nfc_fix_paths(paths: Vec<String>) -> R<Vec<NfcFix>> {
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|p| {
                let from = PathBuf::from(&p);
                let Some(name) = from.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                    return NfcFix { from, to: None, note: Some("이름이 없어요".into()) };
                };
                let fixed = nfc::to_nfc(&name);
                if fixed == name {
                    return NfcFix { from, to: None, note: None };
                }
                let to = from.with_file_name(&fixed);
                if to.exists() {
                    return NfcFix { from, to: None, note: Some(format!("같은 이름({fixed})이 이미 있어 건너뛰었어요")) };
                }
                match std::fs::rename(&from, &to) {
                    Ok(()) => NfcFix { from, to: Some(to), note: None },
                    Err(e) => NfcFix { from, to: None, note: Some(format!("바꾸지 못했어요: {e}")) },
                }
            })
            .collect()
    })
    .await
    .map_err(|e| CmdError { code: "error", message: format!("작업 스레드 오류: {e}") })
}

// ---------- 우클릭 메뉴 ----------

fn exe() -> R<PathBuf> {
    std::env::current_exe().map_err(|e| CmdError { code: "error", message: format!("실행 파일 경로를 알 수 없습니다: {e}") })
}

#[tauri::command]
pub fn ctx_menu_get() -> R<CtxStatus> {
    Ok(ctxmenu::status(&exe()?))
}

#[tauri::command]
pub fn ctx_menu_set(state: State<'_, AppState>, enabled: bool) -> R<CtxStatus> {
    let st = ctxmenu::set(enabled, &exe()?).map_err(err)?;
    {
        let mut s = state.settings.lock().unwrap();
        s.context_menu_off = !enabled;
    }
    let _ = state.save();
    Ok(st)
}

/// 우클릭으로 온 인자를 모으는 곳(앱 상태)
#[derive(Default)]
pub struct CtxQueue(pub Mutex<Collector>);

impl CtxQueue {
    pub fn push_args<I: IntoIterator<Item = S>, S: AsRef<str>>(&self, args: I) -> bool {
        match ctxmenu::parse_args(args) {
            Some((job, files)) if !files.is_empty() => {
                self.0.lock().unwrap().push(job, files, Instant::now());
                true
            }
            _ => false,
        }
    }
}

/// 두 번째 실행에서 넘어온 우클릭 인자: 모아 두고, 조용해지면 화면에 "ctx-ready"를 알린다.
pub fn on_second_instance(app: &AppHandle, argv: &[String]) -> bool {
    let q = app.state::<CtxQueue>();
    if !q.push_args(argv) {
        return false;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(ctxmenu::QUIET + Duration::from_millis(50));
        let quiet = app.state::<CtxQueue>().0.lock().unwrap().is_quiet(Instant::now(), ctxmenu::QUIET);
        if quiet {
            let _ = app.emit("ctx-ready", ());
        }
    });
    true
}

/// 모인 우클릭 묶음 꺼내기. 아직 파일이 오는 중이면 조용해질 때까지(최대 8초) 기다린다. 없으면 빈 목록.
#[tauri::command]
pub async fn ctx_take(app: AppHandle) -> Vec<CtxBatch> {
    let t0 = Instant::now();
    loop {
        {
            let q = app.state::<CtxQueue>();
            let mut c = q.0.lock().unwrap();
            if c.is_empty() {
                return Vec::new();
            }
            if let Some(b) = c.take_if_quiet(Instant::now(), ctxmenu::QUIET) {
                return b;
            }
            if t0.elapsed() > Duration::from_secs(8) {
                return c.take();
            }
        }
        tauri::async_runtime::spawn_blocking(|| std::thread::sleep(Duration::from_millis(100))).await.ok();
    }
}

// ---------- 시작 시간 ----------

/// 프로세스가 만들어진 뒤 지난 시간(ms). Windows는 GetProcessTimes, 그 밖은 앱이 기록한 시각부터.
pub fn since_start_ms(start: Instant) -> f64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        if unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) } != 0 {
            let created = ((c.dwHighDateTime as u64) << 32 | c.dwLowDateTime as u64) as i128;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as i128 / 100 + 116_444_736_000_000_000).unwrap_or(0);
            if now > created {
                return (now - created) as f64 / 10_000.0;
            }
        }
    }
    start.elapsed().as_secs_f64() * 1000.0
}

pub struct StartTime(pub Instant);

/// 화면이 첫 화면을 다 그렸을 때 부른다. `WIN_FILETOOL_STARTUP_LOG=<파일>`이면 그 파일에 한 줄 덧붙인다.
#[tauri::command]
pub fn ui_ready(start: State<'_, StartTime>, what: Option<String>) -> f64 {
    let ms = since_start_ms(start.0);
    if let Some(p) = std::env::var_os("WIN_FILETOOL_STARTUP_LOG").filter(|v| !v.is_empty()) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{} {:.0}", what.as_deref().unwrap_or("home"), ms);
        }
    }
    ms
}
