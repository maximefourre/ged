//! Admin bulk download: a stored ZIP written entry by entry.
//!
//! File bytes are copied in fixed chunks straight into the archive. The
//! central directory (names and sizes only) is the only thing held until the
//! end, so a large document does not sit in memory beside its ZIP entry.
//! Entry names are the caller's already-sanitised strings; this writer never
//! interprets them as paths.
//!
//! ZIP64 starts at the same thresholds as Rails `zip_kit`: an entry whose
//! size or local-header offset does not fit in a ZIP `u32` (4 GiB), or an
//! archive with more than 65535 entries or a central directory that does not
//! fit. The local header stays a data descriptor (general-purpose bit 3).
//! The 64-bit sizes are written after the bytes and again in the central
//! directory, which is what a reader uses.

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

/// Version needed for a classic archive, and for ZIP64 (4.5).
const VER_CLASSIC: u16 = 20;
const VER_ZIP64: u16 = 45;

/// Sentinel ZIP stores when a 16-bit or 32-bit field overflows.
const ZIP64_U16: u16 = u16::MAX;
const ZIP64_U32: u32 = u32::MAX;

fn exceeds_u32(n: u64) -> bool {
    n > u64::from(u32::MAX)
}

/// One entry needs a ZIP64 extra when its size or its local-header offset
/// does not fit the classic fields. Matches `ZipKit::ZipWriter`.
fn entry_needs_zip64(size: u64, local_off: u64) -> bool {
    exceeds_u32(size) || exceeds_u32(local_off)
}

/// The end-of-central-directory record needs ZIP64 when the count, the
/// directory, or the offset of that record itself does not fit.
fn archive_needs_zip64(count: u64, cd_size: u64, cd_off: u64) -> bool {
    count > u64::from(u16::MAX)
        || exceeds_u32(cd_size)
        || exceeds_u32(cd_off)
        || exceeds_u32(cd_off.saturating_add(cd_size))
}

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

struct EntryMeta {
    name: Vec<u8>,
    crc: u32,
    size: u64,
    local_off: u64,
    time: u16,
    date: u16,
}

struct Archive<W> {
    writer: W,
    entries: Vec<EntryMeta>,
    offset: u64,
    time: u16,
    date: u16,
}

impl<W: AsyncWrite + Unpin> Archive<W> {
    fn new(writer: W) -> Self {
        let (time, date) = dos_now();
        Self {
            writer,
            entries: Vec::new(),
            offset: 0,
            time,
            date,
        }
    }

    async fn write_entry<R: AsyncRead + Unpin>(
        &mut self,
        name: &[u8],
        reader: &mut R,
    ) -> io::Result<()> {
        if name.len() > usize::from(u16::MAX) {
            eprintln!("ged: admin: zip entry name too long, skipped");
            return Ok(());
        }
        let local_off = self.offset;
        self.write_local(name).await?;
        let (crc, size) = copy_chunked(reader, &mut self.writer).await?;
        self.commit_entry(name, crc, size, local_off).await
    }

    /// Records an entry whose body is not copied through `writer`.
    ///
    /// The logical offset still advances by `size`, which is how `zip_kit`'s
    /// `simulate_write` keeps a streamed archive consistent when the bytes
    /// bypass the writer. Tests use it for a body past 4 GiB.
    #[cfg(test)]
    async fn write_accounted_entry(&mut self, name: &[u8], crc: u32, size: u64) -> io::Result<()> {
        if name.len() > usize::from(u16::MAX) {
            eprintln!("ged: admin: zip entry name too long, skipped");
            return Ok(());
        }
        let local_off = self.offset;
        self.write_local(name).await?;
        self.commit_entry(name, crc, size, local_off).await
    }

