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

    /// Convert the message in a buffer.
    pub fn into_bytes(self) -> Bytes {
        let mut buffer = BytesMut::new();

        for part in self.parts {
            buffer.extend_from_slice(b"--");
            buffer.extend_from_slice(self.boundary.as_bytes());
            buffer.extend_from_slice(b"\r\n");

            buffer.extend_from_slice(b"Content-Disposition: form-data; ");
            buffer.extend_from_slice(b"name=\"");
            buffer.extend_from_slice(part.name.as_bytes());
            buffer.extend_from_slice(b"\"");
            if let Some(filename) = part.filename {
                buffer.extend_from_slice(b"; filename=\"");
                buffer.extend_from_slice(filename.as_bytes());
                buffer.extend_from_slice(b"\"");
            }
            buffer.extend_from_slice(b"\r\n");

            if let Some(mime) = part.mime {
                buffer.extend_from_slice(b"Content-Type: ");
                buffer.extend_from_slice(mime.as_ref().as_bytes());
                buffer.extend_from_slice(b"\r\n");
            }

            for (header_name, header_value) in part.headers.iter() {
                buffer.extend_from_slice(header_name.as_str().as_bytes());
                buffer.extend_from_slice(b": ");
                buffer.extend_from_slice(header_value.as_bytes());
                buffer.extend_from_slice(b"\r\n");
            }

            buffer.extend_from_slice(b"\r\n");

            match part.content {
                Content::Buffer(bytes) => buffer.extend_from_slice(&bytes),
            }
            buffer.extend_from_slice(b"\r\n");
        }

        buffer.extend_from_slice(b"--");
        buffer.extend_from_slice(self.boundary.as_bytes());
        buffer.extend_from_slice(b"--\r\n");

        buffer.freeze()
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

    /// Create a new part with binary content (`application/octet-stream`)
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
