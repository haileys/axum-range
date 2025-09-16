# axum-range

HTTP range responses for [`axum`][1].

[Documentation][2].

MIT license.

### Example usage

```rust
use axum_extra::TypedHeader;
use axum_extra::headers::Range;

use tokio::fs::File;
use std::io::Cursor;
use bytes::Bytes;

use axum_range::Ranged;
use axum_range::KnownSize;

async fn file(range: Option<TypedHeader<Range>>) -> Ranged<KnownSize<File>> {
    let file = File::open("archlinux-x86_64.iso").await.unwrap();
    let body = KnownSize::file(file).await.unwrap();
    let range = range.map(|TypedHeader(range)| range);
    Ranged::new(range, body)
}

async fn bytes(range: Option<TypedHeader<Range>>) -> Ranged<KnownSize<Cursor<Bytes>>> {
    let buffer = tokio::fs::read("cat.jpg").await.unwrap();
    // processing data here...
    
    let body = KnownSize::bytes(buffer);
    let range = range.map(|TypedHeader(range)| range);
    Ranged::new(range, body)
}
```

[1]: https://docs.rs/axum
[2]: https://docs.rs/axum-range
