use std::{convert::Infallible, future::ready};

use claym::*;
use futures::stream::once;
use send_multipart::*;

#[tokio::test]
async fn test_multipart_empty() {
    let message = Multipart::empty();
    let boundary = parse_boundary(&message);

    let body = message.into_bytes();
    let body = once(ready(Ok::<_, Infallible>(body)));
    let mut parser = multer::Multipart::new(body, boundary);

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
    let boundary = parse_boundary(&message);

    let body = message.into_bytes();
    let body = once(ready(Ok::<_, Infallible>(body)));
    let mut parser = multer::Multipart::new(body, boundary);

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

fn parse_boundary(message: &Multipart) -> String {
    let content_type = assert_ok!(message.get_content_type_header());
    let content_type = assert_ok!(content_type.to_str());
    assert_ok!(multer::parse_boundary(content_type))
}
