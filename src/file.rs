use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use pin_project::pin_project;
use tokio::io::{ReadBuf, AsyncRead, AsyncSeek, AsyncSeekExt};

use crate::{RangeBody, AsyncSeekStart};

/// Implements [`RangeBody`] for any [`AsyncRead`] and [`AsyncSeekStart`], constructed with a fixed byte size.
#[pin_project]
pub struct KnownSize<B: AsyncRead + AsyncSeekStart> {
    byte_size: u64,
    #[pin]
    body: B,
}

impl KnownSize<tokio::fs::File> {
    /// Calls [`tokio::fs::File::metadata`] to determine file size.
    pub async fn file(file: tokio::fs::File) -> io::Result<KnownSize<tokio::fs::File>> {
        let byte_size = file.metadata().await?.len();
        Ok(KnownSize { byte_size, body: file })
    }
}

impl<B: AsyncRead + AsyncSeekStart> KnownSize<B> {
    /// Construct a [`KnownSize`] instance with a byte size supplied manually.
    pub fn sized(body: B, byte_size: u64) -> Self {
        KnownSize { byte_size, body }
    }
}

impl<B: AsyncRead + AsyncSeek + Unpin> KnownSize<B> {
    /// Uses `seek` to determine size by seeking to the end and getting stream position.
    pub async fn seek(mut body: B) -> io::Result<KnownSize<B>> {
        let byte_size = Pin::new(&mut body).seek(io::SeekFrom::End(0)).await?;
        Ok(KnownSize { byte_size, body })
    }
}

impl<B: AsyncRead + AsyncSeekStart> AsyncRead for KnownSize<B> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.project();
        this.body.poll_read(cx, buf)
    }
}

impl<B: AsyncRead + AsyncSeekStart> AsyncSeekStart for KnownSize<B> {
    fn start_seek(
        self: Pin<&mut Self>,
        position: u64,
    ) -> io::Result<()> {
        let this = self.project();
        this.body.start_seek(position)
    }

    fn poll_complete(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.project();
        this.body.poll_complete(cx)
    }
}

impl<B: AsyncRead + AsyncSeekStart> RangeBody for KnownSize<B> {
    fn byte_size(&self) -> u64 {
        self.byte_size
    }
}

#[cfg(test)]
mod tests {
    use tokio::fs::File;
    use crate::RangeBody;

    use super::KnownSize;

    #[tokio::test]
    async fn test_file_size() {
        let file = File::open("test/fixture.txt").await.unwrap();
        let known_size = KnownSize::file(file).await.unwrap();
        assert_eq!(54, known_size.byte_size());
    }

    #[tokio::test]
    async fn test_seek_size() {
        let file = File::open("test/fixture.txt").await.unwrap();
        let known_size = KnownSize::seek(file).await.unwrap();
        assert_eq!(54, known_size.byte_size());
    }
}
