# Transforms

`isobmff::apply_transforms` applies the `clap`, `irot` and `imir` properties of an image to decoded
planes. hevc-rs, av1-rs and gainmap-rs decode; they call this and never write their own crop or
rotation. The parity oracle (libheif, CI only) compares the planes exactly, except where libheif
changes the chroma format (below).

```rust
let image = heif.primary_image()?;
let out = isobmff::apply_transforms(planes, &image.transforms, &limits, &cancel)?;
let size = isobmff::transformed_size(image.width, image.height, &image.transforms, &limits)?; // no planes
```

Types: `Planes { width, height, chroma, planes }`, `Plane { width, height, samples }`,
`Chroma { Mono, Yuv420, Yuv422, Yuv444 }` and the sample type `isobmff::transform::Samples { U8,
U16 }`. It is also `isobmff::PlaneSamples`, because `isobmff::Samples` is the sample walk of a
track. Rows are packed (stride = width). Each plane picks 8 or 16 bits on its own (9 to 16-bit
values are in the low bits); the bit depth does not matter to the geometry.

## Layout of the input

`planes[0]` is luma, `width x height`. Unless `Mono`, `planes[1]` and `planes[2]` are chroma,
`ceil(width/sx) x ceil(height/sy)` (`sx, sy` = 2, 2 for 4:2:0; 2, 1 for 4:2:2; 1, 1 for 4:4:4).
Every further plane (alpha, depth) is full size. Anything else, a zero size, or a sample count
that is not `width * height` is `Malformed("planes")`; nothing panics.

## Order

Transforms run in slice order: the ipma order that `Image::transforms` gives. libheif does the same
and does not enforce the MIAF order (clap, irot, imir). Each `clap` is computed on the size after
the transforms before it. `irot` is anticlockwise (90: output `[y][x]` = input `[x][w-1-y]`).
`imir` with `Axis::TopBottom` swaps rows, `Axis::LeftRight` swaps columns. An empty list returns
the input unchanged, without a copy. `transformed_size` gives the size with the same rules and no
planes; `Image::display_size` uses the same rule, except that a transform that `apply_transforms`
rejects is left out of it (it returns `Option`, not `Result`).

Known difference: libheif rejects a second `irot`, `imir` or `clap` on one item (also with
`ignore_transformations`). `Image::transforms` keeps every property as stored (the metadata rule),
and `apply_transforms` and `transformed_size` apply them all, in order, so `[Rotate(90),
Rotate(90)]` gives pixels where libheif gives an error. Valid files have at most one of each.

Derived images: apply each input's own transforms to that input before it is used (an `iden`
input, each grid tile, each overlay input), then the derived item's transforms to the result.
libheif does this (it decodes inputs with the caller's options: `grid.cc:543`, `iden.cc:98`,
`overlay.cc:392`), and a grid pastes its tiles at their size after their transforms. An `iden`
chain is applied innermost first.

## Clap rounding

`Clap::crop_rounded(width, height)` is the rule of libheif 1.23.7 (`Box_clap::get_crop` and the
clamping in the decoder), read as a design reference, with exact `i128` fractions. With `W x H`
the size at that point, `clapW = widthN/widthD`, and so on:

```
left   = trunc(offX + (W-1)/2 - (clapW-1)/2)        right  = round((clapW-1) + left)
top    = round(offY + (H-1)/2 - (clapH-1)/2)        bottom = round((clapH-1) + top)
trunc(n/d) = n/d toward zero;  round(n/d) = (n + d/2)/d, truncating
```

`right` and `bottom` use the unclamped `left` and `top`. Then `left` and `top` are raised to 0; a
`right` or `bottom` that is negative or at least `W` or `H` becomes `W-1` or `H-1`; `left > right`
or `top > bottom` gives `None`. libheif's `uint32` comparison is why a rectangle entirely left of
or above the image gives the full width or height rather than an error; we do the same so that the
planes match. A rectangle entirely right of or below the image is `Err(Malformed("clap"))`.

Differences from `Clap::crop` (libavif rule): `crop` rejects a fractional origin and a rectangle
that is not inside the image; `crop_rounded` rounds and clamps. `crop` is unchanged.

Hostile values: a zero denominator, a denominator or width or height numerator above `i32::MAX`
(libheif rejects them at parse), a clap width or height that rounds to below 1, and `W` or `H` of 0
or above 2^31 give `None`. The math is checked `i128`, so `i32::MIN` offsets and the largest
denominators cannot overflow. libheif's own `Fraction` loses precision for a denominator above
65 536, a numerator above 65 536 with a denominator above 1, `W-1` above 65 536 and intermediate
products beyond `i32`; we do not, so the result can differ from libheif there.

## Chroma

- Odd crop: the chroma format is kept. The chroma rectangle starts at `floor(x/sx), floor(y/sy)` and
  is `ceil(w/sx) x ceil(h/sy)`, which always fits the chroma plane. For an odd offset the chroma
  shifts by one luma sample. Why: no sample is invented, memory stays a crop, and hevc-rs (chroma
  format Mono or 4:2:0 only) can still hold the result. libheif converts to 4:4:4 (bilinear) when
  the left offset is odd (4:2:2) or the left or top offset is odd (4:2:0); that is the documented
  known difference of the oracle.
- 4:2:2 with `irot` 90 or 270: no 4:2:2 result exists (the chroma would be half the height). The chroma
  planes are made 4:4:4 by copying each sample into two columns, trimmed to the luma width, as they
  are rotated; the output chroma is `Yuv444`. libheif converts to 4:4:4 with bilinear upsampling, so
  it differs from this by the interpolation.
- Other odd sides: each plane is processed at its own size, so the format is kept and the chroma
  moves by one luma sample on that axis (none for co-sited chroma). libheif converts to 4:4:4 in these cases (the known
  difference): 4:2:0 rotate 90 with odd width, rotate 270 with odd height, rotate 180 and mirror
  (either axis) with an odd width or height; 4:2:2 rotate 180 with odd height and a left/right
  mirror with odd width. libheif keeps 4:2:2 for a top/bottom mirror and for rotate 180 with odd
  width, where it matches the helper exactly.
- Alpha, depth and gain-map planes: pass the transforms of the item that the plane belongs to, not
  those of the main image. The exception is an alpha plane without an alpha item of the image (the
  alpha items belong to the tiles of a grid): libheif assembles it into the grid image and applies
  the grid's transforms to it, so keep it as an extra plane of the main call. libheif decodes an alpha or auxiliary item with its own `ipma` list only;
  an alpha plane of another size is scaled by the caller before it is attached as an extra plane
  (libheif: nearest neighbour). A gain map is an item of its own: `Image::gain_map` and its source
  image's `transforms`.

## Limits and cancel

- `max_pixels`: the input size and the output size, checked before any work (not for an empty
  list); `transformed_size` takes `&Limits` and checks both.
- `max_alloc`: every new buffer is charged before it is allocated and given back after the step
  that replaces it. Rotation by 90 or 270 and a crop allocate; mirrors and 180 work in place. The
  input planes belong to the caller and are not charged. The peak of `[Rotate(90), Rotate(90)]` is
  therefore twice the planes; 4:2:2 rotated needs three full planes.
- Cancel: checked between steps, once per 64 x 64 tile of a rotation, and every 64 rows elsewhere.
- No thread is spawned, for any `max_threads`.
- A 90 or 270 degree rotation is cache-blocked (64 x 64 tiles): the output is written contiguously
  and the input is read with a stride only inside a tile. Measure with `bench --transform <iters>`.
