use std::sync::atomic::{AtomicU32, Ordering};

use super::err::{Error, MMFResult};

/// Struct that serves as four packed u8 values, to indicate init, read and write locking on an MMF.
/// The wrapper implementation must set these bytes depending on the situation and actions being taken.
/// Due to the fact Windows only guarantees atomic operations on 32-bit ints, (and 64-bit ones only for 64-bit
/// applications on 64-bit Windows), the safest option here is to ensure we're using a 32-bit Atomic.
/// For all your safety and happy unicorn concerns, the default ordering is [`Ordering::SeqCst`]. This can be changed if
/// consumers so desire, but considering the nature of IPC mechanisms it was deemed logical to use a single total
/// modification order where possible.
///
/// This atomic will be split into the following data:
/// - Highest byte: initialization state; if this is non-zero, the data portion is not initialized.
/// - Second byte: writelock; if this is non-zero, do not attempt to read or write.
/// - Third byte: Readers; don't write if it's non-zero. Increment when locking, decrement when unlocking.
/// - Fourth byte: innocent padding, consumers can use this for their own use cases.
///
/// The choice to limit readers was made for two reasons. First off, there's no situation in which you'd expect more
/// than 255 concurrent readers. Secondly, the initial design did not use atomics but this leads to soundness concerns
/// across the application boundary. The atomic approach means either using 15 bits for storing readers and manually
/// accounting for overflow into the write bit, or it means limiting readers so bitops can be used instead. As the
/// latter is easier to maintain, and we have the spare bandwidth on account of atomic guarantees being exclusively for
/// pointer sizes, that's the chosen approach. Leaving the 4th byte empty is an extension of that choice; 4 bytes is
/// easier to work with than two bytes and a word.
#[derive(Debug)]
pub struct RWLock<'a> {
    /// An Atomic reference. Alignment is usually not an issue considering Windows aligns views to pointers by default.
    chunk: &'a AtomicU32,
    /// [`Ordering`] to use for load ops.
    load_order: Ordering,
    /// [`Ordering`] to use for store ops.
    store_order: Ordering,
}

impl<'a> RWLock<'a> {
    /// Construct a lock from existing pointers.
    /// This is meant to be used with some external mechanism to allow reading and writing lock state directly to and
    /// from some larger struct. The lock will claim the first four bytes behind this pointer; if you do not intend to
    /// have the lock state at the start of the data, make sure to offset the pointer provided.
    ///
    /// SAFETY: there is no way of ensuring this pointer is valid after the first byte without moving out of bounds when
    /// it's not. Users should take care to ensure the first 4 bytes in the pointer are valid. A lock created through
    /// this method provides NO guarantees about the lock states and assumes the user has zeroed out all values behind
    /// it if necessary. This initializer is meant to be used with a pointer for an existing lock, or a pointer whose
    /// values you know will provide correct results. If the data behind the pointer is wrong, this effectively
    /// constructs a poisoned lock.
    /// It _is_ safe to assume the size and alignment are valid on Windows, however, as pointers are 0x4/0x4 or 0x8/0x8
    /// depending on 32-bit or 64-bit. Either is safe for use with AtomicU32 which is 0x4/0x4 on these platforms.
    ///
    /// # Panics
    /// This function _will_ panic if called with a null pointer; ensuring initialization is hard, but ensuring non-null
    /// should not prove difficult to anyone working with raw pointers.
    pub unsafe fn from_existing(pointer: *mut u8) -> Self {
        if pointer.is_null() {
            panic!("Never, ever pass a null pointer into a lock!")
        }
        Self { chunk: AtomicU32::from_ptr(pointer.cast()), load_order: Ordering::SeqCst, store_order: Ordering::SeqCst }
    }

    /// Similar to [`Self::from_existing`], except it clears all state and ensures [`Self::initialized`] returns false.
    /// The same safety bounds apply as for `from_existing` with the exception of poisoned lock risks. It does mean,
    /// however, that it invalidates any other locks that use the same pointer and clears any data in the last byte.
    pub unsafe fn from_raw(pointer: *mut u8) -> Self {
        let lock = Self::from_existing(pointer);
        lock.chunk.store(255 << 24, Ordering::SeqCst);
        lock
    }

    #[allow(dead_code)]
    /// Create a copy of this lock with the specified load and store orders.
    ///
    /// Only allows for the usual valid operation orders and _will_ panic if either load or store is passed a wrong
    /// value. Refer to [`Ordering`] for which values are allowed for which operations. The panic choices were made to
    /// be the same as what is done in the source for `core::sync::atomic.rs` to prevent getting the same panics on
    /// access instead of initialization.
    pub fn set_ordering(&mut self, load: Ordering, store: Ordering) {
        match (load, store) {
            (Ordering::Release, _) => panic!("there is no such thing as release load for atomics"),
            (_, Ordering::Acquire) => panic!("there is no such thing as an acquire store"),
            (Ordering::AcqRel, _) | (_, Ordering::AcqRel) => panic!("there is no such thing as an acquire-release load/store and this struct offers no combined operations."),
            _ => {
                self.load_order = load;
                self.store_order = store;
            }
        };
    }

