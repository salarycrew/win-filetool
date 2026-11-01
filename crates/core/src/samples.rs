//! 데스크톱 점검·데모용 샘플 파일 생성기 (NFD 폴더, 맥식 zip, N쪽 PDF).
//! `filetool-cli selftest`가 실제 윈도 디스크 위에서 모든 작업을 돌려 볼 때 쓴다.

use std::path::Path;

use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream};
use unicode_normalization::UnicodeNormalization;

use crate::error::{io_err, Error, Result};

pub fn nfd(s: &str) -> String {
    s.nfd().collect()
}

/// `label p1` … `label pN`이 찍힌 A4 PDF(2단 페이지 트리, 상속 MediaBox).
pub fn make_pdf(path: &Path, n: u32, label: &str) -> Result<()> {
    let mut doc = Document::with_version("1.5");
    let root_pages = doc.new_object_id();
    let mid_pages = doc.new_object_id();
    let font = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let resources = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
    let mut kids = Vec::new();
    for i in 1..=n {
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 36.into()]),
                Operation::new("Td", vec![100.into(), 600.into()]),
                Operation::new("Tj", vec![Object::string_literal(format!("{label} p{i}"))]),
                Operation::new("ET", vec![]),
            ],
        };
        let encoded = content
            .encode()
            .map_err(|e| Error::Pdf { path: path.to_path_buf(), message: e.to_string() })?;
        let cid = doc.add_object(Stream::new(dictionary! {}, encoded));
        let pid = doc.add_object(dictionary! { "Type" => "Page", "Parent" => mid_pages, "Contents" => cid });
        kids.push(Object::Reference(pid));
    }
    doc.objects.insert(
        mid_pages,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Parent" => root_pages, "Kids" => kids, "Count" => n as i64,
            "Resources" => resources,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        }),
    );
    doc.objects.insert(
        root_pages,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![Object::Reference(mid_pages)], "Count" => n as i64,
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => root_pages });
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(io_err(path))?;
    crate::paths::write_new(path, &bytes)
}

/// 각 쪽 첫 텍스트 문자열(`label pN`)을 순서대로 읽는다(점검용).
pub fn pdf_labels(path: &Path) -> Result<Vec<String>> {
    let doc = crate::pdf::load(path)?;
    Ok(doc
        .get_pages()
        .values()
        .map(|&pid| {
            let s = String::from_utf8_lossy(&doc.get_page_content(pid)).to_string();
            s.find('(')
                .and_then(|a| s[a + 1..].find(')').map(|b| s[a + 1..a + 1 + b].to_string()))
                .unwrap_or_default()
        })
        .collect())
}

/// 각 쪽의 `/Rotate` 값(점검용).
pub fn pdf_rotations(path: &Path) -> Result<Vec<i64>> {
    let doc = crate::pdf::load(path)?;
    Ok(doc
        .get_pages()
        .values()
        .map(|&pid| {
            doc.get_dictionary(pid)
                .ok()
                .and_then(|d| d.get(b"Rotate").ok())
                .and_then(|o| o.as_i64().ok())
                .unwrap_or(0)
        })
        .collect())
}

/// 맥 Archive Utility처럼 UTF-8 플래그 없이 이름 바이트를 그대로 넣은 무압축 zip.
pub fn write_mac_style_zip(path: &Path, entries: &[(&str, &[u8])]) -> Result<()> {
    let raw: Vec<(&[u8], &[u8])> = entries.iter().map(|(n, d)| (n.as_bytes(), *d)).collect();
    write_raw_name_zip(path, &raw, 3)
}

/// 0.3.0 W4: 윈도 탐색기 "압축(ZIP) 폴더"처럼 이름을 CP949 바이트 그대로(UTF-8 표시 없이) 넣은 zip.
pub fn write_windows_cp949_zip(path: &Path, entries: &[(&str, &[u8])]) -> Result<()> {
    let enc: Vec<(Vec<u8>, &[u8])> = entries.iter().map(|(n, d)| (encoding_rs::EUC_KR.encode(n).0.into_owned(), *d)).collect();
    let raw: Vec<(&[u8], &[u8])> = enc.iter().map(|(n, d)| (n.as_slice(), *d)).collect();
    write_raw_name_zip(path, &raw, 0)
}

