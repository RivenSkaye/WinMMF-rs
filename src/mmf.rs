//! Memory-Mapped Files, Rust-style
//!
//! This crate contains everything you need to work with Memory-Mapped files. Or you can just roll your own and build
//! upon the [`Mmf`] trait defined here. This module exports some utilities and ease of use items and you're entirely
//! free to use or not use them. By default, the implementations and namespaces are enabled. If you do not wish to do
//! so, look at the implementation for [`MemoryMappedFile`] and check the `use` statements to see what you need to do to
//! get things working.
//!
//! The internal implementation is built entirely around using [`fixedstr::zstr`] to keep references to strings alive
//! because for some reason everything goes to hell if you don't. MicroSEH is just as much a core component here, and
//! a macro is available for wrapping things that return errors from the windows crate.
//! Take a look at [`wrap_try!`] for more info.
//! ~~wdym don't pass refs to data you're dropping across the ffi boundary?~~
//!
//! While it would be possible to split things out further, using this much to ensure everything works smoothly helps
//! keeping this maintanable and usable. If you need a more minimal implementation, feel free to yank whatever you need
//! from here and instead building the crate without default features.

use super::{
    err::{Error as MMFError, MMFResult},
    states::{MMFLock, RWLock},
};
use fixedstr::{ztr32, ztr64};
use microseh::try_seh;
use windows::{
    core::Error as WErr,
    Win32::{
        Foundation::{SetLastError, HANDLE, WIN32_ERROR},
        System::Memory::{UnmapViewOfFile, MEMORY_MAPPED_VIEW_ADDRESS},
    },
};

#[cfg(feature = "impl_mmf")]
use std::{fmt, num::NonZeroUsize};
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
pub const LOCAL_NAMESPACE: ztr32 = ztr32::const_make("Local\\");
/// Global namespace prefix, requires SeCreateGlobal permission to create.
/// [See MSDN](https://learn.microsoft.com/en-us/windows/win32/memory/creating-named-shared-memory#first-process)
/// for more info
pub const GLOBAL_NAMESPACE: ztr32 = ztr32::const_make("Global\\");

#[cfg(feature = "namespaces")]
/// Namespaces as an enum, to unambiguously represent relevant information.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum Namespace {
    LOCAL,
    GLOBAL,
    CUSTOM,
}

#[cfg(feature = "namespaces")]
impl fmt::Display for Namespace {
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
    /// Write data to the MMF.
    fn write(&self, buffer: &[u8]) -> MMFResult<()>;
}

/// Replace the boilerplate for every time we need to call `try_seh`.
///
/// The function **must** return a result that can be converted into a [`MMFResult`]. If you're ever not certain it
/// does, manually wrap it in an `Ok()`. The resulting boilerplate is literally all it takes to further process the
/// code. The choice for a result is entirely based on the fact that most `windows-rs` calls return results, so it's
/// generally less effort this way.
macro_rules! wrap_try {
    ($func:expr, $res:ident) => {{
        let mut $res: Result<_, _> = Err(WErr::empty());
        let res2 = try_seh(|| $res = $func);
        if let Err(e) = res2 {
            // make sure the error code is set system-side
            unsafe { SetLastError(WIN32_ERROR(e.code() as u32)) };
            return Err(WErr::from_win32().into());
        }
        $res.map_err(MMFError::from)
    }};
}

/// A simple struct wrapping a [Memory Mapped File](https://learn.microsoft.com/en-us/windows/win32/memory/creating-named-shared-memory).
/// It contains all the data required to create and keep alive a [`HANDLE`] to a Memory Mapped File. The [`HANDLE`] is
/// required to create a [`MEMORY_MAPPED_VIEW_ADDRESS`] to read and write data from. In order to expose reading and
/// writing functionality in a safe manner, these are wrapped in a safe API that prefers more short blocks of unsafe
/// over a larger block that also does safe operations.
/// Once this reaches a stable-ish state, an unsafe function feature will be added to get access to the raw handle and
/// map view.
///
/// Supports both x86 and AMD64 by leveraging usize, to allow target-sized ints to be used everywhere.
#[derive(Debug)]
pub struct MemoryMappedFile {
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
    lock: Box<dyn MMFLock>,
    /// The original MemoryMappedView; We need to keep this around for unmapping it.
    map_view: Option<MemoryMappedView>,
    /// The pointer we can actually write into without fucking up the lock
    write_ptr: *mut u8,
}

