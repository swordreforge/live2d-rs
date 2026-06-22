use crate::error::{MocError, MocResult};

/// Index into an [`ObjectRegistry`].
pub type ObjIndex = u32;

/// A format-level blob stored in the MOC object-reference table.
///
/// The registry is populated as the byte stream is traversed.  When the
/// stream emits an `ObjectRef` (tag 33), the schema layer looks up the
/// corresponding index in this registry to get the already-deserialized value.
#[derive(Debug, Clone)]
pub enum Blob {
    /// `null` value  (tag 0).
    Null,
    /// `ObjectRef(n)`  (tag 33) — resolved to the index it pointed at.
    /// This variant exists so that the registry can store *unresolved*
    /// references for schema types that need deferred resolution.
    UnresolvedRef(ObjIndex),
    /// `String`  (tag 1).
    String(Box<str>),
    /// `Int32Array`  (tags 16 / 25).
    I32Array(Box<[i32]>),
    /// `Uint32Array` — used for indices internally.
    U32Array(Box<[u32]>),
    /// `Float32Array`  (tag 27).
    F32Array(Box<[f32]>),
    /// `Array<T>`  (tag 15) — stores *indices* into this registry for each element.
    ObjArray(Box<[ObjIndex]>),
    /// Opaque domain-level object — raw bytes that a [`MocSchema`](crate::MocSchema)
    /// implementation will interpret.
    Opaque(Box<[u8]>),
}

/// The global object-reference table.
///
/// As the byte stream is traversed, every object (both format-level and
/// domain-level) gets pushed here with an auto-incrementing index.
/// Subsequent `ObjectRef(33)` entries resolve to earlier indices.
#[derive(Debug, Default)]
pub struct Registry {
    entries: Vec<Blob>,
}

impl Registry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Number of entries in the registry.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Push a new blob, returning its index.
    pub fn push(&mut self, blob: Blob) -> ObjIndex {
        let idx = self.entries.len() as ObjIndex;
        self.entries.push(blob);
        idx
    }

    /// Resolve an index to a blob reference.
    pub fn get(&self, index: ObjIndex) -> MocResult<&Blob> {
        self.entries.get(index as usize).ok_or(MocError::InvalidObjectRef {
            index,
            registry_len: self.entries.len(),
        })
    }

    /// Pop the last entry (for schema types that need to replace an opaque
    /// blob with a typed one).
    pub fn pop(&mut self) -> Option<Blob> {
        self.entries.pop()
    }

    /// Replace the blob at `index` with a new one (used by schema types to
    /// upgrade an `Opaque` blob to a typed representation).
    pub fn replace(&mut self, index: ObjIndex, blob: Blob) -> MocResult<()> {
        let registry_len = self.entries.len();
        let slot = self.entries.get_mut(index as usize).ok_or(MocError::InvalidObjectRef {
            index,
            registry_len,
        })?;
        *slot = blob;
        Ok(())
    }
}

// ── Convenience accessors ──────────────────────────────────────────

impl Registry {
    /// Get a blob as an f32 array, if that's what it is.
    pub fn get_f32_array(&self, index: ObjIndex) -> MocResult<&[f32]> {
        match self.get(index)? {
            Blob::F32Array(a) => Ok(a),
            other => Err(MocError::InvalidLayout {
                context: "Registry::get_f32_array",
                detail: format!("expected F32Array, got {other:?}"),
            }),
        }
    }

    /// Get a blob as an i32 array.
    pub fn get_i32_array(&self, index: ObjIndex) -> MocResult<&[i32]> {
        match self.get(index)? {
            Blob::I32Array(a) => Ok(a),
            other => Err(MocError::InvalidLayout {
                context: "Registry::get_i32_array",
                detail: format!("expected I32Array, got {other:?}"),
            }),
        }
    }

    /// Get a blob as an object array (returns the element indices).
    pub fn get_obj_array(&self, index: ObjIndex) -> MocResult<&[ObjIndex]> {
        match self.get(index)? {
            Blob::ObjArray(a) => Ok(a),
            other => Err(MocError::InvalidLayout {
                context: "Registry::get_obj_array",
                detail: format!("expected ObjArray, got {other:?}"),
            }),
        }
    }

    /// Get a blob as a string.
    pub fn get_string(&self, index: ObjIndex) -> MocResult<&str> {
        match self.get(index)? {
            Blob::String(s) => Ok(s.as_ref()),
            other => Err(MocError::InvalidLayout {
                context: "Registry::get_string",
                detail: format!("expected String, got {other:?}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_and_get() {
        let mut reg = Registry::new();
        let idx = reg.push(Blob::String("hello".into()));
        assert_eq!(reg.get_string(idx).unwrap(), "hello");
    }

    #[test]
    fn test_invalid_ref() {
        let reg = Registry::new();
        let err = reg.get(0).unwrap_err();
        assert!(
            matches!(&err, MocError::InvalidObjectRef { index: 0, .. }),
            "expected InvalidObjectRef, got {err:?}"
        );
    }

    #[test]
    fn test_replace() {
        let mut reg = Registry::new();
        let idx = reg.push(Blob::Null);
        reg.replace(idx, Blob::String("replaced".into())).unwrap();
        assert_eq!(reg.get_string(idx).unwrap(), "replaced");
    }
}
