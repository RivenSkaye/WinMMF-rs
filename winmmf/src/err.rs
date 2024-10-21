#![deny(clippy::missing_docs_in_private_items)]
#![deny(missing_docs)]
//! Errors produced and introduced in this crate, to help users figure out what went wrong.
//!
//! Handling these is recommended, but if you don't then either the MMF was never opened,
//! or it will be closed when the program ends.

use microseh::Exception;
use std::{borrow::Cow, error::Error as stderr, fmt};
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
    /// 127+ concurrent readers, wtf
    MaxReaders = 3,
    /// It's too big ~~onii-chan~~
    NotEnoughMemory = 4,
    /// These are not the bytes you're looking for
    MMF_NotFound = 5,
    /// Something else was racing you, this is scary.
    LockViolation = 6,
    MaxTriesReached = 7,
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

impl From<HRESULT> for Error {
    fn from(value: HRESULT) -> Self {
        match value {
            HRESULT(30) => Self::ReadLocked,
            HRESULT(19) => Self::WriteLocked,
            HRESULT(8) => Self::NotEnoughMemory,
            HRESULT(9) => Self::Uninitialized,
            HRESULT(2) => Self::MMF_NotFound,
            HRESULT(33) => Self::LockViolation,
            HRESULT(0) => Self::OS_OK(value.into()),
            _ => Self::OS_Err(value.into()),
        }
    }
}

impl From<i32> for Error {
    fn from(value: i32) -> Self {
        Self::from(HRESULT(value))
    }
}

impl From<u32> for Error {
    fn from(value: u32) -> Self {
        // pub struct HRESULT(pub i32) is `repr(transparent)`
        Self::from(value as i32)
    }
}

impl From<WErr> for Error {
    fn from(value: WErr) -> Self {
        // Extract the inner HRESULT
        Self::from(value.code())
    }
}

impl From<Exception> for Error {
    fn from(value: Exception) -> Self {
        // microseh::Exception::ExceptionCode is a `repr(u32)` enum. This is safe to cast.
        Self::from(HRESULT(value.code() as u32 as i32))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text: Cow<str> = match self {
            Self::OS_OK(_) => Cow::from("Task failed successfully!"),
            Self::WriteLocked => Cow::from("Memory Mapped File was locked for writing"),
            Self::ReadLocked => Cow::from("Memory Mapped File was locked for reading"),
            Self::LockViolation => Cow::from("MMF was locked between checking and acquiring the lock!"),
            Self::NotEnoughMemory => Cow::from("The requested write was larger than the buffer size"),
            Self::MMF_NotFound => Cow::from("E002: No memory mapped file has been opened yet!"),
            Self::Uninitialized => Cow::from("Memory Mapped File was not yet initialized"),
            Self::MaxReaders => Cow::from("The maximum amount of readers is already registered"),
            Self::MaxTriesReached => Cow::from("The maximum amount of tries was reached spinning"),
            Self::GeneralFailure => Cow::from("No idea what the hell happened here..."),
            Self::OS_Err(c) => Cow::from(format!("E{c:02}: Generic OS Error")),
        };

        write!(
            f,
            "{text}: {}",
            self.source().map(|e| Cow::from(e.to_string())).unwrap_or(Cow::from("occurred in this crate."))
        )
    }
}

/// Thin wrapper type for [`Result`]s we produced.
pub type MMFResult<T> = Result<T, Error>;
