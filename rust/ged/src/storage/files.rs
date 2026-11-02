//! Streaming a reader to a file with a hard size limit, without ever leaving
//! a partial file behind under its final name.

use std::fmt;
use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use super::content_type::SNIFF_BYTES;

#[derive(Debug)]
pub enum StorageError {
    /// More bytes than the limit allowed; nothing was kept.
    TooLarge {
        limit: u64,
    },
    /// A key or id that is not one this store generates (path traversal etc.).
    InvalidKey,
    Io(io::Error),
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { limit } => write!(f, "file is too large (maximum {limit} bytes)"),
            Self::InvalidKey => f.write_str("invalid storage key"),
            Self::Io(e) => write!(f, "storage i/o error: {e}"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<io::Error> for StorageError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// What was written: size, SHA-256 (hex) and the first bytes (enough for
/// content-type detection).
pub struct Written {
    pub byte_size: u64,
    pub checksum: String,
    pub head: Vec<u8>,
}

/// Streams `reader` into `dest`. The bytes land in `dest` + `.part` and are
/// renamed on success; on any failure (read error, over the limit) the partial
/// file is removed. A body of exactly `limit` bytes is accepted.
pub async fn copy_limited<R: AsyncRead + Unpin>(
    reader: R,
    dest: &Path,
    limit: u64,
) -> Result<Written, StorageError> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = std::path::PathBuf::from(part);

    match write_part(reader, &part, limit).await {
        Ok(written) => match tokio::fs::rename(&part, dest).await {
            Ok(()) => Ok(written),
            Err(e) => {
                let _ = tokio::fs::remove_file(&part).await;
                Err(e.into())
            }
        },
        Err(e) => {
            let _ = tokio::fs::remove_file(&part).await;
            Err(e)
        }
    }
}

async fn write_part<R: AsyncRead + Unpin>(
    mut reader: R,
    part: &Path,
    limit: u64,
) -> Result<Written, StorageError> {
    let mut file = tokio::fs::File::create(part).await?;
    let mut hasher = Sha256::new();
    let mut head = Vec::with_capacity(SNIFF_BYTES.min(8192));
    let mut total: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > limit {
            return Err(StorageError::TooLarge { limit });
        }
        let chunk = &buf[..n];
        if head.len() < SNIFF_BYTES {
            let take = (SNIFF_BYTES - head.len()).min(n);
            head.extend_from_slice(&chunk[..take]);
        }
        hasher.update(chunk);
        file.write_all(chunk).await?;
    }
    file.flush().await?;
    file.sync_all().await?;
    let digest = hasher.finalize();
    Ok(Written {
        byte_size: total,
        checksum: digest.iter().map(|b| format!("{b:02x}")).collect(),
        head,
    })
}
