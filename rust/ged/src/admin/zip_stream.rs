//! Admin bulk download: a stored ZIP written entry by entry.
//!
//! File bytes are copied in fixed chunks straight into the archive. The
//! central directory (names and sizes only) is the only thing held until the
//! end, so a large document does not sit in memory beside its ZIP entry.
//! Entry names are the caller's already-sanitised strings; this writer never
//! interprets them as paths.

use std::io;
use std::path::PathBuf;

use axum::body::Body;
use chrono::{Datelike, Timelike};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::io::ReaderStream;

/// One file to archive. `name` is the entry name, not a path on disk.
pub struct ZipFile {
    pub name: String,
    pub path: PathBuf,
}

/// How much of a file is in flight at once: the read buffer and the duplex
/// that feeds the response.
const CHUNK: usize = 64 * 1024;

/// Bit 3: sizes follow the data. Bit 11: the name is UTF-8.
const FLAG: u16 = 0x0808;

/// A response body that streams `files` as one ZIP. A missing file is skipped
/// and logged, as `DocumentZipper` does, instead of failing the archive.
pub fn stream(files: Vec<ZipFile>) -> Body {
    let (writer, reader) = tokio::io::duplex(CHUNK);
    tokio::spawn(async move {
        if let Err(e) = write_paths(writer, files).await {
            eprintln!("ged: admin: zip stream: {e}");
        }
    });
    Body::from_stream(ReaderStream::new(reader))
}

pub async fn write_paths<W: AsyncWrite + Unpin>(writer: W, files: Vec<ZipFile>) -> io::Result<()> {
    let mut archive = Archive::new(writer);
    for file in files {
        let mut reader = match tokio::fs::File::open(&file.path).await {
            Ok(file) => file,
            Err(e) => {
                eprintln!("ged: admin: zip skip {}: {e}", file.path.display());
                continue;
            }
        };
        archive
            .write_entry(file.name.as_bytes(), &mut reader)
            .await?;
    }
    archive.finish().await
}

struct Archive<W> {
    writer: W,
    central: Vec<u8>,
    offset: u64,
    count: u16,
    time: u16,
    date: u16,
}

impl<W: AsyncWrite + Unpin> Archive<W> {
    fn new(writer: W) -> Self {
        let (time, date) = dos_now();
        Self {
            writer,
            central: Vec::new(),
            offset: 0,
            count: 0,
            time,
            date,
        }
    }

    async fn write_entry<R: AsyncRead + Unpin>(
        &mut self,
        name: &[u8],
        reader: &mut R,
    ) -> io::Result<()> {
        if name.len() > u16::MAX as usize {
            eprintln!("ged: admin: zip entry name too long, skipped");
            return Ok(());
        }
        let local_off = u32_len(self.offset)?;
        let local = local_header(name, self.time, self.date);
        self.writer.write_all(&local).await?;
        self.offset += local.len() as u64;

        let (crc, size) = copy_chunked(reader, &mut self.writer).await?;
        // The payload sits between the local header and the descriptor.
        // Leaving it out of `offset` points the central directory into the
        // file bytes.
        self.offset += size;
        let size = u32_len(size)?;
        let descriptor = data_descriptor(crc, size);
        self.writer.write_all(&descriptor).await?;
        self.offset += descriptor.len() as u64;

        self.central.extend(central_header(
            name, crc, size, local_off, self.time, self.date,
        ));
        self.count = self.count.saturating_add(1);
        Ok(())
    }

    async fn finish(mut self) -> io::Result<()> {
        let cd_off = u32_len(self.offset)?;
        let cd_size = u32_len(self.central.len() as u64)?;
        self.writer.write_all(&self.central).await?;
        self.writer
            .write_all(&eocd(self.count, cd_size, cd_off))
            .await?;
        self.writer.flush().await
    }
}

/// Read and write `CHUNK` bytes at a time. The caller can prove a large file
/// is not buffered by watching how far the read cursor leads the write cursor.
async fn copy_chunked<R, W>(reader: &mut R, writer: &mut W) -> io::Result<(u32, u64)>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut crc = Crc32::new();
    let mut size = 0u64;
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        crc.update(&buf[..n]);
        writer.write_all(&buf[..n]).await?;
        size += u64::try_from(n).unwrap_or(u64::MAX);
    }
    Ok((crc.finish(), size))
}

fn u32_len(n: u64) -> io::Result<u32> {
    u32::try_from(n).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "zip larger than 4 GiB is not produced",
        )
    })
}

fn local_header(name: &[u8], time: u16, date: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(30 + name.len());
    put_u32(&mut out, 0x0403_4b50);
    put_u16(&mut out, 20);
    put_u16(&mut out, FLAG);
    put_u16(&mut out, 0);
    put_u16(&mut out, time);
    put_u16(&mut out, date);
    put_u32(&mut out, 0);
    put_u32(&mut out, 0);
    put_u32(&mut out, 0);
    put_u16(&mut out, name.len() as u16);
    put_u16(&mut out, 0);
    out.extend_from_slice(name);
    out
}

