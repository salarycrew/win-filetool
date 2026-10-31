//! 1b 코어 점검: 도구 찾기(가짜 폴더), argv 만들기, 감시 폴더(임시 폴더), 이미지 변환(생성한 그림),
//! ffmpeg가 PATH에 있으면 HEIC(grid)·영상 변환까지.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use filetool_core::convert::{self, Op};
use filetool_core::docs::DocTarget;
use filetool_core::imageconv::{self, ImageOpts, ImgFormat};
use filetool_core::media::{self, MediaPreset};
use filetool_core::process::{self, Cancel};
use filetool_core::samples;
use filetool_core::tools::{self, SearchEnv, Source, Tool, ToolPaths, Toolset};
use filetool_core::watch::{WatchRule, Watcher};

fn touch(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"").unwrap();
}

fn exe(t: Tool) -> String {
    t.exe_name()
}

#[test]
fn tool_discovery_fake_dirs() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path();
    let env = SearchEnv {
        path_dirs: vec![root.join("emptypath")],
        program_files: vec![root.join("Program Files")],
        local_appdata: Some(root.join("Local")),
        user_profile: Some(root.join("User")),
        exe_dir: Some(root.join("app")),
    };
    // 아무것도 없으면 못 찾음 + 안내
    assert!(tools::find(Tool::Ffmpeg, None, None, &env).is_none());
    let ts = Toolset::discover(&ToolPaths::default(), &env);
    let err = ts.get(Tool::Soffice).unwrap_err().to_string();
    assert!(err.contains("LibreOffice") && err.contains("msiexec /a"), "{err}");

    // winget 패키지 폴더의 ffmpeg (+ 같은 폴더의 ffprobe)
    let wg = root.join("Local/Microsoft/WinGet/Packages/Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe/ffmpeg-9.0-full_build/bin");
    touch(&wg.join(exe(Tool::Ffmpeg)));
    touch(&wg.join(exe(Tool::Ffprobe)));
    let (p, src) = tools::find(Tool::Ffmpeg, None, None, &env).unwrap();
    assert_eq!((p.clone(), src), (wg.join(exe(Tool::Ffmpeg)), Source::KnownLocation));
    let ts = Toolset::discover(&ToolPaths::default(), &env);
    assert_eq!(ts.ffprobe.as_deref(), Some(wg.join(exe(Tool::Ffprobe)).as_path()));

    // PATH가 설치 폴더보다 먼저
    let pathdir = root.join("emptypath");
    touch(&pathdir.join(exe(Tool::Ffmpeg)));
    assert_eq!(tools::find(Tool::Ffmpeg, None, None, &env).unwrap().1, Source::Path);

    // 사용자 지정(폴더를 고르면 그 안/bin/program에서 찾음)이 가장 먼저
    let mine = root.join("my ffmpeg");
    touch(&mine.join("bin").join(exe(Tool::Ffmpeg)));
    let (p, src) = tools::find(Tool::Ffmpeg, Some(&mine), None, &env).unwrap();
    assert_eq!((p, src), (mine.join("bin").join(exe(Tool::Ffmpeg)), Source::UserSetting));
    // 잘못된 사용자 경로면 다음 순서로
    assert_eq!(tools::find(Tool::Ffmpeg, Some(&root.join("nope")), None, &env).unwrap().1, Source::Path);

    // LibreOffice: Program Files, Portable, 앱 옆 tools\*\program
    let lo = root.join("Program Files/LibreOffice/program").join(exe(Tool::Soffice));
    touch(&lo);
    assert_eq!(tools::find(Tool::Soffice, None, None, &env).unwrap().0, lo);
    std::fs::remove_file(&lo).unwrap();
    let portable = root.join("User/PortableApps/LibreOfficePortable/App/libreoffice/program").join(exe(Tool::Soffice));
    touch(&portable);
    assert_eq!(tools::find(Tool::Soffice, None, None, &env).unwrap().0, portable);
    std::fs::remove_file(&portable).unwrap();
    let side = root.join("app/tools/lo/program").join(exe(Tool::Soffice));
    touch(&side);
    assert_eq!(tools::find(Tool::Soffice, None, None, &env).unwrap().0, side);

    // 버전 줄 해석
    assert_eq!(tools::parse_version(Tool::Ffmpeg, "ffmpeg version N-125875-g5d4d3bdc61-20260731 Copyright (c)").as_deref(), Some("N-125875-g5d4d3bdc61-20260731"));
    assert_eq!(tools::parse_version(Tool::Soffice, "LibreOffice 26.2.6.2 8221e31b3ac3\n").as_deref(), Some("LibreOffice 26.2.6.2"));
}

