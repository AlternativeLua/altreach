use anyhow::Result;

pub fn compress(bytes: &[u8]) -> Result<Vec<u8>> {
    Ok(lz4_flex::compress_prepend_size(bytes))
}
