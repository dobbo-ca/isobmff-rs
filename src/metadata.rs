//! Exif and XMP items: the bytes that exif-rs parses.

use std::borrow::Cow;

use crate::Error;
use crate::boxes::be32;

/// The Exif and XMP bytes of an image, see [`Heif::metadata`](crate::Heif::metadata).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Metadata<'a> {
    /// The Exif data from the TIFF header on (what `exif::Exif::parse` takes).
    pub exif: Option<Cow<'a, [u8]>>,
    /// The XMP packet.
    pub xmp: Option<Cow<'a, [u8]>>,
}

/// Strips the `exif_tiff_header_offset` prefix of an Exif item payload: a big-endian u32, that
/// many bytes, then the TIFF header. An item shorter than 4 bytes or an offset past the end is
/// `Malformed("Exif")`.
pub(crate) fn exif_tiff(bytes: Cow<'_, [u8]>) -> Result<Cow<'_, [u8]>, Error> {
    let off = be32(&bytes, 0).ok_or(Error::Malformed("Exif"))?;
    let start = usize::try_from(off)
        .ok()
        .and_then(|o| o.checked_add(4))
        .filter(|&s| s <= bytes.len())
        .ok_or(Error::Malformed("Exif"))?;
    Ok(match bytes {
        Cow::Borrowed(b) => Cow::Borrowed(b.get(start..).ok_or(Error::Malformed("Exif"))?),
        Cow::Owned(mut v) => {
            v.drain(..start);
            Cow::Owned(v)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exif_offset_header() {
        // offset 2: skip 2 bytes ("xx"), the TIFF header starts at "II*\0".
        let b = [0, 0, 0, 2, b'x', b'x', b'I', b'I', b'*', 0];
        assert_eq!(exif_tiff(Cow::Borrowed(&b)).unwrap().as_ref(), b"II*\0");
        assert_eq!(
            exif_tiff(Cow::Owned(b.to_vec())).unwrap().as_ref(),
            b"II*\0"
        );
        // offset 0
        let b = [0, 0, 0, 0, b'M', b'M'];
        assert_eq!(exif_tiff(Cow::Borrowed(&b)).unwrap().as_ref(), b"MM");
        // offset past the end, and too short
        assert_eq!(
            exif_tiff(Cow::Borrowed(&[0, 0, 0, 9, 1, 2])),
            Err(Error::Malformed("Exif"))
        );
        assert_eq!(
            exif_tiff(Cow::Borrowed(&[0, 0])),
            Err(Error::Malformed("Exif"))
        );
        // offset exactly at the end gives an empty block
        assert_eq!(exif_tiff(Cow::Borrowed(&[0, 0, 0, 1, 7])).unwrap().len(), 0);
    }
}
