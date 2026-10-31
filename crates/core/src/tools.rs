//! 외부 도구 찾기: ffmpeg/ffprobe(영상·소리·HEIC), LibreOffice soffice(문서).
//!
//! 찾는 순서: 사용자가 [도구 설정]에서 지정한 경로 → PATH → 흔한 설치 위치
//! (Program Files, winget 패키지 폴더·Links, scoop/chocolatey, LibreOffice Portable,
//! 앱 옆 `tools\*` 폴더). 1b는 자동 설치하지 않고, 없으면 내려받을 곳을 안내한다.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Ffmpeg,
    Ffprobe,
    Soffice,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::Ffmpeg, Tool::Ffprobe, Tool::Soffice];

    pub fn stem(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "ffmpeg",
            Tool::Ffprobe => "ffprobe",
            Tool::Soffice => "soffice",
        }
    }

    /// 이 플랫폼의 실행 파일 이름 (Windows는 `.exe`).
    pub fn exe_name(self) -> String {
        format!("{}{}", self.stem(), std::env::consts::EXE_SUFFIX)
    }

    pub fn label(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "ffmpeg (영상·소리·HEIC 변환)",
            Tool::Ffprobe => "ffprobe (영상 길이 확인, ffmpeg에 함께 들어 있음)",
            Tool::Soffice => "LibreOffice (문서 → PDF, Pages·Numbers·Keynote)",
        }
    }

    pub fn download_url(self) -> &'static str {
        match self {
            Tool::Ffmpeg | Tool::Ffprobe => "https://www.gyan.dev/ffmpeg/builds/",
            Tool::Soffice => "https://ko.libreoffice.org/download/download/",
        }
    }

    /// 없을 때 보여 줄 한국어 안내.
    pub fn guidance(self) -> &'static str {
        match self {
            Tool::Ffmpeg | Tool::Ffprobe => {
                "ffmpeg가 없습니다. 내려받기 페이지에서 'ffmpeg-release-essentials.zip'을 받아 압축을 푼 뒤, \
                 그 안의 bin 폴더(ffmpeg.exe가 있는 곳)를 [경로 지정]으로 고르세요. \
                 명령 프롬프트에서 'winget install Gyan.FFmpeg'로 설치해도 됩니다(관리자 권한 불필요)."
            }
            Tool::Soffice => {
                "LibreOffice가 없습니다. 내려받기 페이지에서 Windows용 설치 파일을 받아 설치하거나, \
                 이미 설치했다면 soffice.exe가 있는 'program' 폴더를 [경로 지정]으로 고르세요. \
                 관리자 권한이 없으면 설치 파일을 폴더에 풀어서 쓸 수 있습니다: \
                 msiexec /a LibreOffice_x.y.z_Win_x86-64.msi /qn TARGETDIR=%USERPROFILE%\\LibreOffice"
            }
        }
    }
}

/// 찾을 위치 묶음. 테스트에서는 가짜 폴더로 채운다.
#[derive(Debug, Clone, Default)]
pub struct SearchEnv {
    /// PATH 폴더들
    pub path_dirs: Vec<PathBuf>,
    /// Program Files, Program Files (x86) 등
    pub program_files: Vec<PathBuf>,
    /// %LOCALAPPDATA%
    pub local_appdata: Option<PathBuf>,
    /// %USERPROFILE%
    pub user_profile: Option<PathBuf>,
    /// 앱 실행 파일이 있는 폴더
    pub exe_dir: Option<PathBuf>,
}

impl SearchEnv {
    pub fn from_system() -> Self {
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        let path_dirs = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
        let mut program_files = Vec::new();
        for k in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            if let Some(p) = var(k) {
                if !program_files.contains(&p) {
                    program_files.push(p);
                }
            }
        }
        if cfg!(windows) && program_files.is_empty() {
            program_files.push(PathBuf::from(r"C:\Program Files"));
        }
        Self {
            path_dirs,
            program_files,
            local_appdata: var("LOCALAPPDATA"),
            user_profile: var("USERPROFILE").or_else(|| var("HOME")),
            exe_dir: std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)),
        }
    }
}

