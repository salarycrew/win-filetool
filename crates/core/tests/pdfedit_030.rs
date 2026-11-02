//! 0.3.0 PDF 편집 코어. 구조(쪽 수·순서·회전·상자·정보·양식) + PDFium 렌더로 보이는 위치 확인.
//! 렌더·글자 넣기 테스트는 PDFium(`third_party/pdfium/linux-x64`)과 한글 글꼴(나눔고딕/Noto CJK)이 있어야 돈다.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use filetool_core::pdfedit::crypt::{self, Cipher, ProtectOpts};
use filetool_core::pdfedit::forms;
use filetool_core::pdfedit::info::{self, DocInfo};
use filetool_core::pdfedit::layout::{self, CropMode, CropOpts, FitOpts, NupOpts, NupOrder};
use filetool_core::pdfedit::plan::{self, BlankSize, EditPlan, PlanPage, PlanSource};
use filetool_core::pdfedit::stamp::{self, ImagePlacement, Layer, NumberFormat, PageNumberOpts, Position, SignatureOpts, TextBox, Tiling, WatermarkKind, WatermarkOpts};
use filetool_core::pdfedit::{self, testpdf, thumbs, Job, Orientation};
use filetool_core::process::Cancel;
use filetool_core::{pdfrender, Error};
use lopdf::{Document, Object};

fn pdfium_ok() -> bool {
    // 0.3.3(보안 강화): WIN_FILETOOL_PDFIUM은 시험용 바꾸기를 켰을 때만 읽는다
    filetool_core::testhooks::enable();
    if std::env::var_os("WIN_FILETOOL_PDFIUM").is_none() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party/pdfium/linux-x64/lib");
        std::env::set_var("WIN_FILETOOL_PDFIUM", p);
    }
    let ok = pdfrender::pdfium_status().is_ok();
    if !ok {
        eprintln!("PDFium 없음 — 렌더 확인 건너뜀");
    }
    ok
}

fn font_ok() -> bool {
    let ok = pdfedit::font::korean_font().is_ok();
    if !ok {
        eprintln!("한글 글꼴 없음 — 건너뜀");
    }
    ok
}

/// A4 세로 / A4 가로 / 90도 회전한 A4 세로 / 작은 쪽(A5)
fn mixed(dir: &Path) -> PathBuf {
    let p = dir.join("섞임.pdf");
    testpdf::make_pages(&p, &[(595.0, 842.0, 0), (842.0, 595.0, 0), (595.0, 842.0, 90), (420.0, 595.0, 0)], "P").unwrap();
    p
}

fn load(p: &Path) -> Document {
    Document::load(p).unwrap()
}

fn labels(p: &Path) -> Vec<String> {
    let d = load(p);
    d.get_pages()
        .values()
        .map(|&pid| {
            let s = String::from_utf8_lossy(&d.get_page_content(pid)).to_string();
            s.find("Td (").and_then(|a| s[a + 4..].find(')').map(|b| s[a + 4..a + 4 + b].to_string())).unwrap_or_default()
        })
        .collect()
}

fn rotations(p: &Path) -> Vec<i64> {
    let d = load(p);
    d.get_pages().values().map(|&id| pdfedit::page_geom(&d, id).rotate).collect()
}

fn vsizes(p: &Path) -> Vec<(i32, i32)> {
    let d = load(p);
    d.get_pages()
        .values()
        .map(|&id| {
            let (w, h) = pdfedit::page_geom(&d, id).visual_size();
            (w.round() as i32, h.round() as i32)
        })
        .collect()
}

/// 보이는 쪽 사각형(왼쪽 위 원점, pt) 안의 어두운 화소 수. 1pt = 1px로 그린다.
fn dark_in(p: &Path, pw: Option<&str>, page: u32, x: f32, y: f32, w: f32, h: f32, thr: u8) -> usize {
    let img = thumbs::render_rgb(p, pw, page, 1.0).unwrap();
    let mut n = 0;
    for yy in y as u32..(y + h) as u32 {
        for xx in x as u32..(x + w) as u32 {
            if xx < img.width() && yy < img.height() {
                let px = img.get_pixel(xx, yy).0;
                if px.iter().any(|&c| c < thr) {
                    n += 1;
                }
            }
        }
    }
    n
}

fn pixel(p: &Path, page: u32, x: u32, y: u32) -> [u8; 3] {
    thumbs::render_rgb(p, None, page, 1.0).unwrap().get_pixel(x, y).0
}

fn out(dir: &Path, name: &str) -> PathBuf {
    dir.join(name)
}

// ---------- 쪽 편집 계획 ----------

