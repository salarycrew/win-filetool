//! 보이지 않는 글자 층(검색·선택·복사용).
//!
//! 글꼴: 그림이 없는 TrueType(글리프 2개: .notdef, 빈 글리프)을 이 코드가 직접 만든다 — Tesseract의 GlyphLessFont(Apache-2.0)와 같은 생각.
//! Type0 / Identity-H, 글자 코드 = 유니코드(BMP) 2바이트, CIDToGIDMap은 모든 코드를 빈 글리프 1번으로,
//! ToUnicode는 코드 = 유니코드(대리 영역 제외)로 적는다. 한글 글꼴을 넣지 않아도 한글 검색·복사가 된다.
//! 글자는 렌더 모드 3(안 보임)으로 낱말 상자 위에 놓고, 가로 비율(Tz)로 낱말 너비를 맞춘다.

use lopdf::{dictionary, Document, Object, ObjectId, Stream};

use crate::pdfedit::{f, mat_mul, Matrix, PageGeom};

/// 글리프 하나의 너비(1000 = 글자 크기)
pub const ADVANCE: f32 = 500.0;
/// 글자 상자 높이 중 기준선 아래 비율(Descent 200 / 1000)
pub const DESCENT: f32 = 0.2;

fn be16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}
fn be32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

fn checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for c in data.chunks(4) {
        let mut b = [0u8; 4];
        b[..c.len()].copy_from_slice(c);
        sum = sum.wrapping_add(u32::from_be_bytes(b));
    }
    sum
}

