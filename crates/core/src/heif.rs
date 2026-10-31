//! HEIF/HEIC 상자 구조 다루기.
//! - [`extract_exif`]: HEIC 안의 Exif 항목을 꺼낸다(ffmpeg는 이미지를 풀지만 EXIF는 옮기지 않으므로).
//! - [`build_grid_heic`]: 점검용으로 아이폰식 "grid"(타일을 이어 붙인) HEIC를 만든다.
//!   타일 HEVC 비트스트림은 ffmpeg(libx265)로 인코딩한 Annex-B 바이트를 받는다.

fn be16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn be_n(b: &[u8], o: usize, n: usize) -> Option<u64> {
    match n {
        0 => Some(0),
        4 => be32(b, o).map(u64::from),
        8 => Some(u64::from_be_bytes(b.get(o..o + 8)?.try_into().ok()?)),
        2 => be16(b, o).map(u64::from),
        _ => None,
    }
}

/// (type, 내용 시작, 내용 끝) 목록
fn boxes(b: &[u8], start: usize, end: usize) -> Vec<([u8; 4], usize, usize)> {
    let mut v = Vec::new();
    let mut i = start;
    while i + 8 <= end {
        let Some(size) = be32(b, i) else { break };
        let ty: [u8; 4] = b[i + 4..i + 8].try_into().unwrap();
        let (hdr, size) = match size {
            1 => match be_n(b, i + 8, 8) {
                Some(s) => (16, s as usize),
                None => break,
            },
            0 => (8, end - i),
            s => (8, s as usize),
        };
        if size < hdr || i + size > end {
            break;
        }
        v.push((ty, i + hdr, i + size));
        i += size;
    }
    v
}

/// HEIC가 맞는지(ftyp brand) 대략 확인.
pub fn is_heif(b: &[u8]) -> bool {
    b.get(4..8) == Some(b"ftyp")
        && b.get(8..12).map(|m| [b"heic", b"heix", b"mif1", b"msf1", b"heim", b"heis", b"hevc"].iter().any(|x| m == *x)).unwrap_or(false)
}

/// HEIF 메타 정보(필요한 만큼만): 항목 종류, 참조, 속성, 위치.
pub struct HeifInfo<'a> {
    b: &'a [u8],
    pub primary: Option<u32>,
    /// 항목 번호 → 종류(4cc)
    pub items: Vec<(u32, [u8; 4])>,
    /// (참조 종류, from, to 목록)
    pub refs: Vec<([u8; 4], u32, Vec<u32>)>,
    /// ipco 속성: (종류, 내용 시작, 끝)
    props: Vec<([u8; 4], usize, usize)>,
    /// 항목 → 속성 번호(1부터) 목록(순서 유지)
    assoc: Vec<(u32, Vec<u16>)>,
    /// 항목 → (construction_method, 조각들(절대 오프셋 또는 idat 기준, 길이))
    locs: Vec<(u32, u16, Vec<(usize, usize)>)>,
    idat: Option<(usize, usize)>,
}

/// grid 이미지 배치
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    /// 타일 항목 번호(왼쪽 위부터 가로 순서)
    pub tiles: Vec<u32>,
    pub rows: u32,
    pub cols: u32,
    pub out_w: u32,
    pub out_h: u32,
    /// 적용 순서대로: Rotate(반시계 90° 단위) 또는 Mirror(axis)
    pub transforms: Vec<Transform>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transform {
    /// 반시계 방향 90° × n
    Rotate(u8),
    /// 0: 세로 축 기준(좌우 뒤집기), 1: 가로 축 기준(상하 뒤집기)
    Mirror(u8),
}