#[test]
fn plan_reorders_rotates_duplicates_inserts_and_merges() {
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    let b = t.path().join("b.pdf");
    testpdf::make_pages(&b, &[(595.0, 842.0, 0), (595.0, 842.0, 0)], "Q").unwrap();
    let plan = EditPlan {
        sources: vec![PlanSource { path: a.clone(), password: None }, PlanSource { path: b.clone(), password: None }],
        pages: vec![
            PlanPage::Page { src: 0, index: 2, rotate: 90 },
            PlanPage::Page { src: 1, index: 1, rotate: 0 },
            PlanPage::Blank { size: BlankSize::Named("same".into()) },
            PlanPage::Page { src: 0, index: 0, rotate: -90 },
            PlanPage::Page { src: 0, index: 0, rotate: 0 },
            PlanPage::Blank { size: BlankSize::Custom { w: 300.0, h: 200.0 } },
        ],
    };
    let o = out(t.path(), "plan.pdf");
    let r = plan::apply(&plan, &o, &mut Job::none()).unwrap();
    assert_eq!(r.pages, 6);
    assert_eq!(labels(&o), vec!["P3", "Q2", "", "P1", "P1", ""]);
    assert_eq!(rotations(&o), vec![180, 0, 0, 270, 0, 0]);
    // 빈 쪽 "same" = 바로 앞쪽(Q2, A4 세로) 크기
    assert_eq!(vsizes(&o)[2], (595, 842));
    assert_eq!(vsizes(&o)[5], (300, 200));
    // 원본은 그대로
    assert_eq!(labels(&a), vec!["P1", "P2", "P3", "P4"]);
    // 같은 이름이 있으면 덮어쓰지 않음
    assert!(matches!(plan::apply(&plan, &o, &mut Job::none()), Err(Error::AlreadyExists(_))));
}

#[test]
fn plan_rejects_bad_input() {
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    let bad = |pages: Vec<PlanPage>| EditPlan { sources: vec![PlanSource { path: a.clone(), password: None }], pages };
    let o = out(t.path(), "x.pdf");
    assert!(plan::apply(&bad(vec![]), &o, &mut Job::none()).is_err());
    assert!(plan::apply(&bad(vec![PlanPage::Page { src: 0, index: 9, rotate: 0 }]), &o, &mut Job::none()).is_err());
    assert!(plan::apply(&bad(vec![PlanPage::Page { src: 3, index: 0, rotate: 0 }]), &o, &mut Job::none()).is_err());
    assert!(plan::apply(&bad(vec![PlanPage::Page { src: 0, index: 0, rotate: 45 }]), &o, &mut Job::none()).is_err());
    assert!(!o.exists());
}

#[test]
fn plan_json_contract_parses() {
    let j = r#"{"sources":[{"path":"a.pdf"},{"path":"b.pdf","password":"1234"}],
               "pages":[{"kind":"page","src":0,"index":2,"rotate":90},{"kind":"blank","size":"a4"},
                        {"kind":"blank","size":{"w":100,"h":200}},{"kind":"page","src":1,"index":0}]}"#;
    let p: EditPlan = serde_json::from_str(j).unwrap();
    assert_eq!(p.pages.len(), 4);
    assert_eq!(p.sources[1].password.as_deref(), Some("1234"));
}

#[test]
fn extract_and_insert() {
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    let e = out(t.path(), "e.pdf");
    plan::extract(&a, None, "4,2", &e, &mut Job::none()).unwrap();
    assert_eq!(labels(&e), vec!["P4", "P2"]);

    let ib = out(t.path(), "ib.pdf");
    plan::insert_blank(&a, None, 1, 2, &BlankSize::Named("a4-landscape".into()), &ib, &mut Job::none()).unwrap();
    assert_eq!(labels(&ib), vec!["P1", "", "", "P2", "P3", "P4"]);
    assert_eq!(vsizes(&ib)[1], (842, 595));

    let b = t.path().join("b.pdf");
    testpdf::make_pages(&b, &[(595.0, 842.0, 0), (595.0, 842.0, 0), (595.0, 842.0, 0)], "Q").unwrap();
    let ip = out(t.path(), "ip.pdf");
    plan::insert_pdf(&a, None, 0, &b, None, Some("3,1"), &ip, &mut Job::none()).unwrap();
    assert_eq!(labels(&ip), vec!["Q3", "Q1", "P1", "P2", "P3", "P4"]);
    // 회전 정보도 따라옴(P3은 90도)
    assert_eq!(rotations(&ip)[4], 90);
}

// ---------- 쪽 미리보기 ----------

#[test]
fn page_infos_and_thumbnails() {
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    let info = thumbs::page_infos(&a, None).unwrap();
    assert_eq!(info.pages.len(), 4);
    assert_eq!(info.pages[2].rotate, 90);
    assert_eq!((info.pages[2].visual_width as i32, info.pages[2].visual_height as i32), (842, 595));
    if !pdfium_ok() {
        return;
    }
    let th = thumbs::thumbnails(&a, None, &[1, 3], 160, &mut |_| Ok(())).unwrap();
    assert_eq!(th.len(), 2);
    let img = image::load_from_memory(&th[1].png).unwrap();
    assert_eq!(img.width(), 160);
    // 3쪽은 90도 회전 → 가로로 긴 썸네일
    assert!(img.height() < 160, "{}", img.height());
    let img1 = image::load_from_memory(&th[0].png).unwrap();
    assert!(img1.height() > 200);
}

// ---------- 쪽 번호 ----------

