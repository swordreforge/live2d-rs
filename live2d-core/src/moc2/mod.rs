//! Cubism 2.1 MOC2 format support — pure Rust, no Cubism Core dependency.

mod reader;
mod resolve;
mod resolve_util;
mod types;

// Phase 3–5 stubs
pub(crate) mod pivot;
pub(crate) mod deformer;
mod runtime;
pub use runtime::Moc2Model;

pub use types::*;

use moc_parser::{BinaryReader, MocResult, ObjIndex, ReadKnownTypeFn, Registry};

/// Recursive object reader that handles both format-level tags (0–47)
/// via `moc_parser` and domain-level tags (48+) via `read_known_type`.
pub(crate) fn read_moc_object(
    reader: &mut BinaryReader,
    registry: &mut Registry,
    known_type_fn: ReadKnownTypeFn,
) -> MocResult<ObjIndex> {
    moc_parser::read_typed_object(reader, registry, known_type_fn)
}

/// Parse MOC binary data into a fully-resolved [`Moc2Data`] model.
///
/// This is the main entry point for loading Cubism 2.1 models.
pub fn parse_moc2(buf: &[u8]) -> MocResult<Moc2Data> {
    let registry = moc_parser::parse_moc(buf, reader::read_known_type)?;
    resolve::resolve_registry(&registry)
}


