//! `ftyp`: the brands.

use crate::boxes::Cur;
use crate::budget::Budget;
use crate::{Error, FourCc};

/// The file type box: major brand, minor version and compatible brands, as given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ftyp {
    /// `major_brand`.
    pub major: FourCc,
    /// `minor_version`.
    pub minor_version: u32,
    /// `compatible_brands`, in file order.
    pub compatible: Vec<FourCc>,
}

impl Ftyp {
    /// True when `brand` is the major brand or one of the compatible brands.
    pub fn has_brand(&self, brand: &[u8; 4]) -> bool {
        self.major.0 == *brand || self.compatible.iter().any(|b| b.0 == *brand)
    }
}

/// Parses the payload of an `ftyp` box.
pub(crate) fn parse(payload: &[u8], budget: &Budget<'_>) -> Result<Ftyp, Error> {
    let mut c = Cur::new("ftyp", payload, 0);
    let major = c.fourcc()?;
    let minor_version = c.u32()?;
    let n = c.remaining().checked_div(4).unwrap_or(0);
    if n.saturating_mul(4) != c.remaining() {
        return Err(Error::Malformed("ftyp"));
    }
    let mut compatible = budget.vec::<FourCc>(n)?;
    for _ in 0..n {
        compatible.push(c.fourcc()?);
    }
    Ok(Ftyp {
        major,
        minor_version,
        compatible,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Limits;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn brands() {
        let limits = Limits::default();
        let cancel = AtomicBool::new(false);
        let budget = Budget::new(&limits, &cancel);
        let f = parse(b"avif\0\0\0\0mif1miaf", &budget).unwrap();
        assert!(f.has_brand(b"avif") && f.has_brand(b"miaf") && !f.has_brand(b"heic"));
        assert!(parse(b"avif\0\0\0\0mif", &budget).is_err());
        assert!(parse(b"avif\0\0", &budget).is_err());
    }
}
