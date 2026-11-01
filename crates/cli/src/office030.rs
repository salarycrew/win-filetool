//! 0.3.0 W2 문서 변환 명령: doc-to-pptx / pdf-to-docx / pdf-to-pptx / pdf-to-txt / extract-images
//! 결과 구조 숫자(제목·표·그림·글상자·슬라이드)와 걸린 시간을 한 줄로 보여 준다(점검용).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use filetool_core::pdf2pptx::PptMode;
use filetool_core::tools::Tool;
use filetool_core::{docx2pptx, officeimages, pdf2docx, pdf2pptx, Error, Result};

use crate::conv::{toolset, Flags};

fn each(files: &[String], mut f: impl FnMut(&Path) -> Result<String>) -> ExitCode {
    if files.is_empty() {
        println!("파일을 지정하세요");
        return ExitCode::from(2);
    }
    let mut failed = 0;
    for p in files {
        match f(Path::new(p)) {
            Ok(line) => println!("완료  {line}"),
            Err(e) => {
                failed += 1;
                println!("실패  {p}: {e}");
            }
        }
    }
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

pub fn run(cmd: &str, args: &[String]) -> Result<Option<ExitCode>> {
    let f = Flags::parse(&args[1.min(args.len())..]);
    let out = f.get("--out").map(PathBuf::from);
    let out = out.as_deref();
    let code = match cmd {
        "doc-to-pptx" => {
            let ts = toolset(&f);
            let so = ts.get(Tool::Soffice).ok().map(Path::to_path_buf);
            each(&f.rest, |p| {
                let r = docx2pptx::doc_to_pptx(p, out, so.as_deref(), None, &mut |_, _| {})?;
                Ok(format!("{} -> {}  슬라이드 {} · 제목 {} · 그림 {} · 표 {}{} · {} ms", p.display(), r.output.display(), r.slides, r.sections, r.images, r.tables, if r.via_libreoffice { " · LibreOffice로 docx 먼저" } else { "" }, r.millis))
            })
        }
        "pdf-to-docx" => each(&f.rest, |p| {
            let r = pdf2docx::pdf_to_docx(p, out, None, &mut |_, _| {})?;
            Ok(format!(
                "{} -> {}  {}쪽 · 제목 {} · 문단 {} · 목록 {} · 표 {} · 그림 {} · 스캔본 쪽 {} · 두 단 쪽 {} · 뺀 머리글/바닥글 {}줄 · {} ms",
                p.display(),
                r.output.display(),
                r.pages,
                r.headings,
                r.paragraphs,
                r.list_items,
                r.tables,
                r.images,
                r.scanned_pages,
                r.two_column_pages,
                r.dropped_lines,
                r.millis
            ))
        }),
        "pdf-to-pptx" => {
            let mode = match f.get("--mode").unwrap_or("image") {
                "image" => PptMode::Image,
                "editable" => PptMode::Editable,
                o => return Err(Error::Invalid(format!("--mode는 image|editable ({o})"))),
            };
            let dpi: f32 = f.num("--dpi", pdf2pptx::DEFAULT_DPI)?;
            each(&f.rest, |p| {
                let r = pdf2pptx::pdf_to_pptx(p, out, mode, Some(dpi), None, &mut |_, _| {})?;
                Ok(format!("{} -> {}  슬라이드 {} · 글상자 {} · 그림 {} · 도형 {} · 쪽 그림 {} · {} ms", p.display(), r.output.display(), r.slides, r.text_boxes, r.pictures, r.shapes, r.image_pages, r.millis))
            })
        }
        "pdf-to-txt" => each(&f.rest, |p| {
            let r = pdf2docx::pdf_to_txt(p, out, None, &mut |_, _| {})?;
            Ok(format!("{} -> {}  {}쪽 · 글자 {} · 스캔본 쪽 {} · {} ms", p.display(), r.output.display(), r.pages, r.chars, r.scanned_pages, r.millis))
        }),
        "extract-images" => each(&f.rest, |p| {
            let r = officeimages::extract_images(p, out)?;
            Ok(format!(
                "{} -> {}  그림 {} · 같은 그림 건너뜀 {} · 형식 몰라 건너뜀 {} · {} ms",
                p.display(),
                r.folder.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "(그림 없음)".into()),
                r.files.len(),
                r.duplicates,
                r.skipped,
                r.millis
            ))
        }),
        _ => return Ok(None),
    };
    Ok(Some(code))
}
