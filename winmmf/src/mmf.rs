#![deny(clippy::missing_docs_in_private_items)]
#![deny(missing_docs)]
//! # Memory-mapped files
//!
//! This module contains all of the important parts to pop open pagefile-backed memory. For more in-depth information
//! and some background, please refer to [the series of blog posts](https://skaye.blog/winmmf/overview) I wrote about
//! my fun adventures with getting low-overhead IPC working.
//!
//! While it would be possible to split things out further, using this much to ensure everything works smoothly helps
//! keeping this maintanable and usable. If you need a more minimal implementation, feel free to yank whatever you need
//! from here and instead building the crate without default features.

use super::{
    err::{Error as MMFError, MMFResult},
    states::MMFLock,
};
use fixedstr::ztr64;
use microseh::try_seh;
use windows::{
    core::Error as WErr,
    Win32::{
        Foundation::HANDLE,
        System::Memory::{UnmapViewOfFile, MEMORY_MAPPED_VIEW_ADDRESS},
    },
};

use std::cell::Cell;
#[cfg(feature = "impl_mmf")]
use std::{fmt, num::NonZeroUsize, ops::Deref};
#[cfg(feature = "impl_mmf")]
use windows::{
    core::PCSTR,
    Win32::{
        Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE},
        System::Memory::{CreateFileMappingA, MapViewOfFile, OpenFileMappingA, FILE_MAP_ALL_ACCESS, PAGE_READWRITE},
    },
};
#[cfg(feature = "impl_mmf")]
use windows_ext::ext::QWordExt;

/// Local namespace prefix
/// Use this to ensure only you and your child processes can read this.
pub const LOCAL_NAMESPACE: ztr64 = ztr64::const_make("Local\\");
/// Global namespace prefix, requires SeCreateGlobal permission to create.
/// [See MSDN](https://learn.microsoft.com/en-us/windows/win32/memory/creating-named-shared-memory#first-process)
/// for more info
pub const GLOBAL_NAMESPACE: ztr64 = ztr64::const_make("Global\\");

/// Namespaces as an enum, to unambiguously represent relevant information.
#[cfg(feature = "namespaces")]
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum Namespace {
    /// Local namespace, always allowed and sharable with children
    LOCAL = 0,
    /// Global namespace, requires SeCreateGlobal. See [`GLOBAL_NAMESPACE`].
    GLOBAL = 1,
    /// Custom namespace, makes it private unless you share/leak handles yourself.
    CUSTOM = 2,
}

/// We do a little transmutation, I'm an aclhemist!
impl TryFrom<u8> for Namespace {
    /// Unit type, as we only need it for checking and never for more info.
    type Error = ();
    /// This can only fail on invalid values. Check validity and transmute safely.
    fn try_from(value: u8) -> Result<Namespace, Self::Error> {
        match value {
            ..=2 => Ok(unsafe { std::mem::transmute::<u8, Self>(value) }),
            _ => Err(()),
        }
    }
}

/// Mostly for debug purposes
#[cfg(feature = "namespaces")]
impl fmt::Display for Namespace {
    /// Presents the namespace, or a useless message for custom namespaces
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LOCAL => write!(f, "{LOCAL_NAMESPACE}"),
            Self::GLOBAL => write!(f, "{GLOBAL_NAMESPACE}"),
            _ => write!(f, "A custom namespace was used here."),
        }
    }
}

