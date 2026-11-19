# Gain maps

A gain map is a second image that, with its metadata, turns the SDR base image into an HDR one
(or back). `Image::gain_map` returns it. Two sources exist.

## ISO 21496-1 (`GainMapSource::Tmap`)

A `tmap` derived item holds the metadata. Item payload (`Derived::Tmap`), big endian:

| Field | Width | Notes |
|---|---|---|
| version | u8 | must be 0, else `Derived::Tmap(None)` |
| minimum_version | u16 | above 0: unsupported, `Tmap(None)` |
| writer_version | u16 | |
| flags | u8 | bit 7 multichannel, bit 6 use_base_colour_space, bit 3 common denominator, bit 2 backward_direction |
| base headroom | u32 / u32 | numerator / denominator |
| alternate headroom | u32 / u32 | |
| per channel (1, or 3 if multichannel) | | |
| gain_map_min | s32 / u32 | |
| gain_map_max | s32 / u32 | |
| gamma | u32 / u32 | |
| base_offset | s32 / u32 | |
| alternate_offset | s32 / u32 | |

- Bit 3 set: one shared u32 denominator comes first, then only numerators.
- A single channel repeats channel 0 in channels 1 and 2.
- `Malformed("tmap")`: a zero denominator, a zero gamma numerator, max below min, or trailing bytes
  after a `writer_version` 0 payload (rules of libavif `gainmap.c`).
- The raw `flags` byte is kept. libultrahdr's encoder calls bit 3 reserved, libavif's reader does
  not accept it; we parse the superset.

Structure (AVIF and HEIF): the `tmap` item has two `dimg` references, `[base, gain map]`, and the
base is the image whose `gain_map` is set. An `altr` entity group lists the `tmap` item and the base.
`tmap`'s `ispe`, `colr`, `clli` and `pixi` describe the alternate (HDR) rendition. The gain map
item is normally hidden. The crate requires the `dimg` pair and the shared `altr` group.

## Apple (`GainMapSource::Apple`)

The gain map is an auxiliary image with auxC type `urn:com:apple:photo:2020:aux:hdrgainmap`,
linked to the master image by `auxl`. Its headroom is in XMP attached to the gain map image
(namespace `http://ns.apple.com/HDRGainMap/1.0/`) and in the Apple MakerNote of the master.

## What the crate returns

`GainMap { image, tmap, metadata, source, xmp }`:

- `image`: the gain map item id (decode it like any coded image).
- Tmap: `tmap` is the tmap item id and `metadata` the parsed `GainMapMetadata`
  (`None` for an unknown version).
- Apple: `tmap` and `metadata` are `None`; `xmp` lists the XMP item ids of the gain map. Read them
  with `Heif::item_bytes`; this crate does not parse Apple's XMP or MakerNote.
- A `tmap` item as an image is `ImageKind::Unsupported(tmap)`. If both sources are present, the
  tmap wins.
