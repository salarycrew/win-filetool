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
use filetool_core::search::query::{Row, SortKey};
use filetool_core::search::service::{default_index_dir, SearchConfig, SearchResult, SearchService, SearchStatus};
use filetool_core::settings::SearchSettings;
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
struct ZipResult {
    extract: zipnfc::ExtractReport,
    rezip: Option<zipnfc::RezipReport>,
}

#[tauri::command]
async fn zip_extract(zip: String, out_dir: Option<String>, rezip: bool) -> CmdResult<ZipResult> {
    let dir = self::out_dir(&out_dir);
    blocking(move || {
        let zip = PathBuf::from(zip);
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
    })
    .await
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
    /// 마지막 검색 결과(번호 목록)와 그때의 색인 세대
    results: Mutex<(u64, Vec<u32>)>,
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

fn search_config(s: &SearchSettings) -> SearchConfig {
    SearchConfig::new(s.effective_roots(), s.excludes.clone(), &index_dir())
}

/// 색인 폴더(시험용 WIN_FILETOOL_INDEX_DIR, 기본 %LOCALAPPDATA%\kr.salarycrew.winfiletool\index)
fn index_dir() -> PathBuf {
    default_index_dir()
}

#[tauri::command]
async fn search_query(state: State<'_, AppState>, query: String, sort: SortKey, desc: bool) -> CmdResult<SearchResult> {
    let svc = state.search.clone();
    let (ids, r) = tauri::async_runtime::spawn_blocking(move || svc.query(&query, sort, desc))
        .await
        .map_err(|e| e.to_string())??;
    *state.results.lock().unwrap() = (r.generation, ids);
    Ok(r)
}

#[derive(Serialize)]
struct RowsOut {
    rows: Vec<Row>,
    /// 색인이 통째로 바뀌어(다시 스캔 반영) 다시 검색해야 함
    stale: bool,
}

/// 결과 줄 [offset, offset+limit) — 화면에 보이는 만큼만
#[tauri::command]
fn search_rows(state: State<'_, AppState>, offset: usize, limit: usize) -> RowsOut {
    let (gen, ids) = {
        let g = state.results.lock().unwrap();
        let end = (offset + limit).min(g.1.len());
        (g.0, g.1.get(offset.min(end)..end).map(|s| s.to_vec()).unwrap_or_default())
    };
    match state.search.rows(&ids, gen) {
        Some(rows) => RowsOut { rows, stale: false },
        None => RowsOut { rows: Vec::new(), stale: true },
    }
}

#[derive(Serialize)]
struct SearchView {
    status: SearchStatus,
    settings: SearchSettings,
    /// 지금 색인하는 루트(설정이 비어 있으면 고정 드라이브)
    effective_roots: Vec<PathBuf>,
}

#[tauri::command]
fn search_status(state: State<'_, AppState>) -> SearchView {
    let settings = state.settings.lock().unwrap().search.clone();
    SearchView { status: state.search.status(), effective_roots: settings.effective_roots(), settings }
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
        state.search.reconfigure(search_config(&settings));
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

/// 시작할 때 검색 칸에 넣을 질의(`--search=…`, 화면 점검용)
#[tauri::command]
fn startup_query() -> Option<String> {
    std::env::args().find_map(|a| a.strip_prefix("--search=").map(str::to_string))
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
    std::env::args().find_map(|a| a.strip_prefix("--tab=").map(str::to_string))
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

fn main() {
    tauri::Builder::default()
        // 한 번만 실행: 두 번째 실행은 기존 창을 앞으로(자동 실행 인자로 온 것은 조용히 무시)
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if !autostart::is_autostart_launch(&argv) {
                show_main(app);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .on_window_event(|window, event| {
            // 감시 규칙이 켜져 있으면 창을 닫아도 트레이에 남아 감시를 계속한다
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" && window.app_handle().state::<AppState>().keep_in_tray() {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            // 시험용: WIN_FILETOOL_CONFIG_DIR로 설정 폴더를 바꿀 수 있다(사용자 설정을 건드리지 않고 점검)
            let config_dir = std::env::var_os("WIN_FILETOOL_CONFIG_DIR")
                .map(PathBuf::from)
                .or_else(|| app.path().app_config_dir().ok())
                .unwrap_or_else(|| std::env::temp_dir().join("win-filetool"));
            let settings = Settings::load(&config_dir);
            // 검색 서비스: 저장한 색인을 불러오고 배경에서 다시 스캔, 실시간 갱신(창이 없어도 트레이에서 계속)
            let search = SearchService::start(search_config(&settings.search));
            let search_settings = settings.search.clone();
            app.manage(AppState {
                config_dir,
                settings: Mutex::new(settings),
                jobs: Mutex::new(HashMap::new()),
                watch_log: Mutex::new(VecDeque::new()),
                watch_paused: AtomicBool::new(false),
                pause_item: Mutex::new(None),
                search,
                results: Mutex::new((0, Vec::new())),
            });
            if let Err(e) = apply_hotkey(app.handle(), &search_settings) {
                eprintln!("{e}");
            }
            build_tray(app)?;
            update_tray_tooltip(app.handle());
            start_watch_thread(app.handle().clone());
            // 창은 숨긴 채로 만들어진다(tauri.conf.json visible=false). 자동 실행이 아니면 바로 보인다.
            if !autostart::is_autostart_launch(std::env::args()) {
                show_main(app.handle());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            inspect_paths,
            nfc_preview,
            nfc_apply,
            zip_extract,
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
            autostart_get,
            autostart_set,
            launched_by_autostart,
            startup_tab,
            search_query,
            search_rows,
            search_status,
            search_set_settings,
            search_rescan,
            open_path,
            open_folder,
            startup_query
        ])
        .build(tauri::generate_context!())
        .expect("파일 도구를 시작하지 못했습니다")
        .run(|app, ev| {
            // 끝낼 때 검색 색인 저장(다음 시작이 빠르게)
            if let RunEvent::Exit = ev {
                let state = app.state::<AppState>();
                let _ = state.search.save_now();
                state.search.stop();
            }
        });
}