fn data_descriptor(crc: u32, size: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&0x0807_4b50u32.to_le_bytes());
    out[4..8].copy_from_slice(&crc.to_le_bytes());
    out[8..12].copy_from_slice(&size.to_le_bytes());
    out[12..16].copy_from_slice(&size.to_le_bytes());
    out
}

fn central_header(
    name: &[u8],
    crc: u32,
    size: u32,
    local_off: u32,
    time: u16,
    date: u16,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(46 + name.len());
    put_u32(&mut out, 0x0201_4b50);
    put_u16(&mut out, 20);
    put_u16(&mut out, 20);
    put_u16(&mut out, FLAG);
    put_u16(&mut out, 0);
    put_u16(&mut out, time);
    put_u16(&mut out, date);
    put_u32(&mut out, crc);
    put_u32(&mut out, size);
    put_u32(&mut out, size);
    put_u16(&mut out, name.len() as u16);
    put_u16(&mut out, 0);
    put_u16(&mut out, 0);
    put_u16(&mut out, 0);
    put_u16(&mut out, 0);
    put_u32(&mut out, 0);
    put_u32(&mut out, local_off);
    out.extend_from_slice(name);
    out
}

fn eocd(count: u16, cd_size: u32, cd_off: u32) -> [u8; 22] {
    let mut out = [0u8; 22];
    out[0..4].copy_from_slice(&0x0605_4b50u32.to_le_bytes());
    out[8..10].copy_from_slice(&count.to_le_bytes());
    out[10..12].copy_from_slice(&count.to_le_bytes());
    out[12..16].copy_from_slice(&cd_size.to_le_bytes());
    out[16..20].copy_from_slice(&cd_off.to_le_bytes());
    out
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn dos_now() -> (u16, u16) {
    let now = chrono::Utc::now();
    let year = u16::try_from(now.year().clamp(1980, 2107) - 1980).unwrap_or(0);
    let time = (now.hour() as u16) << 11 | (now.minute() as u16) << 5 | (now.second() as u16 / 2);
    let date = year << 9 | (now.month() as u16) << 5 | (now.day() as u16);
    (time, date)
}

/// IEEE CRC-32 (ISO-HDLC), the checksum ZIP stores.
struct Crc32(u32);

impl Crc32 {
    fn new() -> Self {
        Self(0xFFFF_FFFF)
    }

    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.0 ^= u32::from(byte);
            for _ in 0..8 {
                let mask = (self.0 & 1).wrapping_neg();
                self.0 = (self.0 >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
    }

    fn finish(self) -> u32 {
        self.0 ^ 0xFFFF_FFFF
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::task::{Context, Poll};

    #[test]
    fn crc32_matches_the_iso_check_value() {
        let mut crc = Crc32::new();
        crc.update(b"123456789");
        assert_eq!(crc.finish(), 0xCBF4_3926);
    }

    #[tokio::test]
    async fn entries_round_trip_with_the_names_and_bytes_given() {
        let bytes = zip_of(&[("a.txt", b"hello"), ("b.txt", b"world")]).await;
        let entries = read_stored(&bytes);
        assert_eq!(
            entries,
            vec![
                ("a.txt".to_owned(), b"hello".to_vec()),
                ("b.txt".to_owned(), b"world".to_vec()),
            ]
        );
    }

    /// Review Focus 1: a hostile name is an entry name, never a path and never
    /// a header. The bytes inside the archive are what a client unzips.
    #[tokio::test]
    async fn hostile_entry_names_stay_inside_one_flat_archive() {
        let hostile = [
            "../etc/passwd",
            "a\r\nContent-Disposition: attachment",
            "quote\".pdf",
            "é/../x",
            "/etc/passwd",
            "line\nbreak",
        ];
        for raw in hostile {
            let safe = crate::storage::disposition::zip_entry_name(
                "11111111-1111-1111-1111-111111111111",
                raw,
            );
            let bytes = zip_of(&[(&safe, b"hi")]).await;
            let entries = read_stored(&bytes);
            assert_eq!(entries.len(), 1, "{raw:?}");
            let name = &entries[0].0;
            assert_eq!(name, &safe, "{raw:?}");
            assert!(!name.contains('/') && !name.contains('\\'), "{name:?}");
            assert!(!name.chars().any(char::is_control), "{name:?}");
            assert_eq!(entries[0].1, b"hi");
        }
    }

    /// The read cursor never runs ahead of the bytes already handed to the
    /// archive by more than one chunk, for a file much larger than that chunk.
    #[tokio::test]
    async fn a_large_file_is_copied_in_chunks_not_buffered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        let size = 8 * 1024 * 1024;
        tokio::fs::write(&path, vec![0x5a; size]).await.unwrap();

        let read = Arc::new(AtomicU64::new(0));
        let written = Arc::new(AtomicU64::new(0));
        let max_pending = Arc::new(AtomicU64::new(0));
        let reader = ProbeRead {
            inner: tokio::fs::File::open(&path).await.unwrap(),
            read: Arc::clone(&read),
        };
        let writer = ProbeWrite {
            inner: Discard,
            read: Arc::clone(&read),
            written: Arc::clone(&written),
            max_pending: Arc::clone(&max_pending),
        };
        write_entries_one(writer, "id-big.bin", reader)
            .await
            .unwrap();

        let pending = max_pending.load(Ordering::Relaxed);
        let produced = written.load(Ordering::Relaxed);
        assert!(
            pending < 256 * 1024,
            "read cursor ran {pending} bytes ahead of the archive (file is {size})"
        );
        assert!(produced > size as u64, "archive shorter than the file");
        assert!(
            produced < size as u64 + 4096,
            "archive {produced} is much larger than the stored file {size}"
        );
    }

    async fn write_entries_one<W, R>(writer: W, name: &str, reader: R) -> io::Result<()>
    where
        W: AsyncWrite + Unpin,
        R: AsyncRead + Unpin,
    {
        let mut archive = Archive::new(writer);
        let mut reader = reader;
        archive.write_entry(name.as_bytes(), &mut reader).await?;
        archive.finish().await
    }

    async fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut archive = Archive::new(&mut out);
        for (name, bytes) in entries {
            let mut reader = std::io::Cursor::new(*bytes);
            archive
                .write_entry(name.as_bytes(), &mut reader)
                .await
                .unwrap();
        }
        archive.finish().await.unwrap();
        out
    }

    fn read_stored(buf: &[u8]) -> Vec<(String, Vec<u8>)> {
        let eocd = (0..buf.len().saturating_sub(21))
            .rev()
            .find(|i| buf[*i..].starts_with(&0x0605_4b50u32.to_le_bytes()))
            .expect("eocd");
        let count = u16::from_le_bytes([buf[eocd + 10], buf[eocd + 11]]) as usize;
        let mut p = u32::from_le_bytes(buf[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            assert_eq!(&buf[p..p + 4], &0x0201_4b50u32.to_le_bytes());
            let method = u16::from_le_bytes([buf[p + 10], buf[p + 11]]);
            assert_eq!(method, 0, "stored");
            let size = u32::from_le_bytes(buf[p + 20..p + 24].try_into().unwrap()) as usize;
            let name_len = u16::from_le_bytes([buf[p + 28], buf[p + 29]]) as usize;
            let extra_len = u16::from_le_bytes([buf[p + 30], buf[p + 31]]) as usize;
            let comment_len = u16::from_le_bytes([buf[p + 32], buf[p + 33]]) as usize;
            let local_off = u32::from_le_bytes(buf[p + 42..p + 46].try_into().unwrap()) as usize;
            let name = String::from_utf8(buf[p + 46..p + 46 + name_len].to_vec()).unwrap();
            assert_eq!(
                &buf[local_off..local_off + 4],
                &0x0403_4b50u32.to_le_bytes()
            );
            let local_name =
                u16::from_le_bytes([buf[local_off + 26], buf[local_off + 27]]) as usize;
            let local_extra =
                u16::from_le_bytes([buf[local_off + 28], buf[local_off + 29]]) as usize;
            let data = local_off + 30 + local_name + local_extra;
            entries.push((name, buf[data..data + size].to_vec()));
            p += 46 + name_len + extra_len + comment_len;
        }
        entries
    }

    struct ProbeRead<R> {
        inner: R,
        read: Arc<AtomicU64>,
    }

    impl<R: AsyncRead + Unpin> AsyncRead for ProbeRead<R> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let before = buf.filled().len();
            match Pin::new(&mut self.inner).poll_read(cx, buf) {
                Poll::Ready(Ok(())) => {
                    let n = buf.filled().len() - before;
                    self.read.fetch_add(n as u64, Ordering::Relaxed);
                    Poll::Ready(Ok(()))
                }
                other => other,
            }
        }
    }

    struct ProbeWrite<W> {
        inner: W,
        read: Arc<AtomicU64>,
        written: Arc<AtomicU64>,
        max_pending: Arc<AtomicU64>,
    }

    impl<W: AsyncWrite + Unpin> AsyncWrite for ProbeWrite<W> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            let pending = self
                .read
                .load(Ordering::Relaxed)
                .saturating_sub(self.written.load(Ordering::Relaxed));
            self.max_pending.fetch_max(pending, Ordering::Relaxed);
            match Pin::new(&mut self.inner).poll_write(cx, buf) {
                Poll::Ready(Ok(n)) => {
                    self.written.fetch_add(n as u64, Ordering::Relaxed);
                    Poll::Ready(Ok(n))
                }
                other => other,
            }
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }

        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    struct Discard;

    impl AsyncWrite for Discard {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
}
