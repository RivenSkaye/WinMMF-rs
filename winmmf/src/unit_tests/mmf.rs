use crate::{mmf::*, states::RWLock};

use std::num::NonZeroUsize;
use windows::Win32::Foundation::{self as WFoundation, SetLastError};

const TESTSTRING: &[u8; 56] = b"This is a testing string to ensure WinMMF Just Works:TM:";

#[test]
pub fn test_write() {
    let file1 = MemoryMappedFile::<RWLock>::new(NonZeroUsize::new(64).unwrap(), "test_write", Namespace::LOCAL)
        .expect("creation failed");
    unsafe { SetLastError(WFoundation::WIN32_ERROR(0)) };
    file1.write(TESTSTRING.as_slice()).expect("Failed to write");
    drop(file1);
}

#[test]
pub fn test_read_self() {
    let file1 = MemoryMappedFile::<RWLock>::new(NonZeroUsize::new(64).unwrap(), "test_read_self", Namespace::LOCAL)
        .expect("creation failed");
    unsafe { SetLastError(WFoundation::WIN32_ERROR(0)) };
    file1.write(TESTSTRING.as_slice()).expect("Failed to write");
    let readback = file1.read(56).expect("Failed to read on 1");
    drop(file1);
    assert_eq!(&readback, TESTSTRING);
}

#[test]
pub fn test_read_other() {
    let file1 = MemoryMappedFile::<RWLock>::new(NonZeroUsize::new(64).unwrap(), "test_read_other", Namespace::LOCAL)
        .expect("creation failed");
    unsafe { SetLastError(WFoundation::WIN32_ERROR(0)) };
    file1.write(TESTSTRING.as_slice()).expect("Failed to write");
    let file2 =
        MemoryMappedFile::<RWLock>::open(NonZeroUsize::new(64).unwrap(), "test_read_other", Namespace::LOCAL, false)
            .expect("2nd open failed");
    unsafe { SetLastError(WFoundation::WIN32_ERROR(0)) };
    let readback = file2.read(56).expect("Failed to read");
    unsafe { SetLastError(WFoundation::WIN32_ERROR(0)) };

    drop(file1);
    drop(file2);
    assert_eq!(&readback, TESTSTRING);
}

#[test]
pub fn test_lock_reopen() {
    let file1 = MemoryMappedFile::<RWLock>::new(NonZeroUsize::new(64).unwrap(), "test_lock_reopen", Namespace::LOCAL)
        .expect("creation failed");
    unsafe { SetLastError(WFoundation::WIN32_ERROR(0)) };
    let file2 =
        MemoryMappedFile::<RWLock>::open(NonZeroUsize::new(64).unwrap(), "test_lock_reopen", Namespace::LOCAL, false)
            .expect("2nd open failed");
    unsafe { SetLastError(WFoundation::WIN32_ERROR(0)) };

    drop(file1);
    let file3 =
        MemoryMappedFile::<RWLock>::open(NonZeroUsize::new(64).unwrap(), "test_lock_reopen", Namespace::LOCAL, false)
            .expect("2nd open failed");
    file3.write(TESTSTRING.as_slice()).expect("Failed to write");
    let readback = file2.read(56).expect("Failed to read on 2");

    drop(file2);
    drop(file3);
    assert_eq!(&readback, TESTSTRING);
}

#[test]
pub fn test_no_use_after_close() {
    let file1 =
        MemoryMappedFile::<RWLock>::new(NonZeroUsize::new(64).unwrap(), "test_no_use_after_close", Namespace::LOCAL)
            .expect("creation failed");
    unsafe { SetLastError(WFoundation::WIN32_ERROR(0)) };
    let file2 = MemoryMappedFile::<RWLock>::open(
        NonZeroUsize::new(64).unwrap(),
        "test_no_use_after_close",
        Namespace::LOCAL,
        false,
    )
    .expect("2nd open failed");
    unsafe { SetLastError(WFoundation::WIN32_ERROR(0)) };

    file1.close().expect("Could not close MMF?");
    drop(file2);
    assert!(file1.read(64).is_err());
    drop(file1);
}

#[test]
pub fn test_no_exist_after_close() {
    let file1 =
        MemoryMappedFile::<RWLock>::new(NonZeroUsize::new(64).unwrap(), "test_no_exist_after_close", Namespace::LOCAL)
            .expect("creation failed");
    unsafe { SetLastError(WFoundation::WIN32_ERROR(0)) };
    file1.write(TESTSTRING.as_slice()).expect("Failed to write");
    drop(file1);

    let file2 =
        MemoryMappedFile::<RWLock>::new(NonZeroUsize::new(64).unwrap(), "test_no_exist_after_close", Namespace::LOCAL)
            .expect("2nd open failed");
    unsafe { SetLastError(WFoundation::WIN32_ERROR(0)) };
    let file3 = MemoryMappedFile::<RWLock>::open(
        NonZeroUsize::new(64).unwrap(),
        "test_no_exist_after_close",
        Namespace::LOCAL,
        false,
    )
    .expect("2nd open failed");
    let readback = file3.read(56).expect("Failed to read");
    unsafe { SetLastError(WFoundation::WIN32_ERROR(0)) };
    drop(file2);
    drop(file3);
    assert_ne!(&readback, TESTSTRING);
}
