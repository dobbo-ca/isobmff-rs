//! A small HEIF/AVIF byte builder for the integration tests, and the sample files built with it.
//! Not every test file uses every item.
#![allow(dead_code)]

pub const ICC: &[u8] = b"fake-icc-profile";
/// A 3-byte "VPS" NAL unit inside the `hvcC` of every HEVC sample.
pub const VPS: [u8; 3] = [0x40, 0x01, 0x02];
/// A TIFF header (big endian, IFD at 8): the payload of an Exif item after its offset header.
pub const TIFF: [u8; 8] = *b"MM\0*\0\0\0\x08";
pub const XMP: &[u8] = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>";
pub const XMP_TYPE: &str = "application/rdf+xml";
pub const ALPHA: &str = "urn:mpeg:mpegB:cicp:systems:auxiliary:alpha";
pub const DEPTH: &str = "urn:mpeg:mpegB:cicp:systems:auxiliary:depth";
pub const APPLE_GAIN_MAP: &str = "urn:com:apple:photo:2020:aux:hdrgainmap";

// ---- boxes ----

pub fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

pub fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = u32::try_from(body.len() + 8)
        .unwrap()
        .to_be_bytes()
        .to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

pub fn fbx(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = vec![version];
    b.extend_from_slice(&flags.to_be_bytes()[1..]);
    b.extend_from_slice(body);
    bx(kind, &b)
}

/// How a top-level box states its size.
#[derive(Debug, Clone, Copy)]
pub enum Style {
    Normal,
    /// `size == 1` and a 64-bit largesize.
    Large,
    /// `size == 0`: the box runs to the end of the file.
    ToEnd,
}

pub fn wrap(kind: &[u8; 4], body: &[u8], style: Style) -> Vec<u8> {
    match style {
        Style::Normal => bx(kind, body),
        Style::Large => {
            let mut v = 1u32.to_be_bytes().to_vec();
            v.extend_from_slice(kind);
            v.extend_from_slice(&((body.len() + 16) as u64).to_be_bytes());
            v.extend_from_slice(body);
            v
        }
        Style::ToEnd => {
            let mut v = 0u32.to_be_bytes().to_vec();
            v.extend_from_slice(kind);
            v.extend_from_slice(body);
            v
        }
    }
}

fn header_len(style: Style) -> usize {
    match style {
        Style::Large => 16,
        _ => 8,
    }
}

// ---- properties ----

pub fn ispe(w: u32, h: u32) -> Vec<u8> {
    fbx(b"ispe", 0, 0, &cat(&[&w.to_be_bytes(), &h.to_be_bytes()]))
}

/// `hvcC`: 4:2:0, 8 bit, one array with the [`VPS`] NAL unit.
pub fn hvcc() -> Vec<u8> {
    let mut b = vec![
        1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 93, 0xF0, 0, 0xFC, 0xFD, 0xF8, 0xF8, 0, 0, 3, 1, 0xA0,
        0, 1, 0, 3,
    ];
    b.extend_from_slice(&VPS);
    bx(b"hvcC", &b)
}

/// `av1C`: 10 bit 4:2:0, level 12, config OBUs `[0x0a, 0x00]`.
pub fn av1c() -> Vec<u8> {
    bx(b"av1C", &[0x81, 0x0c, 0x4c, 0x00, 0x0a, 0x00])
}

pub fn colr_nclx(primaries: u16, transfer: u16, matrix: u16, full: bool) -> Vec<u8> {
    let range = if full { 0x80 } else { 0 };
    bx(
        b"colr",
        &cat(&[
            b"nclx",
            &primaries.to_be_bytes(),
            &transfer.to_be_bytes(),
            &matrix.to_be_bytes(),
            &[range],
        ]),
    )
}

pub fn colr_icc(profile: &[u8]) -> Vec<u8> {
    bx(b"colr", &cat(&[b"prof", profile]))
}

pub fn irot(code: u8) -> Vec<u8> {
    bx(b"irot", &[code])
}

pub fn imir(axis: u8) -> Vec<u8> {
    bx(b"imir", &[axis])
}

pub fn pixi(bits: &[u8]) -> Vec<u8> {
    fbx(b"pixi", 0, 0, &cat(&[&[bits.len() as u8], bits]))
}

pub fn pasp(h: u32, v: u32) -> Vec<u8> {
    bx(b"pasp", &cat(&[&h.to_be_bytes(), &v.to_be_bytes()]))
}

pub fn auxc(aux_type: &str) -> Vec<u8> {
    fbx(b"auxC", 0, 0, &cat(&[aux_type.as_bytes(), &[0]]))
}

/// `clap` from (numerator, denominator) pairs.
pub fn clap(w: (u32, u32), h: (u32, u32), horiz: (i32, u32), vert: (i32, u32)) -> Vec<u8> {
    bx(
        b"clap",
        &cat(&[
            &w.0.to_be_bytes(),
            &w.1.to_be_bytes(),
            &h.0.to_be_bytes(),
            &h.1.to_be_bytes(),
            &horiz.0.to_be_bytes(),
            &horiz.1.to_be_bytes(),
            &vert.0.to_be_bytes(),
            &vert.1.to_be_bytes(),
        ]),
    )
}

/// A property this crate does not know.
pub fn unknown(kind: &[u8; 4]) -> Vec<u8> {
    bx(kind, &[1, 2, 3])
}

// ---- the file builder ----

/// The widths of the `iloc` fields (each 0, 4 or 8) and its version.
#[derive(Debug, Clone, Copy)]
pub struct IlocFmt {
    pub version: u8,
    pub offset_size: u8,
    pub length_size: u8,
    pub base_size: u8,
    pub index_size: u8,
}

pub const ILOC_FORMATS: [IlocFmt; 6] = [
    IlocFmt {
        version: 0,
        offset_size: 4,
        length_size: 4,
        base_size: 0,
        index_size: 0,
    },
    IlocFmt {
        version: 0,
        offset_size: 8,
        length_size: 8,
        base_size: 8,
        index_size: 0,
    },
    IlocFmt {
        version: 1,
        offset_size: 4,
        length_size: 4,
        base_size: 4,
        index_size: 0,
    },
    IlocFmt {
        version: 1,
        offset_size: 8,
        length_size: 4,
        base_size: 8,
        index_size: 4,
    },
    IlocFmt {
        version: 2,
        offset_size: 4,
        length_size: 8,
        base_size: 0,
        index_size: 0,
    },
    IlocFmt {
        version: 2,
        offset_size: 8,
        length_size: 8,
        base_size: 8,
        index_size: 8,
    },
];

#[derive(Debug, Clone)]
pub struct ItemSpec {
    pub id: u32,
    pub kind: [u8; 4],
    pub mime: Option<&'static str>,
    pub hidden: bool,
    /// The item's bytes; `None`: no `iloc` entry.
    pub data: Option<Vec<u8>>,
    /// `iloc` construction method: 0 file (`mdat`), 1 `idat`, 2 item offset (data goes to `idat`).
    pub method: u8,
    /// Raw `iloc` extents `(extent_index, offset, length)` written as they are, with base offset 0,
    /// in place of the extent that `data` would give (the item has an `iloc` entry even with no
    /// `data`). Offsets are relative to the start of `idat` for method 1.
    pub raw: Option<Vec<(u64, u64, u64)>>,
    /// `(essential, 1-based property index)`, in `ipma` order.
    pub assoc: Vec<(bool, u16)>,
}

impl ItemSpec {
    pub fn data(&mut self, d: &[u8]) -> &mut Self {
        self.data = Some(d.to_vec());
        self
    }

    pub fn assoc(&mut self, idx: &[u16]) -> &mut Self {
        self.assoc.extend(idx.iter().map(|&i| (false, i)));
        self
    }

    pub fn essential(&mut self, idx: u16) -> &mut Self {
        self.assoc.push((true, idx));
        self
    }

    pub fn hidden(&mut self) -> &mut Self {
        self.hidden = true;
        self
    }

    /// Makes the item a `mime` item of this content type.
    pub fn mime(&mut self, content_type: &'static str) -> &mut Self {
        self.kind = *b"mime";
        self.mime = Some(content_type);
        self
    }

    /// Sets raw `iloc` extents `(extent_index, offset, length)` (see [`ItemSpec::raw`]).
    pub fn raw(&mut self, extents: &[(u64, u64, u64)]) -> &mut Self {
        self.raw = Some(extents.to_vec());
        self
    }

