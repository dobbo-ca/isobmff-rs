use std::fmt;

/// The [`Limits`](crate::Limits) field that a call would pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Limit {
    /// [`Limits::max_alloc`](crate::Limits::max_alloc).
    Alloc,
    /// [`Limits::max_boxes`](crate::Limits::max_boxes).
    Boxes,
    /// [`Limits::max_depth`](crate::Limits::max_depth).
    Depth,
    /// [`Limits::max_items`](crate::Limits::max_items).
    Items,
    /// [`Limits::max_properties`](crate::Limits::max_properties).
    Properties,
    /// [`Limits::max_references`](crate::Limits::max_references).
    References,
    /// [`Limits::max_extents`](crate::Limits::max_extents).
    Extents,
    /// [`Limits::max_grid_tiles`](crate::Limits::max_grid_tiles).
    GridTiles,
    /// [`Limits::max_pixels`](crate::Limits::max_pixels).
    Pixels,
    /// [`Limits::max_meta_bytes`](crate::Limits::max_meta_bytes).
    MetaBytes,
    /// [`Limits::max_metadata_bytes`](crate::Limits::max_metadata_bytes).
    MetadataBytes,
    /// [`Limits::max_moov_bytes`](crate::Limits::max_moov_bytes).
    MoovBytes,
}

/// Every error of this crate. Limit errors are one variant with a [`Limit`] so a caller can
/// match on the limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The input does not start with an `ftyp` box.
    UnknownFormat,
    /// The file has no top-level `meta` box.
    NoMeta,
    /// [`parse_movie`](crate::parse_movie) or [`read_movie`](crate::read_movie): the file has no
    /// top-level `moov` box.
    NoMoov,
    /// A box or the data of an item runs past the end of the input.
    Truncated,
    /// A structure is invalid. The text names it, for example `"iloc"`. It is a short stable
    /// token, not a sentence.
    Malformed(&'static str),
    /// A feature this crate does not support, for example an item of an unknown type.
    Unsupported(&'static str),
    /// No item has this id.
    NoSuchItem(u32),
    /// A [`Limits`](crate::Limits) field would be passed.
    LimitExceeded(Limit),
    /// The allocator refused a size that is within the limits.
    OutOfMemory,
    /// The cancel flag was set.
    Cancelled,
    /// The reader of [`read`](crate::read) failed (the kind of the I/O error; an early end of
    /// the file is [`Error::Truncated`] instead).
    Io(std::io::ErrorKind),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFormat => f.write_str("unknown format"),
            Self::NoMeta => f.write_str("no meta box"),
            Self::NoMoov => f.write_str("no moov box"),
            Self::Truncated => f.write_str("truncated input"),
            Self::Malformed(what) => write!(f, "malformed {what}"),
            Self::Unsupported(what) => write!(f, "unsupported {what}"),
            Self::NoSuchItem(id) => write!(f, "no item {id}"),
            Self::LimitExceeded(limit) => write!(f, "limit exceeded: {limit:?}"),
            Self::OutOfMemory => f.write_str("out of memory"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::Io(kind) => write!(f, "i/o error: {kind}"),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// A short stable token per variant (`unknown-format`, `no-meta`, `no-moov`, `truncated`, `malformed`,
    /// `unsupported`, `no-such-item`, `limit-<field>`, `out-of-memory`, `cancelled`, `io`). The
    /// parity oracle writes it.
    pub fn class(&self) -> &'static str {
        match self {
            Self::UnknownFormat => "unknown-format",
            Self::NoMeta => "no-meta",
            Self::NoMoov => "no-moov",
            Self::Truncated => "truncated",
            Self::Malformed(_) => "malformed",
            Self::Unsupported(_) => "unsupported",
            Self::NoSuchItem(_) => "no-such-item",
            Self::LimitExceeded(limit) => match limit {
                Limit::Alloc => "limit-alloc",
                Limit::Boxes => "limit-boxes",
                Limit::Depth => "limit-depth",
                Limit::Items => "limit-items",
                Limit::Properties => "limit-properties",
                Limit::References => "limit-references",
                Limit::Extents => "limit-extents",
                Limit::GridTiles => "limit-grid-tiles",
                Limit::Pixels => "limit-pixels",
                Limit::MetaBytes => "limit-meta-bytes",
                Limit::MetadataBytes => "limit-metadata-bytes",
                Limit::MoovBytes => "limit-moov-bytes",
            },
            Self::OutOfMemory => "out-of-memory",
            Self::Cancelled => "cancelled",
            Self::Io(_) => "io",
        }
    }
}

/// Why the bytes of one item cannot be located. Item errors do not fail the parse: they are the
/// `Err` of [`Item::data`](crate::Item::data) and surface when that item's bytes are requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemError {
    /// The item has no entry in `iloc`.
    NoLocation,
    /// A data reference other than 0, an unknown construction method, or a construction method 2
    /// (item offset) item that has no single `iloc` reference or a source chain that is too long.
    Unsupported(&'static str),
    /// An extent overflows or ends past the file (or past `idat`).
    OutOfRange,
    /// Construction method 1 but the file has no `idat` box.
    NoIdat,
}

impl fmt::Display for ItemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoLocation => f.write_str("item has no iloc entry"),
            Self::Unsupported(what) => write!(f, "unsupported {what}"),
            Self::OutOfRange => f.write_str("item extent out of range"),
            Self::NoIdat => f.write_str("item data is in idat but the file has none"),
        }
    }
}

impl std::error::Error for ItemError {}

impl From<ItemError> for Error {
    fn from(e: ItemError) -> Self {
        match e {
            ItemError::NoLocation => Self::Malformed("iloc"),
            ItemError::Unsupported(what) => Self::Unsupported(what),
            ItemError::OutOfRange => Self::Truncated,
            ItemError::NoIdat => Self::Malformed("idat"),
        }
    }
}