#[test]
fn page_numbers_render_at_bottom_center_even_on_rotated_page() {
    if !pdfium_ok() || !font_ok() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    let o = out(t.path(), "num.pdf");
    let opts = PageNumberOpts { format: NumberFormat::OfTotal, skip_first: true, font_size: 14.0, ..Default::default() };
    let r = stamp::page_numbers(&a, None, &opts, &o, &mut Job::none()).unwrap();
    assert!(r.note.contains("1~3"), "{}", r.note);
    // 글자 추출: 2쪽 = "1 / 3", 4쪽 = "3 / 3", 1쪽에는 번호 없음
    assert!(thumbs::page_text(&o, None, 2).unwrap().contains("1 / 3"));
    assert!(thumbs::page_text(&o, None, 4).unwrap().contains("3 / 3"));
    assert!(!thumbs::page_text(&o, None, 1).unwrap().contains("/ 3"));
    // 3쪽(90도 회전, 보이는 크기 842×595): 아래 가운데에 어두운 화소, 위 가운데에는 없음
    let bottom = dark_in(&o, None, 3, 380.0, 595.0 - 50.0, 80.0, 30.0, 128);
    let top = dark_in(&o, None, 3, 380.0, 25.0, 80.0, 30.0, 128);
    let before = dark_in(&a, None, 3, 380.0, 595.0 - 50.0, 80.0, 30.0, 128);
    assert!(bottom > 20 && before == 0 && top == 0, "bottom {bottom} before {before} top {top}");
}

#[test]
fn page_numbers_top_right_dash_format() {
    if !pdfium_ok() || !font_ok() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    let o = out(t.path(), "num2.pdf");
    let opts = PageNumberOpts { position: Position::TopRight, format: NumberFormat::Dash, start: 5, ..Default::default() };
    stamp::page_numbers(&a, None, &opts, &o, &mut Job::none()).unwrap();
    assert!(thumbs::page_text(&o, None, 1).unwrap().contains("- 5 -"));
    // A4 세로 1쪽: 오른쪽 위(오른쪽 여백 28pt 안쪽, 위 28~45pt)
    assert!(dark_in(&o, None, 1, 595.0 - 80.0, 26.0, 55.0, 20.0, 128) > 10);
}

// ---------- 워터마크 ----------

#[test]
fn text_watermark_center_and_tile_with_opacity() {
    if !pdfium_ok() || !font_ok() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    let o = out(t.path(), "wm.pdf");
    let opts = WatermarkOpts {
        kind: WatermarkKind::Text { text: "대외비".into(), font_size: 120.0, color: "#FF0000".into() },
        opacity: 0.3,
        angle: 0.0,
        tiling: Tiling::Center,
        layer: Layer::Over,
        pages: None,
    };
    stamp::watermark(&a, None, &opts, &o, &mut Job::none()).unwrap();
    // 1쪽 가운데: 옅은 빨강(투명도 0.3 → 약 255, 178, 178)
    let img = thumbs::render_rgb(&o, None, 1, 1.0).unwrap();
    let mut reds = 0;
    for y in 380..460 {
        for x in 200..400 {
            let p = img.get_pixel(x, y).0;
            if p[0] > 230 && p[1] < 215 && p[1] > 150 && (p[1] as i32 - p[2] as i32).abs() < 12 {
                reds += 1;
            }
        }
    }
    assert!(reds > 500, "옅은 빨강 화소 {reds}");
    assert!(thumbs::page_text(&o, None, 1).unwrap().contains("대외비"));

    let o2 = out(t.path(), "wm_tile.pdf");
    let tile = WatermarkOpts { tiling: Tiling::Tile, angle: 45.0, kind: WatermarkKind::Text { text: "SAMPLE".into(), font_size: 24.0, color: "#0000FF".into() }, ..opts.clone() };
    stamp::watermark(&a, None, &tile, &o2, &mut Job::none()).unwrap();
    // 네 귀퉁이 근처 모두에 파란 기운
    let img = thumbs::render_rgb(&o2, None, 1, 1.0).unwrap();
    let blue_in = |x0: u32, y0: u32| (y0..y0 + 150).flat_map(|y| (x0..x0 + 150).map(move |x| (x, y))).filter(|&(x, y)| {
        let p = img.get_pixel(x, y).0;
        p[2] > 200 && p[0] < 225
    }).count();
    for (x0, y0) in [(30, 30), (400, 30), (30, 650), (400, 650)] {
        assert!(blue_in(x0, y0) > 30, "({x0},{y0}) {}", blue_in(x0, y0));
    }
}

#[test]
fn watermark_under_content_and_image_watermark() {
    if !pdfium_ok() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    // 그림 워터마크(빨간 PNG, 투명도 0.5, 가운데)
    let png = t.path().join("logo.png");
    image::RgbImage::from_pixel(200, 100, image::Rgb([255, 0, 0])).save(&png).unwrap();
    let o = out(t.path(), "wmi.pdf");
    let opts = WatermarkOpts { kind: WatermarkKind::Image { path: png.clone(), scale: 0.5 }, opacity: 0.5, angle: 0.0, tiling: Tiling::Center, layer: Layer::Under, pages: Some("1".into()) };
    stamp::watermark(&a, None, &opts, &o, &mut Job::none()).unwrap();
    let p = pixel(&o, 1, 297, 421);
    assert!(p[0] > 240 && (110..150).contains(&p[1]), "가운데 {p:?}");
    // 2쪽은 고르지 않음 → 그대로 흰색
    assert_eq!(pixel(&o, 2, 421, 297), [255, 255, 255]);
    // 아래에 깔았으므로 1쪽 글자(P1, 왼쪽 위)는 그대로 검정
    assert!(dark_in(&o, None, 1, 40.0, 50.0, 60.0, 40.0, 60) > 50);
}

// ---------- 서명·도장 ----------

