// SPDX-License-Identifier: Apache-2.0
//! Explicit opaque byte copies for role processes which consume owned payloads.

use std::io::Read;
use up_rust::{UCode, UFrameView, UStatus};

/// Copies the complete ordered payload, including segmented storage.
///
/// This is an explicit owned copy, not a native typed borrow or codec operation.
/// The reader is bounded by the declared length plus one byte; inconsistent view
/// lengths fail rather than silently substituting an empty or truncated payload.
pub fn copy_payload_bytes(frame: &impl UFrameView) -> Result<Vec<u8>, UStatus> {
    let length = frame.payload_len();
    let limit = u64::try_from(length)
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| invalid("payload length cannot be bounded"))?;
    let mut bytes = Vec::new();
    frame
        .payload_reader()
        .take(limit)
        .read_to_end(&mut bytes)
        .map_err(|error| invalid(format!("cannot copy payload view: {error}")))?;
    if bytes.len() != length {
        return Err(invalid(
            "payload reader length differs from the declared view length",
        ));
    }
    Ok(bytes)
}

fn invalid(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::InvalidArgument, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Chain, Cursor};
    use test_case::test_case;
    use up_rust::{PayloadEncoding, UFrameMetadata, UUri};

    struct Segmented {
        metadata: UFrameMetadata,
        first: Vec<u8>,
        second: Vec<u8>,
        declared: usize,
    }

    impl UFrameView for Segmented {
        type PayloadReader<'a> = Chain<Cursor<&'a [u8]>, Cursor<&'a [u8]>>;
        type PayloadSlices<'a> = std::array::IntoIter<&'a [u8], 2>;
        fn metadata(&self) -> &UFrameMetadata {
            &self.metadata
        }
        fn payload_len(&self) -> usize {
            self.declared
        }
        fn payload_reader(&self) -> Self::PayloadReader<'_> {
            Cursor::new(self.first.as_slice()).chain(Cursor::new(self.second.as_slice()))
        }
        fn payload_slices(&self) -> Self::PayloadSlices<'_> {
            [self.first.as_slice(), self.second.as_slice()].into_iter()
        }
    }

    #[test_case(b"abc".as_slice(), b"def".as_slice(), 6, true; "both noncontiguous segments retained")]
    #[test_case(b"".as_slice(), b"".as_slice(), 0, true; "present empty payload")]
    #[test_case(b"abc".as_slice(), b"def".as_slice(), 5, false; "oversized reader rejects")]
    #[test_case(b"abc".as_slice(), b"def".as_slice(), 7, false; "truncated reader rejects")]
    fn complete_copy_preserves_segments_or_rejects_inconsistent_length(
        first: &[u8],
        second: &[u8],
        declared: usize,
        accepted: bool,
    ) {
        let source = UUri::try_from_parts("segments", 0x5BA0, 1, 0x8001).unwrap();
        let view = Segmented {
            metadata: UFrameMetadata::publish(source)
                .with_payload_encoding(PayloadEncoding::RAW)
                .build()
                .unwrap(),
            first: first.to_vec(),
            second: second.to_vec(),
            declared,
        };
        assert!(view.try_contiguous_payload().is_none());
        let result = copy_payload_bytes(&view);
        assert_eq!(result.is_ok(), accepted);
        if accepted {
            assert_eq!(result.unwrap(), [first, second].concat());
        }
    }
}