impl<'a> HeifInfo<'a> {
    pub fn parse(b: &'a [u8]) -> Option<Self> {
        let meta = boxes(b, 0, b.len()).into_iter().find(|x| &x.0 == b"meta")?;
        let children = boxes(b, meta.1 + 4, meta.2);
        let mut info = HeifInfo { b, primary: None, items: vec![], refs: vec![], props: vec![], assoc: vec![], locs: vec![], idat: None };
        for (ty, s, e) in children {
            match &ty {
                b"pitm" => {
                    info.primary = if b[s] == 0 { be16(b, s + 4).map(u32::from) } else { be32(b, s + 4) };
                }
                b"iinf" => {
                    let first = s + 4 + if b[s] == 0 { 2 } else { 4 };
                    for (t2, s2, _) in boxes(b, first, e) {
                        if &t2 != b"infe" {
                            continue;
                        }
                        let (id, ty_at) = match b[s2] {
                            2 => (be16(b, s2 + 4)? as u32, s2 + 8),
                            3 => (be32(b, s2 + 4)?, s2 + 10),
                            _ => continue,
                        };
                        info.items.push((id, b.get(ty_at..ty_at + 4)?.try_into().ok()?));
                    }
                }
                b"iref" => {
                    let v = b[s];
                    for (rt, s2, _) in boxes(b, s + 4, e) {
                        let rd = |o: usize| if v == 0 { be16(b, o).map(u32::from) } else { be32(b, o) };
                        let w = if v == 0 { 2 } else { 4 };
                        let from = rd(s2)?;
                        let n = be16(b, s2 + w)? as usize;
                        let to = (0..n).map(|i| rd(s2 + w + 2 + i * w)).collect::<Option<Vec<_>>>()?;
                        info.refs.push((rt, from, to));
                    }
                }
                b"iprp" => {
                    for (t2, s2, e2) in boxes(b, s, e) {
                        if &t2 == b"ipco" {
                            info.props = boxes(b, s2, e2);
                        } else if &t2 == b"ipma" {
                            let v = b[s2];
                            let flags = b[s2 + 3] & 1;
                            let mut p = s2 + 4;
                            let n = be32(b, p)? as usize;
                            p += 4;
                            for _ in 0..n {
                                let id = if v < 1 { let i = be16(b, p)? as u32; p += 2; i } else { let i = be32(b, p)?; p += 4; i };
                                let k = *b.get(p)? as usize;
                                p += 1;
                                let mut list = Vec::new();
                                for _ in 0..k {
                                    let idx = if flags == 1 { let x = be16(b, p)? & 0x7fff; p += 2; x } else { let x = (*b.get(p)? & 0x7f) as u16; p += 1; x };
                                    list.push(idx);
                                }
                                info.assoc.push((id, list));
                            }
                        }
                    }
                }
                b"idat" => info.idat = Some((s, e)),
                b"iloc" => {
                    let v = b[s];
                    let mut p = s + 4;
                    let off_size = (b[p] >> 4) as usize;
                    let len_size = (b[p] & 15) as usize;
                    let base_size = (b[p + 1] >> 4) as usize;
                    let idx_size = if v >= 1 { (b[p + 1] & 15) as usize } else { 0 };
                    p += 2;
                    let count = if v < 2 { let c = be16(b, p)? as usize; p += 2; c } else { let c = be32(b, p)? as usize; p += 4; c };
                    for _ in 0..count {
                        let id = if v < 2 { let i = be16(b, p)? as u32; p += 2; i } else { let i = be32(b, p)?; p += 4; i };
                        let method = if v >= 1 { let m = be16(b, p)? & 15; p += 2; m } else { 0 };
                        p += 2;
                        let base = be_n(b, p, base_size)? as usize;
                        p += base_size;
                        let n_ext = be16(b, p)? as usize;
                        p += 2;
                        let mut ext = Vec::new();
                        for _ in 0..n_ext {
                            p += idx_size;
                            let off = be_n(b, p, off_size)? as usize;
                            p += off_size;
                            let len = be_n(b, p, len_size)? as usize;
                            p += len_size;
                            ext.push((base + off, len));
                        }
                        info.locs.push((id, method, ext));
                    }
                }
                _ => {}
            }
        }
        Some(info)
    }