/// 글리프 없는(그리지 않는) TrueType 글꼴 바이트(약 600바이트)
pub fn glyphless_ttf() -> Vec<u8> {
    let mut tables: Vec<(&[u8; 4], Vec<u8>)> = Vec::new();
    // cmap: (3,1) format 4, 끝 구간 하나(0xFFFF)만 — 글자 코드가 아니라 CIDToGIDMap으로 글리프를 고르므로 비워 둔다
    let mut cmap = Vec::new();
    be16(&mut cmap, 0);
    be16(&mut cmap, 1);
    be16(&mut cmap, 3);
    be16(&mut cmap, 1);
    be32(&mut cmap, 12);
    for v in [4u16, 24, 0, 2, 2, 0, 0, 0xFFFF, 0, 0xFFFF, 1, 0] {
        be16(&mut cmap, v);
    }
    tables.push((b"cmap", cmap));
    // glyf: 0번은 비어 있고, 1번은 글자 상자(0,-200)~(500,800) 사각형 하나. 렌더 모드 3이라 그려지지 않는다.
    // (윤곽이 없는 글리프를 90도로 돌리면 PDFium이 글자 상자를 0으로 보고 글자 추출에서 빼서 윤곽을 둔다)
    let mut glyf = Vec::new();
    for v in [1i16, 0, -200, ADVANCE as i16, 800, 3, 0] {
        be16(&mut glyf, v as u16); // 윤곽 1개, 상자, 끝점 번호 3, 명령 길이 0
    }
    glyf.extend_from_slice(&[1, 1, 1, 1]); // 네 점 모두 곡선 위, x·y는 2바이트 차이값
    for v in [0i16, 0, ADVANCE as i16, 0, -200, 1000, 0, -1000] {
        be16(&mut glyf, v as u16);
    }
    while glyf.len() % 4 != 0 {
        glyf.push(0);
    }
    let glyf_len = glyf.len() as u16;
    tables.push((b"glyf", glyf));
    // head
    let mut head = Vec::new();
    be32(&mut head, 0x0001_0000);
    be32(&mut head, 0x0001_0000);
    be32(&mut head, 0); // checkSumAdjustment(나중에)
    be32(&mut head, 0x5F0F_3CF5);
    be16(&mut head, 0x000B);
    be16(&mut head, 1000);
    head.extend_from_slice(&[0u8; 16]); // created, modified
    for v in [0i16, -200, ADVANCE as i16, 800] {
        be16(&mut head, v as u16);
    }
    be16(&mut head, 0); // macStyle
    be16(&mut head, 8); // lowestRecPPEM
    be16(&mut head, 2); // fontDirectionHint
    be16(&mut head, 0); // indexToLocFormat: short
    be16(&mut head, 0); // glyphDataFormat
    tables.push((b"head", head));
    // hhea
    let mut hhea = Vec::new();
    be32(&mut hhea, 0x0001_0000);
    for v in [800i16, -200, 0] {
        be16(&mut hhea, v as u16);
    }
    be16(&mut hhea, ADVANCE as u16);
    for v in [0i16, 0, ADVANCE as i16, 1, 0, 0, 0, 0, 0, 0, 0] {
        be16(&mut hhea, v as u16);
    }
    be16(&mut hhea, 2); // numberOfHMetrics
    tables.push((b"hhea", hhea));
    // hmtx
    let mut hmtx = Vec::new();
    for _ in 0..2 {
        be16(&mut hmtx, ADVANCE as u16);
        be16(&mut hmtx, 0);
    }
    tables.push((b"hmtx", hmtx));
    let mut loca = Vec::new();
    for v in [0u16, 0, glyf_len / 2] {
        be16(&mut loca, v);
    }
    tables.push((b"loca", loca));
    // maxp 1.0
    let mut maxp = Vec::new();
    be32(&mut maxp, 0x0001_0000);
    be16(&mut maxp, 2);
    for v in [4u16, 1, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0] {
        be16(&mut maxp, v);
    }
    tables.push((b"maxp", maxp));
    // name: 1(계열), 4(전체), 6(PostScript) — Windows 유니코드
    let name_s: Vec<u8> = "GlyphLessFont".encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
    let mut name = Vec::new();
    let ids = [1u16, 4, 6];
    be16(&mut name, 0);
    be16(&mut name, ids.len() as u16);
    be16(&mut name, 6 + 12 * ids.len() as u16);
    for id in ids {
        for v in [3u16, 1, 0x409, id, name_s.len() as u16, 0] {
            be16(&mut name, v);
        }
    }
    name.extend_from_slice(&name_s);
    tables.push((b"name", name));
    // post 3.0
    let mut post = Vec::new();
    be32(&mut post, 0x0003_0000);
    be32(&mut post, 0);
    be16(&mut post, (-100i16) as u16);
    be16(&mut post, 50);
    be32(&mut post, 1);
    post.extend_from_slice(&[0u8; 16]);
    tables.push((b"post", post));

    tables.sort_by(|a, b| a.0.cmp(b.0));
    let n = tables.len() as u16;
    let mut es = 0u16;
    while (1u16 << (es + 1)) <= n {
        es += 1;
    }
    let sr = (1u16 << es) * 16;
    let mut out = Vec::new();
    be32(&mut out, 0x0001_0000);
    be16(&mut out, n);
    be16(&mut out, sr);
    be16(&mut out, es);
    be16(&mut out, n * 16 - sr);
    let mut offset = 12 + 16 * n as u32;
    let mut body = Vec::new();
    let mut head_off = 0usize;
    for (tag, data) in &tables {
        out.extend_from_slice(*tag);
        be32(&mut out, checksum(data));
        be32(&mut out, offset);
        be32(&mut out, data.len() as u32);
        if *tag == b"head" {
            head_off = (offset as usize) + 8;
        }
        let mut d = data.clone();
        while d.len() % 4 != 0 {
            d.push(0);
        }
        offset += d.len() as u32;
        body.extend_from_slice(&d);
    }
    out.extend_from_slice(&body);
    let adj = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
    out[head_off..head_off + 4].copy_from_slice(&adj.to_be_bytes());
    out
}