/// 이름 바이트를 그대로 넣은 무압축 zip(UTF-8 표시 없음). `host`: 0 = MS-DOS(윈도), 3 = Unix(맥)
pub fn write_raw_name_zip(path: &Path, entries: &[(&[u8], &[u8])], host: u16) -> Result<()> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let u16le = |v: u16| v.to_le_bytes();
    let u32le = |v: u32| v.to_le_bytes();
    for (name, data) in entries {
        let nb: &[u8] = name;
        let crc = crc32(data);
        let offset = out.len() as u32;
        out.extend_from_slice(&u32le(0x04034b50));
        for v in [20u16, 0, 0, 0, 0x21] {
            out.extend_from_slice(&u16le(v));
        }
        for v in [crc, data.len() as u32, data.len() as u32] {
            out.extend_from_slice(&u32le(v));
        }
        out.extend_from_slice(&u16le(nb.len() as u16));
        out.extend_from_slice(&u16le(0));
        out.extend_from_slice(nb);
        out.extend_from_slice(data);

        central.extend_from_slice(&u32le(0x02014b50));
        for v in [(host << 8) | 20, 20, 0, 0, 0, 0x21] {
            central.extend_from_slice(&u16le(v));
        }
        for v in [crc, data.len() as u32, data.len() as u32] {
            central.extend_from_slice(&u32le(v));
        }
        for v in [nb.len() as u16, 0, 0, 0, 0] {
            central.extend_from_slice(&u16le(v));
        }
        let is_dir = name.ends_with(b"/");
        let attr: u32 = match (host, is_dir) {
            (3, true) => (0o040755 << 16) | 0x10,
            (3, false) => 0o100644 << 16,
            (_, true) => 0x10,
            (_, false) => 0x20,
        };
        central.extend_from_slice(&u32le(attr));
        central.extend_from_slice(&u32le(offset));
        central.extend_from_slice(nb);
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&u32le(0x06054b50));
    for v in [0u16, 0, entries.len() as u16, entries.len() as u16] {
        out.extend_from_slice(&u16le(v));
    }
    out.extend_from_slice(&u32le(central.len() as u32));
    out.extend_from_slice(&u32le(cd_offset));
    out.extend_from_slice(&u16le(0));
    crate::paths::write_new(path, &out)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    #[test]
    fn crc32_known_value() {
        assert_eq!(super::crc32(b"123456789"), 0xCBF4_3926);
    }
}

// ---------- 1b: 변환 점검용 샘플 ----------

/// 사분면 색이 다른 그림(왼쪽 위 빨강, 오른쪽 위 초록, 왼쪽 아래 파랑, 오른쪽 아래 흰색).
/// 회전·타일 순서를 색으로 확인할 수 있다.
pub fn quadrant_image(w: u32, h: u32) -> image::RgbImage {
    image::RgbImage::from_fn(w, h, |x, y| {
        let c = match (x >= w / 2, y >= h / 2) {
            (false, false) => [255, 0, 0],
            (true, false) => [0, 255, 0],
            (false, true) => [0, 0, 255],
            (true, true) => [255, 255, 255],
        };
        image::Rgb(c)
    })
}

/// 색 이름(가장 가까운 기본색): "빨강"/"초록"/"파랑"/"흰색"/"기타"
pub fn color_name(p: [u8; 3]) -> &'static str {
    let [r, g, b] = p.map(|v| v as i32);
    let near = |a: i32, t: i32| (a - t).abs() < 70;
    match () {
        _ if near(r, 255) && near(g, 0) && near(b, 0) => "빨강",
        _ if near(r, 0) && near(g, 255) && near(b, 0) => "초록",
        _ if near(r, 0) && near(g, 0) && near(b, 255) => "파랑",
        _ if near(r, 255) && near(g, 255) && near(b, 255) => "흰색",
        _ => "기타",
    }
}

/// 이미지의 네 사분면 가운데 색: [왼위, 오위, 왼아래, 오아래]
pub fn quadrant_colors(img: &image::DynamicImage) -> [&'static str; 4] {
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    let at = |x: u32, y: u32| color_name(rgb.get_pixel(x, y).0);
    [at(w / 4, h / 4), at(3 * w / 4, h / 4), at(w / 4, 3 * h / 4), at(3 * w / 4, 3 * h / 4)]
}

