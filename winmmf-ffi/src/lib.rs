#![feature(let_chains)]

//! # An FFI interface for WinMMF
//!
//! The recommended way of working with this interface is therefore to call this function for the first read, reuse the
//! pointer provided for all subsequent reads by calling [`read_buf`], and then freeing it with [`free_result`] with
//! other teardown and exit steps for your program. If the lifetime for any MMFs should be `&'static` it's possible to
//! leave cleanup to the OS. But no guarantees are made if or when that happens.
//!
//! During the lifetime of your program, if you decide to close any MMFs, they will be ejected from the inner
//! collection. Should you need to reopen one, and you're sure other handles to it yet live in the system, you can open
//! it anew and your data should be there unchanged.
//! Should you forget to free a pointer, use [`free_raw`] at your own risk.

use ffi_support::FfiStr;
use std::{
    num::NonZeroUsize,
    ptr::null_mut,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex, OnceLock,
    },
};
pub use winmmf::Namespace as ValidNamespaces;
use winmmf::{states::RWLock, *};

/// You didn't think I was going to keep _this_ long a type unaliased right?
type MMFWrapper<'a> = Mutex<Vec<MemoryMappedFile<RWLock<'a>>>>;

/// A wrapper to hold any MMFs that are produced during the application lifetime.
static MMFS: OnceLock<MMFWrapper> = OnceLock::new();
/// Currently selected default MMF to operate on. Counting starts from 1.
static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// Lazy wrapper to use when ensuring initialization
fn _init<'a>(cap: usize) -> MMFWrapper<'a> {
    Mutex::new(Vec::with_capacity(cap))
}

/// Initialize the inner object to hold MMF instances.
///
/// Returns: 0 on success, -1 on error.
/// The only conceivable error state would be calling this function more than once.
#[no_mangle]
pub extern "system" fn init(count: Option<NonZeroUsize>) -> isize {
    let cap = count.map(|c| c.get()).unwrap_or(1);
    MMFS.set(_init(cap)).map(|_| 0).unwrap_or(-1)
}

/// Open an existing MMF and push it into the list, returning its index or an error indicator.
///
/// There are several possible return values here, these are:
///
/// - Positive integers: the new index
/// - -1: Size is 0
/// - -2: The name is invalid UTF-8
/// - -3: The namespace is invalid
/// - -4: The MMF could not be opened
/// - -5: The MMF could not be stored
#[no_mangle]
pub extern "system" fn open(size: Option<NonZeroUsize>, name: FfiStr, namespace: u8) -> isize {
    match (size, name.as_opt_str(), namespace.try_into()) {
        (None, _, _) => -1,
        (_, None, _) => -2,
        (_, _, Err(_)) => -3,
        (Some(size), Some(namestr), Ok(ns)) => {
            if let Ok(mapped) = MemoryMappedFile::open(size, namestr, ns, false) {
                MMFS.get_or_init(|| _init(1))
                    .lock()
                    .map(|mut inner| {
                        inner.push(mapped);
                        let idx = inner.len() - 1;
                        _ = CURRENT.compare_exchange(0, idx, Ordering::Acquire, Ordering::Relaxed);
                        idx as isize
                    })
                    .unwrap_or(-5)
            } else {
                -4
            }
        }
    }
}

/// Create a new MMF and push it into the list, returning the new index or an error indicator.
///
/// There are several possible return values here, these are:
///
/// - Positive integers: Success
/// - -1: Size is 0
/// - -2: The name is invalid UTF-8
/// - -3: The namespace is invalid
/// - -4: The MMF could not be opened
/// - -5: The MMF could not be stored
#[no_mangle]
pub extern "system" fn new(size: Option<NonZeroUsize>, name: FfiStr, namespace: u8) -> isize {
    match (size, name.as_opt_str(), namespace.try_into()) {
        (None, _, _) => -1,
        (_, None, _) => -2,
        (_, _, Err(_)) => -3,
        (Some(size), Some(namestr), Ok(ns)) => {
            if let Ok(mapped) = MemoryMappedFile::new(size, namestr, ns) {
                MMFS.get_or_init(|| _init(1))
                    .lock()
                    .map(|mut inner| {
                        inner.push(mapped);
                        let idx = inner.len() - 1;
                        _ = CURRENT.compare_exchange(0, idx, Ordering::Acquire, Ordering::Relaxed);
                        idx as isize
                    })
                    .unwrap_or(-5)
            } else {
                -4
            }
        }
    }
}

