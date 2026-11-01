//! 0.3.0 W4 점검: 사진 고치기·영상 고치기·zip 풀기/만들기·이름 한꺼번에 바꾸기·같은 파일 찾기.
//! 영상·HEIC는 ffmpeg(+libx265)가 PATH에 있을 때만(없으면 건너뛰었다고 출력).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use filetool_core::dupes::{self, ScanOpts};
use filetool_core::imgedit::{self, Crop, EditOps, OutFormat, Resize};
use filetool_core::mediaedit::{self, MediaOps, Trim};
use filetool_core::rename::{self, Numbering, Pair, Pos, Rule, SortBy};
use filetool_core::ziptool::{self, NameEnc};
use filetool_core::{exif, process, samples};

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|p| p.is_file()))
}

fn ff() -> Option<(PathBuf, PathBuf)> {
    Some((which("ffmpeg")?, which("ffprobe")?))
}

fn run(ffmpeg: &Path, args: &[&str]) {
    let a: Vec<OsString> = args.iter().map(OsString::from).collect();
    let o = process::run(ffmpeg, &a, Duration::from_secs(120), None, &mut |_| {}).unwrap();
    assert!(o.success(), "{}", o.stderr_tail);
}

fn jpeg_with(path: &Path, w: u32, h: u32, exif_block: Vec<u8>) {
    use image::ImageEncoder;
    let img = samples::quadrant_image(w, h);
    let mut buf = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 92);
    enc.set_exif_metadata(exif_block).unwrap();
    enc.write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8).unwrap();
    std::fs::write(path, buf).unwrap();
}

fn out_exif(p: &Path) -> Option<Vec<u8>> {
    exif::from_jpeg(&std::fs::read(p).unwrap())
}

// ---------- 사진 ----------

#[test]
fn image_resize_strip_gps_orientation() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("여행.jpg");
    // 저장 픽셀 400×300, EXIF 회전 6(시계 90°) → 보이는 크기 300×400
    jpeg_with(&src, 400, 300, exif::build_sample_gps("2024:05:01 10:20:30", 6, 37, 127));
    let p = imgedit::probe(&src).unwrap();
    assert_eq!((p.width, p.height, p.format.as_str(), p.has_gps), (300, 400, "jpg", true));
    assert_eq!(p.taken_at.as_deref(), Some("2024-05-01 10:20:30"));

    // 정보 지우기 + 가로 150
    let ops = EditOps { resize: Some(Resize { width: Some(150), keep_ratio: true, ..Default::default() }), strip_meta: true, ..Default::default() };
    let o = imgedit::edit(None, &src, &ops, None, None).unwrap();
    assert_eq!(o.output.file_name().unwrap(), "여행_편집.jpg");
    assert_eq!((o.width, o.height), (150, 200));
    let img = image::open(&o.output).unwrap();
    assert_eq!((img.width(), img.height()), (150, 200));
    // 회전이 픽셀에 적용됨: 시계 90° → 왼위=파랑, 오위=빨강
    assert_eq!(samples::quadrant_colors(&img), ["파랑", "빨강", "흰색", "초록"]);
    assert!(out_exif(&o.output).is_none(), "EXIF가 남음");
    let p2 = imgedit::probe(&o.output).unwrap();
    assert!(!p2.has_gps && p2.taken_at.is_none());
    assert!(std::fs::read(&src).unwrap().len() as u64 == o.before, "원본 그대로");

    // 정보 지우지 않으면 GPS·날짜는 남고 회전 값은 1
    let keep = EditOps { rotate: Some(180), ..Default::default() };
    let o = imgedit::edit(None, &src, &keep, None, None).unwrap();
    assert_eq!(o.output.file_name().unwrap(), "여행_편집 (1).jpg");
    let e = out_exif(&o.output).unwrap();
    assert!(exif::has_gps(&e));
    assert_eq!(exif::orientation(&e), Some(1));
    let img = image::open(&o.output).unwrap();
    assert_eq!((img.width(), img.height()), (300, 400));
    assert_eq!(samples::quadrant_colors(&img), ["초록", "흰색", "빨강", "파랑"]);
}