#[test]
fn signature_image_lands_at_visual_spot_on_rotated_page_with_transparent_white() {
    if !pdfium_ok() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    let sig = t.path().join("sig.png");
    testpdf::make_signature_png(&sig).unwrap();
    let o = out(t.path(), "sig.pdf");
    // 3쪽(90도 회전): 보이는 쪽 오른쪽 아래(x 600, y 420, 너비 150)
    let opts = SignatureOpts { image: sig.clone(), placements: vec![ImagePlacement { page: 3, x: 600.0, y: 420.0, w: 150.0, h: 0.0 }], remove_white: true, opacity: 1.0 };
    stamp::place_image(&a, None, &opts, &o, &mut Job::none()).unwrap();
    let inside = dark_in(&o, None, 3, 600.0, 420.0, 150.0, 60.0, 80);
    let outside = dark_in(&o, None, 3, 100.0, 420.0, 150.0, 60.0, 80);
    assert!(inside > 200 && outside == 0, "inside {inside} outside {outside}");
    // 흰 배경을 투명하게 → SMask가 있음
    let d = load(&o);
    let has_smask = d.objects.values().any(|o| o.as_stream().map(|s| s.dict.has(b"SMask")).unwrap_or(false));
    assert!(has_smask);
    // 끄면 SMask 없음(PNG에 알파가 없으므로)
    let o2 = out(t.path(), "sig2.pdf");
    stamp::place_image(&a, None, &SignatureOpts { remove_white: false, ..opts }, &o2, &mut Job::none()).unwrap();
    let d2 = load(&o2);
    assert!(!d2.objects.values().any(|o| o.as_stream().map(|s| s.dict.has(b"SMask")).unwrap_or(false)));
}

// ---------- 글상자 ----------

#[test]
fn korean_text_box_with_background() {
    if !pdfium_ok() || !font_ok() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    let o = out(t.path(), "tb.pdf");
    let boxes = vec![
        TextBox { page: 1, x: 100.0, y: 200.0, text: "확인했습니다\n2026-11-01".into(), font_size: 18.0, color: "#000080".into(), background: Some("#FFFF00".into()), width: 0.0 },
        TextBox { page: 3, x: 50.0, y: 50.0, text: "회전 쪽".into(), font_size: 20.0, color: "#000000".into(), background: None, width: 0.0 },
        // QA-02: 화면에서 고른 빨강·아주 크게가 결과에 그대로
        TextBox { page: 2, x: 60.0, y: 100.0, text: "SIZE24TEST".into(), font_size: 24.0, color: "#C42B1C".into(), background: None, width: 0.0 },
    ];
    let r = stamp::text_boxes(&a, None, &boxes, &o, &mut Job::none()).unwrap();
    assert!(!r.note.contains("빈 네모"), "{}", r.note);
    let txt = thumbs::page_text(&o, None, 1).unwrap();
    assert!(txt.contains("확인했습니다") && txt.contains("2026-11-01"), "{txt}");
    // 노란 바탕 상자가 (100,200) 근처에 있음
    let p = pixel(&o, 1, 98, 202);
    assert!(p[0] > 240 && p[1] > 240 && p[2] < 30, "{p:?}");
    assert!(dark_in(&o, None, 1, 100.0, 200.0, 120.0, 25.0, 100) > 50);
    // 회전 쪽에서도 보이는 왼쪽 위
    assert!(dark_in(&o, None, 3, 50.0, 50.0, 80.0, 25.0, 100) > 30);
    assert!(thumbs::page_text(&o, None, 3).unwrap().contains("회전 쪽"));
    // 빨간 글자: 2쪽 (60,100) 근처에 빨간 점이 있고, 쪽 내용에 #C42B1C(0.769 0.169 0.11)로 칠함
    let img = thumbs::render_rgb(&o, None, 2, 1.0).unwrap();
    let mut red = 0;
    for y in (95..135).step_by(2) {
        for x in (60..220).step_by(2) {
            let p = img.get_pixel(x, y).0;
            if p[0] > 150 && p[1] < 110 && p[2] < 110 {
                red += 1;
            }
        }
    }
    assert!(red > 20, "빨간 점 {red}");
    let d = load(&o);
    let pid = *d.get_pages().get(&2).unwrap();
    let content = String::from_utf8_lossy(&d.get_page_content(pid)).into_owned();
    assert!(content.contains("0.769 0.169 0.11 rg") || content.contains("0.7686 0.1686 0.1098 rg"), "{content}");
    assert!(content.contains(" 24 Tf"), "{content}");
}

// ---------- 암호 ----------

