//! A small library for `multipart/form-data` requests.
//!
//! # Examples
//!
//! ```ignore
//! use hyper::body::Body;
//! use http::{header, Request};
//! use send_multipart::{Multipart, Part, Error};
//!
//! let message = Multipart::from_parts([
//!     Part::text("my_text_field", "field_value"),
//!     Part::bytes("my_file_field", b"<root></root>".as_slice())
//!         .with_filename("message.xml")
//!         .with_mime(mime::TEXT_XML)
//! ]);
//! let request = Request::post("https://my_service.xyz")
//!     .header(header::CONTENT_TYPE, message.get_content_type_header()?)
//!     .body(Body::from(message.into_bytes()))
//!     .unwrap();
//! ```

use std::{
    borrow::Cow,
    hash::{BuildHasher, Hasher},
};

#[cfg(not(feature = "http1"))]
use http0 as http;

#[cfg(feature = "http1")]
use http1 as http;

#[cfg(feature = "stream")]
use futures::{stream::BoxStream, Stream, StreamExt, TryStream, TryStreamExt};

use bytes::{Bytes, BytesMut};
use http::{
    header::{IntoHeaderName, InvalidHeaderValue},
    HeaderMap, HeaderValue,
};
use mime::Mime;

/// This type represents all possible errors that can occour when using multipart message.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    InvalidHeaderValue(InvalidHeaderValue),
    #[cfg(feature = "stream")]
    #[error(transparent)]
    StreamError(Box<dyn std::error::Error + Send + Sync>),
}

/// A `multipart/form-data` request.
pub struct Multipart {
    /// Boundary delimiter of each part.
    boundary: String,
    /// Parts contained in this message.
    parts: Vec<Part>,
}

impl Multipart {
    /// Create a new empty message.
    pub fn empty() -> Self {
        Self {
            boundary: generate_random_boundary(),
            parts: vec![],
        }
    }

    /// Create a new message collecting the given parts.
    pub fn from_parts<I>(parts: I) -> Self
    where
        I: IntoIterator<Item = Part>,
    {
        let mut this = Self::empty();
        this.parts.extend(parts);
        this
    }

    /// Add a new part to the message.
    pub fn add_part(mut self, part: Part) -> Self {
        self.parts.push(part);
        self
    }

    /// Set the boundary used by this message.
    pub fn with_boundary<T>(mut self, boundary: T) -> Self
    where
        T: Into<String>,
    {
        self.boundary = boundary.into();
        self
    }

    /// Get the `content-type` header value.
    pub fn get_content_type_header(&self) -> Result<HeaderValue, Error> {
        format!("multipart/form-data; boundary={}", self.boundary)
            .parse()
            .map_err(Error::InvalidHeaderValue)
    }

    #[cfg(not(feature = "stream"))]
    /// Convert the message in a buffer.
    pub fn into_bytes(self) -> Bytes {
        let mut buffer = BytesMut::new();

        for part in self.parts {
            part.into_bytes(&self.boundary, &mut buffer);
        }

        buffer.extend_from_slice(b"--");
        buffer.extend_from_slice(self.boundary.as_bytes());
        buffer.extend_from_slice(b"--\r\n");

        buffer.freeze()
    }

    #[cfg(feature = "stream")]
    /// Create a stream with the content of the body.
    pub fn into_stream(self) -> impl Stream<Item = Result<Bytes, Error>> {
        use futures::{
            future::ready,
            stream::{empty, once},
        };

        let boundary = self.boundary;
        let parts = self
            .parts
            .into_iter()
            .map(|part| part.into_stream(&boundary))
            .fold(empty().boxed(), |init, last| init.chain(last).boxed());

        let mut buffer = BytesMut::new();
        buffer.extend_from_slice(b"--");
        buffer.extend_from_slice(boundary.as_bytes());
        buffer.extend_from_slice(b"--\r\n");
        let tail = buffer.freeze();

        parts.chain(once(ready(Ok(tail))))
    }

    #[cfg(feature = "stream")]
    /// Convert the message in a buffer.
    pub async fn into_bytes(self) -> Result<Bytes, Error> {
        use futures::future::ready;

        let buffer = BytesMut::new();
        self.into_stream()
            .try_fold(buffer, |mut buffer, bytes| {
                buffer.extend_from_slice(&bytes);
                ready(Ok(buffer))
            })
            .await
            .map(BytesMut::freeze)
    }
}

/// A part of which a multipart request is composed.
pub struct Part {
    /// Field name.
    name: Cow<'static, str>,
    /// Optional file name.
    filename: Option<Cow<'static, str>>,
    /// Mime type.
    mime: Option<Mime>,
    /// The content of this part.
    content: Content,
    /// Additionals headers.
    headers: HeaderMap,
}