    pub fn idat(&mut self) -> &mut Self {
        self.method = 1;
        self
    }

    pub fn method(&mut self, m: u8) -> &mut Self {
        self.method = m;
        self
    }
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub major: [u8; 4],
    pub primary: u32,
    pub items: Vec<ItemSpec>,
    /// Property boxes; the index of the first is 1.
    pub props: Vec<Vec<u8>>,
    /// `iref` boxes: (type, from, to).
    pub refs: Vec<([u8; 4], u32, Vec<u32>)>,
    /// `grpl` boxes: (type, group id, entities).
    pub groups: Vec<([u8; 4], u32, Vec<u32>)>,
    pub iloc: IlocFmt,
    pub meta_style: Style,
    pub mdat_style: Style,
    /// Zero bytes at the start of `mdat`, before the first item.
    pub pad: usize,
    /// Boxes after `mdat`.
    pub tail: Vec<u8>,
    /// Bytes appended to the `iprp` box after `ipco` and `ipma`.
    pub iprp_extra: Vec<u8>,
}

fn uint(b: &mut Vec<u8>, size: u8, v: u64) {
    match size {
        0 => {}
        4 => b.extend_from_slice(&u32::try_from(v).unwrap().to_be_bytes()),
        8 => b.extend_from_slice(&v.to_be_bytes()),
        _ => panic!("bad field size {size}"),
    }
}

impl Spec {
    pub fn new(major: &[u8; 4]) -> Self {
        Self {
            major: *major,
            primary: 1,
            items: Vec::new(),
            props: Vec::new(),
            refs: Vec::new(),
            groups: Vec::new(),
            iloc: ILOC_FORMATS[0],
            meta_style: Style::Normal,
            mdat_style: Style::Normal,
            pad: 0,
            tail: Vec::new(),
            iprp_extra: Vec::new(),
        }
    }

    /// Adds a property box and returns its 1-based index.
    pub fn prop(&mut self, p: Vec<u8>) -> u16 {
        self.props.push(p);
        self.props.len() as u16
    }

    pub fn item(&mut self, id: u32, kind: &[u8; 4]) -> &mut ItemSpec {
        self.items.push(ItemSpec {
            id,
            kind: *kind,
            mime: None,
            hidden: false,
            data: None,
            raw: None,
            method: 0,
            assoc: Vec::new(),
        });
        self.items.last_mut().unwrap()
    }

    /// An `hvc1` item with 4 data bytes (its id), `ispe` and (essential) `hvcC`.
    pub fn hevc(&mut self, id: u32, ispe: u16, hvcc: u16) -> &mut ItemSpec {
        self.item(id, b"hvc1")
            .data(&[id as u8; 4])
            .assoc(&[ispe])
            .essential(hvcc)
    }

    /// An `av01` item with 4 data bytes (its id), `ispe` and (essential) `av1C`.
    pub fn av1(&mut self, id: u32, ispe: u16, av1c: u16) -> &mut ItemSpec {
        self.item(id, b"av01")
            .data(&[id as u8; 4])
            .assoc(&[ispe])
            .essential(av1c)
    }

    pub fn link(&mut self, kind: &[u8; 4], from: u32, to: &[u32]) -> &mut Self {
        self.refs.push((*kind, from, to.to_vec()));
        self
    }

    pub fn group(&mut self, kind: &[u8; 4], id: u32, entities: &[u32]) -> &mut Self {
        self.groups.push((*kind, id, entities.to_vec()));
        self
    }

    /// `mdat` payload, `idat` payload and, per item, `(offset in its payload, length)`.
    #[allow(clippy::type_complexity)]
    fn layout(&self) -> (Vec<u8>, Vec<u8>, Vec<Option<(u64, u64)>>) {
        let mut mdat = vec![0u8; self.pad];
        let mut idat = Vec::new();
        let mut at = Vec::new();
        for it in &self.items {
            if it.raw.is_some() {
                at.push(Some((0, 0)));
                continue;
            }
            at.push(it.data.as_ref().map(|d| {
                let buf = if it.method == 0 { &mut mdat } else { &mut idat };
                let rel = buf.len() as u64;
                buf.extend_from_slice(d);
                (rel, d.len() as u64)
            }));
        }
        (mdat, idat, at)
    }

    fn infe(it: &ItemSpec) -> Vec<u8> {
        let (version, mut b) = if it.id > 0xffff {
            (3, it.id.to_be_bytes().to_vec())
        } else {
            (2, (it.id as u16).to_be_bytes().to_vec())
        };
        b.extend_from_slice(&[0, 0]); // item_protection_index
        b.extend_from_slice(&it.kind);
        b.push(0); // item_name
        if let Some(t) = it.mime {
            b.extend_from_slice(t.as_bytes());
            b.extend_from_slice(&[0, 0]); // end of content_type, empty content_encoding
        }
        fbx(b"infe", version, u32::from(it.hidden), &b)
    }

    fn iloc_box(&self, mdat_at: u64, at: &[Option<(u64, u64)>]) -> Vec<u8> {
        let f = self.iloc;
        let mut b = vec![
            (f.offset_size << 4) | f.length_size,
            (f.base_size << 4) | f.index_size,
        ];
        let entries: Vec<_> = self
            .items
            .iter()
            .zip(at.iter().copied())
            .filter_map(|(it, a)| a.map(|a| (it, a)))
            .collect();
        if f.version == 2 {
            b.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        } else {
            b.extend_from_slice(&(entries.len() as u16).to_be_bytes());
        }
        for (it, (rel, len)) in entries {
            if f.version == 2 {
                b.extend_from_slice(&it.id.to_be_bytes());
            } else {
                b.extend_from_slice(&(it.id as u16).to_be_bytes());
            }
            if f.version >= 1 {
                b.extend_from_slice(&u16::from(it.method).to_be_bytes());
            }
            b.extend_from_slice(&[0, 0]); // data_reference_index
            let origin = if it.method == 0 { mdat_at } else { 0 };
            let (base, off) = if f.base_size > 0 {
                (origin, rel)
            } else {
                (0, origin + rel)
            };
            uint(&mut b, f.base_size, if it.raw.is_some() { 0 } else { base });
            if let Some(raw) = &it.raw {
                b.extend_from_slice(&(raw.len() as u16).to_be_bytes());
                for &(index, offset, length) in raw {
                    uint(&mut b, f.index_size, index);
                    uint(&mut b, f.offset_size, offset);
                    uint(&mut b, f.length_size, length);
                }
                continue;
            }
            b.extend_from_slice(&1u16.to_be_bytes()); // one extent
            uint(&mut b, f.index_size, 0);
            uint(&mut b, f.offset_size, off);
            uint(&mut b, f.length_size, len);
        }
        fbx(b"iloc", f.version, 0, &b)
    }

    fn meta_children(&self, mdat_at: u64, at: &[Option<(u64, u64)>], idat: &[u8]) -> Vec<u8> {
        let mut out = fbx(b"hdlr", 0, 0, &cat(&[&[0; 4], b"pict", &[0; 13]]));
        out.extend_from_slice(&if self.primary > 0xffff {
            fbx(b"pitm", 1, 0, &self.primary.to_be_bytes())
        } else {
            fbx(b"pitm", 0, 0, &(self.primary as u16).to_be_bytes())
        });
        let infes: Vec<u8> = self.items.iter().flat_map(Self::infe).collect();
        out.extend_from_slice(&fbx(
            b"iinf",
            0,
            0,
            &cat(&[&(self.items.len() as u16).to_be_bytes(), &infes]),
        ));
        out.extend_from_slice(&self.iloc_box(mdat_at, at));
        if !idat.is_empty() {
            out.extend_from_slice(&bx(b"idat", idat));
        }
        if !self.refs.is_empty() {
            let mut kids = Vec::new();
            for (kind, from, to) in &self.refs {
                let mut b = (*from as u16).to_be_bytes().to_vec();
                b.extend_from_slice(&(to.len() as u16).to_be_bytes());
                for t in to {
                    b.extend_from_slice(&(*t as u16).to_be_bytes());
                }
                kids.extend_from_slice(&bx(kind, &b));
            }
            out.extend_from_slice(&fbx(b"iref", 0, 0, &kids));
        }
        let with: Vec<&ItemSpec> = self.items.iter().filter(|i| !i.assoc.is_empty()).collect();
        let mut ipma = (with.len() as u32).to_be_bytes().to_vec();
        for it in with {
            ipma.extend_from_slice(&(it.id as u16).to_be_bytes());
            ipma.push(it.assoc.len() as u8);
            for &(essential, idx) in &it.assoc {
                assert!(idx < 128, "ipma index needs the 15-bit form");
                let flag = if essential { 0x80 } else { 0 };
                ipma.push(idx as u8 | flag);
            }
        }
        let ipco = bx(b"ipco", &self.props.concat());
        let ipma = fbx(b"ipma", 0, 0, &ipma);
        out.extend_from_slice(&bx(b"iprp", &cat(&[&ipco, &ipma, &self.iprp_extra])));
        if !self.groups.is_empty() {
            let mut kids = Vec::new();
            for (kind, id, entities) in &self.groups {
                let mut b = id.to_be_bytes().to_vec();
                b.extend_from_slice(&(entities.len() as u32).to_be_bytes());
                for e in entities {
                    b.extend_from_slice(&e.to_be_bytes());
                }
                kids.extend_from_slice(&fbx(kind, 0, 0, &b));
            }
            out.extend_from_slice(&bx(b"grpl", &kids));
        }
        out
    }

