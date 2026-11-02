// 릴리스 빌드에서는 콘솔 창 없이 GUI로만 뜬다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! 파일 도구(Tauri 2). 실제 작업은 `filetool-core`가 하고, 여기서는
//! 화면이 멈추지 않도록 작업을 별도 스레드(spawn_blocking)에서 돌린 뒤 결과만 돌려준다.
//! 오류는 코어의 한국어 메시지를 그대로 문자열로 넘긴다.
//! 1b: 변환(진행률 이벤트·취소), 도구 설정(경로 저장), 감시 폴더(앱이 열려 있는 동안 백그라운드 스레드).
//! 1c: PDF 압축·이미지↔PDF, 트레이(열기/감시 일시정지/끝내기), 윈도 시작 시 실행(`--autostart`면 창 없이 트레이로),
//!     감시 규칙이 켜져 있으면 창을 닫아도 트레이에 남음, 한 번만 실행(두 번째 실행은 기존 창을 앞으로).
//! 2a: 검색 탭(첫 탭). 검색 서비스(색인 불러오기 → 배경 스캔 → 실시간 갱신)는 앱이 켜지면(트레이 포함) 바로 돈다.
//!     트레이 [검색 열기], 전역 단축키(기본 꺼짐), 끝낼 때 색인 저장.
//! 2c: 빠른 검색 서비스(`filetool-svc`, 선택 설치)가 답하면 그 서비스가 색인한 드라이브는 서비스에 묻고(상태 줄 "빠른 색인(서비스)"),
//!     나머지 루트만 앱이 색인한다. 서비스가 없거나 멈추면 조용히 앱 색인으로. [설정] > 검색에서 설치/제거(UAC 1번).

mod cmd_core;
mod cmd_doctools;
mod cmd_runtime;
mod cmd_pdfedit;
mod cmd_office;
mod cmd_tools;
mod cmd_hwp;
mod cmd_ui;
mod cmd_update;
mod icons;
mod remote;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use filetool_core::convert::{self, ConvertOut, Op};
use filetool_core::pdf::{self, SplitMode};
use filetool_core::process::Cancel;
use filetool_core::settings::Settings;
use filetool_core::tools::{self, SearchEnv, Tool, ToolStatus, Toolset};
use filetool_core::watch::{WatchEvent, WatchRule, Watcher};
use filetool_core::img2pdf::{self, Img2PdfOpts, Img2PdfOut};
use filetool_core::pdfcompress::{self, CompressOut, CompressPreset};
use filetool_core::pdfrender::{self, ExtractOut, RenderOpts, RenderOut};
use filetool_core::search::index::root_key;
use filetool_core::search::query::{Row, SortKey};
use filetool_core::search::service::{default_index_dir, RootStatus, SearchConfig, SearchResult, SearchService, SearchStatus};
use filetool_core::search::svcproto::SERVICE_SOURCE_LABEL;
use remote::{Remote, ServiceView};
use filetool_core::settings::{MonitorRect, SearchSettings, UiState, WindowGeom};
use filetool_core::{autostart, nfc, paths, zipnfc};
use serde::Serialize;
use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

type CmdResult<T> = Result<T, String>;

/// 무거운 작업을 작업 스레드에서 실행한다(창이 멈추지 않게).
async fn blocking<T, F>(f: F) -> CmdResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> filetool_core::Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("작업 스레드 오류: {e}"))?
        .map_err(|e| e.to_string())
}

fn out_dir(dir: &Option<String>) -> Option<PathBuf> {
    dir.as_ref().filter(|s| !s.is_empty()).map(PathBuf::from)
}

#[derive(Serialize)]
struct Item {
    path: PathBuf,
    kind: &'static str,
    pages: Option<u32>,
}

#[tauri::command]
async fn inspect_paths(paths: Vec<String>) -> CmdResult<Vec<Item>> {
    blocking(move || {
        Ok(paths
            .into_iter()
            .map(|p| {
                let path = PathBuf::from(p);
                let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
                let (kind, pages) = if path.is_dir() {
                    ("dir", None)
                } else if !path.exists() {
                    ("missing", None)
                } else {
                    match ext.as_deref() {
                        Some("pdf") => ("pdf", pdf::page_count(&path).ok()),
                        Some("zip") => ("zip", None),
                        _ => (
                            match convert::classify(&path) {
                                convert::FileKind::Image => "image",
                                convert::FileKind::Heic => "heic",
                                convert::FileKind::Video => "video",
                                convert::FileKind::Audio => "audio",
                                convert::FileKind::Document => "document",
                                convert::FileKind::Other => "other",
                            },
                            None,
                        ),
                    }
                };
                Item { path, kind, pages }
            })
            .collect())
    })
    .await
}

#[tauri::command]
async fn nfc_preview(folder: String) -> CmdResult<nfc::FolderPlan> {
    blocking(move || nfc::plan_folder(Path::new(&folder))).await
}

#[tauri::command]
async fn nfc_apply(folder: String) -> CmdResult<nfc::ApplyReport> {
    blocking(move || nfc::normalize_folder(Path::new(&folder))).await
}

#[derive(Serialize)]
pub(crate) struct ZipResult {
    extract: zipnfc::ExtractReport,
    rezip: Option<zipnfc::RezipReport>,
}

/// 예전 화면의 zip 풀기(맥 zip NFC + 다시 압축). 명령 이름 `zip_extract`는 0.3.0 W4 [`cmd_tools::zip_extract`]가 받아 넘긴다.
pub(crate) fn zip_extract_legacy(zip: PathBuf, dir: Option<PathBuf>, rezip: bool) -> filetool_core::Result<ZipResult> {
    let target = match &dir {
        Some(d) => {
            let stem = zip.file_stem().map(|s| nfc::to_nfc(&s.to_string_lossy())).unwrap_or_default();
            paths::unique_path(&d.join(stem))
        }
        None => zipnfc::default_extract_dir(&zip),
    };
    let extract = zipnfc::extract_nfc(&zip, &target)?;
    let rezip = if rezip {
        Some(zipnfc::rezip_nfc(&zip, &zipnfc::default_rezip_path(&zip, dir.as_deref()))?)
    } else {
        None
    };
    Ok(ZipResult { extract, rezip })
}

#[derive(Serialize)]
struct PdfOut {
    output: PathBuf,
    pages: u32,
}

#[tauri::command]
async fn pdf_merge(paths: Vec<String>, out_dir: Option<String>) -> CmdResult<PdfOut> {
    let dir = self::out_dir(&out_dir);
    blocking(move || {
        let inputs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        let first = inputs.first().cloned().unwrap_or_default();
        let output = paths::output_path(&first, dir.as_deref(), "_합침", "pdf");
        let pages = pdf::merge(&inputs, &output)?;
        Ok(PdfOut { output, pages })
    })
    .await
}

