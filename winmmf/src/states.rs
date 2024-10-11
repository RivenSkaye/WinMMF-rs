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
//! this lock was designed. Things to note are that any one instance of an [`RWLock`] (and by extension any instance of
//! an MMF) can only ever track a maximum of 127 readers and a single writer. And those are mutually exclusive.
//! This means, that if you want to use MMFs in a place where several things are touching the memory at the same time,
//! you'll deal with errors. Luckily these are usually just abstractions that tell you all is well, unless things go
//! very wrong. And in that case, good luck. [`RWLock::spin`] will be your friend, as you'd only need to handle the case
//! where you spin more than what your native pointer size holds and you should be seeing problems long before then.
//!
//! No guarantees are made about the usefulness and safety of this code, and the project maintainer is not liable for
//! any damages, be they to your PC or your (mental) health.

use core::fmt;
use std::{
    ops::AddAssign,
    sync::atomic::{fence, AtomicU32, AtomicU8, Ordering},
};

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
    /// Check if the data this lock is has been initialized for use
    fn initialized(&self) -> bool;
    /// Checks if this lock is readlocked. Does not indicate writelock status; use
    /// [the other lock probe][`MMFLock::writelocked`] for that.
    fn readlocked(&self) -> bool;
    /// Checks if this lock is writelocked. Does not indicate readlock status; use
    /// [the other lock probe][`MMFLock::readlocked`] for that.
    fn writelocked(&self) -> bool;
    /// Checks if there are any acitve locks, including the initialization locks.
    fn locked(&self) -> bool;
    /// Acquire a readlock, if at all possible. Otherwise error.
    fn lock_read(&self) -> MMFResult<()>;
    /// Release a readlock, clearing the readlock state if this was the last lock.
    fn unlock_read(&self) -> MMFResult<()>;
    /// Lock this file for writing if possible.
    fn lock_write(&self) -> MMFResult<()>;
    /// Nuke all existing write locks as there can only be one, legally.
    fn unlock_write(&self) -> MMFResult<()>;
    /// Spin and return true while the lock is held
    fn spin(&self, tries: &mut usize) -> MMFResult<bool>;
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

impl fmt::Debug for dyn MMFLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Lock {{ {} }}",
            match (self.writelocked(), self.readlocked()) {
                (true, false) => "Write",
                (false, true) => "Read",
                (false, false) => "None",
                (true, true) => "Poisoned",
            }
        )
    }
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
///
/// To prevent a series of potentially problematic results, every unique instance of this lock should track what it
/// holds internally as well. This also allows for every unique lock instance to limit the amount of locks held to
/// ONE write lock and 127 read locks. Any one application shouldn't need more than that, and it allows us to do things
/// like only clearing lock counters if we actually hold a lock to release. If your custom use-case has a need for more
/// than 127 readers in one application, you're free to reuse the code here in a way that allows more readers than the
/// OS. Just change the `current_lock` and `HOLDING_` constants to their 32-bit counterparts, then shift `HOLDING_W` 24
/// bits to the left. The reason the default implementation doesn't do this, is that it was written to ensure it's safe
/// to use. Weird OS quirks when going over the default limits don't fit that bill, so limiting the amount of open
/// handles allows for guaranteeing safety assuming a sane system configuration.
#[cfg(feature = "impl_lock")]
#[derive(Debug)]
pub struct RWLock<'a> {
    /// An Atomic reference to the first 8 bytes in the MemoryMappedView.
    /// Alignment is not an issue considering Windows aligns views to pointers by default.
    chunk: &'a AtomicU32,
    /// Current internal lock state, used to prevent us from releasing locks we don't hold.
    current_lock: AtomicU8,
}