#[test]
fn protect_with_open_password_aes256_and_aes128_then_unprotect() {
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    for (cipher, pw) in [(Cipher::Aes256, "한글암호1"), (Cipher::Aes128, "abc123")] {
        let o = out(t.path(), &format!("enc_{cipher:?}.pdf"));
        let opts = ProtectOpts { open_password: pw.into(), owner_password: "owner!".into(), allow_print: true, allow_copy: false, allow_edit: false, cipher };
        crypt::protect(&a, None, &opts, &o).unwrap();
        assert!(matches!(pdfedit::open(&o, None), Err(Error::PasswordRequired(_))));
        assert!(matches!(pdfedit::open(&o, Some("틀림")), Err(Error::WrongPassword(_))));
        let d = pdfedit::open(&o, Some(pw)).unwrap();
        assert_eq!(d.get_pages().len(), 4);
        // 소유자 암호로도 열림
        assert!(pdfedit::open(&o, Some("owner!")).is_ok());
        let st = crypt::protection(&o, Some(pw)).unwrap();
        assert!(st.needs_password && st.encrypted);
        assert_eq!((st.allow_print, st.allow_copy, st.allow_edit), (Some(true), Some(false), Some(false)));
        assert_eq!(st.method.as_deref(), Some(if cipher == Cipher::Aes256 { "AES-256" } else { "AES-128" }));
        if pdfium_ok() {
            // PDFium도 같은 암호로 열고, 없이 열면 암호 오류
            assert!(thumbs::render_rgb(&o, Some(pw), 1, 0.2).is_ok());
            assert!(matches!(thumbs::render_rgb(&o, None, 1, 0.2), Err(Error::PasswordRequired(_))));
        }
        let u = out(t.path(), &format!("dec_{cipher:?}.pdf"));
        assert!(matches!(crypt::unprotect(&o, "틀림", &u), Err(Error::WrongPassword(_))));
        crypt::unprotect(&o, pw, &u).unwrap();
        assert_eq!(labels(&u), vec!["P1", "P2", "P3", "P4"]);
        assert!(!crypt::protection(&u, None).unwrap().encrypted);
    }
}

#[test]
fn permissions_only_needs_owner_password_to_remove() {
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    let o = out(t.path(), "perm.pdf");
    let opts = ProtectOpts { open_password: String::new(), owner_password: "주인".into(), allow_print: false, allow_copy: true, allow_edit: true, cipher: Cipher::Aes256 };
    crypt::protect(&a, None, &opts, &o).unwrap();
    // 누구나 열림, 인쇄만 막힘
    let st = crypt::protection(&o, None).unwrap();
    assert!(!st.needs_password && st.encrypted);
    assert_eq!(st.allow_print, Some(false));
    let u = out(t.path(), "perm_open.pdf");
    assert!(matches!(crypt::unprotect(&o, "아무거나", &u), Err(Error::WrongPassword(_))));
    crypt::unprotect(&o, "주인", &u).unwrap();
    assert!(!crypt::protection(&u, None).unwrap().encrypted);
    // 아무 제한도, 암호도 없으면 거절
    let none = ProtectOpts { open_password: String::new(), owner_password: String::new(), allow_print: true, allow_copy: true, allow_edit: true, cipher: Cipher::Aes256 };
    assert!(crypt::protect(&a, None, &none, &out(t.path(), "n.pdf")).is_err());
}

#[test]
fn every_action_asks_for_password_on_encrypted_input() {
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    let enc = out(t.path(), "잠김.pdf");
    crypt::protect(&a, None, &ProtectOpts { open_password: "pw".into(), owner_password: "o".into(), allow_print: true, allow_copy: true, allow_edit: true, cipher: Cipher::Aes256 }, &enc).unwrap();
    let o = out(t.path(), "o.pdf");
    let pr = |r: Result<_, Error>| matches!(r, Err(Error::PasswordRequired(_)));
    let mut j = Job::none();
    assert!(pr(plan::extract(&enc, None, "1", &o, &mut j).map(|_| ())));
    assert!(pr(stamp::page_numbers(&enc, None, &PageNumberOpts::default(), &o, &mut j).map(|_| ())));
    assert!(pr(layout::fit_a4(&enc, None, &FitOpts::default(), &o, &mut j).map(|_| ())));
    assert!(pr(layout::nup(&enc, None, &NupOpts::default(), &o, &mut j).map(|_| ())));
    assert!(pr(info::read(&enc, None).map(|_| ())));
    assert!(pr(forms::list(&enc, None).map(|_| ())));
    assert!(pr(thumbs::page_infos(&enc, None).map(|_| ())));
    assert!(pr(crypt::protect(&enc, None, &ProtectOpts { open_password: "x".into(), owner_password: String::new(), allow_print: true, allow_copy: true, allow_edit: true, cipher: Cipher::Aes256 }, &o).map(|_| ())));
    // 오류 문구는 쉬운 한국어
    let msg = plan::extract(&enc, None, "1", &o, &mut j).unwrap_err().to_string();
    assert!(msg.starts_with("암호가 걸린 PDF입니다"), "{msg}");
    // 암호를 주면 됨, 결과는 암호 없이 저장
    plan::extract(&enc, Some("pw"), "2", &o, &mut j).unwrap();
    assert_eq!(labels(&o), vec!["P2"]);
    // 예전 방식(RC4) 암호 PDF도 열림
    let rc4 = out(t.path(), "rc4.pdf");
    filetool_core::samples::make_encrypted_pdf(&rc4, "user").unwrap();
    assert!(matches!(pdfedit::open(&rc4, None), Err(Error::PasswordRequired(_))));
    assert_eq!(pdfedit::open(&rc4, Some("user")).unwrap().get_pages().len(), 2);
}

// ---------- 여백·A4·모아 찍기 ----------

