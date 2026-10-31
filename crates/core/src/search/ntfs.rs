//! 2b: NTFS 디스크 구조 해석(플랫폼 무관). Windows 볼륨 핸들(`\\.\C:`)이든 Linux의 NTFS 이미지 파일이든
//! [`VolRead`]로 읽는다.
//!
//! 길: 부트 섹터 → `$MFT`(레코드 0)의 `$DATA` 데이터 런 → `$MFT`를 런마다 크게(4 MB) 차례로 읽어 FILE 레코드를 푼다.
//! `FSCTL_ENUM_USN_DATA`는 이름·부모만 주고 크기·시각이 없어서 쓰지 않는다.
//!
//! - 레코드마다 fixup(업데이트 시퀀스 배열, 512바이트마다 끝 2바이트)을 되돌린다. 맞지 않으면 그 레코드는 버린다.
//! - `$STANDARD_INFORMATION`: 수정 시각·파일 속성, `$FILE_NAME`: 이름·부모(네임스페이스: POSIX 0, Win32 1, DOS 2, Win32&DOS 3),
//!   이름 없는 `$DATA`: 크기(상주면 값 길이, 비상주면 첫 조각(lowest VCN 0)의 data size), `$REPARSE_POINT`: 태그.
//! - 속성이 레코드 하나에 다 안 들어가면 `$ATTRIBUTE_LIST`가 생기고 나머지는 확장 레코드(base 참조가 있는 레코드)에 있다.
//!   전체 읽기는 확장 레코드의 `$FILE_NAME`·`$DATA`를 base 레코드에 합친다(목록을 따로 따라갈 필요 없음).
//!   레코드 하나만 읽을 때(저널 따라잡기)는 [`parse_attr_list`]로 확장 레코드를 찾아 읽는다.
//! - 쓰지 않는 레코드(in-use 꺼짐)는 건너뛴다.

use std::io;

pub const ATTR_STANDARD_INFORMATION: u32 = 0x10;
pub const ATTR_ATTRIBUTE_LIST: u32 = 0x20;
pub const ATTR_FILE_NAME: u32 = 0x30;
pub const ATTR_DATA: u32 = 0x80;
pub const ATTR_REPARSE_POINT: u32 = 0xC0;
pub const ATTR_END: u32 = 0xFFFF_FFFF;

pub const NS_POSIX: u8 = 0;
pub const NS_WIN32: u8 = 1;
pub const NS_DOS: u8 = 2;
pub const NS_WIN32_DOS: u8 = 3;

/// 파일 참조 번호(FRN)의 레코드 번호 부분(아래 48비트). 위 16비트는 순번(sequence)
pub const REC_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
/// 루트 폴더 레코드 번호
pub const ROOT_REC: u64 = 5;
/// 이 번호 아래는 NTFS 메타데이터 파일($MFT, $LogFile, $Extend …). 탐색기·폴더 훑기에도 안 보인다
pub const FIRST_USER_REC: u64 = 16;

pub fn frn(rec: u64, seq: u16) -> u64 {
    (rec & REC_MASK) | ((seq as u64) << 48)
}
pub fn frn_rec(f: u64) -> u64 {
    f & REC_MASK
}
pub fn frn_seq(f: u64) -> u16 {
    (f >> 48) as u16
}

/// FILETIME(1601년부터 100 ns) → 유닉스 초
pub fn filetime_to_unix(t: u64) -> i64 {
    if t == 0 {
        return 0;
    }
    (t as i64 - 116_444_736_000_000_000) / 10_000_000
}

/// 이름 대리 reparse(정션·심볼릭 링크·마운트 지점) — 따라가지 않는 연결 지점
pub fn is_name_surrogate(tag: u32) -> bool {
    tag & 0x2000_0000 != 0
}

/// 볼륨(또는 이미지)을 아무 위치에서 읽기. Windows 볼륨 핸들은 섹터 단위로 맞춰 읽어야 한다(호출하는 쪽이 맞춤).
pub trait VolRead: Send + Sync {
    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<()>;
}

#[cfg(unix)]
impl VolRead for std::fs::File {
    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<()> {
        use std::os::unix::fs::FileExt;
        self.read_exact_at(buf, off)
    }
}

