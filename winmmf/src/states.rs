#![deny(clippy::missing_docs_in_private_items)]
#![deny(missing_docs)]
//! # States and Locks for MMFs
//!
//! These are the cursed things required to prevent you from footgunning yourself. When not using the default lock
//! implementation, you'll need to implement locking yourself. If you're down for that, good luck and read on.
//! If you're not, you can skip reading this module's documentation. Just use the `impl_lock` feature and enjoy life.
//!
//! The [`MMFLock`] trait is all you'll really care about. It tells you the things the main
//! [`MemoryMappedFile`][crate::mmf::MemoryMappedFile] wants to call so it can safely do its thing. The [`RWLock`]
//! struct provides a way to implement that stuff, as well as some sprinkled on additions of its own relevant to how
//! this lock was designed. Things to note are that any one instance of an [`RWLock`] does not do internal bookkeeping
//! to see if holds any locks itself. This means you should _never_ call the `unlock_*` functions for locks you didn't
//! request yourself. The functions are public only because they need to be implemented and called from the MMF wrapper.
//! The lock _does_ ensure it never claims a lock when it can't, e.g. no readlocks will be taken while a writelock is
//! held. This means that if you want to use MMFs in a place where several things are touching the memory at the same
//! time, you'll deal with errors. Luckily these are usually just abstractions that tell you all is well, unless things
//! go very wrong. And in that case, good luck.
//!
//! The errors in this crate still need some work. They're in active development.
//!
//! Assuming you're comfortable waiting on the locks to be claimable, [`MMFLock::spin_and_lock_read`] and
//! [`MMFLock::spin_and_lock_write`] will be your friends, as you'd only need to handle the case where you spin more
//! than what your native pointer size holds, and you should be seeing problems long before then.
//!
//! No guarantees are made about the usefulness and safety of this code, and the project maintainer is not liable for
//! any damages, be they to your PC or your (mental) health.

use std::sync::atomic::{fence, AtomicU32, Ordering};

use super::err::{Error, MMFResult};

/// Blanket trait for implementing locks to be used with MMFs.
///
/// The default implementation applied to [`RWLock`] can be used with a custom MMF implementation,
/// but either way would require accounting for the fact this lock is designed to be stored inside the MMF.
/// From the lock's point of view, a pointer to some other u32 would work just as well but this requires some other
/// form of synchronizing access accross thread and application boundaries.
///
/// Users are free to use the default lock and MMF implementations independently of one another.
pub trait MMFLock {
    /// Acquire a readlock, if at all possible. Otherwise error.
    fn lock_read(&self) -> MMFResult<()>;
    /// Release a readlock, clearing the readlock state if this was the last lock.
    fn unlock_read(&self) -> MMFResult<()>;
    /// Lock this file for writing if possible.
    fn lock_write(&self) -> MMFResult<()>;
    /// Nuke all existing write locks as there can only be one, legally.
    fn unlock_write(&self) -> MMFResult<()>;
    /// Check if the lock is initialized
    fn initialized(&self) -> bool;
    /// Spin until the lock can be taken, then take it.
    fn spin_and_lock_read(lock: &Self, max_tries: usize) -> MMFResult<()>
    where
        Self: Sized;
    /// Spin until the lock can be taken, then take it.
    fn spin_and_lock_write(lock: &Self, max_tries: usize) -> MMFResult<()>
    where
        Self: Sized;
    /// Create a new lock at the location of an existing pointer.
    ///
    /// # Safety
    /// Only call this to a pointer where the underlying data is from the same trait impl.
    unsafe fn from_existing(pointer: *mut u8) -> Self
    where
        Self: Sized;
    /// Create a new lock from a raw pointer
    ///
    /// # Safety
    /// Only pass this a pointer with enough space to hold the lock.
    unsafe fn from_raw(pointer: *mut u8) -> Self
    where
        Self: Sized;
    /// Set the lock's first byte to an initialized state.
    fn set_init(&self);
    /// Self-consuming wrapper to chain initialization with [`set_init`][`MMFLock::set_init`]
    fn initialize(self) -> Self
    where
        Self: Sized;
}

/// Packed binary data to represent the locking state of the MMF.
///
/// The wrapper implementation must set these bytes depending on the situation and actions being taken.
/// Due to the fact Windows only guarantees atomic operations on 32-bit ints, (and 64-bit ones only for 64-bit
/// applications on 64-bit Windows), the safest option here is to ensure we're using a 32-bit Atomic.
///
/// This atomic will be split into the following data:
/// - First bit: write lock state. A single writer should prevent all other access.
/// - First byte: after the writelock we have a few left, this serves for tracking init state.
/// - The remaining three are for read lock counting. Beware though, that while this allows you to have a count of up to
///   16_777_215 locks, the OS limits all processes to that number of open handles. This means nobody should ever be
///   remotely close to the actual limit. It also means that if for some reason there _are_ (2^24) - 1 locks, we get to
///   call upon "implementation defined results" which are implemented here as
///   [`Error::MaxReaders`][crate::err::Error::MaxReaders]. Enjoy!
#[cfg(feature = "impl_lock")]
#[derive(Debug)]
pub struct RWLock<'a> {
    /// An Atomic reference to the first 4 bytes in the MemoryMappedView.
    /// Alignment is not an issue considering Windows aligns views to pointers by default.
    chunk: &'a AtomicU32,
}