#[test]
fn crop_margins_and_auto_including_rotated_page() {
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    let o = out(t.path(), "crop.pdf");
    let opts = CropOpts { mode: CropMode::Margins { top: 10.0, right: 20.0, bottom: 30.0, left: 40.0 }, pages: Some("1,3".into()) };
    layout::crop(&a, None, &opts, &o, &mut Job::none()).unwrap();
    let d = load(&o);
    let ids: Vec<_> = d.get_pages().into_values().collect();
    let g1 = pdfedit::page_geom(&d, ids[0]);
    assert_eq!(g1.bx.map(|v| v.round() as i32), [40, 30, 575, 832]);
    // 3쪽(90도): 보이는 위 10 → 사용자 공간 왼쪽 10, 보이는 오른쪽 20 → 사용자 위 20 …
    let g3 = pdfedit::page_geom(&d, ids[2]);
    assert_eq!(g3.bx.map(|v| v.round() as i32), [10, 40, 565, 822]);
    assert_eq!(g3.visual_size(), (842.0 - 60.0, 595.0 - 40.0));
    // 2쪽은 그대로
    assert_eq!(pdfedit::page_geom(&d, ids[1]).bx.map(|v| v.round() as i32), [0, 0, 842, 595]);
    if !pdfium_ok() {
        return;
    }
    let o2 = out(t.path(), "crop_auto.pdf");
    layout::crop(&a, None, &CropOpts { mode: CropMode::Auto { padding: 0.0 }, pages: None }, &o2, &mut Job::none()).unwrap();
    let d2 = load(&o2);
    let ids2: Vec<_> = d2.get_pages().into_values().collect();
    // 내용 = 테두리 선(20~22pt 안쪽) → 남는 상자 ≈ [20, 20, w-20, h-20]
    let g = pdfedit::page_geom(&d2, ids2[0]).bx;
    for (got, want) in g.iter().zip([20.0, 20.0, 575.0, 822.0]) {
        assert!((got - want).abs() < 3.0, "{g:?}");
    }
    let g3 = pdfedit::page_geom(&d2, ids2[2]).bx;
    for (got, want) in g3.iter().zip([20.0, 20.0, 575.0, 822.0]) {
        assert!((got - want).abs() < 3.0, "rot {g3:?}");
    }
}

#[test]
fn fit_to_a4_keeps_visual_orientation() {
    if !pdfium_ok() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    let o = out(t.path(), "a4.pdf");
    layout::fit_a4(&a, None, &FitOpts::default(), &o, &mut Job::none()).unwrap();
    // 자동: 가로로 보이는 쪽(2, 3쪽)은 가로 A4
    assert_eq!(vsizes(&o), vec![(595, 842), (842, 595), (842, 595), (595, 842)]);
    assert_eq!(rotations(&o), vec![0, 0, 0, 0]);
    // 3쪽: 원래 90도 회전해 보이던 모양 그대로 → 글자 P3이 보이는 쪽 오른쪽 위에(회전 전 왼쪽 위)
    assert!(thumbs::page_text(&o, None, 3).unwrap().contains("P3"));
    let rot_before = thumbs::render_rgb(&a, None, 3, 1.0).unwrap();
    let rot_after = thumbs::render_rgb(&o, None, 3, 1.0).unwrap();
    assert_eq!(rot_before.dimensions(), rot_after.dimensions());
    let diff = rot_before.pixels().zip(rot_after.pixels()).filter(|(x, y)| x.0.iter().zip(y.0.iter()).any(|(a, b)| (*a as i32 - *b as i32).abs() > 60)).count();
    assert!(diff < 400, "같은 모양이어야 함: 다른 화소 {diff}");
    // 4쪽(A5) → A4로 키움: 글자도 커짐(테두리가 A4 가장자리 근처로)
    assert!(dark_in(&o, None, 4, 0.0, 400.0, 40.0, 40.0, 200) > 5);
    // 세로 고정
    let o2 = out(t.path(), "a4p.pdf");
    layout::fit_a4(&a, None, &FitOpts { orientation: Orientation::Portrait, margin: 20.0, enlarge: false }, &o2, &mut Job::none()).unwrap();
    assert!(vsizes(&o2).iter().all(|s| *s == (595, 842)));
}