    /// `ftyp`, `meta`, `mdat`, then `tail`.
    pub fn build(&self) -> Vec<u8> {
        let f = self.iloc;
        assert!(
            f.version >= 1 || (f.index_size == 0 && self.items.iter().all(|i| i.method == 0)),
            "iloc v0 has no construction method and no extent index"
        );
        let ftyp = bx(b"ftyp", &cat(&[&self.major, &[0; 4], b"mif1", &self.major]));
        let (mdat, idat, at) = self.layout();
        let meta = |mdat_at: u64| {
            let kids = self.meta_children(mdat_at, &at, &idat);
            wrap(b"meta", &cat(&[&[0; 4], &kids]), self.meta_style)
        };
        let mdat_at = (ftyp.len() + meta(0).len() + header_len(self.mdat_style)) as u64;
        let mut out = ftyp;
        out.extend_from_slice(&meta(mdat_at));
        out.extend_from_slice(&wrap(b"mdat", &mdat, self.mdat_style));
        out.extend_from_slice(&self.tail);
        out
    }
}

// ---- sample files ----

/// An Exif item payload: the offset header, `offset` filler bytes, then [`TIFF`].
pub fn exif_item(offset: u32) -> Vec<u8> {
    cat(&[&offset.to_be_bytes(), &vec![0xAA; offset as usize], &TIFF])
}

/// The payload of a `tmap` item: one channel, shared denominator 1000, headroom 0 to 3, gain
/// 0 to 2 (all in 1/1000).
pub fn tmap_payload() -> Vec<u8> {
    // version, minimum_version, writer_version, flags (bit 3: common denominator)
    let mut b = vec![0, 0, 0, 0, 0, 0x08];
    // denominator, base headroom, alternate headroom, min, max, gamma, base offset, alt offset
    for v in [1000u32, 0, 3000, 0, 2000, 1000, 15, 25] {
        b.extend_from_slice(&v.to_be_bytes());
    }
    b
}

/// One `hvc1` image: `hvcC`, `ispe` 64 x 48, `colr` nclx and ICC, `pixi`, `pasp`.
pub fn single() -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    let size = s.prop(ispe(64, 48));
    let cfg = s.prop(hvcc());
    let nclx = s.prop(colr_nclx(1, 13, 6, true));
    let icc = s.prop(colr_icc(ICC));
    let bits = s.prop(pixi(&[8, 8, 8]));
    let aspect = s.prop(pasp(1, 1));
    s.item(1, b"hvc1")
        .data(&[1, 2, 3, 4])
        .assoc(&[size])
        .essential(cfg)
        .assoc(&[nclx, icc, bits, aspect]);
    s.build()
}

/// One `av01` image, 64 x 48, 10 bit 4:2:0, `colr` nclx.
pub fn avif() -> Vec<u8> {
    let mut s = Spec::new(b"avif");
    let size = s.prop(ispe(64, 48));
    let cfg = s.prop(av1c());
    let nclx = s.prop(colr_nclx(1, 13, 6, false));
    s.av1(1, size, cfg).assoc(&[nclx]);
    s.build()
}

/// An iPhone-style grid: 8 x 6 tiles of 512 (item 1 = grid, items 2..=49 = hidden tiles).
pub fn grid_spec() -> Spec {
    let mut s = Spec::new(b"heic");
    let tile = s.prop(ispe(512, 512));
    let cfg = s.prop(hvcc());
    let full = s.prop(ispe(4096, 3072));
    s.item(1, b"grid")
        .data(&[0, 0, 5, 7, 0x10, 0, 0x0c, 0])
        .assoc(&[full]);
    for id in 2..=49 {
        s.hevc(id, tile, cfg).hidden();
    }
    let tiles: Vec<u32> = (2..=49).collect();
    s.link(b"dimg", 1, &tiles);
    s
}

pub fn grid() -> Vec<u8> {
    grid_spec().build()
}

/// An overlay (item 1, canvas 100 x 80) of two hidden images (items 2 and 3).
pub fn overlay() -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    let small = s.prop(ispe(64, 64));
    let cfg = s.prop(hvcc());
    let canvas = s.prop(ispe(100, 80));
    let mut d = vec![0, 0];
    for fill in [0x1111u16, 0x2222, 0x3333, 0xffff] {
        d.extend_from_slice(&fill.to_be_bytes());
    }
    // 100 x 80, then the offsets (0, 0) and (-10, 20)
    d.extend_from_slice(&[0, 100, 0, 80, 0, 0, 0, 0, 0xff, 0xf6, 0, 20]);
    s.item(1, b"iovl").data(&d).assoc(&[canvas]);
    s.hevc(2, small, cfg).hidden();
    s.hevc(3, small, cfg).hidden();
    s.link(b"dimg", 1, &[2, 3]);
    s.build()
}

/// Image 1 with an alpha plane (2), a depth plane (3) and another auxiliary image (4).
pub fn alpha_depth() -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    let size = s.prop(ispe(64, 48));
    let cfg = s.prop(hvcc());
    let alpha = s.prop(auxc(ALPHA));
    let depth = s.prop(auxc(DEPTH));
    let other = s.prop(auxc("urn:x:other"));
    s.hevc(1, size, cfg);
    s.hevc(2, size, cfg).assoc(&[alpha]);
    s.hevc(3, size, cfg).assoc(&[depth]);
    s.hevc(4, size, cfg).assoc(&[other]);
    s.link(b"auxl", 2, &[1]);
    s.link(b"auxl", 3, &[1]);
    s.link(b"auxl", 4, &[1]);
    s.build()
}

/// Image 1 with two thumbnails (2 and 3), 16 x 12.
pub fn thumbnails() -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    let size = s.prop(ispe(64, 48));
    let small = s.prop(ispe(16, 12));
    let cfg = s.prop(hvcc());
    s.hevc(1, size, cfg);
    s.hevc(2, small, cfg);
    s.hevc(3, small, cfg);
    s.link(b"thmb", 2, &[1]);
    s.link(b"thmb", 3, &[1]);
    s.build()
}

/// Image 1 with an Apple gain map (2, `auxl`) whose XMP is item 3.
pub fn apple_gain_map() -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    let size = s.prop(ispe(64, 48));
    let small = s.prop(ispe(32, 24));
    let cfg = s.prop(hvcc());
    let aux = s.prop(auxc(APPLE_GAIN_MAP));
    s.hevc(1, size, cfg);
    s.hevc(2, small, cfg).assoc(&[aux]);
    s.item(3, b"mime").mime(XMP_TYPE).data(XMP);
    s.link(b"auxl", 2, &[1]);
    s.link(b"cdsc", 3, &[2]);
    s.build()
}

/// An AVIF gain map: base 1, gain map 2, `tmap` item 3 (the primary item) with `dimg` [1, 2], and
/// an `altr` group [3, 1].
pub fn tmap_altr() -> Vec<u8> {
    let mut s = Spec::new(b"avif");
    s.primary = 3;
    let size = s.prop(ispe(64, 48));
    let small = s.prop(ispe(32, 24));
    let cfg = s.prop(av1c());
    s.av1(1, size, cfg);
    s.av1(2, small, cfg);
    s.item(3, b"tmap").data(&tmap_payload());
    s.link(b"dimg", 3, &[1, 2]);
    s.group(b"altr", 100, &[3, 1]);
    s.build()
}