#[tauri::command]
async fn pdf_split(path: String, mode: SplitMode, out_dir: Option<String>) -> CmdResult<Vec<PathBuf>> {
    let dir = self::out_dir(&out_dir);
    blocking(move || pdf::split(Path::new(&path), &mode, dir.as_deref())).await
}

#[tauri::command]
async fn pdf_rotate(path: String, pages: String, degrees: i64, out_dir: Option<String>) -> CmdResult<PathBuf> {
    let dir = self::out_dir(&out_dir);
    blocking(move || {
        let src = PathBuf::from(path);
        let output = paths::output_path(&src, dir.as_deref(), &format!("_회전{degrees}"), "pdf");
        pdf::rotate(&src, &pages, degrees, &output)?;
        Ok(output)
    })
    .await
}

#[tauri::command]
async fn pdf_delete(path: String, pages: String, out_dir: Option<String>) -> CmdResult<PdfOut> {
    let dir = self::out_dir(&out_dir);
    blocking(move || {
        let src = PathBuf::from(path);
        let output = paths::output_path(&src, dir.as_deref(), "_페이지삭제", "pdf");
        let pages = pdf::delete_pages(&src, &pages, &output)?;
        Ok(PdfOut { output, pages })
    })
    .await
}

#[tauri::command]
async fn pdf_reorder(path: String, order: String, out_dir: Option<String>) -> CmdResult<PathBuf> {
    let dir = self::out_dir(&out_dir);
    blocking(move || {
        let src = PathBuf::from(path);
        let output = paths::output_path(&src, dir.as_deref(), "_순서변경", "pdf");
        pdf::reorder(&src, &order, &output)?;
        Ok(output)
    })
    .await
}

// ---------- 1b: 변환·도구 설정·감시 폴더 ----------

struct AppState {
    config_dir: PathBuf,
    settings: Mutex<Settings>,
    jobs: Mutex<HashMap<u64, Cancel>>,
    watch_log: Mutex<VecDeque<WatchEvent>>,
    /// 트레이 [감시 일시정지]
    watch_paused: AtomicBool,
    pause_item: Mutex<Option<CheckMenuItem<tauri::Wry>>>,
    /// 2a 검색
    search: Arc<SearchService>,
    /// 마지막 검색 결과
    results: Mutex<Results>,
    /// 2c 빠른 검색 서비스
    remote: Arc<Remote>,
    /// 지금 서비스에 묻는 루트(앱 색인에서는 뺌)
    covered: Mutex<Vec<PathBuf>>,
    /// ux3: 창 위치·크기가 바뀐 시각(조용해지면 설정 파일에 저장)
    ui_dirty: Mutex<Option<Instant>>,
}

/// 마지막 검색 결과: 앱 색인만, 또는 서비스(앞) + 앱 색인(뒤)
enum Results {
    Local { gen: u64, ids: Vec<u32> },
    Mixed { remote_total: usize, local_gen: u64, local_ids: Vec<u32> },
}

impl AppState {
    fn toolset(&self) -> Toolset {
        let paths = self.settings.lock().unwrap().tools.clone();
        Toolset::discover(&paths, &SearchEnv::from_system())
    }
    fn save(&self) -> CmdResult<()> {
        self.settings.lock().unwrap().save(&self.config_dir).map_err(|e| e.to_string())
    }
    fn any_rule_enabled(&self) -> bool {
        self.settings.lock().unwrap().watch_rules.iter().any(|r| r.enabled)
    }
    /// 창을 닫아도 트레이에 남을지: 감시 규칙, 윈도 시작 시 실행(검색 색인을 계속 갱신), 전역 단축키 중 하나라도 켜져 있으면
    fn keep_in_tray(&self) -> bool {
        let hotkey = self.settings.lock().unwrap().search.hotkey_enabled;
        self.any_rule_enabled() || hotkey || std::env::current_exe().map(|e| autostart::status(&e).enabled).unwrap_or(false)
    }
    fn begin_job(&self, job: u64) -> Cancel {
        let c = Cancel::new();
        self.jobs.lock().unwrap().insert(job, c.clone());
        c
    }
    fn end_job(&self, job: u64) {
        self.jobs.lock().unwrap().remove(&job);
    }
}

#[tauri::command]
async fn tools_status(state: State<'_, AppState>) -> CmdResult<Vec<ToolStatus>> {
    let paths = state.settings.lock().unwrap().tools.clone();
    blocking(move || Ok(tools::status_all(&paths, &SearchEnv::from_system()))).await
}

/// 도구 경로 지정(빈 값이면 자동 찾기로 되돌림). ffprobe는 ffmpeg 폴더에서 찾는다.
#[tauri::command]
async fn set_tool_path(state: State<'_, AppState>, tool: Tool, path: Option<String>) -> CmdResult<Vec<ToolStatus>> {
    let p = path.filter(|s| !s.trim().is_empty()).map(PathBuf::from);
    if let Some(p) = &p {
        if tools::resolve_user_path(tool, p).is_none() {
            return Err(format!("그 위치에서 {}을(를) 찾지 못했습니다: {}", tool.exe_name(), p.display()));
        }
    }
    {
        let mut s = state.settings.lock().unwrap();
        match tool {
            Tool::Ffmpeg | Tool::Ffprobe => s.tools.ffmpeg = p,
            Tool::Soffice => s.tools.soffice = p,
        }
    }
    state.save()?;
    tools_status(state).await
}

#[derive(Clone, Serialize)]
struct Progress {
    job: u64,
    fraction: f64,
}

/// 파일 하나 변환. 진행률은 "job-progress" 이벤트로, 취소는 cancel_jobs로.
#[tauri::command]
async fn convert_one(app: AppHandle, state: State<'_, AppState>, job: u64, path: String, op: Op, out_dir: Option<String>) -> CmdResult<ConvertOut> {
    let dir = self::out_dir(&out_dir);
    let cancel = Cancel::new();
    state.jobs.lock().unwrap().insert(job, cancel.clone());
    let ts = state.toolset();
    let res = blocking(move || {
        let mut last = -1i64;
        convert::convert_file(&ts, Path::new(&path), dir.as_deref(), &op, Some(&cancel), &mut |f| {
            let pct = (f * 100.0) as i64;
            if pct != last {
                last = pct;
                let _ = app.emit("job-progress", Progress { job, fraction: f });
            }
        })
    })
    .await;
    state.jobs.lock().unwrap().remove(&job);
    res
}

#[tauri::command]
fn cancel_jobs(state: State<'_, AppState>) -> usize {
    let jobs = state.jobs.lock().unwrap();
    for c in jobs.values() {
        c.cancel();
    }
    jobs.len()
}

#[derive(Serialize)]
struct WatchView {
    rules: Vec<WatchRule>,
    log: Vec<WatchEvent>,
    paused: bool,
}

#[tauri::command]
fn watch_get(state: State<'_, AppState>) -> WatchView {
    WatchView {
        rules: state.settings.lock().unwrap().watch_rules.clone(),
        log: state.watch_log.lock().unwrap().iter().cloned().collect(),
        paused: state.watch_paused.load(Ordering::SeqCst),
    }
}