#[test]
fn image_strip_only_is_lossless() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("a.jpg");
    jpeg_with(&src, 64, 48, exif::build_sample_gps("2024:05:01 10:20:30", 1, 37, 127));
    let o = imgedit::edit(None, &src, &EditOps { strip_meta: true, ..Default::default() }, None, None).unwrap();
    assert!(o.note.unwrap().contains("화질이 그대로"));
    let (a, b) = (std::fs::read(&src).unwrap(), std::fs::read(&o.output).unwrap());
    assert!(out_exif(&o.output).is_none() && b.len() < a.len());
    // 압축 데이터(SOS 뒤)는 같은 바이트
    let sos = |v: &[u8]| v.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
    assert_eq!(&a[sos(&a)..], &b[sos(&b)..]);
    assert_eq!((o.width, o.height), (64, 48));
}

#[test]
fn image_png_alpha_crop_rotate_flip() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("로고.png");
    // 왼쪽 절반은 투명
    let img = image::RgbaImage::from_fn(200, 100, |x, _| if x < 100 { image::Rgba([0, 0, 0, 0]) } else { image::Rgba([255, 0, 0, 255]) });
    img.save(&src).unwrap();
    let ops = EditOps { crop: Some(Crop::Aspect { aspect: "1:1".into(), anchor: "center".into() }), rotate: Some(90), flip: Some("h".into()), ..Default::default() };
    let o = imgedit::edit(None, &src, &ops, None, None).unwrap();
    assert_eq!(o.output.extension().unwrap(), "png");
    let out = image::open(&o.output).unwrap().to_rgba8();
    assert_eq!(out.dimensions(), (100, 100));
    // 가운데 100×100: 왼쪽 반 투명, 오른쪽 반 빨강 → 시계 90°: 위 투명 / 아래 빨강 → 좌우 뒤집기는 그대로
    assert_eq!(out.get_pixel(50, 10)[3], 0, "투명 유지");
    assert_eq!(out.get_pixel(50, 90).0, [255, 0, 0, 255]);
    // 직접 자르기 + WebP
    let ops = EditOps { crop: Some(Crop::Rect { x: 90, y: 0, w: 50, h: 40 }), format: Some(OutFormat::Webp), ..Default::default() };
    let o = imgedit::edit(None, &src, &ops, None, None).unwrap();
    assert_eq!((o.width, o.height, o.output.extension().unwrap().to_str().unwrap()), (50, 40, "webp"));
    // 사진 밖 자르기는 쉬운 말 오류
    let bad = EditOps { crop: Some(Crop::Rect { x: 500, y: 0, w: 5, h: 5 }), ..Default::default() };
    assert!(imgedit::edit(None, &src, &bad, None, None).unwrap_err().to_string().contains("사진 밖"));
}

#[test]
fn image_target_kb() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("photo.jpg");
    std::fs::write(&src, samples::jpeg_bytes(&samples::photo_like(1600, 1200, 7), 95).unwrap()).unwrap();
    let before = std::fs::metadata(&src).unwrap().len();
    for kb in [300u32, 120] {
        let o = imgedit::edit(None, &src, &EditOps { target_kb: Some(kb), ..Default::default() }, None, None).unwrap();
        let t = kb as u64 * 1024;
        assert!(o.after <= t, "{kb}KB 목표: {} > {t} (원본 {before})", o.after);
        assert!(o.after as f64 >= t as f64 * 0.85, "{kb}KB 목표에 너무 작음: {}", o.after);
        assert_eq!((o.width, o.height), (1600, 1200), "화질만으로 맞춤");
        println!("target {kb}KB -> {}B ({:.1}%)", o.after, o.after as f64 / t as f64 * 100.0);
    }
    // 아주 작은 목표: 크기까지 줄여 맞춤(메모)
    let o = imgedit::edit(None, &src, &EditOps { target_kb: Some(15), ..Default::default() }, None, None).unwrap();
    assert!(o.after <= 15 * 1024 && o.width < 1600, "{} {}", o.after, o.width);
    assert!(o.note.as_deref().unwrap_or("").contains("크기를"), "{:?}", o.note);
    // PNG(무손실)는 크기만 줄여 맞춤
    let png = d.path().join("p.png");
    samples::photo_like(600, 400, 3).save(&png).unwrap();
    let o = imgedit::edit(None, &png, &EditOps { target_kb: Some(100), ..Default::default() }, None, None).unwrap();
    assert!(o.after <= 100 * 1024 && o.width < 600, "{} {}", o.after, o.width);
}

