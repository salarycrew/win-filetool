//! 아주 작은 EXIF(TIFF 구조) 도우미: 촬영 날짜 읽기, 회전 값 읽기, 점검용 EXIF 만들기.
//! EXIF 블록은 TIFF 헤더("II*\0" / "MM\0*")부터 시작하는 바이트로 다룬다
//! (JPEG APP1의 "Exif\0\0" 뒤, HEIF Exif 항목의 오프셋 뒤).

const TAG_ORIENTATION: u16 = 0x0112;
const TAG_DATETIME: u16 = 0x0132;
const TAG_EXIF_IFD: u16 = 0x8769;
const TAG_DATETIME_ORIGINAL: u16 = 0x9003;

struct Tiff<'a> {
    b: &'a [u8],
    le: bool,
}

impl<'a> Tiff<'a> {
    fn new(b: &'a [u8]) -> Option<Self> {
        let le = match b.get(0..4)? {
            [b'I', b'I', 42, 0] => true,
            [b'M', b'M', 0, 42] => false,
            _ => return None,
        };
        Some(Self { b, le })
    }
    fn u16(&self, o: usize) -> Option<u16> {
        let s: [u8; 2] = self.b.get(o..o + 2)?.try_into().ok()?;
        Some(if self.le { u16::from_le_bytes(s) } else { u16::from_be_bytes(s) })
    }
    fn u32(&self, o: usize) -> Option<u32> {
        let s: [u8; 4] = self.b.get(o..o + 4)?.try_into().ok()?;
        Some(if self.le { u32::from_le_bytes(s) } else { u32::from_be_bytes(s) })
    }
    /// IFD 항목: (tag, type, count, value/offset 위치)
    fn entries(&self, ifd: usize) -> Vec<(u16, u16, u32, usize)> {
        let n = self.u16(ifd).unwrap_or(0) as usize;
        (0..n.min(512))
            .filter_map(|i| {
                let e = ifd + 2 + i * 12;
                Some((self.u16(e)?, self.u16(e + 2)?, self.u32(e + 4)?, e + 8))
            })
            .collect()
    }
    fn ascii(&self, count: u32, at: usize) -> Option<String> {
        let count = count as usize;
        let start = if count <= 4 { at } else { self.u32(at)? as usize };
        let raw = self.b.get(start..start + count)?;
        let s = raw.split(|c| *c == 0).next()?;
        Some(String::from_utf8_lossy(s).trim().to_string())
    }
}