/// 감시 일시정지(트레이 메뉴와 화면이 같은 값을 씀)
fn set_paused(app: &AppHandle, paused: bool) {
    let state = app.state::<AppState>();
    state.watch_paused.store(paused, Ordering::SeqCst);
    if let Some(item) = state.pause_item.lock().unwrap().as_ref() {
        let _ = item.set_checked(paused);
    }
    update_tray_tooltip(app);
    let _ = app.emit("watch-paused", paused);
}

#[tauri::command]
fn watch_set_paused(app: AppHandle, paused: bool) -> bool {
    set_paused(&app, paused);
    paused
}

/// 규칙 목록 저장. 켠 규칙은 검사(폴더 존재, 출력 하위 폴더 이름)를 통과해야 한다.
#[tauri::command]
fn watch_set_rules(app: AppHandle, state: State<'_, AppState>, rules: Vec<WatchRule>) -> CmdResult<Vec<WatchRule>> {
    for r in rules.iter().filter(|r| r.enabled) {
        r.validate()?;
    }
    state.settings.lock().unwrap().watch_rules = rules.clone();
    state.save()?;
    update_tray_tooltip(&app);
    Ok(rules)
}

// ---------- 1c: PDF 압축·이미지↔PDF ----------

#[tauri::command]
async fn pdf_compress(path: String, preset: CompressPreset, out_dir: Option<String>) -> CmdResult<CompressOut> {
    let dir = self::out_dir(&out_dir);
    blocking(move || pdfcompress::compress(Path::new(&path), dir.as_deref(), preset)).await
}

/// 이미지 여러 장 → PDF 하나(목록 순서). 진행률은 장 수 기준.
#[tauri::command]
async fn images_to_pdf(app: AppHandle, state: State<'_, AppState>, job: u64, paths: Vec<String>, opts: Img2PdfOpts, out_dir: Option<String>) -> CmdResult<Img2PdfOut> {
    let dir = self::out_dir(&out_dir);
    let cancel = state.begin_job(job);
    let ffmpeg = state.toolset().ffmpeg;
    let res = blocking(move || {
        let inputs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        let output = img2pdf::default_output(&inputs, dir.as_deref());
        let n = inputs.len().max(1) as f64;
        img2pdf::images_to_pdf(ffmpeg.as_deref(), &inputs, &output, &opts, Some(&cancel), &mut |i| {
            let _ = app.emit("job-progress", Progress { job, fraction: i as f64 / n });
        })
    })
    .await;
    state.end_job(job);
    res
}

#[tauri::command]
async fn pdf_to_images(app: AppHandle, state: State<'_, AppState>, job: u64, path: String, opts: RenderOpts, out_dir: Option<String>) -> CmdResult<RenderOut> {
    let dir = self::out_dir(&out_dir);
    let cancel = state.begin_job(job);
    let res = blocking(move || {
        pdfrender::render_pages(Path::new(&path), dir.as_deref(), &opts, Some(&cancel), &mut |i, n| {
            let _ = app.emit("job-progress", Progress { job, fraction: i as f64 / n.max(1) as f64 });
        })
    })
    .await;
    state.end_job(job);
    res
}

#[tauri::command]
async fn pdf_extract_images(path: String, out_dir: Option<String>) -> CmdResult<ExtractOut> {
    let dir = self::out_dir(&out_dir);
    blocking(move || pdfrender::extract_images(Path::new(&path), dir.as_deref())).await
}

#[tauri::command]
async fn pdfium_status() -> CmdResult<String> {
    tauri::async_runtime::spawn_blocking(|| pdfrender::pdfium_status().map(|p| p.display().to_string()))
        .await
        .map_err(|e| e.to_string())?
}

// ---------- 1c: 윈도 시작 시 실행 ----------

fn current_exe() -> CmdResult<PathBuf> {
    std::env::current_exe().map_err(|e| format!("실행 파일 위치를 알 수 없습니다: {e}"))
}

#[tauri::command]
fn autostart_get() -> CmdResult<autostart::AutostartStatus> {
    Ok(autostart::status(&current_exe()?))
}

#[tauri::command]
fn autostart_set(enabled: bool) -> CmdResult<autostart::AutostartStatus> {
    autostart::set(enabled, &current_exe()?).map_err(|e| e.to_string())
}


// ---------- 2a: 검색 ----------

/// 앱 색인 설정: 서비스가 맡은 루트(`covered`)는 뺀다
fn search_config(s: &SearchSettings, covered: &[PathBuf]) -> SearchConfig {
    let roots = s.effective_roots().into_iter().filter(|r| !covered.iter().any(|c| root_key(c) == root_key(r))).collect();
    SearchConfig::new(roots, s.excludes.clone(), &index_dir())
}

/// 색인 폴더(시험용 WIN_FILETOOL_INDEX_DIR — 시험용 앱만, 기본 %LOCALAPPDATA%\kr.salarycrew.winfiletool\index)
fn index_dir() -> PathBuf {
    default_index_dir()
}

#[tauri::command]
async fn search_query(state: State<'_, AppState>, query: String, sort: SortKey, desc: bool) -> CmdResult<SearchResult> {
    let svc = state.search.clone();
    let remote = state.remote.clone();
    let covered = state.covered.lock().unwrap().clone();
    let excludes = state.settings.lock().unwrap().search.excludes.clone();
    let (res, r) = tauri::async_runtime::spawn_blocking(move || -> CmdResult<(Results, SearchResult)> {
        let t0 = Instant::now();
        let (ids, local) = svc.query(&query, sort, desc)?;
        if !covered.is_empty() && !query.trim().is_empty() {
            match remote.query(&query, sort, desc, &excludes) {
                Ok(rr) => {
                    let r = SearchResult {
                        total: rr.total + local.total,
                        millis: t0.elapsed().as_secs_f64() * 1000.0,
                        generation: local.generation.wrapping_add(rr.generation),
                        version: local.version.wrapping_add(rr.version),
                    };
                    return Ok((Results::Mixed { remote_total: rr.total, local_gen: local.generation, local_ids: ids }, r));
                }
                // 질의 오류(문법)는 그대로 알리고, 연결 문제는 앱 색인으로(감시 스레드가 곧 루트를 되돌림)
                Err(e) if e.ends_with("[query]") => return Err(e.trim_end_matches(" [query]").to_string()),
                Err(e) => eprintln!("빠른 검색 서비스 질의 실패 → 앱 색인만: {e}"),
            }
        }
        Ok((Results::Local { gen: local.generation, ids }, local))
    })
    .await
    .map_err(|e| e.to_string())??;
    *state.results.lock().unwrap() = res;
    Ok(r)
}