#[test]
fn image_heic_resize_strip() {
    let Some((ffmpeg, _)) = ff() else {
        println!("ffmpeg 없음 — HEIC 건너뜀");
        return;
    };
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("IMG_0001.HEIC");
    if samples::make_grid_heic(&ffmpeg, &src, 64, 120, 100, "2024:05:01 10:20:30").is_err() {
        println!("libx265 없음 — HEIC 건너뜀");
        return;
    }
    let p = imgedit::probe(&src).unwrap();
    // grid 120×100 + irot 반시계 90 → 100×120
    assert_eq!((p.width, p.height, p.format.as_str()), (100, 120, "heic"));
    assert_eq!(p.taken_at.as_deref(), Some("2024-05-01 10:20:30"));
    let ops = EditOps { resize: Some(Resize { percent: Some(50.0), keep_ratio: true, ..Default::default() }), strip_meta: true, ..Default::default() };
    let o = imgedit::edit(Some(&ffmpeg), &src, &ops, None, None).unwrap();
    assert_eq!(o.output.file_name().unwrap(), "IMG_0001_편집.jpg");
    assert_eq!((o.width, o.height), (50, 60));
    assert!(out_exif(&o.output).is_none());
    // GPS가 든 HEIC → 위치 정보 지우기
    let gps = d.path().join("IMG_0002.HEIC");
    samples::make_grid_heic_exif(&ffmpeg, &gps, 64, 120, 100, exif::build_sample_gps("2024:05:01 10:20:30", 6, 37, 127)).unwrap();
    let p = imgedit::probe(&gps).unwrap();
    assert!(p.has_gps && p.taken_at.is_some());
    let o = imgedit::edit(Some(&ffmpeg), &gps, &EditOps { strip_meta: true, ..Default::default() }, None, None).unwrap();
    let q = imgedit::probe(&o.output).unwrap();
    assert!(!q.has_gps && q.taken_at.is_none() && (q.width, q.height) == (100, 120));
    // 정보를 지우지 않으면 GPS가 JPG로 옮겨진다
    let o = imgedit::edit(Some(&ffmpeg), &gps, &EditOps::default(), None, None).unwrap();
    assert!(imgedit::probe(&o.output).unwrap().has_gps);
    // HEIC인데 ffmpeg가 없으면 쉬운 말
    assert!(imgedit::edit(None, &src, &ops, None, None).unwrap_err().to_string().contains("ffmpeg"));
}

// ---------- 영상 ----------