/// Image 1 with the given Exif item (2) and, when `xmp`, an XMP item (3).
pub fn exif_file(exif: &[u8], xmp: bool) -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    let size = s.prop(ispe(64, 48));
    let cfg = s.prop(hvcc());
    s.hevc(1, size, cfg);
    s.item(2, b"Exif").data(exif);
    s.link(b"cdsc", 2, &[1]);
    if xmp {
        s.item(3, b"mime").mime(XMP_TYPE).data(XMP);
        s.link(b"cdsc", 3, &[1]);
    }
    s.build()
}

/// Exif (offset header 2) and XMP.
pub fn exif_xmp() -> Vec<u8> {
    exif_file(&exif_item(2), true)
}

/// An Exif item whose offset header points past the item.
pub fn exif_bad_offset() -> Vec<u8> {
    exif_file(&[0, 0, 0, 200, b'M', b'M'], false)
}

/// `iloc` v1 with construction method 1: a 2 x 1 grid (1) whose payload and an Exif item (4)
/// (offset header 0) are in `idat`; tiles 2 and 3.
pub fn idat_file() -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    s.iloc = ILOC_FORMATS[2];
    let tile = s.prop(ispe(64, 64));
    let cfg = s.prop(hvcc());
    let full = s.prop(ispe(128, 64));
    s.item(1, b"grid")
        .data(&[0, 0, 0, 1, 0, 128, 0, 64])
        .assoc(&[full])
        .idat();
    s.hevc(2, tile, cfg).hidden();
    s.hevc(3, tile, cfg).hidden();
    s.item(4, b"Exif").data(&exif_item(0)).idat();
    s.link(b"dimg", 1, &[2, 3]);
    s.link(b"cdsc", 4, &[1]);
    s.build()
}

/// Image 1 (data `[1; 4]`) and an Exif item 2 (offset header 0), with the given `iloc` layout;
/// the Exif item is in `idat` when the version allows it.
pub fn iloc_file(fmt: IlocFmt) -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    s.iloc = fmt;
    let size = s.prop(ispe(64, 48));
    let cfg = s.prop(hvcc());
    s.hevc(1, size, cfg);
    let exif = s.item(2, b"Exif").data(&exif_item(0));
    if fmt.version >= 1 {
        exif.idat();
    }
    s.link(b"cdsc", 2, &[1]);
    s.build()
}

/// Like [`iloc_file`] (v1) but the Exif item uses construction method 2.
pub fn method2_file() -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    s.iloc = ILOC_FORMATS[2];
    let size = s.prop(ispe(64, 48));
    let cfg = s.prop(hvcc());
    s.hevc(1, size, cfg);
    s.item(2, b"Exif").data(&exif_item(0)).method(2);
    s.link(b"cdsc", 2, &[1]);
    s.build()
}

fn one_image(s: &mut Spec) {
    let size = s.prop(ispe(64, 48));
    let cfg = s.prop(hvcc());
    s.hevc(1, size, cfg);
}

/// A method-2 `mime` item with raw extents `(extent_index, offset, length)` and, when `to` is not
/// empty, an `iloc` reference from it to `to`.
fn method2_item(s: &mut Spec, id: u32, raw: &[(u64, u64, u64)], to: &[u32]) {
    s.item(id, b"mime").mime("text/plain").method(2).raw(raw);
    if !to.is_empty() {
        s.link(b"iloc", id, to);
    }
}

/// Construction method 2 (item offset), `iloc` v1 with 4-byte extent indexes. Item 20 holds
/// `0123456789abcdef` in `idat`; item 31 (method 1) is its bytes 8..12 then 0..4 (`89ab0123`).
/// The method-2 items, each with an `iloc` reference to its sources:
///
/// - 32 (source 31): bytes 2..6 (split over both source extents), then 6 to the end (length 0).
/// - 33 (sources 31, 20): extent 1 is 3 bytes of item 20 (index 2), extent 2 is 2 bytes of 31.
/// - 34 (source 32): a method-2 source, bytes 1..4.
/// - 35 and 36 refer to each other; 37 has an extent index past the list, 38 index 0, 39 a range
///   past the end of its source, 40 no `iloc` reference, 41 a source with no `iloc` entry.
/// - 50..=58 are a chain of nine method-2 items ending in item 31 (50 is one hop too many).
/// - 60..=68 are the same chain numbered the other way (68 is one hop too many): the result does
///   not depend on the order the items are visited in.
pub fn method2_items() -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    s.iloc = ILOC_FORMATS[3];
    let size = s.prop(ispe(64, 48));
    let cfg = s.prop(hvcc());
    s.hevc(1, size, cfg);
    s.item(20, b"mime")
        .mime("text/plain")
        .data(b"0123456789abcdef")
        .idat();
    s.item(31, b"mime")
        .mime("text/plain")
        .idat()
        .raw(&[(0, 8, 4), (0, 0, 4)]);
    method2_item(&mut s, 32, &[(1, 2, 4), (1, 6, 0)], &[31]);
    method2_item(&mut s, 33, &[(2, 0, 3), (1, 4, 2)], &[31, 20]);
    method2_item(&mut s, 34, &[(1, 1, 3)], &[32]);
    method2_item(&mut s, 35, &[(1, 0, 1)], &[36]);
    method2_item(&mut s, 36, &[(1, 0, 1)], &[35]);
    method2_item(&mut s, 37, &[(2, 0, 1)], &[31]);
    method2_item(&mut s, 38, &[(0, 0, 1)], &[31]);
    method2_item(&mut s, 39, &[(1, 6, 4)], &[31]);
    method2_item(&mut s, 40, &[(1, 0, 1)], &[]);
    method2_item(&mut s, 41, &[(1, 0, 1)], &[99]);
    for id in 50..=58u32 {
        let next = if id < 58 { id + 1 } else { 31 };
        method2_item(&mut s, id, &[(1, 0, 2)], &[next]);
    }
    for id in 60..=68u32 {
        let next = if id > 60 { id - 1 } else { 31 };
        method2_item(&mut s, id, &[(1, 0, 2)], &[next]);
    }
    s.build()
}

/// An image whose `iprp` has two unknown children after `ipco` and `ipma`: `junk` and `free`.
pub fn iprp_other_file() -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    one_image(&mut s);
    s.iprp_extra = cat(&[&bx(b"junk", &[1, 2, 3]), &bx(b"free", &[0; 5])]);
    s.build()
}

/// `meta` and `mdat` with 64-bit largesize headers, then a large `free` box.
pub fn largesize() -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    one_image(&mut s);
    s.meta_style = Style::Large;
    s.mdat_style = Style::Large;
    s.tail = wrap(b"free", &[0; 5], Style::Large);
    s.build()
}

/// The last box (`mdat`) has size 0: it runs to the end of the file.
pub fn size_zero() -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    one_image(&mut s);
    s.mdat_style = Style::ToEnd;
    s.build()
}

/// The `clap` of [`transforms`]: 40 x 20 in the middle.
pub fn clap_40x20() -> Vec<u8> {
    clap((40, 1), (20, 1), (0, 1), (0, 1))
}

/// Image 1 (`ispe` 100 x 50) with transforms in the order clap, irot 90, imir left-right; image 2
/// with irot 90, clap, imir top-bottom.
pub fn transforms() -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    let size = s.prop(ispe(100, 50));
    let cfg = s.prop(hvcc());
    let crop = s.prop(clap_40x20());
    let rot = s.prop(irot(1));
    let left_right = s.prop(imir(1));
    let top_bottom = s.prop(imir(0));
    s.hevc(1, size, cfg).assoc(&[crop, rot, left_right]);
    s.hevc(2, size, cfg).assoc(&[rot, crop, top_bottom]);
    s.build()
}

/// Image 1 has an essential unknown property, image 2 the same property not essential.
pub fn essential_unknown() -> Vec<u8> {
    let mut s = Spec::new(b"heic");
    let size = s.prop(ispe(64, 48));
    let cfg = s.prop(hvcc());
    let zzzz = s.prop(unknown(b"zzzz"));
    s.hevc(1, size, cfg).essential(zzzz);
    s.hevc(2, size, cfg).assoc(&[zzzz]);
    s.build()
}

