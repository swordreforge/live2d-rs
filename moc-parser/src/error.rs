use std::fmt;

/// Errors that can occur during MOC binary parsing.
#[derive(Debug)]
pub enum MocError {
    /// The input buffer is too short to read the expected data.
    UnexpectedEof {
        offset: usize,
        expected: usize,
        available: usize,
    },
    /// Invalid magic number — not a valid MOC file.
    InvalidMagic {
        actual: [u8; 4],
    },
    /// Unsupported format version.
    UnsupportedVersion {
        version: u8,
    },
    /// An unknown type tag was encountered.
    UnknownTag {
        offset: usize,
        tag: u8,
    },
    /// An object reference index is out of bounds.
    InvalidObjectRef {
        index: u32,
        registry_len: usize,
    },
    /// An assertion about the data layout failed (e.g. odd row*col for warp).
    InvalidLayout {
        context: &'static str,
        detail: String,
    },
}

impl std::error::Error for MocError {}

impl fmt::Display for MocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof { offset, expected, available } => {
                write!(
                    f,
                    "unexpected EOF at offset {offset}: needed {expected} bytes, have {available}"
                )
            }
            Self::InvalidMagic { actual } => {
                write!(f, "invalid MOC magic: {actual:02x?}")
            }
            Self::UnsupportedVersion { version } => {
                write!(f, "unsupported MOC format version: {version} (supported: 8-11)")
            }
            Self::UnknownTag { offset, tag } => {
                write!(f, "unknown type tag {tag} at offset {offset}")
            }
            Self::InvalidObjectRef { index, registry_len } => {
                write!(f, "invalid object reference #{index} (registry has {registry_len} entries)")
            }
            Self::InvalidLayout { context, detail } => {
                write!(f, "invalid layout in {context}: {detail}")
            }
        }
    }
}

/// Short-hand for `Result<T, MocError>`.
pub type MocResult<T> = Result<T, MocError>;
