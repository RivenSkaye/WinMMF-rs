//! Errors produced and introduced in this crate, to help users figure out what went wrong.
//!
//! Handling these is recommended, but if you don't then either the MMF was never opened, or will be closed when the
//! program ends.

use std::{error::Error as stderr, fmt};
use windows::core::{Error as WErr, HRESULT};

/// Errors used with Memory-Mapped Files.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone)]
#[repr(u8)]
pub enum Error {
    /// Readlocked, don't write.
    ReadLocked = 0,
    /// Writelocked, don't touch.
    WriteLocked = 1,
    /// Uninitialized, who's to say what's in there?
    /// Although MMFs created in this crate will just be nulls.
    Uninitialized = 2,
    /// 255 concurrent readers, wtf
    MaxReaders = 3,
    /// It's too big ~~onii-chan~~
    NotEnoughMemory = 4,
    /// These are not the bytes you're looking for
    MMF_NotFound = 5,
    /// Something else was racing you, this is scary.
    LockViolation = 6,
    /// No explanation, only errors
    GeneralFailure = 253,
    /// Generic OS error that we can't do much with other than catching and forwarding
    OS_Err(WErr) = 254,
    /// Yes, Windows provides an error when everything is OK. Task failed successfully.
    OS_OK(WErr) = 255,
}

impl stderr for Error {
    fn source(&self) -> Option<&(dyn stderr + 'static)> {
        match self {
            Self::OS_Err(w) => Some(w),
            _ => None,
        }
    }
}

impl From<WErr> for Error {
    fn from(value: WErr) -> Self {
        match value.code().into() {
            HRESULT(0) => Self::OS_OK(value),
            HRESULT(19) => Self::WriteLocked,
            HRESULT(30) => Self::ReadLocked,
            HRESULT(33) => Self::LockViolation,
            HRESULT(8) => Self::NotEnoughMemory,
            HRESULT(2) => Self::MMF_NotFound,
            HRESULT(9) => Self::Uninitialized,
            _ => Self::OS_Err(value),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = String::from(match self {
            Self::OS_OK(_) => "Task failed successfully!".to_owned(),
            Self::WriteLocked => "Memory Mapped File was locked for writing".to_owned(),
            Self::ReadLocked => "Memory Mapped File was locked for reading".to_owned(),
            Self::LockViolation => "MMF was locked between checking and acquiring the lock!".to_owned(),
            Self::NotEnoughMemory => "The requested write was larger than the buffer size".to_owned(),
            Self::MMF_NotFound => "E002: No memory mapped file has been opened yet!".to_owned(),
            Self::Uninitialized => "Memory Mapped File was not yet initialized".to_owned(),
            Self::MaxReaders => "The maximum amount of readers is already registered".to_owned(),
            Self::GeneralFailure => "No idea what the hell happened here...".to_owned(),
            Self::OS_Err(c) => format!("E{c:02}: Generic OS Error"),
        });
        write!(f, "{text}: {}", self.source().map(|e| e.to_string()).unwrap_or("occurred in this crate.".to_owned()))
    }
}

/// Thin wrapper type for [`Result`]s we produced.
pub type MMFResult<T> = Result<T, Error>;