/// The content of each part.
enum Content {
    /// Content stored in a buffer.
    Buffer(Bytes),
    #[cfg(feature = "stream")]
    /// Stream content
    Stream(BoxStream<'static, Result<Bytes, Error>>),
}

impl Part {
    /// Create a new empty part with the given name.
    pub fn empty<T>(name: T) -> Self
    where
        T: Into<Cow<'static, str>>,
    {
        Self {
            name: name.into(),
            filename: None,
            mime: None,
            content: Content::Buffer(Bytes::new()),
            headers: HeaderMap::new(),
        }
    }

    /// Create a new part with text content (`text/plain; charset=utf-8`).
    pub fn text<T, U>(name: T, value: U) -> Self
    where
        T: Into<Cow<'static, str>>,
        U: Into<Cow<'static, str>>,
    {
        let bytes = match value.into() {
            Cow::Owned(string) => Bytes::from(string),
            Cow::Borrowed(string) => Bytes::from_static(string.as_bytes()),
        };

        Self {
            name: name.into(),
            filename: None,
            mime: Some(mime::TEXT_PLAIN_UTF_8),
            content: Content::Buffer(bytes),
            headers: HeaderMap::new(),
        }
    }

    /// Create a new part with binary content (`application/octet-stream`).
    pub fn bytes<T, U>(name: T, value: U) -> Self
    where
        T: Into<Cow<'static, str>>,
        U: Into<Cow<'static, [u8]>>,
    {
        let bytes = match value.into() {
            Cow::Owned(bytes) => Bytes::from(bytes),
            Cow::Borrowed(bytes) => Bytes::from_static(bytes),
        };

        Self {
            name: name.into(),
            filename: None,
            mime: Some(mime::APPLICATION_OCTET_STREAM),
            content: Content::Buffer(bytes),
            headers: HeaderMap::new(),
        }
    }

    #[cfg(feature = "stream")]
    /// Create a new part with stream content (`application/octet-stream`).
    pub fn stream<T, U>(name: T, value: U) -> Self
    where
        T: Into<Cow<'static, str>>,
        U: TryStream + Send + 'static,
        Bytes: From<U::Ok>,
        U::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let stream = value
            .map_ok(Bytes::from)
            .map_err(|err| Error::StreamError(err.into()))
            .boxed();

        Self {
            name: name.into(),
            filename: None,
            mime: Some(mime::APPLICATION_OCTET_STREAM),
            content: Content::Stream(stream),
            headers: HeaderMap::new(),
        }
    }

    /// Set the mime of this part.
    pub fn with_mime(mut self, mime: Mime) -> Self {
        self.mime = Some(mime);
        self
    }

    /// Set the filename of this part.
    pub fn with_filename<T>(mut self, filename: T) -> Self
    where
        T: Into<Cow<'static, str>>,
    {
        self.filename = Some(filename.into());
        self
    }

    /// Set the custom headers of this part.
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    /// Add an additional custom header.
    pub fn add_header<K, V>(mut self, name: K, value: V) -> Result<Self, Error>
    where
        K: IntoHeaderName,
        V: TryInto<HeaderValue, Error = http::header::InvalidHeaderValue>,
    {
        let value = value.try_into().map_err(Error::InvalidHeaderValue)?;
        self.headers.insert(name, value);
        Ok(self)
    }

    /// Write in the buffer the headers of this part.
    fn headers_into_bytes(&self, boundary: &str, buffer: &mut BytesMut) {
        buffer.extend_from_slice(b"--");
        buffer.extend_from_slice(boundary.as_bytes());
        buffer.extend_from_slice(b"\r\n");

        buffer.extend_from_slice(b"Content-Disposition: form-data; ");
        buffer.extend_from_slice(b"name=\"");
        buffer.extend_from_slice(self.name.as_bytes());
        buffer.extend_from_slice(b"\"");
        if let Some(filename) = self.filename.as_deref() {
            buffer.extend_from_slice(b"; filename=\"");
            buffer.extend_from_slice(filename.as_bytes());
            buffer.extend_from_slice(b"\"");
        }
        buffer.extend_from_slice(b"\r\n");

        if let Some(mime) = self.mime.as_ref() {
            buffer.extend_from_slice(b"Content-Type: ");
            buffer.extend_from_slice(mime.as_ref().as_bytes());
            buffer.extend_from_slice(b"\r\n");
        }

        for (header_name, header_value) in self.headers.iter() {
            buffer.extend_from_slice(header_name.as_str().as_bytes());
            buffer.extend_from_slice(b": ");
            buffer.extend_from_slice(header_value.as_bytes());
            buffer.extend_from_slice(b"\r\n");
        }

        buffer.extend_from_slice(b"\r\n");
    }