    pub fn item_type(&self, id: u32) -> Option<[u8; 4]> {
        self.items.iter().find(|x| x.0 == id).map(|x| x.1)
    }

    /// 항목 데이터(mdat의 파일 오프셋 또는 idat 안).
    pub fn item_data(&self, id: u32) -> Option<Vec<u8>> {
        let (_, method, ext) = self.locs.iter().find(|x| x.0 == id)?;
        let mut v = Vec::new();
        for &(off, len) in ext {
            let start = match method {
                0 => off,
                1 => self.idat?.0 + off,
                _ => return None,
            };
            let end = if *method == 1 { (start + len).min(self.idat?.1) } else { start + len };
            v.extend_from_slice(self.b.get(start..end)?);
        }
        Some(v)
    }

    fn props_of(&self, id: u32) -> Vec<([u8; 4], &'a [u8])> {
        let Some((_, list)) = self.assoc.iter().find(|x| x.0 == id) else { return vec![] };
        list.iter()
            .filter_map(|&i| self.props.get((i as usize).checked_sub(1)?))
            .map(|&(t, s, e)| (t, &self.b[s..e]))
            .collect()
    }

    /// 대표 이미지가 grid이면 배치 정보.
    pub fn grid(&self) -> Option<Grid> {
        let id = self.primary?;
        if &self.item_type(id)? != b"grid" {
            return None;
        }
        let d = self.item_data(id)?;
        let large = d.get(1)? & 1 == 1;
        let rows = *d.get(2)? as u32 + 1;
        let cols = *d.get(3)? as u32 + 1;
        let (out_w, out_h) = if large { (be32(&d, 4)?, be32(&d, 8)?) } else { (be16(&d, 4)? as u32, be16(&d, 6)? as u32) };
        let tiles = self.refs.iter().find(|r| &r.0 == b"dimg" && r.1 == id)?.2.clone();
        if tiles.len() != (rows * cols) as usize {
            return None;
        }
        let transforms = self
            .props_of(id)
            .into_iter()
            .filter_map(|(t, c)| match &t {
                b"irot" => Some(Transform::Rotate(c.first()? & 3)),
                b"imir" => Some(Transform::Mirror(c.first()? & 1)),
                _ => None,
            })
            .collect();
        Some(Grid { tiles, rows, cols, out_w, out_h, transforms })
    }

    /// Exif 항목의 TIFF 블록.
    pub fn exif(&self) -> Option<Vec<u8>> {
        let id = self.items.iter().find(|x| &x.1 == b"Exif")?.0;
        let data = self.item_data(id)?;
        let skip = be32(&data, 0)? as usize;
        data.get(4 + skip..).map(<[u8]>::to_vec)
    }
}

/// Exif 항목의 TIFF 블록. 없거나 구조를 읽을 수 없으면 None.
pub fn extract_exif(b: &[u8]) -> Option<Vec<u8>> {
    HeifInfo::parse(b)?.exif()
}

// ---------- 점검용 HEIC 만들기 ----------

fn bx(ty: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 8);
    v.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    v.extend_from_slice(ty);
    v.extend_from_slice(body);
    v
}
fn full(ty: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![version, (flags >> 16) as u8, (flags >> 8) as u8, flags as u8];
    b.extend_from_slice(body);
    bx(ty, &b)
}

