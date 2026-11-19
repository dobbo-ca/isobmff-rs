//! `bench [--threads N] <iters> <in-dir>`: for every regular file below `<in-dir>`, the median of
//! `iters` timed runs of parse, a walk of all items, references and properties, `primary_image`
//! and every top-level image. TSV on stdout: `name<TAB>median ns`. Name = relative path with `/`
//! replaced by `__`. A file that does not parse is skipped (the C oracles skip it too).

use isobmff::{Heif, Limits};
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for p in paths {
        if std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir()) {
            walk(root, &p, out);
        } else if std::fs::metadata(&p).is_ok_and(|m| m.is_file()) {
            let Ok(rel) = p.strip_prefix(root) else {
                continue;
            };
            let parts: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push((parts.join("__"), p));
        }
    }
}

/// Touches everything the parse produced; the sum keeps the work observable.
fn touch(h: &Heif) -> u64 {
    let mut acc = u64::from(h.primary) + h.stats.boxes as u64;
    for it in &h.items {
        acc += u64::from(it.id) + u64::from(u32::from_be_bytes(it.item_type.0));
        acc += u64::from(it.hidden) + it.name.len() as u64 + it.content_type.len() as u64;
        acc += it.associations.len() as u64;
        if let Ok(ex) = &it.data {
            acc += ex.iter().map(|e| e.length).sum::<u64>();
        }
    }
    for r in &h.references {
        acc += u64::from(r.from) + r.to.len() as u64;
    }
    for p in &h.properties {
        acc += p.range.length;
    }
    for g in &h.groups {
        acc += g.entities.len() as u64;
    }
    if let Ok(img) = h.primary_image() {
        acc += u64::from(img.width) + img.aux.len() as u64;
    }
    for id in h.top_level_images() {
        if let Ok(img) = h.image(id) {
            acc += u64::from(img.id) + u64::from(img.height) + img.transforms.len() as u64;
        }
    }
    acc
}

fn run(data: &[u8], lim: &Limits, cancel: &AtomicBool) -> Option<u64> {
    let h = isobmff::parse(data, lim, cancel).ok()?;
    Some(touch(&h))
}

fn usage() -> ! {
    eprintln!("usage: bench [--threads N] <iters> <in-dir>");
    std::process::exit(2)
}

fn main() {
    let mut lim = Limits::default();
    let mut pos = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--threads" {
            lim.max_threads = args
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| usage());
        } else {
            pos.push(a);
        }
    }
    let [iters, dir] = &pos[..] else { usage() };
    let iters: usize = iters.parse().unwrap_or_else(|_| usage());
    let iters = iters.max(1);
    let root = Path::new(dir);
    let mut files = Vec::new();
    walk(root, root, &mut files);
    let cancel = AtomicBool::new(false);
    for (name, path) in files {
        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        if run(&data, &lim, &cancel).is_none() {
            continue;
        }
        let mut times: Vec<u128> = Vec::with_capacity(iters);
        for _ in 0..iters {
            let t = Instant::now();
            black_box(run(black_box(&data), &lim, &cancel));
            times.push(t.elapsed().as_nanos());
        }
        times.sort_unstable();
        println!(
            "{name}\t{}",
            times.get(times.len() / 2).copied().unwrap_or(0)
        );
    }
}