/// Every sample file that parses, by name.
pub fn all_files() -> Vec<(&'static str, Vec<u8>)> {
    let mut v = vec![
        ("single", single()),
        ("avif", avif()),
        ("grid", grid()),
        ("overlay", overlay()),
        ("alpha_depth", alpha_depth()),
        ("thumbnails", thumbnails()),
        ("apple_gain_map", apple_gain_map()),
        ("tmap_altr", tmap_altr()),
        ("exif_xmp", exif_xmp()),
        ("exif_bad_offset", exif_bad_offset()),
        ("idat", idat_file()),
        ("method2", method2_file()),
        ("method2_items", method2_items()),
        ("iprp_other", iprp_other_file()),
        ("largesize", largesize()),
        ("size_zero", size_zero()),
        ("transforms", transforms()),
        ("essential_unknown", essential_unknown()),
        ("mini_full", mini_file(&MiniSpec::full())),
        ("mini_hevc", mini_file(&MiniSpec::hevc())),
        ("mini_gain_map", mini_file(&MiniSpec::gain_map())),
    ];
    for f in ILOC_FORMATS {
        v.push(("iloc", iloc_file(f)));
    }
    v
}

// ---- MP4 / MOV ----

/// File offset of the first byte of the `mdat` payload that [`mp4`] writes.
pub const MDAT0: u32 = 28;

fn be(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_be_bytes()).collect()
}

/// A table box: FullBox, `entry_count`, entries.
fn counted(kind: &[u8; 4], version: u8, count: usize, entries: &[u8]) -> Vec<u8> {
    fbx(
        kind,
        version,
        0,
        &cat(&[&(count as u32).to_be_bytes(), entries]),
    )
}

pub fn stts(runs: &[(u32, u32)]) -> Vec<u8> {
    let e: Vec<u8> = runs.iter().flat_map(|&(c, d)| be(&[c, d])).collect();
    counted(b"stts", 0, runs.len(), &e)
}

/// `ctts` with signed offsets (version 0 or 1 have the same bytes).
pub fn ctts(version: u8, runs: &[(u32, i32)]) -> Vec<u8> {
    let e: Vec<u8> = runs
        .iter()
        .flat_map(|&(c, o)| cat(&[&c.to_be_bytes(), &o.to_be_bytes()]))
        .collect();
    counted(b"ctts", version, runs.len(), &e)
}

pub fn stss(list: &[u32]) -> Vec<u8> {
    counted(b"stss", 0, list.len(), &be(list))
}

/// `stsc` from (first_chunk, samples_per_chunk, sample_description_index).
pub fn stsc(runs: &[(u32, u32, u32)]) -> Vec<u8> {
    let e: Vec<u8> = runs.iter().flat_map(|&(a, b, c)| be(&[a, b, c])).collect();
    counted(b"stsc", 0, runs.len(), &e)
}

pub fn stsz(sizes: &[u32]) -> Vec<u8> {
    fbx(
        b"stsz",
        0,
        0,
        &cat(&[
            &0u32.to_be_bytes(),
            &(sizes.len() as u32).to_be_bytes(),
            &be(sizes),
        ]),
    )
}

pub fn stsz_const(size: u32, count: u32) -> Vec<u8> {
    fbx(b"stsz", 0, 0, &be(&[size, count]))
}

/// `stz2` with 4, 8 or 16-bit fields (4-bit: high nibble first).
pub fn stz2(field: u8, sizes: &[u32]) -> Vec<u8> {
    let mut data = Vec::new();
    match field {
        4 => {
            for pair in sizes.chunks(2) {
                let hi = (pair[0] as u8) << 4;
                data.push(hi | pair.get(1).map_or(0, |&b| b as u8));
            }
        }
        8 => data.extend(sizes.iter().map(|&s| s as u8)),
        _ => {
            for &s in sizes {
                data.extend_from_slice(&(s as u16).to_be_bytes());
            }
        }
    }
    fbx(
        b"stz2",
        0,
        0,
        &cat(&[
            &[0, 0, 0, field],
            &(sizes.len() as u32).to_be_bytes(),
            &data,
        ]),
    )
}

pub fn stco(offsets: &[u32]) -> Vec<u8> {
    counted(b"stco", 0, offsets.len(), &be(offsets))
}

pub fn co64(offsets: &[u64]) -> Vec<u8> {
    let e: Vec<u8> = offsets.iter().flat_map(|o| o.to_be_bytes()).collect();
    counted(b"co64", 0, offsets.len(), &e)
}

/// An `stsd` with one visual entry of type `kind`: the 78 fixed bytes, then `children` as given.
pub fn stsd_visual(kind: &[u8; 4], w: u16, h: u16, children: &[u8]) -> Vec<u8> {
    let entry = bx(
        kind,
        &cat(&[
            &[0; 6],
            &1u16.to_be_bytes(),
            &[0; 16],
            &w.to_be_bytes(),
            &h.to_be_bytes(),
            &[0; 50],
            children,
        ]),
    );
    fbx(b"stsd", 0, 0, &cat(&[&1u32.to_be_bytes(), &entry]))
}

/// The default `stsd` of the test tracks: `hvc1` 64 x 48 with `hvcC` and `pasp`.
pub fn stsd_hvc1() -> Vec<u8> {
    stsd_visual(b"hvc1", 64, 48, &cat(&[&hvcc(), &pasp(1, 1)]))
}

/// An `stsd` with one audio entry (not parsed beyond its first 8 bytes).
pub fn stsd_audio() -> Vec<u8> {
    let entry = bx(b"mp4a", &cat(&[&[0; 6], &1u16.to_be_bytes(), &[0; 20]]));
    fbx(b"stsd", 0, 0, &cat(&[&1u32.to_be_bytes(), &entry]))
}

pub const IDENTITY: [i32; 9] = [0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000];

pub fn tkhd(id: u32, w: u32, h: u32, matrix: [i32; 9]) -> Vec<u8> {
    let m: Vec<u8> = matrix.iter().flat_map(|x| x.to_be_bytes()).collect();
    fbx(
        b"tkhd",
        0,
        3,
        &cat(&[
            &[0; 8],
            &id.to_be_bytes(),
            &[0; 4],
            &1000u32.to_be_bytes(),
            &[0; 16],
            &m,
            &(w << 16).to_be_bytes(),
            &(h << 16).to_be_bytes(),
        ]),
    )
}

pub fn mvhd(timescale: u32, duration: u32) -> Vec<u8> {
    fbx(
        b"mvhd",
        0,
        0,
        &cat(&[
            &[0; 8],
            &timescale.to_be_bytes(),
            &duration.to_be_bytes(),
            &[0; 80],
        ]),
    )
}

pub fn mdhd(timescale: u32, duration: u32) -> Vec<u8> {
    fbx(
        b"mdhd",
        0,
        0,
        &cat(&[
            &[0; 8],
            &timescale.to_be_bytes(),
            &duration.to_be_bytes(),
            &[0; 4],
        ]),
    )
}

pub fn hdlr(handler: &[u8; 4]) -> Vec<u8> {
    fbx(b"hdlr", 0, 0, &cat(&[&[0; 4], handler, &[0; 12], b"x\0"]))
}

/// `edts/elst` with version 0 entries (segment_duration, media_time).
pub fn edts_v0(entries: &[(u32, i32)]) -> Vec<u8> {
    let e: Vec<u8> = entries
        .iter()
        .flat_map(|&(d, t)| cat(&[&d.to_be_bytes(), &t.to_be_bytes(), &[0, 1, 0, 0]]))
        .collect();
    bx(b"edts", &counted(b"elst", 0, entries.len(), &e))
}

/// `edts/elst` with version 1 entries.
pub fn edts_v1(entries: &[(u64, i64)]) -> Vec<u8> {
    let e: Vec<u8> = entries
        .iter()
        .flat_map(|&(d, t)| cat(&[&d.to_be_bytes(), &t.to_be_bytes(), &[0, 1, 0, 0]]))
        .collect();
    bx(b"edts", &counted(b"elst", 1, entries.len(), &e))
}

/// A `trak`. The fields can be changed before [`Trak::build`].
pub struct Trak {
    pub id: u32,
    pub handler: [u8; 4],
    pub timescale: u32,
    pub matrix: [i32; 9],
    /// Children of `stbl`, in order.
    pub stbl: Vec<Vec<u8>>,
    /// Extra boxes under `trak`, `mdia` and `minf` (after the usual children).
    pub trak_extra: Vec<Vec<u8>>,
    pub mdia_extra: Vec<Vec<u8>>,
    pub minf_extra: Vec<Vec<u8>>,
}

