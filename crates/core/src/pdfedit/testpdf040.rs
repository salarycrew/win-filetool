//! 0.4 점검용 PDF(테스트와 `filetool-cli redact-selftest`가 함께 씀).
//!
//! [`make_secrets`]: 비밀 글이 여러 곳에 숨은 3쪽 PDF
//! - 1쪽: 글자 객체(`SECRET-ALPHA 900101-1234567`, 전화·이메일·계좌, 한글 글꼴이 있으면 `비밀 홍길동`), 메모 주석(FreeText, 내용·모양 `SECRET-ANNOT`),
//!   글자 양식 칸(값 `SECRET-FIELD`), 링크(`https://secret-link.example`), 가리지 않을 줄 `PUBLIC-ONE`
//! - 2쪽: `PUBLIC-TWO` + 숨긴 레이어 안 `SECRET-OCG`
//! - 3쪽: 90도 돌린 쪽 `ROT-SECRET 02-123-4567`
//! - 문서: 제목·XMP `SECRET-META`, 첨부 파일(`SECRET-ATTACH`), 여는 동작 자바스크립트(`SECRET-JS`)
//!
//! [`SECRETS`]는 가린 뒤 어디에도 남으면 안 되는 글, [`secret_marks`]는 1쪽의 가릴 곳.
//! [`make_scan`]: 글자 PDF를 그림으로 그려 그림만 있는 PDF(스캔본 흉내)로.

use std::path::Path;

use lopdf::{dictionary, Document, Object, Stream};

use super::redact::Mark;
use crate::error::{io_err, Result};

pub const SECRETS: &[&str] = &["SECRET-ALPHA", "900101-1234567", "SECRET-ANNOT", "SECRET-FIELD", "SECRET-META", "SECRET-ATTACH", "SECRET-JS", "SECRET-OCG", "secret-link"];
/// 한글 글꼴이 있을 때만 들어가는 비밀
pub const SECRET_KO: &str = "비밀 홍길동";

/// 1쪽의 가릴 곳(보이는 쪽 pt, 왼쪽 위 원점): 글 네 줄, 메모, 양식 칸
pub fn secret_marks() -> Vec<Mark> {
    vec![
        Mark { page: 1, x: 60.0, y: 60.0, w: 480.0, h: 150.0 },
        Mark { page: 1, x: 295.0, y: 298.0, w: 210.0, h: 48.0 },
        Mark { page: 1, x: 68.0, y: 358.0, w: 236.0, h: 38.0 },
    ]
}

fn helv(doc: &mut Document) -> lopdf::ObjectId {
    doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" })
}

fn form(doc: &mut Document, w: f32, h: f32, ops: &str, font: lopdf::ObjectId) -> lopdf::ObjectId {
    let d = dictionary! {
        "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)],
        "Resources" => dictionary! { "Font" => dictionary! { "Helv" => font } },
    };
    doc.add_object(Stream::new(d, ops.as_bytes().to_vec()))
}

