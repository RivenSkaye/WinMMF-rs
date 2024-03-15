use super::states::RWLock;
use fixedstr::{ztr32, ztr64};
use microseh::try_seh;
use std::num::NonZeroU32;
use windows::{
    core::{Error as WErr, HRESULT, PCSTR},
    Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        System::Memory::{
            CreateFileMappingA, MapViewOfFile, OpenFileMappingA, UnmapViewOfFile,
            FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
        },
    },
};

pub const LOCAL_NAMESPACE: ztr32 = ztr32::const_make("Local\\");
pub const GLOBAL_NAMESPACE: ztr32 = ztr32::const_make("Global\\");

#[repr(u8)]
pub enum Namespace {
    LOCAL,
    GLOBAL,
    CUSTOM,
}

pub trait Mmf {
    fn read(&self, count: usize) -> Result<Vec<u8>, WErr>;
    fn read_to_buf(&self, buffer: &mut Vec<u8>, count: usize) -> Result<(), WErr>;
    fn write(&self, buffer: &[u8]) -> Result<(), WErr>;
}

/// Replace the boilerplate for every time we need to call `try_seh`.
/// The function **must** return a result, if you're ever not certain it does. manually wrap it in an `Ok()`. The
/// resulting boilerplate is literally all it takes to further process the code. The choice for a result is entirely
/// based on the fact that most `windows-rs` calls return results, so it's generally less effort this way.
macro_rules! wrap_try {
    ($func:expr) => {{
        let mut res: Result<_, WErr> = Err(WErr::empty());
        if let Err(e) = try_seh(|| res = $func) {
            return Err(WErr::new(HRESULT(e.code() as i32).into(), e.to_string()));
        }
        res?
    }};
}

/// I can't believe this is now required. WTAF is MS doing here?
const WIN_OK: WErr = WErr::empty();

/// A simple struct wrapping a [Memory Mapped File](https://learn.microsoft.com/en-us/windows/win32/memory/creating-named-shared-memory).
/// It contains all the data required to create and keep alive a [`HANDLE`] to a Memory Mapped File. The [`HANDLE`] is
/// required to create a [`MEMORY_MAPPED_VIEW_ADDRESS`] to read and write data from. In order to expose reading and
/// writing functionality in a safe manner, these are wrapped in a safe API that prefers more short blocks of unsafe
/// over a larger block that also does safe operations.
/// Once this reaches a stable-ish state, an unsafe function feature will be added to get access to the raw handle and
/// map view.
///
/// Currently the size is restricted to a u32 to prevent problems with `isize`/`usize` on 32-bit systems. I could opt to
/// only support 64-bit, but knowing the edge cases with Windows in environments where this can pop up, that will
/// probably end up being a footgun for myself. Will look into doing something with usize maybe. Though do people need
/// 2^64 bytes worth of data for a single MMF? At that point, why not open another instead?
#[derive(Debug)]
pub struct MemoryMappedFile<'a> {
    /// The [`HANDLE`] to the created mapping
    handle: HANDLE,
    /// the "filename" portion
    name: ztr64,
    /// The higher order bits for the size of the opened file.
    /// Always 0 for now, but this might change if a use case exists for more than 2^32 bytes allocated and mapped into
    /// memory at the same time.
    #[allow(dead_code)]
    size_high_order: u32,
    /// The lower order bits for the size of the opened file.
    #[allow(dead_code)]
    size_low_order: u32,
    /// The total size, should be the same as [`Self::size_low_order`].
    /// Might change in a future version if I can be arsed to write pointer-sized code
    size: u32,
    /// The lock. ~~3 pointers to their own bytes~~
    lock: RWLock<'a>,
    /// The original MemoryMappedView; We need to keep this around for unmapping it.
    map_view: Option<MemoryMappedView>,
    /// The pointer we can actually write into without fucking up the lock
    write_ptr: *mut u8,
}

