//! Source-independent byte fingerprints; checkpoints belong only in the index.
use anyhow::{Context, Result, ensure};
use std::{
    fs::{File, Metadata},
    io::{Read, Result as IoResult},
    path::Path,
};

pub struct HashingReader<R> {
    inner: R,
    hasher: blake3::Hasher,
}

impl<R> HashingReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: blake3::Hasher::new(),
        }
    }

    pub fn fingerprint(&self) -> String {
        format!("blake3:{}", self.hasher.finalize().to_hex())
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> IoResult<usize> {
        let count = self.inner.read(buffer)?;
        self.hasher.update(&buffer[..count]);
        Ok(count)
    }
}

pub fn ensure_unchanged(path: &Path, before: &Metadata) -> Result<()> {
    let after = std::fs::metadata(path).context("check source after reading")?;
    ensure!(
        before.len() == after.len() && before.modified()? == after.modified()?,
        "source changed during read; retry refresh"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            before.dev() == after.dev()
                && before.ino() == after.ino()
                && before.ctime() == after.ctime()
                && before.ctime_nsec() == after.ctime_nsec(),
            "source replaced or changed during read; retry refresh"
        );
    }
    Ok(())
}

pub fn hash_file(path: &Path) -> Result<String> {
    let file = File::open(path).context("open source for checksum")?;
    let before = file.metadata().context("inspect source for checksum")?;
    let mut reader = HashingReader::new(file);
    let mut buffer = vec![0u8; 1024 * 1024];
    while reader
        .read(&mut buffer)
        .context("read source for checksum")?
        != 0
    {}
    ensure_unchanged(path, &before)?;
    Ok(reader.fingerprint())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_matches_exact_bytes_and_rejects_a_changed_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.jsonl");
        let bytes = b"synthetic\r\nbytes\n";
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(
            hash_file(&path).unwrap(),
            format!("blake3:{}", blake3::hash(bytes).to_hex())
        );
        let before = std::fs::metadata(&path).unwrap();
        std::fs::write(&path, b"changed").unwrap();
        assert!(ensure_unchanged(&path, &before).is_err());
    }
}