/// 비밀이 숨은 3쪽 PDF. 반환: 한글 비밀을 넣었는지
pub fn make_secrets(path: &Path) -> Result<bool> {
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font = helv(&mut doc);
    // 1쪽
    let p1 = doc.new_object_id();
    let mut ops = String::from(
        "0 g BT /F1 14 Tf 72 760 Td (SECRET-ALPHA 900101-1234567) Tj 0 -30 Td (Call 010-1234-5678 or hong@example.com) Tj 0 -30 Td (Account 110-123-456789) Tj ET\n\
         BT /F1 14 Tf 72 600 Td (PUBLIC-ONE stays visible) Tj ET\n",
    );
    let mut ko = false;
    let mut ko_font = None;
    if let Ok(mut f) = super::font::PdfFont::new(&mut doc) {
        ops.push_str(&f.show("K1", 16.0, 72.0, 670.0, SECRET_KO));
        ko_font = Some(f.id);
        f.finish(&mut doc)?;
        ko = true;
    }
    let c1 = doc.add_object(Stream::new(dictionary! {}, ops.into_bytes()));
    let annot_ap = form(&mut doc, 200.0, 40.0, "0 0 1 rg BT /Helv 12 Tf 4 14 Td (SECRET-ANNOT) Tj ET", font);
    let annot = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "FreeText", "Rect" => vec![300.into(), 500.into(), 500.into(), 540.into()],
        "Contents" => Object::string_literal("SECRET-ANNOT note"), "DA" => Object::string_literal("/Helv 12 Tf 0 0 1 rg"),
        "AP" => dictionary! { "N" => annot_ap }, "P" => p1, "F" => 4,
    });
    let field_ap = form(&mut doc, 228.0, 30.0, "/Tx BMC BT /Helv 12 Tf 2 9 Td (SECRET-FIELD) Tj ET EMC", font);
    let field = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("name"),
        "V" => Object::string_literal("SECRET-FIELD"), "DA" => Object::string_literal("/Helv 12 Tf 0 g"),
        "Rect" => vec![72.into(), 450.into(), 300.into(), 480.into()], "AP" => dictionary! { "N" => field_ap }, "P" => p1, "F" => 4,
    });
    let link = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Link", "Rect" => vec![72.into(), 400.into(), 200.into(), 420.into()], "Border" => vec![0.into(), 0.into(), 0.into()],
        "A" => dictionary! { "S" => "URI", "URI" => Object::string_literal("https://secret-link.example") },
    });
    let mut fonts = dictionary! { "F1" => font };
    if let Some(k) = ko_font {
        fonts.set("K1", k);
    }
    doc.objects.insert(
        p1,
        Object::Dictionary(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => c1,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            "Resources" => dictionary! { "Font" => fonts },
            "Annots" => vec![Object::Reference(annot), Object::Reference(field), Object::Reference(link)],
        }),
    );
    // 2쪽: 숨긴 레이어
    let ocg = doc.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("hidden") });
    let c2 = doc.add_object(Stream::new(
        dictionary! {},
        b"0 g BT /F1 18 Tf 72 760 Td (PUBLIC-TWO page) Tj ET\n/OC /oc1 BDC BT /F1 12 Tf 72 500 Td (SECRET-OCG) Tj ET EMC\n".to_vec(),
    ));
    let p2 = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => c2,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font }, "Properties" => dictionary! { "oc1" => ocg } },
    });
    // 3쪽: 90도
    let c3 = doc.add_object(Stream::new(dictionary! {}, b"0 g BT /F1 16 Tf 72 700 Td (ROT-SECRET 02-123-4567) Tj ET\n".to_vec()));
    let p3 = doc.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => c3, "Rotate" => 90,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
    });
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![Object::Reference(p1), p2.into(), p3.into()], "Count" => 3 }));
    // 첨부·자바스크립트·레이어·양식·XMP
    let ef = doc.add_object(Stream::new(dictionary! { "Type" => "EmbeddedFile" }, b"SECRET-ATTACH contents".to_vec()));
    let spec = doc.add_object(dictionary! { "Type" => "Filespec", "F" => Object::string_literal("secret.txt"), "UF" => Object::string_literal("secret.txt"), "EF" => dictionary! { "F" => ef } });
    let js = doc.add_object(dictionary! { "S" => "JavaScript", "JS" => Object::string_literal("app.alert('SECRET-JS');") });
    let xmp = doc.add_object(
        Stream::new(
            dictionary! { "Type" => "Metadata", "Subtype" => "XML" },
            b"<?xpacket begin='' id='W5M0MpCehiHzreSzNTczkc9d'?><x:xmpmeta xmlns:x='adobe:ns:meta/'><rdf:RDF xmlns:rdf='http://www.w3.org/1999/02/22-rdf-syntax-ns#'><rdf:Description xmlns:dc='http://purl.org/dc/elements/1.1/'><dc:title>SECRET-META</dc:title></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end='w'?>".to_vec(),
        )
        .with_compression(false),
    );
    let catalog = doc.add_object(dictionary! {
        "Type" => "Catalog", "Pages" => pages_id, "Metadata" => xmp, "OpenAction" => js,
        "Names" => dictionary! {
            "EmbeddedFiles" => dictionary! { "Names" => vec![Object::string_literal("secret.txt"), spec.into()] },
            "JavaScript" => dictionary! { "Names" => vec![Object::string_literal("init"), js.into()] },
        },
        "OCProperties" => dictionary! { "OCGs" => vec![Object::Reference(ocg)], "D" => dictionary! { "OFF" => vec![Object::Reference(ocg)], "Order" => vec![Object::Reference(ocg)] } },
        "AcroForm" => dictionary! { "Fields" => vec![Object::Reference(field)], "DR" => dictionary! { "Font" => dictionary! { "Helv" => font } }, "DA" => Object::string_literal("/Helv 12 Tf 0 g") },
    });
    doc.trailer.set("Root", catalog);
    let info = doc.add_object(dictionary! { "Title" => Object::string_literal("SECRET-META title"), "Author" => Object::string_literal("SECRET-META author") });
    doc.trailer.set("Info", info);
    doc.compress();
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(io_err(path))?;
    crate::paths::write_new(path, &bytes)?;
    Ok(ko)
}

