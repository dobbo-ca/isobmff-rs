//! `read` (a `Read + Seek`) against `parse` (the file in memory).

mod common;

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::atomic::AtomicBool;

use common::*;
use isobmff::{Error, Limit, Limits};

#[test]
fn read_equals_parse_for_every_file() {
    let c = AtomicBool::new(false);
    let d = Limits::default();
    for (name, file) in all_files() {
        let a = isobmff::parse(&file, &d, &c).unwrap();
        let b = isobmff::read(Cursor::new(&file[..]), &d, &c).unwrap();
        assert_eq!(a, b, "{name}");
        assert_eq!(a.stats, b.stats, "{name}");
    }
}

#[test]
fn read_and_parse_fail_alike() {
    let c = AtomicBool::new(false);
    let d = Limits::default();
    let mut cut = single();
    // Cut inside `meta`: after `meta`, a damaged tail is not an error (see below).
    let at = cut.windows(4).position(|w| w == b"meta").unwrap();
    cut.truncate(at + 8);
    let free = bx(b"free", &[0; 8]);
    let ftyp_only = bx(b"ftyp", b"heic\0\0\0\0");
    let cases = [
        ("truncated", cut, Error::Truncated),
        ("no ftyp", free, Error::UnknownFormat),
        ("empty", Vec::new(), Error::UnknownFormat),
        ("no meta", ftyp_only, Error::NoMeta),
    ];
    for (name, file, want) in cases {
        assert_eq!(
            isobmff::parse(&file, &d, &c).err(),
            Some(want.clone()),
            "{name}"
        );
        assert_eq!(
            isobmff::read(Cursor::new(&file[..]), &d, &c).err(),
            Some(want),
            "{name}"
        );
    }
}

/// A reader that counts the bytes it hands out.
struct Counting {
    inner: Cursor<Vec<u8>>,
    read: usize,
}

impl Read for Counting {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n;
        Ok(n)
    }
}

impl Seek for Counting {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

fn box_len(file: &[u8], pos: usize) -> usize {
    let head: [u8; 4] = file[pos..pos + 4].try_into().unwrap();
    u32::from_be_bytes(head) as usize
}

#[test]
fn read_copies_only_ftyp_and_meta() {
    let c = AtomicBool::new(false);
    let mut s = grid_spec();
    s.pad = 16 << 20;
    let file = s.build();
    let ftyp_len = box_len(&file, 0);
    let meta_len = box_len(&file, ftyp_len);
    assert!(file.len() > 16 << 20);

    let want = isobmff::parse(&file, &Limits::default(), &c).unwrap();
    // meta fits exactly, max_alloc is far below the size of mdat.
    let tight = Limits {
        max_alloc: 512 << 10,
        max_meta_bytes: meta_len,
        ..Limits::default()
    };
    let mut r = Counting {
        inner: Cursor::new(file.clone()),
        read: 0,
    };
    let got = isobmff::read(&mut r, &tight, &c).unwrap();
    assert_eq!(got, want);
    assert!(
        r.read <= 2 * ftyp_len + meta_len + 256,
        "read {} bytes of a {} byte file (meta is {meta_len})",
        r.read,
        file.len()
    );

    // One byte less than meta is over the cap.
    let over = Limits {
        max_meta_bytes: meta_len - 1,
        ..tight
    };
    assert_eq!(
        isobmff::read(Cursor::new(&file[..]), &over, &c).err(),
        Some(Error::LimitExceeded(Limit::MetaBytes))
    );

    // The extents are absolute offsets: the caller reads the item from the same reader.
    let e = got.item(2).unwrap().data.clone().unwrap()[0];
    let mut buf = vec![0; e.length as usize];
    r.seek(SeekFrom::Start(e.offset)).unwrap();
    r.read_exact(&mut buf).unwrap();
    assert_eq!(buf, [2; 4]);
    let end = want.item(49).unwrap().data.clone().unwrap()[0];
    assert_eq!(end.end(), Some(file.len() as u64));
}

#[test]
fn damaged_tail_after_meta_fails_per_item() {
    let c = AtomicBool::new(false);
    let d = Limits::default();
    let mut file = single();
    file.truncate(file.len() - 3);
    let a = isobmff::parse(&file, &d, &c).unwrap();
    let b = isobmff::read(Cursor::new(&file[..]), &d, &c).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.item_bytes(&file, 1, &d, &c), Err(Error::Truncated));
}