/// Basic trait for Memory Mapped Files.
///
/// Implementing this is ensures you have the bare minimum to actually use your MMF and this _might_ at some point be
/// reworked to provide a proper File-like interface. Actually providing a File interface or trait implementation still
/// requires a great deal of work, however, as raw pointers (which are used in a few places here) are not [`Send`] or
/// [`Sync`]
pub trait Mmf {
    /// Read data from the MMF, return an owned Vec if all goes well.
    /// The standard implementation creates a new Vec and calls [`Self::read_to_buf`]
    fn read(&self, count: usize) -> MMFResult<Vec<u8>>;
    /// Read data from the MMF into a provided buffer.
    fn read_to_buf(&self, buffer: &mut Vec<u8>, count: usize) -> MMFResult<()>;
    /// Read data into a raw pointer and pray it's valid
    ///
    /// # Safety
    /// The caller is responsible to ensure the slice is big enough to read into.
    unsafe fn read_to_raw(&self, buffer: *mut u8, count: usize) -> MMFResult<()>;
    /// Allows for viewing the size without exposing the property.
    fn size(&self) -> usize;
    /// Write data to the MMF.
    fn write(&self, buffer: impl Deref<Target = [u8]>) -> MMFResult<()>;
    /// Spin for `tries` times max, or until reading is allowed.
    ///
    /// This method takes an optional spinning function that returns a result. The spinning function must acquire the
    /// lock, and this function must unlock.
    fn read_spin<F>(&self, count: usize, spinner: Option<F>) -> MMFResult<Vec<u8>>
    where
        F: FnMut(&dyn MMFLock, usize) -> MMFResult<()>;
    /// Spin for `tries` times max, or until reading is allowed.
    ///
    /// This method takes an optional spinning function that returns a result. The spinning function must acquire the
    /// lock, and this function must unlock.
    fn read_to_buf_spin<F>(&self, buffer: &mut Vec<u8>, count: usize, spinner: Option<F>) -> MMFResult<()>
    where
        F: FnMut(&dyn MMFLock, usize) -> MMFResult<()>;
    /// Spin for `tries` times max, or until reading is allowed.
    ///
    /// # Safety
    /// See [`read_to_raw`][Mmf::read_to_raw]
    ///
    /// This method takes an optional spinning function that returns a result. The spinning function must acquire the
    /// lock, and this function must unlock.
    unsafe fn read_to_raw_spin<F>(&self, buffer: *mut u8, count: usize, spinner: Option<F>) -> MMFResult<()>
    where
        F: FnMut(&dyn MMFLock, usize) -> MMFResult<()>;
    /// Spin for `tries` times max, or until writing is allowed.
    ///
    /// This method takes an optional spinning function that returns a result. The spinning function must acquire the
    /// lock, and this function must unlock.
    /// Defaults to [the one in `RWLock`][crate::states::RWLock]
    fn write_spin<F>(&self, buffer: impl Deref<Target = [u8]>, spinner: Option<F>) -> MMFResult<()>
    where
        F: FnMut(&dyn MMFLock, usize) -> MMFResult<()>;
}

/// A simple struct wrapping a [Memory Mapped File](https://learn.microsoft.com/en-us/windows/win32/memory/creating-named-shared-memory).
///
/// It contains all the data required to create and keep alive a [`HANDLE`] to a Memory Mapped File. The [`HANDLE`] is
/// required to create a [`MEMORY_MAPPED_VIEW_ADDRESS`] to read and write data from. In order to expose reading and
/// writing functionality in a safe manner, these are wrapped in a safe API that prefers more short blocks of unsafe
/// over a larger block that also does safe operations.
/// Once this reaches a stable-ish state, an unsafe function feature will be added to get access to the raw handle and
/// map view.
///
/// Supports both x86 and AMD64 by leveraging usize, to allow target-sized ints to be used everywhere.
#[derive(Debug)]
pub struct MemoryMappedFile<LOCK: MMFLock> {
    /// The [`HANDLE`] to the created mapping
    handle: HANDLE,
    /// the "filename" portion
    name: ztr64,
    /// The higher order bits for the size of the opened file.
    #[allow(dead_code)]
    size_high_order: u32,
    /// The lower order bits for the size of the opened file.
    #[allow(dead_code)]
    size_low_order: u32,
    /// The total size, which is the bits of high and low order appeneded.
    size: usize,
    /// The lock struct, which is where some of the cooler magic happens.
    lock: LOCK,
    /// The original MemoryMappedView; We need to keep this around for unmapping it.
    map_view: Option<MemoryMappedView>,
    /// The pointer we can actually write into without fucking up the lock
    write_ptr: *mut u8,
    /// A one-way changing cell to prevent using the MMF after closing it.
    closed: Cell<bool>,
    /// A bool to prevent writing through an MMF opened for reading
    readonly: bool,
}