/// Read `count` bytes from the MMF into the provided buffer.
///
/// It is up to the caller to ensure the buffer is large enough to hold at least `count` bytes. Passing in a buffer
/// smaller than `count` from Rust space is undefined behavior. This function _will_ make the assumption the buffer is
/// exactly `count` items long.
/// Return values are negative integers for errors, or 0 for success.
///
/// # Safety
/// Ensure `buff` is valid for at least `count` bytes and all will be well.
///
/// - -1: No MMFs opened yet
/// - -2: MMF is closed
/// - -3: MMF isn't initialized
/// - -4: ???
#[no_mangle]
pub unsafe extern "system" fn read_buf(mmf_idx: Option<NonZeroUsize>, count: usize, buff: *mut u8) -> isize {
    if buff.is_null() {
        return -4;
    }
    if count == 0 {
        return 0;
    }
    MMFS.get()
        .map(|inner| {
            inner
                .lock()
                .map(|inner| {
                    inner
                        .get(mmf_idx.map(|nsu| nsu.get()).unwrap_or_else(|| CURRENT.load(Ordering::Acquire)))
                        .map(|mmf| {
                            mmf.read_to_raw(buff, count).map(|_| 0).unwrap_or_else(|e| match e {
                                Error::MMF_NotFound => -2,
                                Error::Uninitialized => -3,
                                _ => -4,
                            })
                        })
                        .unwrap_or(-1)
                })
                .unwrap_or(-4)
        })
        .unwrap_or(-1)
}

/// Read `count` bytes or all contents from the MMF and give back a pointer to the data.
///
/// The pointer produced from this function **must** be freed using [`free_result`], regardless of error state.
/// To this end, the returned pointer will _always_ have enough size behind it to fit the entire mapped view. Before
/// freeing it, this pointer may also be used with [read_buf] so you know you have a safe pointer to work with.
/// To further support this, passing a `count` of 0 returns a fresh buffer
///
/// If something went wrong, the data behind the pointer will be an error code, right padded with `0xFF` until the end
/// of the requested buffer. If no size is provided, the returned pointer will be the length of the current active MMF.
#[no_mangle]
pub extern "system" fn read(mmf_idx: Option<NonZeroUsize>, count: usize) -> *mut u8 {
    MMFS.get()
        .map(|inner| {
            inner
                .lock()
                .map(|inner| {
                    inner
                        .get(mmf_idx.map(|nsu| nsu.get()).unwrap_or_else(|| CURRENT.load(Ordering::Acquire)))
                        .map(|mmf| {
                            if count == 0 {
                                let mut ret = vec![0; mmf.size()];
                                ret.shrink_to_fit();
                                let ptr = ret.as_mut_ptr();
                                std::mem::forget(ret);
                                ptr
                            } else {
                                let mut ret = Vec::new();
                                let ptr = ret.as_mut_ptr();

                                match mmf.read_to_buf(&mut ret, count) {
                                    Ok(_) => {
                                        std::mem::forget(ret);
                                        ptr
                                    } /* Becomes a pointer to the first */
                                    // element in the vec
                                    Err(e) => {
                                        let val = match e {
                                            Error::MMF_NotFound => -2_i8,
                                            Error::Uninitialized => -3_i8,
                                            _ => -4_i8,
                                        };
                                        /*Error::MMF_NotFound => -2_i8,
                                        Error::Uninitialized => -3_i8,
                                        _ => -4_i8, */
                                        ret = vec![0xFF; mmf.size()];
                                        ret[0] = val as u8;
                                        ret.shrink_to_fit();
                                        std::mem::forget(ret);
                                        ptr
                                    }
                                }
                            }
                        })
                        .unwrap_or(null_mut())
                })
                .unwrap_or(null_mut())
        })
        .unwrap_or(null_mut())
}

/// Free a pointer used for reading from an MMF by its index number.
///
/// # Safety
/// Do not pass pointers not received from this library. Doing so is UB by definition.
/// Null pointers will be silently ignored.
#[no_mangle]
pub unsafe extern "system" fn free_result(mmf_idx: Option<NonZeroUsize>, res: *mut u8) {
    if res.is_null() {
        return;
    }
    MMFS.get()
        .map(|inner| {
            inner
                .lock()
                .map(|inner| {
                    inner
                        .get(mmf_idx.map(|nsu| nsu.get()).unwrap_or_else(|| CURRENT.load(Ordering::Acquire)))
                        .map(|mmf| unsafe { free_raw(res, mmf.size()) })
                        .unwrap_or(())
                })
                .unwrap_or(())
        })
        .unwrap_or(())
}

