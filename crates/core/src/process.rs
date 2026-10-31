//! 외부 도구(ffmpeg, LibreOffice) 실행. 셸을 거치지 않고 argv로만 실행하며,
//! 작업마다 시간 제한을 두고, 취소하면 자식 프로세스(와 그 자식)를 끝낸다.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

/// 여러 스레드에서 공유하는 취소 표시.
#[derive(Clone, Default, Debug)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

pub struct RunOutput {
    pub code: Option<i32>,
    pub stdout: String,
    /// stderr 마지막 부분(오류 메시지 표시용, 최대 약 8 KB)
    pub stderr_tail: String,
}

impl RunOutput {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// 콘솔 창이 깜빡이지 않게(GUI 앱에서 실행할 때) 하는 Windows 플래그.
pub fn command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 자식과 그 자식들까지 끝낸다(LibreOffice의 soffice.exe는 soffice.bin을 따로 띄운다).
pub fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let pid = child.id().to_string();
        let _ = command(Path::new("taskkill"))
            .args(["/PID", pid.as_str(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// `program args…`를 실행한다. stdout은 줄 단위로 `on_line`에 넘기고(ffmpeg `-progress pipe:1`),
/// 시간 제한·취소 시 프로세스를 끝내고 오류를 돌려준다.
pub fn run(
    program: &Path,
    args: &[OsString],
    timeout: Duration,
    cancel: Option<&Cancel>,
    on_line: &mut dyn FnMut(&str),
) -> Result<RunOutput> {
    let tool = program
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "도구".into());
    if let Some(c) = cancel {
        c.check()?;
    }
    let mut child = command(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Tool { tool: tool.clone(), message: format!("시작하지 못했습니다 ({}): {e}", program.display()) })?;

    let (tx, rx) = mpsc::channel::<String>();
    let stdout = child.stdout.take().expect("stdout piped");
    let out_thread = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut stderr = child.stderr.take().expect("stderr piped");
    let (etx, erx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut tail: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = stderr.read(&mut buf) {
            if n == 0 {
                break;
            }
            tail.extend_from_slice(&buf[..n]);
            if tail.len() > 16 * 1024 {
                tail.drain(..tail.len() - 8 * 1024);
            }
        }
        let _ = etx.send(String::from_utf8_lossy(&tail).into_owned());
    });

    let start = Instant::now();
    let mut stdout_all = String::new();
    let outcome: Result<Option<i32>> = loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                on_line(&line);
                if stdout_all.len() < 64 * 1024 {
                    stdout_all.push_str(&line);
                    stdout_all.push('\n');
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
        if let Some(c) = cancel {
            if c.is_cancelled() {
                kill_tree(&mut child);
                break Err(Error::Cancelled);
            }
        }
        if start.elapsed() > timeout {
            kill_tree(&mut child);
            break Err(Error::Timeout(timeout.as_secs()));
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status.code()),
            Ok(None) => {
                // 출력이 끊긴 뒤에는 바쁜 대기를 피한다
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                kill_tree(&mut child);
                break Err(Error::Tool { tool: tool.clone(), message: e.to_string() });
            }
        }
    };
    // 남은 stdout 줄 처리. 손자 프로세스가 파이프를 쥐고 있어도 멈추지 않도록 최대 2초만 기다린다.
    drop(out_thread);
    let drain_until = Instant::now() + Duration::from_secs(2);
    loop {
        let left = drain_until.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(line) => {
                on_line(&line);
                if stdout_all.len() < 64 * 1024 {
                    stdout_all.push_str(&line);
                    stdout_all.push('\n');
                }
            }
            Err(_) => break,
        }
    }
    let stderr_tail = erx
        .recv_timeout(drain_until.saturating_duration_since(Instant::now()).max(Duration::from_millis(200)))
        .unwrap_or_default();
    let code = outcome?;
    Ok(RunOutput { code, stdout: stdout_all, stderr_tail })
}

/// stderr 마지막 몇 줄(오류 표시용).
pub fn last_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" / ")
}