fn s(v: &[std::ffi::OsString]) -> Vec<String> {
    v.iter().map(|x| x.to_string_lossy().into_owned()).collect()
}

#[test]
fn media_argv_and_progress() {
    let i = Path::new("/v/여행 영상.mov");
    let o = Path::new("/v/여행 영상.mp4");
    let a = s(&media::build_args(&MediaPreset::Mp4, i, o, None));
    assert_eq!(&a[..3], ["-hide_banner", "-nostdin", "-n"]);
    assert!(a.windows(2).any(|w| w == ["-progress", "pipe:1"]));
    assert!(a.windows(2).any(|w| w == ["-c:v", "libx264"]) && a.windows(2).any(|w| w == ["-c:a", "aac"]));
    assert!(a.windows(2).any(|w| w == ["-map", "0:a:0?"]), "소리 없는 영상도 되도록 선택적 매핑");
    assert_eq!(a.last().unwrap(), "/v/여행 영상.mp4");
    let ip = a.iter().position(|x| x == "-i").unwrap();
    assert_eq!(a[ip + 1], "/v/여행 영상.mov", "경로는 셸 따옴표 없이 인자 하나");

    // 메신저: 60초, 25MB → 비트레이트, 720p 제한
    let m = MediaPreset::Messenger { max_mb: 25, max_height: 720 };
    let a = s(&media::build_args(&m, i, o, Some(60.0)));
    let kbps = media::messenger_video_kbps(25, 60.0);
    assert_eq!(kbps, 3119);
    assert!(a.windows(2).any(|w| w[0] == "-b:v" && w[1] == format!("{kbps}k")));
    assert!(a.windows(2).any(|w| w == ["-vf", "scale=-2:'min(720,ih)'"]));
    // 예상 크기 = (영상+소리) × 길이 ≤ 25MB
    let est_bytes = (kbps as f64 + 96.0) * 1000.0 / 8.0 * 60.0;
    assert!(est_bytes <= 25.0 * 1048576.0, "{est_bytes}");
    let a = s(&media::build_args(&m, i, o, None));
    assert!(a.windows(2).any(|w| w == ["-crf", "28"]));

    let a = s(&media::build_args(&MediaPreset::Mp3, i, Path::new("/v/a.mp3"), None));
    assert!(a.contains(&"-vn".into()) && a.windows(2).any(|w| w == ["-c:a", "libmp3lame"]));
    let g = MediaPreset::Gif { start: 1.5, duration: 3.0, width: 480, fps: 12 };
    let a = s(&media::build_args(&g, i, Path::new("/v/a.gif"), None));
    assert_eq!(&a[8..12], ["-ss", "1.5", "-t", "3"]);
    assert!(a.iter().any(|x| x.starts_with("fps=12,scale=480:-2") && x.contains("palettegen")));

    assert_eq!(media::parse_progress_secs("out_time_us=1500000"), Some(1.5));
    assert_eq!(media::parse_progress_secs("out_time_ms=2000000"), Some(2.0));
    assert_eq!(media::parse_progress_secs("out_time=00:01:02.500000"), Some(62.5));
    assert_eq!(media::parse_progress_secs("out_time_us=N/A"), None);
    assert_eq!(media::parse_progress_secs("frame=10"), None);
}

#[test]
fn process_timeout_and_cancel() {
    if cfg!(windows) {
        return;
    }
    let sh = Path::new("/bin/sh");
    let args: Vec<std::ffi::OsString> = vec!["-c".into(), "echo a; echo b; sleep 30".into()];
    let mut lines = Vec::new();
    let t0 = Instant::now();
    let r = process::run(sh, &args, Duration::from_millis(500), None, &mut |l| lines.push(l.to_string()));
    assert!(matches!(r, Err(filetool_core::Error::Timeout(_))));
    assert!(t0.elapsed() < Duration::from_secs(5));
    assert_eq!(lines, ["a", "b"]);
    let c = Cancel::new();
    let c2 = c.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        c2.cancel();
    });
    let t0 = Instant::now();
    let r = process::run(sh, &args, Duration::from_secs(60), Some(&c), &mut |_| {});
    assert!(matches!(r, Err(filetool_core::Error::Cancelled)));
    assert!(t0.elapsed() < Duration::from_secs(5));
}

