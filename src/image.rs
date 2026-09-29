use std::ops::Range;

use crate::error::Error;
use crate::protocol::{
    MrcMode, PSS_GET_BUFFER_IMAGE, PSS_PUT_IMAGE_IN_BUFFER, PYTHONMODULE_CHUNK_SIZE,
    PYTHONMODULE_SUPER_CHUNK_SIZE, decode_fixed_or_error_frame, encode_frame,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferIndex(u8);

impl BufferIndex {
    pub fn new(index: u8) -> Result<Self, Error> {
        if index < 20 {
            Ok(Self(index))
        } else {
            Err(Error::InvalidArgument(format!(
                "buffer index {index} is outside A-T"
            )))
        }
    }

    pub fn from_letter(letter: char) -> Result<Self, Error> {
        if !letter.is_ascii_uppercase() {
            return Err(Error::InvalidArgument(format!(
                "buffer letter '{letter}' must be uppercase"
            )));
        }
        Self::new((letter as u8) - b'A')
    }

    pub fn index(self) -> i32 {
        self.0 as i32
    }

    pub fn validate_for_fft(self, if_fft: bool) -> Result<Self, Error> {
        if if_fft && self.0 >= 8 {
            return Err(Error::InvalidArgument(format!(
                "buffer {} is outside FFT buffers A-H",
                self.letter()
            )));
        }
        Ok(self)
    }

    pub fn letter(self) -> char {
        (b'A' + self.0) as char
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferSelector {
    pub buffer: BufferIndex,
    pub fft: bool,
}

impl BufferSelector {
    pub fn parse(specification: &str) -> Result<Self, Error> {
        let chars: Vec<char> = specification.chars().collect();
        match chars.as_slice() {
            [letter] => Ok(Self {
                buffer: BufferIndex::from_letter(*letter)?,
                fft: false,
            }),
            [letter, 'F'] if letter.is_ascii_uppercase() && *letter <= 'H' => Ok(Self {
                buffer: BufferIndex::from_letter(*letter)?,
                fft: true,
            }),
            _ => Err(Error::InvalidArgument(format!(
                "'{specification}' is not a valid SerialEM buffer specification"
            ))),
        }
    }
}

/// Defensive upper bound for server-declared raw image chunks. This limits
/// handshake amplification without requiring the pinned server's chunk size.
pub const MAX_IMAGE_CHUNKS: usize = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferImageMeta {
    pub mode: MrcMode,
    pub row_bytes: usize,
    pub size_x: usize,
    pub size_y: usize,
    pub byte_count: usize,
    pub chunks: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferImage {
    pub meta: BufferImageMeta,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PutImageOptions {
    pub mode: MrcMode,
    pub size_x: usize,
    pub size_y: usize,
    pub to_buffer: BufferIndex,
    pub base_buffer: BufferIndex,
    pub more_binning: i32,
    pub capture_flag: i32,
}

impl Default for PutImageOptions {
    fn default() -> Self {
        Self::new(MrcMode::Byte, 0, 0)
    }
}

impl PutImageOptions {
    /// Builder-compatible constructor for callers that want PythonModule-like
    /// defaults for buffer and capture parameters while specifying dimensions.
    pub fn new(mode: MrcMode, size_x: usize, size_y: usize) -> Self {
        Self {
            mode,
            size_x,
            size_y,
            to_buffer: BufferIndex(0),
            base_buffer: BufferIndex(0),
            more_binning: 1,
            capture_flag: -1,
        }
    }

    pub fn with_to_buffer(mut self, buffer: BufferIndex) -> Self {
        self.to_buffer = buffer;
        self
    }

    pub fn with_base_buffer(mut self, buffer: BufferIndex) -> Self {
        self.base_buffer = buffer;
        self
    }

    pub fn with_more_binning(mut self, more_binning: i32) -> Self {
        self.more_binning = more_binning;
        self
    }

    pub fn with_capture_flag(mut self, capture_flag: i32) -> Self {
        self.capture_flag = capture_flag;
        self
    }
}

impl BufferImage {
    pub fn item_size(&self) -> usize {
        self.meta.mode.item_size()
    }

    pub fn format(&self) -> &'static str {
        self.meta.mode.format()
    }

    pub fn is_contiguous(&self) -> bool {
        self.meta
            .size_x
            .checked_mul(self.item_size())
            .is_some_and(|row_bytes| self.meta.row_bytes == row_bytes)
    }

    pub fn row(&self, row: usize) -> Option<&[u8]> {
        if row >= self.meta.size_y {
            return None;
        }
        let start = row.checked_mul(self.meta.row_bytes)?;
        let end = start.checked_add(self.meta.row_bytes)?;
        self.bytes.get(start..end)
    }
}

pub fn encode_get_buffer_image(buffer: BufferIndex, if_fft: bool) -> Result<Vec<u8>, Error> {
    buffer.validate_for_fft(if_fft)?;
    encode_frame(
        &[PSS_GET_BUFFER_IMAGE, buffer.index(), i32::from(if_fft)],
        &[],
        &[],
        &[],
    )
}

pub fn decode_buffer_image_meta(frame: &[u8]) -> Result<BufferImageMeta, Error> {
    let decoded = decode_fixed_or_error_frame(frame, 7, 0, 0)?;
    if decoded.longs[0] < 0 {
        return Err(Error::from_server_code(decoded.longs[0]));
    }
    let mode = MrcMode::from_wire(decoded.longs[1])?;
    let row_bytes = positive_usize(decoded.longs[2], "row bytes")?;
    let size_x = positive_usize(decoded.longs[3], "image width")?;
    let size_y = positive_usize(decoded.longs[4], "image height")?;
    let byte_count = positive_usize(decoded.longs[5], "image byte count")?;
    let chunks = positive_usize(decoded.longs[6], "image chunk count")?;
    Ok(BufferImageMeta {
        mode,
        row_bytes,
        size_x,
        size_y,
        byte_count,
        chunks,
    })
}

pub fn validate_buffer_image_meta(
    meta: &BufferImageMeta,
    max_image_bytes: usize,
) -> Result<(), Error> {
    if meta.byte_count > max_image_bytes {
        return Err(Error::ResourceLimit(format!(
            "image declares {} bytes, configured maximum is {max_image_bytes}",
            meta.byte_count
        )));
    }
    let minimum_row_bytes = meta
        .size_x
        .checked_mul(meta.mode.item_size())
        .ok_or_else(|| Error::InvalidFrame("image row byte count overflow".to_string()))?;
    if meta.row_bytes < minimum_row_bytes {
        return Err(Error::InvalidFrame(format!(
            "image row bytes {} is smaller than the packed row size {minimum_row_bytes}",
            meta.row_bytes
        )));
    }
    let expected_bytes = meta
        .row_bytes
        .checked_mul(meta.size_y)
        .ok_or_else(|| Error::InvalidFrame("image byte count overflow".to_string()))?;
    if expected_bytes != meta.byte_count {
        return Err(Error::InvalidFrame(format!(
            "image metadata says {} bytes but row_bytes * height is {expected_bytes}",
            meta.byte_count
        )));
    }
    if meta.chunks == 0 || meta.chunks > meta.byte_count || meta.chunks > MAX_IMAGE_CHUNKS {
        return Err(Error::InvalidFrame(format!(
            "image chunk count {} is invalid for {} bytes (maximum {})",
            meta.chunks, meta.byte_count, MAX_IMAGE_CHUNKS
        )));
    }
    Ok(())
}

pub fn encode_put_image_request(
    options: PutImageOptions,
    byte_count: usize,
    chunks: usize,
) -> Result<Vec<u8>, Error> {
    if options.size_x == 0 || options.size_y == 0 {
        return Err(Error::InvalidArgument(
            "image width and height must be positive".to_string(),
        ));
    }
    if byte_count == 0 || chunks == 0 {
        return Err(Error::InvalidArgument(
            "image transfer must contain bytes and at least one chunk".to_string(),
        ));
    }
    if chunks > byte_count {
        return Err(Error::InvalidArgument(format!(
            "image chunk count {chunks} exceeds byte count {byte_count}"
        )));
    }
    let size_x = i32::try_from(options.size_x)
        .map_err(|_| Error::InvalidArgument("image width exceeds int32".to_string()))?;
    let size_y = i32::try_from(options.size_y)
        .map_err(|_| Error::InvalidArgument("image height exceeds int32".to_string()))?;
    let byte_count = i32::try_from(byte_count)
        .map_err(|_| Error::InvalidArgument("image byte count exceeds int32".to_string()))?;
    let chunks = i32::try_from(chunks)
        .map_err(|_| Error::InvalidArgument("image chunk count exceeds int32".to_string()))?;
    encode_frame(
        &[
            PSS_PUT_IMAGE_IN_BUFFER,
            options.mode.as_wire(),
            size_x,
            size_y,
            byte_count,
            options.to_buffer.index(),
            options.base_buffer.index(),
            options.more_binning,
            options.capture_flag,
            chunks,
        ],
        &[],
        &[],
        &[],
    )
}

pub fn decode_put_image_ack(frame: &[u8]) -> Result<(), Error> {
    let decoded = decode_fixed_or_error_frame(frame, 1, 0, 0)?;
    if decoded.longs[0] < 0 {
        return Err(Error::from_server_code(decoded.longs[0]));
    }
    Ok(())
}

pub fn chunk_count(byte_count: usize, super_chunk_size: usize) -> Result<usize, Error> {
    if super_chunk_size == 0 {
        return Err(Error::InvalidArgument(
            "super-chunk size cannot be zero".to_string(),
        ));
    }
    if byte_count == 0 {
        return Ok(0);
    }
    Ok((byte_count - 1) / super_chunk_size + 1)
}

pub fn chunk_size(byte_count: usize, chunks: usize) -> Result<usize, Error> {
    if byte_count == 0 || chunks == 0 {
        return Err(Error::InvalidArgument(
            "non-empty image transfers require at least one chunk".to_string(),
        ));
    }
    Ok((byte_count - 1) / chunks + 1)
}

/// Plans raw image writes as PythonModule-compatible sub-chunks. The metadata
/// exchange advertises super-chunks capped at 336,200,000 bytes, while each
/// socket write is capped at 16,810,000 bytes.
pub fn image_send_ranges(byte_count: usize) -> Result<Vec<Range<usize>>, Error> {
    if byte_count == 0 {
        return Err(Error::InvalidArgument(
            "image transfer must contain bytes".to_string(),
        ));
    }
    let super_chunks = chunk_count(byte_count, PYTHONMODULE_SUPER_CHUNK_SIZE)?;
    let mut ranges = Vec::new();
    let estimated_ranges = super_chunks
        .checked_mul(20)
        .ok_or_else(|| Error::ResourceLimit("image send plan range count overflow".to_string()))?;
    ranges.try_reserve(estimated_ranges).map_err(|error| {
        Error::ResourceLimit(format!("cannot reserve image send plan: {error:?}"))
    })?;
    let mut super_start = 0usize;
    while super_start < byte_count {
        let super_end = super_start
            .checked_add(PYTHONMODULE_SUPER_CHUNK_SIZE)
            .map_or(byte_count, |end| end.min(byte_count));
        let mut offset = super_start;
        while offset < super_end {
            let end = offset
                .checked_add(PYTHONMODULE_CHUNK_SIZE)
                .map_or(super_end, |end| end.min(super_end));
            ranges.push(offset..end);
            offset = end;
        }
        super_start = super_end;
    }
    Ok(ranges)
}

fn positive_usize(value: i32, label: &str) -> Result<usize, Error> {
    if value <= 0 {
        return Err(Error::InvalidFrame(format!(
            "{label} is {value}, expected positive"
        )));
    }
    Ok(value as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::encode_frame;

    #[test]
    fn put_image_options_builder_supplies_pythonmodule_defaults() {
        let options = PutImageOptions::new(MrcMode::Byte, 2, 3)
            .with_to_buffer(BufferIndex::new(1).unwrap())
            .with_base_buffer(BufferIndex::new(2).unwrap());
        assert_eq!(options.size_x, 2);
        assert_eq!(options.size_y, 3);
        assert_eq!(options.to_buffer.index(), 1);
        assert_eq!(options.base_buffer.index(), 2);
        assert_eq!(options.more_binning, 1);
        assert_eq!(options.capture_flag, -1);
    }

    #[test]
    fn parses_buffer_image_metadata() {
        let frame = encode_frame(&[0, 2, 1024, 256, 128, 131072, 1], &[], &[], &[]).unwrap();
        let meta = decode_buffer_image_meta(&frame).unwrap();
        validate_buffer_image_meta(&meta, 131072).unwrap();
        assert_eq!(meta.mode, MrcMode::Float);
        assert_eq!(meta.row_bytes, 1024);
        assert_eq!(meta.byte_count, 131072);
    }

    #[test]
    fn calculates_pythonmodule_chunk_sizes() {
        assert_eq!(chunk_count(33_620_001, 33_620_000).unwrap(), 2);
        assert_eq!(chunk_size(33_620_001, 2).unwrap(), 16_810_001);
    }

    #[test]
    fn plans_pythonmodule_subchunks_without_allocating_image_data() {
        let ranges = image_send_ranges(PYTHONMODULE_CHUNK_SIZE + 1).unwrap();
        assert_eq!(
            ranges,
            vec![
                0..PYTHONMODULE_CHUNK_SIZE,
                PYTHONMODULE_CHUNK_SIZE..PYTHONMODULE_CHUNK_SIZE + 1
            ]
        );

        let exact = image_send_ranges(PYTHONMODULE_SUPER_CHUNK_SIZE).unwrap();
        assert_eq!(exact.first().unwrap().start, 0);
        assert_eq!(exact.last().unwrap().end, PYTHONMODULE_SUPER_CHUNK_SIZE);
        assert!(
            exact
                .iter()
                .all(|range| range.len() <= PYTHONMODULE_CHUNK_SIZE)
        );

        let large = image_send_ranges(PYTHONMODULE_SUPER_CHUNK_SIZE + 1).unwrap();
        assert_eq!(large.first().unwrap().start, 0);
        assert_eq!(large.last().unwrap().end, PYTHONMODULE_SUPER_CHUNK_SIZE + 1);
        assert!(
            large
                .iter()
                .all(|range| range.len() <= PYTHONMODULE_CHUNK_SIZE)
        );

        assert!(matches!(
            image_send_ranges(usize::MAX),
            Err(Error::ResourceLimit(_))
        ));
    }

    #[test]
    fn validates_python_buffer_specifications() {
        let regular = BufferSelector::parse("T").unwrap();
        assert_eq!(regular.buffer.letter(), 'T');
        assert!(!regular.fft);
        let fft = BufferSelector::parse("HF").unwrap();
        assert_eq!(fft.buffer.letter(), 'H');
        assert!(fft.fft);
        assert!(BufferSelector::parse("IF").is_err());
        assert!(BufferSelector::parse("a").is_err());
    }

    #[test]
    fn maps_image_user_stop_to_the_exit_error() {
        let frame = encode_frame(&[-10, 0, 0, 0, 0, 0, 0], &[], &[], &[]).unwrap();
        assert!(matches!(
            decode_buffer_image_meta(&frame),
            Err(Error::UserStop)
        ));
    }

    #[test]
    fn rejects_extra_payload_in_fixed_image_responses() {
        let meta = encode_frame(&[0, 0, 1, 1, 1, 1, 1], &[], &[], &[0, 0, 0, 0]).unwrap();
        assert!(matches!(
            decode_buffer_image_meta(&meta),
            Err(Error::InvalidFrame(message)) if message.contains("unexpected payload")
        ));
        let ack = encode_frame(&[0], &[], &[], &[0, 0, 0, 0]).unwrap();
        assert!(matches!(
            decode_put_image_ack(&ack),
            Err(Error::InvalidFrame(message)) if message.contains("unexpected payload")
        ));
    }

    #[test]
    fn rejects_excessive_chunk_counts() {
        let frame = encode_frame(
            &[
                0,
                0,
                1,
                1,
                MAX_IMAGE_CHUNKS as i32,
                MAX_IMAGE_CHUNKS as i32,
                MAX_IMAGE_CHUNKS as i32 + 1,
            ],
            &[],
            &[],
            &[],
        )
        .unwrap();
        let meta = decode_buffer_image_meta(&frame).unwrap();
        assert!(matches!(
            validate_buffer_image_meta(&meta, usize::MAX),
            Err(Error::InvalidFrame(message)) if message.contains("maximum")
        ));
    }

    #[test]
    fn rejects_inconsistent_image_metadata_and_fft_buffer_overflow() {
        let frame = encode_frame(&[0, 2, 1024, 256, 128, 131073, 1], &[], &[], &[]).unwrap();
        let meta = decode_buffer_image_meta(&frame).unwrap();
        assert!(matches!(
            validate_buffer_image_meta(&meta, usize::MAX),
            Err(Error::InvalidFrame(_))
        ));
        assert!(BufferIndex::new(8).unwrap().validate_for_fft(true).is_err());

        let overflowing = BufferImageMeta {
            mode: MrcMode::Byte,
            row_bytes: usize::MAX,
            size_x: 1,
            size_y: 2,
            byte_count: usize::MAX,
            chunks: 1,
        };
        assert!(matches!(
            validate_buffer_image_meta(&overflowing, usize::MAX),
            Err(Error::InvalidFrame(_))
        ));
    }

    #[test]
    fn rejects_zero_sized_put_image_requests() {
        let options = PutImageOptions {
            mode: MrcMode::Byte,
            size_x: 0,
            size_y: 1,
            to_buffer: BufferIndex::new(0).unwrap(),
            base_buffer: BufferIndex::new(0).unwrap(),
            more_binning: 1,
            capture_flag: -1,
        };
        assert!(matches!(
            encode_put_image_request(options, 0, 0),
            Err(Error::InvalidArgument(_))
        ));

        let valid_options = PutImageOptions {
            mode: MrcMode::Byte,
            size_x: 1,
            size_y: 1,
            to_buffer: BufferIndex::new(0).unwrap(),
            base_buffer: BufferIndex::new(0).unwrap(),
            more_binning: 1,
            capture_flag: -1,
        };
        assert!(matches!(
            encode_put_image_request(valid_options, 1, 2),
            Err(Error::InvalidArgument(message)) if message.contains("exceeds byte count")
        ));
    }
}