/// 촬영 날짜(DateTimeOriginal, 없으면 DateTime). 예: "2024:05:01 10:20:30"
pub fn date_taken(tiff: &[u8]) -> Option<String> {
    let t = Tiff::new(tiff)?;
    let ifd0 = t.u32(4)? as usize;
    let mut fallback = None;
    for (tag, _ty, count, at) in t.entries(ifd0) {
        match tag {
            TAG_DATETIME => fallback = t.ascii(count, at),
            TAG_EXIF_IFD => {
                let sub = t.u32(at)? as usize;
                for (tag2, _, c2, at2) in t.entries(sub) {
                    if tag2 == TAG_DATETIME_ORIGINAL {
                        if let Some(d) = t.ascii(c2, at2) {
                            return Some(d);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    fallback
}

/// EXIF 회전 값(1~8). 없으면 None.
pub fn orientation(tiff: &[u8]) -> Option<u16> {
    let t = Tiff::new(tiff)?;
    let ifd0 = t.u32(4)? as usize;
    t.entries(ifd0).into_iter().find(|e| e.0 == TAG_ORIENTATION).and_then(|e| t.u16(e.3))
}

/// 점검용 EXIF(빅엔디언): Orientation, DateTime, ExifIFD→DateTimeOriginal.
pub fn build_sample(date: &str, orientation: u16) -> Vec<u8> {
    let mut date_z = date.as_bytes().to_vec();
    date_z.push(0);
    let dlen = date_z.len() as u32;
    // 배치: 헤더(8) | IFD0(2+3*12+4=42) | ExifIFD(2+1*12+4=18) | date(DateTime) | date(Original)
    let ifd0 = 8u32;
    let exif_ifd = ifd0 + 42;
    let d1 = exif_ifd + 18;
    let d2 = d1 + dlen;
    let mut b = Vec::new();
    b.extend_from_slice(b"MM\0*");
    b.extend_from_slice(&ifd0.to_be_bytes());
    let entry = |b: &mut Vec<u8>, tag: u16, ty: u16, count: u32, val: [u8; 4]| {
        b.extend_from_slice(&tag.to_be_bytes());
        b.extend_from_slice(&ty.to_be_bytes());
        b.extend_from_slice(&count.to_be_bytes());
        b.extend_from_slice(&val);
    };
    b.extend_from_slice(&3u16.to_be_bytes());
    let o = orientation.to_be_bytes();
    entry(&mut b, TAG_ORIENTATION, 3, 1, [o[0], o[1], 0, 0]);
    entry(&mut b, TAG_DATETIME, 2, dlen, d1.to_be_bytes());
    entry(&mut b, TAG_EXIF_IFD, 4, 1, exif_ifd.to_be_bytes());
    b.extend_from_slice(&0u32.to_be_bytes());
    b.extend_from_slice(&1u16.to_be_bytes());
    entry(&mut b, TAG_DATETIME_ORIGINAL, 2, dlen, d2.to_be_bytes());
    b.extend_from_slice(&0u32.to_be_bytes());
    b.extend_from_slice(&date_z);
    b.extend_from_slice(&date_z);
    b
}

const TAG_GPS_IFD: u16 = 0x8825;

/// 0.3.0 W4: 위치(GPS) 정보가 들어 있는지(IFD0의 GPS IFD에 항목이 하나라도 있으면).
pub fn has_gps(tiff: &[u8]) -> bool {
    let Some(t) = Tiff::new(tiff) else { return false };
    let Some(ifd0) = t.u32(4) else { return false };
    t.entries(ifd0 as usize)
        .into_iter()
        .find(|e| e.0 == TAG_GPS_IFD)
        .and_then(|e| t.u32(e.3))
        .map(|gps| t.u16(gps as usize).unwrap_or(0) > 0)
        .unwrap_or(false)
}

/// 점검용 EXIF + GPS(빅엔디언): Orientation, DateTime, ExifIFD→DateTimeOriginal, GPSIFD→위도·경도(도 단위 정수).
pub fn build_sample_gps(date: &str, orientation: u16, lat_deg: u32, lon_deg: u32) -> Vec<u8> {
    let mut date_z = date.as_bytes().to_vec();
    date_z.push(0);
    let dlen = date_z.len() as u32;
    // 헤더(8) | IFD0(2+4*12+4=54) | ExifIFD(18) | GPSIFD(2+4*12+4=54) | 위도(24) | 경도(24) | date | date
    let ifd0 = 8u32;
    let exif_ifd = ifd0 + 54;
    let gps_ifd = exif_ifd + 18;
    let lat = gps_ifd + 54;
    let lon = lat + 24;
    let d1 = lon + 24;
    let d2 = d1 + dlen;
    let mut b = Vec::new();
    b.extend_from_slice(b"MM\0*");
    b.extend_from_slice(&ifd0.to_be_bytes());
    let entry = |b: &mut Vec<u8>, tag: u16, ty: u16, count: u32, val: [u8; 4]| {
        b.extend_from_slice(&tag.to_be_bytes());
        b.extend_from_slice(&ty.to_be_bytes());
        b.extend_from_slice(&count.to_be_bytes());
        b.extend_from_slice(&val);
    };
    b.extend_from_slice(&4u16.to_be_bytes());
    let o = orientation.to_be_bytes();
    entry(&mut b, TAG_ORIENTATION, 3, 1, [o[0], o[1], 0, 0]);
    entry(&mut b, TAG_DATETIME, 2, dlen, d1.to_be_bytes());
    entry(&mut b, TAG_EXIF_IFD, 4, 1, exif_ifd.to_be_bytes());
    entry(&mut b, TAG_GPS_IFD, 4, 1, gps_ifd.to_be_bytes());
    b.extend_from_slice(&0u32.to_be_bytes());
    b.extend_from_slice(&1u16.to_be_bytes());
    entry(&mut b, TAG_DATETIME_ORIGINAL, 2, dlen, d2.to_be_bytes());
    b.extend_from_slice(&0u32.to_be_bytes());
    b.extend_from_slice(&4u16.to_be_bytes());
    entry(&mut b, 1, 2, 2, [b'N', 0, 0, 0]);
    entry(&mut b, 2, 5, 3, lat.to_be_bytes());
    entry(&mut b, 3, 2, 2, [b'E', 0, 0, 0]);
    entry(&mut b, 4, 5, 3, lon.to_be_bytes());
    b.extend_from_slice(&0u32.to_be_bytes());
    for deg in [lat_deg, lon_deg] {
        for (n, d) in [(deg, 1u32), (0, 1), (0, 1)] {
            b.extend_from_slice(&n.to_be_bytes());
            b.extend_from_slice(&d.to_be_bytes());
        }
    }
    b.extend_from_slice(&date_z);
    b.extend_from_slice(&date_z);
    b
}

/// JPEG 파일에서 EXIF(TIFF) 블록을 꺼낸다.
pub fn from_jpeg(jpeg: &[u8]) -> Option<Vec<u8>> {
    if jpeg.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    while i + 4 <= jpeg.len() {
        if jpeg[i] != 0xFF {
            return None;
        }
        let marker = jpeg[i + 1];
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        let body = jpeg.get(i + 4..i + 2 + len)?;
        if marker == 0xE1 && body.starts_with(b"Exif\0\0") {
            return Some(body[6..].to_vec());
        }
        i += 2 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_roundtrip() {
        let e = build_sample("2024:05:01 10:20:30", 6);
        assert_eq!(date_taken(&e).as_deref(), Some("2024:05:01 10:20:30"));
        assert_eq!(orientation(&e), Some(6));
        let mut m = e.clone();
        let _ = image::metadata::Orientation::remove_from_exif_chunk(&mut m);
        assert_eq!(orientation(&m), Some(1));
        assert_eq!(date_taken(&m).as_deref(), Some("2024:05:01 10:20:30"));
        assert!(!has_gps(&e));
    }

    #[test]
    fn gps_sample() {
        let e = build_sample_gps("2024:05:01 10:20:30", 6, 37, 127);
        assert!(has_gps(&e));
        assert_eq!(orientation(&e), Some(6));
        assert_eq!(date_taken(&e).as_deref(), Some("2024:05:01 10:20:30"));
    }
}