#[derive(Serialize)]
struct RowsOut {
    rows: Vec<Row>,
    /// 색인이 통째로 바뀌어(다시 스캔 반영) 다시 검색해야 함
    stale: bool,
}

/// 결과 줄 [offset, offset+limit) — 화면에 보이는 만큼만. 서비스 결과가 앞, 앱 색인 결과가 뒤
#[tauri::command]
async fn search_rows(state: State<'_, AppState>, offset: usize, limit: usize) -> CmdResult<RowsOut> {
    let end = offset.saturating_add(limit);
    // (서비스 범위, 앱 색인 세대, 앱 색인 번호들)
    let (remote_range, gen, ids) = {
        let g = state.results.lock().unwrap();
        match &*g {
            Results::Local { gen, ids } => {
                let e = end.min(ids.len());
                (None, *gen, ids.get(offset.min(e)..e).map(|s| s.to_vec()).unwrap_or_default())
            }
            Results::Mixed { remote_total, local_gen, local_ids } => {
                let rt = *remote_total;
                let rr = (offset < rt).then(|| (offset, end.min(rt) - offset));
                let (lo, hi) = (offset.max(rt) - rt, end.max(rt) - rt);
                let hi = hi.min(local_ids.len());
                (rr, *local_gen, local_ids.get(lo.min(hi)..hi).map(|s| s.to_vec()).unwrap_or_default())
            }
        }
    };
    let svc = state.search.clone();
    let remote = state.remote.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut rows = Vec::new();
        if let Some((o, n)) = remote_range {
            match remote.rows(o, n) {
                Ok(Some(r)) => rows.extend(r),
                // 서비스 색인이 통째로 바뀜·연결 끊김 → 다시 검색
                _ => return RowsOut { rows: Vec::new(), stale: true },
            }
        }
        match svc.rows(&ids, gen) {
            Some(r) => rows.extend(r),
            None => return RowsOut { rows: Vec::new(), stale: true },
        }
        RowsOut { rows, stale: false }
    })
    .await
    .map_err(|e| e.to_string())
}

#[derive(Serialize)]
struct SearchView {
    status: SearchStatus,
    settings: SearchSettings,
    /// 지금 색인하는 루트(설정이 비어 있으면 고정 드라이브)
    effective_roots: Vec<PathBuf>,
    /// 2c: 서비스가 맡은 루트
    service_roots: Vec<PathBuf>,
}

/// 앱 색인 상태 + 서비스가 맡은 루트(“빠른 색인(서비스)”). 버전·세대는 둘을 더한 값(바뀌면 화면이 다시 검색)
fn merged_status(state: &AppState, effective: &[PathBuf]) -> (SearchStatus, Vec<PathBuf>) {
    let mut st = state.search.status();
    let covered = state.covered.lock().unwrap().clone();
    if !covered.is_empty() {
        if let Some((rs, me)) = state.remote.status() {
            let (g, v) = state.remote.seen();
            st.version = st.version.wrapping_add(v);
            st.generation = st.generation.wrapping_add(g);
            st.entries += rs.entries;
            st.live_updates += rs.live_updates;
            for r in rs.roots.iter().filter(|r| covered.iter().any(|c| root_key(c) == root_key(Path::new(&r.path)))) {
                st.skipped += r.stats.denied + r.stats.reparse;
                st.excluded += r.stats.excluded;
                st.roots.push(RootStatus {
                    path: r.path.clone(),
                    scanning: r.scanning,
                    scanned_so_far: r.scanned_so_far,
                    last_scan: r.last_scan,
                    stats: r.stats.clone(),
                    source: SERVICE_SOURCE_LABEL.to_string(),
                    source_note: Some(format!("빠른 검색 서비스(pid {})가 색인하고 저널로 따라갑니다 · {}", rs.pid, me.visibility)),
                    catchup: r.catchup.clone(),
                });
            }
        }
    }
    st.roots.sort_by_key(|r| effective.iter().position(|e| root_key(e) == root_key(Path::new(&r.path))).unwrap_or(usize::MAX));
    (st, covered)
}

#[tauri::command]
fn search_status(state: State<'_, AppState>) -> SearchView {
    let settings = state.settings.lock().unwrap().search.clone();
    let effective_roots = settings.effective_roots();
    let (status, service_roots) = merged_status(&state, &effective_roots);
    SearchView { status, effective_roots, settings, service_roots }
}

// ---------- 2c: 빠른 검색 서비스 ----------

/// 서비스 상태를 1초마다(없으면 2초마다) 확인하고, 서비스가 맡는 루트가 바뀌면 앱 색인 루트를 바꾼다
fn start_service_monitor(app: AppHandle) {
    std::thread::Builder::new()
        .name("svc-monitor".into())
        .spawn(move || loop {
            let state = app.state::<AppState>();
            let ok = state.remote.poll().is_some();
            refresh_coverage(&state);
            std::thread::sleep(Duration::from_millis(if ok { 1000 } else { 2000 }));
        })
        .ok();
}

fn refresh_coverage(state: &AppState) {
    let settings = state.settings.lock().unwrap().search.clone();
    let covered = state.remote.covered(&settings.effective_roots());
    let changed = {
        let mut c = state.covered.lock().unwrap();
        if *c != covered {
            *c = covered.clone();
            true
        } else {
            false
        }
    };
    if changed {
        state.search.reconfigure(search_config(&settings, &covered));
    }
}

/// 앱 폴더의 filetool-svc.exe
fn svc_exe() -> Option<PathBuf> {
    let p = std::env::current_exe().ok()?.parent()?.join("filetool-svc.exe");
    p.exists().then_some(p)
}

fn svc_report_file() -> PathBuf {
    std::env::temp_dir().join("win-filetool").join("svc-setup.txt")
}

/// 설치/제거 단추가 UAC(runas)로 실행할 (파일, 인자)
fn svc_command(install: bool) -> Option<(PathBuf, String)> {
    let exe = svc_exe()?;
    Some((exe, format!("{} --report \"{}\"", if install { "install" } else { "uninstall" }, svc_report_file().display())))
}

#[derive(Serialize)]
struct SvcPanel {
    #[serde(flatten)]
    view: ServiceView,
    install_cmd: Option<String>,
    uninstall_cmd: Option<String>,
    /// 앱에 든 서비스 버전
    bundled_version: &'static str,
    /// 설치된 서비스 버전(서비스가 답할 때만)
    installed_version: Option<String>,
    /// 설치된 서비스가 앱에 든 것과 다름 → [다시 설치(갱신)]
    update_available: bool,
}