/// 어디서 찾았는지(화면 표시용).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    UserSetting,
    Path,
    KnownLocation,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::UserSetting => "직접 지정한 경로",
            Source::Path => "PATH",
            Source::KnownLocation => "설치 폴더",
        }
    }
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|it| it.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// 흔한 설치 위치 후보 폴더(실행 파일이 들어 있을 폴더).
pub fn known_dirs(tool: Tool, env: &SearchEnv) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match tool {
        Tool::Ffmpeg | Tool::Ffprobe => {
            if let Some(la) = &env.local_appdata {
                dirs.push(la.join("Microsoft").join("WinGet").join("Links"));
                // winget: Packages\Gyan.FFmpeg_…\ffmpeg-8.0-full_build\bin\ffmpeg.exe
                let pk = la.join("Microsoft").join("WinGet").join("Packages");
                for p in subdirs(&pk) {
                    let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                    if name.contains("ffmpeg") {
                        dirs.push(p.join("bin"));
                        for inner in subdirs(&p) {
                            dirs.push(inner.join("bin"));
                        }
                    }
                }
            }
            for pf in &env.program_files {
                dirs.push(pf.join("ffmpeg").join("bin"));
            }
            if cfg!(windows) {
                dirs.push(PathBuf::from(r"C:\ffmpeg\bin"));
                dirs.push(PathBuf::from(r"C:\ProgramData\chocolatey\bin"));
            }
            if let Some(up) = &env.user_profile {
                dirs.push(up.join("scoop").join("shims"));
                dirs.push(up.join("ffmpeg").join("bin"));
            }
        }
        Tool::Soffice => {
            for pf in &env.program_files {
                dirs.push(pf.join("LibreOffice").join("program"));
            }
            if let Some(up) = &env.user_profile {
                for base in [up.clone(), up.join("Desktop"), up.join("Documents"), up.join("Downloads"), up.join("PortableApps")] {
                    dirs.push(base.join("LibreOfficePortable").join("App").join("libreoffice").join("program"));
                }
                dirs.push(up.join("LibreOffice").join("program"));
                dirs.push(up.join("scoop").join("apps").join("libreoffice").join("current").join("LibreOffice").join("program"));
            }
        }
    }
    // 앱 옆 tools\<아무 이름>\(bin|program) — 휴대용 배치
    if let Some(ed) = &env.exe_dir {
        for base in [ed.join("tools"), ed.join("..").join("tools")] {
            for sub in subdirs(&base) {
                dirs.push(sub.join("bin"));
                dirs.push(sub.join("program"));
                dirs.push(sub.clone());
            }
        }
    }
    dirs
}

