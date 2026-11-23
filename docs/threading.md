# Threading

The crate never spawns a thread and has no thread pool. `Limits::max_threads` exists because every
dobbo-ca crate has it; here it is never read to start work. Every call runs on the calling thread
for every value, including 1, so a caller's own pool is never fought.

Why:

- A parse is a short serial walk over box headers and small tables. It does not touch pixel data
  (`mdat` is skipped), so there is nothing worth splitting.
- `read` copies only `ftyp` and `meta` (at most `max_meta_bytes`) and seeks over `mdat`; `read_movie` copies `ftyp` and `moov` (at most `max_moov_bytes`).
- Output is the same for every `max_threads` value; parity gate 3 checks `--threads 1` against
  the default.

Cancel: set the `&AtomicBool` from any thread; the call returns `Error::Cancelled`. It is checked
every 1024 boxes and before each item is read; a sample walk checks it at every table window refill and every 4096 samples.