#[cfg(feature = "impl_mmf")]
impl MemoryMappedFile {
    /// Attempt to create a new Memory Mapped File. Or fail _graciously_ if we can't.
    /// The size will be automatically divided into the upper and lower halves, as the function to allocate this memory
    /// requires them to be split. The name of the file should be either one of:
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
    /// will make a large part of the file inaccessible to other code trying to read it from a 32-bit process.
    /// The total size allocated will be 4 bytes larger than the specified size, but only after checking the input size
    /// is non-zero.
    pub fn new(size: NonZeroUsize, name: &str, namespace: Namespace) -> MMFResult<Self> {
        // Build the name to use for the MMF
        let init_name = match namespace {
            Namespace::GLOBAL => ztr64::make(&format!("{GLOBAL_NAMESPACE}{name}")),
            Namespace::LOCAL => ztr64::make(&format!("{LOCAL_NAMESPACE}{name}")),
            Namespace::CUSTOM => ztr64::make(name),
        };

        // fuckin' windows
        let mmf_name = PCSTR::from_raw(init_name.to_ptr());
        let (dw_low, dw_high) = (size.get() + 4).split();

        // Safety: handled through microSEH and we check the last error status later. Failure here is failure there.
        let handle = wrap_try!(
            unsafe { CreateFileMappingA(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, dw_high, dw_low, mmf_name) },
            hndl
        )?;

        // Unsafe because `MapViewOfFile` is marked as such, but it should return a NULL pointer when failing; and set
        // the last error state correspondingly.
        let map_view =
            wrap_try!(unsafe { Ok(MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, size.get() + 4)) }, mapview)?;

        // Explicit check to make sure we have something that works (later is now)
        if unsafe { GetLastError() }.is_err() {
            return Err(WErr::from_win32().into());
        }

        // Waste some time to ensure the memory is zeroed out - I learned the importance of this the hard way.
        let mut zeroing = Vec::<u8>::new();
        zeroing.resize(size.get() + 4, 0);
        // safety: we're writing zeroes into memory we just got back from the OS
        unsafe { std::ptr::copy(zeroing.as_ptr(), map_view.Value.cast(), zeroing.len()) };

        // safety: we just zeroed this memory out and we're initializing it freshly
        let lock = Box::new(unsafe { RWLock::from_raw(map_view.Value.cast()).initialize() });
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
        })
    }

    /// Open an existing MMF, if it exists.
    ///
    /// I have no idea what happens if you call this on a fake name. Code responsibly.
    /// In all reality though, it should return an error that you can handle.
    pub fn open(size: NonZeroUsize, name: &str, namespace: Namespace) -> MMFResult<Self> {
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
        let handle = wrap_try!(unsafe { OpenFileMappingA(FILE_MAP_ALL_ACCESS.0, false, mmf_name) }, hndl)?;

        // Unsafe because `MapViewOfFile` is marked as such, but it should return a NULL pointer when failing; and set
        // the last error state correspondingly.
        let map_view =
            wrap_try!(unsafe { Ok(MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, size.get() + 4)) }, mmva)?;

        // Explicit check to make sure we have something that works (later is now)
        if unsafe { GetLastError() }.is_err() {
            return Err(WErr::from_win32().into());
        }

        // Safety: We know where these bytes come from (ideally, they were opened by this lib)
        let lock = Box::new(unsafe { RWLock::from_existing(map_view.Value.cast()) });
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
        })
    }

    #[allow(dead_code)]
    /// Get the namespace of the file, if any. If an empty string is returned, it's Local.
    pub fn namespace(&self) -> String {
        self.name.split_once('\\').unwrap_or_default().0.to_owned()
    }

    #[allow(dead_code)]
    /// Return the filename the MMF is bound to, which is only the whole name if no namespace is provided.
    pub fn filename(&self) -> String {
        self.name.split_once('\\').map(|s| s.1.to_owned()).unwrap_or(self.name.to_string())
    }

    #[allow(dead_code)]
    /// Returns the stored name, which should be `[Namespace\]<FileName>`
    pub fn fullname(&self) -> String {
        self.name.to_string()
    }

    /// Close the MMF. Don't worry about calling this, it's handled in [`Drop`].
    pub fn close(&self) -> MMFResult<()> {
        // Safety: microSEH handles the OS side of this error, and the match handles this end.
        match wrap_try!(unsafe { CloseHandle(self.handle) }, res) {
            Err(MMFError::OS_OK(_)) | Ok(_) => Ok(()),
            err => err.inspect_err(|e| eprintln!("Error closing MMF's handle: {:#?}", e)),
        }
    }
}

#[cfg(feature = "impl_mmf")]
/// Implements a usable file-like interface for working with an MMF. Pass all input as bytes, please.
impl Mmf for MemoryMappedFile {
    /// Attempts to read the entirety of the data as defined in [`Self::size`].
    /// This function succeeds if there is a value in [`Self::map_view`] but it cannot guarantee the data returned is
    /// correct. This is an unfortunate side effect of having to work with raw pointers and bytes in memory.
    /// Assuming nothing external has touched the memory region other than this class, it _should_ be valid data unless
    /// it's marked as uninitialized.
    /// These errors use the most similar error codes from the system API:
    /// - 2: File not found; the MMF isn't opened yet or no map view exists.
    /// - 9: Invalid block; the lock is telling us this data has not yet been initialized.
    /// - 19: Write Protected; the file has a write lock on it which means reading might return incomplete data, or the
    ///   maximum amount of readers has been reached (this should not happen assuming all implementations are clean).
    #[inline]
    fn read(&self, count: usize) -> Result<Vec<u8>, MMFError> {
        let mut buf = Vec::with_capacity(self.size);
        self.read_to_buf(&mut buf, count)?;
        Ok(buf)
    }

