//! ux3: 감시 잠금은 다른 스레드가 자식 프로세스를 띄우는 중에도(fork~exec 사이 fd 복사)
//! 놓은 직후 다시 잡혀야 한다. 1b `watch_lock_single_holder`가 가끔 실패한 원인 재현.
use filetool_core::watch::WatchLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[test]
fn watch_lock_reacquire_while_spawning() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("watch.lock");
    let stop = Arc::new(AtomicBool::new(false));
    let mut spawners = Vec::new();
    for _ in 0..4 {
        let stop = stop.clone();
        spawners.push(std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let exe = if cfg!(windows) { "cmd" } else { "true" };
                let mut c = std::process::Command::new(exe);
                if cfg!(windows) {
                    c.args(["/c", "exit"]);
                }
                let _ = c.status();
            }
        }));
    }
    let mut fails = 0;
    for _ in 0..2000 {
        let a = WatchLock::try_acquire(&p).expect("잡음");
        assert!(WatchLock::try_acquire(&p).is_none(), "두 번째는 못 잡음");
        drop(a);
        match WatchLock::try_acquire(&p) {
            Some(b) => drop(b),
            None => fails += 1,
        }
    }
    stop.store(true, Ordering::Relaxed);
    for s in spawners {
        s.join().unwrap();
    }
    assert_eq!(fails, 0, "놓은 직후 다시 못 잡은 횟수");
}

/// 다른 프로세스는 잡혀 있는 동안 못 잡고, 놓으면 잡는다(Unix: python3의 POSIX 잠금으로 확인, 없으면 건너뜀).
#[cfg(unix)]
#[test]
fn watch_lock_excludes_other_process() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("watch.lock");
    let try_other = || -> Option<bool> {
        let code = "import fcntl,sys\nf=open(sys.argv[1],'a')\ntry:\n fcntl.lockf(f,fcntl.LOCK_EX|fcntl.LOCK_NB)\nexcept OSError:\n sys.exit(1)\n";
        let st = std::process::Command::new("python3").args(["-c", code]).arg(&p).status().ok()?;
        Some(st.success())
    };
    let a = WatchLock::try_acquire(&p).expect("잡음");
    let Some(other) = try_other() else {
        eprintln!("python3 없음 — 건너뜀");
        return;
    };
    assert!(!other, "다른 프로세스는 못 잡음");
    drop(a);
    assert_eq!(try_other(), Some(true), "놓은 뒤 다른 프로세스가 잡음");
}