#[cfg(feature = "impl_mmf")]
impl<LOCK: MMFLock> MemoryMappedFile<LOCK> {
    /// Attempt to create a new Memory Mapped File. Or fail _graciously_ if we can't.
    ///
    /// The size will be automatically divided into the upper and lower halves, as the function to allocate this memory
    /// requires them to be split. The name of the file should be any one of:
    ///
    /// 1. Just a filename, if the namespace is either [`Namespace::GLOBAL`] or [`Namespace::LOCAL`]
    /// 2. A namespaced filename if using [`Namespace::CUSTOM`] and you know what you're doing
    /// 3. Just a filename if using [`Namespace::CUSTOM`] and you don't need other processes to access it.
    ///
    /// Violating these constraints _should_ result in a local namespace, but no guarantees are given and if it leads to
    /// UB, the heat death of the universe, panics or errors or a change in the answer to a value other than 42. you're
    /// on your own.
    ///
    /// The size MUST be a non-zero value; allocating zero bytes errors on the OS end of things. Allocating too much
    /// will make a part of the file inaccessible to other code trying to read it from a 32-bit process.
    /// The total size allocated will be 4 bytes larger than the specified size, but only after checking the input size
    /// is non-zero.
    pub fn new(size: NonZeroUsize, name: impl Into<ztr64>, namespace: Namespace) -> MMFResult<Self> {
        // Build the name to use for the MMF
        let init_name = match namespace {
            Namespace::GLOBAL => GLOBAL_NAMESPACE,
            Namespace::LOCAL => LOCAL_NAMESPACE,
            Namespace::CUSTOM => ztr64::new(),
        } + name.into();

        // fuckin' windows
        let mmf_name = PCSTR::from_raw(init_name.to_ptr());
        let (dw_low, dw_high) = (size.get() + 4).split();

        // Safety: handled through microSEH and we check the last error status later. Failure here is failure there.
        let handle = try_seh(|| unsafe {
            CreateFileMappingA(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, dw_high, dw_low, mmf_name)
        })??;

        // Unsafe because `MapViewOfFile` is marked as such, but it should return a NULL pointer when failing; and set
        // the last error state correspondingly.
        let map_view = try_seh(|| unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, size.get() + 4) })?;

        // Explicit check to make sure we have something that works (later is now)
        if unsafe { GetLastError() }.is_err() {
            return Err(WErr::from_win32().into());
        }

        // Waste some time to ensure the memory is zeroed out - I learned the importance of this the hard way.
        let zeroing = vec![0; size.get() + 4];
        // safety: we're writing zeroes into memory we just got back from the OS
        unsafe { std::ptr::copy(zeroing.as_ptr(), map_view.Value.cast(), zeroing.len()) };

        // safety: we just zeroed this memory out and we're initializing it freshly
        let lock = unsafe { LOCK::from_raw(map_view.Value.cast()).initialize() };
        let write_ptr = unsafe { map_view.Value.cast::<u8>().add(4) };
        Ok(Self {
            handle,
            name: init_name,
            size_high_order: dw_high,
            size_low_order: dw_low,
            size: size.get(),
            map_view: Some(map_view.into()),
            lock,
            write_ptr,
            closed: Cell::new(false),
            readonly: false,
        })
    }

    /// Open an existing MMF, if it exists.
    ///
    /// Defaults to read and write permissions, use the exposed wrappers to open R or RW
    /// I have no idea what happens if you call this on a fake name. Code responsibly.
    /// In all reality though, it should return an error that you can handle.
    pub fn open(size: NonZeroUsize, name: &str, namespace: Namespace, readonly: bool) -> MMFResult<Self> {
        // Build the name to use for the MMF
        let init_name = match namespace {
            Namespace::GLOBAL => ztr64::make(&format!("{GLOBAL_NAMESPACE}{name}")),
            Namespace::LOCAL => ztr64::make(&format!("{LOCAL_NAMESPACE}{name}")),
            Namespace::CUSTOM => ztr64::make(name),
        };
        // fuckin' windows
        let mmf_name = PCSTR::from_raw(init_name.to_ptr());
        let (dw_low, dw_high) = (size.get() + 4).split();

        // Safety: Issues here are issues later, and we check for them later.
        let handle = try_seh(|| unsafe { OpenFileMappingA(FILE_MAP_ALL_ACCESS.0, false, mmf_name) })??;

        // Unsafe because `MapViewOfFile` is marked as such, but it should return a NULL pointer when failing; and set
        // the last error state correspondingly.
        let map_view = try_seh(|| unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, size.get() + 4) })?;

        // Explicit check to make sure we have something that works (later is now)
        if unsafe { GetLastError() }.is_err() {
            return Err(WErr::from_win32().into());
        }

        // Safety: We know where these bytes come from (ideally, they were opened by this lib)
        let lock = unsafe { LOCK::from_existing(map_view.Value.cast()) };
        let write_ptr = unsafe { map_view.Value.cast::<u8>().add(4) };
        Ok(Self {
            handle,
            name: init_name,
            size_high_order: dw_high,
            size_low_order: dw_low,
            size: size.get(),
            lock,
            map_view: Some(map_view.into()),
            write_ptr,
            closed: Cell::new(false),
            readonly,
        })
    }

    /// Open an MMF for reading
    ///
    /// Wrapper around [`open`][Self::open] that always passes true
    pub fn open_read(size: NonZeroUsize, name: &str, namespace: Namespace) -> MMFResult<Self> {
        Self::open(size, name, namespace, true)
    }

    /// Open an MMF for reading and writing
    ///
    /// Wrapper around [`open`][Self::open] that always passes false
    pub fn open_write(size: NonZeroUsize, name: &str, namespace: Namespace) -> MMFResult<Self> {
        Self::open(size, name, namespace, false)
    }

    /// Check if this MMF can be written to
    pub fn is_writable(&self) -> bool {
        !self.readonly && !self.closed.get() && self.lock.initialized()
    }

    /// Check if this MMF can be read from
    pub fn is_readable(&self) -> bool {
        !self.closed.get() && self.lock.initialized()
    }

    /// Get the namespace of the file, if any. If an empty string is returned, it's Local.
    pub fn namespace(&self) -> String {
        self.name.split_once('\\').unwrap_or_default().0.to_owned()
    }

    /// Return the filename the MMF is bound to, which is only the whole name if no namespace is provided.
    pub fn filename(&self) -> String {
        self.name.split_once('\\').map(|s| s.1.to_owned()).unwrap_or(self.name.to_string())
    }

    /// Returns the stored name, which should be `[Namespace\]<FileName>`
    pub fn fullname(&self) -> String {
        self.name.to_string()
    }

    /// Close the MMF. Don't worry about calling this, it's handled in [`Drop`].
    pub fn close(&self) -> MMFResult<()> {
        self.closed.set(true);
        // Safety: microSEH handles the OS side of this error, and the match handles this end.
        match try_seh(|| unsafe { CloseHandle(self.handle) })?.map_err(MMFError::from) {
            Err(MMFError::OS_OK(_)) | Ok(_) => Ok(()),
            err => err.map_err(|e| {
                eprintln!("Error closing MMF's handle: {:#?}", e);
                e
            }),
        }
    }
}

