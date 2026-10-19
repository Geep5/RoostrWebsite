use std::fmt;

/// Why an engine call failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    /// The browser engine could not be fetched, verified or instantiated.
    Load(String),
    /// The engine speaks another ABI than this binding.
    AbiMismatch(u32),
    /// The encoded JSON request exceeds [`crate::REQUEST_LIMIT`].
    RequestTooLarge(usize),
    /// The blob exceeds [`crate::BLOB_LIMIT`] (or the engine refused its reservation).
    BlobTooLarge(usize),
    /// The engine rejected the request: invalid JSON, unknown method or a
    /// domain rule (`{error}` envelope). The message is the engine's, verbatim.
    Domain(String),
    /// The engine trapped, refused a reservation or answered with malformed
    /// bytes. It was reset with its persistent state, and the generation bumped.
    Runtime(String),
    /// The payload could not be serialised.
    Encode(String),
    /// A well-formed result did not have the shape the typed call expects.
    Decode(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Load(message) => write!(f, "cannot load the shared engine: {message}"),
            EngineError::AbiMismatch(version) => {
                write!(
                    f,
                    "the shared engine speaks ABI {version}, this binding {}",
                    crate::ABI_VERSION
                )
            }
            EngineError::RequestTooLarge(length) => {
                write!(f, "engine request of {length} bytes exceeds 16 MiB")
            }
            EngineError::BlobTooLarge(length) => {
                write!(f, "engine blob of {length} bytes exceeds 32 MiB")
            }
            EngineError::Domain(message) => f.write_str(message),
            EngineError::Runtime(message) => write!(f, "engine fault: {message}"),
            EngineError::Encode(message) => write!(f, "cannot encode engine request: {message}"),
            EngineError::Decode(message) => write!(f, "unexpected engine result: {message}"),
        }
    }
}

impl std::error::Error for EngineError {}