#[cfg(windows)]
impl VolRead for std::fs::File {
    fn read_at(&self, off: u64, buf: &mut [u8]) -> io::Result<()> {
        use std::os::windows::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            let n = self.seek_read(&mut buf[done..], off + done as u64)?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "볼륨 끝"));
            }
            done += n;
        }
        Ok(())
    }
}

fn u16le(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]))
}
fn u32le(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}
fn u64le(b: &[u8], o: usize) -> Option<u64> {
    b.get(o..o + 8).map(|s| u64::from_le_bytes(s.try_into().unwrap()))
}

fn utf16_name(b: &[u8], o: usize, chars: usize) -> Option<String> {
    let s = b.get(o..o + chars * 2)?;
    let w: Vec<u16> = s.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Some(String::from_utf16_lossy(&w))
}

// ---------- 부트 섹터 ----------

#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    pub bytes_per_sector: u32,
    pub cluster: u32,
    pub record_size: u32,
    pub mft_lcn: u64,
    pub total_sectors: u64,
    /// 볼륨 일련번호(64비트, 부트 섹터)
    pub serial: u64,
}

pub fn parse_boot(b: &[u8]) -> Option<Geometry> {
    if b.get(3..11)? != b"NTFS    " {
        return None;
    }
    let bps = u16le(b, 0x0B)? as u32;
    let spc_raw = *b.get(0x0D)?;
    let spc = if spc_raw > 0x80 { 1u32 << (256 - spc_raw as u32) } else { spc_raw as u32 };
    if bps == 0 || spc == 0 || !bps.is_power_of_two() {
        return None;
    }
    let cluster = bps * spc;
    let cpr = *b.get(0x40)? as i8;
    let record_size = if cpr < 0 { 1u32 << (-(cpr as i32)) as u32 } else { cpr as u32 * cluster };
    if !(256..=65536).contains(&record_size) {
        return None;
    }
    Some(Geometry {
        bytes_per_sector: bps,
        cluster,
        record_size,
        mft_lcn: u64le(b, 0x30)?,
        total_sectors: u64le(b, 0x28)?,
        serial: u64le(b, 0x48)?,
    })
}

// ---------- fixup ----------

/// 업데이트 시퀀스 배열을 되돌린다. 512바이트(배열 길이로 정한 간격)마다 끝 2바이트가 시퀀스 번호와 같아야 한다.
/// 이미 되돌린 레코드(예: `FSCTL_GET_NTFS_FILE_RECORD` 결과)도 받아들인다. 깨졌으면 false.
pub fn apply_fixup(buf: &mut [u8]) -> bool {
    let (Some(off), Some(cnt)) = (u16le(buf, 4), u16le(buf, 6)) else { return false };
    let (off, cnt) = (off as usize, cnt as usize);
    if cnt < 2 || off + cnt * 2 > buf.len() {
        return false;
    }
    let stride = buf.len() / (cnt - 1);
    if stride < 2 || stride * (cnt - 1) != buf.len() {
        return false;
    }
    let usn = [buf[off], buf[off + 1]];
    for i in 1..cnt {
        let end = i * stride - 2;
        let orig = [buf[off + 2 * i], buf[off + 2 * i + 1]];
        let cur = [buf[end], buf[end + 1]];
        if cur == usn {
            buf[end] = orig[0];
            buf[end + 1] = orig[1];
        } else if cur != orig {
            return false;
        }
    }
    true
}

// ---------- 데이터 런 ----------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Run {
    /// None = 비어 있는 조각(sparse)
    pub lcn: Option<u64>,
    pub len: u64,
}

pub fn decode_runs(b: &[u8]) -> Option<Vec<Run>> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut lcn: i64 = 0;
    while i < b.len() {
        let h = b[i];
        if h == 0 {
            break;
        }
        let (ls, os) = ((h & 0x0F) as usize, (h >> 4) as usize);
        i += 1;
        if ls == 0 || ls > 8 || os > 8 || i + ls + os > b.len() {
            return None;
        }
        let mut len = 0u64;
        for k in 0..ls {
            len |= (b[i + k] as u64) << (8 * k);
        }
        i += ls;
        if os == 0 {
            out.push(Run { lcn: None, len });
        } else {
            let mut d = 0i64;
            for k in 0..os {
                d |= (b[i + k] as i64) << (8 * k);
            }
            // 부호 확장
            let shift = 64 - 8 * os as u32;
            d = (d << shift) >> shift;
            lcn += d;
            if lcn < 0 {
                return None;
            }
            out.push(Run { lcn: Some(lcn as u64), len });
        }
        i += os;
    }
    Some(out)
}