/// EXIF(촬영 날짜, 회전 값)가 들어간 JPEG. 픽셀은 `quadrant_image(w,h)`.
pub fn make_jpeg_with_exif(path: &Path, w: u32, h: u32, date: &str, orientation: u16) -> Result<()> {
    use image::ImageEncoder;
    let img = quadrant_image(w, h);
    let mut buf = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 95);
    let _ = enc.set_exif_metadata(crate::exif::build_sample(date, orientation));
    enc.write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
        .map_err(|e| Error::Invalid(e.to_string()))?;
    std::fs::write(path, buf).map_err(io_err(path))
}

fn run_ffmpeg(ffmpeg: &Path, args: &[&std::ffi::OsStr]) -> Result<()> {
    let args: Vec<std::ffi::OsString> = args.iter().map(|a| a.to_os_string()).collect();
    let out = crate::process::run(ffmpeg, &args, std::time::Duration::from_secs(120), None, &mut |_| {})?;
    if !out.success() {
        return Err(Error::Tool { tool: "ffmpeg".into(), message: crate::process::last_lines(&out.stderr_tail, 3) });
    }
    Ok(())
}

/// 아이폰식 grid HEIC: 2×2 타일(각 `tile`px, 색: 빨강/초록/파랑/흰색), 출력 크기는 타일 합보다
/// 작게 잘라(`out_w`×`out_h`) grid 자르기도 확인하고, `irot`=1(반시계 90°)과 EXIF(날짜, 회전 6)를 넣는다.
/// 타일은 ffmpeg(libx265)로 인코딩한다.
pub fn make_grid_heic(ffmpeg: &Path, path: &Path, tile: u32, out_w: u16, out_h: u16, date: &str) -> Result<()> {
    make_grid_heic_exif(ffmpeg, path, tile, out_w, out_h, crate::exif::build_sample(date, 6))
}

/// 0.3.0 W4: [`make_grid_heic`]과 같되 EXIF 블록을 직접 준다(예: GPS가 든 [`crate::exif::build_sample_gps`]).
pub fn make_grid_heic_exif(ffmpeg: &Path, path: &Path, tile: u32, out_w: u16, out_h: u16, exif: Vec<u8>) -> Result<()> {
    let tmp = crate::tempdir::JobDir::new("heicgen")?;
    let mut tiles = Vec::new();
    for (i, color) in ["red", "lime", "blue", "white"].iter().enumerate() {
        let f = tmp.path().join(format!("t{i}.hevc"));
        let src = format!("color=c={color}:s={tile}x{tile}");
        run_ffmpeg(
            ffmpeg,
            &[
                "-hide_banner".as_ref(), "-nostdin".as_ref(), "-loglevel".as_ref(), "error".as_ref(), "-y".as_ref(),
                "-f".as_ref(), "lavfi".as_ref(), "-i".as_ref(), src.as_ref(), "-frames:v".as_ref(), "1".as_ref(),
                "-pix_fmt".as_ref(), "yuv420p".as_ref(), "-c:v".as_ref(), "libx265".as_ref(),
                "-x265-params".as_ref(), "log-level=error".as_ref(), "-f".as_ref(), "hevc".as_ref(), f.as_os_str(),
            ],
        )?;
        tiles.push(std::fs::read(&f).map_err(io_err(&f))?);
    }
    let bytes = crate::heif::build_grid_heic(&tiles, tile, tile, 2, 2, out_w, out_h, 1, Some(&exif))
        .ok_or_else(|| Error::Invalid("HEIC 샘플을 만들지 못했습니다(HEVC 타일 해석 실패)".into()))?;
    std::fs::write(path, bytes).map_err(io_err(path))
}