    /// Checks if this lock is readlocked. Does not indicate writelock status; use [`Self::writelocked()`] for that.
    #[inline(always)]
    pub fn readlocked(&self) -> bool {
        (self.chunk.load(Ordering::SeqCst) & ((u8::MAX as u32) << 8)) > 0
    }

    /// Checks if this lock is writelocked. Use this to wait before reading.
    #[inline(always)]
    pub fn writelocked(&self) -> bool {
        (self.chunk.load(Ordering::SeqCst) & ((u8::MAX as u32) << 16)) > 0
    }

    /// Checks if there are any acitve locks, including the initialization locks.
    #[inline(always)]
    pub fn locked(&self) -> bool {
        self.chunk.load(Ordering::SeqCst) > 255
    }

    /// Check if the data this lock is has been initialized for use
    #[inline(always)]
    pub fn initialized(&self) -> bool {
        (self.chunk.load(Ordering::SeqCst) & (255 << 24)) == 0
    }

    /// Mark this lock as initialized. This will clear any existing lock state, so make sure no locks are taken.
    /// The last byte is left as-is, so it is possible to store custom data before initialization.
    /// The choice to clear all locks upon setting the init state was made to accommodate uses of
    /// [`Self::from_existing`] where it's reasonable to assume no locks are taken or the code using it handles the
    /// situation where the locks are cleared internally.
    ///
    /// **SAFETY**: the caller is responsible for ensuring no problems arise elsewhere. This method does not do anything
    /// unsafe internally, but if care is not taken it might cause concurrent writes or allow readers while a write is
    /// in progress due to resetting lock state!
    pub unsafe fn set_init(&self) {
        self.chunk.store(self.chunk.load(Ordering::SeqCst) & (u8::MAX as u32), Ordering::SeqCst)
    }

    /// Thin wrapper around [`Self::set_init`] that returns self for chaining calls.
    /// The same safety concerns apply as for `set_init`.
    pub unsafe fn initialize(self) -> Self {
        self.set_init();
        self
    }

    /// Acquire a readlock, if at all possible. Otherwise error.
    pub fn lock_read(&self) -> MMFResult<()> {
        if !self.initialized() {
            return Err(Error::Uninitialized);
        } else if self.writelocked() {
            Err(Error::WriteLocked)
        } else {
            let mut bytes = self.split_lock();
            bytes
                .2
                .checked_add(1)
                .map(|readlock| {
                    bytes.2 = readlock;
                    self.merge_lock(bytes)
                })
                .ok_or(Error::MaxReaders)
        }
    }

    /// Release a readlock, clearing the readlock state if this was the last lock.
    pub fn unlock_read(&self) -> MMFResult<()> {
        if !self.initialized() {
            Err(Error::Uninitialized)
        } else if self.readlocked() {
            let mut bytes = self.split_lock();
            bytes
                .2
                .checked_sub(1)
                .map(|readlock| {
                    bytes.2 = readlock;
                    self.merge_lock(bytes)
                })
                .ok_or(Error::GeneralFailure)
        } else {
            Ok(())
        }
    }

    /// Lock this file for writing if possible. If a read lock exists and the read lock counter is zero, this will clear
    /// the lock. If any actively acquired lock is present, fail gracefully.
    pub fn lock_write(&self) -> MMFResult<()> {
        if !self.initialized() {
            Err(Error::Uninitialized)
        } else if self.writelocked() {
            Err(Error::WriteLocked)
        } else if self.readlocked() {
            Err(Error::ReadLocked)
        } else {
            let mut bytes = self.split_lock();
            bytes.1 = 1;
            Ok(self.merge_lock(bytes))
        }
    }

    /// Nuke all existing write locks as there can only be one, legally.
    /// Note that this should probably not be considered safe to use with multiple writers.
    pub fn unlock_write(&self) {
        let mut bytes = self.split_lock();
        if bytes.1 != 0 {
            bytes.1 = 0
        }
        self.merge_lock(bytes)
    }

    /// Takes the [`u32`] from the lock and provides it as 4 [`u8`]s.
    #[inline(always)]
    fn split_lock(&self) -> (u8, u8, u8, u8) {
        let lock = self.chunk.load(Ordering::SeqCst);
        ((lock >> 24) as u8, (lock >> 16) as u8, (lock >> 8) as u8, lock as u8)
    }

    /// Takes 4 [`u8`]s and packs them together into a [`u32`] to shove them into the lock.
    #[inline(always)]
    fn merge_lock(&self, bytes: (u8, u8, u8, u8)) {
        let lock = (bytes.0 as u32) << 24 & (bytes.1 as u32) << 16 & (bytes.2 as u32) << 8 & bytes.3 as u32;
        self.chunk.store(lock, Ordering::SeqCst)
    }
}