    async fn write_local(&mut self, name: &[u8]) -> io::Result<()> {
        let local = local_header(name, self.time, self.date);
        self.writer.write_all(&local).await?;
        let Ok(n) = u64::try_from(local.len()) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "zip header length",
            ));
        };
        self.offset = self.offset.saturating_add(n);
        Ok(())
    }

    async fn commit_entry(
        &mut self,
        name: &[u8],
        crc: u32,
        size: u64,
        local_off: u64,
    ) -> io::Result<()> {
        // The payload sits between the local header and the descriptor.
        // `copy_chunked` has already written it; an accounted entry only
        // advances the cursor. Leaving the size out of `offset` would point
        // the central directory into the file bytes.
        self.offset = self.offset.saturating_add(size);
        let descriptor = data_descriptor(crc, size);
        self.writer.write_all(&descriptor).await?;
        let Ok(n) = u64::try_from(descriptor.len()) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "zip header length",
            ));
        };
        self.offset = self.offset.saturating_add(n);
        self.entries.push(EntryMeta {
            name: name.to_vec(),
            crc,
            size,
            local_off,
            time: self.time,
            date: self.date,
        });
        Ok(())
    }

    async fn finish(mut self) -> io::Result<()> {
        let cd_off = self.offset;
        let mut central = Vec::new();
        for entry in &self.entries {
            central.extend(central_header(entry));
        }
        self.writer.write_all(&central).await?;
        let Ok(cd_size) = u64::try_from(central.len()) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "zip central directory length",
            ));
        };
        let count = u64::try_from(self.entries.len()).unwrap_or(u64::MAX);
        if archive_needs_zip64(count, cd_size, cd_off) {
            let eocd64_off = cd_off.saturating_add(cd_size);
            self.writer
                .write_all(&zip64_eocd(count, cd_size, cd_off))
                .await?;
            self.writer.write_all(&zip64_locator(eocd64_off)).await?;
            self.writer
                .write_all(&eocd(ZIP64_U16, ZIP64_U32, ZIP64_U32))
                .await?;
        } else {
            self.writer
                .write_all(&eocd(
                    u16::try_from(count).unwrap_or(ZIP64_U16),
                    u32::try_from(cd_size).unwrap_or(ZIP64_U32),
                    u32::try_from(cd_off).unwrap_or(ZIP64_U32),
                ))
                .await?;
        }
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

fn local_header(name: &[u8], time: u16, date: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(30 + name.len());
    put_u32(&mut out, 0x0403_4b50);
    put_u16(&mut out, VER_CLASSIC);
    put_u16(&mut out, FLAG);
    put_u16(&mut out, 0);
    put_u16(&mut out, time);
    put_u16(&mut out, date);
    put_u32(&mut out, 0);
    put_u32(&mut out, 0);
    put_u32(&mut out, 0);
    put_u16(&mut out, u16::try_from(name.len()).unwrap_or(ZIP64_U16));
    put_u16(&mut out, 0);
    out.extend_from_slice(name);
    out
}

/// 16 bytes when the size fits in a `u32`, 24 when it needs ZIP64. The local
/// header has already been sent with zeros; readers take the width from the
/// central directory, as `zip_kit` does for a streamed entry.
fn data_descriptor(crc: u32, size: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    put_u32(&mut out, 0x0807_4b50);
    put_u32(&mut out, crc);
    if exceeds_u32(size) {
        put_u64(&mut out, size);
        put_u64(&mut out, size);
    } else {
        let narrow = u32::try_from(size).unwrap_or(ZIP64_U32);
        put_u32(&mut out, narrow);
        put_u32(&mut out, narrow);
    }
    out
}

fn central_header(entry: &EntryMeta) -> Vec<u8> {
    let zip64 = entry_needs_zip64(entry.size, entry.local_off);
    let extra = if zip64 {
        zip64_central_extra(entry.size, entry.local_off)
    } else {
        Vec::new()
    };
    let version = if zip64 { VER_ZIP64 } else { VER_CLASSIC };
    let mut out = Vec::with_capacity(46 + entry.name.len() + extra.len());
    put_u32(&mut out, 0x0201_4b50);
    put_u16(&mut out, version);
    put_u16(&mut out, version);
    put_u16(&mut out, FLAG);
    put_u16(&mut out, 0);
    put_u16(&mut out, entry.time);
    put_u16(&mut out, entry.date);
    put_u32(&mut out, entry.crc);
    if zip64 {
        put_u32(&mut out, ZIP64_U32);
        put_u32(&mut out, ZIP64_U32);
    } else {
        let size = u32::try_from(entry.size).unwrap_or(ZIP64_U32);
        put_u32(&mut out, size);
        put_u32(&mut out, size);
    }
    put_u16(
        &mut out,
        u16::try_from(entry.name.len()).unwrap_or(ZIP64_U16),
    );
    put_u16(&mut out, u16::try_from(extra.len()).unwrap_or(ZIP64_U16));
    put_u16(&mut out, 0);
    put_u16(&mut out, if zip64 { ZIP64_U16 } else { 0 });
    put_u16(&mut out, 0);
    put_u32(&mut out, 0);
    if zip64 {
        put_u32(&mut out, ZIP64_U32);
    } else {
        put_u32(
            &mut out,
            u32::try_from(entry.local_off).unwrap_or(ZIP64_U32),
        );
    }
    out.extend_from_slice(&entry.name);
    out.extend_from_slice(&extra);
    out
}

