//! `multipart/form-data` intake for `POST /api/v1/documents`.
//!
//! Rack parses the whole multipart body before the controller runs (a broken
//! one is a 400 even without a token), spooling file parts to temp files.
//! This does the same: text parts become ordinary parameters, and the `file`
//! part is streamed straight into the [`Storage`] under a fresh key — never
//! buffered in memory — so the caller either keeps it (sync upload), moves it
//! to staging (async upload) or deletes it. Whatever goes wrong here leaves no
//! file behind.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::HeaderMap;
use axum::http::header::CONTENT_TYPE;
use bytes::Bytes;
use futures_util::{Stream, TryStreamExt};
use tokio_util::io::StreamReader;

use crate::http::params::{FORM_BYTES_LIMIT, RackParams};
use crate::storage::{Storage, StorageError, Stored};

/// Rack's `multipart_file_limit` (the old name is `multipart_part_limit`).
/// A part counts when its `Content-Disposition` carries a `filename`,
/// including a blank one. The 128th such part is [`IntakeError::PartLimit`].
pub const FILE_PART_LIMIT: usize = 128;

/// Rack's `multipart_total_part_limit`. The 4096th part, of any kind, is
/// [`IntakeError::PartLimit`]. Text parts below that are ordinary parameters.
pub const TOTAL_PART_LIMIT: usize = 4096;

/// Most bytes of one part's headers kept while scanning for a bare CR/LF.
/// The part body is never copied into this buffer.
const PART_HEADER_LIMIT: usize = 64 * 1024;

pub struct UploadedFile {
    pub stored: Stored,
    /// The name the client gave the part.
    pub original_filename: String,
    /// The part's `Content-Type` header as sent; `None` when absent.
    pub content_type: Option<String>,
}

