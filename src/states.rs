use core::fmt;
use std::{
    ops::AddAssign,
    sync::atomic::{fence, AtomicU32, Ordering},
};

use super::err::{Error, MMFResult};

/// Blanket trait for implementing locks to be used with MMFs.
/// The default implementation applied to [`RWLock`] can be used with a custom MMF implementation and vice-versa,
/// but either way would require accounting for the fact this lock is designed to be stored inside the MMF.
/// From the lock's point of view, a pointer to some other u32 would work just as well but this requires some other
/// form of synchronizing access accross thread and application boundaries.
///
/// Users are free to use the default lock and MMF implementations independently of one another.
pub trait MMFLock {
    /// Check if the data this lock is has been initialized for use
    fn initialized(&self) -> bool;
    /// Checks if this lock is readlocked. Does not indicate writelock status; use [`MMFLock::writelocked`] for that.
    fn readlocked(&self) -> bool;
    /// Checks if this lock is writelocked. Use this to wait before reading.
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
    fn unlock_write(&self);
    fn spin(&self, tries: &mut usize) -> MMFResult<bool>;
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
                _ => unreachable!(),
            }
        )
    }
}

/// Packed binary data to represent the locking state of the MMF.
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
///   call upon "implementation defined results" which for this struct means we hit `unreachable!()`. Enjoy!
#[cfg(feature = "impl_lock")]
#[derive(Debug)]
pub struct RWLock<'a> {
    /// An Atomic reference. Alignment is usually not an issue considering Windows aligns views to pointers by default.
    chunk: &'a AtomicU32,
    /// A short-circuit way to prevent unneeded lock modifications. Valid values are tracked in this struct.
    /// Possible values are:
    /// - 128: we hold the write lock
    /// - 1  : we hold the read lock
    /// - 0  : we hold no locks at all
    /// The remaining 6 bitflags possible are as of yet undetermined.
    current_locks: u8,
}

#[cfg(feature = "impl_lock")]
impl<'a> RWLock<'a> {
    /// Mask to check if we're holding a READ lock
    const HOLDING_R: u8 = 0b00000001;
    /// Mask to check if we're holding a WRITE lock
    const HOLDING_W: u8 = 0b10000000;

    /// Mask to check if the lock is initialized
    pub const INITIALIZE_MASK: u32 = 255 << 24;
    /// Mask to check if it's locked for WRITING
    pub const WRITE_LOCK_MASK: u32 = 0b1 << 31;
    /// Mask to check if it's locked for READING
    pub const READ_LOCK_MASK: u32 = !Self::INITIALIZE_MASK;

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
        Self { chunk: AtomicU32::from_ptr(pointer.cast()), current_locks: 0 }
    }

    /// Similar to [`Self::from_existing`], except it clears all state and ensures [`Self::initialized`] returns false.
    /// The same safety bounds apply as for `from_existing` with the exception of poisoned lock risks. It does mean,
    /// however, that it invalidates any other locks that use the same pointer and clears any data in the last byte.
    pub unsafe fn from_raw(pointer: *mut u8) -> Self {
        let lock = Self::from_existing(pointer);
        lock.chunk.store(Self::INITIALIZE_MASK, Ordering::Release);
        lock
    }

    /// Mark this lock as initialized. This will clear any existing lock state, so make sure no locks are taken.
    /// The last byte is left as-is, so it is possible to store custom data before initialization.
    /// The choice to clear all locks upon setting the init state was made to accommodate uses of
    /// [`Self::from_existing`] where it's reasonable to assume no locks are taken or the code using it handles the
    /// situation where the locks are cleared internally.
    pub fn set_init(&self) {
        fence(Ordering::AcqRel);
        _ = self.chunk.compare_exchange(Self::INITIALIZE_MASK, 0, Ordering::Release, Ordering::Relaxed);
        fence(Ordering::AcqRel);
    }

    /// Thin wrapper around [`Self::set_init`] that returns self for chaining calls.
    /// The same safety concerns apply as for `set_init`.
    pub fn initialize(self) -> Self {
        self.set_init();
        self
    }

    /// Takes the [`u32`] from the lock and provides it as a [`u8`] and a [`u32`] (by lack of a proper u24).
    #[inline(always)]
    fn split_lock(lock: u32) -> (u8, u32) {
        ((lock >> 24) as u8, lock & Self::READ_LOCK_MASK)
    }

    /// Takes 4 [`u8`]s and packs them together into a [`u32`] to shove them into the lock.
    #[inline(always)]
    fn merge_lock(&self, bytes: (u8, u32)) -> u32 {
        ((bytes.0 as u32) << 24) | bytes.1
    }
}

impl<'a> MMFLock for RWLock<'a> {
    #[inline(always)]
    fn initialized(&self) -> bool {
        (self.chunk.load(Ordering::Acquire) & Self::INITIALIZE_MASK) == 0
    }
    #[inline(always)]
    fn readlocked(&self) -> bool {
        (self.chunk.load(Ordering::Acquire) & Self::READ_LOCK_MASK) > 0
    }

    #[inline(always)]
    fn writelocked(&self) -> bool {
        (self.chunk.load(Ordering::Acquire) & Self::WRITE_LOCK_MASK) > 0
    }

    #[inline(always)]
    fn locked(&self) -> bool {
        self.chunk.load(Ordering::Acquire) > 255
    }

    fn lock_read(&self) -> MMFResult<()> {
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

    fn unlock_read(&self) -> MMFResult<()> {
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

    fn lock_write(&self) -> MMFResult<()> {
        if !self.initialized() {
            Err(Error::Uninitialized)
        } else if self.writelocked() {
            Err(Error::WriteLocked)
        } else if self.readlocked() {
            Err(Error::ReadLocked)
        } else {
            let mut bytes = RWLock::split_lock(1);
            bytes.1 = 1;
            Ok(self.merge_lock(bytes))
        }
    }

    fn unlock_write(&self) {
        let mut bytes = RWLock::split_lock(1);
        if bytes.1 != 0 {
            bytes.1 = 0
        }
        self.merge_lock(bytes)
    }

    fn spin(&self, tries: &mut usize) -> MMFResult<bool> {
        tries.add_assign(1);
        if self.current_locks > 0 {
            Ok(true)
        } else if usize::MAX.gt(tries) {
            Err(Error::LockViolation)
        } else {
            Ok(false)
        }
    }
}

/// Takes the [`u32`] from the lock and provides it as a [`u8`] and a [`u32`] (by lack of a proper u24).
#[cfg(feature = "impl_lock")]
#[inline(always)]
fn split_lock(lock: u32) -> (u8, u32) {
    ((lock >> 24) as u8, lock & RWLock::READ_LOCK_MASK)
}

/// Takes 4 [`u8`]s and packs them together into a [`u32`] to shove them into the lock.
#[cfg(feature = "impl_lock")]
#[inline(always)]
fn merge_lock(bytes: (u8, u32)) -> u32 {
    ((bytes.0 as u32) << 24) | bytes.1
}