/// 사용자가 고른 경로(실행 파일이거나 폴더)에서 실행 파일을 찾는다.
pub fn resolve_user_path(tool: Tool, p: &Path) -> Option<PathBuf> {
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    if p.is_dir() {
        for d in [p.to_path_buf(), p.join("bin"), p.join("program"), p.join("App").join("libreoffice").join("program")] {
            let c = d.join(tool.exe_name());
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

/// 도구를 찾는다. `related`는 ffprobe를 찾을 때 먼저 볼 ffmpeg 경로(같은 폴더).
pub fn find(tool: Tool, user: Option<&Path>, related: Option<&Path>, env: &SearchEnv) -> Option<(PathBuf, Source)> {
    if let Some(u) = user {
        if let Some(p) = resolve_user_path(tool, u) {
            return Some((p, Source::UserSetting));
        }
    }
    if let Some(r) = related.and_then(Path::parent) {
        let c = r.join(tool.exe_name());
        if c.is_file() {
            return Some((c, Source::KnownLocation));
        }
    }
    for d in &env.path_dirs {
        let c = d.join(tool.exe_name());
        if c.is_file() {
            return Some((c, Source::Path));
        }
    }
    for d in known_dirs(tool, env) {
        let c = d.join(tool.exe_name());
        if c.is_file() {
            return Some((c, Source::KnownLocation));
        }
    }
    None
}

/// 실제로 실행해 버전 문자열을 얻는다(실행할 수 없으면 None).
pub fn probe_version(tool: Tool, exe: &Path) -> Option<String> {
    let (program, args): (PathBuf, Vec<OsString>) = match tool {
        Tool::Ffmpeg | Tool::Ffprobe => (exe.to_path_buf(), vec!["-hide_banner".into(), "-version".into()]),
        Tool::Soffice => (soffice_console(exe), vec!["--version".into()]),
    };
    let out = crate::process::run(&program, &args, Duration::from_secs(30), None, &mut |_| {}).ok()?;
    parse_version(tool, &out.stdout)
}

pub fn parse_version(tool: Tool, stdout: &str) -> Option<String> {
    let first = stdout.lines().find(|l| !l.trim().is_empty())?.trim();
    match tool {
        // "ffmpeg version N-125875-g5d4d3bdc61-20260731 Copyright …"
        Tool::Ffmpeg | Tool::Ffprobe => first.split_whitespace().skip_while(|w| *w != "version").nth(1).map(str::to_string),
        // "LibreOffice 26.2.6.2 8221e31…"
        Tool::Soffice => {
            let mut it = first.split_whitespace();
            let name = it.next()?;
            it.next().map(|v| format!("{name} {v}"))
        }
    }
}

/// Windows에서는 soffice.exe 옆의 soffice.com(콘솔용)이 표준 출력을 돌려준다.
pub fn soffice_console(exe: &Path) -> PathBuf {
    let com = exe.with_extension("com");
    if cfg!(windows) && com.is_file() {
        com
    } else {
        exe.to_path_buf()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolStatus {
    pub tool: Tool,
    pub label: &'static str,
    pub found: bool,
    pub path: Option<PathBuf>,
    pub source: Option<&'static str>,
    pub version: Option<String>,
    pub guidance: Option<&'static str>,
    pub download_url: &'static str,
}

/// 변환에 쓸 도구 경로 묶음.
#[derive(Debug, Clone, Default)]
pub struct Toolset {
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    pub soffice: Option<PathBuf>,
}

/// 사용자 지정 경로(설정)를 받아 도구를 찾는다.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolPaths {
    #[serde(default)]
    pub ffmpeg: Option<PathBuf>,
    #[serde(default)]
    pub soffice: Option<PathBuf>,
}

impl Toolset {
    pub fn discover(user: &ToolPaths, env: &SearchEnv) -> Self {
        let ffmpeg = find(Tool::Ffmpeg, user.ffmpeg.as_deref(), None, env).map(|f| f.0);
        let ffprobe = find(Tool::Ffprobe, user.ffmpeg.as_deref(), ffmpeg.as_deref(), env).map(|f| f.0);
        let soffice = find(Tool::Soffice, user.soffice.as_deref(), None, env).map(|f| f.0);
        Self { ffmpeg, ffprobe, soffice }
    }

    pub fn get(&self, tool: Tool) -> crate::Result<&Path> {
        let p = match tool {
            Tool::Ffmpeg => &self.ffmpeg,
            Tool::Ffprobe => &self.ffprobe,
            Tool::Soffice => &self.soffice,
        };
        p.as_deref().ok_or_else(|| crate::Error::ToolMissing {
            tool: tool.stem().into(),
            hint: tool.guidance().into(),
        })
    }
}

/// [도구 설정] 화면용: 찾은 위치와 버전(실제 실행해서 확인).
pub fn status_all(user: &ToolPaths, env: &SearchEnv) -> Vec<ToolStatus> {
    let ffmpeg = find(Tool::Ffmpeg, user.ffmpeg.as_deref(), None, env);
    let ffprobe = find(Tool::Ffprobe, user.ffmpeg.as_deref(), ffmpeg.as_ref().map(|f| f.0.as_path()), env);
    let soffice = find(Tool::Soffice, user.soffice.as_deref(), None, env);
    [(Tool::Ffmpeg, ffmpeg), (Tool::Ffprobe, ffprobe), (Tool::Soffice, soffice)]
        .into_iter()
        .map(|(tool, f)| {
            let version = f.as_ref().and_then(|(p, _)| probe_version(tool, p));
            let found = f.is_some();
            ToolStatus {
                tool,
                label: tool.label(),
                found,
                source: f.as_ref().map(|(_, s)| s.label()),
                path: f.map(|(p, _)| p),
                version,
                guidance: if found { None } else { Some(tool.guidance()) },
                download_url: tool.download_url(),
            }
        })
        .collect()
}
