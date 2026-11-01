//! 한글 글자를 PDF에 쓰기 위한 글꼴.
//!
//! 라이선스: 앱에 글꼴 파일을 넣지 않는다. 실행 중에 **그 PC에 있는 글꼴**(Windows는 `C:\Windows\Fonts\malgun.ttf` 맑은 고딕)을
//! 읽어, 쓴 글자만 부분 글꼴(subset)로 결과 PDF에 넣는다. 글꼴의 OS/2 `fsType`이 "넣기 금지(Restricted)"거나
//! 부분 넣기를 막은 글꼴은 건너뛴다. 맑은 고딕은 넣기 허용 글꼴이다(편집 가능 넣기).
//! 찾는 순서: 환경 변수 `WIN_FILETOOL_FONT`(파일, `경로#번호`로 TTC 번호 지정 가능) → 맑은 고딕 → 나눔고딕 → 굴림 →
//! (Linux·macOS 테스트용) 나눔고딕·Noto Sans CJK KR·Apple SD 산돌고딕 Neo.
//!
//! PDF에는 Type0(Identity-H) + CIDFontType2(TrueType 윤곽) 또는 CIDFontType0(CFF 윤곽)으로 넣고,
//! 글자 복사가 되도록 ToUnicode CMap을 붙인다. 부분 글꼴 만들기는 `subsetter`(MIT/Apache-2.0),
//! 글꼴 읽기는 `ttf-parser`(MIT/Apache-2.0).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lopdf::{dictionary, Document, Object, ObjectId, Stream};
use subsetter::GlyphRemapper;

use crate::error::{Error, Result};

pub struct FontData {
    pub path: PathBuf,
    pub index: u32,
    data: Vec<u8>,
    pub name: String,
    cff: bool,
    upem: f32,
    ascender: f32,
    descender: f32,
    cap_height: f32,
    bbox: [f32; 4],
    cmap: BTreeMap<u32, u16>,
    advances: Vec<u16>,
}

impl std::fmt::Debug for FontData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FontData({} #{} {})", self.path.display(), self.index, self.name)
    }
}

const HINT: &str = "글자를 넣으려면 한글 글꼴이 필요합니다. Windows에 기본으로 있는 맑은 고딕(C:\\Windows\\Fonts\\malgun.ttf)을 찾지 못했습니다";

fn candidates() -> Vec<(PathBuf, Option<u32>)> {
    let mut v: Vec<(PathBuf, Option<u32>)> = Vec::new();
    if let Some(e) = std::env::var_os("WIN_FILETOOL_FONT") {
        let s = e.to_string_lossy().into_owned();
        match s.rsplit_once('#') {
            Some((p, n)) if n.parse::<u32>().is_ok() => v.push((PathBuf::from(p), n.parse().ok())),
            _ => v.push((PathBuf::from(s), None)),
        }
    }
    let win = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let fonts = win.join("Fonts");
    for n in ["malgun.ttf", "NanumGothic.ttf", "gulim.ttc"] {
        v.push((fonts.join(n), None));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        v.push((local.join(r"Microsoft\Windows\Fonts\NanumGothic.ttf"), None));
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        v.push((home.join(".local/share/fonts/nanum/NanumGothic.ttf"), None));
        v.push((home.join(".local/share/fonts/NanumGothic.ttf"), None));
        v.push((home.join(".fonts/NanumGothic.ttf"), None));
    }
    for p in [
        "/usr/share/fonts/truetype/nanum/NanumGothic.ttf",
        "/usr/share/fonts/nanum/NanumGothic.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    ] {
        v.push((PathBuf::from(p), None));
    }
    v
}

/// 쓸 수 있는 한글 글꼴(처음 한 번 찾고 기억)
pub fn korean_font() -> Result<Arc<FontData>> {
    static CACHE: Mutex<Option<Arc<FontData>>> = Mutex::new(None);
    let mut g = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(f) = g.as_ref() {
        return Ok(f.clone());
    }
    let mut why = Vec::new();
    for (path, index) in candidates() {
        if !path.is_file() {
            continue;
        }
        match load_font(&path, index) {
            Ok(f) => {
                let f = Arc::new(f);
                *g = Some(f.clone());
                return Ok(f);
            }
            Err(e) => why.push(format!("{}: {e}", path.display())),
        }
    }
    let hint = if why.is_empty() { HINT.to_string() } else { format!("{HINT} ({})", why.join("; ")) };
    Err(Error::ToolMissing { tool: "한글 글꼴".into(), hint })
}

