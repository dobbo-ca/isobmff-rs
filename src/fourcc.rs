//! Four-character codes.

use std::fmt;

/// A four-byte box, item, brand or property type, for example `ispe` or `hvc1`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FourCc(pub [u8; 4]);

impl FourCc {
    /// The four bytes.
    pub fn as_bytes(&self) -> &[u8; 4] {
        &self.0
    }
}

impl fmt::Display for FourCc {
    /// Printable ASCII as is, every other byte as `\xNN`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &b in &self.0 {
            if b.is_ascii_graphic() || b == b' ' {
                write!(f, "{}", char::from(b))?;
            } else {
                write!(f, "\\x{b:02x}")?;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for FourCc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FourCc(\"{self}\")")
    }
}
