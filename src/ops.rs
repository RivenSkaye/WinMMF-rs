use windows::{
    core::Error as WErr,
    Win32::Foundation::{self as WFoundation, SetLastError},
};

use super::mmf::*;
use std::num::NonZeroU32;

pub fn test_mmf(input: &[u8]) -> Result<(), WErr> {
    let file1 =
        MemoryMappedFile::new(NonZeroU32::new(64).unwrap(), "testfile", Namespace::LOCAL).expect("creation failed");
    unsafe { SetLastError(WFoundation::WIN32_ERROR(0)) };
    file1.write(input).expect("Failed to write");

    let file2 =
        MemoryMappedFile::open(NonZeroU32::new(64).unwrap(), "testfile", Namespace::LOCAL).expect("2nd open failed");
    unsafe { SetLastError(WFoundation::WIN32_ERROR(0)) };

    println!("{file1:#?}");
    println!("{file2:#?}");

    let readback_1 = file1.read(input.len()).expect("Failed to read on 1");
    assert_eq!(&readback_1, input);

    let readback = file2.read(input.len()).expect("Failed to read");
    unsafe { SetLastError(WFoundation::WIN32_ERROR(0)) };

    assert_eq!(&readback, input);
    println!("{}", String::from_utf8(readback).unwrap());

    println!("{file1:#?}");
    println!("{file2:#?}");

    Ok(println!("Done testing! Read: {}", String::from_utf8_lossy(readback_1.as_slice())))
}