/// Uncompressed size, compressed size, local-header offset, disk start.
/// Present only when one of those classic fields is `0xFFFF` / `0xFFFFFFFF`.
fn zip64_central_extra(size: u64, local_off: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    put_u16(&mut out, 0x0001);
    put_u16(&mut out, 28);
    put_u64(&mut out, size);
    put_u64(&mut out, size);
    put_u64(&mut out, local_off);
    put_u32(&mut out, 0);
    out
}

fn zip64_eocd(count: u64, cd_size: u64, cd_off: u64) -> [u8; 56] {
    let mut out = [0u8; 56];
    out[0..4].copy_from_slice(&0x0606_4b50u32.to_le_bytes());
    out[4..12].copy_from_slice(&44u64.to_le_bytes());
    out[12..14].copy_from_slice(&VER_ZIP64.to_le_bytes());
    out[14..16].copy_from_slice(&VER_ZIP64.to_le_bytes());
    out[24..32].copy_from_slice(&count.to_le_bytes());
    out[32..40].copy_from_slice(&count.to_le_bytes());
    out[40..48].copy_from_slice(&cd_size.to_le_bytes());
    out[48..56].copy_from_slice(&cd_off.to_le_bytes());
    out
}

fn zip64_locator(eocd64_off: u64) -> [u8; 20] {
    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&0x0706_4b50u32.to_le_bytes());
    out[8..16].copy_from_slice(&eocd64_off.to_le_bytes());
    out[16..20].copy_from_slice(&1u32.to_le_bytes());
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

