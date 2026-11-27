//! The charge ledger for [`Limits::max_alloc`]: every buffer whose size comes from the input is
//! charged here before it is allocated. Same shape as exif-rs `src/budget.rs`.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Error, Limit, Limits};

/// One per public call. Not `Sync`: the crate runs on the calling thread only.
pub(crate) struct Budget<'a> {
    used: Cell<usize>,
    max: usize,
    cancel: &'a AtomicBool,
}

impl<'a> Budget<'a> {
    pub(crate) fn new(limits: &Limits, cancel: &'a AtomicBool) -> Self {
        Self::with_max(limits.max_alloc, cancel)
    }

    /// A ledger of `max` bytes: for a call that takes no `Limits` and uses the `max_alloc` of
    /// the parse.
    pub(crate) fn with_max(max: usize, cancel: &'a AtomicBool) -> Self {
        Self {
            used: Cell::new(0),
            max,
            cancel,
        }
    }

    /// Charges `n` bytes, or fails with `LimitExceeded(Alloc)` and charges nothing.
    pub(crate) fn charge(&self, n: usize) -> Result<(), Error> {
        let used = self
            .used
            .get()
            .checked_add(n)
            .filter(|&u| u <= self.max)
            .ok_or(Error::LimitExceeded(Limit::Alloc))?;
        self.used.set(used);
        Ok(())
    }

    /// Gives back `n` bytes charged before (a temporary buffer that was dropped).
    pub(crate) fn release(&self, n: usize) {
        self.used.set(self.used.get().saturating_sub(n));
    }

    /// An empty `Vec` with room for `n` items, charged first.
    pub(crate) fn vec<T>(&self, n: usize) -> Result<Vec<T>, Error> {
        let bytes = n
            .checked_mul(size_of::<T>())
            .ok_or(Error::LimitExceeded(Limit::Alloc))?;
        self.charge(bytes)?;
        let mut v = Vec::new();
        if v.try_reserve_exact(n).is_err() {
            self.release(bytes);
            return Err(Error::OutOfMemory);
        }
        Ok(v)
    }

    /// A charged copy of `src`.
    pub(crate) fn copy(&self, src: &[u8]) -> Result<Vec<u8>, Error> {
        let mut v = self.vec(src.len())?;
        v.extend_from_slice(src);
        Ok(v)
    }

    /// A charged `String` of `src`; bytes that are not UTF-8 become U+FFFD.
    pub(crate) fn string(&self, src: &[u8]) -> Result<String, Error> {
        let v = self.copy(src)?;
        match String::from_utf8(v) {
            Ok(s) => Ok(s),
            Err(e) => {
                // The lossy copy is at most 3 bytes per input byte.
                self.charge(src.len().saturating_mul(3))?;
                Ok(String::from_utf8_lossy(e.as_bytes()).into_owned())
            }
        }
    }

    /// A charged shared string of `src` (see [`string`](Self::string)).
    pub(crate) fn arc_str(&self, src: &[u8]) -> Result<Arc<str>, Error> {
        let s = self.string(src)?;
        self.charge(s.len())?;
        Ok(Arc::from(s))
    }

    /// `Err(Cancelled)` when the cancel flag is set.
    pub(crate) fn check_cancel(&self) -> Result<(), Error> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}