fn opts(format: ImgFormat) -> ImageOpts {
    ImageOpts { format, quality: 90, max_side: None, keep_exif: true }
}

#[test]
fn image_round_trips() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("사진.png");
    samples::quadrant_image(120, 80).save(&src).unwrap();
    let mut prev = src.clone();
    for f in [ImgFormat::Jpg, ImgFormat::Webp, ImgFormat::Bmp, ImgFormat::Tiff, ImgFormat::Gif, ImgFormat::Png] {
        let r = imageconv::convert_raster(&prev, None, &opts(f)).unwrap();
        assert_eq!(r.output.extension().unwrap(), f.ext());
        let img = image::open(&r.output).unwrap();
        assert_eq!((img.width(), img.height()), (120, 80), "{f:?}");
        assert_eq!(samples::quadrant_colors(&img), ["빨강", "초록", "파랑", "흰색"], "{f:?}");
        prev = r.output;
    }
    // 마지막 png는 원본과 같은 이름 → "사진 (1).png", 원본은 그대로
    assert_eq!(prev.file_name().unwrap(), "사진 (1).png");
    assert_eq!((image::open(&src).unwrap().width()), 120);

    // GIF는 첫 장면만
    let gif = d.path().join("anim.gif");
    {
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame, RgbaImage};
        let f = std::fs::File::create(&gif).unwrap();
        let mut enc = GifEncoder::new(f);
        for c in [[255u8, 0, 0, 255], [0, 0, 255, 255]] {
            enc.encode_frame(Frame::from_parts(RgbaImage::from_pixel(10, 10, image::Rgba(c)), 0, 0, Delay::from_numer_denom_ms(100, 1))).unwrap();
        }
    }
    let r = imageconv::convert_raster(&gif, None, &opts(ImgFormat::Png)).unwrap();
    assert_eq!(samples::color_name(image::open(&r.output).unwrap().to_rgb8().get_pixel(5, 5).0), "빨강");

    // 품질: JPG 30 < 95, WebP 30 < 95
    let big = d.path().join("big.png");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(400, 300, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x * y) % 256) as u8]))).save(&big).unwrap();
    for f in [ImgFormat::Jpg, ImgFormat::Webp] {
        let lo = imageconv::convert_raster(&big, None, &ImageOpts { quality: 30, ..opts(f) }).unwrap();
        let hi = imageconv::convert_raster(&big, None, &ImageOpts { quality: 95, ..opts(f) }).unwrap();
        let (a, b) = (std::fs::metadata(&lo.output).unwrap().len(), std::fs::metadata(&hi.output).unwrap().len());
        assert!(a < b, "{f:?} {a} < {b}");
    }
    // 긴 변 제한
    let r = imageconv::convert_raster(&big, None, &ImageOpts { max_side: Some(200), ..opts(ImgFormat::Jpg) }).unwrap();
    assert_eq!((r.width, r.height), (200, 150));
    // 투명 PNG → JPG는 흰 바탕
    let tp = d.path().join("transparent.png");
    image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 0])).save(&tp).unwrap();
    let r = imageconv::convert_raster(&tp, None, &opts(ImgFormat::Jpg)).unwrap();
    assert_eq!(samples::color_name(image::open(&r.output).unwrap().to_rgb8().get_pixel(4, 4).0), "흰색");
}

