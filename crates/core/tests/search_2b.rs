//! 2b: NTFS FILE 레코드 해석(손으로 만든 레코드), USN 레코드, 색인 v1→v2, 저널 따라잡기 반영,
//! 그리고 (K12에 mkntfs·ntfs-3g가 있으면) 진짜 NTFS 이미지의 $MFT를 읽어 폴더 훑기 결과와 비교.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use filetool_core::search::index::{Index, SourceKind, UsnCursor, INDEX_FILE, INDEX_FILE_V1};
use filetool_core::search::mft::{self, build_tree, FileState, FrnMap, JournalErr, JournalInfo, MftSource, VolumeView};
use filetool_core::search::ntfs::{self, frn, MftTable, Record, UsnRec};
use filetool_core::search::scan::{self, DirWalkSource, Excludes, RawEntry, ScanControl, ScanOpts};

// ---------- FILE 레코드 만들기 ----------

fn align8(n: usize) -> usize {
    (n + 7) & !7
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|c| c.to_le_bytes()).collect()
}

fn resident(ty: u32, value: &[u8]) -> Vec<u8> {
    let len = align8(24 + value.len());
    let mut a = vec![0u8; len];
    a[0..4].copy_from_slice(&ty.to_le_bytes());
    a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    a[8] = 0;
    a[16..20].copy_from_slice(&(value.len() as u32).to_le_bytes());
    a[20..22].copy_from_slice(&24u16.to_le_bytes());
    a[24..24 + value.len()].copy_from_slice(value);
    a
}

fn named_resident(ty: u32, name: &str, value: &[u8]) -> Vec<u8> {
    let n = utf16(name);
    let vo = align8(24 + n.len());
    let len = align8(vo + value.len());
    let mut a = vec![0u8; len];
    a[0..4].copy_from_slice(&ty.to_le_bytes());
    a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    a[9] = (n.len() / 2) as u8;
    a[10..12].copy_from_slice(&24u16.to_le_bytes());
    a[16..20].copy_from_slice(&(value.len() as u32).to_le_bytes());
    a[20..22].copy_from_slice(&(vo as u16).to_le_bytes());
    a[24..24 + n.len()].copy_from_slice(&n);
    a[vo..vo + value.len()].copy_from_slice(value);
    a
}

fn nonresident(ty: u32, lowest_vcn: u64, runs: &[u8], data_size: u64) -> Vec<u8> {
    let len = align8(64 + runs.len() + 1);
    let mut a = vec![0u8; len];
    a[0..4].copy_from_slice(&ty.to_le_bytes());
    a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
    a[8] = 1;
    a[16..24].copy_from_slice(&lowest_vcn.to_le_bytes());
    a[32..34].copy_from_slice(&64u16.to_le_bytes());
    a[40..48].copy_from_slice(&data_size.next_multiple_of(4096).to_le_bytes());
    a[48..56].copy_from_slice(&data_size.to_le_bytes());
    a[56..64].copy_from_slice(&data_size.to_le_bytes());
    a[64..64 + runs.len()].copy_from_slice(runs);
    a
}

/// 2024-05-01 00:00:00 UTC 근처 FILETIME
const FT: u64 = 133_590_240_000_000_000;

fn si(mtime: u64, attrs: u32) -> Vec<u8> {
    let mut v = vec![0u8; 72];
    v[0..8].copy_from_slice(&mtime.to_le_bytes());
    v[8..16].copy_from_slice(&mtime.to_le_bytes());
    v[32..36].copy_from_slice(&attrs.to_le_bytes());
    resident(ntfs::ATTR_STANDARD_INFORMATION, &v)
}

fn fname(parent: u64, ns: u8, name: &str) -> Vec<u8> {
    let n = utf16(name);
    let mut v = vec![0u8; 66 + n.len()];
    v[0..8].copy_from_slice(&parent.to_le_bytes());
    v[64] = (n.len() / 2) as u8;
    v[65] = ns;
    v[66..].copy_from_slice(&n);
    resident(ntfs::ATTR_FILE_NAME, &v)
}

fn attr_list(entries: &[(u32, u64)]) -> Vec<u8> {
    let mut v = Vec::new();
    for (ty, seg) in entries {
        let mut e = vec![0u8; 32];
        e[0..4].copy_from_slice(&ty.to_le_bytes());
        e[4..6].copy_from_slice(&32u16.to_le_bytes());
        e[7] = 26;
        e[16..24].copy_from_slice(&seg.to_le_bytes());
        v.extend(e);
    }
    resident(ntfs::ATTR_ATTRIBUTE_LIST, &v)
}

struct Rb {
    seq: u16,
    flags: u16,
    base: u64,
    attrs: Vec<Vec<u8>>,
}

