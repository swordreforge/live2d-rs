#![deny(unused_must_use)]
#![deny(trivial_numeric_casts)]
#![deny(non_ascii_idents)]

mod error;
mod reader;
mod registry;
mod schema;

pub use error::{MocError, MocResult};
pub use reader::BinaryReader;
pub use registry::{Blob, ObjIndex, Registry};
pub use schema::{parse_moc, read_typed_object, ReadKnownTypeFn};
