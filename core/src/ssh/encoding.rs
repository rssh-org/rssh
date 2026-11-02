use std::borrow::Cow;

use encoding_rs::{CoderResult, Decoder, GBK};
use serde_json::json;

use crate::error::{AppError, AppResult};
use crate::models::TerminalEncoding;

/// The GUI/headless terminal contract is UTF-8. Only the remote interactive
/// shell uses the profile encoding; native connection logs stay UTF-8.
pub(super) fn encode_input(encoding: TerminalEncoding, data: &[u8]) -> AppResult<Cow<'_, [u8]>> {
    if encoding == TerminalEncoding::Utf8 {
        return Ok(Cow::Borrowed(data));
    }
    let text = std::str::from_utf8(data).map_err(|_| {
        AppError::config(
            "terminal_encoding_invalid_utf8",
            json!({ "encoding": encoding.as_str() }),
        )
    })?;
    let (encoded, _, unmappable) = GBK.encode(text);
    // encoding_rs substitutes HTML numeric references for unrepresentable
    // characters. Never send that fallback: it changes shell command syntax.
    if unmappable {
        return Err(AppError::config(
            "terminal_encoding_unmappable",
            json!({ "encoding": encoding.as_str() }),
        ));
    }
    Ok(encoded)
}

pub(super) struct OutputDecoder {
    decoder: Option<Decoder>,
}

impl OutputDecoder {
    pub(super) fn new(encoding: TerminalEncoding) -> Self {
        Self {
            decoder: match encoding {
                TerminalEncoding::Utf8 => None,
                TerminalEncoding::Gbk => Some(GBK.new_decoder_without_bom_handling()),
            },
        }
    }

    /// Keep decoder state for a single SSH stream and flush it once at EOF.
    /// UTF-8 remains byte-for-byte unchanged, including partial characters.
    pub(super) fn decode<'a>(&mut self, data: &'a [u8], last: bool) -> Cow<'a, [u8]> {
        let Some(decoder) = self.decoder.as_mut() else {
            return Cow::Borrowed(data);
        };
        let capacity = decoder
            .max_utf8_buffer_length(data.len())
            .expect("SSH chunks fit in a UTF-8 buffer");
        let mut output = String::with_capacity(capacity);
        let (result, read, _) = decoder.decode_to_string(data, &mut output, last);
        debug_assert_eq!(result, CoderResult::InputEmpty);
        debug_assert_eq!(read, data.len());
        Cow::Owned(output.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_preserves_raw_bytes_and_chunk_boundaries() {
        let mut decoder = OutputDecoder::new(TerminalEncoding::Utf8);
        for bytes in [&b"\x1b[31m\xe4\xb8"[..], &b"\xad\xff\x1b[0m"[..]] {
            assert_eq!(decoder.decode(bytes, false).as_ref(), bytes);
            assert_eq!(
                encode_input(TerminalEncoding::Utf8, bytes)
                    .unwrap()
                    .as_ref(),
                bytes
            );
        }
        assert!(decoder.decode(&[], true).is_empty());
    }

    #[test]
    fn gbk_stream_preserves_text_and_terminal_controls_at_every_split() {
        let input = b"\x1b[31m\xd6\xd0\xce\xc4\x1b[0m\r\n\x1b]2;\xd6\xd0\xce\xc4\x07";
        let expected = "\x1b[31m中文\x1b[0m\r\n\x1b]2;中文\x07";
        for split in 0..=input.len() {
            let mut decoder = OutputDecoder::new(TerminalEncoding::Gbk);
            let mut output = decoder.decode(&input[..split], false).into_owned();
            output.extend_from_slice(&decoder.decode(&input[split..], true));
            assert_eq!(output, expected.as_bytes(), "split at {split}");
        }
    }

    #[test]
    fn gbk_stream_keeps_partial_character_until_close() {
        let mut decoder = OutputDecoder::new(TerminalEncoding::Gbk);
        assert_eq!(decoder.decode(b"text\xd6", false).as_ref(), b"text");
        assert_eq!(decoder.decode(&[], true).as_ref(), "\u{fffd}".as_bytes());
    }

    #[test]
    fn gbk_stream_does_not_switch_encoding_for_a_utf8_bom() {
        let mut decoder = OutputDecoder::new(TerminalEncoding::Gbk);
        assert_ne!(decoder.decode(b"\xef\xbb\xbfabc", true).as_ref(), b"abc");
    }

    #[test]
    fn gbk_stdout_and_stderr_keep_independent_partial_characters() {
        let mut stdout = OutputDecoder::new(TerminalEncoding::Gbk);
        let mut stderr = OutputDecoder::new(TerminalEncoding::Gbk);
        assert!(stdout.decode(b"\xd6", false).is_empty());
        assert!(stderr.decode(b"\xce", false).is_empty());
        assert_eq!(stdout.decode(b"\xd0", true).as_ref(), "中".as_bytes());
        assert_eq!(stderr.decode(b"\xc4", true).as_ref(), "文".as_bytes());
    }

    #[test]
    fn gbk_input_encodes_chinese_without_changing_control_sequences() {
        let bytes = encode_input(
            TerminalEncoding::Gbk,
            "\x1b[200~echo 中文\r\x1b[201~\x03".as_bytes(),
        )
        .unwrap();
        assert_eq!(
            bytes.as_ref(),
            b"\x1b[200~echo \xd6\xd0\xce\xc4\r\x1b[201~\x03"
        );
    }

    #[test]
    fn gbk_input_rejects_unmappable_characters_instead_of_mutating_commands() {
        let error =
            encode_input(TerminalEncoding::Gbk, "echo 😀; touch file".as_bytes()).unwrap_err();
        assert_eq!(error.code(), "terminal_encoding_unmappable");
    }

    #[test]
    fn gbk_input_rejects_invalid_utf8() {
        let error = encode_input(TerminalEncoding::Gbk, &[0xff]).unwrap_err();
        assert_eq!(error.code(), "terminal_encoding_invalid_utf8");
    }
}