/// Annex-B(00 00 01 시작 코드) → NAL 목록
pub fn split_annexb(b: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= b.len() {
        if b[i] == 0 && b[i + 1] == 0 && b[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::new();
    for (k, &s) in starts.iter().enumerate() {
        let mut e = starts.get(k + 1).map(|n| n - 3).unwrap_or(b.len());
        while e > s && b[e - 1] == 0 {
            e -= 1; // 다음 4바이트 시작 코드의 0
        }
        nals.push(&b[s..e]);
    }
    nals
}

fn nal_type(n: &[u8]) -> u8 {
    (n[0] >> 1) & 0x3f
}

fn unescape(n: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(n.len());
    let mut zeros = 0;
    for &c in n {
        if zeros >= 2 && c == 3 {
            zeros = 0;
            continue;
        }
        zeros = if c == 0 { zeros + 1 } else { 0 };
        v.push(c);
    }
    v
}

/// 타일 하나(Annex-B) → (hvcC 내용, 길이 접두 VCL 데이터)
fn tile_parts(annexb: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let nals = split_annexb(annexb);
    let vps = nals.iter().find(|n| nal_type(n) == 32)?;
    let sps = nals.iter().find(|n| nal_type(n) == 33)?;
    let pps = nals.iter().find(|n| nal_type(n) == 34)?;
    let rbsp = unescape(&sps[2..]);
    let ptl = rbsp.get(1..13)?; // profile_space/tier/idc(1) + compat(4) + constraint(6) + level(1)
    let mut c = vec![1u8];
    c.extend_from_slice(ptl);
    c.extend_from_slice(&[0xF0, 0x00, 0xFC, 0xFD, 0xF8, 0xF8, 0x00, 0x00, 0x0F, 3]);
    for n in [vps, sps, pps] {
        c.push(0x80 | nal_type(n));
        c.extend_from_slice(&1u16.to_be_bytes());
        c.extend_from_slice(&(n.len() as u16).to_be_bytes());
        c.extend_from_slice(n);
    }
    let mut data = Vec::new();
    for n in nals.iter().filter(|n| nal_type(n) < 32) {
        data.extend_from_slice(&(n.len() as u32).to_be_bytes());
        data.extend_from_slice(n);
    }
    Some((c, data))
}

/// `cols × rows` 타일(왼쪽 위부터 가로 순서)로 된 grid HEIC.
/// `irot`: 반시계 방향 90° 단위 회전(0~3, 아이폰 세로 사진은 보통 EXIF 6 + irot). `exif`: TIFF 블록.
pub fn build_grid_heic(
    tiles: &[Vec<u8>],
    tile_w: u32,
    tile_h: u32,
    cols: u8,
    rows: u8,
    out_w: u16,
    out_h: u16,
    irot: u8,
    exif: Option<&[u8]>,
) -> Option<Vec<u8>> {
    let n = tiles.len();
    if n != cols as usize * rows as usize || n == 0 {
        return None;
    }
    let parts: Vec<(Vec<u8>, Vec<u8>)> = tiles.iter().map(|t| tile_parts(t)).collect::<Option<_>>()?;
    let grid_id = n as u16 + 1;
    let exif_id = n as u16 + 2;
    let grid_data = {
        let mut g = vec![0u8, 0, rows - 1, cols - 1];
        g.extend_from_slice(&out_w.to_be_bytes());
        g.extend_from_slice(&out_h.to_be_bytes());
        g
    };
    let exif_data = exif.map(|e| {
        let mut d = 0u32.to_be_bytes().to_vec();
        d.extend_from_slice(e);
        d
    });
    // mdat 안 배치
    // mdat: 타일, Exif / idat: grid 설명(아이폰과 같은 배치)
    let mut payloads: Vec<(u16, &[u8])> = parts.iter().enumerate().map(|(i, p)| (i as u16 + 1, p.1.as_slice())).collect();
    if let Some(e) = &exif_data {
        payloads.push((exif_id, e));
    }
    let mut all_ids: Vec<u16> = payloads.iter().map(|p| p.0).collect();
    all_ids.push(grid_id);
    all_ids.sort();

    let ftyp = bx(b"ftyp", b"heic\0\0\0\0mif1heicmiaf");
    let build_meta = |mdat_start: u32| -> Vec<u8> {
        let hdlr = full(b"hdlr", 0, 0, b"\0\0\0\0pict\0\0\0\0\0\0\0\0\0\0\0\0\0");
        let pitm = full(b"pitm", 0, 0, &grid_id.to_be_bytes());
        let mut iinf_body = (all_ids.len() as u16).to_be_bytes().to_vec();
        for id in &all_ids {
            let (ty, flags) = if *id == grid_id { (b"grid", 0) } else if *id == exif_id { (b"Exif", 1) } else { (b"hvc1", 1) };
            let mut e = id.to_be_bytes().to_vec();
            e.extend_from_slice(&[0, 0]);
            e.extend_from_slice(ty);
            e.push(0);
            iinf_body.extend(full(b"infe", 2, flags, &e));
        }
        let iinf = full(b"iinf", 0, 0, &iinf_body);
        let mut dimg = grid_id.to_be_bytes().to_vec();
        dimg.extend_from_slice(&(n as u16).to_be_bytes());
        for i in 1..=n as u16 {
            dimg.extend_from_slice(&i.to_be_bytes());
        }
        let mut iref_body = bx(b"dimg", &dimg);
        if exif_data.is_some() {
            let mut c = exif_id.to_be_bytes().to_vec();
            c.extend_from_slice(&1u16.to_be_bytes());
            c.extend_from_slice(&grid_id.to_be_bytes());
            iref_body.extend(bx(b"cdsc", &c));
        }
        let iref = full(b"iref", 0, 0, &iref_body);
        // ipco: [hvcC×n][ispe 타일][ispe 전체][irot]
        let mut ipco = Vec::new();
        for p in &parts {
            ipco.extend(bx(b"hvcC", &p.0));
        }
        let ispe = |w: u32, h: u32| {
            let mut b = w.to_be_bytes().to_vec();
            b.extend_from_slice(&h.to_be_bytes());
            full(b"ispe", 0, 0, &b)
        };
        ipco.extend(ispe(tile_w, tile_h));
        ipco.extend(ispe(out_w as u32, out_h as u32));
        ipco.extend(bx(b"irot", &[irot & 3]));
        let ispe_tile = n as u8 + 1;
        let ispe_grid = n as u8 + 2;
        let irot_idx = n as u8 + 3;
        let mut ipma = ((n + 1) as u32).to_be_bytes().to_vec();
        for i in 0..n {
            ipma.extend_from_slice(&(i as u16 + 1).to_be_bytes());
            ipma.extend_from_slice(&[2, 0x80 | (i as u8 + 1), ispe_tile]);
        }
        ipma.extend_from_slice(&grid_id.to_be_bytes());
        ipma.extend_from_slice(&[2, ispe_grid, 0x80 | irot_idx]);
        let iprp = bx(b"iprp", &[bx(b"ipco", &ipco), full(b"ipma", 0, 0, &ipma)].concat());
        // iloc v1: construction_method 0(파일 오프셋) / 1(idat)
        let mut iloc_body = vec![0x44, 0x00];
        iloc_body.extend_from_slice(&((payloads.len() + 1) as u16).to_be_bytes());
        let mut off = mdat_start + 8;
        for (id, d) in &payloads {
            iloc_body.extend_from_slice(&id.to_be_bytes());
            iloc_body.extend_from_slice(&[0, 0, 0, 0, 0, 1]);
            iloc_body.extend_from_slice(&off.to_be_bytes());
            iloc_body.extend_from_slice(&(d.len() as u32).to_be_bytes());
            off += d.len() as u32;
        }
        iloc_body.extend_from_slice(&grid_id.to_be_bytes());
        iloc_body.extend_from_slice(&[0, 1, 0, 0, 0, 1]);
        iloc_body.extend_from_slice(&0u32.to_be_bytes());
        iloc_body.extend_from_slice(&(grid_data.len() as u32).to_be_bytes());
        let iloc = full(b"iloc", 1, 0, &iloc_body);
        let idat = bx(b"idat", &grid_data);
        full(b"meta", 0, 0, &[hdlr, pitm, iinf, iref, iprp, idat, iloc].concat())
    };
    let meta_len = build_meta(0).len();
    let meta = build_meta((ftyp.len() + meta_len) as u32);
    let mdat_body: Vec<u8> = payloads.iter().flat_map(|(_, d)| d.iter().copied()).collect();
    Some([ftyp, meta, bx(b"mdat", &mdat_body)].concat())
}
