//! 0.3.0 PDF 편집 명령(코어 `filetool_core::pdfedit`). 목록·정보 명령은 JSON으로 출력한다.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use filetool_core::pdfedit::crypt::{self, Cipher, ProtectOpts};
use filetool_core::pdfedit::forms;
use filetool_core::pdfedit::info::{self, DocInfo};
use filetool_core::pdfedit::layout::{self, CropMode, CropOpts, FitOpts, NupOpts, NupOrder};
use filetool_core::pdfedit::plan::{self, BlankSize, EditPlan};
use filetool_core::pdfedit::stamp::{self, ImagePlacement, Layer, NumberFormat, PageNumberOpts, Position, SignatureOpts, TextBox, Tiling, WatermarkKind, WatermarkOpts};
use filetool_core::pdfedit::{self, default_output, thumbs, EditOut, Job, Orientation};
use filetool_core::{Error, Result};

pub const USAGE: &str = "0.3.0 PDF 편집 (공통: --password 암호  --out 폴더  --output 파일.pdf)
  filetool-cli pdf-pages <pdf>                                    쪽 크기·회전(JSON)
  filetool-cli pdf-thumbs <pdf> [--width 160] [--pages 1-3] [--out 폴더] [--bench]   쪽 미리보기 PNG / 속도 재기
  filetool-cli pdf-plan <계획.json> --output <파일.pdf>            쪽 편집 계획 적용(docs/design.md 0.3.0)
  filetool-cli pdf-extract <pdf> <쪽 1-3,5>                       고른 쪽만 새 PDF
  filetool-cli pdf-insert-blank <pdf> <뒤쪽번호(0=맨 앞)> [--count 1] [--size same|a4|a4-landscape|letter|WxH]
  filetool-cli pdf-insert <pdf> <뒤쪽번호> <다른.pdf> [--other-pages 1-3] [--other-password 암호]
  filetool-cli pdf-number <pdf> [--position bottom-center] [--start 1] [--format plain|of-total|dash] [--size 10] [--margin-x 28] [--margin-y 28] [--skip-first] [--color #000000] [--pages 2-]
  filetool-cli pdf-watermark <pdf> (--text 글자 [--size 48] [--color #808080] | --image 그림 [--scale 0.5]) [--opacity 0.3] [--angle 45] [--tile] [--under] [--pages]
  filetool-cli pdf-protect <pdf> [--open-password 암호] [--owner-password 암호] [--no-print] [--no-copy] [--no-edit] [--aes128]
  filetool-cli pdf-unprotect <pdf> --password <암호>
  filetool-cli pdf-protection <pdf>                               암호·권한 상태(JSON)
  filetool-cli pdf-sign <pdf> <그림> --page N --x X --y Y --w W [--h H] [--remove-white] [--opacity 1]   (보이는 쪽 왼쪽 위 기준 pt)
  filetool-cli pdf-textbox <pdf> --page N --x X --y Y --text 글자 [--size 12] [--color #000000] [--bg #FFFFFF] [--width 0]
  filetool-cli pdf-crop <pdf> (--auto [--padding 8] | --margins 위,오른쪽,아래,왼쪽) [--pages]
  filetool-cli pdf-fit-a4 <pdf> [--orientation auto|portrait|landscape] [--margin 0] [--no-enlarge]
  filetool-cli pdf-nup <pdf> [--per 2|4] [--orientation auto|portrait|landscape] [--order across|down] [--gap 8] [--margin 14]
  filetool-cli pdf-meta <pdf>                                     문서 정보(JSON)
  filetool-cli pdf-meta-set <pdf> [--title] [--author] [--subject] [--keywords] [--creator] [--producer]   (빈 값 = 지움)
  filetool-cli pdf-meta-clear <pdf>                               문서 정보·XMP 지우기
  filetool-cli pdf-form-list <pdf>                                양식 칸(JSON)
  filetool-cli pdf-form-fill <pdf> 이름=값 ... [--flatten]
  filetool-cli pdf-flatten <pdf>                                  양식·주석 평평하게
  filetool-cli pdfedit-selftest <빈 폴더>                         샘플을 만들어 모든 편집 작업 실행 + 전/후 그림(evidence/)";

const SWITCHES: [&str; 11] = ["--bench", "--skip-first", "--tile", "--under", "--no-print", "--no-copy", "--no-edit", "--aes128", "--remove-white", "--auto", "--no-enlarge"];
const SWITCHES2: [&str; 1] = ["--flatten"];

struct Args {
    opts: Vec<(String, Option<String>)>,
    rest: Vec<String>,
}

impl Args {
    fn parse(args: &[String]) -> Self {
        let mut opts = Vec::new();
        let mut rest = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a.starts_with("--") && a.len() > 2 {
                if SWITCHES.contains(&a.as_str()) || SWITCHES2.contains(&a.as_str()) {
                    opts.push((a.clone(), None));
                } else {
                    opts.push((a.clone(), args.get(i + 1).cloned()));
                    i += 1;
                }
            } else {
                rest.push(a.clone());
            }
            i += 1;
        }
        Args { opts, rest }
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.opts.iter().rev().find(|(n, _)| n == k).and_then(|(_, v)| v.as_deref())
    }
    fn has(&self, k: &str) -> bool {
        self.opts.iter().any(|(n, _)| n == k)
    }
    fn num<T: std::str::FromStr>(&self, k: &str, default: T) -> Result<T> {
        match self.get(k) {
            None => Ok(default),
            Some(v) => v.parse().map_err(|_| Error::Invalid(format!("{k} 값이 숫자가 아닙니다: {v}"))),
        }
    }
    fn req<T: std::str::FromStr>(&self, k: &str) -> Result<T> {
        let v = self.get(k).ok_or_else(|| Error::Invalid(format!("{k}를 넣어 주세요")))?;
        v.parse().map_err(|_| Error::Invalid(format!("{k} 값이 숫자가 아닙니다: {v}")))
    }
    fn pos(&self, i: usize) -> Result<&str> {
        self.rest.get(i).map(String::as_str).ok_or_else(|| Error::Invalid(format!("인자가 부족합니다\n{USAGE}")))
    }
    fn pw(&self) -> Option<&str> {
        self.get("--password")
    }
    fn output(&self, input: &Path, suffix: &str) -> PathBuf {
        match self.get("--output") {
            Some(o) => PathBuf::from(o),
            None => default_output(input, self.get("--out").map(Path::new), suffix),
        }
    }
}

fn orientation(s: Option<&str>) -> Result<Orientation> {
    Ok(match s.unwrap_or("auto") {
        "auto" => Orientation::Auto,
        "portrait" => Orientation::Portrait,
        "landscape" => Orientation::Landscape,
        o => return Err(Error::Invalid(format!("--orientation은 auto|portrait|landscape ({o})"))),
    })
}

fn blank_size(s: Option<&str>) -> Result<BlankSize> {
    let s = s.unwrap_or("same");
    if let Some((w, h)) = s.split_once('x') {
        if let (Ok(w), Ok(h)) = (w.parse(), h.parse()) {
            return Ok(BlankSize::Custom { w, h });
        }
    }
    Ok(BlankSize::Named(s.to_string()))
}

fn done(r: EditOut) -> ExitCode {
    println!("완료  {}  ({}쪽) {}", r.output.display(), r.pages, r.note);
    ExitCode::SUCCESS
}

fn json<T: filetool_core::pdfedit::Serialize>(v: &T) -> ExitCode {
    println!("{}", pdfedit::to_json(v));
    ExitCode::SUCCESS
}

pub fn run(cmd: &str, args: &[String]) -> Result<Option<ExitCode>> {
    if cmd == "help-pdfedit" {
        println!("{USAGE}");
        return Ok(Some(ExitCode::SUCCESS));
    }
    if cmd == "pdfedit-selftest" {
        let dir = PathBuf::from(args.get(1).ok_or_else(|| Error::Invalid("빈 폴더를 주세요".into()))?);
        return selftest(&dir).map(Some);
    }
    if !cmd.starts_with("pdf-") {
        return Ok(None);
    }
    let a = Args::parse(&args[1.min(args.len())..]);
    let mut progress = |i: u32, n: u32| {
        if n >= 20 && (i % 10 == 0 || i == n) {
            eprintln!("  진행 {i}/{n}");
        }
    };
    let mut job = Job::new(None, &mut progress);
    let code = match cmd {
        "pdf-pages" => json(&thumbs::page_infos(Path::new(a.pos(0)?), a.pw())?),
        "pdf-thumbs" => {
            let input = PathBuf::from(a.pos(0)?);
            let width: u32 = a.num("--width", 160)?;
            if a.has("--bench") {
                let (n, ms, open_ms) = thumbs::bench(&input, a.pw(), width)?;
                println!("쪽 {n} · 너비 {width}px · 쪽당 {ms:.1} ms · 문서 열기 {open_ms:.1} ms · 합계 {:.0} ms", ms * n as f64 + open_ms);
                return Ok(Some(ExitCode::SUCCESS));
            }
            let n = thumbs::page_count(&input, a.pw())?;
            let pages: Vec<u32> = match a.get("--pages") {
                Some(s) => filetool_core::pages::parse_list(s, n)?,
                None => (1..=n).collect(),
            };
            let dir = filetool_core::pdfrender::output_folder(&input, a.get("--out").map(Path::new), "_미리보기");
            std::fs::create_dir_all(&dir).map_err(|e| Error::Io { path: dir.clone(), source: e })?;
            thumbs::thumbnails(&input, a.pw(), &pages, width, &mut |t| {
                let p = dir.join(format!("p{:03}.png", t.page));
                filetool_core::paths::write_new(&p, &t.png)?;
                println!("{}쪽  {}×{}  {:.1} ms  {}", t.page, t.width, t.height, t.ms, p.display());
                Ok(())
            })?;
            ExitCode::SUCCESS
        }
        "pdf-plan" => {
            let p = PathBuf::from(a.pos(0)?);
            let text = std::fs::read_to_string(&p).map_err(|e| Error::Io { path: p.clone(), source: e })?;
            let plan: EditPlan = pdfedit::from_json(&text)?;
            let first = plan.sources.first().map(|s| s.path.clone()).unwrap_or_else(|| p.clone());
            let out = a.output(&first, "_쪽편집");
            done(plan::apply(&plan, &out, &mut job)?)
        }
        "pdf-extract" => {
            let input = PathBuf::from(a.pos(0)?);
            done(plan::extract(&input, a.pw(), a.pos(1)?, &a.output(&input, "_쪽뽑기"), &mut job)?)
        }
        "pdf-insert-blank" => {
            let input = PathBuf::from(a.pos(0)?);
            let after: u32 = a.pos(1)?.parse().map_err(|_| Error::Invalid("뒤쪽 번호는 숫자로".into()))?;
            done(plan::insert_blank(&input, a.pw(), after, a.num("--count", 1)?, &blank_size(a.get("--size"))?, &a.output(&input, "_빈쪽추가"), &mut job)?)
        }
        "pdf-insert" => {
            let input = PathBuf::from(a.pos(0)?);
            let after: u32 = a.pos(1)?.parse().map_err(|_| Error::Invalid("뒤쪽 번호는 숫자로".into()))?;
            let other = PathBuf::from(a.pos(2)?);
            done(plan::insert_pdf(&input, a.pw(), after, &other, a.get("--other-password"), a.get("--other-pages"), &a.output(&input, "_끼워넣기"), &mut job)?)
        }
        "pdf-number" => {
            let input = PathBuf::from(a.pos(0)?);
            let mut o = PageNumberOpts::default();
            if let Some(p) = a.get("--position") {
                o.position = Position::parse(p).ok_or_else(|| Error::Invalid(format!("--position: top-left|top-center|top-right|bottom-left|bottom-center|bottom-right ({p})")))?;
            }
            o.format = match a.get("--format").unwrap_or("plain") {
                "plain" => NumberFormat::Plain,
                "of-total" => NumberFormat::OfTotal,
                "dash" => NumberFormat::Dash,
                f => return Err(Error::Invalid(format!("--format: plain|of-total|dash ({f})"))),
            };
            o.start = a.num("--start", 1)?;
            o.font_size = a.num("--size", 10.0)?;
            o.margin_x = a.num("--margin-x", o.margin_x)?;
            o.margin_y = a.num("--margin-y", o.margin_y)?;
            o.skip_first = a.has("--skip-first");
            if let Some(c) = a.get("--color") {
                o.color = c.into();
            }
            o.pages = a.get("--pages").map(String::from);
            done(stamp::page_numbers(&input, a.pw(), &o, &a.output(&input, "_쪽번호"), &mut job)?)
        }
        "pdf-watermark" => {
            let input = PathBuf::from(a.pos(0)?);
            let kind = match (a.get("--text"), a.get("--image")) {
                (Some(t), None) => WatermarkKind::Text { text: t.into(), font_size: a.num("--size", 48.0)?, color: a.get("--color").unwrap_or("#808080").into() },
                (None, Some(i)) => WatermarkKind::Image { path: PathBuf::from(i), scale: a.num("--scale", 0.5)? },
                _ => return Err(Error::Invalid("--text 또는 --image 중 하나를 주세요".into())),
            };
            let o = WatermarkOpts {
                kind,
                opacity: a.num("--opacity", 0.3)?,
                angle: a.num("--angle", 45.0)?,
                tiling: if a.has("--tile") { Tiling::Tile } else { Tiling::Center },
                layer: if a.has("--under") { Layer::Under } else { Layer::Over },
                pages: a.get("--pages").map(String::from),
            };
            done(stamp::watermark(&input, a.pw(), &o, &a.output(&input, "_워터마크"), &mut job)?)
        }
        "pdf-protect" => {
            let input = PathBuf::from(a.pos(0)?);
            let o = ProtectOpts {
                open_password: a.get("--open-password").unwrap_or("").into(),
                owner_password: a.get("--owner-password").unwrap_or("").into(),
                allow_print: !a.has("--no-print"),
                allow_copy: !a.has("--no-copy"),
                allow_edit: !a.has("--no-edit"),
                cipher: if a.has("--aes128") { Cipher::Aes128 } else { Cipher::Aes256 },
            };
            done(crypt::protect(&input, a.pw(), &o, &a.output(&input, "_암호"))?)
        }
        "pdf-unprotect" => {
            let input = PathBuf::from(a.pos(0)?);
            done(crypt::unprotect(&input, a.pw().unwrap_or(""), &a.output(&input, "_암호풀림"))?)
        }
        "pdf-protection" => json(&crypt::protection(Path::new(a.pos(0)?), a.pw())?),
        "pdf-sign" => {
            let input = PathBuf::from(a.pos(0)?);
            let o = SignatureOpts {
                image: PathBuf::from(a.pos(1)?),
                placements: vec![ImagePlacement { page: a.req("--page")?, x: a.req("--x")?, y: a.req("--y")?, w: a.req("--w")?, h: a.num("--h", 0.0)? }],
                remove_white: a.has("--remove-white"),
                opacity: a.num("--opacity", 1.0)?,
            };
            done(stamp::place_image(&input, a.pw(), &o, &a.output(&input, "_서명"), &mut job)?)
        }
        "pdf-textbox" => {
            let input = PathBuf::from(a.pos(0)?);
            let b = TextBox {
                page: a.req("--page")?,
                x: a.req("--x")?,
                y: a.req("--y")?,
                text: a.get("--text").ok_or_else(|| Error::Invalid("--text를 넣어 주세요".into()))?.replace("\\n", "\n"),
                font_size: a.num("--size", 12.0)?,
                color: a.get("--color").unwrap_or("#000000").into(),
                background: a.get("--bg").map(String::from),
                width: a.num("--width", 0.0)?,
            };
            done(stamp::text_boxes(&input, a.pw(), &[b], &a.output(&input, "_글상자"), &mut job)?)
        }
        "pdf-crop" => {
            let input = PathBuf::from(a.pos(0)?);
            let mode = if a.has("--auto") {
                CropMode::Auto { padding: a.num("--padding", 8.0)? }
            } else {
                let m = a.get("--margins").ok_or_else(|| Error::Invalid("--auto 또는 --margins 위,오른쪽,아래,왼쪽".into()))?;
                let v: Vec<f32> = m.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                if v.len() != 4 {
                    return Err(Error::Invalid("--margins는 숫자 4개(위,오른쪽,아래,왼쪽 pt)".into()));
                }
                CropMode::Margins { top: v[0], right: v[1], bottom: v[2], left: v[3] }
            };
            done(layout::crop(&input, a.pw(), &CropOpts { mode, pages: a.get("--pages").map(String::from) }, &a.output(&input, "_여백자름"), &mut job)?)
        }
        "pdf-fit-a4" => {
            let input = PathBuf::from(a.pos(0)?);
            let o = FitOpts { orientation: orientation(a.get("--orientation"))?, margin: a.num("--margin", 0.0)?, enlarge: !a.has("--no-enlarge") };
            done(layout::fit_a4(&input, a.pw(), &o, &a.output(&input, "_A4"), &mut job)?)
        }
        "pdf-nup" => {
            let input = PathBuf::from(a.pos(0)?);
            let o = NupOpts {
                per_sheet: a.num("--per", 2)?,
                orientation: orientation(a.get("--orientation"))?,
                order: match a.get("--order").unwrap_or("across") {
                    "across" => NupOrder::Across,
                    "down" => NupOrder::Down,
                    o => return Err(Error::Invalid(format!("--order: across|down ({o})"))),
                },
                gap: a.num("--gap", 8.0)?,
                margin: a.num("--margin", 14.0)?,
            };
            done(layout::nup(&input, a.pw(), &o, &a.output(&input, "_모아찍기"), &mut job)?)
        }
        "pdf-meta" => json(&info::read(Path::new(a.pos(0)?), a.pw())?),
        "pdf-meta-set" => {
            let input = PathBuf::from(a.pos(0)?);
            let g = |k: &str| a.get(k).map(String::from);
            let u = DocInfo { title: g("--title"), author: g("--author"), subject: g("--subject"), keywords: g("--keywords"), creator: g("--creator"), producer: g("--producer") };
            done(info::set(&input, a.pw(), &u, &a.output(&input, "_정보수정"))?)
        }
        "pdf-meta-clear" => {
            let input = PathBuf::from(a.pos(0)?);
            done(info::clear(&input, a.pw(), &a.output(&input, "_정보삭제"))?)
        }
        "pdf-form-list" => json(&forms::list(Path::new(a.pos(0)?), a.pw())?),
        "pdf-form-fill" => {
            let input = PathBuf::from(a.pos(0)?);
            let mut values = BTreeMap::new();
            for kv in &a.rest[1..] {
                let (k, v) = kv.split_once('=').ok_or_else(|| Error::Invalid(format!("이름=값 형식으로 적어 주세요 ({kv})")))?;
                values.insert(k.to_string(), v.to_string());
            }
            done(forms::fill(&input, a.pw(), &values, a.has("--flatten"), &a.output(&input, "_양식채움"), &mut job)?)
        }
        "pdf-flatten" => {
            let input = PathBuf::from(a.pos(0)?);
            done(forms::flatten(&input, a.pw(), &a.output(&input, "_평평하게"), &mut job)?)
        }
        _ => return Ok(None),
    };
    let _ = pdfedit::IDENTITY;
    Ok(Some(code))
}

// ---------- 점검: 샘플로 모든 작업 실행 ----------

/// 쪽 하나를 그려(가로 360px) 전/후를 나란히 붙인 PNG
fn evidence(before: &Path, bpw: Option<&str>, after: &Path, apw: Option<&str>, page: u32, out: &Path) -> Result<()> {
    let render = |p: &Path, pw: Option<&str>| -> Result<image::RgbImage> {
        let info = thumbs::page_infos(p, pw)?;
        let pg = page.min(info.pages.len() as u32).max(1);
        let vw = info.pages[pg as usize - 1].visual_width.max(1.0);
        thumbs::render_rgb(p, pw, pg, 360.0 / vw)
    };
    let a = render(before, bpw)?;
    let b = render(after, apw)?;
    let gap = 16;
    let h = a.height().max(b.height());
    let mut canvas = image::RgbImage::from_pixel(a.width() + b.width() + gap, h, image::Rgb([200, 200, 200]));
    image::imageops::overlay(&mut canvas, &a, 0, 0);
    image::imageops::overlay(&mut canvas, &b, (a.width() + gap) as i64, 0);
    canvas.save(out).map_err(|e| Error::Invalid(e.to_string()))
}

fn selftest(dir: &Path) -> Result<ExitCode> {
    use filetool_core::pdfedit::testpdf;
    std::fs::create_dir_all(dir).map_err(|e| Error::Io { path: dir.into(), source: e })?;
    if std::fs::read_dir(dir).map(|mut d| d.next().is_some()).unwrap_or(false) {
        return Err(Error::Invalid(format!("빈 폴더를 주세요: {}", dir.display())));
    }
    let ev = dir.join("evidence");
    std::fs::create_dir_all(&ev).map_err(|e| Error::Io { path: ev.clone(), source: e })?;
    let mixed = dir.join("섞임.pdf");
    testpdf::make_pages(&mixed, &[(595.0, 842.0, 0), (842.0, 595.0, 0), (595.0, 842.0, 90), (420.0, 595.0, 0)], "P")?;
    let other = dir.join("다른.pdf");
    testpdf::make_pages(&other, &[(595.0, 842.0, 0), (595.0, 842.0, 0)], "Q")?;
    let form = dir.join("양식.pdf");
    testpdf::make_form(&form)?;
    let sig = dir.join("서명.png");
    testpdf::make_signature_png(&sig)?;
    let many = dir.join("100쪽.pdf");
    testpdf::make_many(&many, 100)?;
    match pdfedit::font::korean_font() {
        Ok(f) => println!("글꼴  {} (#{} {})", f.path.display(), f.index, f.name),
        Err(e) => println!("글꼴  없음: {e}"),
    }
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut failed = 0;
    let mut row = |name: &str, r: Result<String>| {
        match r {
            Ok(s) => rows.push((name.to_string(), format!("성공  {s}"))),
            Err(e) => {
                failed += 1;
                rows.push((name.to_string(), format!("실패  {e}")))
            }
        }
    };
    let o = |s: &str| dir.join(s);
    let mut j = Job::none();
    let shot = |before: &Path, after: &Path, page: u32, name: &str| -> Result<String> {
        evidence(before, None, after, None, page, &ev.join(format!("{name}.png")))?;
        Ok(format!("evidence/{name}.png"))
    };

    // 1. 미리보기·계획
    row("쪽 정보", thumbs::page_infos(&mixed, None).map(|d| d.pages.iter().map(|p| format!("{}×{}/{}", p.visual_width as i32, p.visual_height as i32, p.rotate)).collect::<Vec<_>>().join(" ")));
    row("미리보기 160px 100쪽", thumbs::bench(&many, None, 160).map(|(n, ms, open)| format!("{n}쪽 쪽당 {ms:.1} ms (열기 {open:.1} ms)")));
    let plan_json = format!(
        r#"{{"sources":[{{"path":{}}},{{"path":{}}}],"pages":[{{"kind":"page","src":0,"index":2,"rotate":90}},{{"kind":"page","src":1,"index":1}},{{"kind":"blank","size":"a4"}},{{"kind":"page","src":0,"index":0}}]}}"#,
        pdfedit::to_json(&mixed),
        pdfedit::to_json(&other)
    );
    let r = pdfedit::from_json::<EditPlan>(&plan_json).and_then(|p| plan::apply(&p, &o("계획.pdf"), &mut j));
    row("쪽 편집 계획", r.and_then(|r| Ok(format!("{}쪽 {}", r.pages, shot(&mixed, &o("계획.pdf"), 1, "01_plan")?))));
    row("쪽 뽑기 4,2", plan::extract(&mixed, None, "4,2", &o("뽑기.pdf"), &mut j).map(|r| format!("{}쪽", r.pages)));
    row("빈 쪽 넣기", plan::insert_blank(&mixed, None, 1, 1, &BlankSize::Named("a4".into()), &o("빈쪽.pdf"), &mut j).map(|r| format!("{}쪽", r.pages)));
    row("다른 PDF 끼워 넣기", plan::insert_pdf(&mixed, None, 2, &other, None, None, &o("끼움.pdf"), &mut j).map(|r| format!("{}쪽", r.pages)));
    // 2. 얹기
    let r = stamp::page_numbers(&mixed, None, &PageNumberOpts { format: NumberFormat::OfTotal, font_size: 14.0, ..Default::default() }, &o("쪽번호.pdf"), &mut j);
    row("쪽 번호(1 / N, 회전 쪽)", r.and_then(|r| Ok(format!("{} {}", r.note, shot(&mixed, &o("쪽번호.pdf"), 3, "02_page_numbers_rotated")?))));
    let wm = WatermarkOpts { kind: WatermarkKind::Text { text: "대외비 사본".into(), font_size: 60.0, color: "#C00000".into() }, opacity: 0.25, angle: 45.0, tiling: Tiling::Center, layer: Layer::Over, pages: None };
    row("워터마크 글자(가운데)", stamp::watermark(&mixed, None, &wm, &o("워터마크.pdf"), &mut j).and_then(|_| shot(&mixed, &o("워터마크.pdf"), 1, "03_watermark_text")));
    let wmt = WatermarkOpts { tiling: Tiling::Tile, layer: Layer::Under, kind: WatermarkKind::Text { text: "SAMPLE 견본".into(), font_size: 20.0, color: "#0050C0".into() }, ..wm.clone() };
    row("워터마크 글자(바둑판, 아래)", stamp::watermark(&mixed, None, &wmt, &o("워터마크_바둑판.pdf"), &mut j).and_then(|_| shot(&mixed, &o("워터마크_바둑판.pdf"), 2, "04_watermark_tile")));
    let wmi = WatermarkOpts { kind: WatermarkKind::Image { path: sig.clone(), scale: 0.6 }, opacity: 0.4, angle: 30.0, tiling: Tiling::Center, layer: Layer::Over, pages: None };
    row("워터마크 그림", stamp::watermark(&mixed, None, &wmi, &o("워터마크_그림.pdf"), &mut j).and_then(|_| shot(&mixed, &o("워터마크_그림.pdf"), 1, "05_watermark_image")));
    let so = SignatureOpts { image: sig.clone(), placements: vec![ImagePlacement { page: 3, x: 600.0, y: 430.0, w: 180.0, h: 0.0 }, ImagePlacement { page: 1, x: 380.0, y: 720.0, w: 160.0, h: 0.0 }], remove_white: true, opacity: 1.0 };
    row("서명 그림(회전 쪽, 흰 배경 투명)", stamp::place_image(&mixed, None, &so, &o("서명.pdf"), &mut j).and_then(|_| shot(&mixed, &o("서명.pdf"), 3, "06_signature_rotated")));
    let tb = vec![TextBox { page: 1, x: 80.0, y: 160.0, text: "확인했습니다 — 홍길동\n2026년 11월 1일".replace("\\n", "\n"), font_size: 18.0, color: "#003399".into(), background: Some("#FFFFFF".into()), width: 0.0 }];
    row("글상자(한글, 흰 바탕)", stamp::text_boxes(&mixed, None, &tb, &o("글상자.pdf"), &mut j).and_then(|r| Ok(format!("{} {}", r.note, shot(&mixed, &o("글상자.pdf"), 1, "07_text_box")?))));
    // 3. 암호
    let po = ProtectOpts { open_password: "1234".into(), owner_password: "owner-5678".into(), allow_print: true, allow_copy: false, allow_edit: false, cipher: Cipher::Aes256 };
    row("암호 걸기 AES-256(열기 1234)", crypt::protect(&mixed, None, &po, &o("암호_AES256.pdf")).map(|r| r.note));
    row("암호 걸기 AES-128(열기 1234)", crypt::protect(&mixed, None, &ProtectOpts { cipher: Cipher::Aes128, ..po.clone() }, &o("암호_AES128.pdf")).map(|r| r.note));
    row("암호 없이 열기 → 거절", match pdfedit::open(&o("암호_AES256.pdf"), None) { Err(Error::PasswordRequired(_)) => Ok("PasswordRequired".into()), r => Err(Error::Invalid(format!("예상과 다름: {:?}", r.map(|_| ())))) });
    row("암호 풀기(1234)", crypt::unprotect(&o("암호_AES256.pdf"), "1234", &o("암호풀림.pdf")).map(|r| r.note));
    // 4. 모양
    row("여백 자르기(자동)", layout::crop(&mixed, None, &CropOpts { mode: CropMode::Auto { padding: 4.0 }, pages: None }, &o("여백.pdf"), &mut j).and_then(|_| shot(&mixed, &o("여백.pdf"), 3, "08_crop_auto_rotated")));
    row("A4에 맞추기", layout::fit_a4(&mixed, None, &FitOpts::default(), &o("A4.pdf"), &mut j).and_then(|_| shot(&mixed, &o("A4.pdf"), 4, "09_fit_a4_small_page")));
    row("모아 찍기 2쪽", layout::nup(&mixed, None, &NupOpts::default(), &o("모아2.pdf"), &mut j).and_then(|r| Ok(format!("{}장 {}", r.pages, shot(&mixed, &o("모아2.pdf"), 1, "10_nup2")?))));
    row("모아 찍기 4쪽", layout::nup(&mixed, None, &NupOpts { per_sheet: 4, ..Default::default() }, &o("모아4.pdf"), &mut j).and_then(|r| Ok(format!("{}장 {}", r.pages, shot(&mixed, &o("모아4.pdf"), 1, "11_nup4")?))));
    // 5. 정보·양식
    let di = DocInfo { title: Some("분기 보고서".into()), author: Some("홍길동".into()), ..Default::default() };
    row("문서 정보 바꾸기", info::set(&mixed, None, &di, &o("정보.pdf")).and_then(|_| info::read(&o("정보.pdf"), None)).map(|r| format!("제목={:?} 작성자={:?} XMP {:?}B", r.info.title, r.info.author, r.xmp_bytes)));
    row("문서 정보 지우기", info::clear(&o("정보.pdf"), None, &o("정보삭제.pdf")).and_then(|_| info::read(&o("정보삭제.pdf"), None)).map(|r| format!("제목={:?} XMP={:?}", r.info.title, r.xmp_bytes)));
    row("양식 칸 목록", forms::list(&form, None).map(|v| v.iter().map(|f| format!("{}:{}", f.name, f.kind)).collect::<Vec<_>>().join(" ")));
    let mut vals = BTreeMap::new();
    for (k, v) in [("name", "홍길동"), ("agree", "true"), ("size", "M"), ("city", "서울"), ("color", "Blue")] {
        vals.insert(k.to_string(), v.to_string());
    }
    row("양식 채우기", forms::fill(&form, None, &vals, false, &o("양식채움.pdf"), &mut j).and_then(|_| forms::list(&o("양식채움.pdf"), None)).map(|v| v.iter().map(|f| format!("{}={}", f.name, f.value)).collect::<Vec<_>>().join(" ")));
    row("평평하게", forms::flatten(&o("양식채움.pdf"), None, &o("평평.pdf"), &mut j).and_then(|r| Ok(format!("{} {}", r.note, shot(&form, &o("평평.pdf"), 1, "12_form_fill_flatten")?))));
    println!();
    for (k, v) in &rows {
        println!("{k:<28} {v}");
    }
    println!("\n결과: {}개 중 실패 {failed}", rows.len());
    Ok(if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) })
}