// ---------- FILE 레코드 ----------

#[derive(Debug, Clone, PartialEq)]
pub struct FileName {
    pub parent: u64,
    pub ns: u8,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttrListLoc {
    Resident(Vec<u8>),
    NonResident { runs: Vec<Run>, size: u64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttrListEntry {
    pub ty: u32,
    pub lowest_vcn: u64,
    pub segment: u64,
    pub name_len: u8,
}

/// 이름 없는 `$DATA`의 비상주 조각(`$MFT` 자신의 위치를 알 때 씀)
#[derive(Debug, Clone, PartialEq)]
pub struct DataExtent {
    pub lowest_vcn: u64,
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Record {
    pub seq: u16,
    pub in_use: bool,
    pub is_dir: bool,
    /// 확장 레코드면 base 레코드의 FRN, base면 0
    pub base: u64,
    pub names: Vec<FileName>,
    /// `$STANDARD_INFORMATION` 수정 시각(FILETIME)
    pub mtime: Option<u64>,
    pub attrs: u32,
    /// 이름 없는 `$DATA` 크기(이 레코드에 첫 조각이 있을 때)
    pub size: Option<u64>,
    /// 이름 없는 `$DATA`의 실제로 쓴 크기(비상주 initialized size, `$MFT`용)
    pub init_size: Option<u64>,
    pub reparse_tag: Option<u32>,
    pub attr_list: Option<AttrListLoc>,
    pub data_extents: Vec<DataExtent>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RecErr {
    /// "FILE" 서명이 아님(쓴 적 없는 자리 포함)
    NoMagic,
    /// fixup이 맞지 않음(쓰다 끊김·깨짐)
    Fixup,
    /// 속성 구조가 잘못됨
    Corrupt,
}

/// 메모리(캐시)에 있는 레코드용 fixup: 모든 섹터 끝이 시퀀스 번호면(디스크에서 읽은 그대로) 되돌리고,
/// 아니면 이미 풀린 레코드로 보고 그대로 둔다. `FSCTL_GET_NTFS_FILE_RECORD`는 캐시의 레코드를 주는데,
/// 캐시에서는 섹터 끝이 진짜 데이터이고 배열 값은 마지막으로 디스크에 쓸 때의 것이라 맞지 않을 수 있다(엄격히 보면 멀쩡한 레코드를 버림).
pub fn apply_fixup_lenient(buf: &mut [u8]) -> bool {
    let (Some(off), Some(cnt)) = (u16le(buf, 4), u16le(buf, 6)) else { return false };
    let (off, cnt) = (off as usize, cnt as usize);
    if cnt < 2 || off + cnt * 2 > buf.len() {
        return false;
    }
    let stride = buf.len() / (cnt - 1);
    if stride < 2 || stride * (cnt - 1) != buf.len() {
        return false;
    }
    let usn = [buf[off], buf[off + 1]];
    let raw = (1..cnt).all(|i| buf[i * stride - 2] == usn[0] && buf[i * stride - 1] == usn[1]);
    if raw {
        return apply_fixup(buf);
    }
    true
}

/// FILE 레코드 하나를 푼다(디스크에서 읽은 것: fixup을 엄격히 되돌림 — 안 맞으면 쓰다 끊긴 레코드).
/// 쓰지 않는 레코드도 머리만 채워 돌려준다(`in_use == false`).
pub fn parse_record(buf: &mut [u8]) -> Result<Record, RecErr> {
    parse_record_with(buf, true)
}

/// 캐시에서 받은 레코드(`FSCTL_GET_NTFS_FILE_RECORD`)용: [`apply_fixup_lenient`]
pub fn parse_cached_record(buf: &mut [u8]) -> Result<Record, RecErr> {
    parse_record_with(buf, false)
}

fn parse_record_with(buf: &mut [u8], strict: bool) -> Result<Record, RecErr> {
    if buf.len() < 48 || &buf[0..4] != b"FILE" {
        return Err(RecErr::NoMagic);
    }
    let ok = if strict { apply_fixup(buf) } else { apply_fixup_lenient(buf) };
    if !ok {
        return Err(RecErr::Fixup);
    }
    let b = &*buf;
    let flags = u16le(b, 0x16).ok_or(RecErr::Corrupt)?;
    let mut r = Record {
        seq: u16le(b, 0x10).ok_or(RecErr::Corrupt)?,
        in_use: flags & 1 != 0,
        is_dir: flags & 2 != 0,
        base: u64le(b, 0x20).ok_or(RecErr::Corrupt)?,
        ..Default::default()
    };
    if !r.in_use {
        return Ok(r);
    }
    let used = (u32le(b, 0x18).ok_or(RecErr::Corrupt)? as usize).min(b.len());
    let mut p = u16le(b, 0x14).ok_or(RecErr::Corrupt)? as usize;
    while p + 16 <= used {
        let ty = u32le(b, p).ok_or(RecErr::Corrupt)?;
        if ty == ATTR_END {
            break;
        }
        let len = u32le(b, p + 4).ok_or(RecErr::Corrupt)? as usize;
        if len < 16 || p + len > used {
            return Err(RecErr::Corrupt);
        }
        let a = &b[p..p + len];
        let nonres = a[8] != 0;
        let name_len = a[9];
        if !nonres {
            let vl = u32le(a, 16).ok_or(RecErr::Corrupt)? as usize;
            let vo = u16le(a, 20).ok_or(RecErr::Corrupt)? as usize;
            let v = a.get(vo..vo + vl).ok_or(RecErr::Corrupt)?;
            match ty {
                ATTR_STANDARD_INFORMATION => {
                    r.mtime = u64le(v, 8);
                    r.attrs = u32le(v, 32).unwrap_or(0);
                }
                ATTR_FILE_NAME => {
                    let parent = u64le(v, 0).ok_or(RecErr::Corrupt)?;
                    let nl = *v.get(64).ok_or(RecErr::Corrupt)? as usize;
                    let ns = *v.get(65).ok_or(RecErr::Corrupt)?;
                    let name = utf16_name(v, 66, nl).ok_or(RecErr::Corrupt)?;
                    r.names.push(FileName { parent, ns, name });
                }
                ATTR_DATA if name_len == 0 => {
                    r.size = Some(vl as u64);
                    r.init_size = Some(vl as u64);
                }
                ATTR_REPARSE_POINT => r.reparse_tag = u32le(v, 0),
                ATTR_ATTRIBUTE_LIST => r.attr_list = Some(AttrListLoc::Resident(v.to_vec())),
                _ => {}
            }
        } else {
            let lowest = u64le(a, 16).ok_or(RecErr::Corrupt)?;
            let ro = u16le(a, 32).ok_or(RecErr::Corrupt)? as usize;
            let data_size = u64le(a, 48).ok_or(RecErr::Corrupt)?;
            let init = u64le(a, 56).ok_or(RecErr::Corrupt)?;
            let runs = || a.get(ro..).and_then(decode_runs).ok_or(RecErr::Corrupt);
            match ty {
                ATTR_DATA if name_len == 0 => {
                    if lowest == 0 {
                        r.size = Some(data_size);
                        r.init_size = Some(init);
                    }
                    r.data_extents.push(DataExtent { lowest_vcn: lowest, runs: runs()? });
                }
                ATTR_ATTRIBUTE_LIST => r.attr_list = Some(AttrListLoc::NonResident { runs: runs()?, size: data_size }),
                _ => {}
            }
        }
        p += len;
    }
    Ok(r)
}

/// `$ATTRIBUTE_LIST` 값 풀기
pub fn parse_attr_list(b: &[u8]) -> Vec<AttrListEntry> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 26 <= b.len() {
        let ty = u32le(b, p).unwrap();
        let len = u16le(b, p + 4).unwrap() as usize;
        if len < 26 || p + len > b.len() || ty == ATTR_END {
            break;
        }
        out.push(AttrListEntry {
            ty,
            name_len: b[p + 6],
            lowest_vcn: u64le(b, p + 8).unwrap(),
            segment: u64le(b, p + 16).unwrap(),
        });
        p += len;
    }
    out
}

/// 레코드의 이름들 중 보여 줄 것: DOS(8.3) 이름은 빼고, (부모, 이름)이 같은 것은 하나만.
/// 하드 링크(다른 부모 또는 다른 이름의 POSIX/Win32 이름)는 각각 항목이 된다 — 폴더 훑기(FindFirstFile)도 링크마다 한 줄이다.
pub fn visible_names(names: &[FileName]) -> Vec<&FileName> {
    let mut out: Vec<&FileName> = Vec::new();
    for n in names {
        if n.ns == NS_DOS {
            continue;
        }
        if out.iter().any(|o| o.parent == n.parent && o.name == n.name) {
            continue;
        }
        out.push(n);
    }
    out
}

// ---------- USN 레코드 ----------

#[derive(Debug, Clone, PartialEq)]
pub struct UsnRec {
    pub frn: u64,
    pub parent: u64,
    pub usn: i64,
    pub reason: u32,
    pub attrs: u32,
    pub name: String,
    pub major: u16,
}

/// USN_RECORD_V2/V3 하나. (레코드, 길이). V4(범위 기록) 등은 None(길이만 넘김)
pub fn parse_usn_record(b: &[u8]) -> Option<(Option<UsnRec>, usize)> {
    let len = u32le(b, 0)? as usize;
    if len < 8 || len > b.len() {
        return None;
    }
    let major = u16le(b, 4)?;
    let r = &b[..len];
    let rec = match major {
        2 => {
            let nl = u16le(r, 56)? as usize;
            let no = u16le(r, 58)? as usize;
            Some(UsnRec {
                frn: u64le(r, 8)?,
                parent: u64le(r, 16)?,
                usn: u64le(r, 24)? as i64,
                reason: u32le(r, 40)?,
                attrs: u32le(r, 52)?,
                name: utf16_name(r, no, nl / 2)?,
                major,
            })
        }
        3 => {
            // FILE_ID_128: NTFS는 아래 64비트가 FRN
            let nl = u16le(r, 72)? as usize;
            let no = u16le(r, 74)? as usize;
            Some(UsnRec {
                frn: u64le(r, 8)?,
                parent: u64le(r, 24)?,
                usn: u64le(r, 40)? as i64,
                reason: u32le(r, 56)?,
                attrs: u32le(r, 68)?,
                name: utf16_name(r, no, nl / 2)?,
                major,
            })
        }
        _ => None,
    };
    Some((rec, len))
}

/// `FSCTL_READ_USN_JOURNAL` 출력: 앞 8바이트 = 다음 USN, 그 뒤 레코드들(8바이트 정렬)
pub fn parse_usn_buffer(b: &[u8]) -> (i64, Vec<UsnRec>) {
    let next = u64le(b, 0).unwrap_or(0) as i64;
    let mut out = Vec::new();
    let mut p = 8;
    while p < b.len() {
        match parse_usn_record(&b[p..]) {
            Some((r, len)) => {
                if let Some(r) = r {
                    out.push(r);
                }
                p += (len + 7) & !7;
            }
            None => break,
        }
    }
    (next, out)
}

// ---------- $MFT 전체 읽기 ----------

pub const LINK_ROOT_SELF: u8 = 0xFF;

/// 이름 하나(하드 링크마다 하나)
#[derive(Debug, Clone, Copy)]
pub struct Link {
    pub rec: u32,
    pub parent_rec: u32,
    pub parent_seq: u16,
    pub ns: u8,
    name_off: u32,
    name_len: u32,
}

pub const T_IN_USE: u8 = 1;
pub const T_DIR: u8 = 2;
/// 이름 대리 reparse(정션·심볼릭 링크)
pub const T_LINK: u8 = 4;
/// base 레코드를 읽었음(확장 레코드만 보고 아직 base를 못 본 것과 구분)
pub const T_BASE_SEEN: u8 = 8;

/// `$MFT`를 다 읽은 결과(레코드 번호로 찾는 열 배열)
#[derive(Debug, Default)]
pub struct MftTable {
    pub geometry: Option<Geometry>,
    pub seq: Vec<u16>,
    pub flags: Vec<u8>,
    pub size: Vec<u64>,
    pub mtime: Vec<i64>,
    pub links: Vec<Link>,
    names: String,
    /// 읽은 레코드 수(쓰는 것), 깨진 레코드 수(fixup·구조), 확장 레코드 수
    pub in_use: u64,
    pub bad: u64,
    pub extensions: u64,
    /// 읽은 바이트
    pub bytes_read: u64,
    pub read_ms: u64,
}

impl MftTable {
    pub fn with_records(n: usize) -> Self {
        MftTable { seq: vec![0; n], flags: vec![0; n], size: vec![0; n], mtime: vec![0; n], ..Default::default() }
    }

    pub fn len(&self) -> usize {
        self.flags.len()
    }

    pub fn is_empty(&self) -> bool {
        self.flags.is_empty()
    }

    pub fn link_name(&self, l: &Link) -> &str {
        &self.names[l.name_off as usize..(l.name_off + l.name_len) as usize]
    }

    fn grow(&mut self, n: usize) {
        if n > self.flags.len() {
            self.seq.resize(n, 0);
            self.flags.resize(n, 0);
            self.size.resize(n, 0);
            self.mtime.resize(n, 0);
        }
    }

    /// 레코드 하나를 합친다(base든 확장이든)
    pub fn add(&mut self, rec_no: u64, r: &Record) {
        if !r.in_use {
            return;
        }
        self.in_use += 1;
        let base = if r.base != 0 { frn_rec(r.base) } else { rec_no };
        if base >= u32::MAX as u64 {
            return;
        }
        self.grow(base as usize + 1);
        let b = base as usize;
        if r.base == 0 {
            self.seq[b] = r.seq;
            self.flags[b] |= T_IN_USE | T_BASE_SEEN;
            if r.is_dir {
                self.flags[b] |= T_DIR;
            }
            if r.reparse_tag.map(is_name_surrogate).unwrap_or(false) {
                self.flags[b] |= T_LINK;
            }
            if let Some(t) = r.mtime {
                self.mtime[b] = filetime_to_unix(t);
            }
        } else {
            self.extensions += 1;
        }
        if let Some(s) = r.size {
            self.size[b] = s;
        }
        for n in visible_names(&r.names) {
            let pr = frn_rec(n.parent);
            if pr >= u32::MAX as u64 {
                continue;
            }
            let off = self.names.len() as u32;
            self.names.push_str(&n.name);
            self.links.push(Link {
                rec: b as u32,
                parent_rec: pr as u32,
                parent_seq: frn_seq(n.parent),
                ns: n.ns,
                name_off: off,
                name_len: n.name.len() as u32,
            });
        }
    }

    /// 다른 조각(병렬로 푼 것)을 합친다
    fn merge(&mut self, part: MftPart) {
        for (rec_no, r) in part.recs {
            self.add(rec_no, &r);
        }
        self.bad += part.bad;
    }

    #[inline]
    pub fn in_use_dir(&self, rec: usize, seq: u16) -> bool {
        rec < self.flags.len() && self.flags[rec] & (T_IN_USE | T_DIR) == (T_IN_USE | T_DIR) && self.seq[rec] == seq
    }
}

struct MftPart {
    recs: Vec<(u64, Record)>,
    bad: u64,
}

/// 맞춰 읽기: `align` 배수 위치·길이로 읽고 필요한 부분만
pub fn read_aligned(vol: &dyn VolRead, off: u64, len: usize, align: u64) -> io::Result<Vec<u8>> {
    let start = off / align * align;
    let end = (off + len as u64).div_ceil(align) * align;
    let mut buf = vec![0u8; (end - start) as usize];
    vol.read_at(start, &mut buf)?;
    let s = (off - start) as usize;
    Ok(buf[s..s + len].to_vec())
}

/// VCN → 바이트 위치 (조각 목록에서)
fn vcn_to_offset(extents: &[(u64, Run)], vcn: u64, cluster: u64) -> Option<u64> {
    for (start, run) in extents {
        if vcn >= *start && vcn < start + run.len {
            return run.lcn.map(|l| (l + vcn - start) * cluster);
        }
    }
    None
}

fn flatten_extents(exts: &[DataExtent]) -> Vec<(u64, Run)> {
    let mut v = Vec::new();
    for e in exts {
        let mut vcn = e.lowest_vcn;
        for r in &e.runs {
            v.push((vcn, *r));
            vcn += r.len;
        }
    }
    v.sort_by_key(|x| x.0);
    v
}

/// 비상주 값 전체 읽기(작은 것만: 속성 목록 등)
pub fn read_runs(vol: &dyn VolRead, runs: &[Run], size: u64, cluster: u64, align: u64) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(size as usize);
    for r in runs {
        if out.len() as u64 >= size {
            break;
        }
        let want = (r.len * cluster).min(size - out.len() as u64) as usize;
        match r.lcn {
            Some(l) => out.extend(read_aligned(vol, l * cluster, want, align)?),
            None => out.resize(out.len() + want, 0),
        }
    }
    out.truncate(size as usize);
    Ok(out)
}

/// `$MFT`의 위치(조각 목록)와 레코드 수
#[derive(Debug, Clone)]
pub struct MftLayout {
    pub geometry: Geometry,
    pub extents: Vec<(u64, Run)>,
    pub records: u64,
}

pub fn mft_layout(vol: &dyn VolRead) -> io::Result<MftLayout> {
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    let boot = read_aligned(vol, 0, 512, 4096)?;
    let g = parse_boot(&boot).ok_or_else(|| bad("NTFS 부트 섹터가 아님"))?;
    let cl = g.cluster as u64;
    let rs = g.record_size as usize;
    let align = (g.bytes_per_sector as u64).max(512);
    let mut r0buf = read_aligned(vol, g.mft_lcn * cl, rs, align)?;
    let r0 = parse_record(&mut r0buf).map_err(|e| bad(&format!("$MFT 레코드 0: {e:?}")))?;
    let mut exts = r0.data_extents.clone();
    let mut size = r0.size.unwrap_or(0);
    let mut init = r0.init_size.unwrap_or(size);
    // $MFT가 아주 조각나 속성 목록이 생긴 경우: 다른 레코드에 있는 $DATA 조각도 모은다
    if let Some(al) = &r0.attr_list {
        let list = match al {
            AttrListLoc::Resident(v) => v.clone(),
            AttrListLoc::NonResident { runs, size } => read_runs(vol, runs, *size, cl, align)?,
        };
        let mut todo: Vec<u64> =
            parse_attr_list(&list).into_iter().filter(|e| e.ty == ATTR_DATA && e.name_len == 0 && frn_rec(e.segment) != 0).map(|e| frn_rec(e.segment)).collect();
        todo.dedup();
        for _ in 0..4 {
            let flat = flatten_extents(&exts);
            let mut left = Vec::new();
            for rec in todo {
                let byte = rec * rs as u64;
                match vcn_to_offset(&flat, byte / cl, cl) {
                    Some(o) => {
                        let mut b = read_aligned(vol, o + byte % cl, rs, align)?;
                        if let Ok(r) = parse_record(&mut b) {
                            if r.size.is_some() {
                                size = r.size.unwrap();
                                init = r.init_size.unwrap_or(size);
                            }
                            exts.extend(r.data_extents);
                        }
                    }
                    None => left.push(rec),
                }
            }
            todo = left;
            if todo.is_empty() {
                break;
            }
        }
    }
    let extents = flatten_extents(&exts);
    if extents.is_empty() {
        return Err(bad("$MFT 데이터 런이 없음"));
    }
    Ok(MftLayout { records: size.min(init) / rs as u64, geometry: g, extents })
}

/// `$MFT`를 통째로 읽어 푼다. 읽기 스레드 하나 + 푸는 스레드 여럿.
pub fn read_mft(vol: &dyn VolRead, threads: usize, cancel: &std::sync::atomic::AtomicBool) -> io::Result<MftTable> {
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::sync_channel;
    use std::sync::Mutex;
    let t0 = std::time::Instant::now();
    let lay = mft_layout(vol)?;
    let g = lay.geometry.clone();
    let cl = g.cluster as u64;
    let rs = g.record_size as u64;
    let total = lay.records;
    // 조각마다 4 MB(클러스터·레코드 배수)씩
    let chunk_clusters = ((4u64 << 20) / cl).max(1).max(rs.div_ceil(cl));
    let table = Mutex::new(MftTable::with_records(total as usize));
    let (tx, rx) = sync_channel::<(u64, Vec<u8>)>(threads.max(1) * 2);
    let rx = Mutex::new(rx);
    let mut read_err: Option<io::Error> = None;
    let mut bytes = 0u64;
    std::thread::scope(|sc| {
        for _ in 0..threads.max(1) {
            sc.spawn(|| loop {
                let msg = rx.lock().unwrap().recv();
                let Ok((first, mut buf)) = msg else { return };
                let mut part = MftPart { recs: Vec::with_capacity(buf.len() / rs as usize), bad: 0 };
                for (i, rb) in buf.chunks_exact_mut(rs as usize).enumerate() {
                    let rec_no = first + i as u64;
                    if rec_no >= total {
                        break;
                    }
                    match parse_record(rb) {
                        Ok(r) if r.in_use => part.recs.push((rec_no, r)),
                        Ok(_) | Err(RecErr::NoMagic) => {}
                        Err(_) => part.bad += 1,
                    }
                }
                table.lock().unwrap().merge(part);
            });
        }
        'outer: for (vcn, run) in &lay.extents {
            let Some(lcn) = run.lcn else { continue };
            let mut c = 0;
            while c < run.len {
                if cancel.load(Ordering::Relaxed) {
                    break 'outer;
                }
                let first_rec = (vcn + c) * cl / rs;
                if first_rec >= total {
                    break 'outer;
                }
                let n = chunk_clusters.min(run.len - c);
                let mut buf = vec![0u8; (n * cl) as usize];
                if let Err(e) = vol.read_at((lcn + c) * cl, &mut buf) {
                    read_err = Some(e);
                    break 'outer;
                }
                bytes += buf.len() as u64;
                if tx.send((first_rec, buf)).is_err() {
                    break 'outer;
                }
                c += n;
            }
        }
        drop(tx);
    });
    if let Some(e) = read_err {
        return Err(e);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "취소"));
    }
    let mut t = table.into_inner().unwrap();
    t.geometry = Some(g);
    t.bytes_read = bytes;
    t.read_ms = t0.elapsed().as_millis() as u64;
    Ok(t)
}

