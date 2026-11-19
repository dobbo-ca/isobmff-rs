# Limits

`parse`, `read`, `item_bytes` and `metadata` take `&Limits` and a cancel flag; the `Heif` graph accessors do not (they work on the bounded graph). Each cap has its own `Limit` variant;
exceeding it gives `Error::LimitExceeded(Limit::X)` (`Error::class()`: `limit-` plus the kebab-case
name, for example `limit-grid-tiles`). Allocations from input sizes are charged to `max_alloc`
before they happen (`Budget::vec`).

| Field | Default | Counts | libheif 1.23.7 default |
|---|---|---|---|
| `max_alloc` | 256 MiB | peak bytes of every input-sized allocation of one call | max_total_memory 4 GiB |
| `max_threads` | `available_parallelism()` | never spawns (docs/threading.md) | - |
| `max_boxes` | 1 000 000 | boxes visited in one call, all levels | max_children_per_box 100 (per box) |
| `max_depth` | 32 | box nesting depth, top level = 0 | - |
| `max_items` | 65 535 | `infe` entries, `iloc` items, `ipma` entries (each count) | max_items 1000 |
| `max_properties` | 65 535 | `ipco` children; associations per call / 16 | max_children_per_box 100 |
| `max_references` | 1 000 000 | all `to` ids of all `iref` boxes plus all `grpl` entity ids | max_items 1000 per iref box |
| `max_extents` | 1 000 000 | iloc extents, all items together | max_iloc_extents_per_item 32 |
| `max_grid_tiles` | 65 536 | rows x columns of a grid; inputs of an iovl | max_number_of_tiles 4096 x 4096 |
| `max_pixels` | 1 << 30 | ispe, grid and iovl width x height | max_image_size_pixels 32768 x 32768 |
| `max_meta_bytes` | 64 MiB | size of the `meta` box (and of the `ftyp` payload) that `read` copies; `parse` has no such cap | - |
| `max_metadata_bytes` | 64 MiB | one Exif or XMP item returned by `metadata()` | max_memory_block_size 4 GiB |

## Why these values

- Real phone files are big graphs: an iPhone 48 MP photo has one grid with 48 `dimg` references
  (8 x 6 tiles of 512), plus thumbnails, depth, a gain map and its own grid. 200 MP phone files
  have about 200 tiles.
- The defaults are far above every corpus file (see "Corpus maxima") and bound memory: 65 535 items
  at about 200 bytes each is about 13 MiB.
- `max_pixels` equals libheif's default: 32768 x 32768.

## Compared with libheif

libheif's per-box caps (100 children per box, 32 extents per item, 1000 references per iref box)
are lower than ours on purpose: a valid file with more must parse. libvips 16 sets the iref
reference cap to 16, which rejects iPhone files with many `dimg` references (libheif issue
#1845). We keep the cap high and bound memory with `max_alloc` and the total counts instead.

## Corpus maxima

The parity run (`summary.md`, from `oracle stats`) prints the largest value in the corpus for each
count (parity run, 643 files).

| Count | Corpus maximum | Limit |
|---|---|---|
| items | 51 | 65 535 |
| references (one iref entry) | 48 | 1 000 000 (all boxes together) |
| grid tiles | 48 | 65 536 |
| extents (one item) | 2 | 1 000 000 (all items together) |
| boxes | 77 | 1 000 000 |
| box depth | 3 | 32 |
| properties | 14 | 65 535 |
