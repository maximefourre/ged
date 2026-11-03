//! Temporary on-disk staging for async uploads: the HTTP request writes the raw
//! bytes here, the background job reads and deletes them
//! (`<root>/<upload_request_id>`, like Rails' `StagingStore`).

use std::path::{Path, PathBuf};

use tokio::io::AsyncRead;
use uuid::Uuid;

use super::files::{StorageError, copy_limited};

#[derive(Debug, Clone)]
pub struct StagingStore {
    root: PathBuf,
}

impl StagingStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The staging file of an upload request. The id is a `Uuid`, so the name
    /// is always 36 hex-and-dash characters: no separators, no traversal.
    pub fn path(&self, id: Uuid) -> PathBuf {
        self.root.join(id.hyphenated().to_string())
    }

    /// Writes the raw upload; at most `limit` bytes. On any failure nothing
    /// remains in the staging directory. Returns the byte size.
    pub async fn write<R: AsyncRead + Unpin>(
        &self,
        id: Uuid,
        reader: R,
        limit: u64,
    ) -> Result<u64, StorageError> {
        Ok(copy_limited(reader, &self.path(id), limit).await?.byte_size)
    }

    pub async fn open(&self, id: Uuid) -> Result<tokio::fs::File, StorageError> {
        Ok(tokio::fs::File::open(self.path(id)).await?)
    }

    /// `FileUtils.rm_f`: already gone is fine.
    pub async fn delete(&self, id: Uuid) -> Result<(), StorageError> {
        match tokio::fs::remove_file(self.path(id)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Parses a directory entry name: `<uuid>` (a staged upload) or
    /// `<uuid>.part` (a write cut short). Anything else is not ours.
    pub fn parse_name(name: &str) -> Option<(Uuid, bool)> {
        let (stem, part) = match name.strip_suffix(".part") {
            Some(stem) => (stem, true),
            None => (name, false),
        };
        // `Uuid::parse_str` also accepts simple/braced forms; we only write hyphenated.
        let id = Uuid::parse_str(stem).ok()?;
        (id.hyphenated().to_string() == stem).then_some((id, part))
    }

    pub async fn exists(&self, id: Uuid) -> bool {
        tokio::fs::try_exists(self.path(id)).await.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn staging() -> (tempfile::TempDir, StagingStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = StagingStore::new(dir.path().join("staging"));
        (dir, store)
    }

    fn entries(store: &StagingStore) -> Vec<String> {
        match std::fs::read_dir(store.root()) {
            Ok(rd) => rd
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    #[tokio::test]
    async fn write_open_delete_cycle() {
        let (_dir, store) = staging();
        let id = Uuid::new_v4();
        assert_eq!(store.write(id, &b"raw bytes"[..], 100).await.unwrap(), 9);
        assert!(store.exists(id).await);
        assert_eq!(entries(&store), vec![id.to_string()]);
        let mut body = String::new();
        store
            .open(id)
            .await
            .unwrap()
            .read_to_string(&mut body)
            .await
            .unwrap();
        assert_eq!(body, "raw bytes");
        store.delete(id).await.unwrap();
        assert!(!store.exists(id).await);
        store.delete(id).await.unwrap();
    }

    /// Review Focus 2 (storage half): an oversized upload leaves no staging file.
    #[tokio::test]
    async fn an_oversized_upload_leaves_no_staging_file() {
        let (_dir, store) = staging();
        let id = Uuid::new_v4();
        let err = store.write(id, &[1u8; 101][..], 100).await.unwrap_err();
        assert!(matches!(err, StorageError::TooLarge { limit: 100 }));
        assert!(entries(&store).is_empty(), "{:?}", entries(&store));
        assert!(!store.exists(id).await);
        // the limit itself is fine
        assert_eq!(store.write(id, &[1u8; 100][..], 100).await.unwrap(), 100);
    }

    #[tokio::test]
    async fn an_upload_cut_mid_body_leaves_no_staging_file() {
        let (_dir, store) = staging();
        let id = Uuid::new_v4();
        let (mut tx, rx) = tokio::io::duplex(64);
        let writer = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            tx.write_all(&[7u8; 40]).await.unwrap();
            // dropped without finishing is a clean EOF; abort with an error instead
        });
        // a reader that fails after some bytes
        struct Cut<R>(R, bool);
        impl<R: AsyncRead + Unpin> AsyncRead for Cut<R> {
            fn poll_read(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
                buf: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                if self.1 {
                    return std::task::Poll::Ready(Err(std::io::Error::other("cut")));
                }
                let r = std::pin::Pin::new(&mut self.0).poll_read(cx, buf);
                if let std::task::Poll::Ready(Ok(())) = &r
                    && buf.filled().is_empty()
                {
                    self.1 = true;
                    return std::task::Poll::Ready(Err(std::io::Error::other("cut")));
                }
                r
            }
        }
        writer.await.unwrap();
        let err = store.write(id, Cut(rx, false), 1000).await.unwrap_err();
        assert!(matches!(err, StorageError::Io(_)));
        assert!(entries(&store).is_empty(), "{:?}", entries(&store));
    }

    #[test]
    fn parse_name_recognises_staging_and_part_files_only() {
        let id = Uuid::new_v4();
        assert_eq!(StagingStore::parse_name(&id.to_string()), Some((id, false)));
        assert_eq!(
            StagingStore::parse_name(&format!("{id}.part")),
            Some((id, true))
        );
        assert_eq!(StagingStore::parse_name("notes.txt"), None);
        assert_eq!(StagingStore::parse_name(&id.simple().to_string()), None);
        assert_eq!(StagingStore::parse_name(&format!("{id}.tmp")), None);
    }

    #[test]
    fn ids_cannot_escape_the_root() {
        let (_dir, store) = staging();
        let path = store.path(Uuid::new_v4());
        assert_eq!(path.parent().unwrap(), store.root());
        assert_eq!(path.file_name().unwrap().len(), 36);
    }
}