#[cfg(feature = "impl_lock")]
impl RWLock<'_> {
    /// Mask to check if the lock is initialized
    pub const INITIALIZE_MASK: u32 = 255 << 24;
    /// Mask to check if it's locked for WRITING
    pub const WRITE_LOCK_MASK: u32 = 0b1 << 31;
    /// Mask to check if it's locked for READING
    pub const READ_LOCK_MASK: u32 = !Self::INITIALIZE_MASK;

    /// Bitmask to check if we're holding the write lock ourselves. One bit to rule them all.
    pub const HOLDING_W: u8 = 0b10000000;
    /// Bitmask for readlocks.
    ///
    /// Any of these mean we hold a lock, all of these means we **can't hold any more read locks**.
    pub const HOLDING_R: u8 = !Self::HOLDING_W;
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
    /// assert!(other_lock.writelocked());
    /// assert!(!other_lock.readlocked());
    /// assert!(other_lock.lock_read().is_err());
    ///
    /// assert!(other_lock.unlock_write().is_err());
    /// assert!(lock.unlock_write().is_ok());
    /// # }
    /// ```
    unsafe fn from_existing(pointer: *mut u8) -> Self {
        if pointer.is_null() {
            panic!("Never, ever pass a null pointer into a lock!")
        }
        Self { chunk: AtomicU32::from_ptr(pointer.cast()), current_lock: AtomicU8::new(0) }
    }

    /// Similar to [`Self::from_existing`], except it clears all state and ensures [`Self::initialized`] returns false.
    ///
    /// # Safety
    /// The same safety bounds apply as for `from_existing` with the exception of poisoned lock risks. It does mean,
    /// however, that it invalidates any other locks that use the same pointer and clears any data.
    unsafe fn from_raw(pointer: *mut u8) -> Self {
        let lock = Self::from_existing(pointer);
        lock.chunk.store(Self::INITIALIZE_MASK, Ordering::Release);
        lock
    }

    /// Mark this lock as initialized. This will clear any existing lock state, so make sure no locks are taken.
    ///
    /// The choice to clear all locks upon setting the init state was made to accommodate uses of
    /// [`Self::from_existing`] where it's reasonable to assume no locks are taken or the code using it handles the
    /// situation where the locks are cleared internally.
    fn set_init(&self) {
        fence(Ordering::AcqRel);
        self.chunk.store(0, Ordering::Release);
        self.current_lock.store(0, Ordering::Release);
        fence(Ordering::AcqRel);
    }

    /// Thin wrapper around [`Self::set_init`] that returns self for chaining calls.
    ///
    /// # Safety
    /// The same safety concerns apply as for `set_init`.
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

    /// Check if this lock has been initialized at all.
    ///
    /// Regardless of locking state, and abuse of the 7 empty bits, a lock _should_ not have all bits on the first byte
    /// set to one. If it does, either the lock isn't initialized, or the user is not being very smart.
    #[inline(always)]
    fn initialized(&self) -> bool {
        fence(Ordering::AcqRel);
        (self.chunk.load(Ordering::Acquire) & Self::INITIALIZE_MASK) < Self::INITIALIZE_MASK
            || self.current_lock.load(Ordering::Acquire) < 255
    }

    /// Check if the lock is held for reading. This should only prevent new write locks.
    #[inline(always)]
    fn readlocked(&self) -> bool {
        fence(Ordering::AcqRel);
        (self.chunk.load(Ordering::Acquire) & Self::READ_LOCK_MASK) > 0
            || (self.current_lock.load(Ordering::Acquire) & Self::HOLDING_R) > 0
    }

    /// Check if the lock is held for writing. This should prevent ALL other locking operations.
    #[inline(always)]
    fn writelocked(&self) -> bool {
        fence(Ordering::AcqRel);
        (self.chunk.load(Ordering::Acquire) & Self::WRITE_LOCK_MASK) == Self::WRITE_LOCK_MASK
            || (self.current_lock.load(Ordering::Acquire) & Self::HOLDING_W) == Self::HOLDING_W
    }

    /// Check if the lock is held at all.
    ///
    /// This compares using the initialization mask is done in a similar vein to niche optimizations;
    /// it should never be possible to hold read _and_ write locks. Similarly, if we hold no internal
    /// locks, our internal lock state is guaranteed to be zero.
    #[inline(always)]
    fn locked(&self) -> bool {
        fence(Ordering::AcqRel);
        (self.chunk.load(Ordering::Acquire) & Self::INITIALIZE_MASK) < Self::INITIALIZE_MASK
            || self.current_lock.load(Ordering::Acquire) > 0
    }

    /// Increment the counter for read locks ***if and only if*** we can safely lock this for reading
    fn lock_read(&self) -> MMFResult<()> {
        if !self.initialized() {
            Err(Error::Uninitialized)
        } else if self.writelocked() {
            Err(Error::WriteLocked)
        } else {
            fence(Ordering::AcqRel);
            let ret = self
                .chunk
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |lock| {
                    if (lock & Self::READ_LOCK_MASK) == Self::READ_LOCK_MASK
                        || self.current_lock.load(Ordering::Acquire) == Self::HOLDING_R
                    {
                        None
                    } else {
                        self.current_lock.fetch_add(1, Ordering::AcqRel);
                        Some(lock + 1)
                    }
                })
                .map(|_| ())
                .map_err(|_| Error::MaxReaders);
            fence(Ordering::AcqRel);
            ret
        }
    }

    /// Decrease the read lock counter if we can safely do so.
    fn unlock_read(&self) -> MMFResult<()> {
        if !self.initialized() {
            Err(Error::Uninitialized)
        } else if self.writelocked() {
            Err(Error::WriteLocked)
        } else {
            fence(Ordering::AcqRel);
            let ret = self
                .chunk
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |lock| {
                    if (lock & Self::READ_LOCK_MASK) == 0 || self.current_lock.load(Ordering::Acquire) == 0 {
                        None
                    } else {
                        self.current_lock.fetch_sub(1, Ordering::AcqRel);
                        Some(lock - 1)
                    }
                })
                .map(|_| ())
                .map_err(|_| Error::MaxReaders);
            fence(Ordering::AcqRel);
            ret
        }
    }

    /// Set the write lock bit to 1 if possible.
    fn lock_write(&self) -> MMFResult<()> {
        if !self.initialized() {
            Err(Error::Uninitialized)
        } else if self.writelocked() {
            Err(Error::WriteLocked)
        } else if self.readlocked() {
            Err(Error::ReadLocked)
        } else {
            fence(Ordering::AcqRel);
            self.chunk
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |lock| {
                    self.current_lock.fetch_or(Self::HOLDING_W, Ordering::AcqRel);
                    Some(lock | Self::WRITE_LOCK_MASK)
                })
                .map(|_| ())
                .map_err(|_| Error::GeneralFailure)
        }
    }

    /// Release a write lock if we're the ones holding it
    fn unlock_write(&self) -> MMFResult<()> {
        if !self.writelocked() {
            return Ok(());
        }
        if !self.initialized() {
            Err(Error::Uninitialized)
        } else {
            fence(Ordering::AcqRel);
            self.chunk
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |lock| {
                    if (self.current_lock.load(Ordering::Acquire) & Self::HOLDING_W) == 0 {
                        None
                    } else {
                        self.current_lock.fetch_xor(Self::HOLDING_W, Ordering::AcqRel);
                        Some(lock ^ Self::WRITE_LOCK_MASK)
                    }
                })
                .map(|_| ())
                .map_err(|_| Error::GeneralFailure)
        }
    }

    /// Very naive spinning implementation. Runs a finite amount of times.
    ///
    /// This spinning implementation just checks if the lock is held for as many times as it needs to. If it encounters
    /// the upper bound of the native pointer size before the lock is released, it returns an error.
    /// If uni taught me one thing, it would be that `while true` on locks will eventually lead to the big funny.
    fn spin(&self, tries: &mut usize) -> MMFResult<bool> {
        tries.add_assign(1);
        let held = self.locked();
        if usize::MAX.eq(tries) && held {
            Err(Error::LockViolation)
        } else {
            Ok(held)
        }
    }
}