/// 글꼴 파일 하나 읽기. TTC면 `index`(없으면 한국어 얼굴을 고름).
pub fn load_font(path: &Path, index: Option<u32>) -> std::result::Result<FontData, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let n = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
    let index = match index {
        Some(i) => i,
        None => {
            // 이름에 KR/Korean이 들어간 얼굴 우선
            let mut pick = 0;
            for i in 0..n {
                if let Ok(face) = ttf_parser::Face::parse(&data, i) {
                    let fam = family_name(&face).unwrap_or_default();
                    if fam.contains("KR") || fam.contains("Korean") {
                        pick = i;
                        break;
                    }
                }
            }
            pick
        }
    };
    let face = ttf_parser::Face::parse(&data, index).map_err(|e| format!("글꼴을 읽지 못했습니다: {e}"))?;
    if matches!(face.permissions(), Some(ttf_parser::Permissions::Restricted)) {
        return Err("이 글꼴은 PDF에 넣는 것이 금지되어 있습니다".into());
    }
    if !face.is_subsetting_allowed() {
        return Err("이 글꼴은 부분 넣기가 금지되어 있습니다".into());
    }
    if face.glyph_index('가').is_none() {
        return Err("한글 글자가 없는 글꼴입니다".into());
    }
    let cff = face.tables().glyf.is_none();
    let mut cmap = BTreeMap::new();
    if let Some(t) = face.tables().cmap {
        for sub in t.subtables {
            if !sub.is_unicode() {
                continue;
            }
            sub.codepoints(|c| {
                if let Some(gid) = sub.glyph_index(c) {
                    cmap.entry(c).or_insert(gid.0);
                }
            });
        }
    }
    let advances = (0..face.number_of_glyphs()).map(|g| face.glyph_hor_advance(ttf_parser::GlyphId(g)).unwrap_or(0)).collect();
    let b = face.global_bounding_box();
    let name = face
        .names()
        .into_iter()
        .find(|n| n.name_id == ttf_parser::name_id::POST_SCRIPT_NAME)
        .and_then(|n| n.to_string().or_else(|| String::from_utf8(n.name.to_vec()).ok()))
        .unwrap_or_else(|| "KoreanFont".into());
    let name: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    let upem = face.units_per_em() as f32;
    Ok(FontData {
        path: path.to_path_buf(),
        index,
        name: if name.is_empty() { "KoreanFont".into() } else { name },
        cff,
        upem,
        ascender: face.ascender() as f32,
        descender: face.descender() as f32,
        cap_height: face.capital_height().map(|v| v as f32).unwrap_or(face.ascender() as f32 * 0.7),
        bbox: [b.x_min as f32, b.y_min as f32, b.x_max as f32, b.y_max as f32],
        cmap,
        advances,
        data,
    })
}

fn family_name(face: &ttf_parser::Face) -> Option<String> {
    face.names()
        .into_iter()
        .filter(|n| n.name_id == ttf_parser::name_id::FAMILY || n.name_id == ttf_parser::name_id::FULL_NAME)
        .find_map(|n| n.to_string())
}

impl FontData {
    fn gid(&self, c: char) -> u16 {
        self.cmap.get(&(c as u32)).copied().unwrap_or(0)
    }
    /// 1000 단위 너비
    fn adv1000(&self, gid: u16) -> f32 {
        self.advances.get(gid as usize).copied().unwrap_or(0) as f32 * 1000.0 / self.upem
    }
    /// 글자 크기 1일 때 위로 올라가는 높이(pt)
    pub fn ascent(&self) -> f32 {
        self.ascender / self.upem
    }
    pub fn descent(&self) -> f32 {
        self.descender / self.upem
    }
    pub fn cap(&self) -> f32 {
        self.cap_height / self.upem
    }
    /// 이 글꼴에 없는 글자(빈 네모로 나옴)
    pub fn missing(&self, text: &str) -> Vec<char> {
        let mut v: Vec<char> = text.chars().filter(|c| !c.is_control() && self.gid(*c) == 0).collect();
        v.dedup();
        v
    }
}