#[cfg(feature = "impl_lock")]
impl RWLock<'_> {
    /// Mask to check if the lock is initialized
    pub const INITIALIZE_MASK: u32 = 255 << 24;
    /// Mask to check if it's locked for WRITING
    pub const WRITE_LOCK_MASK: u32 = 0b1 << 31;
    /// Mask to check if it's locked for READING
    pub const READ_LOCK_MASK: u32 = !Self::INITIALIZE_MASK;

    /// Check if this lock has been initialized at all.
    ///
    /// Regardless of locking state, and abuse of the 7 empty bits, a lock _should_ not have all bits on the first byte
    /// set to one. If it does, either the lock isn't initialized, or the user is not being very smart.
    fn initialized(chunk: u32) -> bool {
        (chunk & Self::INITIALIZE_MASK) < Self::INITIALIZE_MASK
    }

    /// Check if the lock is held for reading. This should only prevent new write locks.
    fn readlocked(chunk: u32) -> bool {
        (chunk & Self::READ_LOCK_MASK) > 0
    }

    /// Check if the lock is held for writing. This should prevent ALL other locking operations.
    fn writelocked(chunk: u32) -> bool {
        (chunk & Self::WRITE_LOCK_MASK) == Self::WRITE_LOCK_MASK
    }
}

