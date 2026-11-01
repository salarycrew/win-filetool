//! 점검용 PDF·그림 만들기(테스트와 `filetool-cli pdfedit-selftest`가 함께 씀).
//!
//! - [`make_pages`]: 크기·회전이 섞인 PDF. 쪽마다 큰 글자 `P1`…과 테두리(렌더 확인용)
//! - [`make_form`]: 글자·체크 상자·라디오·콤보·목록 칸 + 메모 주석 + 링크가 있는 양식 PDF
//! - [`make_signature_png`]: 흰 바탕에 검은 선(서명 흉내)

use std::path::Path;

use lopdf::{dictionary, Document, Object, Stream};

use crate::error::{io_err, Result};

/// (가로 pt, 세로 pt, /Rotate)
pub type PageSpec = (f32, f32, i64);

fn helv(doc: &mut Document) -> lopdf::ObjectId {
    doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "Encoding" => "WinAnsiEncoding" })
}

fn finish(mut doc: Document, pages_id: lopdf::ObjectId, kids: Vec<Object>, extra_catalog: Option<(&str, Object)>, path: &Path) -> Result<()> {
    let n = kids.len() as i64;
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => n }));
    let mut cat = dictionary! { "Type" => "Catalog", "Pages" => pages_id };
    if let Some((k, v)) = extra_catalog {
        cat.set(k, v);
    }
    let catalog = doc.add_object(cat);
    doc.trailer.set("Root", catalog);
    let info = doc.add_object(dictionary! { "Title" => Object::string_literal("Sample"), "Producer" => Object::string_literal("win-filetool test") });
    doc.trailer.set("Info", info);
    doc.compress();
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(io_err(path))?;
    crate::paths::write_new(path, &bytes)
}

/// 쪽마다 `label`+번호를 왼쪽 위에 크게(기준선 = 위에서 90pt), 쪽 가장자리 안쪽 21pt에 회색 테두리 선.
pub fn make_pages(path: &Path, pages: &[PageSpec], label: &str) -> Result<()> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font = helv(&mut doc);
    let mut kids = Vec::new();
    for (i, &(w, h, rot)) in pages.iter().enumerate() {
        let ops = format!(
            "0.6 G 2 w 21 21 {} {} re S\n0 g BT /F1 40 Tf 40 {} Td ({}{}) Tj ET\n",
            w - 42.0,
            h - 42.0,
            h - 90.0,
            label,
            i + 1
        );
        let c = doc.add_object(Stream::new(dictionary! {}, ops.into_bytes()));
        let mut pd = dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => c,
            "MediaBox" => vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
        };
        if rot != 0 {
            pd.set("Rotate", rot);
        }
        kids.push(Object::Reference(doc.add_object(pd)));
    }
    finish(doc, pages_id, kids, None, path)
}

/// 같은 A4 세로 쪽 `n`장(속도 재기용)
pub fn make_many(path: &Path, n: u32) -> Result<()> {
    let specs: Vec<PageSpec> = (0..n).map(|_| (595.0, 842.0, 0)).collect();
    make_pages(path, &specs, "P")
}

fn ap(doc: &mut Document, w: f32, h: f32, ops: &str, font: Option<lopdf::ObjectId>) -> lopdf::ObjectId {
    let mut d = dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)] };
    if let Some(f) = font {
        d.set("Resources", dictionary! { "Font" => dictionary! { "Helv" => f } });
    }
    doc.add_object(Stream::new(d, ops.as_bytes().to_vec()))
}