/// Implements a usable file-like interface for working with an MMF. Pass all input as bytes, please.
#[cfg(feature = "impl_mmf")]
impl<LOCK: MMFLock> Mmf for MemoryMappedFile<LOCK> {
    /// Attempts to read bytes up to the entirety of the data as defined in [`Self::size`].
    ///
    /// This function succeeds if there is a value in [`Self::map_view`] but it cannot guarantee the data returned is
    /// correct. This is an unfortunate side effect of having to work with raw pointers and bytes in memory.
    /// Assuming nothing external has touched the memory region other than this class, it _should_ be valid data unless
    /// it's marked as uninitialized. The returned error for this is an instance of the
    /// [crate's error enum][crate::err::Error]
    ///
    /// - 1: Write Protected; the file has a write lock on it which means reading might return incomplete data, or the
    ///   maximum amount of readers has been reached (this should not happen assuming all implementations are clean).
    /// - 2: Invalid block; the lock is telling us this data has not yet been initialized.
    /// - 5: File not found; the MMF isn't opened yet or no map view exists.
    #[inline]
    fn read(&self, count: usize) -> Result<Vec<u8>, MMFError> {
        let mut buf = Vec::with_capacity(self.size);
        self.read_to_buf(&mut buf, count)?;
        Ok(buf)
    }

