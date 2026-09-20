//! Copy one file with hashing and optional verification.

use crate::config::Verify;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct CopyOutcome {
    pub bytes: u64,
    pub blake3: String,
    pub verified: bool,
}

const BUF: usize = 1 << 20;

/// Copy `src` to `dst` (creating parents), hashing the stream. Writes to a `.part` file and
/// renames on success. `progress` receives bytes written so far.
pub fn copy_file(src: &Path, dst: &Path, verify: Verify, mut progress: impl FnMut(u64)) -> anyhow::Result<CopyOutcome> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let part = dst.with_extension(match dst.extension().and_then(|e| e.to_str()) {
        Some(e) => format!("{e}.part"),
        None => "part".to_string(),
    });

    let mut input = File::open(src)?;
    let mut output = File::create(&part)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; BUF];
    let mut total: u64 = 0;
    loop {
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        output.write_all(&buf[..n])?;
        total += n as u64;
        progress(total);
    }
    output.flush()?;
    drop(output);
    let hash = hasher.finalize().to_hex().to_string();

    let verified = match verify {
        Verify::Hash => {
            let back = hash_file(&part)?;
            if back != hash {
                let _ = fs::remove_file(&part);
                anyhow::bail!("verification failed for {}: hash mismatch after write", dst.display());
            }
            true
        }
        Verify::SizeOnly => {
            let len = fs::metadata(&part)?.len();
            if len != total {
                let _ = fs::remove_file(&part);
                anyhow::bail!("verification failed for {}: size mismatch", dst.display());
            }
            false
        }
    };

    if let Ok(md) = fs::metadata(src)
        && let Ok(mtime) = md.modified()
    {
        let _ = filetime::set_file_mtime(&part, filetime::FileTime::from_system_time(mtime));
    }
    fs::rename(&part, dst)?;
    Ok(CopyOutcome { bytes: total, blake3: hash, verified })
}

pub fn hash_file(path: &Path) -> anyhow::Result<String> {
    let mut f = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; BUF];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_and_hashes() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("a.bin");
        let dst = tmp.path().join("out/sub/a.bin");
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        fs::write(&src, &data).unwrap();
        let mut last = 0;
        let out = copy_file(&src, &dst, Verify::Hash, |b| last = b).unwrap();
        assert_eq!(out.bytes, data.len() as u64);
        assert_eq!(last, data.len() as u64);
        assert!(out.verified);
        assert_eq!(out.blake3, blake3::hash(&data).to_hex().to_string());
        assert_eq!(fs::read(&dst).unwrap(), data);
        assert!(!tmp.path().join("out/sub/a.bin.part").exists());
    }
}