impl Trak {
    /// A video track whose `stbl` is `stsd_hvc1()` and then `tables`.
    pub fn video(id: u32, tables: &[Vec<u8>]) -> Trak {
        let mut stbl = vec![stsd_hvc1()];
        stbl.extend_from_slice(tables);
        Trak {
            id,
            handler: *b"vide",
            timescale: 30000,
            matrix: IDENTITY,
            stbl,
            trak_extra: Vec::new(),
            mdia_extra: Vec::new(),
            minf_extra: Vec::new(),
        }
    }

    pub fn build(&self) -> Vec<u8> {
        let stbl = bx(b"stbl", &self.stbl.concat());
        let minf = bx(b"minf", &cat(&[&stbl, &self.minf_extra.concat()]));
        let mdia = bx(
            b"mdia",
            &cat(&[
                &mdhd(self.timescale, 90000),
                &hdlr(&self.handler),
                &minf,
                &self.mdia_extra.concat(),
            ]),
        );
        bx(
            b"trak",
            &cat(&[
                &tkhd(self.id, 64, 48, self.matrix),
                &mdia,
                &self.trak_extra.concat(),
            ]),
        )
    }
}

/// `moov`: `mvhd` (timescale 1000), the tracks, then `extra` boxes.
pub fn moov(tracks: &[Vec<u8>], extra: &[Vec<u8>]) -> Vec<u8> {
    bx(
        b"moov",
        &cat(&[&mvhd(1000, 5000), &tracks.concat(), &extra.concat()]),
    )
}

/// `ftyp`, an `mdat` of `mdat_len` bytes (byte `i` is `i % 251`), then `moov`. The `mdat`
/// payload starts at [`MDAT0`].
pub fn mp4(moov: &[u8], mdat_len: usize) -> Vec<u8> {
    let data: Vec<u8> = (0..mdat_len).map(|i| (i % 251) as u8).collect();
    cat(&[&bx(b"ftyp", b"isom\0\0\0\0isom"), &bx(b"mdat", &data), moov])
}

/// One video track of 7 samples: `stsc` with three runs (the middle one holds no samples, the
/// last starts past the last chunk), variable sizes, `co64` or `stco`.
pub fn movie_runs() -> Vec<u8> {
    movie_runs_with(&[])
}

/// [`movie_runs`] with more tables (for example an `stss`) after the usual ones.
pub fn movie_runs_with(extra: &[Vec<u8>]) -> Vec<u8> {
    let at = |rel: u32| MDAT0 + rel;
    let mut tables = vec![
        stts(&[(7, 100)]),
        stsc(&[(1, 2, 1), (3, 0, 1), (5, 1, 1), (8, 9, 1)]),
        stsz(&[10, 11, 12, 13, 14, 15, 16]),
        stco(&[at(0), at(100), at(200), at(300), at(400), at(500), at(600)]),
    ];
    tables.extend_from_slice(extra);
    let t = Trak::video(1, &tables);
    mp4(&moov(&[t.build()], &[]), 800)
}

/// A video track of 6 samples: `stss` [1, 4, 6], `ctts` v1 with a negative offset, `stz2`.
pub fn movie_sync() -> Vec<u8> {
    let t = Trak::video(
        1,
        &[
            stts(&[(6, 10)]),
            ctts(1, &[(1, 20), (2, -5), (3, 0)]),
            stss(&[1, 4, 6]),
            stsc(&[(1, 3, 1)]),
            stz2(8, &[4, 5, 6, 7, 8, 9]),
            co64(&[u64::from(MDAT0), u64::from(MDAT0) + 50]),
        ],
    );
    mp4(&moov(&[t.build()], &[]), 200)
}

/// Video and audio tracks, an edit list, `udta` in `moov`.
pub fn movie_two_tracks() -> Vec<u8> {
    let mut v = Trak::video(
        1,
        &[
            stts(&[(4, 1001)]),
            stsc(&[(1, 4, 1)]),
            stsz_const(5, 4),
            stco(&[MDAT0]),
        ],
    );
    v.trak_extra.push(edts_v0(&[(2000, -1), (3000, 0)]));
    let mut a = Trak::video(
        2,
        &[
            stts(&[(3, 1024)]),
            stsc(&[(1, 3, 1)]),
            stsz(&[1, 2, 3]),
            stco(&[MDAT0 + 40]),
        ],
    );
    a.handler = *b"soun";
    a.stbl[0] = stsd_audio();
    a.timescale = 48000;
    mp4(
        &moov(&[v.build(), a.build()], &[bx(b"udta", &[1, 2, 3, 4])]),
        100,
    )
}

// ---- fragmented MP4 ----

/// `trex`: the defaults of track `id`.
pub fn trex(id: u32, desc: u32, dur: u32, size: u32, flags: u32) -> Vec<u8> {
    fbx(b"trex", 0, 0, &be(&[id, desc, dur, size, flags]))
}

/// `tfhd` of track `id` with `flags` and the optional `fields` they announce, in order
/// (base_data_offset is two words).
pub fn tfhd(id: u32, flags: u32, fields: &[u32]) -> Vec<u8> {
    fbx(b"tfhd", 0, flags, &be(&[&[id][..], fields].concat()))
}

/// `tfdt`, version 0 (32 bits) or 1 (64 bits).
pub fn tfdt(version: u8, t: u64) -> Vec<u8> {
    let body = if version == 0 {
        (t as u32).to_be_bytes().to_vec()
    } else {
        t.to_be_bytes().to_vec()
    };
    fbx(b"tfdt", version, 0, &body)
}

/// `trun`: `sample_count`, then `head` (data_offset, first_sample_flags, as `flags` announces
/// them), then the per-sample `records`.
pub fn trun(version: u8, flags: u32, count: u32, head: &[u32], records: &[u32]) -> Vec<u8> {
    fbx(
        b"trun",
        version,
        flags,
        &be(&[&[count][..], head, records].concat()),
    )
}

pub fn traf(children: &[Vec<u8>]) -> Vec<u8> {
    bx(b"traf", &children.concat())
}

/// `moof`: an `mfhd`, then the `traf` boxes.
pub fn moof(trafs: &[Vec<u8>]) -> Vec<u8> {
    bx(
        b"moof",
        &cat(&[&fbx(b"mfhd", 0, 0, &be(&[1])), &trafs.concat()]),
    )
}

/// The offset of the first byte of the `mdat` payload of a file.
pub fn mdat0(f: &[u8]) -> u64 {
    f.windows(4).position(|w| w == b"mdat").unwrap() as u64 + 4
}

/// A fragmented file: `ftyp`, a `moov` with `tracks` video tracks (ids 1..) without samples and
/// an `mvex` of `trexes`, the boxes that `moofs(moof_start, mdat_payload_start)` returns, then an
/// `mdat` of 400 bytes. The result of `moofs` must have the same length for any arguments.
pub fn frag_file(tracks: u32, trexes: &[Vec<u8>], moofs: impl Fn(u64, u64) -> Vec<u8>) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"isom\0\0\0\0isom");
    let traks: Vec<Vec<u8>> = (1..=tracks)
        .map(|id| Trak::video(id, &[stts(&[]), stsc(&[]), stsz(&[]), stco(&[])]).build())
        .collect();
    let m = moov(&traks, &[bx(b"mvex", &trexes.concat())]);
    let start = (ftyp.len() + m.len()) as u64;
    let len = moofs(0, 0).len() as u64;
    let data: Vec<u8> = (0..400).map(|i| (i % 251) as u8).collect();
    cat(&[
        &ftyp,
        &m,
        &moofs(start, start + len + 8),
        &bx(b"mdat", &data),
    ])
}