#[tauri::command]
async fn svc_view(state: State<'_, AppState>) -> CmdResult<SvcPanel> {
    let remote = state.remote.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let fmt = |c: Option<(PathBuf, String)>| c.map(|(e, a)| format!("\"{}\" {a}", e.display()));
        let exe = svc_exe();
        let view = remote.view(exe.as_ref().map(|p| p.display().to_string()));
        // 방금 갱신했으면 예전 연결은 끊겼다: 한 번 더 붙어서 지금 서비스의 버전을 읽는다
        let installed_version = remote.poll().or_else(|| remote.poll()).and_then(|st| filetool_core::search::svcpipe::server_version(&st.server));
        let bundled_version = env!("CARGO_PKG_VERSION");
        let update_available = filetool_core::search::svcpipe::needs_update(&view.info, installed_version.as_deref(), exe.as_deref(), bundled_version);
        SvcPanel { view, install_cmd: fmt(svc_command(true)), uninstall_cmd: fmt(svc_command(false)), bundled_version, installed_version, update_available }
    })
    .await
    .map_err(|e| e.to_string())
}

/// 설치/제거: filetool-svc.exe를 관리자 권한으로(UAC 창 1번) 실행하고 끝날 때까지 기다린다. 결과 글
#[tauri::command]
async fn svc_setup(install: bool) -> CmdResult<String> {
    tauri::async_runtime::spawn_blocking(move || -> CmdResult<String> {
        let (exe, args) = svc_command(install).ok_or("앱 폴더에 filetool-svc.exe가 없습니다(설치 파일로 설치한 앱에 들어 있습니다)")?;
        let report = svc_report_file();
        let _ = std::fs::create_dir_all(report.parent().unwrap());
        let _ = std::fs::remove_file(&report);
        let code = elevate::run_as_admin(&exe, &args)?;
        let text = std::fs::read_to_string(&report).unwrap_or_default();
        let _ = std::fs::remove_file(&report);
        if code == 0 {
            Ok(text)
        } else {
            Err(format!("{}{}(종료 코드 {code})", text, if text.is_empty() { "" } else { "\n" }))
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

mod elevate {
    use std::path::Path;

    /// `runas`로 실행(UAC 창)하고 끝날 때까지 기다린다. 종료 코드
    #[cfg(windows)]
    pub fn run_as_admin(exe: &Path, args: &str) -> Result<u32, String> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
        use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SHELLEXECUTEINFOW};
        let w = |s: &std::ffi::OsStr| s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let verb = w("runas".as_ref());
        let file = w(exe.as_os_str());
        let params = w(args.as_ref());
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = 0x40 | 0x100; // SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = 0; // SW_HIDE
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            let e = unsafe { GetLastError() };
            return Err(if e == 1223 { "관리자 권한 확인(UAC)을 취소했습니다".into() } else { format!("관리자 권한으로 실행하지 못했습니다(오류 {e})") });
        }
        if info.hProcess.is_null() {
            return Err("설치 단계 프로세스를 알 수 없습니다".into());
        }
        unsafe { WaitForSingleObject(info.hProcess, 180_000) };
        let mut code = 1u32;
        unsafe {
            GetExitCodeProcess(info.hProcess, &mut code);
            CloseHandle(info.hProcess);
        }
        Ok(code)
    }

    #[cfg(not(windows))]
    pub fn run_as_admin(_exe: &Path, _args: &str) -> Result<u32, String> {
        Err("Windows에서만 됩니다".into())
    }
}

/// 검색 설정 저장(루트·제외가 바뀌면 다시 스캔, 단축키는 바로 등록/해제)
#[tauri::command]
fn search_set_settings(app: AppHandle, state: State<'_, AppState>, settings: SearchSettings) -> CmdResult<SearchView> {
    let mut settings = settings;
    settings.excludes.retain(|e| !e.trim().is_empty());
    settings.hotkey = settings.hotkey.trim().to_string();
    if settings.hotkey_enabled {
        settings.hotkey.parse::<Shortcut>().map_err(|e| format!("단축키를 알 수 없습니다({}): {e}", settings.hotkey))?;
    }
    let old = state.settings.lock().unwrap().search.clone();
    state.settings.lock().unwrap().search = settings.clone();
    state.save()?;
    if old.roots != settings.roots || old.excludes != settings.excludes {
        let covered = state.remote.covered(&settings.effective_roots());
        *state.covered.lock().unwrap() = covered.clone();
        state.search.reconfigure(search_config(&settings, &covered));
    }
    if old.hotkey_enabled != settings.hotkey_enabled || old.hotkey != settings.hotkey {
        apply_hotkey(&app, &settings)?;
    }
    Ok(search_status(state))
}

#[tauri::command]
fn search_rescan(state: State<'_, AppState>) {
    state.search.rescan_all();
}

/// 결과 열기(기본 프로그램)
#[tauri::command]
fn open_path(path: String) -> CmdResult<()> {
    if !Path::new(&path).exists() {
        return Err(format!("파일이 없습니다(옮겨졌거나 지워짐): {path}"));
    }
    #[cfg(windows)]
    {
        std::process::Command::new("explorer").arg(&path).spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open").arg(&path).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 들어 있는 폴더를 열고 그 파일을 선택
#[tauri::command]
fn open_folder(path: String) -> CmdResult<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if Path::new(&path).exists() {
            // explorer는 /select,"경로" 형식만 받는다(따옴표가 인자 안에 있어야 함)
            std::process::Command::new("explorer").raw_arg(format!("/select,\"{path}\"")).spawn().map_err(|e| e.to_string())?;
        } else if let Some(p) = Path::new(&path).parent().filter(|p| p.exists()) {
            std::process::Command::new("explorer").arg(p).spawn().map_err(|e| e.to_string())?;
        } else {
            return Err(format!("폴더가 없습니다: {path}"));
        }
    }
    #[cfg(not(windows))]
    {
        let p = Path::new(&path).parent().map(|p| p.to_path_buf()).unwrap_or_default();
        std::process::Command::new("xdg-open").arg(p).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// ux3: 폴더 자체를 연다(여러 파일을 만든 작업의 [폴더 열기])
#[tauri::command]
fn open_dir(path: String) -> CmdResult<()> {
    if !Path::new(&path).is_dir() {
        return Err(format!("폴더가 없습니다(옮겨졌거나 지워짐): {path}"));
    }
    #[cfg(windows)]
    {
        std::process::Command::new("explorer").arg(&path).spawn().map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open").arg(&path).spawn().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// ux3: 검색 결과 아이콘(확장자마다 한 번, 캐시). 키: 소문자 확장자, "" = 확장자 없음, "/dir" = 폴더
#[tauri::command]
async fn file_icons(keys: Vec<String>) -> CmdResult<HashMap<String, Option<String>>> {
    tauri::async_runtime::spawn_blocking(move || icons::icons(keys)).await.map_err(|e| e.to_string())
}

/// ux3: 기억한 화면 상태(탭·정렬·열 너비)
#[tauri::command]
fn ui_get(state: State<'_, AppState>) -> UiState {
    state.settings.lock().unwrap().ui.clone()
}

#[derive(serde::Deserialize)]
struct UiPatch {
    tab: Option<String>,
    sort_key: Option<String>,
    sort_desc: Option<bool>,
    col_widths: Option<Vec<u32>>,
}

/// ux3: 화면 상태 일부를 바꿔 저장(창 위치·크기는 창 이벤트에서)
#[tauri::command]
fn ui_set(patch: UiPatch, state: State<'_, AppState>) -> CmdResult<()> {
    {
        let mut s = state.settings.lock().unwrap();
        if let Some(t) = patch.tab {
            s.ui.tab = Some(t);
        }
        if let Some(k) = patch.sort_key {
            s.ui.sort_key = Some(k);
        }
        if let Some(d) = patch.sort_desc {
            s.ui.sort_desc = Some(d);
        }
        if let Some(c) = patch.col_widths {
            s.ui.col_widths = c.into_iter().take(8).map(|w| w.clamp(40, 2000)).collect();
        }
    }
    state.save()
}

/// ux3: 창 위치·크기를 기억(보이고 최소화가 아닐 때만, 최대화면 표시만 바꾸고 보통 크기는 그대로)
fn record_geometry(w: &tauri::Window) {
    let Some(state) = w.app_handle().try_state::<AppState>() else { return };
    if !w.is_visible().unwrap_or(false) || w.is_minimized().unwrap_or(false) {
        return;
    }
    let maximized = w.is_maximized().unwrap_or(false);
    let mut s = state.settings.lock().unwrap();
    let geom = if maximized {
        s.ui.window.map(|g| WindowGeom { maximized: true, ..g })
    } else {
        match (w.outer_position(), w.inner_size()) {
            (Ok(p), Ok(sz)) if sz.width > 0 && sz.height > 0 => {
                Some(WindowGeom { x: p.x, y: p.y, width: sz.width, height: sz.height, maximized: false })
            }
            _ => None,
        }
    };
    if geom.is_some() && geom != s.ui.window {
        s.ui.window = geom;
        drop(s);
        *state.ui_dirty.lock().unwrap() = Some(Instant::now());
    }
}

/// 창을 옮기거나 크기를 바꾸는 동안은 쓰지 않고, 0.8초 조용해지면 설정 파일에 저장
fn start_ui_saver(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(400));
        let state = app.state::<AppState>();
        let due = {
            let mut d = state.ui_dirty.lock().unwrap();
            match *d {
                Some(t) if t.elapsed() >= Duration::from_millis(800) => {
                    *d = None;
                    true
                }
                _ => false,
            }
        };
        if due {
            let _ = state.save();
        }
    });
}

/// 기억한 창 위치·크기를 지금 연결된 모니터 안으로 맞춰 적용(창이 숨은 채일 때, 보이기 전에)
fn restore_geometry(app: &tauri::App, g: WindowGeom) {
    let Some(w) = app.get_webview_window("main") else { return };
    let primary = w.primary_monitor().ok().flatten().map(|m| *m.position());
    let mut mons: Vec<(bool, MonitorRect)> = w
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let r = m.work_area();
            (Some(*m.position()) == primary, MonitorRect { x: r.position.x, y: r.position.y, width: r.size.width, height: r.size.height })
        })
        .collect();
    mons.sort_by_key(|(p, _)| !*p); // 주 모니터를 맨 앞(겹치는 곳이 없을 때 여기로)
    let mons: Vec<MonitorRect> = mons.into_iter().map(|(_, m)| m).collect();
    let g = g.clamp_to(&mons);
    let _ = w.set_size(tauri::PhysicalSize::new(g.width, g.height));
    let _ = w.set_position(tauri::PhysicalPosition::new(g.x, g.y));
    if g.maximized {
        let _ = w.maximize();
    }
}

