use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};

pub(crate) fn encode_record<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut encoded = Vec::new();
    ciborium::into_writer(value, &mut encoded)
        .context("failed to encode compact storage record")?;
    Ok(encoded)
}

pub(crate) fn decode_record<T: DeserializeOwned>(value: &[u8]) -> Result<T> {
    ciborium::from_reader(value).context("failed to decode compact storage record")
}