#[test]
fn nup_two_and_four_per_sheet() {
    if !pdfium_ok() {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("five.pdf");
    testpdf::make_pages(&a, &[(595.0, 842.0, 0); 5], "P").unwrap();
    let o = out(t.path(), "n2.pdf");
    let r = layout::nup(&a, None, &NupOpts::default(), &o, &mut Job::none()).unwrap();
    assert_eq!(r.pages, 3);
    assert_eq!(vsizes(&o), vec![(842, 595); 3]);
    // 1장: 왼쪽에 P1, 오른쪽에 P2
    let txt = thumbs::page_text(&o, None, 1).unwrap();
    assert!(txt.contains("P1") && txt.contains("P2"), "{txt}");
    assert!(dark_in(&o, None, 1, 30.0, 40.0, 100.0, 40.0, 60) > 30);
    assert!(dark_in(&o, None, 1, 450.0, 40.0, 100.0, 40.0, 60) > 30);
    let o4 = out(t.path(), "n4.pdf");
    let r4 = layout::nup(&a, None, &NupOpts { per_sheet: 4, order: NupOrder::Down, ..Default::default() }, &o4, &mut Job::none()).unwrap();
    assert_eq!(r4.pages, 2);
    assert_eq!(vsizes(&o4), vec![(595, 842); 2]);
    // 세로 먼저: 왼쪽 위 P1, 왼쪽 아래 P2, 오른쪽 위 P3
    let img = thumbs::render_rgb(&o4, None, 1, 1.0).unwrap();
    let _ = img;
    assert!(dark_in(&o4, None, 1, 20.0, 20.0, 80.0, 40.0, 60) > 20, "왼쪽 위");
    assert!(dark_in(&o4, None, 1, 20.0, 440.0, 80.0, 40.0, 60) > 20, "왼쪽 아래");
    assert!(dark_in(&o4, None, 1, 310.0, 20.0, 80.0, 40.0, 60) > 20, "오른쪽 위");
    assert!(dark_in(&o4, None, 1, 310.0, 440.0, 80.0, 40.0, 60) > 20, "오른쪽 아래");
    assert!(dark_in(&o4, None, 2, 310.0, 20.0, 80.0, 40.0, 60) == 0, "2장에는 P5 하나");
}

// ---------- 문서 정보 ----------

#[test]
fn doc_info_set_read_clear() {
    let t = tempfile::tempdir().unwrap();
    let a = mixed(t.path());
    let r0 = info::read(&a, None).unwrap();
    assert_eq!(r0.info.title.as_deref(), Some("Sample"));
    assert_eq!(r0.xmp_bytes, None);
    let o = out(t.path(), "info.pdf");
    let upd = DocInfo { title: Some("분기 보고서".into()), author: Some("홍길동".into()), keywords: Some("보고, 3분기".into()), producer: Some(String::new()), ..Default::default() };
    info::set(&a, None, &upd, &o).unwrap();
    let r = info::read(&o, None).unwrap();
    assert_eq!(r.info.title.as_deref(), Some("분기 보고서"));
    assert_eq!(r.info.author.as_deref(), Some("홍길동"));
    assert_eq!(r.info.producer, None, "빈 값 = 지움");
    assert!(r.xmp_bytes.unwrap() > 100);
    // XMP에도 같은 제목
    let d = load(&o);
    let m = d.catalog().unwrap().get(b"Metadata").unwrap().as_reference().unwrap();
    let xmp = String::from_utf8(d.get_object(m).unwrap().as_stream().unwrap().content.clone()).unwrap();
    assert!(xmp.contains("분기 보고서") && xmp.contains("홍길동"), "{xmp}");
    let c = out(t.path(), "clear.pdf");
    info::clear(&o, None, &c).unwrap();
    let rc = info::read(&c, None).unwrap();
    assert_eq!(rc.info, DocInfo::default());
    assert_eq!(rc.xmp_bytes, None);
    assert!(load(&c).trailer.get(b"Info").is_err());
}

// ---------- 빈 파일(QA-05) ----------

#[test]
fn empty_file_is_reported_as_empty() {
    let t = tempfile::tempdir().unwrap();
    let e = t.path().join("빈파일.pdf");
    std::fs::write(&e, b"").unwrap();
    assert!(matches!(pdfedit::open(&e, None), Err(Error::EmptyFile(_))));
    assert!(matches!(filetool_core::pdf::load(&e), Err(Error::EmptyFile(_))));
    let msg = pdfedit::open(&e, None).unwrap_err().to_string();
    assert!(msg.contains("빈 파일입니다"), "{msg}");
    let jpg = t.path().join("빈사진.jpg");
    std::fs::write(&jpg, b"").unwrap();
    let opts: filetool_core::imageconv::ImageOpts = serde_json::from_str(r#"{"format":"png"}"#).unwrap();
    assert!(matches!(filetool_core::imageconv::convert_raster(&jpg, None, &opts), Err(Error::EmptyFile(_))));
    // 내용이 있으면 빈 파일 오류가 아님
    std::fs::write(&e, b"not a pdf").unwrap();
    assert!(!matches!(pdfedit::open(&e, None), Err(Error::EmptyFile(_))));
}

// ---------- 양식 ----------

#[test]
fn form_list_fill_flatten() {
    let t = tempfile::tempdir().unwrap();
    let f = t.path().join("form.pdf");
    testpdf::make_form(&f).unwrap();
    let fields = forms::list(&f, None).unwrap();
    let kinds: Vec<(String, String)> = fields.iter().map(|x| (x.name.clone(), x.kind.clone())).collect();
    assert_eq!(
        kinds,
        vec![
            ("name".into(), "text".into()),
            ("agree".into(), "checkbox".into()),
            ("size".into(), "radio".into()),
            ("city".into(), "combo".into()),
            ("color".into(), "list".into())
        ]
    );
    assert_eq!(fields[2].options, vec!["S", "M"]);
    assert_eq!(fields[3].options, vec!["Seoul", "Busan", "서울"]);
    assert_eq!(fields[1].value, "Off");
    assert_eq!(fields[0].page, Some(1));
    if !font_ok() {
        return;
    }
    let mut v = BTreeMap::new();
    v.insert("name".to_string(), "홍길동".to_string());
    v.insert("agree".to_string(), "true".to_string());
    v.insert("size".to_string(), "M".to_string());
    v.insert("city".to_string(), "서울".to_string());
    v.insert("color".to_string(), "Blue".to_string());
    let o = out(t.path(), "filled.pdf");
    forms::fill(&f, None, &v, false, &o, &mut Job::none()).unwrap();
    let after = forms::list(&o, None).unwrap();
    let vals: Vec<&str> = after.iter().map(|x| x.value.as_str()).collect();
    assert_eq!(vals, vec!["홍길동", "Yes", "M", "서울", "Blue"]);
    // 잘못된 값·이름
    let mut bad = BTreeMap::new();
    bad.insert("size".to_string(), "XL".to_string());
    assert!(forms::fill(&f, None, &bad, false, &out(t.path(), "b1.pdf"), &mut Job::none()).is_err());
    let mut bad2 = BTreeMap::new();
    bad2.insert("없는칸".to_string(), "x".to_string());
    assert!(forms::fill(&f, None, &bad2, false, &out(t.path(), "b2.pdf"), &mut Job::none()).is_err());

    // QA-01: 화면이 보내는 JSON(체크 상자 boolean, 글자 칸 문자열) 그대로 받아 채운다
    let json: BTreeMap<String, forms::FormValue> =
        serde_json::from_str(r#"{"name":"QA","agree":false,"size":"S","city":"Seoul","color":""}"#).unwrap();
    let off = out(t.path(), "json_off.pdf");
    forms::fill(&f, None, &forms::text_values(json), false, &off, &mut Job::none()).unwrap();
    let vals: Vec<String> = forms::list(&off, None).unwrap().into_iter().map(|x| x.value).collect();
    assert_eq!(vals, vec!["QA", "Off", "S", "Seoul", ""]);
    let json: BTreeMap<String, forms::FormValue> = serde_json::from_str(r#"{"name":"QA","agree":true}"#).unwrap();
    let on = out(t.path(), "json_on.pdf");
    forms::fill(&o, None, &forms::text_values(json), false, &on, &mut Job::none()).unwrap();
    let after: Vec<String> = forms::list(&on, None).unwrap().into_iter().map(|x| x.value).collect();
    assert_eq!(after[..2], ["QA".to_string(), "Yes".to_string()]);
    // 화면이 문자열로 보내는 체크 상자 값("true"/"false")
    let json: BTreeMap<String, forms::FormValue> = serde_json::from_str(r#"{"agree":"false"}"#).unwrap();
    let s_off = out(t.path(), "json_str_off.pdf");
    forms::fill(&on, None, &forms::text_values(json), false, &s_off, &mut Job::none()).unwrap();
    assert_eq!(forms::list(&s_off, None).unwrap()[1].value, "Off");

    // 평평하게: 양식 없어짐, 링크는 남음
    let fl = out(t.path(), "flat.pdf");
    let r = forms::flatten(&o, None, &fl, &mut Job::none()).unwrap();
    assert!(r.note.contains('7'), "{}", r.note); // 위젯 6 + 메모 1
    let d = load(&fl);
    assert!(d.catalog().unwrap().get(b"AcroForm").is_err());
    let pid = *d.get_pages().get(&1).unwrap();
    let annots = d.get_dictionary(pid).unwrap().get(b"Annots").unwrap().as_array().unwrap().clone();
    assert_eq!(annots.len(), 1);
    let a0 = d.get_dictionary(annots[0].as_reference().unwrap()).unwrap();
    assert_eq!(a0.get(b"Subtype").unwrap().as_name().unwrap(), b"Link");
    assert!(forms::list(&fl, None).unwrap().is_empty());
    if !pdfium_ok() {
        return;
    }
    // 렌더: 글자 칸의 홍길동, 체크 상자 속 검정, 라디오 M(오른쪽) 켜짐, 메모의 빨강
    let txt = thumbs::page_text(&fl, None, 1).unwrap();
    assert!(txt.contains("홍길동") && txt.contains("서울"), "{txt}");
    let h = 842.0;
    assert!(dark_in(&fl, None, 1, 105.0, h - 666.0, 10.0, 10.0, 60) > 50, "체크");
    assert!(dark_in(&fl, None, 1, 146.0, h - 614.0, 8.0, 8.0, 60) > 30, "라디오 M");
    assert!(dark_in(&fl, None, 1, 106.0, h - 614.0, 8.0, 8.0, 60) == 0, "라디오 S 꺼짐");
    let p = pixel(&fl, 1, 415, (h - 415.0) as u32);
    assert!(p[0] > 200 && p[1] < 50, "메모 {p:?}");

    // 채우면서 바로 평평하게
    let both = out(t.path(), "fill_flat.pdf");
    forms::fill(&f, None, &v, true, &both, &mut Job::none()).unwrap();
    assert!(load(&both).catalog().unwrap().get(b"AcroForm").is_err());
    assert!(thumbs::page_text(&both, None, 1).unwrap().contains("홍길동"));
}

// ---------- 취소·진행률 ----------

#[test]
fn cancel_stops_without_output_and_progress_reports() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("many.pdf");
    testpdf::make_many(&a, 30).unwrap();
    let c = Cancel::new();
    c.cancel();
    let mut ticks = 0;
    let mut cb = |_i: u32, _n: u32| ticks += 1;
    let o = out(t.path(), "c.pdf");
    let r = layout::nup(&a, None, &NupOpts::default(), &o, &mut Job::new(Some(&c), &mut cb));
    assert!(matches!(r, Err(Error::Cancelled)));
    assert!(!o.exists());
    let mut seen = Vec::new();
    let mut cb2 = |i: u32, n: u32| seen.push((i, n));
    let o2 = out(t.path(), "p.pdf");
    layout::nup(&a, None, &NupOpts { per_sheet: 4, ..Default::default() }, &o2, &mut Job::new(None, &mut cb2)).unwrap();
    assert_eq!(seen.first(), Some(&(0, 8)));
    assert_eq!(seen.last(), Some(&(8, 8)));
    let _ = Object::Null;
}