/// ToUnicode CMap: 코드 = 유니코드(0000~FFFF, 대리 영역 D800~DFFF 제외)
fn to_unicode_cmap() -> String {
    let mut s = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let hi: Vec<u16> = (0u16..=0xFF).filter(|h| !(0xD8..=0xDF).contains(h)).collect();
    for chunk in hi.chunks(100) {
        s.push_str(&format!("{} beginbfrange\n", chunk.len()));
        for h in chunk {
            s.push_str(&format!("<{h:02X}00> <{h:02X}FF> <{h:02X}00>\n"));
        }
        s.push_str("endbfrange\n");
    }
    s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    s
}

/// 문서에 글리프 없는 Type0 글꼴을 넣고 그 id(쪽마다 리소스로 붙인다)
pub fn add_font(doc: &mut Document) -> ObjectId {
    let ttf = glyphless_ttf();
    let mut ff = Stream::new(dictionary! { "Length1" => ttf.len() as i64 }, ttf);
    let _ = ff.compress();
    let ff = doc.add_object(ff);
    let fd = doc.add_object(dictionary! {
        "Type" => "FontDescriptor", "FontName" => "GlyphLessFont", "Flags" => 5,
        "FontBBox" => vec![0.into(), (-200).into(), (ADVANCE as i64).into(), 800.into()],
        "ItalicAngle" => 0, "Ascent" => 800, "Descent" => -200, "CapHeight" => 800, "StemV" => 80,
        "FontFile2" => ff,
    });
    let mut map = Vec::with_capacity(0x20000);
    for _ in 0..0x10000 {
        map.extend_from_slice(&[0, 1]);
    }
    let mut c2g = Stream::new(dictionary! {}, map);
    let _ = c2g.compress();
    let c2g = doc.add_object(c2g);
    let cid = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "CIDFontType2", "BaseFont" => "GlyphLessFont",
        "CIDSystemInfo" => dictionary! { "Registry" => Object::string_literal("Adobe"), "Ordering" => Object::string_literal("Identity"), "Supplement" => 0 },
        "FontDescriptor" => fd, "DW" => ADVANCE as i64, "CIDToGIDMap" => c2g,
    });
    let mut tu = Stream::new(dictionary! {}, to_unicode_cmap().into_bytes());
    let _ = tu.compress();
    let tu = doc.add_object(tu);
    doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type0", "BaseFont" => "GlyphLessFont", "Encoding" => "Identity-H",
        "DescendantFonts" => vec![Object::Reference(cid)], "ToUnicode" => tu,
    })
}

/// 쪽 위에 놓을 낱말(보이는 쪽 pt, 원점 왼쪽 위)
#[derive(Debug, Clone)]
pub struct Placed {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// 글 방향(시계 방향)
    pub rot: u16,
}

fn hex_utf16(s: &str) -> String {
    let mut h = String::with_capacity(s.len() * 4);
    for c in s.chars() {
        let u = c as u32;
        let u = if u > 0xFFFF { 0xFFFD } else { u };
        h.push_str(&format!("{u:04X}"));
    }
    h
}

/// 줄 안 낱말: 다음 낱말까지 빈칸을 붙여 너비를 늘린다(복사하면 낱말 사이 빈칸이 생김).
pub fn line_words(words: &[Placed]) -> Vec<Placed> {
    let along = |p: &Placed| -> (f32, f32) {
        match p.rot {
            90 => (p.y, p.y + p.h),
            180 => (-(p.x + p.w), -p.x),
            270 => (-(p.y + p.h), -p.y),
            _ => (p.x, p.x + p.w),
        }
    };
    let mut v: Vec<Placed> = words.to_vec();
    v.sort_by(|a, b| along(a).0.partial_cmp(&along(b).0).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let mut p = v[i].clone();
        if i + 1 < n {
            let (a0, a1) = along(&p);
            let next0 = along(&v[i + 1]).0;
            let gap = next0 - a1;
            // 너무 먼 낱말(다른 칸)은 빈칸만 붙이고 늘리지 않는다
            let thick = if p.rot % 180 == 0 { p.h } else { p.w };
            if gap > 0.0 && gap < thick * 3.0 {
                let ext = next0 - a0;
                match p.rot {
                    90 => p.h = ext,
                    180 => {
                        p.x = p.x + p.w - ext;
                        p.w = ext
                    }
                    270 => {
                        p.y = p.y + p.h - ext;
                        p.h = ext
                    }
                    _ => p.w = ext,
                }
            }
            p.text.push(' ');
        }
        out.push(p);
    }
    out
}

