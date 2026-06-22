use crate::error::MocResult;
use crate::reader::BinaryReader;
use crate::registry::ObjIndex;
use crate::Registry;

/// Function signature for domain-level type dispatch (tags >= 48).
pub type ReadKnownTypeFn = fn(&mut BinaryReader, &mut Registry, u8) -> MocResult<ObjIndex>;

/// Read a tagged object from the MOC stream.
///
/// Tags 0–47 are format-level primitives handled directly.
/// Tags 48+ are dispatched via `known_type_fn`.
pub fn read_typed_object(
    reader: &mut BinaryReader,
    registry: &mut Registry,
    known_type_fn: ReadKnownTypeFn,
) -> MocResult<ObjIndex> {
    let tag = reader.read_vlq()? as u8;

    match tag {
        0 => Ok(registry.push(crate::Blob::Null)),
        1 => {
            let s = reader.read_string()?;
            Ok(registry.push(crate::Blob::String(s.into())))
        }
        15 => {
            let count = reader.read_vlq()?;
            let mut indices = Vec::with_capacity(count as usize);
            for _ in 0..count {
                indices.push(read_typed_object(reader, registry, known_type_fn)?);
            }
            Ok(registry.push(crate::Blob::ObjArray(indices.into())))
        }
        16 | 25 => {
            let count = reader.read_vlq()? as usize;
            let mut vals = Vec::with_capacity(count);
            for _ in 0..count {
                vals.push(reader.read_i32()?);
            }
            Ok(registry.push(crate::Blob::I32Array(vals.into())))
        }
        26 => {
            let count = reader.read_vlq()? as usize;
            let mut vals = Vec::with_capacity(count);
            for _ in 0..count {
                let low = reader.read_u32()?;
                let high = reader.read_u32()?;
                vals.push(f64::from_bits((high as u64) << 32 | low as u64) as f32);
            }
            Ok(registry.push(crate::Blob::F32Array(vals.into())))
        }
        27 => {
            let count = reader.read_vlq()? as usize;
            let mut vals = Vec::with_capacity(count);
            for _ in 0..count {
                vals.push(reader.read_f32()?);
            }
            Ok(registry.push(crate::Blob::F32Array(vals.into())))
        }
        33 => {
            // ObjectRef: 4-byte big-endian signed int32 index into the
            // flat object list.  Python DOES NOT push/appende for ObjectRef
            // — it returns the existing object directly.  To keep our
            // Registry indices aligned we do the same: return the target
            // index without pushing anything.
            let idx = reader.read_i32()? as u32;
            Ok(idx)
        }
        // Everything else is a domain-level type — dispatch to
        // known_type_fn.  The Python reference does the same: it
        // handles only tags 0,1,15,16,25,26,27,33 at format level
        // and sends every other tag to Live2DObjectFactory.
        _ => known_type_fn(reader, registry, tag),
    }
}

/// Parse a complete MOC stream into a [`Registry`].
///
/// `known_type_fn` handles domain-level type tags (>=48).
pub fn parse_moc(
    buf: &[u8],
    known_type_fn: ReadKnownTypeFn,
) -> MocResult<Registry> {
    let mut reader = BinaryReader::new(buf)?;
    let mut registry = Registry::new();

    let _root_idx = read_typed_object(&mut reader, &mut registry, known_type_fn)?;

    if reader.version() >= 8 {
        if reader.remaining() >= 4 {
            let _check1 = reader.read_u16()?;
            let _check2 = reader.read_u16()?;
        }
    }

    Ok(registry)
}