/// Two fragmented tracks, two `moof` boxes. Track 1 (trex: 100 ticks, 10 bytes, flags 0): moof 1
/// has `default-base-is-moof`, tfhd defaults (200 ticks, 20 bytes, flags 0x0200_0000), a run of
/// 2 samples with `data_offset` and a run of 2 samples with per-sample duration and size and no
/// `data_offset`; moof 2 has an explicit base, `tfdt` 5000 (version 1), a version 1 run with
/// first_sample_flags, sizes and cto (-5, 3). Track 2 (trex: 100 ticks, 10 bytes, flags
/// 0x10000) has one run of 2 samples with only the trex defaults, in moof 1, implicit base.
pub fn movie_frag() -> Vec<u8> {
    frag_file(
        2,
        &[trex(1, 1, 100, 10, 0), trex(2, 1, 100, 10, 0x10000)],
        |moof_at, data| {
            let a = traf(&[
                tfhd(1, 0x2_0038, &[200, 20, 0x0200_0000]),
                trun(0, 0x1, 2, &[(data - moof_at) as u32], &[]),
                trun(0, 0x300, 2, &[], &[50, 7, 60, 8]),
            ]);
            let b = traf(&[tfhd(2, 0, &[]), trun(0, 0, 2, &[], &[])]);
            let m1 = moof(&[a, b]);
            let c = traf(&[
                tfhd(1, 0x1, &[0, (data + 100) as u32]),
                tfdt(1, 5000),
                trun(
                    1,
                    0x1 | 0x4 | 0x200 | 0x800,
                    2,
                    &[4, 0x10000],
                    &[6, -5i32 as u32, 7, 3],
                ),
            ]);
            cat(&[&m1, &moof(&[c])])
        },
    )
}

/// Every sample movie file that parses, by name.
pub fn all_movie_files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("runs", movie_runs()),
        ("sync", movie_sync()),
        ("two_tracks", movie_two_tracks()),
        ("frag", movie_frag()),
    ]
}

// ---- the `mini` box ----

/// Writes bits, most significant first; the last byte is padded with zero bits.
#[derive(Default)]
pub struct BitWriter {
    bytes: Vec<u8>,
    used: u32,
}

impl BitWriter {
    pub fn flag(&mut self, on: bool) {
        if self.used == 0 {
            self.bytes.push(0);
        }
        let last = self.bytes.last_mut().unwrap();
        if on {
            *last |= 0x80u8 >> self.used;
        }
        self.used = (self.used + 1) % 8;
    }

    pub fn put(&mut self, value: u64, bits: u32) {
        for i in (0..bits).rev() {
            self.flag((value >> i) & 1 == 1);
        }
    }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// The gain map part of a [`MiniSpec`] (`gainmap_flag`).
#[derive(Clone)]
pub struct GainSpec {
    pub same_dims: bool,
    pub width: u32,
    pub height: u32,
    pub matrix: u8,
    pub full_range: bool,
    pub chroma: u8,
    pub float: bool,
    pub float_log2: u8,
    pub depth: u8,
    /// (primaries, transfer, matrix, full range) when `tmap_explicit_cicp_flag` is set.
    pub tmap_cicp: Option<(u8, u8, u8, bool)>,
    pub hdr: isobmff::MiniHdr,
}

impl GainSpec {
    /// A 32 x 24 4:0:0 8 bit gain map of its own size.
    pub fn small() -> Self {
        Self {
            same_dims: false,
            width: 32,
            height: 24,
            matrix: 6,
            full_range: true,
            chroma: 0,
            float: false,
            float_log2: 0,
            depth: 8,
            tmap_cicp: None,
            hdr: isobmff::MiniHdr::default(),
        }
    }
}

/// Everything a `mini` box says; [`mini_payload`] writes it. The `icc`, `exif` and `xmp` flags
/// are set when their data is not empty, the large flags when a size needs them (or `large`).
#[derive(Clone)]
pub struct MiniSpec {
    /// The `ftyp` minor version: `avif` or `heic`.
    pub minor: [u8; 4],
    pub version: u8,
    /// `explicit_codec_types_flag`: (`infe_type`, `codec_config_type`).
    pub codec: Option<([u8; 4], [u8; 4])>,
    pub float: bool,
    /// `bit_depth_log2_minus4` of a float image.
    pub float_log2: u8,
    /// 8, or 9..=16.
    pub depth: u8,
    pub full_range: bool,
    pub alpha: bool,
    pub premultiplied: bool,
    pub cicp: Option<(u8, u8, u8)>,
    /// `hdr_flag` (set anyway when there is a gain map).
    pub hdr: bool,
    pub hdr_main: isobmff::MiniHdr,
    pub gain: Option<GainSpec>,
    pub chroma: u8,
    pub chroma_h: bool,
    pub chroma_v: bool,
    pub orientation: u8,
    pub width: u32,
    pub height: u32,
    pub compressed: bool,
    pub large: bool,
    pub main_cfg: Vec<u8>,
    pub alpha_cfg: Vec<u8>,
    pub gain_cfg: Vec<u8>,
    pub icc: Vec<u8>,
    pub tmap_icc: Vec<u8>,
    pub gain_meta: Vec<u8>,
    pub alpha_data: Vec<u8>,
    pub gain_data: Vec<u8>,
    pub main_data: Vec<u8>,
    pub exif: Vec<u8>,
    pub xmp: Vec<u8>,
    /// Bytes after the last chunk.
    pub trailing: Vec<u8>,
}

impl MiniSpec {
    /// AV1 (`ftyp` minor version `avif`), 64 x 48, 4:2:0, 8 bit, full range, orientation 1, the
    /// [`av1c`] payload as configuration, no metadata.
    pub fn av1() -> Self {
        Self {
            minor: *b"avif",
            version: 0,
            codec: None,
            float: false,
            float_log2: 0,
            depth: 8,
            full_range: true,
            alpha: false,
            premultiplied: false,
            cicp: None,
            hdr: false,
            hdr_main: isobmff::MiniHdr::default(),
            gain: None,
            chroma: 1,
            chroma_h: false,
            chroma_v: false,
            orientation: 1,
            width: 64,
            height: 48,
            compressed: false,
            large: false,
            main_cfg: av1c()[8..].to_vec(),
            alpha_cfg: Vec::new(),
            gain_cfg: Vec::new(),
            icc: Vec::new(),
            tmap_icc: Vec::new(),
            gain_meta: Vec::new(),
            alpha_data: Vec::new(),
            gain_data: Vec::new(),
            main_data: b"MAIN-DATA-0123".to_vec(),
            exif: Vec::new(),
            xmp: Vec::new(),
            trailing: Vec::new(),
        }
    }

    /// HEVC (`ftyp` minor version `heic`) with the [`hvcc`] payload.
    pub fn hevc() -> Self {
        Self {
            minor: *b"heic",
            main_cfg: hvcc()[8..].to_vec(),
            ..Self::av1()
        }
    }

    /// Alpha (premultiplied), ICC, Exif and XMP, explicit CICP, 10 bit, orientation 6.
    pub fn full() -> Self {
        Self {
            depth: 10,
            alpha: true,
            premultiplied: true,
            cicp: Some((9, 16, 9)),
            orientation: 6,
            icc: b"ICC-PROFILE-BYTES".to_vec(),
            alpha_data: b"ALPHA-DATA-4567".to_vec(),
            exif: TIFF.to_vec(),
            xmp: XMP.to_vec(),
            ..Self::av1()
        }
    }