/// 소리가 있는 짧은 시험 영상(testsrc2 + 440Hz). 확장자에 맞는 형식으로 저장(.mov 등).
pub fn make_test_video(ffmpeg: &Path, path: &Path, secs: u32, w: u32, h: u32) -> Result<()> {
    let v = format!("testsrc2=s={w}x{h}:r=30:d={secs}");
    let a = format!("sine=f=440:d={secs}");
    let t = secs.to_string();
    // WebM은 VP8/Opus만 담을 수 있다. 나머지는 빠른 MPEG-4 Part 2 + PCM.
    let webm = path.extension().map(|e| e.eq_ignore_ascii_case("webm")).unwrap_or(false);
    let (vc, vq, vqv, ac): (&str, &str, &str, &str) = if webm { ("libvpx", "-b:v", "800k", "libopus") } else { ("mpeg4", "-q:v", "3", "pcm_s16le") };
    run_ffmpeg(
        ffmpeg,
        &[
            "-hide_banner".as_ref(), "-nostdin".as_ref(), "-loglevel".as_ref(), "error".as_ref(), "-y".as_ref(),
            "-f".as_ref(), "lavfi".as_ref(), "-i".as_ref(), v.as_ref(), "-f".as_ref(), "lavfi".as_ref(), "-i".as_ref(), a.as_ref(),
            "-t".as_ref(), t.as_ref(), "-c:v".as_ref(), vc.as_ref(), vq.as_ref(), vqv.as_ref(),
            "-c:a".as_ref(), ac.as_ref(), path.as_os_str(),
        ],
    )
}

// ---------- 1c: PDF 압축·이미지↔PDF 점검용 ----------

/// 사진처럼 압축이 잘 안 되는 그림(부드러운 그라데이션 + 잔무늬 잡음). 같은 인자면 항상 같은 픽셀.
pub fn photo_like(w: u32, h: u32, seed: u32) -> image::RgbImage {
    let mut s = seed.wrapping_mul(2654435761).wrapping_add(1);
    image::RgbImage::from_fn(w, h, |x, y| {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        let n = (s % 48) as i32 - 24;
        let fx = x as f32 / w as f32;
        let fy = y as f32 / h as f32;
        let r = (200.0 * fx + 30.0 * (fy * 9.0).sin()) as i32 + n;
        let g = (180.0 * fy + 40.0 * (fx * 7.0).cos()) as i32 + n;
        let b = (120.0 + 100.0 * (fx * fy * 5.0).sin()) as i32 - n;
        image::Rgb([r.clamp(0, 255) as u8, g.clamp(0, 255) as u8, b.clamp(0, 255) as u8])
    })
}

pub fn jpeg_bytes(img: &image::RgbImage, quality: u8) -> Result<Vec<u8>> {
    use image::ImageEncoder;
    let mut buf = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality)
        .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| Error::Invalid(e.to_string()))?;
    Ok(buf)
}

/// 사진이 많은 A4 PDF: 쪽마다 글자(`label pN`) + 벡터 사각형 + 큰 JPEG 사진(`photo_px` 가로, 품질 95)을
/// 가로 6인치(432pt)로 찍는다. 1쪽에는 무손실(Flate) RGB 그림 하나를 2인치로 더 넣는다.
/// 같은 사진 객체를 두 쪽 이상에서 쓰지 않는다(쪽마다 다른 사진).
pub fn make_photo_pdf(path: &Path, pages: u32, label: &str, photo_px: u32) -> Result<()> {
    let mut doc = Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let ph = photo_px * 3 / 4;
    let mut kids = Vec::new();
    for i in 1..=pages {
        let jpg = jpeg_bytes(&photo_like(photo_px, ph, i), 95)?;
        let im = doc.add_object(
            Stream::new(
                dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => photo_px as i64, "Height" => ph as i64,
                    "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8, "Filter" => "DCTDecode" },
                jpg,
            )
            .with_compression(false),
        );
        let mut xobj = dictionary! { "Ph" => im };
        let mut ops = vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 28.into()]),
            Operation::new("Td", vec![72.into(), 760.into()]),
            Operation::new("Tj", vec![Object::string_literal(format!("{label} p{i}"))]),
            Operation::new("ET", vec![]),
            // 벡터: 테두리 사각형
            Operation::new("RG", vec![0.into(), 0.into(), 1.into()]),
            Operation::new("w", vec![3.into()]),
            Operation::new("re", vec![60.into(), 300.into(), 456.into(), 348.into()]),
            Operation::new("S", vec![]),
            Operation::new("q", vec![]),
            Operation::new("cm", vec![432.into(), 0.into(), 0.into(), 324.into(), 72.into(), 312.into()]),
            Operation::new("Do", vec![Object::Name(b"Ph".to_vec())]),
            Operation::new("Q", vec![]),
        ];
        if i == 1 {
            let raw = photo_like(600, 400, 99).into_raw();
            let mut s = Stream::new(
                dictionary! { "Type" => "XObject", "Subtype" => "Image", "Width" => 600, "Height" => 400,
                    "ColorSpace" => "DeviceRGB", "BitsPerComponent" => 8 },
                raw,
            );
            let _ = s.compress();
            let fl = doc.add_object(s);
            xobj.set("Fl", fl);
            ops.extend([
                Operation::new("q", vec![]),
                Operation::new("cm", vec![144.into(), 0.into(), 0.into(), 96.into(), 72.into(), 120.into()]),
                Operation::new("Do", vec![Object::Name(b"Fl".to_vec())]),
                Operation::new("Q", vec![]),
            ]);
        }
        let content = Content { operations: ops }.encode().map_err(|e| Error::Pdf { path: path.into(), message: e.to_string() })?;
        let cid = doc.add_object(Stream::new(dictionary! {}, content));
        let pid = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => cid,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font }, "XObject" => xobj },
        });
        kids.push(Object::Reference(pid));
    }
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => pages as i64 }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(io_err(path))?;
    crate::paths::write_new(path, &bytes)
}