    /// See the documentation for [Self::read()], except this takes a buffer to write to.
    /// If the buffer is smaller than the MMF, data will be truncated.
    fn read_to_buf(&self, buffer: &mut Vec<u8>, count: usize) -> Result<(), MMFError> {
        let to_read = if count == 0 { buffer.capacity().min(self.size) } else { count };
        if let Some(_) = &self.map_view {
            if !self.lock.initialized() {
                return Err(MMFError::Uninitialized);
            }
            if let Err(e) = self.lock.lock_read() {
                return Err(e);
            }
            // safety: memory may overlap with copy_to. With the size check, we also ensure we don't copy more bytes
            // than what fits. in the target Vec. If someone gave us a dirty Vec, that's on them. Notably, that would
            // cause the same kind of problems in safe code, because a dirty Vec violates soundness.
            unsafe {
                buffer.set_len(to_read);
                self.write_ptr.copy_to(buffer.as_mut_ptr(), to_read)
            };
            self.lock.unlock_read().unwrap();
            Ok(())
        } else {
            Err(MMFError::MMF_NotFound)
        }
    }

    /// Attempt to write a complete buffer into the MMF. Uses pointers and memcpy to be fast.
    /// This function errors only if the lock could not be acquired or when trying to write more data than fits. Writing
    /// more data than the MMF can hold is UB so this is prevented by erroring out instead. If the input buffer is
    /// smaller than the destination file, the end is zeroed out. The start of the buffer is also padded by the lock
    /// bytes to signal and flag locking.
    ///
    /// Error codes produced by this function:
    /// - 8: Not enough memory; the write was blocked because it was too large.
    /// - 5: Access denied; the lock could not be acquired.
    /// - 9: Invalid block; the lock is telling us this data has not yet been initialized.
    /// - All errors from [Self::read()] as a read is required to update the lock.
    fn write(&self, buffer: &[u8]) -> MMFResult<()> {
        let cap = buffer.len().min(self.size);
        if cap < buffer.len() {
            return Err(MMFError::NotEnoughMemory);
        }
        if !self.lock.initialized() {
            return Err(MMFError::Uninitialized);
        }
        if self.lock.readlocked() {
            return Err(MMFError::ReadLocked);
        }
        if self.lock.writelocked() {
            return Err(MMFError::WriteLocked);
        }
        if let Some(_) = &self.map_view {
            if let Err(_) = self.lock.lock_write() {
                return Err(MMFError::LockViolation);
            }
            let src_ptr = buffer.as_ptr();
            // We ensured this size is correct and filled out when instantiating the MMF, this is just writing the same
            // amount of bytes to the same place in memory.
            unsafe { src_ptr.copy_to(self.write_ptr, cap) };
            Ok(self.lock.unlock_write())
        } else {
            Err(MMFError::Uninitialized)
        }
    }
}

/// Small struct wrapping a Windows type just to spare my eyes.
#[derive(Debug, Clone)]
pub struct MemoryMappedView {
    address: MEMORY_MAPPED_VIEW_ADDRESS,
}

/// I like `into()`
impl From<MEMORY_MAPPED_VIEW_ADDRESS> for MemoryMappedView {
    fn from(value: MEMORY_MAPPED_VIEW_ADDRESS) -> Self {
        Self { address: value }
    }
}

/// Handle unmapping the view because we're nice like that.
impl MemoryMappedView {
    /// Unmaps the view.
    /// There is currently no way to undo this, short of closing the MMF.
    /// If you need to do or change something that causes unmapping of the view, and you do need to keep the relevant
    /// data, it's best to open a new MMF before closing it. When the last handle to an MMF closes, it's destroyed.
    fn unmap(&self) -> MMFResult<()> {
        match wrap_try!(unsafe { UnmapViewOfFile(self.address) }, res) {
            Err(MMFError::OS_OK(_)) | Ok(_) => Ok(()),
            err => err.inspect_err(|e| eprintln!("Error unmapping the view of the MMF: {:#?}", e)),
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

#[cfg(feature = "impl_mmf")]
/// Implement closing the handle to the MMF before dropping it, so the system can clean up resources.
impl Drop for MemoryMappedFile {
    fn drop(&mut self) {
        self.close().unwrap_or(())
    }
}
