use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use pin_project::pin_project;
use tokio::io::{ReadBuf, AsyncRead, AsyncSeek};

use crate::RangeBody;

#[pin_project]
pub struct KnownSize<B: AsyncRead + AsyncSeek> {
    byte_size: u64,
    #[pin]
    body: B,
}

impl<B: AsyncRead + AsyncSeek> KnownSize<B> {
    pub async fn file(file: tokio::fs::File) -> io::Result<KnownSize<tokio::fs::File>> {
        let byte_size = file.metadata().await?.len();
        Ok(KnownSize { byte_size, body: file })
    }
}

impl<B: AsyncRead + AsyncSeek> AsyncRead for KnownSize<B> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.project();
        this.body.poll_read(cx, buf)
    }
}

impl<B: AsyncRead + AsyncSeek> AsyncSeek for KnownSize<B> {
    fn start_seek(
        self: Pin<&mut Self>,
        position: io::SeekFrom
    ) -> io::Result<()> {
        let this = self.project();
        this.body.start_seek(position)
    }

    fn poll_complete(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<u64>> {
        let this = self.project();
        this.body.poll_complete(cx)
    }
}

impl<B: AsyncRead + AsyncSeek> RangeBody for KnownSize<B> {
    fn byte_size(&self) -> u64 {
        self.byte_size
    }
}