#[test]
fn jpeg_exif_orientation_and_date() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("세로.jpg");
    // 가로 120×80 픽셀 + EXIF 회전 6(시계 90°) → 보이는 모습 80×120
    samples::make_jpeg_with_exif(&src, 120, 80, "2024:05:01 10:20:30", 6).unwrap();
    let r = imageconv::convert_raster(&src, None, &opts(ImgFormat::Jpg)).unwrap();
    assert_eq!((r.width, r.height), (80, 120));
    assert_eq!(r.exif_date.as_deref(), Some("2024:05:01 10:20:30"));
    let out = std::fs::read(&r.output).unwrap();
    let ex = filetool_core::exif::from_jpeg(&out).expect("출력에 EXIF");
    assert_eq!(filetool_core::exif::orientation(&ex), Some(1), "회전은 픽셀에 적용했으니 1");
    assert_eq!(filetool_core::exif::date_taken(&ex).as_deref(), Some("2024:05:01 10:20:30"));
    // 시계 90°: 왼위 빨강 → 오른위
    assert_eq!(samples::quadrant_colors(&image::open(&r.output).unwrap()), ["파랑", "빨강", "흰색", "초록"]);
    // 수정 시각 유지
    let m = |p: &Path| std::fs::metadata(p).unwrap().modified().unwrap();
    assert_eq!(m(&src), m(&r.output));
    // keep_exif=false면 EXIF 없음
    let r = imageconv::convert_raster(&src, None, &ImageOpts { keep_exif: false, ..opts(ImgFormat::Jpg) }).unwrap();
    assert!(filetool_core::exif::from_jpeg(&std::fs::read(&r.output).unwrap()).is_none());
}

fn img_rule(dir: &Path) -> WatchRule {
    WatchRule { id: "r1".into(), folder: dir.to_path_buf(), op: Op::Image(opts(ImgFormat::Jpg)), out_subfolder: "변환됨".into(), enabled: true }
}

#[test]
fn watch_folder_stability_and_state() {
    let d = tempfile::tempdir().unwrap();
    let inbox = d.path().join("받은 사진");
    std::fs::create_dir_all(&inbox).unwrap();
    let state = d.path().join("state.json");
    let rule = img_rule(&inbox);
    rule.validate().unwrap();
    let tools = Toolset::default();
    let stable = Duration::from_secs(2);
    let mut w = Watcher::new(state.clone(), stable);
    let t0 = Instant::now();

    // 복사 중: 크기가 바뀌는 동안은 처리하지 않음
    let png = inbox.join("a.png");
    let mut bytes = Vec::new();
    samples::quadrant_image(40, 40).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
    std::fs::write(&png, &bytes[..bytes.len() / 2]).unwrap();
    std::fs::write(inbox.join("~$잠금.png"), b"x").unwrap();
    std::fs::write(inbox.join("다운로드중.png.crdownload"), b"x").unwrap();
    std::fs::write(inbox.join("메모.txt"), b"x").unwrap();
    assert!(w.run_once(&[rule.clone()], &tools, None, t0).is_empty());
    std::fs::write(&png, &bytes).unwrap(); // 복사 끝
    assert!(w.run_once(&[rule.clone()], &tools, None, t0 + Duration::from_secs(1)).is_empty(), "크기가 바뀌었으니 다시 기다림");
    assert!(w.run_once(&[rule.clone()], &tools, None, t0 + Duration::from_secs(2)).is_empty(), "안정된 지 1초");
    let ev = w.run_once(&[rule.clone()], &tools, None, t0 + Duration::from_secs(4));
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert!(ev[0].ok, "{:?}", ev[0]);
    assert_eq!(ev[0].output.as_deref(), Some(inbox.join("변환됨").join("a.jpg").as_path()));
    // 같은 파일은 다시 처리하지 않음(같은 감시기, 새 감시기=앱 재시작)
    assert!(w.run_once(&[rule.clone()], &tools, None, t0 + Duration::from_secs(10)).is_empty());
    let mut w2 = Watcher::new(state.clone(), stable);
    let t1 = Instant::now();
    assert!(w2.run_once(&[rule.clone()], &tools, None, t1).is_empty());
    assert!(w2.run_once(&[rule.clone()], &tools, None, t1 + Duration::from_secs(5)).is_empty());
    let outs: Vec<_> = std::fs::read_dir(inbox.join("변환됨")).unwrap().collect();
    assert_eq!(outs.len(), 1);

    // 꺼진 규칙은 처리 안 함
    std::fs::write(inbox.join("b.png"), &bytes).unwrap();
    let off = WatchRule { enabled: false, ..rule.clone() };
    assert!(w2.run_once(&[off.clone()], &tools, None, t1).is_empty());
    assert!(w2.run_once(&[off], &tools, None, t1 + Duration::from_secs(5)).is_empty());
    // 켜면 처리
    w2.run_once(&[rule.clone()], &tools, None, t1 + Duration::from_secs(6));
    let ev = w2.run_once(&[rule.clone()], &tools, None, t1 + Duration::from_secs(9));
    assert_eq!(ev.len(), 1);

    // 출력 하위 폴더가 비면 규칙 거부
    let bad = WatchRule { out_subfolder: " ".into(), ..rule.clone() };
    assert!(bad.validate().is_err());
    assert!(WatchRule { out_subfolder: "../밖".into(), ..rule.clone() }.validate().is_err());

    // 실패한 파일도 기록해서 계속 다시 시도하지 않음(내용이 바뀌면 다시)
    std::fs::write(inbox.join("깨짐.png"), b"not a png at all").unwrap();
    w2.run_once(&[rule.clone()], &tools, None, t1 + Duration::from_secs(10));
    let ev = w2.run_once(&[rule.clone()], &tools, None, t1 + Duration::from_secs(13));
    assert_eq!(ev.len(), 1);
    assert!(!ev[0].ok);
    assert!(w2.run_once(&[rule.clone()], &tools, None, t1 + Duration::from_secs(20)).is_empty());

    // 상태 파일에 기록됨
    let st: serde_json::Value = serde_json::from_slice(&std::fs::read(&state).unwrap()).unwrap();
    let done = st["rules"]["r1"]["done"].as_object().unwrap();
    assert!(done.contains_key("a.png") && done.contains_key("b.png") && done.contains_key("깨짐.png"));
    assert_eq!(done["깨짐.png"]["ok"], false);
}

