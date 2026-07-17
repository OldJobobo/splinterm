//! Versioned, transport-independent messages exchanged over the local socket.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use splinterm_core::Dojo;

pub const PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub version: u16,
    pub message: T,
}

impl<T> Envelope<T> {
    #[must_use]
    pub const fn new(message: T) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            message,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    Ping,
    ListDojos,
    CreateDojo { name: String, cwd: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Pong,
    Dojos { dojos: Vec<Dojo> },
    DojoCreated { dojo: Dojo },
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_json_is_explicit_and_versioned() {
        let json = serde_json::to_string(&Envelope::new(Request::Ping)).unwrap();

        assert_eq!(json, r#"{"version":1,"message":{"type":"ping"}}"#);
    }
}