impl<'a> MemoryMappedFile<'a> {
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
    /// The size MUST be a non-zero value; allocating zero bytes errors on the OS end of things. The choice for a u32
    /// was made to guarantee compatibility with 32-bit systems AND 32-bit compiled applications. Allocating too much
    /// will make a large part of the file inaccessible to other code trying to read it from a 32-bit process.
    /// The total size allocated will be 4 bytes larger than the specified size, but only after checking the input size
    /// is non-zero.
    pub fn new(size: NonZeroU32, name: &str, namespace: Namespace) -> Result<Self, WErr> {
        // Build the name to use for the MMF
        let init_name = match namespace {
            Namespace::GLOBAL => ztr64::make(&format!("{GLOBAL_NAMESPACE}{name}")),
            Namespace::LOCAL => ztr64::make(&format!("{LOCAL_NAMESPACE}{name}")),
            Namespace::CUSTOM => ztr64::make(name),
        };
        // fuckin' windows
        let mmf_name = PCSTR::from_raw(init_name.to_ptr());
        // Acquire a handle and exit if we snag an error
        let handle = wrap_try!(unsafe {
            CreateFileMappingA(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                size.get() + 4,
                mmf_name,
            )
        });

        // Unsafe because `MapViewOfFile` is marked as such, but it should return a NULL pointer when failing; and set
        // the last error state correspondingly.
        let map_view = wrap_try!(unsafe {
            Ok(MapViewOfFile(
                handle,
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                size.get() as usize + 4,
            ))
        });

        // Waste some time to ensure the memory is zeroed out - I learned the importance of this the hard way.
        let mut zeroing = Vec::with_capacity(size.get() as usize + 4);
        zeroing.resize(size.get() as usize + 4, 0);
        unsafe { std::ptr::copy(zeroing.as_ptr(), map_view.Value.cast(), zeroing.len()) };

        // safety:
        let lock = unsafe { RWLock::from_raw(map_view.Value.cast()).initialize() };
        let write_ptr = unsafe { map_view.Value.cast::<u8>().add(4) };
        Ok(Self {
            handle,
            name: init_name,
            size_high_order: 0,
            size_low_order: size.get(),
            size: size.get(),
            lock,
            map_view: Some(map_view.into()),
            write_ptr,
        })
    }

    pub fn open(size: NonZeroU32, name: &str, namespace: Namespace) -> Result<Self, WErr> {
        // Build the name to use for the MMF
        let init_name = match namespace {
            Namespace::GLOBAL => ztr64::make(&format!("{GLOBAL_NAMESPACE}{name}")),
            Namespace::LOCAL => ztr64::make(&format!("{LOCAL_NAMESPACE}{name}")),
            Namespace::CUSTOM => ztr64::make(name),
        };
        // fuckin' windows
        let mmf_name = PCSTR::from_raw(init_name.to_ptr());
        let handle = wrap_try!(unsafe { OpenFileMappingA(FILE_MAP_ALL_ACCESS.0, false, mmf_name) });
        // Unsafe because `MapViewOfFile` is marked as such, but it should return a NULL pointer when failing; and set
        // the last error state correspondingly.
        let map_view = wrap_try!(unsafe {
            Ok(MapViewOfFile(
                handle,
                FILE_MAP_ALL_ACCESS,
                0,
                0,
                size.get() as usize + 4,
            ))
        });
        let lock = unsafe { RWLock::from_existing(map_view.Value.cast()) };
        let write_ptr = unsafe { map_view.Value.cast::<u8>().add(4) };
        Ok(Self {
            handle,
            name: init_name,
            size_high_order: 0,
            size_low_order: size.get(),
            size: size.get(),
            lock,
            map_view: Some(map_view.into()),
            write_ptr,
        })
    }

    #[allow(dead_code)]
    pub fn namespace(&self) -> String {
        self.name.split_once('\\').unwrap_or_default().0.to_owned()
    }

    #[allow(dead_code)]
    pub fn filename(&self) -> String {
        self.name
            .split_once('\\')
            .map(|s| s.1.to_owned())
            .unwrap_or(self.name.to_string())
    }

    #[allow(dead_code)]
    pub fn fullname(&self) -> String {
        self.name.to_string()
    }

    pub fn close(&self) -> Result<(), WErr> {
        match wrap_try!(unsafe { Ok(CloseHandle(self.handle)) }) {
            Err(WIN_OK) | Ok(_) => Ok(()),
            err => err.inspect_err(|e| {
                eprintln!(
                    "Error closing MMF's handle ({}): {:#?}",
                    e.code(),
                    e.message()
                )
            }),
        }
    }
}

