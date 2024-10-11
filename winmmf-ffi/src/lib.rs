#![feature(let_chains)]

use ffi_support::FfiStr;
use std::{
    num::NonZeroUsize,
    sync::{Mutex, OnceLock},
};
pub use winmmf::Namespace as ValidNamespaces;
use winmmf::{states::RWLock, *};

/// You didn't think I was foing to keep _this_ long a type unaliased right?
type MMFWrapper<'a> = Mutex<Vec<MemoryMappedFile<RWLock<'a>>>>;

/// A wrapper to hold any MMFs that are produced during the application lifetime.
static MMFS: OnceLock<MMFWrapper> = OnceLock::new();

/// Lazy wrapper to use when ensuring initialization
fn _init<'a>(cap: usize) -> MMFWrapper<'a> {
    Mutex::new(Vec::with_capacity(cap))
}

/// Initialize the inner object to hold MMF instances.
///
/// Returns: 0 on success, 1 on error.
/// The only conceivable error state would be calling this function more than once.
#[no_mangle]
pub extern "system" fn init(count: Option<NonZeroUsize>) -> usize {
    let cap = count.map(|c| c.get()).unwrap_or(1);
    MMFS.set(_init(cap)).map(|_| 0).unwrap_or(1)
}

/// Open an existing MMF and push it into the list.
///
/// The caller is responsible for tracking current indices when more than one MMF is opened.
///
/// There are several possible return values here, these are:
/// 0: Success
/// 1: Size is 0
/// 2: The name is invalid UTF-8
/// 3: The namespace is invalid
/// 4: The MMF could not be opened
/// 5: The MMF could not be stored
#[no_mangle]
pub extern "system" fn open(size: Option<NonZeroUsize>, name: FfiStr, namespace: u8) -> usize {
    match (size, name.as_opt_str(), namespace.try_into()) {
        (None, _, _) => 1,
        (_, None, _) => 2,
        (_, _, Err(_)) => 3,
        (Some(size), Some(namestr), Ok(ns)) => {
            if let Ok(mapped) = MemoryMappedFile::open(size, namestr, ns) {
                MMFS.get_or_init(|| _init(1))
                    .lock()
                    .map(|mut inner| {
                        inner.push(mapped);
                        0
                    })
                    .unwrap_or(5)
            } else {
                4
            }
        }
    }
}

/// Create a new MMF and push it into the list.
///
/// The caller is responsible for tracking current indices when more than one MMF is opened.
///
/// There are several possible return values here, these are:
/// 0: Success
/// 1: Size is 0
/// 2: The name is invalid UTF-8
/// 3: The namespace is invalid
/// 4: The MMF could not be opened
/// 5: The MMF could not be stored
#[no_mangle]
pub extern "system" fn new(size: Option<NonZeroUsize>, name: FfiStr, namespace: u8) -> usize {
    match (size, name.as_opt_str(), namespace.try_into()) {
        (None, _, _) => 1,
        (_, None, _) => 2,
        (_, _, Err(_)) => 3,
        (Some(size), Some(namestr), Ok(ns)) => {
            if let Ok(mapped) = MemoryMappedFile::new(size, namestr, ns) {
                MMFS.get_or_init(|| _init(1))
                    .lock()
                    .map(|mut inner| {
                        inner.push(mapped);
                        0
                    })
                    .unwrap_or(5)
            } else {
                4
            }
        }
    }
}
