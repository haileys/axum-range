pub mod file;

use std::{io, mem};
use std::ops::Bound;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::TypedHeader;
use axum::http::StatusCode;
use axum::headers::{Range, ContentRange, ContentLength};
use axum::response::{IntoResponse, Response};
use bytes::{Bytes, BytesMut};
use futures::Stream;
use pin_project::pin_project;
use tokio::io::{ReadBuf, AsyncRead, AsyncSeek};

pub const IO_BUFFER_SIZE: usize = 64 * 1024;

pub trait RangeBody: AsyncRead + AsyncSeek {
    /// The total size of the underlying file.
    ///
    /// This should not change for the lifetime of the object once queried.
    /// Behaviour is not guaranteed if it does change.
    fn byte_size(&self) -> u64;
}

pub struct Ranged<B: RangeBody> {
    range: Option<Range>,
    body: B,
}

impl<B: RangeBody> Ranged<B> {
    pub fn new(range: Option<Range>, body: B) -> Self {
        Ranged { range, body }
    }

    pub fn try_respond(self) -> Result<RangedResponse<B>, RangeNotSatisfiable> {
        let total_bytes = self.body.byte_size();

        // we don't support multiple byte ranges, only none or one
        // fortunately, only responding with one of the requested ranges and
        // no more seems to be compliant with the HTTP spec.
        let range = self.range.and_then(|header| header.iter().nth(0));

        // pull seek positions out of range header
        let seek_start = match range {
            Some((Bound::Included(seek_start), _)) => seek_start,
            _ => 0,
        };

        let seek_end_excl = match range {
            // HTTP byte ranges are inclusive, so we translate to exclusive by adding 1:
            Some((_, Bound::Included(end))) => end + 1,
            _ => total_bytes,
        };

        // check seek positions and return with 416 Range Not Satisfiable if invalid
        let seek_start_beyond_seek_end = seek_start > seek_end_excl;
        let seek_end_beyond_file_range = seek_end_excl > total_bytes;

        if seek_start_beyond_seek_end || seek_end_beyond_file_range {
            let content_range = ContentRange::unsatisfied_bytes(total_bytes);
            return Err(RangeNotSatisfiable(content_range));
        }

        // if we're good, build the response
        let content_range = range.map(|_| {
            ContentRange::bytes(seek_start..seek_end_excl, total_bytes)
                .expect("ContentRange::bytes cannot panic in this usage")
        });

        let stream = RangedStream {
            state: StreamState::Seek { start: seek_start, remaining: seek_end_excl - seek_start },
            body: self.body,
        };

        Ok(RangedResponse {
            content_range,
            content_length: ContentLength(total_bytes),
            stream,
        })
    }
}

impl<B: RangeBody> IntoResponse for Ranged<B> {
    fn into_response(self) -> Response {
        self.try_respond().into_response()
    }
}

pub struct RangeNotSatisfiable(pub ContentRange);

impl IntoResponse for RangeNotSatisfiable {
    fn into_response(self) -> Response {
        let status = StatusCode::RANGE_NOT_SATISFIABLE;
        let header = TypedHeader(self.0);
        (status, header, ()).into_response()
    }
}

pub struct RangedResponse<B> {
    pub content_range: Option<ContentRange>,
    pub content_length: ContentLength,
    pub stream: RangedStream<B>,
}

impl<B: RangeBody> IntoResponse for RangedResponse<B> {
    fn into_response(self) -> Response {
        todo!();
    }
}

#[pin_project]
pub struct RangedStream<B> {
    state: StreamState,
    #[pin]
    body: B,
}

enum StreamState {
    Seek { start: u64, remaining: u64 },
    Seeking { remaining: u64 },
    Reading { buffer: BytesMut, remaining: u64 },
}

fn allocate_buffer() -> BytesMut {
    BytesMut::with_capacity(IO_BUFFER_SIZE)
}

impl<B: RangeBody> Stream for RangedStream<B> {
    type Item = io::Result<Bytes>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>
    ) -> Poll<Option<io::Result<Bytes>>> {
        let mut this = self.project();

        if let StreamState::Seek { start, remaining } = *this.state {
            match this.body.as_mut().start_seek(io::SeekFrom::Start(start)) {
                Err(e) => { return Poll::Ready(Some(Err(e))); }
                Ok(()) => { *this.state = StreamState::Seeking { remaining }; }
            }
        }

        if let StreamState::Seeking { remaining } = *this.state {
            match this.body.as_mut().poll_complete(cx) {
                Poll::Pending => { return Poll::Pending; }
                Poll::Ready(Err(e)) => { return Poll::Ready(Some(Err(e))); }
                Poll::Ready(Ok(_)) => {
                    let buffer = allocate_buffer();
                    *this.state = StreamState::Reading { buffer, remaining };
                }
            }
        }

        if let StreamState::Reading { buffer, remaining } = this.state {
            let uninit = buffer.spare_capacity_mut();

            // calculate max number of bytes to read in this iteration, the
            // smaller of the buffer size and the number of bytes remaining
            let nbytes = std::cmp::min(
                uninit.len(),
                usize::try_from(*remaining).unwrap_or(usize::MAX),
            );

            let mut read_buf = ReadBuf::uninit(&mut uninit[0..nbytes]);

            match this.body.as_mut().poll_read(cx, &mut read_buf) {
                Poll::Pending => { return Poll::Pending; }
                Poll::Ready(Err(e)) => { return Poll::Ready(Some(Err(e))); }
                Poll::Ready(Ok(())) => {
                    match read_buf.filled().len() {
                        0 => { return Poll::Ready(None); }
                        n => {
                            // replace state buffer and take this one to return
                            let chunk = mem::replace(buffer, allocate_buffer());
                            // subtract the number of bytes we just read from
                            // state.remaining, this usize->u64 conversion is
                            // guaranteed to always succeed, because n cannot be
                            // larger than remaining due to the cmp::min above
                            *remaining -= u64::try_from(n).unwrap();
                            // return this chunk
                            return Poll::Ready(Some(Ok(chunk.freeze())));
                        }
                    }
                }
            }
        }

        unreachable!();
    }
}