/// 암호가 걸린 PDF(RC4 128, 열기 암호 `user_pw`, 빈 값이면 열기 암호 없이 권한 암호만).
pub fn make_encrypted_pdf(path: &Path, user_pw: &str) -> Result<()> {
    let tmp = crate::tempdir::JobDir::new("encgen")?;
    let plain = tmp.path().join("plain.pdf");
    make_pdf(&plain, 2, "SECRET")?;
    let mut doc = Document::load(&plain).map_err(|e| Error::Pdf { path: plain.clone(), message: e.to_string() })?;
    // 암호 키 계산에 문서 ID가 필요하다
    let id = Object::String(b"win-filetool-enc".to_vec(), lopdf::StringFormat::Hexadecimal);
    doc.trailer.set("ID", Object::Array(vec![id.clone(), id]));
    let state = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V2 {
        document: &doc,
        owner_password: "owner-pw",
        user_password: user_pw,
        key_length: 128,
        permissions: lopdf::Permissions::all(),
    })
    .map_err(|e| Error::Invalid(e.to_string()))?;
    doc.encrypt(&state).map_err(|e| Error::Invalid(e.to_string()))?;
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(io_err(path))?;
    crate::paths::write_new(path, &bytes)
}

/// PDF 안 이미지 객체들의 (가로, 세로, 필터, 바이트 수) — 점검용
pub fn pdf_images(path: &Path) -> Result<Vec<(u32, u32, String, usize)>> {
    let doc = crate::pdf::load(path)?;
    let mut v: Vec<(u32, u32, String, usize)> = doc
        .objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .filter(|s| s.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image".as_slice()))
        .map(|s| {
            let g = |k: &[u8]| s.dict.get(k).and_then(Object::as_i64).unwrap_or(0) as u32;
            let f = s.dict.get(b"Filter").and_then(Object::as_name).map(|n| String::from_utf8_lossy(n).into_owned()).unwrap_or_default();
            (g(b"Width"), g(b"Height"), f, s.content.len())
        })
        .collect();
    v.sort();
    Ok(v)
}

/// 각 쪽 콘텐츠 스트림(풀어 둔 바이트) — 압축 전후 글자·벡터가 그대로인지 비교용
pub fn pdf_page_contents(path: &Path) -> Result<Vec<Vec<u8>>> {
    let doc = crate::pdf::load(path)?;
    Ok(doc.get_pages().values().map(|&pid| doc.get_page_content(pid)).collect())
}

/// 각 쪽 MediaBox (가로, 세로) pt
pub fn pdf_page_sizes(path: &Path) -> Result<Vec<(f64, f64)>> {
    let doc = crate::pdf::load(path)?;
    Ok(doc
        .get_pages()
        .values()
        .map(|&pid| {
            let mb = doc.get_dictionary(pid).ok().and_then(|d| d.get(b"MediaBox").ok()).and_then(|o| o.as_array().ok()).cloned().unwrap_or_default();
            let n = |i: usize| mb.get(i).and_then(|o| o.as_float().ok()).unwrap_or(0.0) as f64;
            (n(2) - n(0), n(3) - n(1))
        })
        .collect())
}
