//! The disk store for document files (replaces Active Storage's Disk service).
//! Layout: `<root>/<k0k1>/<k2k3>/<key>`, like Active Storage. A key is always
//! generated here (32 lowercase hex characters) and is validated again on
//! every access, so a key coming from anywhere else can never address a path
//! outside the root.

use std::path::{Path, PathBuf};

use tokio::io::AsyncRead;

use super::files::{StorageError, Written, copy_limited};

#[derive(Debug, Clone)]
pub struct Storage {
    root: PathBuf,
}

/// A stored file.
pub struct Stored {
    pub key: String,
    pub written: Written,
}

impl Storage {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where a key lives; `InvalidKey` for anything this store did not generate.
    pub fn path_for(&self, key: &str) -> Result<PathBuf, StorageError> {
        let valid = key.len() == 32 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !valid {
            return Err(StorageError::InvalidKey);
        }
        Ok(self.root.join(&key[0..2]).join(&key[2..4]).join(key))
    }

    /// Stores the stream under a fresh key; at most `limit` bytes.
    pub async fn put<R: AsyncRead + Unpin>(
        &self,
        reader: R,
        limit: u64,
    ) -> Result<Stored, StorageError> {
        let key = uuid::Uuid::new_v4().simple().to_string();
        let path = self.path_for(&key)?;
        let written = copy_limited(reader, &path, limit).await?;
        Ok(Stored { key, written })
    }

    /// Opens a stored file for streaming (`NotFound` surfaces as `Io`).
    pub async fn open(&self, key: &str) -> Result<tokio::fs::File, StorageError> {
        Ok(tokio::fs::File::open(self.path_for(key)?).await?)
    }

    /// Removes the file; already gone is fine (`FileUtils.rm_f`).
    pub async fn delete(&self, key: &str) -> Result<(), StorageError> {
        match tokio::fs::remove_file(self.path_for(key)?).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn exists(&self, key: &str) -> bool {
        match self.path_for(key) {
            Ok(path) => tokio::fs::try_exists(path).await.unwrap_or(false),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn store() -> (tempfile::TempDir, Storage) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().join("files"));
        (dir, storage)
    }

    fn files_under(root: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() { stack.push(path) } else { out.push(path) }
            }
        }
        out
    }

    #[tokio::test]
    async fn put_then_open_round_trips_with_size_checksum_and_head() {
        let (_dir, storage) = store();
        let stored = storage.put(&b"%PDF-1.4 hello"[..], 1024).await.unwrap();
        assert_eq!(stored.written.byte_size, 14);
        assert_eq!(stored.written.checksum, sha256_hex(b"%PDF-1.4 hello"));
        assert_eq!(stored.written.head, b"%PDF-1.4 hello");
        let mut body = Vec::new();
        storage.open(&stored.key).await.unwrap().read_to_end(&mut body).await.unwrap();
        assert_eq!(body, b"%PDF-1.4 hello");
        let path = storage.path_for(&stored.key).unwrap();
        assert!(path.starts_with(storage.root()));
        assert_eq!(path.parent().unwrap().parent().unwrap().parent().unwrap(), storage.root());
    }

    fn sha256_hex(data: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[tokio::test]
    async fn exactly_the_limit_is_accepted_one_more_byte_is_not() {
        let (_dir, storage) = store();
        assert!(storage.put(&[0u8; 10][..], 10).await.is_ok());
        let err = storage.put(&[0u8; 11][..], 10).await.err().unwrap();
        assert!(matches!(err, StorageError::TooLarge { limit: 10 }), "{err}");
        // only the first file remains: the rejected one left nothing behind
        assert_eq!(files_under(storage.root()).len(), 1);
    }

    #[tokio::test]
    async fn a_failing_reader_leaves_no_file_behind() {
        struct Breaks(u8);
        impl AsyncRead for Breaks {
            fn poll_read(
                mut self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                if self.0 == 0 {
                    return std::task::Poll::Ready(Err(std::io::Error::other("connection reset")));
                }
                self.0 -= 1;
                buf.put_slice(b"some bytes");
                std::task::Poll::Ready(Ok(()))
            }
        }
        let (_dir, storage) = store();
        let err = storage.put(Breaks(3), 1 << 20).await.err().unwrap();
        assert!(matches!(err, StorageError::Io(_)));
        assert!(files_under(storage.root()).is_empty(), "{:?}", files_under(storage.root()));
    }

    #[tokio::test]
    async fn delete_removes_the_file_and_is_idempotent() {
        let (_dir, storage) = store();
        let stored = storage.put(&b"x"[..], 10).await.unwrap();
        assert!(storage.exists(&stored.key).await);
        storage.delete(&stored.key).await.unwrap();
        assert!(!storage.exists(&stored.key).await);
        assert!(files_under(storage.root()).is_empty());
        storage.delete(&stored.key).await.unwrap();
    }

    #[tokio::test]
    async fn opening_a_missing_key_is_not_found() {
        let (_dir, storage) = store();
        let err = storage.open(&"a".repeat(32)).await.err().unwrap();
        assert!(matches!(&err, StorageError::Io(e) if e.kind() == std::io::ErrorKind::NotFound));
    }

    #[tokio::test]
    async fn hostile_keys_never_address_a_path() {
        let (dir, storage) = store();
        let secret = dir.path().join("secret");
        std::fs::write(&secret, "s").unwrap();
        for key in [
            "../secret",
            "../../secret",
            "/etc/passwd",
            "..%2f..%2fsecret",
            "",
            "a",
            &"A".repeat(32),
            &format!("{}/x", "a".repeat(30)),
            &format!("{}\0", "a".repeat(31)),
            &"a".repeat(33),
        ] {
            assert!(matches!(storage.path_for(key), Err(StorageError::InvalidKey)), "{key:?}");
            assert!(storage.open(key).await.is_err(), "{key:?}");
            assert!(storage.delete(key).await.is_err(), "{key:?}");
        }
        assert!(secret.exists());
    }
}