    /// Spinning form of [`read`][Self::read]
    fn read_spin<F>(&self, count: usize, spinner: Option<F>) -> MMFResult<Vec<u8>>
    where
        F: FnMut(&dyn MMFLock, usize) -> MMFResult<()>,
    {
        let mut buf = Vec::with_capacity(self.size);
        self.read_to_buf_spin(&mut buf, count, spinner)?;
        Ok(buf)
    }

    /// See the documentation for [Self::read()], except this takes a buffer to write to.
    ///
    /// If the count is 0, the entire MMF will be read into the buffer. If the buffer is smaller than the amount of data
    /// to be read, it _will be grown_ to fit the requested data, using [`Vec::reserve_exact`]. The returned error for
    /// this is an instance of the [crate's error enum][crate::err::Error]
    fn read_to_buf(&self, buffer: &mut Vec<u8>, count: usize) -> MMFResult<()> {
        let buf_cap = buffer.capacity();
        let to_read = if count == 0 { self.size } else { count };

        if buf_cap < to_read {
            buffer.reserve_exact(to_read - buf_cap);
        }
        unsafe {
            self.read_to_raw(buffer.as_mut_ptr(), count)?;
            buffer.set_len(to_read);
        }
        Ok(())
    }

    /// Spinning version of [`read_to_buf`][Self::read_to_buf]
    fn read_to_buf_spin<F>(&self, buffer: &mut Vec<u8>, count: usize, spinner: Option<F>) -> MMFResult<()>
    where
        F: FnMut(&dyn MMFLock, usize) -> MMFResult<()>,
    {
        let buf_cap = buffer.capacity();
        let to_read = if count == 0 { self.size } else { count };

        if buf_cap < to_read {
            buffer.reserve_exact(to_read - buf_cap);
        }
        unsafe {
            self.read_to_raw_spin(buffer.as_mut_ptr(), count, spinner)?;
            buffer.set_len(to_read);
        }
        Ok(())
    }

    /// Read into a raw pointer and pray it's valid for `count` bytes.
    ///
    /// If the count is 0, this operation will exit with a [non-specific error][MMFError::GeneralFailure]. This method
    /// provides a best effort to ensure that the read portion of the copy is sound by clamping `count` to the MMFs
    /// size. This prevents, at the very least, UB from reading beyond the end of the MMF. It also ensures the MMF is
    /// opened and initialized, with the usual errors from [`read`][Self::read] to make these problems known to callers.
    ///
    /// # Safety
    /// It is the caller's responsibility to ensure that `buffer` is valid for at least `count` bytes. Failing to do so
    /// is UB. See the documentation for [`std::ptr::copy`] for safety concerns, the provided `buffer` is the `dst`.
    unsafe fn read_to_raw(&self, buffer: *mut u8, count: usize) -> Result<(), MMFError> {
        if self.closed.get() {
            Err(MMFError::MMF_NotFound)
        } else if count == 0 {
            Err(MMFError::GeneralFailure)
        } else if self.map_view.is_some() {
            if !self.lock.initialized() {
                return Err(MMFError::Uninitialized);
            }
            self.lock.lock_read()?;

            // safety: memory may overlap with copy_to. With the size check, we also ensure we don't copy more bytes
            // than what fits in the buffer. If someone gave us a dirty slice, that's on them. Notably, they would
            // get UB from providing a slice with an incorrect internally registered length.
            unsafe {
                self.write_ptr.copy_to(buffer, count.min(self.size));
            }
            self.lock.unlock_read().unwrap();
            Ok(())
        } else {
            Err(MMFError::MMF_NotFound)
        }
    }

