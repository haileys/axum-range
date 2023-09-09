mod file;
mod stream;

use std::io;
use std::ops::Bound;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::TypedHeader;
use axum::http::StatusCode;
use axum::headers::{Range, ContentRange, ContentLength};
use axum::response::{IntoResponse, Response};
use tokio::io::{AsyncRead, AsyncSeek};

pub use file::KnownSize;
pub use stream::RangedStream;

pub const IO_BUFFER_SIZE: usize = 64 * 1024;

pub trait AsyncSeekStart {
    fn start_seek(self: Pin<&mut Self>, position: u64) -> io::Result<()> ;
    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>>;
}

impl<T: AsyncSeek> AsyncSeekStart for T {
    fn start_seek(self: Pin<&mut Self>, position: u64) -> io::Result<()> {
        AsyncSeek::start_seek(self, io::SeekFrom::Start(position))
    }

    fn poll_complete(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncSeek::poll_complete(self, cx).map_ok(|_| ())
    }
}

pub trait RangeBody: AsyncRead + AsyncSeekStart {
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
        // we could use >= above but I think this reads more clearly:
        let zero_length_range = seek_start == seek_end_excl;

        if seek_start_beyond_seek_end || seek_end_beyond_file_range || zero_length_range {
            let content_range = ContentRange::unsatisfied_bytes(total_bytes);
            return Err(RangeNotSatisfiable(content_range));
        }

        // if we're good, build the response
        let content_range = range.map(|_| {
            ContentRange::bytes(seek_start..seek_end_excl, total_bytes)
                .expect("ContentRange::bytes cannot panic in this usage")
        });

        let content_length = ContentLength(seek_end_excl - seek_start);

        let stream = RangedStream::new(self.body, seek_start, content_length.0);

        Ok(RangedResponse {
            content_range,
            content_length,
            stream,
        })
    }
}

impl<B: RangeBody> IntoResponse for Ranged<B> {
    fn into_response(self) -> Response {
        self.try_respond().into_response()
    }
}

#[derive(Debug, Clone)]
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

#[cfg(test)]
mod tests {
    use std::io;

    use axum::headers::ContentRange;
    use axum::headers::Header;
    use axum::headers::Range;
    use axum::http::HeaderValue;
    use bytes::Bytes;
    use futures::{pin_mut, Stream, StreamExt};
    use tokio::fs::File;

    use crate::Ranged;
    use crate::KnownSize;

    async fn collect_stream(stream: impl Stream<Item = io::Result<Bytes>>) -> String {
        let mut string = String::new();
        pin_mut!(stream);
        while let Some(chunk) = stream.next().await.transpose().unwrap() {
            string += std::str::from_utf8(&chunk).unwrap();
        }
        string
    }

    fn range(header: &str) -> Option<Range> {
        let val = HeaderValue::from_str(header).unwrap();
        Some(Range::decode(&mut [val].iter()).unwrap())
    }

    async fn body() -> KnownSize<File> {
        let file = File::open("test/fixture.txt").await.unwrap();
        KnownSize::file(file).await.unwrap()
    }

    #[tokio::test]
    async fn test_full_response() {
        let ranged = Ranged::new(None, body().await);

        let response = ranged.try_respond().expect("try_respond should return Ok");

        assert_eq!(54, response.content_length.0);
        assert!(response.content_range.is_none());
        assert_eq!("Hello world this is a file to test range requests on!\n",
            &collect_stream(response.stream).await);
    }

    #[tokio::test]
    async fn test_partial_response_1() {
        let ranged = Ranged::new(range("bytes=0-29"), body().await);

        let response = ranged.try_respond().expect("try_respond should return Ok");

        assert_eq!(30, response.content_length.0);

        let expected_content_range = ContentRange::bytes(0..30, 54).unwrap();
        assert_eq!(Some(expected_content_range), response.content_range);

        assert_eq!("Hello world this is a file to ",
            &collect_stream(response.stream).await);
    }

    #[tokio::test]
    async fn test_partial_response_2() {
        let ranged = Ranged::new(range("bytes=30-53"), body().await);

        let response = ranged.try_respond().expect("try_respond should return Ok");

        assert_eq!(24, response.content_length.0);

        let expected_content_range = ContentRange::bytes(30..54, 54).unwrap();
        assert_eq!(Some(expected_content_range), response.content_range);

        assert_eq!("test range requests on!\n",
            &collect_stream(response.stream).await);
    }

    #[tokio::test]
    async fn test_unbounded_start_response() {
        let ranged = Ranged::new(range("bytes=-20"), body().await);

        let response = ranged.try_respond().expect("try_respond should return Ok");

        assert_eq!(21, response.content_length.0);

        let expected_content_range = ContentRange::bytes(0..21, 54).unwrap();
        assert_eq!(Some(expected_content_range), response.content_range);

        assert_eq!("Hello world this is a",
            &collect_stream(response.stream).await);
    }

    #[tokio::test]
    async fn test_unbounded_end_response() {
        let ranged = Ranged::new(range("bytes=40-"), body().await);

        let response = ranged.try_respond().expect("try_respond should return Ok");

        assert_eq!(14, response.content_length.0);

        let expected_content_range = ContentRange::bytes(40..54, 54).unwrap();
        assert_eq!(Some(expected_content_range), response.content_range);

        assert_eq!(" requests on!\n",
            &collect_stream(response.stream).await);
    }

    #[tokio::test]
    async fn test_one_byte_response() {
        let ranged = Ranged::new(range("bytes=30-30"), body().await);

        let response = ranged.try_respond().expect("try_respond should return Ok");

        assert_eq!(1, response.content_length.0);

        let expected_content_range = ContentRange::bytes(30..31, 54).unwrap();
        assert_eq!(Some(expected_content_range), response.content_range);

        assert_eq!("t",
            &collect_stream(response.stream).await);
    }

    #[tokio::test]
    async fn test_invalid_range() {
        let ranged = Ranged::new(range("bytes=30-29"), body().await);

        let err = ranged.try_respond().err().expect("try_respond should return Err");

        let expected_content_range = ContentRange::unsatisfied_bytes(54);
        assert_eq!(expected_content_range, err.0)
    }

    #[tokio::test]
    async fn test_range_end_exceed_length() {
        let ranged = Ranged::new(range("bytes=30-99"), body().await);

        let err = ranged.try_respond().err().expect("try_respond should return Err");

        let expected_content_range = ContentRange::unsatisfied_bytes(54);
        assert_eq!(expected_content_range, err.0)
    }

    #[tokio::test]
    async fn test_range_start_exceed_length() {
        let ranged = Ranged::new(range("bytes=99-"), body().await);

        let err = ranged.try_respond().err().expect("try_respond should return Err");

        let expected_content_range = ContentRange::unsatisfied_bytes(54);
        assert_eq!(expected_content_range, err.0)
    }
}