pub struct Intake {
    pub params: RackParams,
    pub file: Option<UploadedFile>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum IntakeError {
    /// More than `GED_MAX_UPLOAD_BYTES` in the body: 413 with an empty body.
    TooLarge,
    /// Unparsable multipart or parameters: the generic 400.
    Malformed,
    /// Rack's file limit (128 filenames) or total part limit (4096).
    /// The Rails stack answers 500 with an empty body.
    PartLimit,
}

/// The `boundary` of a `multipart/*` content type (Rack's
/// `%r|\Amultipart/.*boundary=\"?([^\";,]+)\"?|ni`). `None` means the body is
/// not multipart as far as Rack is concerned.
pub fn boundary(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(CONTENT_TYPE)?.to_str().ok()?;
    let lower = value.to_ascii_lowercase();
    if !lower.starts_with("multipart/") {
        return None;
    }
    // `.*boundary=` is greedy: the last occurrence wins
    let at = lower.rfind("boundary=")? + "boundary=".len();
    let rest = value[at..].strip_prefix('"').unwrap_or(&value[at..]);
    let end = rest.find(['"', ';', ',']).unwrap_or(rest.len());
    let boundary = &rest[..end];
    (!boundary.is_empty()).then(|| boundary.to_owned())
}

/// Reads the multipart body. `max_body` is `GED_MAX_UPLOAD_BYTES`, the limit
/// on the *request body* (the proxy's `MAX_REQUEST_BODY`).
///
/// A part whose headers contain a bare CR or LF is dropped, as Rack does, and
/// the other parts are kept. Observed on the Rails stack (2026-11-11): a
/// filename `line\n74.txt` or `line\r74.txt`. Only that header block is
/// buffered, and only up to [`PART_HEADER_LIMIT`]. The part body is forwarded
/// as it arrives; the `file` part is streamed into [`Storage`].
pub async fn read(
    boundary: String,
    body: Body,
    storage: &Storage,
    max_body: u64,
) -> Result<Intake, IntakeError> {
    let seen = Arc::new(AtomicU64::new(0));
    let exceeded = Arc::new(AtomicBool::new(false));
    let counted = {
        let seen = seen.clone();
        let exceeded = exceeded.clone();
        body.into_data_stream()
            .map_err(std::io::Error::other)
            .and_then(move |chunk| {
                let total =
                    seen.fetch_add(chunk.len() as u64, Ordering::SeqCst) + chunk.len() as u64;
                let over = total > max_body;
                if over {
                    exceeded.store(true, Ordering::SeqCst);
                }
                std::future::ready(if over {
                    Err(std::io::Error::other("body over the upload limit"))
                } else {
                    Ok(chunk)
                })
            })
    };
    let scrubbed = ScrubStream {
        gather: Vec::new(),
        inner: Box::pin(counted),
        scrubber: Scrubber::new(&boundary),
        pending: VecDeque::new(),
        inner_done: false,
        finished: false,
        burst: 0,
    };
    let mut multipart = multer::Multipart::new(scrubbed, boundary);

    let mut fields = String::new();
    let mut file: Option<UploadedFile> = None;
    let mut parts = 0usize;
    let mut file_parts = 0usize;
    let mut text_bytes = 0usize;
    let result = loop {
        let mut field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break Ok(()),
            Err(_) if exceeded.load(Ordering::SeqCst) => break Err(IntakeError::TooLarge),
            Err(_) => break Err(IntakeError::Malformed),
        };
        parts += 1;
        // Rack opens a tempfile for every part that has a filename parameter,
        // then drops a blank filename from the params. Both still count.
        if field.file_name().is_some() {
            file_parts += 1;
        }
        if file_parts >= FILE_PART_LIMIT || parts >= TOTAL_PART_LIMIT {
            break Err(IntakeError::PartLimit);
        }
        let Some(name) = field.name().map(str::to_owned) else {
            continue;
        };
        match field.file_name().map(str::to_owned) {
            // A file input left empty: Rack drops the part.
            Some(filename) if filename.is_empty() => {}
            Some(filename) if name == "file" => {
                let content_type = field
                    .headers()
                    .get(CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                let reader = StreamReader::new(field.map_err(std::io::Error::other));
                match storage.put(reader, max_body).await {
                    Ok(stored) => {
                        // a repeated `file` part: the last one wins
                        if let Some(previous) = file.replace(UploadedFile {
                            stored,
                            original_filename: filename,
                            content_type,
                        }) {
                            let _ = storage.delete(&previous.stored.key).await;
                        }
                    }
                    Err(StorageError::TooLarge { .. }) => {
                        exceeded.store(true, Ordering::SeqCst);
                        break Err(IntakeError::TooLarge);
                    }
                    Err(_) if exceeded.load(Ordering::SeqCst) => break Err(IntakeError::TooLarge),
                    Err(_) => break Err(IntakeError::Malformed),
                }
            }
            // A file under another name is not something the API reads.
            Some(_) => {}
            None => {
                // Chunk by chunk: stop as soon as the running total passes
                // the form limit, so an unauthenticated client cannot make
                // the server hold a whole upload-sized text part (review M1).
                let mut bytes: Vec<u8> = Vec::new();
                let mut failed = None;
                loop {
                    match field.chunk().await {
                        Ok(Some(chunk)) => {
                            text_bytes += chunk.len();
                            if text_bytes > FORM_BYTES_LIMIT {
                                failed = Some(IntakeError::Malformed);
                                break;
                            }
                            bytes.extend_from_slice(&chunk);
                        }
                        Ok(None) => break,
                        Err(_) => {
                            failed = Some(if exceeded.load(Ordering::SeqCst) {
                                IntakeError::TooLarge
                            } else {
                                IntakeError::Malformed
                            });
                            break;
                        }
                    }
                }
                if let Some(error) = failed {
                    break Err(error);
                }
                let Ok(value) = String::from_utf8(bytes) else {
                    break Err(IntakeError::Malformed);
                };
                if !fields.is_empty() {
                    fields.push('&');
                }
                fields.push_str(&urlencode(&name));
                fields.push('=');
                fields.push_str(&urlencode(&value));
            }
        }
    };

    let outcome = match result {
        Ok(()) => RackParams::parse(&fields)
            .map(|params| Intake { params, file: None })
            .map_err(|_| IntakeError::Malformed),
        Err(error) => Err(error),
    };
    match outcome {
        Ok(mut intake) => {
            intake.file = file;
            Ok(intake)
        }
        Err(e) => {
            if let Some(file) = file {
                let _ = storage.delete(&file.stored.key).await;
            }
            Err(e)
        }
    }
}

#[derive(Debug)]
enum ScrubError {
    /// Part headers ran past [`PART_HEADER_LIMIT`] without a blank line.
    HeaderTooLarge,
    /// A broken-header part ended before the next delimiter.
    Truncated,
}

/// Forwards a multipart body, dropping parts whose headers contain a bare CR
/// or LF. Header bytes are kept only up to [`PART_HEADER_LIMIT`]. Part bodies
/// are emitted as they arrive; at most `delim.len()` bytes are held back so a
/// boundary split across chunks is still recognised.
struct Scrubber {
    marker: Vec<u8>,
    delim: Vec<u8>,
    phase: Phase,
    buf: Vec<u8>,
    /// Parsed before the rest of the current chunk. A header over-read or a
    /// holdback tail only — never the file body.
    lead: Option<Vec<u8>>,
    /// Tails waiting behind [`Self::lead`], oldest last.
    lead_after: Vec<Vec<u8>>,
    /// The phase advanced; the same input slice is still the next bytes.
    respin: bool,
    peak: usize,
    /// Number of `push` calls (the CPU cost driver, see [`GATHER_BYTES`]).
    #[cfg(test)]
    pushes: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Preamble,
    Part,
    Body,
    Skip,
    Copy,
}

struct ScrubStream<S> {
    inner: Pin<Box<S>>,
    scrubber: Scrubber,
    pending: VecDeque<Bytes>,
    inner_done: bool,
    finished: bool,
    /// Bytes handed out since the last `Pending`; see [`BURST_LIMIT`].
    burst: usize,
    /// Small chunks received while a part header is still being read, pushed
    /// to the scrubber together; see [`GATHER_BYTES`].
    gather: Vec<u8>,
}

/// While a preamble or part header is incomplete the scrubber rescans what it
/// holds (up to [`PART_HEADER_LIMIT`]) on every push, so a header sent one
/// byte at a time cost 8 s of CPU per connection before authentication
/// (measured, final review m12). Chunks smaller than this are gathered until
/// they reach it, which divides the pushes (and the cost) by the chunk size
/// for a hostile client and changes nothing for a normal one, whose headers
/// arrive in one or two reads. What is gathered is never part of a file body.
const GATHER_BYTES: usize = 4096;

/// `multer` polls its source in a loop until `Pending`, so an always-ready
/// source is read to the end into its buffer. Yielding `Pending` after this
/// many bytes bounds what it holds ahead of the field being read (review M1).
const BURST_LIMIT: usize = 256 * 1024;

impl<S> ScrubStream<S> {
    /// Feeds the scrubber and queues what it releases. The `Err` is the item
    /// to return when the headers exceed the limit.
    fn push_chunk(
        &mut self,
        chunk: &[u8],
    ) -> Result<(), Poll<Option<Result<Bytes, std::io::Error>>>> {
        match self.scrubber.push(chunk) {
            Ok(bytes) => {
                if !bytes.is_empty() {
                    self.pending.push_back(Bytes::from(bytes));
                }
                Ok(())
            }
            Err(_) => {
                self.finished = true;
                Err(Poll::Ready(Some(Err(std::io::Error::other(
                    "multipart part headers exceed the limit",
                )))))
            }
        }
    }
}

impl<S> Stream for ScrubStream<S>
where
    S: Stream<Item = Result<Bytes, std::io::Error>>,
{
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if this.burst >= BURST_LIMIT {
                this.burst = 0;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            if let Some(chunk) = this.pending.pop_front() {
                if !chunk.is_empty() {
                    this.burst += chunk.len();
                    return Poll::Ready(Some(Ok(chunk)));
                }
                continue;
            }
            if this.finished {
                return Poll::Ready(None);
            }
            if this.inner_done && !this.gather.is_empty() {
                let gathered = std::mem::take(&mut this.gather);
                if let Err(poisoned) = this.push_chunk(&gathered) {
                    return poisoned;
                }
                continue;
            }
            if this.inner_done {
                return match this.scrubber.finish() {
                    Ok(bytes) => {
                        this.finished = true;
                        if bytes.is_empty() {
                            Poll::Ready(None)
                        } else {
                            Poll::Ready(Some(Ok(Bytes::from(bytes))))
                        }
                    }
                    Err(_) => {
                        this.finished = true;
                        Poll::Ready(Some(Err(std::io::Error::other(
                            "multipart part headers are incomplete",
                        ))))
                    }
                };
            }
            match this.inner.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => this.inner_done = true,
                Poll::Ready(Some(Err(err))) => return Poll::Ready(Some(Err(err))),
                Poll::Ready(Some(Ok(chunk))) => {
                    let small = chunk.len() < GATHER_BYTES;
                    if small && this.scrubber.reading_headers() {
                        this.gather.extend_from_slice(&chunk);
                        if this.gather.len() < GATHER_BYTES {
                            continue;
                        }
                        let gathered = std::mem::take(&mut this.gather);
                        if let Err(poisoned) = this.push_chunk(&gathered) {
                            return poisoned;
                        }
                    } else if this.gather.is_empty() {
                        if let Err(poisoned) = this.push_chunk(&chunk) {
                            return poisoned;
                        }
                    } else {
                        this.gather.extend_from_slice(&chunk);
                        let gathered = std::mem::take(&mut this.gather);
                        if let Err(poisoned) = this.push_chunk(&gathered) {
                            return poisoned;
                        }
                    }
                }
            }
        }
    }
}

