use up_rust::{UCode, UStatus, UUri};

fn format_parse_error(flag: &str, raw: &str, expected: &str, max: u128) -> String {
    format!(
        "invalid value for {flag}: '{raw}' (expected {expected} in decimal or 0x-prefixed hex, range 0..={max})"
    )
}

fn parse_unsigned(flag: &str, raw: &str, expected: &str, max: u128) -> Result<u128, String> {
    if raw.is_empty() || raw.chars().any(char::is_whitespace) || raw.contains('_') {
        return Err(format_parse_error(flag, raw, expected, max));
    }

    let parsed = if let Some(hex_digits) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X"))
    {
        if hex_digits.is_empty() {
            return Err(format_parse_error(flag, raw, expected, max));
        }
        u128::from_str_radix(hex_digits, 16)
            .map_err(|_| format_parse_error(flag, raw, expected, max))?
    } else {
        raw.parse::<u128>()
            .map_err(|_| format_parse_error(flag, raw, expected, max))?
    };

    if parsed > max {
        return Err(format_parse_error(flag, raw, expected, max));
    }

    Ok(parsed)
}

pub(crate) fn parse_u32_flag(flag: &str, raw: &str) -> Result<u32, String> {
    parse_unsigned(flag, raw, "u32", u32::MAX as u128).map(|value| value as u32)
}

pub(crate) fn parse_u16_flag(flag: &str, raw: &str) -> Result<u16, String> {
    parse_unsigned(flag, raw, "u16", u16::MAX as u128).map(|value| value as u16)
}

pub(crate) fn parse_u8_flag(flag: &str, raw: &str) -> Result<u8, String> {
    parse_unsigned(flag, raw, "u8", u8::MAX as u128).map(|value| value as u8)
}

pub(crate) fn invalid_argument_status(message: impl Into<String>) -> UStatus {
    UStatus::fail_with_code(UCode::INVALID_ARGUMENT, message.into())
}

pub(crate) fn parse_u32_status(flag: &str, raw: &str) -> Result<u32, UStatus> {
    parse_u32_flag(flag, raw).map_err(invalid_argument_status)
}

pub(crate) fn parse_u16_status(flag: &str, raw: &str) -> Result<u16, UStatus> {
    parse_u16_flag(flag, raw).map_err(invalid_argument_status)
}

pub(crate) fn parse_u8_status(flag: &str, raw: &str) -> Result<u8, UStatus> {
    parse_u8_flag(flag, raw).map_err(invalid_argument_status)
}

pub(crate) fn build_uuri(
    authority: &str,
    uentity: u32,
    uversion: u8,
    resource: u16,
) -> Result<UUri, UStatus> {
    UUri::try_from_parts(authority, uentity, uversion, resource).map_err(|error| {
        invalid_argument_status(format!(
            "unable to build UUri from authority='{authority}', uentity={uentity:#X}, uversion={uversion:#X}, resource={resource:#X}: {error}"
        ))
    })
}