#[test]
fn media_probe_trim_mute_rotate_frame() {
    let Some((ffmpeg, ffprobe)) = ff() else {
        println!("ffmpeg 없음 — 영상 건너뜀");
        return;
    };
    let d = tempfile::tempdir().unwrap();
    let plain = d.path().join("plain.mov");
    samples::make_test_video(&ffmpeg, &plain, 6, 320, 240).unwrap();
    // 회전 표시(반시계 90 = 플레이어는 시계 270으로 돌림)
    let src = d.path().join("동영상.mov");
    run(&ffmpeg, &["-hide_banner", "-loglevel", "error", "-display_rotation", "90", "-i", plain.to_str().unwrap(), "-c", "copy", src.to_str().unwrap()]);
    let p = mediaedit::probe(&ffprobe, &src).unwrap();
    assert!((p.duration - 6.0).abs() < 0.2, "{p:?}");
    assert_eq!((p.width, p.height, p.rotation, p.has_audio), (240, 320, 270, true));
    assert_eq!(p.video_codec.as_deref(), Some("mpeg4"));
    assert_eq!(p.audio_codec.as_deref(), Some("pcm_s16le"));

    // 앞뒤 자르기(키프레임 맞춰 그대로)
    let mut pr = Vec::new();
    let o = mediaedit::edit(&ffmpeg, &ffprobe, &src, &MediaOps { trim: Some(Trim { start: 1.3, end: 4.0 }), ..Default::default() }, None, None, &mut |f| pr.push(f)).unwrap();
    assert_eq!(o.output.file_name().unwrap(), "동영상_편집.mov");
    assert!(!o.reencoded);
    assert!((o.duration - 2.7).abs() <= 0.5, "길이 {}", o.duration);
    assert!(o.note.is_some());
    assert_eq!(pr.last().copied(), Some(1.0));
    let q = mediaedit::probe(&ffprobe, &o.output).unwrap();
    assert_eq!(q.rotation, 270, "자르기는 회전 정보 유지");

    // 소리 빼기
    let o = mediaedit::edit(&ffmpeg, &ffprobe, &src, &MediaOps { mute: Some(true), ..Default::default() }, None, None, &mut |_| {}).unwrap();
    let q = mediaedit::probe(&ffprobe, &o.output).unwrap();
    assert!(!q.has_audio && q.audio_codec.is_none());
    assert!((q.duration - 6.0).abs() < 0.3);

    // 돌리기(회전 정보): 270 + 90 = 0
    let o = mediaedit::edit(&ffmpeg, &ffprobe, &src, &MediaOps { rotate: Some(90), ..Default::default() }, None, None, &mut |_| {}).unwrap();
    let q = mediaedit::probe(&ffprobe, &o.output).unwrap();
    assert!(!o.reencoded);
    assert_eq!((q.rotation, q.width, q.height), (0, 320, 240));

    // MKV는 다시 압축 + 픽셀 돌리기
    let mkv = d.path().join("clip.mkv");
    run(&ffmpeg, &["-hide_banner", "-loglevel", "error", "-i", plain.to_str().unwrap(), "-c:v", "copy", "-c:a", "copy", mkv.to_str().unwrap()]);
    let o = mediaedit::edit(&ffmpeg, &ffprobe, &mkv, &MediaOps { rotate: Some(90), trim: Some(Trim { start: 0.5, end: 2.5 }), ..Default::default() }, None, None, &mut |_| {}).unwrap();
    assert!(o.reencoded && o.output.extension().unwrap() == "mp4");
    let q = mediaedit::probe(&ffprobe, &o.output).unwrap();
    assert_eq!((q.rotation, q.width, q.height), (0, 240, 320));
    assert!((q.duration - 2.0).abs() <= 0.5, "{}", q.duration);

    // 화면 한 장
    let jpg = mediaedit::frame_jpeg(&ffmpeg, &src, 2.0, Some(120), Some(p.duration), None).unwrap();
    let im = image::load_from_memory(&jpg).unwrap();
    assert_eq!(im.width(), 120);
    assert_eq!(im.height(), 160, "회전 반영");
    let saved = mediaedit::frame_save(&ffmpeg, Some(&ffprobe), &src, 3.4, None, None).unwrap();
    assert_eq!(saved.file_name().unwrap(), "동영상_장면_00-03.jpg");
    assert!(image::open(&saved).is_ok());
    // 길이보다 뒤면 끝 장면(이름도 끝 시각)
    let saved = mediaedit::frame_save(&ffmpeg, Some(&ffprobe), &src, 65.0, None, None).unwrap();
    assert_eq!(saved.file_name().unwrap(), "동영상_장면_00-05.jpg");
    assert!(image::open(&saved).is_ok());
}