    /// A gain map of its own size with the [`tmap_payload`] metadata and a `tmap` ICC profile.
    pub fn gain_map() -> Self {
        Self {
            hdr_main: isobmff::MiniHdr {
                clli: Some(isobmff::MiniClli {
                    max_content_light_level: 1000,
                    max_pic_average_light_level: 400,
                }),
                ..isobmff::MiniHdr::default()
            },
            gain: Some(GainSpec {
                hdr: isobmff::MiniHdr {
                    ndwt: Some(203),
                    ..isobmff::MiniHdr::default()
                },
                ..GainSpec::small()
            }),
            tmap_icc: b"TMAP-ICC".to_vec(),
            gain_meta: tmap_payload()[1..].to_vec(),
            gain_data: b"GAIN-DATA-89".to_vec(),
            ..Self::av1()
        }
    }
}

fn put_centred(w: &mut BitWriter, chroma: u8, h: bool, v: bool) {
    if chroma == 1 || chroma == 2 {
        w.flag(h);
    }
    if chroma == 1 {
        w.flag(v);
    }
}

fn put_depth(w: &mut BitWriter, float: bool, log2: u8, depth: u8) {
    if float {
        w.put(u64::from(log2), 2);
    } else if depth > 8 {
        w.flag(true);
        w.put(u64::from(depth - 9), 3);
    } else {
        w.flag(false);
    }
}

fn put_hdr(w: &mut BitWriter, h: &isobmff::MiniHdr) {
    w.flag(h.clli.is_some());
    w.flag(h.mdcv.is_some());
    w.flag(h.cclv.is_some());
    w.flag(h.amve.is_some());
    w.flag(h.reve.is_some());
    w.flag(h.ndwt.is_some());
    if let Some(c) = h.clli {
        w.put(u64::from(c.max_content_light_level), 16);
        w.put(u64::from(c.max_pic_average_light_level), 16);
    }
    if let Some(m) = h.mdcv {
        for (x, y) in m.primaries {
            w.put(u64::from(x), 16);
            w.put(u64::from(y), 16);
        }
        w.put(u64::from(m.white_point.0), 16);
        w.put(u64::from(m.white_point.1), 16);
        w.put(u64::from(m.max_luminance), 32);
        w.put(u64::from(m.min_luminance), 32);
    }
    if let Some(c) = h.cclv {
        w.put(0, 2);
        w.flag(c.primaries.is_some());
        w.flag(c.min_luminance.is_some());
        w.flag(c.max_luminance.is_some());
        w.flag(c.avg_luminance.is_some());
        w.put(0, 2);
        for (x, y) in c.primaries.into_iter().flatten() {
            w.put(u64::from(x as u32), 32);
            w.put(u64::from(y as u32), 32);
        }
        for v in [c.min_luminance, c.max_luminance, c.avg_luminance]
            .into_iter()
            .flatten()
        {
            w.put(u64::from(v), 32);
        }
    }
    if let Some(a) = h.amve {
        w.put(u64::from(a.ambient_illumination), 32);
        w.put(u64::from(a.ambient_light_x), 16);
        w.put(u64::from(a.ambient_light_y), 16);
    }
    if let Some(r) = h.reve {
        for (a, x, y) in r.values {
            w.put(u64::from(a), 32);
            w.put(u64::from(x), 16);
            w.put(u64::from(y), 16);
        }
    }
    if let Some(n) = h.ndwt {
        w.put(u64::from(n), 32);
    }
}

/// The payload of a `mini` box (the bytes after its header).
pub fn mini_payload(m: &MiniSpec) -> Vec<u8> {
    let gain = m.gain.as_ref();
    let hdr = m.hdr || gain.is_some();
    let (icc, exif, xmp) = (!m.icc.is_empty(), !m.exif.is_empty(), !m.xmp.is_empty());
    let gain_data = if gain.is_some() { m.gain_data.len() } else { 0 };
    let tmap_icc = if gain.is_some() { m.tmap_icc.len() } else { 0 };
    let gain_meta = if gain.is_some() { m.gain_meta.len() } else { 0 };
    let alpha_data = if m.alpha { m.alpha_data.len() } else { 0 };
    let large_dim = m.large
        || m.width.max(m.height) > 128
        || gain.is_some_and(|g| !g.same_dims && g.width.max(g.height) > 128);
    let large_meta = m.large
        || [m.icc.len(), tmap_icc, m.exif.len(), m.xmp.len()]
            .iter()
            .any(|&n| n > 1024)
        || gain_meta > 1023;
    let large_cfg = m.large
        || [m.main_cfg.len(), m.alpha_cfg.len(), m.gain_cfg.len()]
            .iter()
            .any(|&n| n > 7);
    let large_data =
        m.large || m.main_data.len() > 32768 || alpha_data > 32767 || gain_data > 32767;

    let mut w = BitWriter::default();
    w.put(u64::from(m.version), 2);
    w.flag(m.codec.is_some());
    w.flag(m.float);
    w.flag(m.full_range);
    w.flag(m.alpha);
    w.flag(m.cicp.is_some());
    w.flag(hdr);
    w.flag(icc);
    w.flag(exif);
    w.flag(xmp);
    w.put(u64::from(m.chroma), 2);
    w.put(u64::from(m.orientation - 1), 3);
    w.flag(large_dim);
    let dn = if large_dim { 15 } else { 7 };
    w.put(u64::from(m.width - 1), dn);
    w.put(u64::from(m.height - 1), dn);
    put_centred(&mut w, m.chroma, m.chroma_h, m.chroma_v);
    put_depth(&mut w, m.float, m.float_log2, m.depth);
    if m.alpha {
        w.flag(m.premultiplied);
    }
    if let Some((p, t, mc)) = m.cicp {
        for v in [p, t, mc] {
            w.put(u64::from(v), 8);
        }
    }
    if let Some((infe, cfg)) = m.codec {
        w.put(u64::from(u32::from_be_bytes(infe)), 32);
        w.put(u64::from(u32::from_be_bytes(cfg)), 32);
    }
    if hdr {
        w.flag(gain.is_some());
        if let Some(g) = gain {
            w.flag(g.same_dims);
            if !g.same_dims {
                w.put(u64::from(g.width - 1), dn);
                w.put(u64::from(g.height - 1), dn);
            }
            w.put(u64::from(g.matrix), 8);
            w.flag(g.full_range);
            w.put(u64::from(g.chroma), 2);
            put_centred(&mut w, g.chroma, false, false);
            w.flag(g.float);
            put_depth(&mut w, g.float, g.float_log2, g.depth);
            w.flag(tmap_icc > 0);
            w.flag(g.tmap_cicp.is_some());
            if let Some((p, t, mc, full)) = g.tmap_cicp {
                for v in [p, t, mc] {
                    w.put(u64::from(v), 8);
                }
                w.flag(full);
            }
        }
        put_hdr(&mut w, &m.hdr_main);
        if let Some(g) = gain {
            put_hdr(&mut w, &g.hdr);
        }
    }
    if icc || exif || xmp || gain.is_some() {
        w.flag(large_meta);
    }
    w.flag(large_cfg);
    w.flag(large_data);
    let (mb, cb, db) = (
        if large_meta { 20 } else { 10 },
        if large_cfg { 12 } else { 3 },
        if large_data { 28 } else { 15 },
    );
    if icc {
        w.put(m.icc.len() as u64 - 1, mb);
    }
    if tmap_icc > 0 {
        w.put(tmap_icc as u64 - 1, mb);
    }
    if gain.is_some() {
        w.put(gain_meta as u64, mb);
        w.put(gain_data as u64, db);
        if gain_data > 0 {
            w.put(m.gain_cfg.len() as u64, cb);
        }
    }
    w.put(m.main_cfg.len() as u64, cb);
    w.put(m.main_data.len() as u64 - 1, db);
    if m.alpha {
        w.put(alpha_data as u64, db);
        if alpha_data > 0 {
            w.put(m.alpha_cfg.len() as u64, cb);
        }
    }
    if exif || xmp {
        w.flag(m.compressed);
    }
    if exif {
        w.put(m.exif.len() as u64 - 1, mb);
    }
    if xmp {
        w.put(m.xmp.len() as u64 - 1, mb);
    }
    let mut out = w.finish();
    out.extend_from_slice(&m.main_cfg);
    if alpha_data > 0 {
        out.extend_from_slice(&m.alpha_cfg);
    }
    if gain_data > 0 {
        out.extend_from_slice(&m.gain_cfg);
    }
    out.extend_from_slice(&m.icc);
    if gain.is_some() {
        out.extend_from_slice(&m.tmap_icc);
        out.extend_from_slice(&m.gain_meta);
    }
    if m.alpha {
        out.extend_from_slice(&m.alpha_data);
    }
    if gain.is_some() {
        out.extend_from_slice(&m.gain_data);
    }
    out.extend_from_slice(&m.main_data);
    out.extend_from_slice(&m.exif);
    out.extend_from_slice(&m.xmp);
    out.extend_from_slice(&m.trailing);
    out
}

/// `ftyp` (major `mif3`, the minor version of `m`, no compatible brands) and the `mini` box.
pub fn mini_file(m: &MiniSpec) -> Vec<u8> {
    mini_file_of(m.minor, &bx(b"mini", &mini_payload(m)))
}

/// `ftyp` (major `mif3`) with the given minor version, then `rest`.
pub fn mini_file_of(minor: [u8; 4], rest: &[u8]) -> Vec<u8> {
    cat(&[&bx(b"ftyp", &cat(&[b"mif3", &minor])), rest])
}

/// The position of the first `needle` in `file`.
pub fn find(file: &[u8], needle: &[u8]) -> u64 {
    file.windows(needle.len())
        .position(|w| w == needle)
        .unwrap() as u64
}