/// You had better know how big that thing is.
///
/// # Safety
///
/// If the provided size is incorrect, you might be leaking bytes (too small, mostly harmless) or you might be invoking
/// UB (too large, harmful to the universe). If you're just gambling the size, I hope you anger the Duolingo bird.
#[no_mangle]
pub unsafe extern "system" fn free_raw(res: *mut u8, size: usize) {
    drop(Vec::from_raw_parts(res, size, size))
}

/// Expose writing data as well. Slightly less complex for FFI purposes than reading.
///
/// # Safety
/// `data` must be at least `count` bytes long, or somebody's getting hurt.
///
/// Return values for this function are:
/// - 0: Write was successful!
/// - -1: Writing not allowed (readonly or closed)
/// - -2: Buffer is bigger than the MMF
/// - -3: Uninitialized
/// - -4: Read- or WriteLocked
/// - -5: Programmer issue
#[no_mangle]
pub unsafe extern "system" fn write(mmf_idx: Option<NonZeroUsize>, data: *mut u8, size: usize) -> isize {
    if data.is_null() {
        -5
    } else if size > 0 {
        MMFS.get()
            .map(|inner| {
                inner
                    .lock()
                    .map(|inner| {
                        inner
                            .get(mmf_idx.map(|nsu| nsu.get()).unwrap_or_else(|| CURRENT.load(Ordering::Acquire)))
                            .map(|mmf| {
                                let buff = unsafe { std::slice::from_raw_parts_mut(data, size) };
                                match mmf.write(buff) {
                                    Ok(_) => 0,
                                    Err(Error::MMF_NotFound) => -1,
                                    Err(Error::NotEnoughMemory) => -2,
                                    Err(Error::Uninitialized) => -3,
                                    Err(Error::ReadLocked) | Err(Error::WriteLocked) => -4,
                                    _ => -5,
                                }
                            })
                            .unwrap_or(-3)
                    })
                    .unwrap_or(-5)
            })
            .unwrap_or(-5)
    } else {
        0 // Copying zero bytes is always successful.
    }
}

/// Convenience function to open a read-only MMF and get a usable pointer for future read calls.
///
/// - If you pass in a size of 0, you get a null pointer.
/// - If you don't provide a usable name, you get a null pointer.
/// - If you pass an invalid namespace, you get a null pointer.
/// - If the returned pointer is not null, it's valid until you close the MMF. All bytes behind it will be its index,
///   unless you have more than 255 MMFs open. Please don't open that many...
///
/// This pointer still has to be [`free`d][free_result]
#[no_mangle]
pub extern "system" fn open_ro(size: Option<NonZeroUsize>, name: FfiStr, namespace: u8) -> *mut u8 {
    match (size, name.as_opt_str(), namespace.try_into()) {
        (None, _, _) => null_mut(),
        (_, None, _) => null_mut(),
        (_, _, Err(_)) => null_mut(),
        (Some(size), Some(namestr), Ok(ns)) => {
            if let Ok(mapped) = MemoryMappedFile::open(size, namestr, ns, true) {
                MMFS.get_or_init(|| _init(1))
                    .lock()
                    .map(|mut inner| {
                        inner.push(mapped);
                        let count = inner.len() - 1;
                        _ = CURRENT.compare_exchange(0, count, Ordering::Acquire, Ordering::Relaxed);
                        vec![count.min(0xFF) as u8; count] // clamp and truncate
                    })
                    .map(|mut ret| {
                        let ptr = ret.as_mut_ptr();
                        std::mem::forget(ret);
                        ptr
                    })
                    .unwrap_or(null_mut())
            } else {
                null_mut()
            }
        }
    }
}

/// Close the MMF
///
/// Closes the specific instance stored here without interferring with other processes that might be using it.
#[no_mangle]
pub extern "system" fn close(mmf_idx: usize) {
    MMFS.get()
        .map(|inner| inner.lock().map(|mut inner| drop(inner.remove(mmf_idx))).unwrap_or_default())
        .unwrap_or_default()
}