// ---------- zip ----------

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = walkdir::WalkDir::new(dir).min_depth(1).into_iter().filter_map(|e| e.ok()).map(|e| e.path().strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/")).collect();
    v.sort();
    v
}

#[test]
fn zip_cp949_windows_and_mac_utf8() {
    let d = tempfile::tempdir().unwrap();
    // 윈도 탐색기식: CP949 이름, UTF-8 표시 없음
    let win = d.path().join("자료.zip");
    samples::write_windows_cp949_zip(&win, &[("보고서/", b""), ("보고서/1분기 실적.txt", "내용".as_bytes()), ("사진.jpg", b"jpg"), ("../밖.txt", b"x")]).unwrap();
    let l = ziptool::list(&win, NameEnc::Auto).unwrap();
    assert_eq!((l.detected_encoding, l.garbled_guess), ("cp949", true));
    let n: Vec<&str> = l.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(n, ["보고서/", "보고서/1분기 실적.txt", "사진.jpg", "../밖.txt"]);
    assert!(l.entries[0].is_dir && l.entries[1].size == "내용".len() as u64);
    // 억지로 UTF-8로 읽으면 깨진다(화면에서 고를 수 있게)
    assert!(ziptool::list(&win, NameEnc::Utf8).unwrap().entries[1].name.contains('\u{FFFD}'));

    let o = ziptool::extract(&win, NameEnc::Auto, None, None, &mut |_, _, _| {}).unwrap();
    assert_eq!(o.folder, d.path().join("자료"));
    assert_eq!(o.count, 2);
    assert_eq!(names(&o.folder), ["보고서", "보고서/1분기 실적.txt", "사진.jpg"]);
    assert_eq!(std::fs::read_to_string(o.folder.join("보고서/1분기 실적.txt")).unwrap(), "내용");
    assert_eq!(o.skipped.len(), 1);
    assert!(o.skipped[0].reason.contains("폴더 밖"));
    assert!(!d.path().join("밖.txt").exists());
    // 두 번 풀면 새 폴더(덮어쓰지 않음)
    let o2 = ziptool::extract(&win, NameEnc::Auto, None, None, &mut |_, _, _| {}).unwrap();
    assert_eq!(o2.folder, d.path().join("자료 (1)"));

    // 맥: UTF-8 NFD 이름(표시 없음) + __MACOSX
    let mac = d.path().join("mac.zip");
    let (a, b, j) = (samples::nfd("폴더/"), samples::nfd("폴더/한글 문서.txt"), samples::nfd("__MACOSX/폴더/._한글 문서.txt"));
    samples::write_mac_style_zip(&mac, &[(a.as_str(), b""), (b.as_str(), b"hi"), (j.as_str(), b"x"), ("폴더/.DS_Store", b"x")]).unwrap();
    let l = ziptool::list(&mac, NameEnc::Auto).unwrap();
    assert_eq!(l.detected_encoding, "utf8");
    assert_eq!(l.entries[1].name, "폴더/한글 문서.txt", "NFC로");
    let o = ziptool::extract(&mac, NameEnc::Auto, None, None, &mut |_, _, _| {}).unwrap();
    assert_eq!(names(&o.folder), ["폴더", "폴더/한글 문서.txt"]);
    assert_eq!(o.skipped.len(), 2);
}