    /// Spinning version of [`read_to_raw`][Self::read_to_raw]
    ///
    /// # Safety
    /// It is the caller's responsibility to ensure that `buffer` is valid for at least `count` bytes. Failing to do so
    /// is UB. See the documentation for [`std::ptr::copy`] for safety concerns, the provided `buffer` is the `dst`.
    unsafe fn read_to_raw_spin<F>(&self, buffer: *mut u8, count: usize, spinner: Option<F>) -> MMFResult<()>
    where
        F: FnMut(&dyn MMFLock, usize) -> MMFResult<()>,
    {
        if self.closed.get() {
            Err(MMFError::MMF_NotFound)
        } else if count == 0 {
            Err(MMFError::GeneralFailure)
        } else if self.map_view.is_some() {
            if let Some(mut spinner) = spinner {
                spinner(&self.lock, usize::MAX)?;
            } else {
                LOCK::spin_and_lock_read(&self.lock, usize::MAX)?;
            }

            // safety: memory may be overlapped with copy_to. With the size check, we also ensure we don't copy more
            // bytes than what fits in the buffer. If someone gave us a dirty slice, that's on them.
            // Notably, they would get UB from providing a pointer with too little space.
            unsafe {
                self.write_ptr.copy_to(buffer, count.min(self.size));
            }
            self.lock.unlock_read().unwrap();
            Ok(())
        } else {
            Err(MMFError::MMF_NotFound)
        }
    }

    /// Attempt to write a complete buffer into the MMF. Uses pointers and memcpy to be fast.
    ///
    /// This function errors only if the lock could not be acquired or when trying to write more data than fits. Writing
    /// more data than the MMF can hold is UB so this is prevented by erroring out instead. If the input buffer is
    /// smaller than the destination file, the end is zeroed out. The start of the buffer is also padded by the lock
    /// bytes to signal and flag locking.The returned error for this is an instance of the
    /// [crate's error enum][crate::err::Error]
    ///
    /// Error codes produced by this function:
    /// - 0 or 1: Access denied; the lock could not be acquired or the MMF is read-only.
    /// - 4: Not enough memory; the write was blocked because it was too large.
    /// - All errors from [Self::read()] as a read is required to update the lock.
    fn write(&self, buffer: impl Deref<Target = [u8]>) -> MMFResult<()> {
        if self.readonly || self.closed.get() {
            return Err(MMFError::MMF_NotFound);
        }
        let cap = buffer.len().min(self.size);
        if cap < buffer.len() {
            Err(MMFError::NotEnoughMemory)
        } else if !self.lock.initialized() {
            Err(MMFError::Uninitialized)
        } else if self.map_view.is_some() {
            self.lock.lock_write()?;
            let src_ptr = buffer.as_ptr();
            // We ensured this size is correct and filled out when instantiating the MMF, this is just writing the same
            // amount of bytes to the same place in memory.
            unsafe { src_ptr.copy_to(self.write_ptr, cap) };
            self.lock.unlock_write()
        } else {
            Err(MMFError::MMF_NotFound)
        }
    }

