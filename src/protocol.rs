use crate::error::Error;

pub const PSS_REGULAR_COMMAND: i32 = 1;
pub const PSS_CHUNK_HANDSHAKE: i32 = 2;
pub const PSS_OK_TO_RUN_EXTERNAL_SCRIPT: i32 = 3;
pub const PSS_GET_BUFFER_IMAGE: i32 = 4;
pub const PSS_PUT_IMAGE_IN_BUFFER: i32 = 5;

pub const DEFAULT_EXTERNAL_PORT: u16 = 48888;
pub const PYTHONMODULE_CHUNK_SIZE: usize = 16_810_000;
pub const PYTHONMODULE_SUPER_CHUNK_SIZE: usize = 336_200_000;
/// SerialEM stores framed lengths in signed 32-bit integers.
pub const PROTOCOL_MAX_FRAME_BYTES: usize = i32::MAX as usize;
/// SerialEM string fields are NUL-terminated byte sequences interpreted as UTF-8 by this crate.
pub const SERIAL_EM_STRING_ENCODING: &str = "UTF-8";

pub const MRC_MODE_BYTE: i32 = 0;
pub const MRC_MODE_SHORT: i32 = 1;
pub const MRC_MODE_FLOAT: i32 = 2;
pub const MRC_MODE_USHORT: i32 = 6;
pub const MRC_MODE_RGB: i32 = 16;

pub const SCRIPT_NORMAL_EXIT: i32 = -123456;
pub const SCRIPT_USER_STOP: i32 = -234561;
pub const SCRIPT_EXIT_NO_EXC: i32 = -654321;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MrcMode {
    Byte,
    Short,
    Float,
    UShort,
    Rgb,
}

impl MrcMode {
    pub fn as_wire(self) -> i32 {
        match self {
            Self::Byte => MRC_MODE_BYTE,
            Self::Short => MRC_MODE_SHORT,
            Self::Float => MRC_MODE_FLOAT,
            Self::UShort => MRC_MODE_USHORT,
            Self::Rgb => MRC_MODE_RGB,
        }
    }

    pub fn from_wire(value: i32) -> Result<Self, Error> {
        match value {
            MRC_MODE_BYTE => Ok(Self::Byte),
            MRC_MODE_SHORT => Ok(Self::Short),
            MRC_MODE_FLOAT => Ok(Self::Float),
            MRC_MODE_USHORT => Ok(Self::UShort),
            MRC_MODE_RGB => Ok(Self::Rgb),
            _ => Err(Error::InvalidFrame(format!("unsupported MRC mode {value}"))),
        }
    }

    pub fn item_size(self) -> usize {
        match self {
            Self::Byte => 1,
            Self::Short | Self::UShort => 2,
            Self::Float => 4,
            Self::Rgb => 3,
        }
    }