/// 글자 PDF(`src`)의 쪽들을 `dpi`로 그려 그림만 있는 PDF(`dst`)로. `rotate`: 그림을 시계 방향으로 돌려 넣을 각도(누운 스캔 흉내)
pub fn make_scan(src: &Path, dst: &Path, dpi: u32, rotate: u16) -> Result<u32> {
    make_scan_n(src, dst, dpi, rotate, 0)
}

/// [`make_scan`] + 쪽을 되풀이해 `pages`쪽으로 채움(0 = 원래 쪽 수). 큰 스캔본(50쪽) 속도·취소 점검용
pub fn make_scan_n(src: &Path, dst: &Path, dpi: u32, rotate: u16, pages: u32) -> Result<u32> {
    let p = crate::pdfrender::pdfium()?;
    let pdf = p.load_pdf_from_file(src, None).map_err(|e| crate::pdfrender::open_err(src, e))?;
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();
    let n = pdf.pages().len() as u32;
    let total = if pages == 0 { n } else { pages };
    for k in 0..total {
        let page = pdf.pages().get((k % n.max(1)) as i32).map_err(|e| crate::pdfrender::render_err(src, e))?;
        let img = crate::ocr::pdf::render(&page, dpi, src)?.to_rgb8();
        let img = match rotate {
            90 => image::imageops::rotate90(&img),
            180 => image::imageops::rotate180(&img),
            270 => image::imageops::rotate270(&img),
            _ => img,
        };
        let (w, h) = img.dimensions();
        let mut jpg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 90).encode_image(&img).map_err(|e| crate::Error::Invalid(e.to_string()))?;
        let im = doc.add_object(
            Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => w as i64, "Height" => h as i64, "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8, "Filter" => "DCTDecode" }, jpg)
                .with_compression(false),
        );
        let (pw, ph) = (w as f32 * 72.0 / dpi as f32, h as f32 * 72.0 / dpi as f32);
        let c = doc.add_object(Stream::new(dictionary! {}, format!("q {pw} 0 0 {ph} 0 0 cm /Im0 Do Q").into_bytes()));
        kids.push(Object::Reference(doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => c,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real(pw), Object::Real(ph)],
            "Resources" => dictionary! { "XObject" => dictionary! { "Im0" => im } },
        })));
    }
    let n = kids.len() as u32;
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => n as i64 }));
    let cat = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", cat);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(io_err(dst))?;
    crate::paths::write_new(dst, &bytes)?;
    Ok(n)
}

/// 휴대폰으로 찍은 것 같은 사진(JPG): 쪽 하나를 그려 흑백·낮은 대비(밝기 70~185)·2도 기울임·살짝 흐림·잡음, JPEG 품질 60
pub fn make_photo(src: &Path, page: u32, dst: &Path) -> Result<()> {
    let p = crate::pdfrender::pdfium()?;
    let pdf = p.load_pdf_from_file(src, None).map_err(|e| crate::pdfrender::open_err(src, e))?;
    let pg = pdf.pages().get(page.saturating_sub(1) as i32).map_err(|e| crate::pdfrender::render_err(src, e))?;
    let img = crate::ocr::pdf::render(&pg, 150, src)?.to_luma8();
    let (w, h) = img.dimensions();
    // 2도 돌리기(가장 가까운 점), 바깥은 회색 책상
    let (s, c) = 2f32.to_radians().sin_cos();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let mut seed = 12345u32;
    let out = image::GrayImage::from_fn(w, h, |x, y| {
        let (dx, dy) = (x as f32 - cx, y as f32 - cy);
        let (sx, sy) = (c * dx + s * dy + cx, -s * dx + c * dy + cy);
        let v = if sx >= 0.0 && sy >= 0.0 && (sx as u32) < w && (sy as u32) < h { img.get_pixel(sx as u32, sy as u32)[0] as f32 } else { 120.0 };
        // 낮은 대비 + 위쪽이 어두운 그늘 + 잡음
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        let noise = ((seed >> 16) % 21) as f32 - 10.0;
        let shade = 1.0 - 0.18 * (1.0 - y as f32 / h as f32);
        image::Luma([((70.0 + v * (115.0 / 255.0)) * shade + noise).clamp(0.0, 255.0) as u8])
    });
    let out = image::imageops::blur(&out, 0.8);
    let mut jpg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 60).encode_image(&out).map_err(|e| crate::Error::Invalid(e.to_string()))?;
    crate::paths::write_new(dst, &jpg)
}