/// 레코드 하나 읽기(이미지·볼륨에서 직접). 확장 레코드 합치기는 [`MftTable`]처럼 부르는 쪽에서.
pub fn read_record_at(vol: &dyn VolRead, lay: &MftLayout, rec: u64) -> io::Result<Vec<u8>> {
    let cl = lay.geometry.cluster as u64;
    let rs = lay.geometry.record_size as u64;
    let byte = rec * rs;
    let o = vcn_to_offset(&lay.extents, byte / cl, cl).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "레코드 범위 밖"))?;
    read_aligned(vol, o + byte % cl, rs as usize, (lay.geometry.bytes_per_sector as u64).max(512))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_signed_offsets() {
        // 길이 0x10 @ 0x1000, 길이 0x20 @ -0x800 상대(=0x800), sparse 0x5
        let b = [0x21, 0x10, 0x00, 0x10, 0x21, 0x20, 0x00, 0xF8, 0x01, 0x05, 0x00];
        let r = decode_runs(&b).unwrap();
        assert_eq!(r, vec![Run { lcn: Some(0x1000), len: 0x10 }, Run { lcn: Some(0x800), len: 0x20 }, Run { lcn: None, len: 5 }]);
    }

    #[test]
    fn boot_geometry() {
        let mut b = vec![0u8; 512];
        b[3..11].copy_from_slice(b"NTFS    ");
        b[0x0B..0x0D].copy_from_slice(&512u16.to_le_bytes());
        b[0x0D] = 8;
        b[0x30..0x38].copy_from_slice(&786432u64.to_le_bytes());
        b[0x40] = 0xF6; // -10 → 1024
        b[0x48..0x50].copy_from_slice(&0x1234_5678_9abc_def0u64.to_le_bytes());
        let g = parse_boot(&b).unwrap();
        assert_eq!((g.cluster, g.record_size, g.mft_lcn, g.serial), (4096, 1024, 786432, 0x1234_5678_9abc_def0));
    }
}
