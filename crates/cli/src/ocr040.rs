//! 0.4 글자 인식(OCR)·진짜 가리기 명령(코어 `filetool_core::ocr`, `pdfedit::redact`). 결과는 JSON.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use filetool_core::ocr::pdf::{self as ocrpdf, OcrPdfOpts};
use filetool_core::ocr::{self, Recognizer};
use filetool_core::pdfedit::redact::{self, Mark, Preset, RedactOpts};
use filetool_core::pdfedit::{self, testpdf, testpdf040, thumbs, Job};
use filetool_core::process::Cancel;
use filetool_core::tools::Tool;
use filetool_core::{pdf2docx, Error, Result};

pub const USAGE: &str = "0.4 글자 인식(OCR)·가리기 (공통: --password 암호  --out 폴더  --lang ko|en-US|auto)
  filetool-cli ocr status                                         이 PC의 글자 인식 언어·크기 제한(JSON)
  filetool-cli ocr pdf <pdf...> [--dpi 300] [--redo] [--pages 1-3] [--cancel-after N]   스캔 PDF를 검색 가능하게(이름_글자인식.pdf)
  filetool-cli ocr text <jpg|png|heic|pdf...>                     그림·스캔 PDF → 이름_글자.txt
  filetool-cli ocr docx <pdf...>                                  PDF → Word(스캔 쪽은 글자 인식해서 문단으로)
  filetool-cli ocr image <그림>                                   그림 한 장 인식 결과(줄·낱말 상자 JSON)
  filetool-cli redact find <pdf> [--words 말1,말2] [--presets rrn,phone,email,account] [--no-ocr]   찾은 곳(JSON, 아무것도 바꾸지 않음)
  filetool-cli redact apply <pdf> (--marks 상자.json | --words … --presets …) [--dpi 200|300] [--no-ocr]   이름_가림.pdf + 확인 결과
  filetool-cli redact verify <pdf> <글...>                        글이 파일 어디에든 남았는지(JSON)
  filetool-cli redact image <jpg|png|heic> [--boxes x,y,w,h;x,y,w,h] [--text 은행_제출용]   사진 가리기(이름_가림.jpg, 위치·촬영 정보 없음)
  filetool-cli ocr make-scan <글자.pdf> <출력.pdf> [--dpi 200] [--rotate 0|90|180|270] [--pages N]   점검용 스캔본(그림만 있는 PDF)
  filetool-cli ocr make-photo <글자.pdf> <출력.jpg> [--page 1]   점검용 휴대폰 사진 흉내(흐리고 기울어짐)
  filetool-cli ocr-selftest <빈 폴더>                              가리기·글자 인식 점검(이 PC에 OCR이 있으면 인식까지)";

const SWITCHES: [&str; 3] = ["--redo", "--no-ocr", "--json"];

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
                if SWITCHES.contains(&a.as_str()) {
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
    fn list(&self, k: &str) -> Vec<String> {
        self.get(k).map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default()
    }
    fn out(&self) -> Option<PathBuf> {
        self.get("--out").map(PathBuf::from)
    }
    fn pw(&self) -> Option<&str> {
        self.get("--password")
    }
    fn files(&self, from: usize) -> Result<Vec<PathBuf>> {
        let v: Vec<PathBuf> = self.rest.iter().skip(from).map(PathBuf::from).collect();
        if v.is_empty() {
            return Err(Error::Invalid(format!("파일을 주세요\n{USAGE}")));
        }
        Ok(v)
    }
}

fn json<T: serde::Serialize>(v: &T) {
    println!("{}", pdfedit::to_json(v));
}

fn presets(a: &Args) -> Result<Vec<Preset>> {
    a.list("--presets")
        .iter()
        .map(|p| match p.as_str() {
            "rrn" | "주민등록번호" => Ok(Preset::Rrn),
            "phone" | "전화" | "전화번호" => Ok(Preset::Phone),
            "email" | "이메일" => Ok(Preset::Email),
            "account" | "계좌" | "계좌번호" => Ok(Preset::Account),
            "all" => Err(Error::Invalid("all".into())),
            o => Err(Error::Invalid(format!("모르는 규칙: {o} (rrn, phone, email, account)"))),
        })
        .collect::<Result<Vec<_>>>()
        .or_else(|e| if e.to_string() == "all" { Ok(vec![Preset::Rrn, Preset::Phone, Preset::Email, Preset::Account]) } else { Err(e) })
}

fn ffmpeg() -> Option<PathBuf> {
    let f = crate::conv::Flags { opts: vec![], rest: vec![] };
    crate::conv::toolset(&f).get(Tool::Ffmpeg).ok().map(Path::to_path_buf)
}

