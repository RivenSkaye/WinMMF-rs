use std::{error::Error as stderr, fmt};

#[derive(Debug)]
pub enum Error {
    ReadLocked,
    WriteLocked,
    Uninitialized,
    MaxReaders,
    #[allow(dead_code)]
    GeneralFailure,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::ReadLocked => "Memory Mapped File was locked for reading",
            Self::WriteLocked => "Memory Mapped File was locked for writing",
            Self::Uninitialized => "Memory Mapped File was not yet initialized",
            Self::MaxReaders => "The maximum amount of readers is already registered",
            Self::GeneralFailure => "No idea what the hell happened here...",
        };
        write!(f, "{text}: {}", self.source().map(|e| e.to_string()).unwrap_or("occurred in this crate.".to_owned()))
    }
}

impl stderr for Error {}

pub type MMFResult<T> = Result<T, Error>;
