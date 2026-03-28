use anyhow::Result;

pub fn compress(bytes: &[u8]) -> Result<Vec<u8>> {
    let encoded = zstd::encode_all(bytes, 0)?;

    Ok(encoded)
}