fn put_u64(out: &mut Vec<u8>, value: u64) {
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

    /// First length that does not fit a ZIP `u32` (zip_kit `FOUR_BYTE_MAX_UINT + 1`).
    const FOUR_GIB: u64 = (u32::MAX as u64) + 1;

    /// Name `a`: local 31, classic descriptor 16, classic central 47.
    /// The archive stays classic while `size + 94` still fits in a `u32`.
    fn classic_accounted_limit() -> u64 {
        u64::from(u32::MAX) - 94
    }

    async fn accounted(name: &str, size: u64) -> Vec<u8> {
        let mut out = Vec::new();
        let mut archive = Archive::new(&mut out);
        archive
            .write_accounted_entry(name.as_bytes(), 0, size)
            .await
            .unwrap();
        archive.finish().await.unwrap();
        assert!(
            out.len() < 4096,
            "accounted entry wrote {} bytes for a logical size of {size}",
            out.len()
        );
        out
    }

    fn sigs(buf: &[u8], sig: u32) -> Vec<usize> {
        let needle = sig.to_le_bytes();
        buf.windows(4)
            .enumerate()
            .filter(|(_, window)| *window == needle)
            .map(|(i, _)| i)
            .collect()
    }

    #[tokio::test]
    async fn a_size_that_still_fits_stays_a_classic_zip() {
        let size = classic_accounted_limit();
        let buf = accounted("a", size).await;
        assert!(sigs(&buf, 0x0606_4b50).is_empty(), "unexpected zip64 eocd");
        assert!(
            sigs(&buf, 0x0706_4b50).is_empty(),
            "unexpected zip64 locator"
        );
        let local = 31;
        assert_eq!(&buf[local..local + 4], &0x0807_4b50u32.to_le_bytes());
        let stored = u32::from_le_bytes(buf[local + 8..local + 12].try_into().unwrap());
        assert_eq!(u64::from(stored), size);
        assert_eq!(buf.len(), local + 16 + 47 + 22);
    }

    #[tokio::test]
    async fn an_archive_past_4_gib_uses_zip64_for_the_end_record_only() {
        let size = classic_accounted_limit() + 1;
        let buf = accounted("a", size).await;
        assert_eq!(sigs(&buf, 0x0606_4b50).len(), 1);
        let central = sigs(&buf, 0x0201_4b50);
        assert_eq!(central.len(), 1);
        let at = central[0];
        let needed = u16::from_le_bytes([buf[at + 6], buf[at + 7]]);
        assert_eq!(needed, VER_CLASSIC, "the entry size still fits a u32");
        let stored = u32::from_le_bytes(buf[at + 20..at + 24].try_into().unwrap());
        assert_eq!(u64::from(stored), size);
        let eocd64 = sigs(&buf, 0x0606_4b50)[0];
        let count = u64::from_le_bytes(buf[eocd64 + 24..eocd64 + 32].try_into().unwrap());
        assert_eq!(count, 1);
        let eocd = sigs(&buf, 0x0605_4b50)[0];
        assert_eq!(u16::from_le_bytes([buf[eocd + 10], buf[eocd + 11]]), 0xFFFF);
        assert_eq!(
            u32::from_le_bytes(buf[eocd + 16..eocd + 20].try_into().unwrap()),
            0xFFFF_FFFF
        );
    }

    /// The body is not copied (zip_kit `simulate_write`). The descriptor and
    /// the central extra still carry the 64-bit size a streamed entry emits
    /// once `copy_chunked` has counted past 4 GiB.
    #[tokio::test]
    async fn a_streamed_entry_past_4_gib_is_zip64_and_is_not_buffered() {
        let buf = accounted("big.bin", FOUR_GIB).await;
        let local = 30 + "big.bin".len();
        assert_eq!(&buf[local..local + 4], &0x0807_4b50u32.to_le_bytes());
        let size = u64::from_le_bytes(buf[local + 8..local + 16].try_into().unwrap());
        let compressed = u64::from_le_bytes(buf[local + 16..local + 24].try_into().unwrap());
        assert_eq!((size, compressed), (FOUR_GIB, FOUR_GIB));
        assert_eq!(&buf[local + 24..local + 28], &0x0201_4b50u32.to_le_bytes());
        let at = local + 24;
        assert_eq!(u16::from_le_bytes([buf[at + 6], buf[at + 7]]), VER_ZIP64);
        assert_eq!(
            u32::from_le_bytes(buf[at + 20..at + 24].try_into().unwrap()),
            ZIP64_U32
        );
        let extra_len = u16::from_le_bytes([buf[at + 30], buf[at + 31]]);
        assert_eq!(extra_len, 32);
        let extra = at + 46 + "big.bin".len();
        assert_eq!(&buf[extra..extra + 2], &0x0001u16.to_le_bytes());
        let recorded = u64::from_le_bytes(buf[extra + 4..extra + 12].try_into().unwrap());
        assert_eq!(recorded, FOUR_GIB);
        assert_eq!(sigs(&buf, 0x0606_4b50).len(), 1);
    }

    #[tokio::test]
    async fn more_than_65535_entries_write_a_zip64_end_record() {
        let mut out = Vec::new();
        let mut archive = Archive::new(&mut out);
        for i in 0..65_536 {
            let name = format!("{i:05}");
            let mut reader = std::io::Cursor::new(&b""[..]);
            archive
                .write_entry(name.as_bytes(), &mut reader)
                .await
                .unwrap();
        }
        archive.finish().await.unwrap();

        assert_eq!(sigs(&out, 0x0606_4b50).len(), 1);
        let eocd64 = sigs(&out, 0x0606_4b50)[0];
        let count = u64::from_le_bytes(out[eocd64 + 24..eocd64 + 32].try_into().unwrap());
        let cd_size = u64::from_le_bytes(out[eocd64 + 40..eocd64 + 48].try_into().unwrap());
        let cd_off = u64::from_le_bytes(out[eocd64 + 48..eocd64 + 56].try_into().unwrap());
        assert_eq!(count, 65_536);
        let cd_off = usize::try_from(cd_off).unwrap();
        assert!(cd_off < out.len());
        assert_eq!(&out[cd_off..cd_off + 4], &0x0201_4b50u32.to_le_bytes());
        assert_eq!(&out[cd_off + 46..cd_off + 51], b"00000");
        let last = cd_off + 51 * 65_535;
        assert_eq!(&out[last..last + 4], &0x0201_4b50u32.to_le_bytes());
        assert_eq!(&out[last + 46..last + 51], b"65535");
        assert_eq!(cd_size, 51 * 65_536);

        let eocd = sigs(&out, 0x0605_4b50)[0];
        assert_eq!(u16::from_le_bytes([out[eocd + 10], out[eocd + 11]]), 0xFFFF);
        assert_eq!(
            u32::from_le_bytes(out[eocd + 16..eocd + 20].try_into().unwrap()),
            0xFFFF_FFFF
        );
        assert_eq!(&out[..4], &0x0403_4b50u32.to_le_bytes());
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