/// 문서 하나에 글꼴 하나: 쓴 글자를 모았다가 [`PdfFont::finish`]에서 부분 글꼴로 넣는다.
pub struct PdfFont {
    font: Arc<FontData>,
    remap: GlyphRemapper,
    /// 새 gid → (원래 gid, 글자)
    used: BTreeMap<u16, (u16, char)>,
    /// 미리 잡아 둔 Type0 글꼴 객체 id(쪽 리소스에서 참조)
    pub id: ObjectId,
}

impl PdfFont {
    pub fn new(doc: &mut Document) -> Result<Self> {
        let font = korean_font()?;
        Ok(Self::with_font(doc, font))
    }
    pub fn with_font(doc: &mut Document, font: Arc<FontData>) -> Self {
        let id = doc.new_object_id();
        // 자리만 잡아 둔다(finish에서 채움)
        doc.objects.insert(id, Object::Null);
        PdfFont { font, remap: GlyphRemapper::new(), used: BTreeMap::new(), id }
    }
    pub fn data(&self) -> &FontData {
        &self.font
    }
    /// 글자 크기 `size`일 때 너비(pt)
    pub fn width(&self, text: &str, size: f32) -> f32 {
        text.chars().filter(|c| !c.is_control()).map(|c| self.font.adv1000(self.font.gid(c))).sum::<f32>() * size / 1000.0
    }
    /// 내용 스트림용 `<…>` 글자열(새 gid 2바이트씩)
    pub fn encode(&mut self, text: &str) -> String {
        let mut s = String::with_capacity(text.len() * 4 + 2);
        s.push('<');
        for c in text.chars().filter(|c| !c.is_control()) {
            let old = self.font.gid(c);
            let new = self.remap.remap(old);
            self.used.entry(new).or_insert((old, c));
            s.push_str(&format!("{new:04X}"));
        }
        s.push('>');
        s
    }
    /// `BT … ET` 한 줄. (x, y)는 글자 기준선 왼쪽 끝.
    pub fn show(&mut self, res_name: &str, size: f32, x: f32, y: f32, text: &str) -> String {
        let hex = self.encode(text);
        format!("BT /{} {} Tf {} {} Td {} Tj ET\n", res_name, super::f(size), super::f(x), super::f(y), hex)
    }
    /// 글자를 일정 너비에서 줄바꿈(글자 단위, 빈칸 우선)
    pub fn wrap(&self, text: &str, size: f32, max_w: f32) -> Vec<String> {
        let mut out = Vec::new();
        for para in text.split('\n') {
            let para = para.trim_end_matches('\r');
            if max_w <= 0.0 || self.width(para, size) <= max_w {
                out.push(para.to_string());
                continue;
            }
            let mut line = String::new();
            for c in para.chars() {
                let mut cand = line.clone();
                cand.push(c);
                if self.width(&cand, size) > max_w && !line.is_empty() {
                    // 빈칸에서 끊을 수 있으면 거기서
                    if let Some(pos) = line.rfind(' ') {
                        let rest = line[pos + 1..].to_string();
                        out.push(line[..pos].to_string());
                        line = rest;
                        line.push(c);
                    } else {
                        out.push(std::mem::take(&mut line));
                        line.push(c);
                    }
                } else {
                    line = cand;
                }
            }
            out.push(line);
        }
        out
    }