/// 보이지 않는 글자 내용 스트림 조각(`BT … ET`). `font` = 쪽 리소스 이름(예: `OCR1`).
pub fn ops(font: &str, geom: &PageGeom, words: &[Placed]) -> String {
    let (_, vh) = geom.visual_size();
    let v2u = geom.visual_to_user();
    let mut s = String::from("BT\n3 Tr\n");
    for p in words {
        let n = p.text.chars().count();
        if n == 0 || p.w <= 0.0 || p.h <= 0.0 {
            continue;
        }
        // 보이는 쪽(아래 원점, y 위로)에서 글 방향 d, 글자 위쪽 u, 기준선 시작점 o
        let (len, thick, d, u, o): (f32, f32, (f32, f32), (f32, f32), (f32, f32)) = match p.rot {
            90 => (p.h, p.w, (0.0, -1.0), (1.0, 0.0), (p.x + DESCENT * p.w, vh - p.y)),
            180 => (p.w, p.h, (-1.0, 0.0), (0.0, -1.0), (p.x + p.w, vh - (p.y + DESCENT * p.h))),
            270 => (p.h, p.w, (0.0, 1.0), (-1.0, 0.0), (p.x + p.w - DESCENT * p.w, vh - (p.y + p.h))),
            _ => (p.w, p.h, (1.0, 0.0), (0.0, 1.0), (p.x, vh - (p.y + p.h) + DESCENT * p.h)),
        };
        let size = thick.max(0.5);
        let natural = n as f32 * ADVANCE / 1000.0 * size;
        let tz = (100.0 * len / natural).clamp(1.0, 5000.0);
        let tm: Matrix = mat_mul([d.0, d.1, u.0, u.1, o.0, o.1], v2u);
        s.push_str(&format!(
            "/{font} {} Tf {} Tz {} {} {} {} {} {} Tm <{}> Tj\n",
            f(size),
            f(tz),
            f(tm[0]),
            f(tm[1]),
            f(tm[2]),
            f(tm[3]),
            f(tm[4]),
            f(tm[5]),
            hex_utf16(&p.text)
        ));
    }
    s.push_str("ET\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyphless_font_parses() {
        let ttf = glyphless_ttf();
        let face = ttf_parser::Face::parse(&ttf, 0).expect("TrueType로 읽혀야 함");
        assert_eq!(face.number_of_glyphs(), 2);
        assert_eq!(face.units_per_em(), 1000);
        assert_eq!(face.glyph_hor_advance(ttf_parser::GlyphId(1)), Some(ADVANCE as u16));
        let bb = face.glyph_bounding_box(ttf_parser::GlyphId(1)).expect("1번 글리프 상자");
        assert_eq!((bb.x_min, bb.y_min, bb.x_max, bb.y_max), (0, -200, ADVANCE as i16, 800));
        // 전체 체크섬 = 0xB1B0AFBA
        assert_eq!(checksum(&ttf), 0xB1B0_AFBA);
    }

    #[test]
    fn to_unicode_skips_surrogates() {
        let c = to_unicode_cmap();
        assert!(c.contains("<AC00> <ACFF> <AC00>"));
        assert!(!c.contains("<D800>"));
        assert_eq!(c.matches("beginbfrange").count(), 3);
    }

    #[test]
    fn words_get_spaces_and_widths() {
        let w = |t: &str, x: f32| Placed { text: t.into(), x, y: 10.0, w: 20.0, h: 10.0, rot: 0 };
        let out = line_words(&[w("B", 40.0), w("A", 10.0)]);
        assert_eq!(out[0].text, "A ");
        assert_eq!(out[0].w, 30.0);
        assert_eq!(out[1].text, "B");
        assert_eq!(hex_utf16("가A"), "AC000041");
    }
}