impl Scrubber {
    fn new(boundary: &str) -> Self {
        let marker = format!("--{boundary}").into_bytes();
        let mut delim = Vec::with_capacity(2 + marker.len());
        delim.extend_from_slice(b"\r\n");
        delim.extend_from_slice(&marker);
        Self {
            marker,
            delim,
            phase: Phase::Preamble,
            buf: Vec::new(),
            lead: None,
            lead_after: Vec::new(),
            respin: false,
            peak: 0,
            #[cfg(test)]
            pushes: 0,
        }
    }

    #[cfg(test)]
    fn peak(&self) -> usize {
        self.peak
    }

    fn note(&mut self) {
        self.peak = self.peak.max(self.buf.len());
    }

    /// A preamble or part header is still being read (and rescanned per push).
    fn reading_headers(&self) -> bool {
        matches!(self.phase, Phase::Preamble | Phase::Part)
    }

    fn push(&mut self, data: &[u8]) -> Result<Vec<u8>, ScrubError> {
        #[cfg(test)]
        {
            self.pushes += 1;
        }
        let mut out = Vec::new();
        self.drive(data, &mut out)?;
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<u8>, ScrubError> {
        let mut out = Vec::new();
        match self.phase {
            Phase::Preamble | Phase::Part | Phase::Body => {
                out.extend_from_slice(&self.buf);
                self.buf.clear();
            }
            Phase::Copy => {}
            // The broken part never reached its next delimiter. Replaying it
            // would mean keeping the body. `read` turns this into Malformed,
            // which is also what multer does with the unfiltered bytes.
            Phase::Skip => return Err(ScrubError::Truncated),
        }
        Ok(out)
    }

    /// One chunk, iterative. A recursive resume copied the tail of the chunk
    /// once per part and overflowed the stack at a few thousand parts.
    fn drive(&mut self, data: &[u8], out: &mut Vec<u8>) -> Result<(), ScrubError> {
        let mut pos = 0usize;
        let mut guard = 0usize;
        let guard_limit = data.len().saturating_mul(4).saturating_add(10_000);
        loop {
            guard += 1;
            if guard > guard_limit {
                return Err(ScrubError::HeaderTooLarge);
            }
            if let Some(next) = self.lead.take() {
                let consumed = self.consume(&next, out)?;
                if self.lead.is_some() {
                    let remainder = next.get(consumed..).unwrap_or(&[]).to_vec();
                    if !remainder.is_empty() {
                        self.lead_after.push(remainder);
                    }
                    continue;
                }
                if self.respin {
                    self.respin = false;
                    self.lead = Some(next.get(consumed..).unwrap_or(&[]).to_vec());
                    continue;
                }
                if consumed < next.len() {
                    if consumed == 0 {
                        return Ok(());
                    }
                    self.lead = Some(next[consumed..].to_vec());
                    continue;
                }
                if let Some(tail) = self.lead_after.pop() {
                    self.lead = Some(tail);
                }
                continue;
            }
            if pos >= data.len() {
                return Ok(());
            }
            let consumed = self.consume(&data[pos..], out)?;
            if self.lead.is_some() || self.respin {
                self.respin = false;
                pos += consumed;
                continue;
            }
            if consumed == 0 {
                return Ok(());
            }
            pos += consumed;
        }
    }

    fn consume(&mut self, data: &[u8], out: &mut Vec<u8>) -> Result<usize, ScrubError> {
        match self.phase {
            Phase::Preamble => self.drive_preamble(data, out),
            Phase::Part => self.drive_part(data, out),
            Phase::Body => self.drive_body(data, out),
            Phase::Skip => self.drive_skip(data, out),
            Phase::Copy => {
                out.extend_from_slice(data);
                Ok(data.len())
            }
        }
    }

    /// Resume at `marker_end` of `hold ++ data` without copying the tail.
    fn resume_after(
        &mut self,
        hold: &[u8],
        data: &[u8],
        marker_end: usize,
    ) -> Result<usize, ScrubError> {
        debug_assert!(marker_end <= hold.len() + data.len());
        if marker_end < hold.len() {
            self.lead = Some(hold[marker_end..].to_vec());
            return Ok(0);
        }
        if marker_end == hold.len() {
            self.respin = true;
            return Ok(0);
        }
        Ok(marker_end - hold.len())
    }

    fn drive_preamble(&mut self, data: &[u8], out: &mut Vec<u8>) -> Result<usize, ScrubError> {
        let hold = std::mem::take(&mut self.buf);
        if let Some(start) = find_across(&hold, data, &self.marker) {
            let end = hold.len() + data.len();
            let marker_end = (start + self.marker.len()).min(end);
            self.buf = virtual_to_vec(&hold, data, start, marker_end);
            self.note();
            self.phase = Phase::Part;
            let _ = (end, out);
            return self.resume_after(&hold, data, marker_end);
        }
        let total = hold.len() + data.len();
        if total > PART_HEADER_LIMIT {
            extend_virtual(out, &hold, data, 0, total);
            self.buf.clear();
            self.phase = Phase::Copy;
            return Ok(data.len());
        }
        self.buf = virtual_to_vec(&hold, data, 0, total);
        self.note();
        Ok(data.len())
    }

    fn drive_part(&mut self, data: &[u8], out: &mut Vec<u8>) -> Result<usize, ScrubError> {
        let mut consumed = 0;
        while consumed < data.len() && self.phase == Phase::Part {
            if self.buf.len() < self.marker.len() + 2 {
                let need = self.marker.len() + 2 - self.buf.len();
                let take = need.min(data.len() - consumed);
                if take == 0 {
                    break;
                }
                self.buf.extend_from_slice(&data[consumed..consumed + take]);
                self.note();
                consumed += take;
                if self.buf.len() < self.marker.len() + 2 {
                    break;
                }
                if self.diverge_non_header(out) {
                    break;
                }
                continue;
            }
            if self.try_delim(out)? {
                continue;
            }
            let avail = &data[consumed..];
            if let Some(blank_at) = find_across(&self.buf, avail, b"\r\n\r\n") {
                let blank_end = blank_at + 4;
                if blank_end > self.buf.len() {
                    let add = blank_end - self.buf.len();
                    if self.buf.len() + add > PART_HEADER_LIMIT {
                        return Err(ScrubError::HeaderTooLarge);
                    }
                    self.buf.extend_from_slice(&avail[..add]);
                    self.note();
                    consumed += add;
                } else if blank_end < self.buf.len() {
                    let rest = self.buf[blank_end..].to_vec();
                    self.buf.truncate(blank_end);
                    self.finish_headers(out)?;
                    self.lead = Some(rest);
                    break;
                }
                self.finish_headers(out)?;
                break;
            }
            let room = PART_HEADER_LIMIT.saturating_sub(self.buf.len());
            if room == 0 {
                if header_has_bare_break(self.header_bytes(), true) {
                    self.begin_skip();
                } else if !self.try_delim(out)? {
                    return Err(ScrubError::HeaderTooLarge);
                }
                break;
            }
            let take = room.min(avail.len());
            self.buf.extend_from_slice(&avail[..take]);
            self.note();
            consumed += take;
            if self.try_delim(out)? {
                continue;
            }
            if header_has_bare_break(self.header_bytes(), true) {
                self.begin_skip();
                break;
            }
            if self.buf.len() >= PART_HEADER_LIMIT {
                return Err(ScrubError::HeaderTooLarge);
            }
        }
        Ok(consumed)
    }

    fn drive_body(&mut self, data: &[u8], out: &mut Vec<u8>) -> Result<usize, ScrubError> {
        let hold = std::mem::take(&mut self.buf);
        if let Some(start) = find_across(&hold, data, &self.delim) {
            extend_virtual(out, &hold, data, 0, start + 2);
            let end = hold.len() + data.len();
            let marker_at = start + 2;
            let marker_end = (marker_at + self.marker.len()).min(end);
            self.buf = virtual_to_vec(&hold, data, marker_at, marker_end);
            self.note();
            self.phase = Phase::Part;
            let _ = end;
            return self.resume_after(&hold, data, marker_end);
        }
        let total = hold.len() + data.len();
        let keep = (self.delim.len() - 1).min(total);
        extend_virtual(out, &hold, data, 0, total - keep);
        self.buf = virtual_to_vec(&hold, data, total - keep, total);
        self.note();
        Ok(data.len())
    }

    fn drive_skip(&mut self, data: &[u8], out: &mut Vec<u8>) -> Result<usize, ScrubError> {
        let hold = std::mem::take(&mut self.buf);
        if let Some(start) = find_across(&hold, data, &self.delim) {
            let end = hold.len() + data.len();
            let marker_at = start + 2;
            let marker_end = (marker_at + self.marker.len()).min(end);
            self.buf = virtual_to_vec(&hold, data, marker_at, marker_end);
            self.note();
            self.phase = Phase::Part;
            let _ = (end, out);
            return self.resume_after(&hold, data, marker_end);
        }
        let total = hold.len() + data.len();
        let keep = (self.delim.len() - 1).min(total);
        self.buf = virtual_to_vec(&hold, data, total - keep, total);
        self.note();
        self.phase = Phase::Skip;
        let _ = out;
        Ok(data.len())
    }

    fn header_bytes(&self) -> &[u8] {
        let start = self.marker.len() + 2;
        if self.buf.len() <= start {
            &[]
        } else {
            &self.buf[start..]
        }
    }

    /// `--boundary--` (epilogue) or a boundary that is not a header line:
    /// copy the rest, as the buffered scanner did.
    fn diverge_non_header(&mut self, out: &mut Vec<u8>) -> bool {
        let marker_len = self.marker.len();
        let next = &self.buf[marker_len..marker_len + 2];
        if next == b"--" || next != b"\r\n" {
            out.extend_from_slice(&self.buf);
            self.buf.clear();
            self.phase = Phase::Copy;
            true
        } else {
            false
        }
    }

    fn finish_headers(&mut self, out: &mut Vec<u8>) -> Result<(), ScrubError> {
        let start = self.marker.len() + 2;
        let Some(blank) = find_subslice(&self.buf[start..], b"\r\n\r\n") else {
            return Err(ScrubError::HeaderTooLarge);
        };
        let broken = header_is_broken(&self.buf[start..start + blank]);
        if broken {
            self.buf.clear();
            self.phase = Phase::Skip;
        } else {
            out.extend_from_slice(&self.buf);
            self.buf.clear();
            self.phase = Phase::Body;
        }
        Ok(())
    }

    fn begin_skip(&mut self) {
        let keep = (self.delim.len() - 1).min(self.buf.len());
        if keep == 0 {
            self.buf.clear();
        } else {
            self.buf.drain(..self.buf.len() - keep);
        }
        self.note();
        self.phase = Phase::Skip;
    }

    /// A part with no blank line, ended by the next delimiter.
    fn try_delim(&mut self, out: &mut Vec<u8>) -> Result<bool, ScrubError> {
        let start = self.marker.len() + 2;
        if self.buf.len() < start {
            return Ok(false);
        }
        let Some(rel) = find_subslice(&self.buf[start..], &self.delim) else {
            return Ok(false);
        };
        let content = &self.buf[start..start + rel];
        let headers = match find_subslice(content, b"\r\n\r\n") {
            Some(i) => &content[..i],
            None => content,
        };
        let broken = header_is_broken(headers);
        let emit_until = start + rel + 2;
        let rest = self.buf[emit_until..].to_vec();
        if !broken {
            out.extend_from_slice(&self.buf[..emit_until]);
        }
        self.buf = rest;
        self.note();
        self.phase = Phase::Part;
        if self.buf.len() >= self.marker.len() + 2 {
            self.drain_buffered(out)?;
        }
        Ok(true)
    }

    fn drain_buffered(&mut self, out: &mut Vec<u8>) -> Result<(), ScrubError> {
        loop {
            if self.phase != Phase::Part || self.buf.len() < self.marker.len() + 2 {
                return Ok(());
            }
            if self.diverge_non_header(out) {
                return Ok(());
            }
            if self.try_delim(out)? {
                continue;
            }
            let start = self.marker.len() + 2;
            if find_subslice(&self.buf[start..], b"\r\n\r\n").is_some()
                && self.buf.ends_with(b"\r\n\r\n")
            {
                self.finish_headers(out)?;
                return Ok(());
            }
            return Ok(());
        }
    }
}

fn extend_virtual(out: &mut Vec<u8>, hold: &[u8], data: &[u8], from: usize, to: usize) {
    if from >= to {
        return;
    }
    if to <= hold.len() {
        out.extend_from_slice(&hold[from..to]);
        return;
    }
    if from >= hold.len() {
        let start = from - hold.len();
        let end = to - hold.len();
        out.extend_from_slice(&data[start..end]);
        return;
    }
    out.extend_from_slice(&hold[from..]);
    out.extend_from_slice(&data[..to - hold.len()]);
}

fn virtual_to_vec(hold: &[u8], data: &[u8], from: usize, to: usize) -> Vec<u8> {
    let mut out = Vec::new();
    extend_virtual(&mut out, hold, data, from, to);
    out
}

fn find_across(hold: &[u8], data: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let total = hold.len() + data.len();
    if total < needle.len() {
        return None;
    }
    let earliest = hold.len().saturating_sub(needle.len() - 1);
    for start in earliest..hold.len() {
        if start + needle.len() <= total && eq_at(hold, data, start, needle) {
            return Some(start);
        }
    }
    find_subslice(data, needle).map(|pos| hold.len() + pos)
}

fn eq_at(hold: &[u8], data: &[u8], start: usize, needle: &[u8]) -> bool {
    for (offset, expected) in needle.iter().enumerate() {
        let idx = start + offset;
        let found = if idx < hold.len() {
            hold[idx]
        } else {
            let data_idx = idx - hold.len();
            if data_idx >= data.len() {
                return false;
            }
            data[data_idx]
        };
        if found != *expected {
            return false;
        }
    }
    true
}

/// Drops parts whose headers contain a bare CR or LF. Kept as the oracle for
/// the streaming scrubber on bodies that fit in a test. A filename of
/// `line\n74.txt` or `line\r74.txt` is the case observed on the Rails stack
/// (2026-11-11).
#[cfg(test)]
fn drop_broken_header_parts(body: &[u8], boundary: &str) -> Vec<u8> {
    let marker = format!("--{boundary}");
    let marker = marker.as_bytes();
    let Some(first) = find_subslice(body, marker) else {
        return body.to_vec();
    };
    let mut pos = first;
    let mut out = Vec::with_capacity(body.len());
    loop {
        if pos + marker.len() > body.len() || &body[pos..pos + marker.len()] != marker {
            break;
        }
        let after = pos + marker.len();
        if body.get(after..after + 2) == Some(b"--") {
            out.extend_from_slice(&body[pos..]);
            break;
        }
        let Some(content_start) = body
            .get(after..after + 2)
            .and_then(|tail| (tail == b"\r\n").then_some(after + 2))
        else {
            out.extend_from_slice(&body[pos..]);
            break;
        };
        let mut delimiter = Vec::with_capacity(2 + marker.len());
        delimiter.extend_from_slice(b"\r\n");
        delimiter.extend_from_slice(marker);
        let Some(rel) = find_subslice(&body[content_start..], &delimiter) else {
            out.extend_from_slice(&body[pos..]);
            break;
        };
        let content_end = content_start + rel;
        let content = &body[content_start..content_end];
        let headers = match find_subslice(content, b"\r\n\r\n") {
            Some(i) => &content[..i],
            None => content,
        };
        if !header_is_broken(headers) {
            // Include the CRLF that introduces the next delimiter.
            out.extend_from_slice(&body[pos..content_end + 2]);
        }
        pos = content_end + 2;
    }
    out
}

fn header_is_broken(headers: &[u8]) -> bool {
    header_has_bare_break(headers, false)
}

/// `incomplete` is set while the blank line is not in yet: a CR at the very
/// end may be the first byte of a CRLF that the next chunk will finish.
fn header_has_bare_break(headers: &[u8], incomplete: bool) -> bool {
    let end = if incomplete && headers.last() == Some(&b'\r') {
        headers.len() - 1
    } else {
        headers.len()
    };
    headers[..end]
        .iter()
        .enumerate()
        .any(|(i, byte)| match byte {
            b'\n' => i == 0 || headers[i - 1] != b'\r',
            b'\r' => headers.get(i + 1) != Some(&b'\n'),
            _ => false,
        })
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len())
        .position(|window| window == needle)
}