impl<'a> Mmf for MemoryMappedFile<'a> {
    /// Attempts to read the entirety of the data as defined in [`Self::size`].
    /// This function succeeds if there is a value in [`Self::map_view`] but it cannot guarantee the data returned is
    /// correct. This is an unfortunate side effect of having to work with raw pointers and bytes in memory.
    /// Assuming nothing external has touched the memory region other than this class, it _should_ be valid data unless
    /// it's marked as uninitialized.
    /// The errors returned from this function can be retrieved by calling [`WErr::code()`] and those are repurposed as
    /// follows:
    /// - 2: File not found; the MMF isn't opened yet or no map view exists.
    /// - 9: Invalid block; the lock is telling us this data has not yet been initialized.
    /// - 19: Write Protected; the file has a write lock on it which means reading might return incomplete data, or the
    ///   maximum amount of readers has been reached (this should not happen assuming all implementations are clean).
    #[inline]
    fn read(&self, count: usize) -> Result<Vec<u8>, WErr> {
        let mut buf = Vec::with_capacity(self.size as usize);
        self.read_to_buf(&mut buf, count)?;
        Ok(buf)
    }

    /// See the documentation for [Self::read()], except this takes a buffer to write to.
    /// If the buffer is smaller than the MMF, data will be truncated.
    fn read_to_buf(&self, buffer: &mut Vec<u8>, count: usize) -> Result<(), WErr> {
        let to_read = if count == 0 {
            buffer.capacity().min(self.size as usize)
        } else {
            count
        };
        if let Some(_) = &self.map_view {
            if !self.lock.initialized() {
                return Err(WErr::new(HRESULT(9).into(), "File is uninitialized!"));
            }
            if self.lock.writelocked() {
                return Err(WErr::new(HRESULT(19).into(), "File is writelocked!"));
            }
            if let Err(_) = self.lock.lock_read() {
                return Err(WErr::new(
                    HRESULT(19).into(),
                    "The lock has hit the maximum amount of readers!",
                ));
            }
            // safety: the buffer is allocated elsewhere, so we know the memory doesn't overlap. With the size check, we
            // also ensure we don't copy more bytes than what fits
            unsafe {
                buffer.set_len(to_read);
                self.write_ptr.copy_to(buffer.as_mut_ptr(), to_read)
            };
            self.lock.unlock_read().unwrap();
            Ok(())
        } else {
            Err(WErr::new(
                HRESULT(2).into(),
                "No memory mapped file has been opened yet!",
            ))
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
    fn write(&self, buffer: &[u8]) -> Result<(), WErr> {
        let cap = buffer.len().min(self.size as usize);
        if cap < buffer.len() {
            return Err(WErr::new(HRESULT(19).into(), "Buffer too large!"));
        }
        if !self.lock.initialized() {
            return Err(WErr::new(HRESULT(9).into(), "File is uninitialized!"));
        }
        if self.lock.locked() {
            return Err(WErr::new(HRESULT(5).into(), "File is locked elsewhere!"));
        }
        if let Some(_) = &self.map_view {
            if let Err(_) = self.lock.lock_write() {
                return Err(WErr::new(HRESULT(5).into(), "Lock could not be acquired!"));
            }
            let src_ptr = buffer.as_ptr();
            // We ensured this size is correct and filled out when instantiating the MMF, this is just writing the same
            // amount of bytes to the same place in memory.
            unsafe { src_ptr.copy_to(self.write_ptr, cap) };
            Ok(self.lock.unlock_write())
        } else {
            Err(WErr::new(
                HRESULT(9).into(),
                "No memory mapped file has been opened yet!",
            ))
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemoryMappedView {
    address: MEMORY_MAPPED_VIEW_ADDRESS,
}

impl From<MEMORY_MAPPED_VIEW_ADDRESS> for MemoryMappedView {
    fn from(value: MEMORY_MAPPED_VIEW_ADDRESS) -> Self {
        Self { address: value }
    }
}

impl MemoryMappedView {
    fn unmap(&self) -> Result<(), WErr> {
        match wrap_try!(unsafe { Ok(UnmapViewOfFile(self.address)) }) {
            Err(WIN_OK) | Ok(_) => Ok(()),
            err => err.inspect_err(|e| {
                eprintln!(
                    "Error unmapping the view of the MMF ({}): {:#?}",
                    e.code(),
                    e.message()
                )
            }),
        }
    }
}

impl Drop for MemoryMappedView {
    fn drop(&mut self) {
        self.unmap().unwrap_or(())
    }
}

impl<'a> Drop for MemoryMappedFile<'a> {
    fn drop(&mut self) {
        self.close().unwrap_or(())
    }
}