#[test]
fn settings_roundtrip() {
    let d = tempfile::tempdir().unwrap();
    let mut s = filetool_core::settings::Settings::default();
    s.tools.soffice = Some(PathBuf::from(r"C:\LO\program"));
    s.watch_rules.push(img_rule(d.path()));
    s.watch_rules.push(WatchRule { id: "doc".into(), op: Op::Doc { target: DocTarget::Pdf }, ..img_rule(d.path()) });
    s.watch_rules.push(WatchRule { id: "v".into(), op: Op::Media(MediaPreset::Messenger { max_mb: 25, max_height: 720 }), ..img_rule(d.path()) });
    s.save(d.path()).unwrap();
    let l = filetool_core::settings::Settings::load(d.path());
    assert_eq!(l.watch_rules.len(), 3);
    assert_eq!(l.tools.soffice, s.tools.soffice);
    assert!(matches!(l.watch_rules[2].op, Op::Media(MediaPreset::Messenger { max_mb: 25, .. })));
    // 손상된 파일이면 기본값
    std::fs::write(d.path().join("settings.json"), b"{broken").unwrap();
    assert!(filetool_core::settings::Settings::load(d.path()).watch_rules.is_empty());
}

#[test]
fn op_accepts() {
    let img = Op::Image(opts(ImgFormat::Jpg));
    assert!(img.accepts(Path::new("a.HEIC")) && img.accepts(Path::new("a.webp")) && !img.accepts(Path::new("a.mov")));
    let mp3 = Op::Media(MediaPreset::Mp3);
    assert!(mp3.accepts(Path::new("a.mov")) && mp3.accepts(Path::new("a.wav")));
    assert!(!Op::Media(MediaPreset::Mp4).accepts(Path::new("a.wav")));
    let pdf = Op::Doc { target: DocTarget::Pdf };
    assert!(pdf.accepts(Path::new("a.pages")) && pdf.accepts(Path::new("a.hwp")) && !pdf.accepts(Path::new("a.pdf")));
    assert!(!Op::Doc { target: DocTarget::Docx }.accepts(Path::new("a.key")));
}

// ---------- ffmpeg가 있으면 ----------

fn system_tools() -> Option<Toolset> {
    let ts = Toolset::discover(&ToolPaths::default(), &SearchEnv::from_system());
    if ts.ffmpeg.is_none() || ts.ffprobe.is_none() {
        eprintln!("ffmpeg/ffprobe 없음 — 건너뜀");
        return None;
    }
    Some(ts)
}

