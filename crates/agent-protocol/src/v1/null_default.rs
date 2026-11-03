//! Deserialize helper matching Go's handling of JSON `null`.
//!
//! `encoding/json` unmarshals `null` into a non-pointer field by leaving the
//! zero value in place (no error). Serde instead rejects `null` for `String`,
//! integers, floats and structs, so every such field opts into this helper.
//! `Option<T>` fields need no helper because `null` already maps to `None`.

use serde::Deserialize;
use serde::Deserializer;

/// Deserialize `T`, mapping JSON `null` to `T::default()`.
pub(crate) fn deserialize<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}