/// 진행률을 표준 오류로(10% 단위), `--cancel-after N`이면 N쪽 뒤에 멈춤(취소 점검용)
fn progress_job<'a>(cancel: &'a Cancel, after: u32, cb: &'a mut dyn FnMut(u32, u32)) -> Job<'a> {
    let _ = after;
    Job::new(Some(cancel), cb)
}

pub fn run(cmd: &str, args: &[String]) -> Result<Option<ExitCode>> {
    let a = Args::parse(args);
    let sub = a.rest.get(1).map(String::as_str).unwrap_or("");
    let lang = a.get("--lang");
    match (cmd, sub) {
        ("ocr", "status") => {
            json(&ocr::status());
        }
        ("ocr", "pdf") => {
            let rec = ocr::engine(lang)?;
            let opts = OcrPdfOpts { dpi: a.num("--dpi", 300u32)?, redo_text_pages: a.has("--redo"), pages: a.get("--pages").map(String::from) };
            let cancel_after = a.num("--cancel-after", 0u32)?;
            for f in a.files(2)? {
                let out = pdfedit::default_output(&f, a.out().as_deref(), "_글자인식");
                let cancel = Cancel::new();
                let mut last = 0u32;
                let c2 = cancel.clone();
                let mut cb = move |i: u32, n: u32| {
                    let pct = i * 100 / n.max(1);
                    if pct / 10 != last / 10 || i == n {
                        eprintln!("진행 {i}/{n}쪽");
                        last = pct;
                    }
                    if cancel_after > 0 && i >= cancel_after {
                        c2.cancel();
                    }
                };
                let mut job = progress_job(&cancel, cancel_after, &mut cb);
                match ocrpdf::make_searchable(&f, a.pw(), &opts, &out, rec.as_ref(), &mut job) {
                    Ok(o) => json(&o),
                    Err(Error::Cancelled) => {
                        println!("{{\"cancelled\": true, \"output_exists\": {}}}", out.exists());
                        return Ok(Some(ExitCode::from(3)));
                    }
                    Err(e) => return Err(e),
                }
            }
        }
        ("ocr", "text") => {
            let rec = ocr::engine(lang)?;
            let ff = ffmpeg();
            for f in a.files(2)? {
                let mut o = ocrpdf::file_to_txt(&f, a.pw(), ff.as_deref(), a.out().as_deref(), rec.as_ref(), &mut Job::none())?;
                if !a.has("--json") {
                    o.text = o.text.chars().take(300).collect();
                }
                json(&o);
            }
        }
        ("ocr", "docx") => {
            let rec = ocr::engine(lang)?;
            for f in a.files(2)? {
                let o = pdf2docx::pdf_to_docx_with(&f, a.out().as_deref(), Some(rec.as_ref()), None, &mut |_, _| {})?;
                json(&o);
            }
        }
        ("ocr", "make-scan") => {
            let (src, dst) = (PathBuf::from(a.rest.get(2).ok_or_else(|| Error::Invalid(USAGE.into()))?), PathBuf::from(a.rest.get(3).ok_or_else(|| Error::Invalid(USAGE.into()))?));
            let n = testpdf040::make_scan_n(&src, &dst, a.num("--dpi", 200u32)?, a.num("--rotate", 0u16)?, a.num("--pages", 0u32)?)?;
            println!("{{\"output\": {:?}, \"pages\": {n}}}", dst.display().to_string());
        }
        ("ocr", "make-photo") => {
            let (src, dst) = (PathBuf::from(a.rest.get(2).ok_or_else(|| Error::Invalid(USAGE.into()))?), PathBuf::from(a.rest.get(3).ok_or_else(|| Error::Invalid(USAGE.into()))?));
            testpdf040::make_photo(&src, a.num("--page", 1u32)?, &dst)?;
            println!("{{\"output\": {:?}}}", dst.display().to_string());
        }
        ("ocr", "image") => {
            let rec = ocr::engine(lang)?;
            for f in a.files(2)? {
                let img = ocrpdf::load_image(&f, ffmpeg().as_deref(), None)?;
                let t = Instant::now();
                let r = ocr::recognize(rec.as_ref(), &img, None)?;
                println!("{{\"ms\": {}, \"language\": \"{}\"}}", t.elapsed().as_millis(), rec.language());
                json(&r);
            }
        }
        ("redact", "find") => {
            let f = a.files(2)?.remove(0);
            let rec = if a.has("--no-ocr") { None } else { ocr::engine(lang).ok() };
            let o = redact::find(&f, a.pw(), &a.list("--words"), &presets(&a)?, rec.as_deref(), &mut Job::none())?;
            json(&o);
        }
        ("redact", "apply") => {
            let f = a.files(2)?.remove(0);
            let rec = if a.has("--no-ocr") { None } else { ocr::engine(lang).ok() };
            let mut terms = Vec::new();
            let marks: Vec<Mark> = if let Some(m) = a.get("--marks") {
                let s = std::fs::read_to_string(m).map_err(|e| Error::Io { path: m.into(), source: e })?;
                pdfedit::from_json(&s)?
            } else {
                let found = redact::find(&f, a.pw(), &a.list("--words"), &presets(&a)?, rec.as_deref(), &mut Job::none())?;
                terms = found.hits.iter().map(|h| h.text.clone()).collect();
                found.hits.into_iter().flat_map(|h| h.rects).collect()
            };
            let opts = RedactOpts { dpi: a.num("--dpi", 200u32)?, ocr: !a.has("--no-ocr"), check_terms: terms };
            let out = redact::default_output(&f, a.out().as_deref());
            let o = redact::apply(&f, a.pw(), &marks, &opts, &out, rec.as_deref(), &mut Job::none())?;
            json(&o);
            if !o.verify.clean {
                return Ok(Some(ExitCode::from(4)));
            }
        }
        ("redact", "verify") => {
            let f = PathBuf::from(a.rest.get(2).ok_or_else(|| Error::Invalid(USAGE.into()))?);
            let terms: Vec<String> = a.rest.iter().skip(3).cloned().collect();
            let v = redact::verify(&f, a.pw(), &terms)?;
            json(&v);
            if !v.clean {
                return Ok(Some(ExitCode::from(4)));
            }
        }
        ("redact", "image") => {
            let f = a.files(2)?.remove(0);
            let boxes = a
                .get("--boxes")
                .unwrap_or("")
                .split(';')
                .filter(|b| !b.trim().is_empty())
                .map(|b| {
                    let v: Vec<f32> = b.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    if v.len() == 4 {
                        Ok(filetool_core::imgredact::PxBox { x: v[0], y: v[1], w: v[2], h: v[3] })
                    } else {
                        Err(Error::Invalid(format!("상자는 x,y,w,h 로 적어 주세요: {b}")))
                    }
                })
                .collect::<Result<Vec<_>>>()?;
            let opts = filetool_core::imgredact::ImgRedactOpts { boxes, text: a.get("--text").map(String::from) };
            json(&filetool_core::imgredact::redact(&f, ffmpeg().as_deref(), &opts, a.out().as_deref())?);
        }
        ("ocr-selftest", _) => {
            let dir = PathBuf::from(a.rest.get(1).ok_or_else(|| Error::Invalid(USAGE.into()))?);
            return selftest(&dir).map(Some);
        }
        ("help-ocr", _) => println!("{USAGE}"),
        _ => return Ok(None),
    }
    Ok(Some(ExitCode::SUCCESS))
}