#[test]
fn heic_grid_with_ffmpeg() {
    let Some(ts) = system_tools() else { return };
    let d = tempfile::tempdir().unwrap();
    let heic = d.path().join("IMG_0001.HEIC");
    samples::make_grid_heic(ts.ffmpeg.as_deref().unwrap(), &heic, 256, 500, 400, "2023:08:15 14:03:22").unwrap();
    let bytes = std::fs::read(&heic).unwrap();
    assert!(filetool_core::heif::is_heif(&bytes));
    let ex = filetool_core::heif::extract_exif(&bytes).unwrap();
    assert_eq!(filetool_core::exif::date_taken(&ex).as_deref(), Some("2023:08:15 14:03:22"));

    let mut p = 0.0;
    let r = convert::convert_file(&ts, &heic, None, &Op::Image(opts(ImgFormat::Jpg)), None, &mut |x| p = x).unwrap();
    assert_eq!(p, 1.0);
    let img = image::open(&r.output).unwrap();
    // grid 500×400을 irot(반시계 90°)로 돌려 400×500
    assert_eq!((img.width(), img.height()), (400, 500), "{}", r.note);
    assert_eq!(samples::quadrant_colors(&img), ["초록", "흰색", "빨강", "파랑"]);
    let ex = filetool_core::exif::from_jpeg(&std::fs::read(&r.output).unwrap()).unwrap();
    assert_eq!(filetool_core::exif::orientation(&ex), Some(1));
    assert!(r.note.contains("2023:08:15"), "{}", r.note);
    let r = convert::convert_file(&ts, &heic, None, &Op::Image(ImageOpts { max_side: Some(250), ..opts(ImgFormat::Png) }), None, &mut |_| {}).unwrap();
    let img = image::open(&r.output).unwrap();
    assert_eq!((img.width(), img.height()), (200, 250));
}

#[test]
fn video_presets_with_ffmpeg() {
    let Some(ts) = system_tools() else { return };
    let d = tempfile::tempdir().unwrap();
    let mov = d.path().join("클립.mov");
    samples::make_test_video(ts.ffmpeg.as_deref().unwrap(), &mov, 3, 640, 360).unwrap();
    let probe = |p: &Path| media::probe_streams(ts.ffprobe.as_deref().unwrap(), p).join(" | ");

    let mut ticks = Vec::new();
    let r = convert::convert_file(&ts, &mov, None, &Op::Media(MediaPreset::Mp4), None, &mut |x| ticks.push(x)).unwrap();
    let info = probe(&r.output);
    assert!(info.contains("codec_name=h264") && info.contains("codec_name=aac"), "{info}");
    assert!(ticks.iter().any(|x| *x > 0.0 && *x < 1.0) || ticks.len() > 1, "{ticks:?}");
    assert_eq!(*ticks.last().unwrap(), 1.0);

    let r = convert::convert_file(&ts, &mov, None, &Op::Media(MediaPreset::Messenger { max_mb: 1, max_height: 240 }), None, &mut |_| {}).unwrap();
    let info = probe(&r.output);
    assert!(info.contains("height=240"), "{info}");
    assert!(std::fs::metadata(&r.output).unwrap().len() <= 1024 * 1024);

    let r = convert::convert_file(&ts, &mov, None, &Op::Media(MediaPreset::Mp3), None, &mut |_| {}).unwrap();
    assert!(probe(&r.output).contains("codec_name=mp3"));
    let r = convert::convert_file(&ts, &mov, None, &Op::Media(MediaPreset::M4a), None, &mut |_| {}).unwrap();
    assert!(probe(&r.output).contains("codec_name=aac"));
    let r = convert::convert_file(&ts, &mov, None, &Op::Media(MediaPreset::Gif { start: 0.5, duration: 1.0, width: 160, fps: 10 }), None, &mut |_| {}).unwrap();
    let info = probe(&r.output);
    assert!(info.contains("codec_name=gif") && info.contains("width=160"), "{info}");

    // 취소하면 만들던 파일이 남지 않음
    let c = Cancel::new();
    c.cancel();
    let before = std::fs::read_dir(d.path()).unwrap().count();
    assert!(matches!(convert::convert_file(&ts, &mov, None, &Op::Media(MediaPreset::Mp4), Some(&c), &mut |_| {}), Err(filetool_core::Error::Cancelled)));
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), before);
}

#[test]
fn watch_lock_single_holder() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("watch.lock");
    let a = filetool_core::watch::WatchLock::try_acquire(&p).expect("첫 번째는 잡음");
    assert!(filetool_core::watch::WatchLock::try_acquire(&p).is_none(), "두 번째는 못 잡음");
    drop(a);
    assert!(filetool_core::watch::WatchLock::try_acquire(&p).is_some(), "풀린 뒤 다시 잡음");
}
