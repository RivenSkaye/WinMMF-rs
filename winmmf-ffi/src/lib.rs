#![feature(let_chains)]

use std::{
    ffi::{c_char, CStr},
    num::{NonZeroU32, NonZeroUsize},
    sync::{Mutex, OnceLock},
};
use winmmf::{states::RWLock, *};

type MMFWrapper<'a> = Mutex<Vec<MemoryMappedFile<RWLock<'a>>>>;
static MMFS: OnceLock<MMFWrapper> = OnceLock::new();

fn _init<'a>(cap: usize) -> MMFWrapper<'a> {
    Mutex::new(Vec::with_capacity(cap))
}

pub extern "system" fn init(count: Option<NonZeroU32>) -> u32 {
    let cap = count.map(|c| c.get()).unwrap_or(1) as usize;
    MMFS.set(_init(cap)).map(|_| 0).unwrap_or(1)
}

pub extern "system" fn open(size: u32, name: *const c_char, namespace: u8) -> u32 {
    if let Ok(ns) = namespace.try_into()
        && let Some(size) = NonZeroUsize::new(size as usize)
        && let Ok(namestr) = unsafe { CStr::from_ptr(name) }.to_str()
        && let Ok(mapped) = MemoryMappedFile::new(size, namestr, ns)
    {
        if MMFS.get_or_init(|| _init(1)).lock().map(|mut inner| inner.push(mapped)).is_ok() {
            0
        } else {
            2
        }
    } else {
        1
    }
}