/// 양식 칸 5종 + 메모 주석(모양 있음) + 링크(모양 없음). A4 세로 한 쪽.
///
/// 칸 이름: `name`(글자), `agree`(체크, 켜짐 상태 `Yes`), `size`(라디오 `S`/`M`), `city`(콤보: Seoul, Busan, 서울),
/// `color`(목록: Red, Blue)
pub fn make_form(path: &Path) -> Result<()> {
    let mut doc = Document::with_version("1.6");
    let pages_id = doc.new_object_id();
    let page_id = doc.new_object_id();
    let font = helv(&mut doc);
    let border = |w: f32, h: f32| format!("0 G 1 w 0.5 0.5 {} {} re S\n", w - 1.0, h - 1.0);
    let mut annots = Vec::new();
    let mut fields = Vec::new();

    // 글자 칸
    let name_ap = ap(&mut doc, 200.0, 24.0, &border(200.0, 24.0), Some(font));
    let name = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Tx", "T" => Object::string_literal("name"),
        "Rect" => vec![100.into(), 700.into(), 300.into(), 724.into()], "P" => page_id, "F" => 4,
        "DA" => Object::string_literal("/Helv 12 Tf 0 g"), "AP" => dictionary! { "N" => name_ap },
    });
    annots.push(Object::Reference(name));
    fields.push(Object::Reference(name));

    // 체크 상자
    let on = ap(&mut doc, 20.0, 20.0, &format!("{}0 g 4 4 12 12 re f\n", border(20.0, 20.0)), None);
    let off = ap(&mut doc, 20.0, 20.0, &border(20.0, 20.0), None);
    let agree = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Btn", "T" => Object::string_literal("agree"),
        "Rect" => vec![100.into(), 650.into(), 120.into(), 670.into()], "P" => page_id, "F" => 4,
        "V" => "Off", "AS" => "Off", "AP" => dictionary! { "N" => dictionary! { "Yes" => on, "Off" => off } },
    });
    annots.push(Object::Reference(agree));
    fields.push(Object::Reference(agree));

    // 라디오(부모 + 위젯 둘)
    let radio_parent = doc.new_object_id();
    let mut kids = Vec::new();
    for (i, st) in ["S", "M"].iter().enumerate() {
        let x = 100.0 + i as f32 * 40.0;
        let on = ap(&mut doc, 20.0, 20.0, &format!("{}0 g 6 6 8 8 re f\n", border(20.0, 20.0)), None);
        let off = ap(&mut doc, 20.0, 20.0, &border(20.0, 20.0), None);
        let mut states = lopdf::Dictionary::new();
        states.set(st.as_bytes().to_vec(), on);
        states.set("Off", off);
        let w = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Widget", "Parent" => radio_parent,
            "Rect" => vec![Object::Real(x), 600.into(), Object::Real(x + 20.0), 620.into()], "P" => page_id, "F" => 4,
            "AS" => "Off", "AP" => dictionary! { "N" => states },
        });
        kids.push(Object::Reference(w));
        annots.push(Object::Reference(w));
    }
    doc.objects.insert(
        radio_parent,
        Object::Dictionary(dictionary! { "FT" => "Btn", "Ff" => (1 << 15) | (1 << 14), "T" => Object::string_literal("size"), "V" => "Off", "Kids" => kids }),
    );
    fields.push(Object::Reference(radio_parent));

    // 콤보
    let city_ap = ap(&mut doc, 150.0, 22.0, &border(150.0, 22.0), Some(font));
    let city = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Ch", "Ff" => 1 << 17, "T" => Object::string_literal("city"),
        "Opt" => vec![Object::string_literal("Seoul"), Object::string_literal("Busan"), lopdf::text_string("서울")],
        "Rect" => vec![100.into(), 550.into(), 250.into(), 572.into()], "P" => page_id, "F" => 4,
        "DA" => Object::string_literal("/Helv 11 Tf 0 g"), "AP" => dictionary! { "N" => city_ap },
    });
    annots.push(Object::Reference(city));
    fields.push(Object::Reference(city));

    // 목록
    let color_ap = ap(&mut doc, 150.0, 40.0, &border(150.0, 40.0), Some(font));
    let color = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Widget", "FT" => "Ch", "T" => Object::string_literal("color"),
        "Opt" => vec![Object::string_literal("Red"), Object::string_literal("Blue")],
        "Rect" => vec![100.into(), 490.into(), 250.into(), 530.into()], "P" => page_id, "F" => 4,
        "DA" => Object::string_literal("/Helv 10 Tf 0 g"), "AP" => dictionary! { "N" => color_ap },
    });
    annots.push(Object::Reference(color));
    fields.push(Object::Reference(color));

    // 메모 주석(빨간 네모 모양) + 링크
    let note_ap = ap(&mut doc, 30.0, 30.0, "1 0 0 rg 0 0 30 30 re f\n", None);
    let note = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Square", "Rect" => vec![400.into(), 400.into(), 430.into(), 430.into()], "F" => 4,
        "Contents" => Object::string_literal("note"), "AP" => dictionary! { "N" => note_ap },
    });
    annots.push(Object::Reference(note));
    let link = doc.add_object(dictionary! {
        "Type" => "Annot", "Subtype" => "Link", "Rect" => vec![100.into(), 300.into(), 200.into(), 320.into()],
        "A" => dictionary! { "S" => "URI", "URI" => Object::string_literal("https://example.com") },
    });
    annots.push(Object::Reference(link));

    let c = doc.add_object(Stream::new(dictionary! {}, b"0 g BT /F1 14 Tf 100 750 Td (Form sample) Tj ET\n".to_vec()));
    doc.objects.insert(
        page_id,
        Object::Dictionary(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => c,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
            "Annots" => annots,
        }),
    );
    let acro = dictionary! {
        "Fields" => fields,
        "DA" => Object::string_literal("/Helv 0 Tf 0 g"),
        "DR" => dictionary! { "Font" => dictionary! { "Helv" => font } },
    };
    finish(doc, pages_id, vec![Object::Reference(page_id)], Some(("AcroForm", Object::Dictionary(acro))), path)
}

/// 흰 바탕 300×120에 검은 굵은 선 몇 개(서명 흉내)
pub fn make_signature_png(path: &Path) -> Result<()> {
    let mut img = image::RgbImage::from_pixel(300, 120, image::Rgb([255, 255, 255]));
    for x in 20..280u32 {
        let t = x as f32 / 30.0;
        let y = (60.0 + t.sin() * 30.0) as i32;
        for dy in -3..=3 {
            for dx in 0..2 {
                let (px, py) = (x + dx, (y + dy) as u32);
                if px < 300 && py < 120 {
                    img.put_pixel(px, py, image::Rgb([10, 10, 10]));
                }
            }
        }
    }
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(img).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).map_err(|e| crate::Error::Invalid(e.to_string()))?;
    crate::paths::write_new(path, &bytes)
}