#[test]
fn zip_create_roundtrip_utf8_flag() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("여름 사진");
    std::fs::create_dir_all(src.join("바다")).unwrap();
    std::fs::write(src.join("바다/파도.jpg"), b"jpgdata").unwrap();
    std::fs::write(src.join(samples::nfd("메모.txt")), "메모 내용".repeat(100)).unwrap();
    std::fs::write(src.join(".DS_Store"), b"x").unwrap();
    let extra = d.path().join("목록.txt");
    std::fs::write(&extra, b"list").unwrap();
    let o = ziptool::create(&[src.clone(), extra.clone()], None, None, &mut |_, _, _| {}).unwrap();
    assert_eq!(o.output, d.path().join("여름 사진.zip"));
    assert_eq!(o.count, 3);
    assert_eq!(o.size, std::fs::metadata(&o.output).unwrap().len());
    // 이름: NFC + UTF-8 표시
    let mut a = zip::ZipArchive::new(std::fs::File::open(&o.output).unwrap()).unwrap();
    let mut got = Vec::new();
    for i in 0..a.len() {
        let f = a.by_index_raw(i).unwrap();
        let name = String::from_utf8(f.name_raw().to_vec()).unwrap();
        if !name.is_ascii() {
            assert!(f.flags().as_u16() & (1 << 11) != 0, "UTF-8 표시 없음: {name}");
        }
        got.push(name);
    }
    got.sort();
    assert_eq!(got, ["목록.txt", "여름 사진/", "여름 사진/메모.txt", "여름 사진/바다/", "여름 사진/바다/파도.jpg"]);
    // 다시 풀면 같은 이름·내용
    let out = d.path().join("풀기");
    let x = ziptool::extract(&o.output, NameEnc::Auto, Some(&out), None, &mut |_, _, _| {}).unwrap();
    assert_eq!(x.count, 3);
    assert_eq!(std::fs::read(x.folder.join("여름 사진/바다/파도.jpg")).unwrap(), b"jpgdata");
    assert_eq!(std::fs::read_to_string(x.folder.join("여름 사진/메모.txt")).unwrap(), "메모 내용".repeat(100));
    // 두 번째는 (1)
    let o2 = ziptool::create(&[src], None, None, &mut |_, _, _| {}).unwrap();
    assert_eq!(o2.output, d.path().join("여름 사진 (1).zip"));
}

// ---------- 이름 바꾸기 ----------

#[test]
fn rename_swap_chain_undo() {
    let d = tempfile::tempdir().unwrap();
    let undo_dir = d.path().join("appdata");
    let w = d.path().join("files");
    std::fs::create_dir_all(&w).unwrap();
    for (n, c) in [("a.txt", "A"), ("b.txt", "B"), ("c.txt", "C")] {
        std::fs::write(w.join(n), c).unwrap();
    }
    // 맞바꾸기 a↔b + 이어 바꾸기 c→d
    let pairs = vec![
        Pair { from: w.join("a.txt"), to: w.join("b.txt") },
        Pair { from: w.join("b.txt"), to: w.join("a.txt") },
        Pair { from: w.join("c.txt"), to: w.join("d.txt") },
    ];
    let r = rename::apply(&pairs, &undo_dir).unwrap();
    assert_eq!((r.done, r.failed.len()), (3, 0), "{:?}", r.failed);
    let rd = |n: &str| std::fs::read_to_string(w.join(n)).unwrap();
    assert_eq!((rd("a.txt"), rd("b.txt"), rd("d.txt")), ("B".into(), "A".into(), "C".into()));
    assert!(!w.join("c.txt").exists());
    let id = r.undo_id.unwrap();
    assert!(undo_dir.join("rename-undo").join(format!("{id}.json")).is_file());
    let u = rename::undo(&id, &undo_dir).unwrap();
    assert_eq!((u.done, u.failed.len()), (3, 0));
    assert_eq!((rd("a.txt"), rd("b.txt"), rd("c.txt")), ("A".into(), "B".into(), "C".into()));
    assert!(rename::undo(&id, &undo_dir).is_err(), "두 번 되돌리기 안 됨");
    // 임시 파일이 남지 않음
    assert_eq!(names(&w), ["a.txt", "b.txt", "c.txt"]);

    // 이미 있는 이름(목록 밖)은 건너뛰고 원래대로
    std::fs::write(w.join("x.txt"), "X").unwrap();
    let r = rename::apply(&[Pair { from: w.join("a.txt"), to: w.join("x.txt") }], &undo_dir).unwrap();
    assert_eq!(r.done, 0);
    assert!(r.failed[0].reason.contains("이미 있습니다"));
    assert!(r.undo_id.is_none());
    assert_eq!(rd("x.txt"), "X");
}