#[cfg(feature = "impl_lock")]
/// Implements a good enough implementation of a lock for MMFs
impl MMFLock for RWLock<'_> {
    /// Construct a lock from existing pointers.
    ///
    /// This is meant to be used with some external mechanism to allow reading and writing lock state directly to and
    /// from some larger struct. The lock will claim the first four bytes behind this pointer; if you do not intend to
    /// have the lock state at the start of the data, make sure to offset the pointer provided.
    ///
    /// # Safety
    /// there is no way of ensuring this pointer is valid after the first byte without moving out of bounds when
    /// it's not. Users should take care to ensure the first 4 bytes in the pointer are valid. A lock created through
    /// this method provides NO guarantees about the lock states and assumes the user has zeroed out all values behind
    /// it if necessary. This initializer is meant to be used with a pointer for an existing lock, or a pointer whose
    /// values you know will provide correct results. If the data behind the pointer is wrong, this effectively
    /// constructs a poisoned lock.
    /// It _is_ safe to assume the size and alignment are valid on Windows, however, as pointers are 0x4/0x4 or 0x8/0x8
    /// depending on 32-bit or 64-bit. Either is safe for use with AtomicU32 which is 0x4/0x4 on these platforms.
    ///
    /// ## Panics
    /// This function _will_ panic if called with a null pointer; ensuring initialization is hard, but ensuring non-null
    /// should not prove difficult to anyone working with raw pointers.
    ///
    /// ## example
    /// ```
    /// # use std::sync::atomic::AtomicU32;
    /// # use winmmf::{states::*, *};
    /// # unsafe {
    /// let bop = AtomicU32::new(0);
    /// let ptr = bop.as_ptr();
    /// let lock = RWLock::from_raw(ptr.cast());
    /// lock.set_init();
    /// // You're now free to do anything with the lock while `bop` lives
    /// let new_ptr = bop.as_ptr();
    /// let other_lock = RWLock::from_existing(new_ptr.cast());
    ///
    /// lock.lock_write().unwrap();
    ///
    /// assert_eq!(other_lock.lock_read(), Err(err::Error::WriteLocked));
    ///
    /// assert!(other_lock.unlock_read().is_err());
    /// assert!(lock.unlock_write().is_ok());
    /// # }
    /// ```
    unsafe fn from_existing(pointer: *mut u8) -> Self {
        if pointer.is_null() {
            panic!("Never, ever pass a null pointer into a lock!")
        }
        Self { chunk: AtomicU32::from_ptr(pointer.cast()) }
    }

    /// Similar to [`Self::from_existing`], except it clears all state and ensures [`Self::initialized`] returns false.
    ///
    /// # Safety
    /// The same safety bounds apply as for `from_existing` with the exception of poisoned lock risks. It does mean,
    /// however, that it invalidates any other locks that use the same pointer and clears any data.
    unsafe fn from_raw(pointer: *mut u8) -> Self {
        if pointer.is_null() {
            panic!("Never, ever pass a null pointer into a lock!")
        }
        let lock = Self { chunk: AtomicU32::from_ptr(pointer.cast()) };
        lock.chunk.store(Self::INITIALIZE_MASK, Ordering::Release);
        lock
    }

    /// Mark this lock as initialized if it isn't yet.
    ///
    /// In pre-0.3 versions of this crate, this would clear existing locks. This is a bad idea though, as a naive caller
    /// might not realize they're not the only process using the MMF.
    fn set_init(&self) {
        _ = self.chunk.compare_exchange(Self::INITIALIZE_MASK, 0, Ordering::Release, Ordering::Relaxed);
    }

    /// Thin wrapper around [`Self::set_init`] that returns self for chaining calls.
    ///
    /// ## Usage
    /// ```
    /// # use std::sync::atomic::AtomicU32;
    /// # use winmmf::{states::*, *};
    /// let bop = AtomicU32::new(0);
    /// let ptr = bop.as_ptr();
    /// let lock = unsafe { RWLock::from_existing(ptr.cast()).initialize() };
    /// assert!(lock.initialized());
    /// ```
    fn initialize(self) -> Self {
        self.set_init();
        self
    }

    /// Check if the lock is initialized
    fn initialized(&self) -> bool {
        Self::initialized(self.chunk.load(Ordering::Acquire))
    }

    /// Increment the counter for read locks ***if and only if*** we can safely lock this for reading
    fn lock_read(&self) -> MMFResult<()> {
        loop {
            let chunk = self.chunk.load(Ordering::Acquire);

            if !Self::initialized(chunk) {
                return Err(Error::Uninitialized);
            }

            if Self::writelocked(chunk) {
                return Err(Error::WriteLocked);
            }

            if (chunk & Self::READ_LOCK_MASK) == Self::READ_LOCK_MASK {
                return Err(Error::MaxReaders);
            }

            if self.chunk.compare_exchange_weak(chunk, chunk + 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                break;
            }
        }

        fence(Ordering::SeqCst);
        Ok(())
    }

    /// Decrease the read lock counter if we can safely do so.
    fn unlock_read(&self) -> MMFResult<()> {
        loop {
            let chunk = self.chunk.load(Ordering::Acquire);

            if !Self::initialized(chunk) {
                return Err(Error::Uninitialized);
            }

            if Self::writelocked(chunk) {
                // this error code is used in lock_read to indicate you cannot lock it because there is a write lock
                // however this bit uses it to signal that it's write locked and your lock usage is broken.
                // should this be two seperate error codes? ditto for other usages of this error code.
                //
                // states should probably also use a lock type that is seperate from the high level API since that error
                // isn't possible there
                return Err(Error::WriteLocked);
            }

            if chunk == 0 {
                // joel: use a better error code like "NoReaders" or something? indicates bad lock usage
                return Err(Error::GeneralFailure);
            }

            if self.chunk.compare_exchange_weak(chunk, chunk - 1, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                break;
            }
        }

        fence(Ordering::SeqCst);
        Ok(())
    }

    /// Set the write lock bit to 1 if possible.
    fn lock_write(&self) -> MMFResult<()> {
        loop {
            let chunk = self.chunk.load(Ordering::Acquire);

            if !Self::initialized(chunk) {
                return Err(Error::Uninitialized);
            }

            if Self::writelocked(chunk) {
                return Err(Error::WriteLocked);
            }

            if Self::readlocked(chunk) {
                return Err(Error::ReadLocked);
            }

            if self
                .chunk
                .compare_exchange_weak(chunk, chunk | Self::WRITE_LOCK_MASK, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        }

        fence(Ordering::SeqCst);
        Ok(())
    }

    /// Release a write lock if one is being held
    fn unlock_write(&self) -> MMFResult<()> {
        loop {
            let chunk = self.chunk.load(Ordering::Acquire);

            if !Self::initialized(chunk) {
                return Err(Error::Uninitialized);
            }

            if !Self::writelocked(chunk) {
                // this should also probably be a different error code, indicates bad lock usage
                return Err(Error::WriteLocked);
            }

            if Self::readlocked(chunk) {
                // this should also probably be a different error code, indicates bad lock usage
                return Err(Error::ReadLocked);
            }

            if self
                .chunk
                .compare_exchange_weak(chunk, chunk ^ Self::WRITE_LOCK_MASK, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                break;
            }
        }

        fence(Ordering::SeqCst);
        Ok(())
    }

    /// Very crude implementation of spinning with no backoff.
    fn spin_and_lock_read(lock: &Self, max_tries: usize) -> MMFResult<()> {
        let mut tries = 0;

        while match lock.lock_read() {
            Ok(_) => false,
            Err(Error::WriteLocked) => true,
            err => return err,
        } {
            tries += 1;
            if tries >= max_tries {
                return Err(Error::MaxTriesReached);
            }
        }

        Ok(())
    }

    /// Very crude implementation of spinning with no backoff.
    fn spin_and_lock_write(lock: &Self, max_tries: usize) -> MMFResult<()> {
        let mut tries = 0;

        while match lock.lock_write() {
            Ok(_) => false,
            Err(Error::WriteLocked | Error::ReadLocked) => true,
            err => return err,
        } {
            tries += 1;
            if tries >= max_tries {
                return Err(Error::MaxTriesReached);
            }
        }

        Ok(())
    }
}