    /// 쓴 글자만 담은 부분 글꼴을 만들어 미리 잡은 id에 Type0 글꼴을 넣는다.
    pub fn finish(self, doc: &mut Document) -> Result<()> {
        let font = &self.font;
        let sub = subsetter::subset(&font.data, font.index, &self.remap)
            .map_err(|e| Error::Invalid(format!("글꼴을 PDF에 넣지 못했습니다({}): {e:?}", font.path.display())))?;
        // 부분 글꼴 표시(6글자 대문자) — 쓴 글자에서 만든다
        let mut h: u32 = 2166136261;
        for (k, (o, _)) in &self.used {
            h = (h ^ (*k as u32) ^ ((*o as u32) << 16)).wrapping_mul(16777619);
        }
        let tag: String = (0..6).map(|i| (b'A' + ((h >> (i * 5)) % 26) as u8) as char).collect();
        let base = format!("{tag}+{}", font.name);
        let scale = 1000.0 / font.upem;
        let mut ff = Stream::new(dictionary! {}, sub);
        if font.cff {
            ff.dict.set("Subtype", "OpenType");
        }
        let _ = ff.compress();
        let ff_id = doc.add_object(ff);
        let mut desc = dictionary! {
            "Type" => "FontDescriptor",
            "FontName" => Object::Name(base.clone().into_bytes()),
            "Flags" => 4,
            "FontBBox" => vec![
                Object::Integer((font.bbox[0] * scale) as i64), Object::Integer((font.bbox[1] * scale) as i64),
                Object::Integer((font.bbox[2] * scale) as i64), Object::Integer((font.bbox[3] * scale) as i64),
            ],
            "ItalicAngle" => 0,
            "Ascent" => Object::Integer((font.ascender * scale) as i64),
            "Descent" => Object::Integer((font.descender * scale) as i64),
            "CapHeight" => Object::Integer((font.cap_height * scale) as i64),
            "StemV" => 80,
        };
        desc.set(if font.cff { "FontFile3" } else { "FontFile2" }, Object::Reference(ff_id));
        let desc_id = doc.add_object(desc);
        // 너비: [cid [w] cid [w] …]
        let mut w = Vec::new();
        for (new, (old, _)) in &self.used {
            w.push(Object::Integer(*new as i64));
            w.push(Object::Array(vec![Object::Integer(font.adv1000(*old).round() as i64)]));
        }
        let mut cid = dictionary! {
            "Type" => "Font",
            "Subtype" => if font.cff { "CIDFontType0" } else { "CIDFontType2" },
            "BaseFont" => Object::Name(base.clone().into_bytes()),
            "CIDSystemInfo" => dictionary! {
                "Registry" => Object::string_literal("Adobe"),
                "Ordering" => Object::string_literal("Identity"),
                "Supplement" => 0,
            },
            "FontDescriptor" => desc_id,
            "DW" => 1000,
            "W" => w,
        };
        if !font.cff {
            cid.set("CIDToGIDMap", "Identity");
        }
        let cid_id = doc.add_object(cid);
        let tu_id = doc.add_object(Stream::new(dictionary! {}, to_unicode(&self.used).into_bytes()));
        let type0 = dictionary! {
            "Type" => "Font",
            "Subtype" => "Type0",
            "BaseFont" => Object::Name(base.into_bytes()),
            "Encoding" => "Identity-H",
            "DescendantFonts" => vec![Object::Reference(cid_id)],
            "ToUnicode" => tu_id,
        };
        doc.objects.insert(self.id, Object::Dictionary(type0));
        Ok(())
    }
}

fn to_unicode(used: &BTreeMap<u16, (u16, char)>) -> String {
    let mut s = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let entries: Vec<_> = used.iter().filter(|(k, _)| **k != 0).collect();
    for chunk in entries.chunks(100) {
        s.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (k, (_, c)) in chunk {
            let mut buf = [0u16; 2];
            let hex: String = c.encode_utf16(&mut buf).iter().map(|u| format!("{u:04X}")).collect();
            s.push_str(&format!("<{:04X}> <{}>\n", k, hex));
        }
        s.push_str("endbfchar\n");
    }
    s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_korean_font_and_measures() {
        let Ok(f) = korean_font() else {
            eprintln!("한글 글꼴 없음 — 건너뜀");
            return;
        };
        let mut doc = Document::with_version("1.7");
        let mut pf = PdfFont::with_font(&mut doc, f);
        assert!(pf.width("가나다", 10.0) > 20.0);
        let hex = pf.encode("가A");
        assert_eq!(hex.len(), 10);
        assert!(pf.data().missing("가나다abc").is_empty());
        let lines = pf.wrap("가나다라마바사 아자차카타파하", 10.0, 45.0);
        assert!(lines.len() >= 2, "{lines:?}");
        pf.finish(&mut doc).unwrap();
    }
}