    #[cfg(not(feature = "stream"))]
    /// Convert this part in a buffer.
    fn into_bytes(self, boundary: &str, buffer: &mut BytesMut) {
        self.headers_into_bytes(boundary, buffer);
        match self.content {
            Content::Buffer(bytes) => buffer.extend_from_slice(&bytes),
        }
        buffer.extend_from_slice(b"\r\n");
    }

    #[cfg(feature = "stream")]
    fn into_stream(self, boundary: &str) -> impl Stream<Item = Result<Bytes, Error>> {
        use futures::{future::ready, stream::once};

        let mut buffer = BytesMut::new();
        self.headers_into_bytes(boundary, &mut buffer);
        match self.content {
            Content::Buffer(bytes) => {
                buffer.extend_from_slice(&bytes);
                buffer.extend_from_slice(b"\r\n");

                let buffer = buffer.freeze();
                futures::stream::once(ready(Ok(buffer))).boxed()
            }
            Content::Stream(stream) => {
                let buffer = buffer.freeze();
                let tail = Bytes::from_static(b"\r\n");
                once(ready(Ok(buffer)))
                    .chain(stream)
                    .chain(once(ready(Ok(tail))))
                    .boxed()
            }
        }
    }
}

/// Generate a new random integer (xorshift64).
fn xorshit64() -> u64 {
    use std::cell::Cell;

    fn seed() -> u64 {
        use std::hash::RandomState;
        let state = RandomState::new();
        for count in 0.. {
            let mut hasher = state.build_hasher();
            hasher.write_usize(count);
            let seed = hasher.finish();
            if seed != 0 {
                return seed;
            }
        }
        unreachable!("failed to generate a random seed");
    }

    thread_local! {
        static STATE: Cell<u64> = Cell::new(seed());
    }

    STATE.with(|state| {
        let mut n = state.get();
        n ^= n << 13;
        n ^= n >> 7;
        n ^= n << 17;
        state.set(n);
        n
    })
}

fn generate_random_boundary() -> String {
    let a = xorshit64();
    let b = xorshit64();
    let c = xorshit64();
    let d = xorshit64();
    format!("{a:016x}{b:016x}{c:016x}{d:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    use claym::*;

    fn create_multipart_parser(message: Multipart) -> multer::Multipart<'static> {
        let content_type = assert_ok!(message.get_content_type_header());
        let content_type = assert_ok!(content_type.to_str());
        let boundary = assert_ok!(multer::parse_boundary(content_type));

        #[cfg(feature = "stream")]
        let parser = {
            let body = message.into_stream();
            multer::Multipart::new(body, boundary)
        };

        #[cfg(not(feature = "stream"))]
        let parser = {
            use futures::{future::ready, stream::once};

            let body = message.into_bytes();
            let body = once(ready(Ok::<_, Error>(body)));
            multer::Multipart::new(body, boundary)
        };

        parser
    }

    #[tokio::test]
    async fn test_multipart_empty() {
        let message = Multipart::empty();
        let mut parser = create_multipart_parser(message);
        assert_matches!(assert_ok!(parser.next_field().await), None);
    }

    #[tokio::test]
    async fn test_multipart_basic_with_stream() {
        let message = Multipart::from_parts([
            Part::text("my_text_field", "abcd"),
            Part::bytes(
                "my_file_field",
                b"Hello world\nHello\r\nWorld\rAgain".as_slice(),
            )
            .with_mime(mime::TEXT_PLAIN)
            .with_filename("a-text-file.txt"),
        ]);
        let mut parser = create_multipart_parser(message);

        while let Some((idx, field)) = parser.next_field_with_idx().await.unwrap() {
            if idx == 0 {
                assert_eq!(field.name(), Some("my_text_field"));
                assert_eq!(field.file_name(), None);
                assert_eq!(field.content_type(), Some(&mime::TEXT_PLAIN_UTF_8));
                assert_eq!(field.index(), 0);

                assert_eq!(field.text().await, Ok("abcd".to_owned()));
            } else if idx == 1 {
                assert_eq!(field.name(), Some("my_file_field"));
                assert_eq!(field.file_name(), Some("a-text-file.txt"));
                assert_eq!(field.content_type(), Some(&mime::TEXT_PLAIN));
                assert_eq!(field.index(), 1);

                assert_eq!(
                    field.text().await,
                    Ok("Hello world\nHello\r\nWorld\rAgain".to_owned())
                );
            }
        }
    }
}