impl Rb {
    fn file(seq: u16) -> Rb {
        Rb { seq, flags: 1, base: 0, attrs: Vec::new() }
    }
    fn dir(seq: u16) -> Rb {
        Rb { seq, flags: 3, base: 0, attrs: Vec::new() }
    }
    fn a(mut self, a: Vec<u8>) -> Rb {
        self.attrs.push(a);
        self
    }
    /// 1024바이트 레코드 + fixup 보호(512바이트마다 끝 2바이트를 시퀀스 번호로)
    fn build(&self) -> Vec<u8> {
        let mut b = vec![0u8; 1024];
        b[0..4].copy_from_slice(b"FILE");
        b[4..6].copy_from_slice(&48u16.to_le_bytes());
        b[6..8].copy_from_slice(&3u16.to_le_bytes());
        b[0x10..0x12].copy_from_slice(&self.seq.to_le_bytes());
        b[0x12..0x14].copy_from_slice(&1u16.to_le_bytes());
        b[0x14..0x16].copy_from_slice(&56u16.to_le_bytes());
        b[0x16..0x18].copy_from_slice(&self.flags.to_le_bytes());
        b[0x1C..0x20].copy_from_slice(&1024u32.to_le_bytes());
        b[0x20..0x28].copy_from_slice(&self.base.to_le_bytes());
        let mut p = 56;
        for a in &self.attrs {
            b[p..p + a.len()].copy_from_slice(a);
            p += a.len();
        }
        b[p..p + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        p += 8;
        assert!(p <= 1020, "레코드가 너무 큼");
        b[0x18..0x1C].copy_from_slice(&(p as u32).to_le_bytes());
        // 업데이트 시퀀스: 번호 0x0007
        b[48..50].copy_from_slice(&7u16.to_le_bytes());
        for i in 1..3 {
            let end = i * 512 - 2;
            let (o0, o1) = (b[end], b[end + 1]);
            b[48 + 2 * i] = o0;
            b[48 + 2 * i + 1] = o1;
            b[end] = 7;
            b[end + 1] = 0;
        }
        b
    }
}

fn parse(b: &[u8]) -> Record {
    let mut v = b.to_vec();
    ntfs::parse_record(&mut v).expect("레코드")
}

#[test]
fn fixup_applied_and_corruption_rejected() {
    // 이름을 길게 해 첫 섹터 끝(510)을 걸치게 한다
    let long = "가".repeat(120);
    let r = Rb::file(3).a(si(FT, 0x20)).a(fname(frn(5, 5), ntfs::NS_POSIX, &long)).a(resident(ntfs::ATTR_DATA, b"hello"));
    let b = r.build();
    let rec = parse(&b);
    assert_eq!(rec.names[0].name, long, "fixup으로 섹터 끝 2바이트가 원래대로");
    assert_eq!(rec.seq, 3);
    // 이미 되돌린 레코드(FSCTL_GET_NTFS_FILE_RECORD 결과처럼)도 받아들임
    let mut once = b.clone();
    assert!(ntfs::apply_fixup(&mut once));
    assert_eq!(parse(&once).names[0].name, long);
    // 둘째 섹터 끝이 다르면(쓰다 끊김) 거절
    let mut bad = b.clone();
    bad[1022] = 0x55;
    assert_eq!(ntfs::parse_record(&mut bad), Err(ntfs::RecErr::Fixup));
    // 캐시에서 받은 레코드(FSCTL_GET_NTFS_FILE_RECORD): 섹터 끝이 진짜 데이터이고 배열 값은 옛것 → 엄격하면 버리지만 너그럽게는 그대로 읽음
    let mut cached = b.clone();
    assert!(ntfs::apply_fixup(&mut cached));
    cached[48 + 2] ^= 0xFF; // 배열의 옛 값(마지막 디스크 쓰기 뒤 레코드가 바뀜)
    let mut strict = cached.clone();
    assert_eq!(ntfs::parse_record(&mut strict), Err(ntfs::RecErr::Fixup));
    assert_eq!(ntfs::parse_cached_record(&mut cached).unwrap().names[0].name, long);
    // 디스크에서 읽은 그대로(섹터 끝 = 시퀀스 번호)면 너그러운 쪽도 되돌린다
    let mut raw = b.clone();
    assert_eq!(ntfs::parse_cached_record(&mut raw).unwrap().names[0].name, long);
    // 서명 없음
    let mut zero = vec![0u8; 1024];
    assert_eq!(ntfs::parse_record(&mut zero), Err(ntfs::RecErr::NoMagic));
}

#[test]
fn resident_and_nonresident_data_size_and_si() {
    let r = parse(&Rb::file(1).a(si(FT, 0x20)).a(fname(frn(5, 5), 1, "a.txt")).a(resident(ntfs::ATTR_DATA, &[1u8; 300])).build());
    assert_eq!(r.size, Some(300));
    assert_eq!(r.mtime, Some(FT));
    assert_eq!(ntfs::filetime_to_unix(FT), 1_714_550_400);
    assert!(r.in_use && !r.is_dir);
    // 비상주: 첫 조각(lowest VCN 0)의 data size, 이름 있는 $DATA(대체 스트림)는 크기에 안 셈
    let runs = [0x21, 0x10, 0x00, 0x10, 0x00];
    let r = parse(
        &Rb::file(1)
            .a(si(FT, 0))
            .a(fname(frn(5, 5), 1, "big.bin"))
            .a(named_resident(ntfs::ATTR_DATA, "Zone.Identifier", b"[ZoneTransfer]"))
            .a(nonresident(ntfs::ATTR_DATA, 0, &runs, 5_000_000_123))
            .build(),
    );
    assert_eq!(r.size, Some(5_000_000_123));
    assert_eq!(r.data_extents[0].runs, vec![ntfs::Run { lcn: Some(0x1000), len: 0x10 }]);
    // 둘째 조각(lowest VCN ≠ 0)만 있는 확장 레코드는 크기를 주지 않음
    let r = parse(&Rb { seq: 1, flags: 1, base: frn(40, 1), attrs: vec![nonresident(ntfs::ATTR_DATA, 0x10, &runs, 999)] }.build());
    assert_eq!(r.size, None);
    assert_eq!(r.base, frn(40, 1));
}

#[test]
fn attribute_list_and_extension_records_merge() {
    // base(레코드 40): $SI + 속성 목록, $FILE_NAME·$DATA는 확장 레코드 41에
    let base = Rb::file(2).a(si(FT, 0)).a(attr_list(&[(0x10, frn(40, 2)), (0x30, frn(41, 1)), (0x80, frn(41, 1))])).build();
    let ext = Rb { seq: 1, flags: 1, base: frn(40, 2), attrs: vec![fname(frn(5, 5), 1, "조각난 파일.dat"), resident(ntfs::ATTR_DATA, &[0u8; 77])] }.build();
    let b = parse(&base);
    let list = match b.attr_list.as_ref().unwrap() {
        ntfs::AttrListLoc::Resident(v) => ntfs::parse_attr_list(v),
        _ => panic!("상주 목록"),
    };
    assert_eq!(list.len(), 3);
    assert_eq!((list[1].ty, list[1].segment), (0x30, frn(41, 1)));
    assert!(b.names.is_empty() && b.size.is_none());
    // 레코드 하나 읽기(저널 반영): 확장 레코드를 합친 상태
    let e = parse(&ext);
    let fs = FileState::from_records(frn(40, 2), &b, &[e.clone()]);
    assert_eq!(fs.names, vec![(frn(5, 5), "조각난 파일.dat".to_string())]);
    assert_eq!(fs.size, 77);
    // 전체 읽기: 확장 레코드가 base보다 먼저 와도 합쳐짐
    let mut t = table_with_root();
    t.add(41, &e);
    t.add(40, &b);
    let ix = tree(&t, &ScanOpts::default());
    assert_eq!(listing(&ix), vec![("/r/조각난 파일.dat".into(), false, 77)]);
}

#[test]
fn file_name_namespaces_hard_links_and_unused_records() {
    let mut t = table_with_root();
    // 폴더 20 "Documents"(Win32) + DOS 이름 "DOCUME~1"
    t.add(20, &parse(&Rb::dir(1).a(si(FT, 0x10)).a(fname(frn(5, 5), 2, "DOCUME~1")).a(fname(frn(5, 5), 1, "Documents")).build()));
    // 파일 21: Win32&DOS 한 이름 + POSIX 하드 링크(다른 폴더) + 같은 폴더의 두 번째 링크
    t.add(
        21,
        &parse(
            &Rb::file(4)
                .a(si(FT, 0))
                .a(fname(frn(20, 1), 3, "REPORT.TXT"))
                .a(fname(frn(5, 5), 0, "link-to-report.txt"))
                .a(fname(frn(20, 1), 0, "report-copy.txt"))
                .a(resident(ntfs::ATTR_DATA, b"12345"))
                .build(),
        ),
    );
    // 지운 레코드 22(쓰지 않음 표시) — 이름이 남아 있어도 빠짐
    t.add(22, &parse(&Rb { seq: 1, flags: 0, base: 0, attrs: vec![si(FT, 0), fname(frn(20, 1), 1, "deleted.txt")] }.build()));
    // 부모 순번이 다른 이름(옛 폴더 아래) → 고아로 빠짐
    t.add(23, &parse(&Rb::file(1).a(si(FT, 0)).a(fname(frn(20, 9), 1, "orphan.txt")).build()));
    // 메타데이터: $Extend(11) 아래 $UsnJrnl(레코드 30) — 보이지 않음
    t.add(11, &parse(&Rb::dir(11).a(si(FT, 0)).a(fname(frn(5, 5), 3, "$Extend")).build()));
    t.add(30, &parse(&Rb::file(1).a(si(FT, 0)).a(fname(frn(11, 11), 3, "$UsnJrnl")).build()));
    // 정션(이름 대리 reparse) 폴더 24: 항목으로만, 연결 지점으로 셈
    let mut rp = vec![0u8; 16];
    rp[0..4].copy_from_slice(&0xA000_0003u32.to_le_bytes());
    t.add(24, &parse(&Rb::dir(1).a(si(FT, 0x400)).a(fname(frn(5, 5), 1, "Junction")).a(resident(ntfs::ATTR_REPARSE_POINT, &rp)).build()));
    let ix = RwLock::new(Index::new());
    let rid = ix.write().unwrap().add_root(Path::new("/r"));
    let mut orphans = 0;
    let st = build_tree(&t, Path::new("/r"), rid, &ix, &ScanOpts::default(), &ScanControl::default(), &mut orphans);
    let ix = ix.into_inner().unwrap();
    assert_eq!(
        listing(&ix),
        vec![
            ("/r/Documents".into(), true, 0),
            ("/r/Documents/REPORT.TXT".into(), false, 5),
            ("/r/Documents/report-copy.txt".into(), false, 5),
            ("/r/Junction".into(), true, 0),
            ("/r/link-to-report.txt".into(), false, 5),
        ]
    );
    assert_eq!((st.dirs, st.files, st.reparse), (2, 3, 1));
    assert_eq!(orphans, 1);
    // 같은 레코드의 세 링크는 같은 FRN
    let f: Vec<u64> = (0..ix.len() as u32).filter(|&i| ix.is_live(i) && !ix.is_dir(i)).map(|i| ix.frn(i)).collect();
    assert!(f.iter().all(|&x| x == frn(21, 4)));
}

#[test]
fn excludes_apply_to_mft_tree() {
    let mut t = table_with_root();
    t.add(20, &parse(&Rb::dir(1).a(si(FT, 0)).a(fname(frn(5, 5), 1, "$Recycle.Bin")).build()));
    t.add(21, &parse(&Rb::file(1).a(si(FT, 0)).a(fname(frn(20, 1), 1, "trash.txt")).build()));
    t.add(22, &parse(&Rb::dir(1).a(si(FT, 0)).a(fname(frn(5, 5), 1, "Windows")).build()));
    t.add(23, &parse(&Rb::dir(1).a(si(FT, 0)).a(fname(frn(22, 1), 1, "WinSxS")).build()));
    t.add(24, &parse(&Rb::dir(1).a(si(FT, 0)).a(fname(frn(22, 1), 1, "System32")).build()));
    let mut ex = scan::default_excludes();
    ex.push("*\\Windows\\WinSxS".into()); // Linux 경로라 ?:\ 대신
    let opts = ScanOpts { excludes: Excludes::new(&ex), ..Default::default() };
    let ix = tree(&t, &opts);
    assert_eq!(listing(&ix), vec![("/r/Windows".into(), true, 0), ("/r/Windows/System32".into(), true, 0)]);
}

#[test]
fn usn_record_v2_v3() {
    let mut b = vec![0u8; 8];
    b[0..8].copy_from_slice(&4096i64.to_le_bytes());
    // V2
    let n = utf16("새 문서.txt");
    let mut r2 = vec![0u8; align8(60 + n.len())];
    let l = r2.len() as u32;
    r2[0..4].copy_from_slice(&l.to_le_bytes());
    r2[4..6].copy_from_slice(&2u16.to_le_bytes());
    r2[8..16].copy_from_slice(&frn(100, 3).to_le_bytes());
    r2[16..24].copy_from_slice(&frn(5, 5).to_le_bytes());
    r2[24..32].copy_from_slice(&1000i64.to_le_bytes());
    r2[40..44].copy_from_slice(&0x100u32.to_le_bytes()); // FILE_CREATE
    r2[52..56].copy_from_slice(&0x20u32.to_le_bytes());
    r2[56..58].copy_from_slice(&(n.len() as u16).to_le_bytes());
    r2[58..60].copy_from_slice(&60u16.to_le_bytes());
    r2[60..60 + n.len()].copy_from_slice(&n);
    b.extend(&r2);
    // V3(FILE_ID_128)
    let n3 = utf16("b.txt");
    let mut r3 = vec![0u8; align8(76 + n3.len())];
    let l = r3.len() as u32;
    r3[0..4].copy_from_slice(&l.to_le_bytes());
    r3[4..6].copy_from_slice(&3u16.to_le_bytes());
    r3[8..16].copy_from_slice(&frn(200, 1).to_le_bytes());
    r3[24..32].copy_from_slice(&frn(100, 3).to_le_bytes());
    r3[40..48].copy_from_slice(&2000i64.to_le_bytes());
    r3[56..60].copy_from_slice(&0x2000u32.to_le_bytes()); // RENAME_NEW_NAME
    r3[72..74].copy_from_slice(&(n3.len() as u16).to_le_bytes());
    r3[74..76].copy_from_slice(&76u16.to_le_bytes());
    r3[76..76 + n3.len()].copy_from_slice(&n3);
    b.extend(&r3);
    // V4(범위 기록)는 건너뜀
    let mut r4 = vec![0u8; 40];
    r4[0..4].copy_from_slice(&40u32.to_le_bytes());
    r4[4..6].copy_from_slice(&4u16.to_le_bytes());
    b.extend(&r4);
    let (next, recs) = ntfs::parse_usn_buffer(&b);
    assert_eq!(next, 4096);
    assert_eq!(recs.len(), 2);
    assert_eq!((recs[0].frn, recs[0].parent, recs[0].usn, recs[0].reason, recs[0].name.as_str(), recs[0].major), (frn(100, 3), frn(5, 5), 1000, 0x100, "새 문서.txt", 2));
    assert_eq!((recs[1].frn, recs[1].parent, recs[1].usn, recs[1].name.as_str(), recs[1].major), (frn(200, 1), frn(100, 3), 2000, "b.txt", 3));
}

// ---------- 색인 v1 → v2 ----------

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
fn fnv(data: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    for b in data {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// 2a(v1) 형식 그대로 쓰기: 루트 R:\ 아래 폴더 "문서", 그 아래 파일 "a.txt"
fn write_v1(path: &Path) {
    let mut w = Vec::new();
    w.extend(b"WFTIDX01");
    w.extend(1u32.to_le_bytes());
    w.extend(0i64.to_le_bytes());
    w.extend(1u32.to_le_bytes());
    let p = "/r";
    w.extend((p.len() as u32).to_le_bytes());
    w.extend(p.as_bytes());
    w.extend(0u32.to_le_bytes());
    w.extend(1_700_000_000i64.to_le_bytes());
    for v in [1u64, 1, 0, 0, 0, 12] {
        w.extend(v.to_le_bytes());
    }
    let names = ["/r", "문서", "a.txt"];
    w.extend(3u32.to_le_bytes());
    for v in [u32::MAX, 0, 1] {
        w.extend(v.to_le_bytes());
    }
    w.extend([1u8 | 4, 1, 0]);
    for v in [0u64, 0, 42] {
        w.extend(v.to_le_bytes());
    }
    for v in [0i64, 1_700_000_000, 1_700_000_001] {
        w.extend(v.to_le_bytes());
    }
    for n in names {
        w.extend((n.len() as u16).to_le_bytes());
    }
    for n in names {
        w.extend(n.as_bytes());
    }
    let h = fnv(&w);
    w.extend(h.to_le_bytes());
    std::fs::write(path, w).unwrap();
}

#[test]
fn index_v1_loads_and_v2_keeps_frn_and_cursor() {
    let d = tempfile::tempdir().unwrap();
    let v1 = d.path().join(INDEX_FILE_V1);
    let v2 = d.path().join(INDEX_FILE);
    write_v1(&v1);
    let ix = Index::load_dir_file(&v2).expect("v1 읽기");
    assert_eq!(ix.live_count(), 2);
    let a = ix.lookup(Path::new("/r/문서/a.txt")).unwrap();
    assert_eq!((ix.size(a), ix.mtime(a), ix.frn(a)), (42, 1_700_000_001, 0));
    assert_eq!(ix.roots()[0].source, SourceKind::Walk);
    assert_eq!(ix.roots()[0].usn, None);
    assert_eq!(ix.roots()[0].stats.millis, 12);
    // v2로 저장하면 v1은 지워지고, FRN·원본·저널 위치가 남는다
    let mut ix = ix;
    let rid = ix.roots()[0].id;
    let docs = ix.lookup(Path::new("/r/문서")).unwrap();
    ix.push_child_frn(docs, "b.txt", false, 7, 5, frn(77, 2));
    let cur = UsnCursor { volume_serial: 0xABCD, journal_id: 0x1D0_0000_0001, next_usn: 123_456_789 };
    if let Some(r) = ix.roots_mut().iter_mut().find(|r| r.id == rid) {
        r.source = SourceKind::Mft;
        r.usn = Some(cur);
    }
    ix.save(&v2).unwrap();
    assert!(v2.exists() && !v1.exists());
    let back = Index::load_dir_file(&v2).unwrap();
    let b = back.lookup(Path::new("/r/문서/b.txt")).unwrap();
    assert_eq!(back.frn(b), frn(77, 2));
    assert_eq!(back.roots()[0].source, SourceKind::Mft);
    assert_eq!(back.roots()[0].usn, Some(cur));
    // 다른 버전은 거절
    let mut bytes = std::fs::read(&v2).unwrap();
    bytes[8] = 9;
    let n = bytes.len();
    let h = fnv(&bytes[..n - 8]);
    bytes[n - 8..].copy_from_slice(&h.to_le_bytes());
    std::fs::write(&v2, &bytes).unwrap();
    assert!(Index::load(&v2).is_err());
}

// ---------- 저널 따라잡기 ----------

#[derive(Default)]
struct Fake {
    serial: u64,
    journal: JournalInfo,
    log: Vec<UsnRec>,
    files: HashMap<u64, FileState>,
    /// 폴더 경로 → 목록(새로 나타난 폴더 채우기)
    dirs: HashMap<PathBuf, Vec<(u64, RawEntry)>>,
}

struct FakeView(Mutex<Fake>);

impl VolumeView for FakeView {
    fn serial(&self) -> u64 {
        self.0.lock().unwrap().serial
    }
    fn query_journal(&self) -> Result<JournalInfo, JournalErr> {
        Ok(self.0.lock().unwrap().journal)
    }
    fn read_journal(&self, id: u64, start: i64) -> Result<(i64, Vec<UsnRec>), JournalErr> {
        let f = self.0.lock().unwrap();
        if id != f.journal.journal_id {
            return Err(JournalErr::Reset("id".into()));
        }
        // 한 번에 두 개씩(여러 번 읽기 확인)
        let recs: Vec<UsnRec> = f.log.iter().filter(|r| r.usn >= start).take(2).cloned().collect();
        let next = recs.last().map(|r| r.usn + 1).unwrap_or(f.journal.next_usn);
        Ok((next, recs))
    }
    fn record(&self, f: u64) -> Option<FileState> {
        self.0.lock().unwrap().files.get(&ntfs::frn_rec(f)).filter(|s| s.frn == f).cloned()
    }
    fn list_dir(&self, path: &Path) -> std::io::Result<Vec<(u64, RawEntry)>> {
        Ok(self.0.lock().unwrap().dirs.get(path).cloned().unwrap_or_default())
    }
}

fn st(f: u64, dir: bool, size: u64, names: &[(u64, &str)]) -> FileState {
    FileState { frn: f, is_dir: dir, link: false, size, mtime: 1_700_000_000, names: names.iter().map(|(p, n)| (*p, n.to_string())).collect() }
}

fn ev(usn: i64, f: u64) -> UsnRec {
    UsnRec { frn: f, parent: 0, usn, reason: 0, attrs: 0, name: String::new(), major: 2 }
}

#[test]
fn catch_up_applies_creates_renames_moves_deletes() {
    let root = frn(5, 5);
    let (docs, pics, old, a, b, sub, inner, trash, restored, rin) =
        (frn(20, 1), frn(21, 1), frn(22, 1), frn(30, 1), frn(31, 1), frn(40, 1), frn(41, 1), frn(50, 1), frn(60, 1), frn(61, 1));
    // 처음 상태: R:\docs\a.txt, R:\docs\old\, R:\docs\sub\inner.txt, R:\pics\, R:\$Recycle.Bin\(제외)
    let mut t = table_with_root();
    let rec = |dir: bool, seq: u16, names: &[(u64, u8, &str)], size: usize| {
        let mut r = if dir { Rb::dir(seq) } else { Rb::file(seq) }.a(si(FT, 0));
        for (p, ns, n) in names {
            r = r.a(fname(*p, *ns, n));
        }
        if !dir {
            r = r.a(resident(ntfs::ATTR_DATA, &vec![0u8; size]));
        }
        parse(&r.build())
    };
    t.add(20, &rec(true, 1, &[(root, 1, "docs")], 0));
    t.add(21, &rec(true, 1, &[(root, 1, "pics")], 0));
    t.add(22, &rec(true, 1, &[(docs, 1, "old")], 0));
    t.add(30, &rec(false, 1, &[(docs, 1, "a.txt")], 10));
    t.add(40, &rec(true, 1, &[(docs, 1, "sub")], 0));
    t.add(41, &rec(false, 1, &[(sub, 1, "inner.txt")], 3));
    t.add(50, &rec(true, 1, &[(root, 1, "$Recycle.Bin")], 0));
    let opts = ScanOpts { excludes: Excludes::new(&scan::default_excludes()), ..Default::default() };
    let index = RwLock::new(tree(&t, &opts));
    let rid = index.read().unwrap().roots()[0].id;
    let cur = UsnCursor { volume_serial: 9, journal_id: 77, next_usn: 100 };

    // 그 뒤 일어난 일(저널):
    //  1) a.txt → pics\a-renamed.txt 로 옮기며 이름 바꿈, 크기 10→20
    //  2) docs\sub(아래 inner.txt째) → pics\sub2 로 옮김
    //  3) docs\old 지움
    //  4) 새 폴더 docs\new\ 와 그 안 b.txt (저널에는 b.txt가 먼저 — 부모를 다음 차례에)
    //  5) 휴지통에 있던 폴더 restored(아래 r.txt)를 docs로 되살림 — 아래 항목은 저널에 없음 → 목록으로 채움
    //  6) inner.txt에 하드 링크 하나 더(pics\inner-link.txt)
    let new_dir = frn(70, 1);
    let mut fk = Fake { serial: 9, journal: JournalInfo { journal_id: 77, first_usn: 50, next_usn: 200 }, ..Default::default() };
    fk.files.insert(20, st(docs, true, 0, &[(root, "docs")]));
    fk.files.insert(21, st(pics, true, 0, &[(root, "pics")]));
    fk.files.insert(30, st(a, false, 20, &[(pics, "a-renamed.txt")]));
    fk.files.insert(40, st(sub, true, 0, &[(pics, "sub2")]));
    fk.files.insert(41, st(inner, false, 3, &[(sub, "inner.txt"), (pics, "inner-link.txt")]));
    fk.files.insert(70, st(new_dir, true, 0, &[(docs, "new")]));
    fk.files.insert(31, st(b, false, 5, &[(new_dir, "b.txt")]));
    fk.files.insert(60, st(restored, true, 0, &[(docs, "restored")]));
    fk.dirs.insert(
        PathBuf::from("/r/docs/restored"),
        vec![(rin, RawEntry { name: "r.txt".into(), is_dir: false, size: 8, mtime: 1_700_000_100, link: false })],
    );
    let _ = (trash, old);
    fk.log = vec![ev(100, a), ev(110, sub), ev(120, old), ev(130, b), ev(140, new_dir), ev(150, restored), ev(160, inner), ev(170, a)];
    let view = FakeView(Mutex::new(fk));

    let (frns, newc, n) = mft::catch_up(&view, &cur).unwrap();
    assert_eq!(n, 8);
    assert_eq!(frns, vec![a, sub, old, b, new_dir, restored, inner], "중복 빼고 처음 나온 순서");
    assert_eq!(newc, UsnCursor { next_usn: 200, ..cur });
    let mut map = FrnMap::empty(rid);
    let s = mft::apply_changes(&index, &mut map, Path::new("/r"), &frns, &view, &opts);
    let ix = index.read().unwrap();
    assert_eq!(
        listing(&ix),
        vec![
            ("/r/docs".into(), true, 0),
            ("/r/docs/new".into(), true, 0),
            ("/r/docs/new/b.txt".into(), false, 5),
            ("/r/docs/restored".into(), true, 0),
            ("/r/docs/restored/r.txt".into(), false, 8),
            ("/r/pics".into(), true, 0),
            ("/r/pics/a-renamed.txt".into(), false, 20),
            ("/r/pics/inner-link.txt".into(), false, 3),
            ("/r/pics/sub2".into(), true, 0),
            ("/r/pics/sub2/inner.txt".into(), false, 3),
        ]
    );
    assert_eq!(s.removed, 1 + 1, "old 지움 + a.txt 옛 자리");
    assert_eq!(s.moved, 1, "sub 폴더째 옮김");
    // 옮긴 뒤에도 FRN으로 찾힘(다음 변경을 이을 수 있음)
    let moved_inner = ix.lookup(Path::new("/r/pics/sub2/inner.txt")).unwrap();
    assert_eq!(ix.frn(moved_inner), inner);
    drop(ix);
    // 같은 변경을 한 번 더 반영해도 그대로(지금 상태를 다시 읽어 맞추므로)
    let before = listing(&index.read().unwrap());
    let s2 = mft::apply_changes(&index, &mut map, Path::new("/r"), &frns, &view, &opts);
    assert_eq!(listing(&index.read().unwrap()), before);
    assert_eq!(s2.added + s2.removed + s2.moved, 0);

    // 휴지통(제외)으로 옮긴 파일은 색인에서 빠짐
    view.0.lock().unwrap().files.insert(30, st(a, false, 20, &[(trash, "a-renamed.txt")]));
    mft::apply_changes(&index, &mut map, Path::new("/r"), &[a], &view, &opts);
    assert!(index.read().unwrap().lookup(Path::new("/r/pics/a-renamed.txt")).is_none());
    // 레코드가 다른 파일에 다시 쓰임(순번 2) → 옛 항목 지우고 새 이름
    view.0.lock().unwrap().files.insert(31, st(frn(31, 2), false, 1, &[(pics, "reused.txt")]));
    mft::apply_changes(&index, &mut map, Path::new("/r"), &[frn(31, 2)], &view, &opts);
    let ix = index.read().unwrap();
    assert!(ix.lookup(Path::new("/r/docs/new/b.txt")).is_none());
    assert!(ix.lookup(Path::new("/r/pics/reused.txt")).is_some());
}

#[test]
fn catch_up_detects_reset_and_wrap() {
    let mut fk = Fake { serial: 9, journal: JournalInfo { journal_id: 78, first_usn: 0, next_usn: 10 }, ..Default::default() };
    fk.log = vec![];
    let view = FakeView(Mutex::new(fk));
    let cur = UsnCursor { volume_serial: 9, journal_id: 77, next_usn: 5 };
    assert!(matches!(mft::catch_up(&view, &cur), Err(JournalErr::Reset(_))), "저널 ID가 다르면 다시 읽기");
    view.0.lock().unwrap().journal = JournalInfo { journal_id: 77, first_usn: 1000, next_usn: 2000 };
    assert!(matches!(mft::catch_up(&view, &cur), Err(JournalErr::Reset(_))), "저장한 위치가 지워졌으면 다시 읽기");
    let other = UsnCursor { volume_serial: 1, ..cur };
    assert!(matches!(mft::catch_up(&view, &other), Err(JournalErr::Reset(_))), "다른 볼륨");
    // 바뀐 것 없음
    view.0.lock().unwrap().journal = JournalInfo { journal_id: 77, first_usn: 0, next_usn: 5 };
    let (f, c, n) = mft::catch_up(&view, &cur).unwrap();
    assert!(f.is_empty() && n == 0 && c.next_usn == 5);
}

// ---------- 도우미 ----------

fn table_with_root() -> MftTable {
    let mut t = MftTable::with_records(64);
    t.add(5, &parse(&Rb::dir(5).a(si(FT, 0x16)).a(fname(frn(5, 5), 3, ".")).build()));
    t
}

fn tree(t: &MftTable, opts: &ScanOpts) -> Index {
    let ix = RwLock::new(Index::new());
    let rid = ix.write().unwrap().add_root(Path::new("/r"));
    let mut o = 0;
    build_tree(t, Path::new("/r"), rid, &ix, opts, &ScanControl::default(), &mut o);
    ix.into_inner().unwrap()
}

/// (경로, 폴더?, 크기) 이름순
fn listing(ix: &Index) -> Vec<(String, bool, u64)> {
    let mut v: Vec<(String, bool, u64)> = (0..ix.len() as u32)
        .filter(|&i| ix.is_live(i) && ix.parent(i) != u32::MAX)
        .map(|i| (ix.full_path(i).to_string_lossy().into_owned(), ix.is_dir(i), ix.size(i)))
        .collect();
    v.sort();
    v
}

// ---------- 진짜 NTFS 이미지 ----------

fn have(cmd: &str) -> Option<PathBuf> {
    for d in ["/usr/sbin", "/sbin", "/usr/bin", "/bin"] {
        let p = Path::new(d).join(cmd);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// mkntfs로 64 MB 이미지를 만들고 ntfs-3g(FUSE, sudo 불필요)로 채운 뒤, 이미지 파일의 $MFT를 직접 읽은 트리와
/// 마운트한 폴더를 훑은 트리를 비교한다(이름·종류·크기·수정 시각). 도구가 없으면 건너뜀.
#[cfg(unix)]
#[test]
fn real_ntfs_image_matches_walk() {
    let (Some(mkntfs), Some(ntfs3g)) = (have("mkntfs"), have("ntfs-3g")) else {
        eprintln!("mkntfs/ntfs-3g 없음 → 건너뜀");
        return;
    };
    let d = tempfile::tempdir().unwrap();
    let img = d.path().join("vol.img");
    let mnt = d.path().join("mnt");
    std::fs::create_dir(&mnt).unwrap();
    std::fs::File::create(&img).unwrap().set_len(64 << 20).unwrap();
    let ok = std::process::Command::new(&mkntfs).args(["-F", "-Q", "-q"]).arg(&img).stderr(std::process::Stdio::null()).status().unwrap();
    assert!(ok.success());
    let ok = std::process::Command::new(&ntfs3g).arg(&img).arg(&mnt).status().unwrap();
    if !ok.success() {
        eprintln!("ntfs-3g 마운트 실패(FUSE 없음?) → 건너뜀");
        return;
    }
    struct Unmount(PathBuf);
    impl Drop for Unmount {
        fn drop(&mut self) {
            let _ = std::process::Command::new("fusermount").arg("-u").arg(&self.0).status();
        }
    }
    let guard = Unmount(mnt.clone());
    // 채우기: 한글·NFD 이름, 깊은 폴더, 큰(비상주) 파일, 빈 파일, 하드 링크, 많은 하드 링크(속성 목록이 생기게), 지운 파일
    let m = &mnt;
    std::fs::create_dir_all(m.join("문서/2024/회의록")).unwrap();
    std::fs::create_dir_all(m.join("사진/여행")).unwrap();
    std::fs::create_dir_all(m.join("$Recycle.Bin/S-1-5-21")).unwrap();
    std::fs::write(m.join("$Recycle.Bin/S-1-5-21/버린 파일.txt"), b"x").unwrap();
    std::fs::write(m.join("문서/보고서.txt"), "보고서".repeat(10)).unwrap();
    std::fs::write(m.join("문서/2024/회의록/주간 회의.md"), b"# hi").unwrap();
    std::fs::write(m.join("사진/여행/".to_string() + &filetool_core::samples::nfd("바다.jpg")), vec![7u8; 300_000]).unwrap();
    std::fs::write(m.join("empty.bin"), b"").unwrap();
    std::fs::write(m.join("big.bin"), vec![1u8; 3_000_000]).unwrap();
    for i in 0..300 {
        std::fs::write(m.join(format!("문서/2024/f{i:03}.txt")), format!("{i}")).unwrap();
    }
    std::fs::hard_link(m.join("문서/보고서.txt"), m.join("사진/보고서-링크.txt")).unwrap();
    std::fs::create_dir(m.join("links")).unwrap();
    for i in 0..120 {
        std::fs::hard_link(m.join("big.bin"), m.join(format!("links/아주 긴 하드 링크 이름 번호 {i:03} — 속성 목록이 생기도록.bin"))).unwrap();
    }
    std::fs::write(m.join("지울 파일.txt"), b"bye").unwrap();
    std::fs::remove_file(m.join("지울 파일.txt")).unwrap();
    // 폴더 훑기(마운트) 결과
    let opts = ScanOpts { excludes: Excludes::new(&scan::default_excludes()), ..Default::default() };
    let walk = RwLock::new(Index::new());
    scan::scan_root_into(&DirWalkSource, m, &walk, &opts, &ScanControl::default());
    let walk = walk.into_inner().unwrap();
    drop(guard);
    // 이미지의 $MFT 직접 읽기
    let f = std::fs::File::open(&img).unwrap();
    let src = MftSource::new(Arc::new(f), None);
    let mi = RwLock::new(Index::new());
    let stm = scan::scan_root_into(&src, m, &mi, &opts, &ScanControl::default());
    let mi = mi.into_inner().unwrap();
    let info = src.last.lock().unwrap().clone().expect("MFT로 읽음");
    eprintln!("이미지 MFT: {info:?}, 통계 {stm:?}");
    assert_eq!(mi.roots()[0].source, SourceKind::Mft);
    assert!(info.extension_records > 0, "많은 하드 링크로 확장 레코드(속성 목록)가 생겼어야 함");
    let set = |ix: &Index| -> BTreeMap<String, (bool, u64, i64)> {
        (0..ix.len() as u32)
            .filter(|&i| ix.is_live(i) && ix.parent(i) != u32::MAX)
            .map(|i| (ix.full_path(i).to_string_lossy().into_owned(), (ix.is_dir(i), ix.size(i), ix.mtime(i))))
            .collect()
    };
    let (a, b) = (set(&walk), set(&mi));
    let only_walk: Vec<_> = a.keys().filter(|k| !b.contains_key(*k)).collect();
    let only_mft: Vec<_> = b.keys().filter(|k| !a.contains_key(*k)).collect();
    assert!(only_walk.is_empty() && only_mft.is_empty(), "훑기만 {only_walk:?} / MFT만 {only_mft:?}");
    let diff: Vec<_> = a.iter().filter(|(k, v)| b[*k] != **v).map(|(k, v)| (k, v, b[k])).collect();
    assert!(diff.is_empty(), "다른 값: {diff:?}");
    assert_eq!(a.len(), 3 + 2 + 5 + 300 + 1 + 1 + 120, "항목 수 {}", a.len());
    assert!(mi.lookup(&m.join("links")).is_some());
    assert!(mi.lookup(&m.join("지울 파일.txt")).is_none());
}
