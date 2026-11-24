# isobmff-rs

Parses ISO-BMFF containers: HEIF (HEIC), AVIF (including the `mini` box) and the tracks of MP4/MOV/3GP/AVIS
files. It decodes no pixels. It gives the item graph of a file (primary image, tiles, alpha, depth, thumbnails,
gain map, Exif/XMP) and the keyframe byte ranges of a track; every coded item is a codec configuration record
plus byte ranges of the file. hevc-rs and av1-rs decode those bytes; exif-rs parses the Exif and XMP bytes.
Pure Rust, no dependencies, `#![forbid(unsafe_code)]`, bounded on hostile input. libheif, libavif and ffprobe
are the parity oracles in CI.

```toml
isobmff = { git = "https://github.com/dobbo-ca/isobmff-rs", rev = "3a6579a61b4a4b5eb535f297afa4a0899ed1afd6" }
```

## Quick start

```rust
use std::sync::atomic::AtomicBool;
use isobmff::{Error, ImageKind, Limits};

let limits = Limits { max_threads: 1, max_alloc: 64 << 20, ..Limits::default() };
let cancel = AtomicBool::new(false);
let heif = isobmff::parse(&file, &limits, &cancel)?;       // or isobmff::read(reader, ..)
let image = heif.primary_image()?;
println!("{}x{} shown as {:?}", image.width, image.height, image.display_size());
match &image.kind {
    ImageKind::Coded { codec, config } => {                 // config: hvcC or av1C
        let extents = &heif.item(image.id).ok_or(Error::NoSuchItem(image.id))?.data;
        println!("{codec:?} {config:?} {extents:?}");        // absolute file ranges
        let bytes = heif.item_bytes(&file, image.id, &limits, &cancel)?; // the coded bytes
    }
    ImageKind::Grid(grid) => println!("{} tiles: {:?}", grid.tiles.len(), grid.tiles), // row-major
    _ => {}
}
let meta = heif.metadata(&file, image.id, &limits, &cancel)?; // meta.exif (TIFF bytes), meta.xmp

// MP4/MOV: the byte ranges of the first 3 keyframes (docs/design.md, "MP4/MOV tracks")
let movie = isobmff::parse_movie(&file, &limits, &cancel)?;   // or isobmff::read_movie(reader, ..)
let track = movie.video_track().ok_or(Error::Unsupported("no video track"))?;
let config = track.entries.first().and_then(|e| e.config()); // hvcC / av1C / avcC / vvcC
let first = track.sync_samples(std::io::Cursor::new(&file[..]), 3, &limits, &cancel)?;
```

## Resource limits

`parse`, `read`, `item_bytes`, `metadata`, `parse_movie`, `read_movie` and the sample walks of `Track` take `&Limits` and a cancel flag (`&AtomicBool`); there is no unlimited entry point. Details and rationale: [docs/limits.md](docs/limits.md).

| Field | Default | Unit | Error (`Limit::...`) |
|---|---|---|---|
| `max_alloc` | 256 MiB | bytes, enforced peak | `Alloc` |
| `max_threads` | `available_parallelism` | threads | none |
| `max_boxes` | 1 000 000 | boxes per call | `Boxes` |
| `max_depth` | 32 | box nesting | `Depth` |
| `max_items` | 65 535 | items (infe, iloc, ipma) | `Items` |
| `max_properties` | 65 535 | ipco children, or sample entry children of all tracks | `Properties` |
| `max_references` | 1 000 000 | iref `to` ids + grpl entities | `References` |
| `max_extents` | 1 000 000 | iloc extents; work of construction method 2 | `Extents` |
| `max_grid_tiles` | 65 536 | tiles of a grid, inputs of an iovl | `GridTiles` |
| `max_pixels` | 1 << 30 | width x height | `Pixels` |
| `max_meta_bytes` | 64 MiB | bytes of `meta` (or `mini`) copied by `read` | `MetaBytes` |
| `max_metadata_bytes` | 64 MiB | bytes of one Exif or XMP item | `MetadataBytes` |
| `max_moov_bytes` | 64 MiB | bytes of `moov`, and of each `moof`, copied by `read_movie` | `MoovBytes` |

Each limit returns `Error::LimitExceeded(Limit::...)`. No thread is ever spawned. Set the cancel flag from another thread: the call returns `Error::Cancelled`.

## Docs

[design](docs/design.md), [parity](docs/parity.md), [limits](docs/limits.md), [threading](docs/threading.md),
[safety](docs/safety.md), [fuzzing](docs/fuzzing.md), [benchmarks](docs/benchmarks.md),
[dependencies](docs/dependencies.md), [testing](docs/testing.md), [gain maps](docs/gainmap.md), [corpus](docs/corpus.md),
[transforms](docs/transforms.md) (`apply_transforms`: clap, irot, imir on decoded planes).