#[test]
fn rename_preview_rules() {
    let d = tempfile::tempdir().unwrap();
    let w = d.path();
    let files: Vec<PathBuf> = ["IMG_10.JPG", "IMG_2.JPG", "IMG_1.JPG"].iter().map(|n| w.join(n)).collect();
    for f in &files {
        std::fs::write(f, b"x").unwrap();
    }
    // 촬영 날짜가 있는 사진 하나
    jpeg_with(&w.join("IMG_1.JPG"), 8, 8, exif::build_sample("2024:05:01 10:20:30", 1));
    let rule = Rule {
        find: Some("IMG_".into()),
        replace: Some("".into()),
        numbering: Some(Numbering { start: 1, step: 1, digits: 3, position: Pos::Prefix }),
        prefix: Some("여행_".into()),
        ext: Some(rename::ExtRule::Lower),
        ..Default::default()
    };
    let p = rename::preview(&files, &rule, SortBy::Name).unwrap();
    let to: Vec<String> = p.iter().map(|x| x.to.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(to, ["여행_001_1.jpg", "여행_002_2.jpg", "여행_003_10.jpg"]);
    assert!(p.iter().all(|x| x.conflict.is_none()));
    // 정규식 + 번호로 바꾸기 + 날짜(촬영, 없으면 수정 날짜) 뒤
    let rule = Rule {
        find: Some(r"^IMG_(\d+)$".into()),
        replace: Some("사진$1".into()),
        regex: true,
        date: Some(rename::DateRule { source: rename::DateSource::Taken, format: "YYYYMMDD".into(), position: Pos::Suffix }),
        ..Default::default()
    };
    let p = rename::preview(&files, &rule, SortBy::Name).unwrap();
    assert_eq!(p[0].to.file_name().unwrap(), "사진1_20240501.JPG");
    assert!(p[0].note.is_none() && p[1].note.as_deref().unwrap().contains("수정한 날짜"));
    // 겹침·쓸 수 없는 이름·이미 있는 이름
    std::fs::write(w.join("고정.JPG"), b"x").unwrap();
    let rule = Rule { numbering: Some(Numbering { start: 1, step: 0, digits: 1, position: Pos::Replace }), ..Default::default() };
    let p = rename::preview(&files, &rule, SortBy::Name).unwrap();
    assert!(p.iter().all(|x| x.conflict.as_deref() == Some("다른 파일과 새 이름이 겹칩니다")));
    let p = rename::preview(&files[..1], &Rule { find: Some("10".into()), replace: Some("a:b".into()), ..Default::default() }, SortBy::Name).unwrap();
    assert!(p[0].conflict.as_deref().unwrap().contains("쓸 수 없는 글자"));
    let p = rename::preview(&files[..1], &Rule { find: Some("IMG_10".into()), replace: Some("고정".into()), ..Default::default() }, SortBy::Name).unwrap();
    assert_eq!(p[0].conflict.as_deref(), Some("같은 이름의 파일이 이미 있습니다"));
    // 날짜순 정렬(촬영 날짜 2024 < 다른 파일 수정 날짜=지금)
    let p = rename::preview(&files, &Rule { numbering: Some(Numbering { start: 1, step: 1, digits: 2, position: Pos::Suffix }), ..Default::default() }, SortBy::Taken).unwrap();
    assert_eq!(p[0].from.file_name().unwrap(), "IMG_1.JPG");
    assert_eq!(p[0].to.file_name().unwrap(), "IMG_1_01.JPG");
}

// ---------- 같은 파일 ----------

#[test]
fn duplicates_group_and_trash_guard() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path().join("root");
    for sub in ["a", "b/c", ".hidden"] {
        std::fs::create_dir_all(r.join(sub)).unwrap();
    }
    let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let mut big2 = big.clone();
    big2[150_000] ^= 0xFF; // 크기·앞뒤 64KiB는 같고 가운데만 다름
    std::fs::write(r.join("a/영상.bin"), &big).unwrap();
    std::fs::write(r.join("b/c/영상 복사본.bin"), &big).unwrap();
    std::fs::write(r.join("b/다른 영상.bin"), &big2).unwrap();
    std::fs::write(r.join("a/메모.txt"), b"same small").unwrap();
    std::fs::write(r.join("b/메모 (1).txt"), b"same small").unwrap();
    std::fs::write(r.join(".hidden/메모.txt"), b"same small").unwrap();
    std::fs::write(r.join("a/x.txt"), b"abcdefghij").unwrap(); // 같은 크기 다른 내용
    std::fs::write(r.join("a/empty1"), b"").unwrap();
    std::fs::write(r.join("b/empty2"), b"").unwrap();

    let mut stages = Vec::new();
    let s = dupes::scan(&[r.clone()], &ScanOpts::default(), None, &mut |st, _, _, _| stages.push(st.to_string())).unwrap();
    assert_eq!(s.groups.len(), 2, "{:#?}", s.groups);
    assert_eq!(s.scanned, 8, "숨김 폴더 빼고 8개");
    let g0 = &s.groups[0];
    assert_eq!(g0.size, 300_000);
    let rel = |p: &Path| p.strip_prefix(&r).unwrap().to_string_lossy().replace('\\', "/");
    assert_eq!(g0.files.iter().map(|f| rel(&f.path)).collect::<Vec<_>>(), ["a/영상.bin", "b/c/영상 복사본.bin"]);
    assert_eq!(g0.hash.len(), 64);
    assert_eq!(s.groups[1].files.len(), 2);
    assert_eq!(s.wasted_bytes, 300_000 + 10);
    assert!(stages.iter().any(|x| x == "hash"));
    // 숨김 포함 + 최소 크기
    let s2 = dupes::scan(&[r.clone()], &ScanOpts { include_hidden: true, min_size: 0 }, None, &mut |_, _, _, _| {}).unwrap();
    assert_eq!(s2.groups.iter().map(|g| g.files.len()).collect::<Vec<_>>(), [2, 3]);
    let s3 = dupes::scan(&[r.clone()], &ScanOpts { include_hidden: false, min_size: 1000 }, None, &mut |_, _, _, _| {}).unwrap();
    assert_eq!(s3.groups.len(), 1);

    // 휴지통(점검용으로 다른 폴더로 옮기는 가짜)
    let bin = d.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let mut moved = Vec::new();
    let mut fake = |p: &Path| -> filetool_core::Result<()> {
        moved.push(p.to_path_buf());
        std::fs::rename(p, bin.join(format!("{}", moved.len()))).unwrap();
        Ok(())
    };
    // 묶음의 모든 파일 → 통째로 거절
    let all: Vec<PathBuf> = g0.files.iter().map(|f| f.path.clone()).chain([s.groups[1].files[0].path.clone()]).collect();
    let e = dupes::trash(&all, &s.groups, &mut fake).unwrap_err().to_string();
    assert!(e.contains("모두 지우게"), "{e}");
    // 결과에 없는 파일도 거절
    assert!(dupes::trash(&[r.join("a/x.txt")], &s.groups, &mut fake).is_err());
    assert!(r.join("a/영상.bin").exists() && r.join("b/c/영상 복사본.bin").exists());
    // 하나만
    let t = dupes::trash(&[g0.files[1].path.clone()], &s.groups, &mut fake).unwrap();
    assert_eq!((t.done, t.failed.len()), (1, 0));
    assert!(!r.join("b/c/영상 복사본.bin").exists() && r.join("a/영상.bin").exists());
    // 남길 파일이 그사이 바뀌면 거절
    std::fs::write(r.join("b/메모 (1).txt"), b"changed!!!").unwrap();
    let e = dupes::trash(&[r.join("a/메모.txt")], &s.groups, &mut fake).unwrap_err().to_string();
    assert!(e.contains("바뀌었거나"), "{e}");
    drop(fake);
    assert_eq!(moved.len(), 1);
}