    fn write_spin<F>(&self, buffer: impl Deref<Target = [u8]>, spinner: Option<F>) -> MMFResult<()>
    where
        F: FnMut(&dyn MMFLock, usize) -> MMFResult<()>,
    {
        if self.readonly || self.closed.get() {
            return Err(MMFError::MMF_NotFound);
        }
        let cap = buffer.len().min(self.size);
        if cap < buffer.len() {
            Err(MMFError::NotEnoughMemory)
        } else if self.map_view.is_some() {
            if let Some(mut spinner) = spinner {
                spinner(&self.lock, usize::MAX)?;
            } else {
                LOCK::spin_and_lock_write(&self.lock, usize::MAX)?;
            }
            let src_ptr = buffer.as_ptr();
            // We ensured this size is correct and filled out when instantiating the MMF, this is just writing the same
            // amount of bytes to the same place in memory.
            unsafe { src_ptr.copy_to(self.write_ptr, cap) };
            self.lock.unlock_write()
        } else {
            Err(MMFError::MMF_NotFound)
        }
    }

    /// Returns the size of the data portion of the MMF.
    ///
    /// This allows users to know the MMF's size without exposing it publicly in case someone has a `&mut MMF` because
    /// that would be very very dangerous.
    fn size(&self) -> usize {
        self.size
    }
}

/// Small struct wrapping a Windows type just to spare my eyes.
#[derive(Debug, Clone)]
pub struct MemoryMappedView {
    /// The address to use for reads and writes
    address: MEMORY_MAPPED_VIEW_ADDRESS,
}

/// I like `into()`
impl From<MEMORY_MAPPED_VIEW_ADDRESS> for MemoryMappedView {
    /// This literally just stuffs it into the only field
    fn from(value: MEMORY_MAPPED_VIEW_ADDRESS) -> Self {
        Self { address: value }
    }
}

/// Handle unmapping the view because we're nice like that.
impl MemoryMappedView {
    /// Unmaps the view to release resources.
    ///
    /// There is currently no way to undo this, short of closing the MMF.
    /// If you need to do or change something that causes unmapping of the view, and you do need to keep the relevant
    /// data, it's best to open a new MMF before closing it. When the last handle to an MMF closes, it's destroyed.
    fn unmap(&self) -> MMFResult<()> {
        match try_seh(|| unsafe { UnmapViewOfFile(self.address) })?.map_err(MMFError::from) {
            Err(MMFError::OS_OK(_)) | Ok(_) => Ok(()),
            err => err.map_err(|e| {
                eprintln!("Error unmapping the view of the MMF: {:#?}", e);
                e
            }),
        }
    }
}

/// Handle unmapping on drop.
impl Drop for MemoryMappedView {
    /// Unmap the view before dropping.
    fn drop(&mut self) {
        self.unmap().unwrap_or(())
    }
}

/// Implement closing the handle to the MMF before dropping it, so the system can clean up resources.
#[cfg(feature = "impl_mmf")]
impl<LOCK: MMFLock> Drop for MemoryMappedFile<LOCK> {
    /// Ignore any errors when closing the handle.
    fn drop(&mut self) {
        self.close().unwrap_or(())
    }
}

/// Send marker for use in shared contexts
///
/// # Safety
/// The default MMF implementation doesn't do anything unless the lock gives an all clear, so it's safe to mark it
/// `Send` when the lock itself is.
#[cfg(all(feature = "mmf_send", feature = "impl_mmf"))]
unsafe impl<LOCK: MMFLock + Send + Sync> Send for MemoryMappedFile<LOCK> {}

/// Sync marker for use in shared contexts
///
/// # Safety
/// The default MMF implementation doesn't do anything unless the lock gives an all clear, so it's safe to mark it
/// `Sync` when the lock itself is.
#[cfg(all(feature = "mmf_send", feature = "impl_mmf"))]
unsafe impl<LOCK: MMFLock + Send + Sync> Sync for MemoryMappedFile<LOCK> {}