/// Percent-encodes everything but unreserved characters, so that
/// [`RackParams::parse`] gets back exactly the bytes the part carried.
fn urlencode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use futures_util::Stream;

    fn headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(CONTENT_TYPE, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn the_boundary_is_read_like_rack() {
        assert_eq!(
            boundary(&headers("multipart/form-data; boundary=XX")).as_deref(),
            Some("XX")
        );
        assert_eq!(
            boundary(&headers("multipart/form-data; boundary=\"a b\"; x=1")).as_deref(),
            Some("a b")
        );
        assert_eq!(
            boundary(&headers("Multipart/Mixed;Boundary=zz,q")).as_deref(),
            Some("zz")
        );
        assert_eq!(boundary(&headers("multipart/form-data")), None);
        assert_eq!(boundary(&headers("multipart/form-data; boundary=")), None);
        assert_eq!(boundary(&headers("application/json; boundary=XX")), None);
        assert_eq!(boundary(&HeaderMap::new()), None);
    }

    fn body(boundary: &str, parts: &[(&str, Option<&str>, &str)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, filename, data) in parts {
            out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            match filename {
                Some(f) => out.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\nContent-Type: application/pdf\r\n\r\n").as_bytes(),
                ),
                None => out.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                ),
            }
            out.extend_from_slice(data.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        out
    }

    fn storage() -> (tempfile::TempDir, Storage) {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::new(dir.path().join("files"));
        (dir, storage)
    }

    fn files_under(root: &std::path::Path) -> usize {
        let mut count = 0;
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path)
                } else {
                    count += 1
                }
            }
        }
        count
    }

    #[tokio::test]
    async fn text_parts_become_parameters_and_the_file_is_stored() {
        let (_dir, storage) = storage();
        let data = body(
            "XX",
            &[
                ("department_id", None, "7"),
                ("tags[type]", None, "facture"),
                ("file", Some("a.pdf"), "PDF!"),
            ],
        );
        let intake = read("XX".into(), Body::from(data), &storage, 1 << 20)
            .await
            .unwrap();
        assert_eq!(intake.params.str("department_id"), Some("7"));
        assert!(intake.params.get("tags").is_some());
        let file = intake.file.unwrap();
        assert_eq!(file.original_filename, "a.pdf");
        assert_eq!(file.content_type.as_deref(), Some("application/pdf"));
        assert_eq!(file.stored.written.byte_size, 4);
        assert_eq!(files_under(storage.root()), 1);
    }

    #[tokio::test]
    async fn a_file_whose_header_has_a_bare_line_break_is_dropped_like_rack() {
        // Rack keeps parsing and ignores the part. Observed 2026-11-11:
        // `filename="line\n74.txt"` yields the text fields and no file, and a
        // text part after the broken file is kept.
        let (_dir, storage) = storage();
        let data = body(
            "XX",
            &[
                ("department_id", None, "3"),
                ("file", Some("line\n74.txt"), "%PDF"),
                ("after", None, "later"),
            ],
        );
        let intake = read("XX".into(), Body::from(data), &storage, 1 << 20)
            .await
            .unwrap();
        assert_eq!(intake.params.str("department_id"), Some("3"));
        assert_eq!(intake.params.str("after"), Some("later"));
        assert!(intake.file.is_none());
        assert_eq!(files_under(storage.root()), 0);

        let cr = body(
            "XX",
            &[
                ("department_id", None, "9"),
                ("file", Some("line\r74.txt"), "HELLO"),
            ],
        );
        let intake = read("XX".into(), Body::from(cr), &storage, 1 << 20)
            .await
            .unwrap();
        assert_eq!(intake.params.str("department_id"), Some("9"));
        assert!(intake.file.is_none());
    }

    #[tokio::test]
    async fn an_empty_file_input_is_dropped_and_other_file_names_ignored() {
        let (_dir, storage) = storage();
        let data = body(
            "XX",
            &[("file", Some(""), ""), ("other", Some("x.txt"), "zzz")],
        );
        let intake = read("XX".into(), Body::from(data), &storage, 1 << 20)
            .await
            .unwrap();
        assert!(intake.file.is_none());
        assert_eq!(files_under(storage.root()), 0);
    }

    #[tokio::test]
    async fn a_repeated_file_part_keeps_the_last_and_leaves_one_file() {
        let (_dir, storage) = storage();
        let data = body(
            "XX",
            &[
                ("file", Some("a.pdf"), "one"),
                ("file", Some("b.pdf"), "second"),
            ],
        );
        let intake = read("XX".into(), Body::from(data), &storage, 1 << 20)
            .await
            .unwrap();
        assert_eq!(intake.file.unwrap().original_filename, "b.pdf");
        assert_eq!(files_under(storage.root()), 1);
    }

    /// Review Focus 2: a body cut in the middle of the file part is a clean
    /// 400 and leaves no stored file.
    #[tokio::test]
    async fn a_body_cut_mid_file_is_malformed_and_leaves_nothing() {
        let (_dir, storage) = storage();
        let mut data = body(
            "XX",
            &[
                ("department_id", None, "7"),
                ("file", Some("a.pdf"), &"x".repeat(5000)),
            ],
        );
        data.truncate(data.len() - 2000);
        let intake = read("XX".into(), Body::from(data), &storage, 1 << 20).await;
        assert_eq!(intake.err(), Some(IntakeError::Malformed));
        assert_eq!(files_under(storage.root()), 0);
    }

    #[tokio::test]
    async fn a_connection_dropped_mid_body_leaves_nothing() {
        let (_dir, storage) = storage();
        let head = body("XX", &[("file", Some("a.pdf"), &"x".repeat(3000))]);
        let first = bytes::Bytes::from(head[..1500].to_vec());
        let stream = futures_util::stream::iter(vec![
            Ok::<_, std::io::Error>(first),
            Err(std::io::Error::other("connection reset")),
        ]);
        let intake = read("XX".into(), Body::from_stream(stream), &storage, 1 << 20).await;
        assert_eq!(intake.err(), Some(IntakeError::Malformed));
        assert_eq!(files_under(storage.root()), 0);
    }

    /// Review Focus 2: over the limit is 413 and nothing stays on disk —
    /// the limit is on the whole body, exactly at it is accepted.
    #[tokio::test]
    async fn a_body_over_the_limit_is_too_large_and_exactly_at_it_is_accepted() {
        let (_dir, storage) = storage();
        let data = body(
            "XX",
            &[
                ("department_id", None, "7"),
                ("file", Some("a.pdf"), &"x".repeat(4000)),
            ],
        );
        let at_limit = data.len() as u64;
        let ok = read("XX".into(), Body::from(data.clone()), &storage, at_limit).await;
        assert!(ok.is_ok(), "{:?}", ok.err());
        let before = files_under(storage.root());
        let over = read("XX".into(), Body::from(data), &storage, at_limit - 1).await;
        assert_eq!(over.err(), Some(IntakeError::TooLarge));
        assert_eq!(files_under(storage.root()), before);
    }

    #[tokio::test]
    async fn invalid_utf8_in_a_text_part_and_too_many_parts_are_malformed() {
        let (_dir, storage) = storage();
        let mut raw = b"--XX\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\n".to_vec();
        raw.extend_from_slice(&[0xff, 0xfe]);
        raw.extend_from_slice(b"\r\n--XX--\r\n");
        let bad = read("XX".into(), Body::from(raw), &storage, 1 << 20).await;
        assert_eq!(bad.err(), Some(IntakeError::Malformed));
    }

    /// Rack's file limit counts filenames only. 129 text parts (the fuzzer's
    /// `multipart-edge`) parse; the controller then reports a missing
    /// `department_id`. Observed on the Rails stack, 2026-11-11.
    #[tokio::test]
    async fn text_parts_past_the_file_limit_are_kept() {
        let (_dir, storage) = storage();
        let many: Vec<(String, Option<&str>, &str)> =
            (0..129).map(|i| (format!("p{i}"), None, "v")).collect();
        let borrowed: Vec<(&str, Option<&str>, &str)> =
            many.iter().map(|(n, f, d)| (n.as_str(), *f, *d)).collect();
        let intake = read(
            "XX".into(),
            Body::from(body("XX", &borrowed)),
            &storage,
            1 << 20,
        )
        .await
        .expect("129 text parts are under the total limit");
        assert_eq!(intake.params.str("p0"), Some("v"));
        assert_eq!(intake.params.str("p128"), Some("v"));
        assert!(intake.file.is_none());
        assert_eq!(files_under(storage.root()), 0);
    }

    /// The 128th part that carries a filename is Rack's
    /// `MultipartPartLimitError`. Earlier file parts are not kept.
    #[tokio::test]
    async fn the_128th_file_part_is_over_the_file_limit() {
        let (_ok_dir, ok_storage) = storage();
        let names: Vec<String> = (0..127).map(|i| format!("f{i}.txt")).collect();
        let parts: Vec<(&str, Option<&str>, &str)> = names
            .iter()
            .map(|name| ("file", Some(name.as_str()), "x"))
            .collect();
        let ok = read(
            "XX".into(),
            Body::from(body("XX", &parts)),
            &ok_storage,
            1 << 20,
        )
        .await
        .expect("127 file parts are accepted");
        assert_eq!(ok.file.unwrap().original_filename, "f126.txt");
        assert_eq!(files_under(ok_storage.root()), 1);

        let (_over_dir, over_storage) = storage();
        let names: Vec<String> = (0..128).map(|i| format!("f{i}.txt")).collect();
        let parts: Vec<(&str, Option<&str>, &str)> = names
            .iter()
            .map(|name| ("file", Some(name.as_str()), "x"))
            .collect();
        let over = read(
            "XX".into(),
            Body::from(body("XX", &parts)),
            &over_storage,
            1 << 20,
        )
        .await;
        assert_eq!(over.err(), Some(IntakeError::PartLimit));
        assert_eq!(files_under(over_storage.root()), 0);
    }

    /// A blank `filename=""` still opens a Rack tempfile, so it counts.
    #[tokio::test]
    async fn a_blank_filename_counts_toward_the_file_limit() {
        let (_dir, storage) = storage();
        let parts: Vec<(&str, Option<&str>, &str)> =
            (0..128).map(|_| ("file", Some(""), "")).collect();
        let over = read(
            "XX".into(),
            Body::from(body("XX", &parts)),
            &storage,
            1 << 20,
        )
        .await;
        assert_eq!(over.err(), Some(IntakeError::PartLimit));
        assert_eq!(files_under(storage.root()), 0);
    }

    /// `multipart_total_part_limit` is 4096: 4095 parts parse, the 4096th is refused.
    #[tokio::test]
    async fn the_4096th_part_is_over_the_total_limit() {
        let (_dir, storage) = storage();
        let names: Vec<String> = (0..4095).map(|i| format!("p{i}")).collect();
        let parts: Vec<(&str, Option<&str>, &str)> = names
            .iter()
            .map(|name| (name.as_str(), None, "v"))
            .collect();
        let ok = read(
            "XX".into(),
            Body::from(body("XX", &parts)),
            &storage,
            8 << 20,
        )
        .await
        .expect("4095 text parts are accepted");
        assert_eq!(ok.params.str("p0"), Some("v"));
        assert_eq!(ok.params.str("p4094"), Some("v"));

        let names: Vec<String> = (0..4096).map(|i| format!("p{i}")).collect();
        let parts: Vec<(&str, Option<&str>, &str)> = names
            .iter()
            .map(|name| (name.as_str(), None, "v"))
            .collect();
        let over = read(
            "XX".into(),
            Body::from(body("XX", &parts)),
            &storage,
            8 << 20,
        )
        .await;
        assert_eq!(over.err(), Some(IntakeError::PartLimit));
    }

    #[tokio::test]
    async fn a_broken_first_part_is_dropped_and_the_file_after_it_is_kept() {
        let (_dir, storage) = storage();
        let data = body(
            "XX",
            &[
                ("note", Some("bad\rname.txt"), "nope"),
                ("department_id", None, "4"),
                ("file", Some("ok.pdf"), "PDF!"),
            ],
        );
        let intake = read("XX".into(), Body::from(data), &storage, 1 << 20)
            .await
            .unwrap();
        assert_eq!(intake.params.str("department_id"), Some("4"));
        assert_eq!(intake.file.unwrap().original_filename, "ok.pdf");
        assert_eq!(files_under(storage.root()), 1);
    }

    #[tokio::test]
    async fn a_broken_last_part_is_dropped_and_the_text_before_it_stays() {
        let (_dir, storage) = storage();
        let data = body(
            "XX",
            &[
                ("department_id", None, "5"),
                ("file", Some("line\r74.txt"), "PDF!"),
            ],
        );
        let intake = read("XX".into(), Body::from(data), &storage, 1 << 20)
            .await
            .unwrap();
        assert_eq!(intake.params.str("department_id"), Some("5"));
        assert!(intake.file.is_none());
        assert_eq!(files_under(storage.root()), 0);
    }

    #[tokio::test]
    async fn a_truncated_broken_header_is_malformed_and_leaves_nothing() {
        let (_dir, storage) = storage();
        let data = b"--XX\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a\nb\"\r\n\r\nHELLO".to_vec();
        let intake = read("XX".into(), Body::from(data), &storage, 1 << 20).await;
        assert_eq!(intake.err(), Some(IntakeError::Malformed));
        assert_eq!(files_under(storage.root()), 0);
    }

    #[tokio::test]
    async fn headers_past_the_bound_are_malformed() {
        let (_dir, storage) = storage();
        let mut data = b"--XX\r\nContent-Disposition: form-data; name=\"".to_vec();
        data.extend(std::iter::repeat_n(b'A', PART_HEADER_LIMIT));
        data.extend_from_slice(b"\"\r\n\r\nZ\r\n--XX--\r\n");
        let intake = read("XX".into(), Body::from(data), &storage, 8 << 20).await;
        assert_eq!(intake.err(), Some(IntakeError::Malformed));
        assert_eq!(files_under(storage.root()), 0);
    }

    #[test]
    fn the_scrubber_matches_the_buffered_oracle_and_does_not_keep_the_body() {
        let samples = [
            body(
                "XX",
                &[
                    ("department_id", None, "7"),
                    ("file", Some("a.pdf"), "PDF!"),
                ],
            ),
            body(
                "XX",
                &[
                    ("department_id", None, "3"),
                    ("file", Some("line\n74.txt"), "%PDF"),
                    ("after", None, "later"),
                ],
            ),
            body(
                "XX",
                &[
                    ("file", Some("line\r74.txt"), "HELLO"),
                    ("department_id", None, "9"),
                ],
            ),
            body("XX", &[("only", None, "text")]),
            {
                let mut raw = b"preamble".to_vec();
                raw.extend(body("XX", &[("a", None, "b")]));
                raw
            },
            b"not multipart at all".to_vec(),
            {
                // The file bytes contain the next delimiter. Both scanners cut
                // there; the oracle is the buffered function.
                let mut raw = body("XX", &[("file", Some("a.pdf"), "AB")]);
                let at = raw.windows(2).position(|w| w == b"AB").unwrap();
                raw.splice(at..at + 2, b"\r\n--XXcut".iter().copied());
                raw
            },
        ];
        for sample in &samples {
            let expected = drop_broken_header_parts(sample, "XX");
            for chunk in [1usize, 2, 3, 7, 64, sample.len().max(1)] {
                let got = scrub_all("XX", sample, chunk).expect("complete body");
                assert_eq!(got, expected, "chunk {chunk}");
            }
        }

        let mut huge = Vec::new();
        huge.extend_from_slice(
            b"--XX\r\nContent-Disposition: form-data; name=\"department_id\"\r\n\r\n1\r\n",
        );
        huge.extend_from_slice(
            b"--XX\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.pdf\"\r\nContent-Type: application/pdf\r\n\r\n",
        );
        huge.extend(std::iter::repeat_n(b'x', 1024 * 1024));
        huge.extend_from_slice(b"\r\n--XX--\r\n");
        let expected = drop_broken_header_parts(&huge, "XX");
        let (got, peak) = scrub_with_peak("XX", &huge, 8 * 1024).expect("good file");
        assert_eq!(got, expected);
        assert!(
            peak <= 4096,
            "scrubber kept {peak} bytes of a {} byte body",
            huge.len()
        );

        let mut broken =
            b"--XX\r\nContent-Disposition: form-data; name=\"department_id\"\r\n\r\n1\r\n".to_vec();
        broken.extend_from_slice(
            b"--XX\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a\nb.pdf\"\r\nContent-Type: application/pdf\r\n\r\n",
        );
        broken.extend(std::iter::repeat_n(b'y', 1024 * 1024));
        broken.extend_from_slice(
            b"\r\n--XX\r\nContent-Disposition: form-data; name=\"after\"\r\n\r\nlater\r\n--XX--\r\n",
        );
        let expected = drop_broken_header_parts(&broken, "XX");
        let (got, peak) = scrub_with_peak("XX", &broken, 8 * 1024).expect("broken part");
        assert_eq!(got, expected);
        assert!(expected.len() < broken.len());
        assert!(
            peak <= 4096,
            "scrubber kept {peak} bytes while dropping a {} byte part",
            broken.len()
        );
    }

    fn scrub_all(boundary: &str, data: &[u8], chunk: usize) -> Result<Vec<u8>, ScrubError> {
        scrub_with_peak(boundary, data, chunk).map(|(out, _)| out)
    }

    fn scrub_with_peak(
        boundary: &str,
        data: &[u8],
        chunk: usize,
    ) -> Result<(Vec<u8>, usize), ScrubError> {
        let mut scrubber = Scrubber::new(boundary);
        let mut out = Vec::new();
        for piece in data.chunks(chunk.max(1)) {
            out.extend(scrubber.push(piece)?);
        }
        out.extend(scrubber.finish()?);
        Ok((out, scrubber.peak()))
    }

    #[test]
    fn the_first_chunk_of_a_large_file_is_forwarded_at_once() {
        let header = b"--XX\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.pdf\"\r\nContent-Type: application/pdf\r\n\r\n";
        let mut chunk = header.to_vec();
        chunk.extend(std::iter::repeat_n(b'x', 64 * 1024));
        let mut scrubber = Scrubber::new("XX");
        let out = scrubber.push(&chunk).expect("push");
        assert!(
            out.len() > 60_000,
            "emitted {} bytes, peak {}",
            out.len(),
            scrubber.peak()
        );
        assert!(scrubber.peak() <= 4096, "peak {}", scrubber.peak());
    }

    /// Multer fills its buffer until the stream returns `Pending`. A socket
    /// does that between chunks; an always-ready stream is drained whole
    /// before the first field is parsed, which hides a streaming `read`.
    /// The tail stays pending until a file exists, and a second poll with
    /// no file panics — that is what the old whole-body buffer did.
    #[tokio::test]
    async fn a_large_file_part_is_stored_before_the_rest_of_the_body_is_read() {
        let (_dir, storage) = storage();
        let header = b"--XX\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.pdf\"\r\nContent-Type: application/pdf\r\n\r\n";
        let mut first = header.to_vec();
        first.extend(std::iter::repeat_n(b'x', 64 * 1024));
        let mut second = vec![b'y'; 64 * 1024];
        second.extend_from_slice(b"\r\n--XX--\r\n");
        let stream = TwoChunks {
            root: storage.root().to_path_buf(),
            first: Some(bytes::Bytes::from(first)),
            second: Some(bytes::Bytes::from(second)),
            stalled: false,
        };
        let intake = read("XX".into(), Body::from_stream(stream), &storage, 1 << 20)
            .await
            .expect("large file part");
        let file = intake.file.expect("file");
        assert_eq!(file.original_filename, "a.pdf");
        assert_eq!(file.stored.written.byte_size, 128 * 1024);
        assert_eq!(files_under(storage.root()), 1);
    }

    /// The old `read` polled until the stream ended, then parsed. That second
    /// poll of the tail happens before `Storage` creates a file.
    #[test]
    fn draining_the_body_before_storage_is_rejected() {
        let dir = std::env::temp_dir().join(format!("ged-two-chunks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let mut stream = std::pin::pin!(TwoChunks {
            root: dir.clone(),
            first: Some(bytes::Bytes::from_static(b"head")),
            second: Some(bytes::Bytes::from_static(b"tail")),
            stalled: false,
        });
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        assert!(matches!(
            stream.as_mut().poll_next(&mut cx),
            std::task::Poll::Ready(Some(Ok(_)))
        ));
        assert!(matches!(
            stream.as_mut().poll_next(&mut cx),
            std::task::Poll::Pending
        ));
        let polled_again = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = stream.as_mut().poll_next(&mut cx);
        }));
        assert!(
            polled_again.is_err(),
            "a second poll with no file must fail"
        );

        std::fs::write(dir.join("x.part"), b"").expect("part");
        let mut stream = std::pin::pin!(TwoChunks {
            root: dir.clone(),
            first: None,
            second: Some(bytes::Bytes::from_static(b"tail")),
            stalled: true,
        });
        match stream.as_mut().poll_next(&mut cx) {
            std::task::Poll::Ready(Some(Ok(bytes))) => assert_eq!(&bytes[..], b"tail"),
            other => panic!("expected the tail once a file exists, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Review M1: a text part is read chunk by chunk and abandoned once the
    /// running total passes `FORM_BYTES_LIMIT`; the rest of a 45 MiB part is
    /// never pulled from the connection (nor held in memory).
    #[tokio::test]
    async fn an_oversized_text_part_stops_the_read_at_the_form_limit() {
        use futures_util::StreamExt;
        let (_dir, storage) = storage();
        let pulled = Arc::new(AtomicU64::new(0));
        let counter = pulled.clone();
        let head = bytes::Bytes::from_static(
            b"--XX\r\nContent-Disposition: form-data; name=\"tags[x]\"\r\n\r\n",
        );
        let chunk = bytes::Bytes::from(vec![b'a'; 64 * 1024]);
        let chunks = 45 * 16; // 45 MiB of text, no closing boundary
        let stream = futures_util::stream::iter(
            std::iter::once(head).chain((0..chunks).map(move |_| chunk.clone())),
        )
        .map(move |b| {
            counter.fetch_add(b.len() as u64, Ordering::SeqCst);
            Ok::<_, std::io::Error>(b)
        });
        let result = read(
            "XX".into(),
            Body::from_stream(stream),
            &storage,
            50 * 1024 * 1024,
        )
        .await;
        assert_eq!(result.err(), Some(IntakeError::Malformed));
        let pulled = pulled.load(Ordering::SeqCst) as usize;
        assert!(
            pulled <= FORM_BYTES_LIMIT + 1024 * 1024,
            "pulled {pulled} bytes for a part capped at {FORM_BYTES_LIMIT}"
        );
    }

    /// Final review m12: a header sent one byte per chunk reaches the
    /// scrubber in 4 KiB pushes, not 65,000 (each push rescans the header).
    #[tokio::test]
    async fn a_header_sent_byte_by_byte_costs_few_scrubber_pushes() {
        use futures_util::StreamExt;
        let mut input = b"--XX\r\nContent-Disposition: form-data; name=\"a\"\r\nX-Pad: ".to_vec();
        input.resize(PART_HEADER_LIMIT - 100, b'a');
        let total = input.len();
        let source = futures_util::stream::iter(
            input
                .into_iter()
                .map(|b| Ok::<_, std::io::Error>(Bytes::from(vec![b]))),
        );
        let mut stream = ScrubStream {
            gather: Vec::new(),
            inner: Box::pin(source),
            scrubber: Scrubber::new("XX"),
            pending: VecDeque::new(),
            inner_done: false,
            finished: false,
            burst: 0,
        };
        let mut forwarded = 0;
        let mut ended_with_error = false;
        while let Some(item) = stream.next().await {
            match item {
                Ok(bytes) => forwarded += bytes.len(),
                Err(_) => ended_with_error = true,
            }
        }
        // The header never ends: the stream fails, but every byte it saw was
        // accounted for and the scrubber was driven only a few times.
        assert!(ended_with_error || forwarded == total);
        let pushes = stream.scrubber.pushes;
        assert!(
            pushes <= total / GATHER_BYTES + 2,
            "{pushes} pushes for {total} one-byte chunks"
        );
    }

    struct TwoChunks {
        root: std::path::PathBuf,
        first: Option<bytes::Bytes>,
        second: Option<bytes::Bytes>,
        /// Set after the one `Pending` that splits the chunks.
        stalled: bool,
    }

    impl futures_util::Stream for TwoChunks {
        type Item = Result<bytes::Bytes, std::io::Error>;

        fn poll_next(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<Self::Item>> {
            if self.first.is_some() {
                return std::task::Poll::Ready(self.first.take().map(Ok));
            }
            if self.second.is_some() {
                if files_under(&self.root) == 0 {
                    assert!(
                        !self.stalled,
                        "the second chunk was pulled before any byte reached storage"
                    );
                    self.stalled = true;
                    return std::task::Poll::Pending;
                }
                return std::task::Poll::Ready(self.second.take().map(Ok));
            }
            std::task::Poll::Ready(None)
        }
    }
}
