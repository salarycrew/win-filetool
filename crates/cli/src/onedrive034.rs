//! 0.3.4 점검: OneDrive가 가져간 폴더 진단·가져오기·되돌리기(앱 화면과 같은 코어 함수).
//! 진단은 읽기만 한다. 가짜 환경은 `WIN_FILETOOL_ONEDRIVE_FAKE=<json>`(CLI·시험용 앱만 읽음).

use std::path::PathBuf;
use std::process::ExitCode;

use filetool_core::onedrive::{self, Case, Env, Folder, RestoreOpts};
use filetool_core::process::Cancel;
use filetool_core::{Error, Result};

pub const USAGE: &str = "  0.3.4 OneDrive가 가져간 폴더:
  filetool-cli onedrive-check [--json]                                    바탕 화면·문서·사진(·다운로드) 위치와 경우(A~D) — 읽기만
  filetool-cli onedrive-restore --folder desktop|documents|pictures|downloads --dry-run|--apply [--download] [--log-dir 폴더] [--json]
               OneDrive 폴더에 남은 파일을 이 PC 폴더로 복사(덮어쓰지 않음, OneDrive 쪽은 그대로). --download: 클라우드에만 있는 파일도 받기
  filetool-cli onedrive-undo <번호> [--log-dir 폴더]                        그 가져오기로 복사한 파일 중 바뀌지 않은 것만 지우기";

fn flag(args: &[String], f: &str) -> bool {
    args.iter().any(|a| a == f)
}

fn val<'a>(args: &'a [String], f: &str) -> Option<&'a str> {
    args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn log_dir(args: &[String]) -> PathBuf {
    if let Some(d) = val(args, "--log-dir") {
        return PathBuf::from(d);
    }
    if let Some(d) = filetool_core::testhooks::var("WIN_FILETOOL_CONFIG_DIR") {
        return PathBuf::from(d).join("onedrive-restore");
    }
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join(filetool_core::bundle::APP_DIR).join("onedrive-restore")
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

pub fn run(cmd: &str, args: &[String]) -> Result<Option<ExitCode>> {
    match cmd {
        "onedrive-check" => {
            let env = Env::current()?;
            let r = onedrive::diagnose(&env);
            if flag(args, "--json") {
                println!("{}", json(&r));
                return Ok(Some(ExitCode::SUCCESS));
            }
            if r.fake {
                println!("(시험용 가짜 환경)");
            }
            println!("요약: {}", r.summary);
            println!("OneDrive 실행 중: {}", if r.onedrive_running { "예" } else { "아니요" });
            if r.accounts.is_empty() {
                println!("OneDrive 계정: 없음(이 PC에 설정되어 있지 않음)");
            }
            for a in &r.accounts {
                println!(
                    "OneDrive 계정: {} {} — 폴더 {}, 로그인 {}",
                    if a.business { "회사·학교" } else { "개인" },
                    a.root.display(),
                    if a.exists { "있음" } else { "없음" },
                    if a.signed_in { "됨" } else { "안 됨" }
                );
            }
            for f in &r.folders {
                println!("[{:?}] {} — {}", f.case, f.name, f.title);
                println!("     지금 위치: {} ({}, 파일 {}개 {}, 클라우드에만 {}개 {})", f.current.display(), if f.current_exists { "있음" } else { "없음" },
                    f.here.files, onedrive::human(f.here.bytes), f.here.cloud_files, onedrive::human(f.here.cloud_bytes));
                if let (Some(d), Some(c)) = (&f.onedrive_copy, &f.onedrive_count) {
                    println!("     OneDrive에 남은 것: {} (파일 {}개 {}, 클라우드에만 {}개 {})", d.display(), c.files, onedrive::human(c.bytes), c.cloud_files, onedrive::human(c.cloud_bytes));
                }
                match &f.local_default_count {
                    Some(c) => println!("     이 PC 기본 위치: {} (파일 {}개 {})", f.local_default.display(), c.files, onedrive::human(c.bytes)),
                    None => println!("     이 PC 기본 위치: {}{}", f.local_default.display(), if f.local_default_exists { " (지금 위치와 같음)" } else { " (없음)" }),
                }
                println!("     {}", f.detail);
                match f.case {
                    Case::A => println!("     할 일: OneDrive 백업 끄기 — {}", onedrive::BACKUP_OFF_STEPS),
                    Case::B => println!("     할 일: filetool-cli onedrive-restore --folder {} --dry-run (그다음 --apply)", f.key),
                    Case::C => println!("     할 일 없어요"),
                    Case::D => println!("     안내: {}", onedrive::RECYCLE_HELP),
                }
            }
            Ok(Some(ExitCode::SUCCESS))
        }
        "onedrive-restore" => {
            let key = val(args, "--folder").ok_or_else(|| Error::Invalid(format!("--folder가 필요합니다\n{USAGE}")))?;
            let folder = Folder::from_key(key).ok_or_else(|| Error::Invalid(format!("폴더를 알 수 없습니다: {key}")))?;
            let (dry, apply) = (flag(args, "--dry-run"), flag(args, "--apply"));
            if dry == apply {
                return Err(Error::Invalid("--dry-run 또는 --apply 중 하나를 주세요".into()));
            }
            let env = Env::current()?;
            let opts = RestoreOpts { allow_download: flag(args, "--download"), dry_run: dry };
            let t = std::time::Instant::now();
            let o = onedrive::restore(&env, folder, opts, &log_dir(args), &Cancel::new(), &mut |_, _, _| {})?;
            if flag(args, "--json") {
                println!("{}", json(&o));
                return Ok(Some(ExitCode::SUCCESS));
            }
            println!("{} → {}", o.from.display(), o.to.display());
            for p in &o.plan {
                println!("  {:6} {} -> {}{}", p.action, p.rel, p.dest.display(), p.reason.as_ref().map(|r| format!(" ({r})")).unwrap_or_default());
            }
            for s in &o.skipped {
                println!("  못 가져옴 {}: {}", s.path.display(), s.reason);
            }
            println!("결과: {} ({:.1}초)", o.summary, t.elapsed().as_secs_f64());
            if let Some(l) = &o.log {
                println!("기록: {} — 되돌리기: filetool-cli onedrive-undo {}", l.display(), o.id);
            }
            if !dry {
                println!("알림: {}", onedrive::DELETE_WARNING);
            }
            Ok(Some(if o.skipped.is_empty() && !o.cancelled { ExitCode::SUCCESS } else { ExitCode::from(1) }))
        }
        "onedrive-undo" => {
            let id = args.get(1).filter(|a| !a.starts_with("--")).ok_or_else(|| Error::Invalid(format!("되돌리기 번호가 필요합니다\n{USAGE}")))?;
            let env = Env::current()?;
            let u = onedrive::undo(&env, &log_dir(args), id)?;
            for s in &u.failed {
                println!("  못 지움 {}: {}", s.path.display(), s.reason);
            }
            println!("결과: {} (이미 없음 {}, 지운 빈 폴더 {})", u.summary, u.gone, u.dirs_removed);
            Ok(Some(ExitCode::SUCCESS))
        }
        _ => Ok(None),
    }
}
