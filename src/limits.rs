/// Caps on one call. Every public entry point takes `&Limits` and a cancel flag; there is no
/// unlimited entry point. The defaults are finite and far above every real phone and camera
/// file (an iPhone 48 MP file has 48 tiles in one grid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The peak working memory of one call, in bytes (default 256 MiB): every buffer whose size
    /// comes from the input (items, properties, references, extents, the copy of `meta` that
    /// [`read`](crate::read) makes, metadata copies) is charged before it is allocated. More is
    /// [`Error::LimitExceeded`](crate::Error::LimitExceeded)`(`[`Limit::Alloc`](crate::Limit::Alloc)`)`.
    pub max_alloc: usize,
    /// The most OS threads one call uses (default `std::thread::available_parallelism()`). This
    /// crate never spawns a thread: every call runs on the calling thread, for every value. The
    /// field exists so that a caller sets the same `Limits` fields on every dobbo-ca crate.
    pub max_threads: usize,
    /// The most boxes visited in one call, all levels together (default 1 000 000).
    /// [`Limit::Boxes`](crate::Limit::Boxes).
    pub max_boxes: usize,
    /// The deepest box nesting, top level = 0 (default 32). [`Limit::Depth`](crate::Limit::Depth).
    pub max_depth: usize,
    /// The most `infe` entries, `iloc` items and `ipma` entries, each counted alone (default
    /// 65 535). [`Limit::Items`](crate::Limit::Items).
    pub max_items: usize,
    /// The most `ipco` children (default 65 535); the associations of one call are capped at 16
    /// times this. [`Limit::Properties`](crate::Limit::Properties).
    pub max_properties: usize,
    /// The most `to` ids of all `iref` boxes plus entity ids of `grpl`, together (default
    /// 1 000 000). [`Limit::References`](crate::Limit::References).
    pub max_references: usize,
    /// The most `iloc` extents, all items together (default 1 000 000).
    /// [`Limit::Extents`](crate::Limit::Extents).
    pub max_extents: usize,
    /// The most tiles of one grid (rows x columns) and inputs of one overlay (default 65 536).
    /// [`Limit::GridTiles`](crate::Limit::GridTiles).
    pub max_grid_tiles: usize,
    /// The most pixels of one image: `ispe` width x height and grid or overlay output size
    /// (default `1 << 30`, 32768 x 32768). [`Limit::Pixels`](crate::Limit::Pixels).
    pub max_pixels: u64,
    /// The largest `meta` box or `ftyp` payload that [`read`](crate::read) copies (default 64 MiB).
    /// [`Limit::MetaBytes`](crate::Limit::MetaBytes).
    pub max_meta_bytes: usize,
    /// The largest Exif or XMP item that [`Heif::metadata`](crate::Heif::metadata) returns
    /// (default 64 MiB). [`Limit::MetadataBytes`](crate::Limit::MetadataBytes).
    pub max_metadata_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_alloc: 256 << 20,
            max_threads: std::thread::available_parallelism().map_or(1, usize::from),
            max_boxes: 1_000_000,
            max_depth: 32,
            max_items: 65_535,
            max_properties: 65_535,
            max_references: 1_000_000,
            max_extents: 1_000_000,
            max_grid_tiles: 65_536,
            max_pixels: 1 << 30,
            max_meta_bytes: 64 << 20,
            max_metadata_bytes: 64 << 20,
        }
    }
}
