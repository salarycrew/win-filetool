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
    let mut out = Vec::new();
    let mut central = Vec::new();
    let u16le = |v: u16| v.to_le_bytes();
    let u32le = |v: u32| v.to_le_bytes();
    for (name, data) in entries {
        let nb = name.as_bytes();
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
        for v in [(3u16 << 8) | 20, 20, 0, 0, 0, 0x21] {
            central.extend_from_slice(&u16le(v));
        }
        for v in [crc, data.len() as u32, data.len() as u32] {
            central.extend_from_slice(&u32le(v));
        }
        for v in [nb.len() as u16, 0, 0, 0, 0] {
            central.extend_from_slice(&u16le(v));
        }
        let attr: u32 = if name.ends_with('/') { (0o040755 << 16) | 0x10 } else { 0o100644 << 16 };
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
    let exif = crate::exif::build_sample(date, 6);
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
