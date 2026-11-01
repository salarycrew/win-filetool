//! 0.3.2 핵심 할 일 가운데 앱과 점검용 CLI가 같이 쓰는 묶음 처리.
//!
//! - [`photo_shrink`]: 사진 여러 장 → JPG(긴 변 제한). 원본이 JPG이고 결과가 더 크면 새 파일을 지우고 "그대로".
//!   4장씩 동시에, 하나가 실패해도 계속. 멈추면 이번에 만든 파일을 지운다.
//! - [`nfc_fix_path`]: 고른 파일·폴더 이름 자체를 NFC로(맥에서 온 자모 분리). 같은 이름이 있으면 건너뜀.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use serde::Serialize;

use crate::convert::{self, Op};
use crate::error::{Error, Result};
use crate::imageconv::{ImageOpts, ImgFormat};
use crate::process::Cancel;
use crate::tools::Toolset;

#[derive(Debug, Clone, Serialize)]
pub struct PhotoItem {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub before: u64,
    pub after: u64,
    pub note: Option<String>,
    pub error: Option<String>,
    /// 원본보다 커져서 새 파일을 만들지 않음
    pub unchanged: bool,
}

fn len(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
}

pub fn photo_opts(max_side: Option<u32>, quality: Option<u8>) -> ImageOpts {
    ImageOpts { format: ImgFormat::Jpg, quality: quality.unwrap_or(85).clamp(40, 95), max_side: max_side.filter(|m| *m >= 320), keep_exif: true }
}

fn shrink_one(ts: &Toolset, src: &Path, opts: &ImageOpts, out_dir: Option<&Path>, cancel: &Cancel) -> PhotoItem {
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

/// 사진 여러 장 줄이기. `progress(끝낸 수, 전체, 방금 끝낸 파일)`. 멈추면 `Error::Cancelled`(만든 파일은 지움).
pub fn photo_shrink(
    ts: &Toolset,
    paths: &[PathBuf],
    opts: &ImageOpts,
    out_dir: Option<&Path>,
    cancel: &Cancel,
    progress: &mut dyn FnMut(usize, usize, &Path),
) -> Result<Vec<PhotoItem>> {
    let n = paths.len();
    let results: Vec<Mutex<Option<PhotoItem>>> = (0..n).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map(|v| v.get()).unwrap_or(2).clamp(1, 4).min(n.max(1));
    let (tx, rx) = std::sync::mpsc::channel::<usize>();
    std::thread::scope(|s| {
        for _ in 0..workers {
            let tx = tx.clone();
            let (results, next, done) = (&results, &next, &done);
            s.spawn(move || loop {
                if cancel.is_cancelled() {
                    break;
                }
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= n {
                    break;
                }
                let it = shrink_one(ts, &paths[i], opts, out_dir, cancel);
                *results[i].lock().unwrap() = Some(it);
                done.fetch_add(1, Ordering::SeqCst);
                let _ = tx.send(i);
            });
        }
        drop(tx);
        for i in rx {
            progress(done.load(Ordering::SeqCst), n, &paths[i]);
        }
    });
    if cancel.is_cancelled() {
        for r in &results {
            if let Some(PhotoItem { output: Some(o), .. }) = r.lock().unwrap().as_ref() {
                let _ = std::fs::remove_file(o);
            }
        }
        return Err(Error::Cancelled);
    }
    Ok(results.into_iter().filter_map(|m| m.into_inner().unwrap()).collect())
}

#[derive(Debug, Clone, Serialize)]
pub struct NfcFix {
    pub from: PathBuf,
    pub to: Option<PathBuf>,
    pub note: Option<String>,
}

/// 이름 자체를 NFC로(바꿀 것이 없으면 `to = None, note = None`)
pub fn nfc_fix_path(from: &Path) -> NfcFix {
    let from = from.to_path_buf();
    let Some(name) = from.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return NfcFix { from, to: None, note: Some("이름이 없어요".into()) };
    };
    let fixed = crate::nfc::to_nfc(&name);
    if fixed == name {
        return NfcFix { from, to: None, note: None };
    }
    let to = from.with_file_name(&fixed);
    // NTFS는 NFD·NFC 이름을 다른 이름으로 본다: 바른 이름이 이미 있으면 덮어쓰지 않고 건너뜀
    if to.exists() {
        return NfcFix { from, to: None, note: Some(format!("같은 이름({fixed})이 이미 있어 건너뛰었어요")) };
    }
    match std::fs::rename(&from, &to) {
        Ok(()) => NfcFix { from, to: Some(to), note: None },
        Err(e) => NfcFix { from, to: None, note: Some(format!("바꾸지 못했어요: {e}")) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfc_fix_renames_and_skips_collisions() {
        let t = tempfile::tempdir().unwrap();
        let nfd = crate::samples::nfd("한글 보고서.txt");
        let p = t.path().join(&nfd);
        std::fs::write(&p, b"x").unwrap();
        let r = nfc_fix_path(&p);
        let to = r.to.expect("바뀜");
        assert_eq!(to.file_name().unwrap().to_string_lossy(), "한글 보고서.txt");
        assert!(to.exists());
        // 이미 바른 이름은 그대로
        let r2 = nfc_fix_path(&to);
        assert!(r2.to.is_none() && r2.note.is_none());
        // 같은 이름이 있으면 건너뜀(Linux는 NFD/NFC를 다른 이름으로 봄)
        let p2 = t.path().join(crate::samples::nfd("한글 보고서.txt"));
        std::fs::write(&p2, b"y").unwrap();
        let r3 = nfc_fix_path(&p2);
        assert!(r3.to.is_none());
        assert!(r3.note.unwrap().contains("이미 있어"));
        assert_eq!(std::fs::read(&to).unwrap(), b"x", "있던 파일은 그대로");
    }

    #[test]
    fn photo_shrink_batch_keeps_going_and_never_grows_jpg() {
        let t = tempfile::tempdir().unwrap();
        let big = t.path().join("큰 사진.png");
        crate::samples::photo_like(2400, 1600, 3).save(&big).unwrap();
        let small = t.path().join("작은.jpg");
        std::fs::write(&small, crate::samples::jpeg_bytes(&crate::samples::photo_like(200, 150, 1), 40).unwrap()).unwrap();
        let bad = t.path().join("깨진.jpg");
        std::fs::write(&bad, b"not a jpeg at all").unwrap();
        let ts = Toolset::default();
        let paths = vec![big.clone(), small.clone(), bad.clone()];
        let mut seen = 0;
        let out = photo_shrink(&ts, &paths, &photo_opts(Some(1280), None), None, &Cancel::new(), &mut |d, n, _| {
            seen = d;
            assert_eq!(n, 3);
        })
        .unwrap();
        assert_eq!(seen, 3);
        assert_eq!(out.len(), 3);
        let o = out[0].output.as_ref().unwrap();
        assert_eq!(o.extension().unwrap(), "jpg");
        assert!(out[0].after < out[0].before);
        let im = image::open(o).unwrap();
        assert_eq!((im.width(), im.height()), (1280, 853));
        assert!(out[1].unchanged && out[1].output.is_none(), "작은 JPG는 커지면 그대로");
        assert!(out[2].error.is_some(), "깨진 파일은 오류로, 나머지는 계속");
        // 멈추면 만든 파일을 남기지 않음
        let c = Cancel::new();
        c.cancel();
        assert!(matches!(photo_shrink(&ts, &paths, &photo_opts(None, None), Some(t.path()), &c, &mut |_, _, _| {}), Err(Error::Cancelled)));
    }
}