/// 시작할 때 검색 칸에 넣을 질의(`--search=…`, 화면 점검용)
#[tauri::command]
fn startup_query() -> Option<String> {
    own_args().find_map(|a| a.strip_prefix("--search=").map(str::to_string))
}

/// 검색 탭을 앞으로(트레이·단축키)
fn show_search(app: &AppHandle) {
    show_main(app);
    let _ = app.emit("goto-tab", "search");
}

fn apply_hotkey(app: &AppHandle, s: &SearchSettings) -> CmdResult<()> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    if s.hotkey_enabled && !s.hotkey.is_empty() {
        let sc: Shortcut = s.hotkey.parse().map_err(|e| format!("단축키를 알 수 없습니다({}): {e}", s.hotkey))?;
        gs.on_shortcut(sc, |app, _sc, ev| {
            if ev.state == ShortcutState::Pressed {
                show_search(app);
            }
        })
        .map_err(|e| format!("단축키를 등록하지 못했습니다(다른 프로그램이 쓰는 중일 수 있음): {e}"))?;
    }
    Ok(())
}

// ---------- 1c: 창·트레이 ----------

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn update_tray_tooltip(app: &AppHandle) {
    let state = app.state::<AppState>();
    let n = state.settings.lock().unwrap().watch_rules.iter().filter(|r| r.enabled).count();
    let paused = state.watch_paused.load(Ordering::SeqCst);
    let tip = if n == 0 {
        "파일 도구".to_string()
    } else if paused {
        format!("파일 도구 — 감시 일시정지 (규칙 {n}개)")
    } else {
        format!("파일 도구 — 감시 중 (규칙 {n}개)")
    };
    if let Some(t) = app.tray_by_id("main") {
        let _ = t.set_tooltip(Some(tip));
    }
}

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "열기", true, None::<&str>)?;
    let search = MenuItem::with_id(app, "search", "검색 열기", true, None::<&str>)?;
    let pause = CheckMenuItem::with_id(app, "pause", "감시 일시정지", true, false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "끝내기", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &search, &pause, &quit])?;
    *app.state::<AppState>().pause_item.lock().unwrap() = Some(pause);
    let mut b = TrayIconBuilder::with_id("main")
        .tooltip("파일 도구")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id.as_ref() {
            "open" => show_main(app),
            "search" => show_search(app),
            "pause" => {
                let now = !app.state::<AppState>().watch_paused.load(Ordering::SeqCst);
                set_paused(app, now);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        b = b.icon(icon.clone());
    }
    b.build(app)?;
    Ok(())
}

/// 이번 실행이 윈도 시작 시 자동 실행인지(`--autostart`)
#[tauri::command]
fn launched_by_autostart() -> bool {
    autostart::is_autostart_launch(std::env::args())
}

/// 시작 탭(`--tab=image` 같은 인자, 화면 점검용)
#[tauri::command]
fn startup_tab() -> Option<String> {
    own_args().find_map(|a| a.strip_prefix("--tab=").map(str::to_string))
}