/// 가리기(어느 PC에서나) + 글자 인식(OCR이 있는 PC). 결과 표를 찍고 실패가 있으면 1.
pub fn selftest(dir: &Path) -> Result<ExitCode> {
    std::fs::create_dir_all(dir).map_err(|e| Error::Io { path: dir.into(), source: e })?;
    if std::fs::read_dir(dir).map(|mut d| d.next().is_some()).unwrap_or(false) {
        return Err(Error::Invalid(format!("빈 폴더를 주세요: {}", dir.display())));
    }
    let mut failed = 0;
    let mut row = |name: &str, r: Result<String>| match r {
        Ok(s) => println!("성공  {name}: {s}"),
        Err(e) => {
            failed += 1;
            println!("실패  {name}: {e}")
        }
    };
    let src = dir.join("비밀.pdf");
    let ko = testpdf040::make_secrets(&src)?;
    let mut terms: Vec<String> = testpdf040::SECRETS.iter().map(|s| s.to_string()).collect();
    if ko {
        terms.push(testpdf040::SECRET_KO.into());
    }
    let st = ocr::status();
    println!("글자 인식: {}", pdfedit::to_json(&st).replace('\n', " "));
    let rec: Option<Box<dyn Recognizer>> = ocr::engine(None).ok();
    // 1) 찾기
    let found = redact::find(&src, None, &["ROT-SECRET".into()], &[Preset::Rrn, Preset::Phone, Preset::Email, Preset::Account], rec.as_deref(), &mut Job::none());
    let mut marks = testpdf040::secret_marks();
    row(
        "찾아서 가리기(규칙)",
        found.as_ref().map_err(|e| Error::Invalid(e.to_string())).and_then(|f| {
            let kinds: Vec<&str> = f.hits.iter().map(|h| h.kind.as_str()).collect();
            for k in ["rrn", "phone", "email", "account", "word"] {
                if !kinds.contains(&k) {
                    return Err(Error::Invalid(format!("{k} 못 찾음: {kinds:?}")));
                }
            }
            Ok(format!("{}곳 ({})", f.hits.len(), f.hits.iter().map(|h| format!("{}쪽 {}", h.page, h.text)).collect::<Vec<_>>().join(", ")))
        }),
    );
    if let Ok(f) = &found {
        marks.extend(f.hits.iter().filter(|h| h.page == 3).flat_map(|h| h.rects.clone()));
    }
    terms.push("ROT-SECRET".into());
    // 2) 가리기 + 확인
    let out = redact::default_output(&src, None);
    let res = redact::apply(&src, None, &marks, &RedactOpts { dpi: 200, ocr: true, check_terms: terms.clone() }, &out, rec.as_deref(), &mut Job::none());
    row(
        "가리기(글자·주석·양식 값·문서 정보·첨부·JS·숨긴 레이어)",
        res.as_ref().map_err(|e| Error::Invalid(e.to_string())).and_then(|o| {
            if !o.verify.clean {
                return Err(Error::Invalid(format!("남음: {:?}", o.verify)));
            }
            Ok(format!("{} · 쪽 {:?} · 다시 얹은 글자 층 {}쪽 · 지움 {:?}", out.file_name().unwrap_or_default().to_string_lossy(), o.redacted_pages, o.ocr_pages, o.removed))
        }),
    );
    row(
        "가리기 뒤 다시 확인(PDFium 글자·풀어 본 스트림·문자열·문서 정보)",
        redact::verify(&out, None, &terms).and_then(|v| {
            if v.clean && !v.has_info && !v.has_xmp {
                Ok(format!("{}개 글, 스트림 {}개에서 0건", v.terms, v.streams))
            } else {
                Err(Error::Invalid(format!("{v:?}")))
            }
        }),
    );
    row(
        "가리지 않은 쪽은 그대로",
        thumbs::page_text(&out, None, 2).and_then(|t| if t.contains("PUBLIC-TWO") { Ok("2쪽 글자 있음".into()) } else { Err(Error::Invalid(format!("2쪽: {t:?}"))) }),
    );
    // 3) 글자 인식(이 PC에 OCR이 있을 때)
    match rec.as_deref() {
        None => println!("건너뜀 글자 인식: {}", st.hint.clone().unwrap_or_default()),
        Some(r) => {
            let txt = dir.join("글.pdf");
            testpdf::make_pages(&txt, &[(595.0, 842.0, 0), (842.0, 595.0, 0)], "Page ")?;
            let sc = dir.join("스캔.pdf");
            testpdf040::make_scan(&txt, &sc, 200, 0)?;
            let o = dir.join("스캔_글자인식.pdf");
            let t = Instant::now();
            row(
                "스캔 PDF 검색 가능하게",
                ocrpdf::make_searchable(&sc, None, &OcrPdfOpts::default(), &o, r, &mut Job::none()).and_then(|x| {
                    let t1 = thumbs::page_text(&o, None, 1)?;
                    if t1.replace(' ', "").contains("Page1") {
                        Ok(format!("{} · {}ms/쪽 · 1쪽 글자 {:?}", x.note, x.ms_per_page, t1.trim()))
                    } else {
                        Err(Error::Invalid(format!("1쪽 글자 {t1:?}")))
                    }
                }),
            );
            // 누운 스캔(그림을 90도 돌림)
            let sc90 = dir.join("누운스캔.pdf");
            testpdf040::make_scan(&txt, &sc90, 200, 90)?;
            let o90 = dir.join("누운스캔_글자인식.pdf");
            row(
                "누운 스캔 돌려 읽기",
                ocrpdf::make_searchable(&sc90, None, &OcrPdfOpts::default(), &o90, r, &mut Job::none()).and_then(|x| {
                    let t1 = thumbs::page_text(&o90, None, 1)?;
                    if x.rotated_pages >= 1 && t1.replace(' ', "").contains("Page1") {
                        Ok(format!("돌려 읽은 쪽 {} · 1쪽 {:?}", x.rotated_pages, t1.trim()))
                    } else {
                        Err(Error::Invalid(format!("돌린 쪽 {} · {t1:?}", x.rotated_pages)))
                    }
                }),
            );
            row(
                "스캔 PDF → 글자(TXT)",
                ocrpdf::file_to_txt(&sc, None, None, None, r, &mut Job::none()).map(|x| format!("{} → {}", x.note, x.output.file_name().unwrap_or_default().to_string_lossy())),
            );
            println!("(글자 인식 시간 {}ms)", t.elapsed().as_millis());
        }
    }
    println!("{}", if failed == 0 { "ocr-selftest: 모두 통과" } else { "ocr-selftest: 실패 있음" });
    Ok(if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) })
}
