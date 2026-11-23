# Safety

## No unsafe, no SIMD

`#![forbid(unsafe_code)]` is set in `src/lib.rs`. A container parser has no SIMD kernel and no hot
loop that needs `unsafe`: it reads a few kilobytes of headers and tables, and the cost is dominated
by box walking, not arithmetic. So there is no `ISOBMFF_SIMD` variable, no tier dispatch and no
tier tests. The crate has no `unsafe` inventory because it has no `unsafe`.

## Hostile input

- Non-test code denies `clippy::unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`,
  `indexing_slicing` and `arithmetic_side_effects`. Reads use `get`, `checked_*` and
  `split_at_checked`.
- Never trust a size: every box size, extent and count is checked against its parent, the input
  length and `Limits` before use. Size arithmetic on input values is checked or done in wider
  integers (`Clap::crop` uses `i128`).
- Never allocate from an input size before it passes `Limits`: every `Vec` sized by the input is
  made by `Budget::vec` (charge, then `try_reserve_exact`). A failed reservation is
  `Error::OutOfMemory`.
- Recursion is bounded by `max_depth`; every walk is bounded by `max_boxes` and checks the cancel flag.
- A bad item (extent past the end of the file, a method-2 item with no single `iloc` reference, too long a chain or a cycle) is an `ItemError`
  in `Item::data` and fails only that item's bytes. A bad property is `PropValue::Invalid` and fails
  only the images that use it. Neither fails the parse.
- Strings from the file are UTF-8; invalid bytes become U+FFFD.
- Fuzzing (docs/fuzzing.md) runs with `-rss_limit_mb=2048 -malloc_limit_mb=512`, so an allocation
  outside the limits is a crash.