/// 앱이 열려 있는 동안 2초마다 켜진 규칙을 살핀다.
fn start_watch_thread(app: AppHandle) {
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let mut watcher = Watcher::new(state.config_dir.join("watch-state.json"), Duration::from_secs(3));
        let mut known: Vec<String> = Vec::new();
        // 다른 창(프로세스)이 이미 감시 중이면 잠금이 풀릴 때까지 이 창은 감시하지 않는다
        let lock_path = state.config_dir.join("watch.lock");
        let mut lock: Option<filetool_core::watch::WatchLock> = None;
        loop {
            if lock.is_none() {
                lock = filetool_core::watch::WatchLock::try_acquire(&lock_path);
                if lock.is_none() {
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
                // 잠금을 얻은 시점의 기록을 다시 읽는다(다른 창이 처리한 것 반영)
                watcher = Watcher::new(state.config_dir.join("watch-state.json"), Duration::from_secs(3));
            }
            let rules = state.settings.lock().unwrap().watch_rules.clone();
            // 지운 규칙의 기록은 버린다
            for id in known.iter().filter(|id| !rules.iter().any(|r| &r.id == *id)) {
                watcher.forget_rule(id);
            }
            known = rules.iter().map(|r| r.id.clone()).collect();
            if rules.iter().any(|r| r.enabled) && !state.watch_paused.load(Ordering::SeqCst) {
                let ts = state.toolset();
                for ev in watcher.run_once(&rules, &ts, None, Instant::now()) {
                    let mut log = state.watch_log.lock().unwrap();
                    log.push_back(ev.clone());
                    while log.len() > 200 {
                        log.pop_front();
                    }
                    drop(log);
                    let _ = app.emit("watch-event", ev);
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

/// 앱 자신의 옵션 인자: `--`(우클릭 메뉴가 넘긴 파일 경로의 시작) 앞까지만. 경로를 옵션으로 읽지 않는다.
fn own_args() -> impl Iterator<Item = String> {
    std::env::args().take_while(|a| a != "--")
}

/// 0.3.2 설치 파일이 부르는 `--ctx-menu=install|remove`: 창 없이 우클릭 메뉴만 등록·제거하고 끝낸다.
/// install은 사용자가 [설정]에서 끈 적이 있으면(`context_menu_off`) 다시 켜지 않는다.
fn ctx_menu_cli() -> Option<i32> {
    let mode = own_args().find_map(|a| a.strip_prefix("--ctx-menu=").map(str::to_string))?;
    let exe = std::env::current_exe().ok()?;
    let code = match mode.as_str() {
        "install" => {
            let default_dir = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("kr.salarycrew.winfiletool");
            let (dir, _) = filetool_core::portable::config_dir(exe.parent(), default_dir);
            if Settings::load(&dir).context_menu_off {
                0
            } else {
                filetool_core::ctxmenu::set(true, &exe).map(|_| 0).unwrap_or(1)
            }
        }
        "remove" => filetool_core::ctxmenu::set(false, &exe).map(|_| 0).unwrap_or(1),
        _ => 2,
    };
    Some(code)
}

/// 우클릭으로 여러 파일을 골랐을 때 탐색기는 파일마다 이 exe를 실행한다. 이미 앱이 떠 있으면
/// Tauri를 띄우지 않고 single-instance 플러그인과 같은 메시지(WM_COPYDATA, cwd·인자를 NUL로 구분)로 인자만 넘기고 바로 끝낸다.
#[cfg(windows)]
fn forward_ctx_fast() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId, SendMessageTimeoutW, SMTO_NORMAL, WM_COPYDATA};
    #[repr(C)]
    struct CopyData {
        dw_data: usize,
        cb_data: u32,
        lp_data: *const u8,
    }
    if !own_args().any(|a| a.starts_with(filetool_core::ctxmenu::CTX_ARG)) {
        return false;
    }
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (class, title) = (wide("kr.salarycrew.winfiletool-sic"), wide("kr.salarycrew.winfiletool-siw"));
    unsafe {
        let hwnd = FindWindowW(class.as_ptr(), title.as_ptr());
        if hwnd.is_null() {
            return false;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != 0 {
            AllowSetForegroundWindow(pid);
        }
        let mut data = std::env::current_dir().unwrap_or_default().to_string_lossy().into_owned();
        for a in std::env::args_os() {
            data.push('\0');
            data.push_str(&a.to_string_lossy());
        }
        let cds = CopyData { dw_data: 1543, cb_data: data.len() as u32, lp_data: data.as_ptr() };
        SendMessageTimeoutW(hwnd, WM_COPYDATA, 0, &cds as *const _ as isize, SMTO_NORMAL, 10_000, std::ptr::null_mut()) != 0
    }
}
#[cfg(not(windows))]
fn forward_ctx_fast() -> bool {
    false
}

fn main() {
    // 0.3.3(보안 강화): DLL은 System32에서만 찾는다(휴대용 exe 옆·다운로드 폴더의 DLL을 불러오지 않게) — 맨 먼저
    filetool_core::dllsafe::harden();
    let started = Instant::now();
    // 0.3.3: 시험용 바꾸기(한글·Office 숨기기, 문서 도구 주소)는 개발 빌드·시험용 exe(`test-hooks`)에서만 읽는다
    #[cfg(any(debug_assertions, feature = "test-hooks"))]
    filetool_core::testhooks::enable();
    if let Some(code) = ctx_menu_cli() {
        std::process::exit(code);
    }
    if forward_ctx_fast() {
        return;
    }
    // 0.3.0: WebView2가 없으면 창을 만들다 꺼지므로 먼저 확인하고, 없으면 한국어 안내 후 끝낸다
    if !cmd_runtime::ensure_webview2() {
        std::process::exit(2);
    }
    cmd_core::mark("main", started);
    let ctx_queue = cmd_core::CtxQueue::default();
    ctx_queue.push_args(std::env::args());
    tauri::Builder::default()
        // 한 번만 실행: 두 번째 실행은 기존 창을 앞으로(자동 실행 인자로 온 것은 조용히 무시).
        // 0.3.2 우클릭 메뉴로 온 것은 파일을 모아 두었다가 한 묶음으로 화면에 넘긴다.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if cmd_core::on_second_instance(app, &argv) {
                show_main(app);
            } else if !autostart::is_autostart_launch(&argv) {
                show_main(app);
            }
        }))
        .manage(ctx_queue)
        .manage(cmd_core::StartTime(started))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .on_window_event(|window, event| {
            if window.label() == "main" && matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_)) {
                record_geometry(window);
            }
            // 감시 규칙이 켜져 있으면 창을 닫아도 트레이에 남아 감시를 계속한다
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" && window.app_handle().state::<AppState>().keep_in_tray() {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(move |app| {
            cmd_core::mark("setup", started);
            // 설정 폴더: WIN_FILETOOL_CONFIG_DIR(시험용 앱만) → exe 옆 win-filetool-data\(휴대용, 0.3.0) → %APPDATA%
            let default_dir = app.path().app_config_dir().unwrap_or_else(|_| std::env::temp_dir().join("win-filetool"));
            let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
            let (config_dir, config_source) = filetool_core::portable::config_dir(exe_dir.as_deref(), default_dir);
            app.manage(cmd_runtime::RuntimeInfo { config_dir: config_dir.clone(), config_source });
            cmd_runtime::install_progress_events(app.handle());
            let settings = Settings::load(&config_dir);
            // 2c: 빠른 검색 서비스가 답하면 그 서비스가 맡은 드라이브는 앱 색인에서 뺀다(최대 1.5초만 기다림)
            let remote = Arc::new(Remote::default());
            let covered = {
                let (tx, rx) = std::sync::mpsc::channel();
                let r2 = remote.clone();
                let roots = settings.search.effective_roots();
                std::thread::spawn(move || {
                    r2.poll();
                    let _ = tx.send(r2.covered(&roots));
                });
                rx.recv_timeout(Duration::from_millis(1500)).unwrap_or_default()
            };
            cmd_core::mark("service-poll", started);
            // 검색 서비스: 저장한 색인을 불러오고 배경에서 다시 스캔, 실시간 갱신(창이 없어도 트레이에서 계속)
            let search = SearchService::start(search_config(&settings.search, &covered));
            let search_settings = settings.search.clone();
            let saved_geom = settings.ui.window;
            app.manage(AppState {
                config_dir,
                settings: Mutex::new(settings),
                jobs: Mutex::new(HashMap::new()),
                watch_log: Mutex::new(VecDeque::new()),
                watch_paused: AtomicBool::new(false),
                pause_item: Mutex::new(None),
                search,
                results: Mutex::new(Results::Local { gen: 0, ids: Vec::new() }),
                remote,
                covered: Mutex::new(covered),
                ui_dirty: Mutex::new(None),
            });
            if let Some(g) = saved_geom {
                restore_geometry(app, g);
            }
            start_ui_saver(app.handle().clone());
            start_service_monitor(app.handle().clone());
            if let Err(e) = apply_hotkey(app.handle(), &search_settings) {
                eprintln!("{e}");
            }
            build_tray(app)?;
            update_tray_tooltip(app.handle());
            start_watch_thread(app.handle().clone());
            // 0.3.3(보안 강화): 지난번 한글 변환이 강제 종료돼 남은 HKCU 보안 모듈 값을 지운다(지금 변환 중이 아니면)
            std::thread::spawn(|| {
                filetool_core::hwp::hancom::cleanup_stale_module();
            });
            // 창은 숨긴 채로 만들어진다(tauri.conf.json visible=false). 자동 실행이 아니면 바로 보인다.
            if !autostart::is_autostart_launch(std::env::args()) {
                show_main(app.handle());
            }
            cmd_core::mark("window-shown", started);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            cmd_doctools::doctools_status,
            cmd_doctools::doctools_needed,
            cmd_doctools::doctools_download,
            cmd_doctools::doctools_install_file,
            cmd_doctools::doctools_remove,
            inspect_paths,
            nfc_preview,
            nfc_apply,
            pdf_merge,
            pdf_split,
            pdf_rotate,
            pdf_delete,
            pdf_reorder,
            tools_status,
            set_tool_path,
            convert_one,
            cancel_jobs,
            watch_get,
            watch_set_rules,
            watch_set_paused,
            pdf_compress,
            images_to_pdf,
            pdf_to_images,
            pdf_extract_images,
            pdfium_status,
            cmd_office::doc_to_pptx,
            cmd_office::pdf_to_docx,
            cmd_office::pdf_to_pptx,
            cmd_office::pdf_to_txt,
            cmd_office::extract_images,
            cmd_hwp::hwp_capabilities,
            autostart_get,
            autostart_set,
            launched_by_autostart,
            startup_tab,
            search_query,
            search_rows,
            search_status,
            search_set_settings,
            search_rescan,
            svc_view,
            svc_setup,
            open_path,
            open_folder,
            open_dir,
            file_icons,
            ui_get,
            ui_set,
            startup_query,
            cmd_runtime::app_info,
            cmd_runtime::tools_bundle_status,
            cmd_runtime::tools_prepare,
            // 0.3.0 PDF 편집
            cmd_pdfedit::pdf_pages,
            cmd_pdfedit::pdf_thumbs,
            cmd_pdfedit::pdf_apply_plan,
            cmd_pdfedit::pdf_extract,
            cmd_pdfedit::pdf_insert_blank,
            cmd_pdfedit::pdf_insert_pdf,
            cmd_pdfedit::pdf_page_numbers,
            cmd_pdfedit::pdf_watermark,
            cmd_pdfedit::pdf_place_image,
            cmd_pdfedit::pdf_text_boxes,
            cmd_pdfedit::pdf_crop,
            cmd_pdfedit::pdf_fit_a4,
            cmd_pdfedit::pdf_nup,
            cmd_pdfedit::pdf_protect,
            cmd_pdfedit::pdf_unprotect,
            cmd_pdfedit::pdf_protection,
            cmd_pdfedit::pdf_info_get,
            cmd_pdfedit::pdf_info_set,
            cmd_pdfedit::pdf_info_clear,
            cmd_pdfedit::pdf_form_list,
            cmd_pdfedit::pdf_form_fill,
            cmd_pdfedit::pdf_flatten,
            // 0.3.0 W4 이미지·영상·파일 정리
            cmd_tools::cancel_job,
            cmd_tools::image_probe,
            cmd_tools::image_edit,
            cmd_tools::media_probe,
            cmd_tools::media_frame,
            cmd_tools::media_frame_save,
            cmd_tools::media_edit,
            cmd_tools::zip_list,
            cmd_tools::zip_extract,
            cmd_tools::zip_create,
            cmd_tools::rename_preview,
            cmd_tools::rename_apply,
            cmd_tools::rename_undo,
            cmd_tools::dup_scan,
            cmd_tools::dup_trash,
            // 0.3.0 화면(W5) 읽기 전용 도우미
            cmd_ui::file_stats,
            cmd_ui::list_files,
            cmd_ui::image_data_url,
            // 0.3.2 핵심 6개·우클릭 메뉴
            cmd_core::pdf_compress_target,
            cmd_core::pdf_merge_plan,
            cmd_core::photo_shrink,
            cmd_core::nfc_fix_paths,
            cmd_core::ctx_menu_get,
            cmd_core::ctx_menu_set,
            cmd_core::ctx_take,
            cmd_core::ui_ready,
            // 0.3.1 새 버전 알림
            cmd_update::update_view,
            cmd_update::update_check,
            cmd_update::update_set_enabled,
            cmd_update::update_skip,
            cmd_update::update_open
        ])
        .build(tauri::generate_context!())
        .expect("파일 도구를 시작하지 못했습니다")
        .run(|app, ev| {
            // 끝낼 때 검색 색인 저장(다음 시작이 빠르게)
            if let RunEvent::Exit = ev {
                let state = app.state::<AppState>();
                if state.ui_dirty.lock().unwrap().take().is_some() {
                    let _ = state.save();
                }
                let _ = state.search.save_now();
                state.search.stop();
            }
        });
}
