use std::{error::Error as stderr, fmt};
use windows::core::{Error as WErr, HRESULT};

#[allow(non_camel_case_types)]
#[derive(Debug)]
#[repr(u8)]
pub enum Error {
    ReadLocked = 0,
    WriteLocked = 1,
    Uninitialized = 2,
    MaxReaders = 3,
    #[allow(dead_code)]
    GeneralFailure = 4,
    MMF_NotFound = 5,
    OS_Err(WErr) = 254,
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
            HRESULT(9) => Self::Uninitialized,
            HRESULT(19) => Self::WriteLocked,
            HRESULT(2) => Self::MMF_NotFound,
            HRESULT(0) => Self::OS_OK(value),
            _ => Self::OS_Err(value),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = String::from(match self {
            Self::ReadLocked => "Memory Mapped File was locked for reading".to_owned(),
            Self::WriteLocked => "Memory Mapped File was locked for writing".to_owned(),
            Self::MMF_NotFound => "E002: No memory mapped file has been opened yet!".to_owned(),
            Self::OS_Err(c) => format!("E{c:02}: Generic OS Error"),
            Self::Uninitialized => "Memory Mapped File was not yet initialized".to_owned(),
            Self::MaxReaders => "The maximum amount of readers is already registered".to_owned(),
            Self::GeneralFailure => "No idea what the hell happened here...".to_owned(),
            Self::OS_OK(_) => "Task failed successfully!".to_owned(),
        });
        write!(f, "{text}: {}", self.source().map(|e| e.to_string()).unwrap_or("occurred in this crate.".to_owned()))
    }
}

pub type MMFResult<T> = Result<T, Error>;