    pub fn format(self) -> &'static str {
        match self {
            Self::Byte => "B",
            Self::Short => "h",
            Self::Float => "f",
            Self::UShort => "H",
            Self::Rgb => "BBB",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptItem {
    pub int_value: i32,
    pub double_value: f64,
    pub string_value: String,
}

impl Default for ScriptItem {
    fn default() -> Self {
        Self {
            int_value: 0,
            double_value: 0.0,
            string_value: String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DecodedFrame {
    pub longs: Vec<i32>,
    pub bools: Vec<i32>,
    pub doubles: Vec<f64>,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReportValue {
    Number(f64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegularResponse {
    pub return_code: i32,
    pub highest_report_index: i32,
    pub error_occurred: i32,
    pub reports: Vec<ReportValue>,
}

pub fn encode_frame(
    longs: &[i32],
    bools: &[i32],
    doubles: &[f64],
    payload: &[u8],
) -> Result<Vec<u8>, Error> {
    encode_frame_with_limit(longs, bools, doubles, payload, PROTOCOL_MAX_FRAME_BYTES)
}

pub fn encode_frame_with_limit(
    longs: &[i32],
    bools: &[i32],
    doubles: &[f64],
    payload: &[u8],
    max_frame_bytes: usize,
) -> Result<Vec<u8>, Error> {
    if max_frame_bytes < 4 {
        return Err(Error::InvalidArgument(
            "max frame limit must be at least four bytes".to_string(),
        ));
    }
    let max_frame_bytes = max_frame_bytes.min(PROTOCOL_MAX_FRAME_BYTES);
    if payload.len() % 4 != 0 {
        return Err(Error::InvalidArgument(format!(
            "long-array payload length {} is not divisible by four",
            payload.len()
        )));
    }

    let total_len = 4usize
        .checked_add(
            longs
                .len()
                .checked_mul(4)
                .ok_or_else(|| Error::ResourceLimit("too many LONG fields".to_string()))?,
        )
        .and_then(|length| length.checked_add(bools.len().checked_mul(4)?))
        .and_then(|length| length.checked_add(doubles.len().checked_mul(8)?))
        .and_then(|length| length.checked_add(payload.len()))
        .ok_or_else(|| Error::ResourceLimit("frame is too large".to_string()))?;
    if total_len > PROTOCOL_MAX_FRAME_BYTES || total_len > max_frame_bytes {
        return Err(Error::ResourceLimit(format!(
            "frame is {total_len} bytes, configured maximum is {}",
            max_frame_bytes.min(PROTOCOL_MAX_FRAME_BYTES)
        )));
    }
    let total_len_u32 = u32::try_from(total_len)
        .map_err(|_| Error::ResourceLimit("frame exceeds uint32 length".to_string()))?;

    let mut frame = Vec::new();
    frame.try_reserve_exact(total_len).map_err(|error| {
        Error::ResourceLimit(format!("cannot reserve {total_len} frame bytes: {error:?}"))
    })?;
    frame.extend_from_slice(&total_len_u32.to_le_bytes());
    for value in longs {
        frame.extend_from_slice(&value.to_le_bytes());
    }
    for value in bools {
        frame.extend_from_slice(&value.to_le_bytes());
    }
    for value in doubles {
        frame.extend_from_slice(&value.to_le_bytes());
    }
    frame.extend_from_slice(payload);
    Ok(frame)
}

pub fn decode_frame(
    frame: &[u8],
    long_count: usize,
    bool_count: usize,
    double_count: usize,
) -> Result<DecodedFrame, Error> {
    if frame.len() < 4 {
        return Err(Error::InvalidFrame(
            "frame is shorter than its length field".to_string(),
        ));
    }
    let declared_len = u32::from_le_bytes(frame[0..4].try_into().unwrap()) as usize;
    if declared_len != frame.len() {
        return Err(Error::InvalidFrame(format!(
            "length field declares {declared_len} bytes, received {}",
            frame.len()
        )));
    }

    let fields_len = 4usize
        .checked_add(
            long_count
                .checked_mul(4)
                .ok_or_else(|| Error::InvalidFrame("too many LONG fields".to_string()))?,
        )
        .and_then(|length| length.checked_add(bool_count.checked_mul(4)?))
        .and_then(|length| length.checked_add(double_count.checked_mul(8)?))
        .ok_or_else(|| Error::InvalidFrame("field layout is too large".to_string()))?;
    if fields_len > frame.len() {
        return Err(Error::InvalidFrame(format!(
            "field layout needs {fields_len} bytes, frame has {}",
            frame.len()
        )));
    }

    let mut offset = 4usize;
    let mut longs = Vec::with_capacity(long_count);
    for _ in 0..long_count {
        longs.push(read_i32(frame, &mut offset)?);
    }
    let mut bools = Vec::with_capacity(bool_count);
    for _ in 0..bool_count {
        bools.push(read_i32(frame, &mut offset)?);
    }
    let mut doubles = Vec::with_capacity(double_count);
    for _ in 0..double_count {
        doubles.push(read_f64(frame, &mut offset)?);
    }

    Ok(DecodedFrame {
        longs,
        bools,
        doubles,
        payload: frame[offset..].to_vec(),
    })
}

/// Decodes SerialEM's compact negative-error response. For an error response,
/// the server sends only the frame length and one negative LONG (8 bytes total).
pub fn decode_negative_error_frame(frame: &[u8]) -> Result<Option<Error>, Error> {
    if frame.len() < 4 {
        return Err(Error::InvalidFrame(
            "frame is shorter than its length field".to_string(),
        ));
    }
    let declared_len = u32::from_le_bytes(frame[0..4].try_into().unwrap()) as usize;
    if declared_len != frame.len() {
        return Err(Error::InvalidFrame(format!(
            "length field declares {declared_len} bytes, received {}",
            frame.len()
        )));
    }
    if declared_len != 8 {
        return Ok(None);
    }
    let code = i32::from_le_bytes(frame[4..8].try_into().unwrap());
    if code < 0 {
        Ok(Some(Error::from_server_code(code)))
    } else {
        Ok(None)
    }
}

pub fn decode_fixed_or_error_frame(
    frame: &[u8],
    long_count: usize,
    bool_count: usize,
    double_count: usize,
) -> Result<DecodedFrame, Error> {
    if let Some(error) = decode_negative_error_frame(frame)? {
        return Err(error);
    }
    decode_fixed_frame(frame, long_count, bool_count, double_count)
}

pub fn decode_fixed_frame(
    frame: &[u8],
    long_count: usize,
    bool_count: usize,
    double_count: usize,
) -> Result<DecodedFrame, Error> {
    let decoded = decode_frame(frame, long_count, bool_count, double_count)?;
    if !decoded.payload.is_empty() {
        return Err(Error::InvalidFrame(format!(
            "fixed-format frame contains {} unexpected payload bytes",
            decoded.payload.len()
        )));
    }
    Ok(decoded)
}

pub fn encode_ok_to_run_external_script() -> Result<Vec<u8>, Error> {
    encode_frame(&[PSS_OK_TO_RUN_EXTERNAL_SCRIPT], &[], &[], &[])
}

pub fn encode_regular_command(
    function_code: i32,
    items: &[ScriptItem],
    last_non_empty_index: usize,
) -> Result<Vec<u8>, Error> {
    encode_regular_command_with_limit(
        function_code,
        items,
        last_non_empty_index,
        PROTOCOL_MAX_FRAME_BYTES,
    )
}

pub fn encode_regular_command_with_limit(
    function_code: i32,
    items: &[ScriptItem],
    last_non_empty_index: usize,
    max_frame_bytes: usize,
) -> Result<Vec<u8>, Error> {
    let max_frame_bytes = max_frame_bytes.min(PROTOCOL_MAX_FRAME_BYTES);
    let last_non_empty_index_wire = i32::try_from(last_non_empty_index).map_err(|_| {
        Error::InvalidArgument("last non-empty argument index exceeds int32".to_string())
    })?;
    let payload = if last_non_empty_index == 0 {
        // PythonModule::AddLongsAndStrings(NULL, 0, NULL, 0) still allocates
        // one LONG-array word and sends it as zero-filled padding.
        vec![0u8; 4]
    } else {
        if items.len() <= last_non_empty_index {
            return Err(Error::InvalidArgument(format!(
                "{} script items are insufficient for last non-empty index {last_non_empty_index}",
                items.len()
            )));
        }
        let payload_limit = max_frame_bytes.checked_sub(20).ok_or_else(|| {
            Error::InvalidArgument("max frame limit is smaller than a regular header".to_string())
        })?;
        encode_item_array_with_limit(&items[..=last_non_empty_index], payload_limit)?
    };
    let payload_words = i32::try_from(payload.len() / 4)
        .map_err(|_| Error::ResourceLimit("script argument array is too large".to_string()))?;
    encode_frame_with_limit(
        &[
            PSS_REGULAR_COMMAND,
            function_code,
            last_non_empty_index_wire,
            payload_words,
        ],
        &[],
        &[],
        &payload,
        max_frame_bytes,
    )
}

pub fn encode_item_array(items: &[ScriptItem]) -> Result<Vec<u8>, Error> {
    encode_item_array_with_limit(items, PROTOCOL_MAX_FRAME_BYTES)
}

pub fn encode_item_array_with_limit(
    items: &[ScriptItem],
    max_payload_bytes: usize,
) -> Result<Vec<u8>, Error> {
    if items.iter().any(|item| !item.double_value.is_finite()) {
        return Err(Error::InvalidArgument(
            "script double arguments must be finite".to_string(),
        ));
    }
    if items
        .iter()
        .any(|item| item.string_value.as_bytes().contains(&0))
    {
        return Err(Error::InvalidArgument(
            "script string arguments cannot contain NUL bytes".to_string(),
        ));
    }

    let fixed_bytes = items
        .len()
        .checked_mul(4 + 8)
        .ok_or_else(|| Error::ResourceLimit("script argument array is too large".to_string()))?;
    let string_bytes = items.iter().try_fold(0usize, |total, item| {
        let item_bytes =
            item.string_value.len().checked_add(1).ok_or_else(|| {
                Error::ResourceLimit("script string array is too large".to_string())
            })?;
        total
            .checked_add(item_bytes)
            .ok_or_else(|| Error::ResourceLimit("script string array is too large".to_string()))
    })?;
    let required_bytes = fixed_bytes
        .checked_add(string_bytes)
        .ok_or_else(|| Error::ResourceLimit("script argument array is too large".to_string()))?;
    // This mirrors the C++ (lenTot + 5) / 4 allocation rule. The extra byte is
    // consumed by the terminating NUL written after the string list.
    let word_count = required_bytes
        .checked_add(5)
        .ok_or_else(|| Error::ResourceLimit("script argument array is too large".to_string()))?
        / 4;
    let total_bytes = word_count
        .checked_mul(4)
        .ok_or_else(|| Error::ResourceLimit("script argument array is too large".to_string()))?;
    if total_bytes > max_payload_bytes {
        return Err(Error::ResourceLimit(format!(
            "script argument array is {total_bytes} bytes, configured payload maximum is {max_payload_bytes}"
        )));
    }
    let mut payload = Vec::new();
    payload.try_reserve_exact(total_bytes).map_err(|error| {
        Error::ResourceLimit(format!(
            "cannot reserve {total_bytes} script argument bytes: {error:?}"
        ))
    })?;
    payload.resize(total_bytes, 0);

    let mut offset = 0usize;
    for item in items {
        payload[offset..offset + 4].copy_from_slice(&item.int_value.to_le_bytes());
        offset += 4;
    }
    for item in items {
        payload[offset..offset + 8].copy_from_slice(&item.double_value.to_le_bytes());
        offset += 8;
    }
    for item in items {
        let bytes = item.string_value.as_bytes();
        payload[offset..offset + bytes.len()].copy_from_slice(bytes);
        offset += bytes.len();
        payload[offset] = 0;
        offset += 1;
    }
    // The C++ implementation appends one extra NUL after the string list.
    payload[offset] = 0;
    Ok(payload)
}

pub fn decode_regular_response(frame: &[u8]) -> Result<RegularResponse, Error> {
    if let Some(error) = decode_negative_error_frame(frame)? {
        return Err(error);
    }
    let decoded = decode_frame(frame, 4, 0, 0)?;
    let return_code = decoded.longs[0];
    let highest_report_index = decoded.longs[1];
    let error_occurred = decoded.longs[2];
    let declared_words = decoded.longs[3];
    if declared_words < 0 {
        return Err(Error::InvalidFrame(format!(
            "negative regular response array length {declared_words}"
        )));
    }
    let declared_bytes = (declared_words as usize)
        .checked_mul(4)
        .ok_or_else(|| Error::InvalidFrame("regular response array is too large".to_string()))?;
    if decoded.payload.len() != declared_bytes {
        return Err(Error::InvalidFrame(format!(
            "regular response declares {declared_bytes} payload bytes, received {}",
            decoded.payload.len()
        )));
    }

    let report_count = if highest_report_index < 0 {
        0
    } else {
        (highest_report_index as usize)
            .checked_add(1)
            .ok_or_else(|| Error::InvalidFrame("report count overflow".to_string()))?
    };
    let fixed_report_bytes = report_count
        .checked_mul(4 + 8)
        .ok_or_else(|| Error::InvalidFrame("report array is too large".to_string()))?;
    if fixed_report_bytes > declared_bytes {
        return Err(Error::InvalidFrame(format!(
            "regular response needs {fixed_report_bytes} bytes for {report_count} reports, array has {declared_bytes}"
        )));
    }

    let payload = &decoded.payload[..declared_bytes];
    let mut offset = 0usize;
    let mut string_flags = Vec::with_capacity(report_count);
    for _ in 0..report_count {
        string_flags.push(read_i32(payload, &mut offset)? != 0);
    }
    let mut numbers = Vec::with_capacity(report_count);
    for _ in 0..report_count {
        numbers.push(read_f64(payload, &mut offset)?);
    }

    let mut reports = Vec::with_capacity(report_count);
    for (is_string, number) in string_flags.into_iter().zip(numbers) {
        if !is_string {
            reports.push(ReportValue::Number(number));
            continue;
        }
        let end = payload[offset..]
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| Error::InvalidFrame("unterminated report string".to_string()))?
            + offset;
        let text = String::from_utf8(payload[offset..end].to_vec())?;
        offset = end + 1;
        reports.push(ReportValue::Text(text));
    }

    Ok(RegularResponse {
        return_code,
        highest_report_index,
        error_occurred,
        reports,
    })
}

fn read_i32(bytes: &[u8], offset: &mut usize) -> Result<i32, Error> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| Error::InvalidFrame("integer offset overflow".to_string()))?;
    if end > bytes.len() {
        return Err(Error::InvalidFrame("truncated 32-bit field".to_string()));
    }
    let value = i32::from_le_bytes(bytes[*offset..end].try_into().unwrap());
    *offset = end;
    Ok(value)
}

fn read_f64(bytes: &[u8], offset: &mut usize) -> Result<f64, Error> {
    let end = offset
        .checked_add(8)
        .ok_or_else(|| Error::InvalidFrame("double offset overflow".to_string()))?;
    if end > bytes.len() {
        return Err(Error::InvalidFrame("truncated double field".to_string()));
    }
    let value = f64::from_le_bytes(bytes[*offset..end].try_into().unwrap());
    *offset = end;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_codec_round_trips_small_payload_lengths() {
        for payload_len in 0..128usize {
            let padded_len = payload_len.div_ceil(4) * 4;
            let payload = vec![payload_len as u8; padded_len];
            let frame = encode_frame(&[payload_len as i32], &[], &[], &payload).unwrap();
            let decoded = decode_frame(&frame, 1, 0, 0).unwrap();
            assert_eq!(decoded.longs, vec![payload_len as i32]);
            assert_eq!(decoded.payload, payload);
        }
    }

    #[test]
    fn rejects_regular_argument_allocation_before_transport_limit() {
        let items = [
            ScriptItem::default(),
            ScriptItem {
                int_value: 1,
                double_value: 1.0,
                string_value: "payload".to_string(),
            },
        ];
        assert!(matches!(
            encode_regular_command_with_limit(123, &items, 1, 20),
            Err(Error::ResourceLimit(message)) if message.contains("argument array")
        ));
        assert!(matches!(
            encode_frame_with_limit(&[1], &[], &[], &[], 4),
            Err(Error::ResourceLimit(message)) if message.contains("frame")
        ));
    }

    #[test]
    fn encodes_empty_regular_command_with_one_zero_item() {
        let frame = encode_regular_command(42, &[], 0).unwrap();
        assert_eq!(
            frame,
            vec![
                24, 0, 0, 0, // total frame length
                1, 0, 0, 0, // PSS_RegularCommand
                42, 0, 0, 0, // function code
                0, 0, 0, 0, // lastNonEmptyInd
                1, 0, 0, 0, // one LONG-array word
                0, 0, 0, 0, // reserved item zero
            ]
        );
    }

    #[test]
    fn encodes_one_regular_argument_with_the_reserved_item_zero() {
        let items = vec![
            ScriptItem::default(),
            ScriptItem {
                int_value: 7,
                double_value: 7.0,
                string_value: "7".into(),
            },
        ];
        let frame = encode_regular_command(42, &items, 1).unwrap();
        assert_eq!(frame.len(), 52);
        assert_eq!(u32::from_le_bytes(frame[0..4].try_into().unwrap()), 52);
        assert_eq!(
            i32::from_le_bytes(frame[4..8].try_into().unwrap()),
            PSS_REGULAR_COMMAND
        );
        assert_eq!(i32::from_le_bytes(frame[8..12].try_into().unwrap()), 42);
        assert_eq!(i32::from_le_bytes(frame[12..16].try_into().unwrap()), 1);
        assert_eq!(i32::from_le_bytes(frame[16..20].try_into().unwrap()), 8);
        assert_eq!(i32::from_le_bytes(frame[20..24].try_into().unwrap()), 0);
        assert_eq!(i32::from_le_bytes(frame[24..28].try_into().unwrap()), 7);
        assert_eq!(f64::from_le_bytes(frame[28..36].try_into().unwrap()), 0.0);
        assert_eq!(f64::from_le_bytes(frame[36..44].try_into().unwrap()), 7.0);
        assert_eq!(&frame[44..48], &[0, b'7', 0, 0]);
        assert_eq!(&frame[48..52], &[0, 0, 0, 0]);
    }

    #[test]
    fn encodes_and_decodes_regular_reports() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0i32.to_le_bytes());
        payload.extend_from_slice(&1i32.to_le_bytes());
        payload.extend_from_slice(&3.5f64.to_le_bytes());
        payload.extend_from_slice(&0.0f64.to_le_bytes());
        payload.extend_from_slice(b"ok\0");
        payload.resize(28, 0);
        let frame = encode_frame(&[0, 1, 0, 7], &[], &[], &payload).unwrap();
        let response = decode_regular_response(&frame).unwrap();
        assert_eq!(
            response.reports,
            vec![ReportValue::Number(3.5), ReportValue::Text("ok".into())]
        );
    }

    #[test]
    fn decodes_compact_negative_error_frames_for_all_fixed_surfaces() {
        let busy = encode_frame(&[-9], &[], &[], &[]).unwrap();
        assert!(matches!(decode_regular_response(&busy), Err(Error::Busy)));
        let stop = encode_frame(&[-10], &[], &[], &[]).unwrap();
        assert!(matches!(
            decode_fixed_or_error_frame(&stop, 1, 1, 0),
            Err(Error::UserStop)
        ));
        let other = encode_frame(&[-4], &[], &[], &[]).unwrap();
        assert!(matches!(
            decode_fixed_or_error_frame(&other, 7, 0, 0),
            Err(Error::SerialEm { code: -4 })
        ));
    }

    #[test]
    fn rejects_extra_payload_in_fixed_frames() {
        let frame = encode_frame(&[0], &[1], &[], &[0, 0, 0, 0]).unwrap();
        assert!(matches!(
            decode_fixed_frame(&frame, 1, 1, 0),
            Err(Error::InvalidFrame(message)) if message.contains("unexpected payload")
        ));
    }

    #[test]
    fn rejects_extra_regular_response_payload() {
        let frame = encode_frame(&[0, -1, 0, 1], &[], &[], &[0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        assert!(matches!(
            decode_regular_response(&frame),
            Err(Error::InvalidFrame(message)) if message.contains("declares 4 payload bytes")
        ));
    }

    #[test]
    fn rejects_argument_index_that_cannot_fit_the_wire_type() {
        assert!(matches!(
            encode_regular_command(42, &[], usize::MAX),
            Err(Error::InvalidArgument(message)) if message.contains("exceeds int32")
        ));
    }

    #[test]
    fn encodes_string_and_integer_items_in_separate_wire_regions() {
        let items = vec![
            ScriptItem::default(),
            ScriptItem {
                int_value: 3,
                double_value: 3.0,
                string_value: "3".into(),
            },
            ScriptItem {
                int_value: 0,
                double_value: 0.0,
                string_value: "abc".into(),
            },
        ];
        let payload = encode_item_array(&items).unwrap();
        assert_eq!(&payload[0..12], &[0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(f64::from_le_bytes(payload[12..20].try_into().unwrap()), 0.0);
        assert_eq!(f64::from_le_bytes(payload[20..28].try_into().unwrap()), 3.0);
        assert_eq!(&payload[36..44], &[0, b'3', 0, b'a', b'b', b'c', 0, 0]);
        assert!(payload[44..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn round_trips_non_ascii_utf8_report_strings() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&1i32.to_le_bytes());
        payload.extend_from_slice(&0f64.to_le_bytes());
        payload.extend_from_slice("样品".as_bytes());
        payload.push(0);
        while payload.len() % 4 != 0 {
            payload.push(0);
        }
        let words = (payload.len() / 4) as i32;
        let frame = encode_frame(&[0, 0, 0, words], &[], &[], &payload).unwrap();
        let response = decode_regular_response(&frame).unwrap();
        assert_eq!(response.reports, vec![ReportValue::Text("样品".into())]);
    }

    #[test]
    fn rejects_nul_in_script_string() {
        let items = vec![ScriptItem {
            string_value: "bad\0value".into(),
            ..ScriptItem::default()
        }];
        assert!(encode_item_array(&items).is_err());
    }

    #[test]
    fn rejects_mismatched_frame_length_and_truncated_reports() {
        let mut frame = encode_frame(&[0, -1, 0, 0], &[], &[], &[]).unwrap();
        frame[0..4].copy_from_slice(&999u32.to_le_bytes());
        assert!(decode_regular_response(&frame).is_err());

        let truncated = encode_frame(&[0, 0, 0, 0], &[], &[], &[]).unwrap();
        assert!(decode_regular_response(&truncated).is_err());
    }
}
