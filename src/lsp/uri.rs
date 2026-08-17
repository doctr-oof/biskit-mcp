use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

/// Characters that may appear unescaped in a file URI path segment.
fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':')
}

pub fn from_path(path: &Path) -> Result<String> {
    let Some(text) = path.to_str() else {
        bail!("path is not valid UTF-8: {}", path.display());
    };

    let prefix = if text.starts_with("//") || text.starts_with(r"\\") {
        "file:"
    } else if text.starts_with('/') || text.starts_with('\\') {
        "file://"
    } else {
        "file:///"
    };

    // Escapes are rare in a source path, so sizing for "no escape needed" is almost always right
    // and never wrong by more than the reallocation it saves.
    let mut encoded = String::with_capacity(prefix.len() + text.len());
    encoded.push_str(prefix);

    for byte in text.bytes() {
        let byte = if byte == b'\\' { b'/' } else { byte };
        if is_unreserved(byte) {
            encoded.push(byte as char);
            continue;
        }
        // Pushing the two hex digits directly avoids a heap allocation per escaped byte.
        encoded.push('%');
        encoded.push(HEX_DIGITS[(byte >> 4) as usize] as char);
        encoded.push(HEX_DIGITS[(byte & 0x0f) as usize] as char);
    }
    Ok(encoded)
}

pub fn to_path(uri: &str) -> Result<PathBuf> {
    let Some(remainder) = uri.strip_prefix("file://") else {
        bail!("not a file URI: {uri}");
    };

    let decoded = percent_decode(remainder)?;
    let trimmed = decoded.strip_prefix('/').unwrap_or(&decoded);
    let looks_like_windows_drive = trimmed
        .as_bytes()
        .get(1)
        .is_some_and(|byte| *byte == b':' && trimmed.as_bytes()[0].is_ascii_alphabetic());

    if looks_like_windows_drive {
        return Ok(PathBuf::from(trimmed.replace('/', "\\")));
    }
    Ok(PathBuf::from(decoded))
}

fn percent_decode(input: &str) -> Result<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' {
            let Some(hex) = input.get(index + 1..index + 3) else {
                bail!("truncated percent escape in URI: {input}");
            };
            let value = u8::from_str_radix(hex, 16)
                .map_err(|_| anyhow::anyhow!("invalid percent escape in URI: {input}"))?;
            out.push(value);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    Ok(String::from_utf8(out)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_paths_round_trip() {
        let uri = from_path(Path::new(r"C:\Users\dev\my game\src\init.luau")).unwrap();
        assert_eq!(uri, "file:///C:/Users/dev/my%20game/src/init.luau");
        assert_eq!(
            to_path(&uri).unwrap(),
            PathBuf::from(r"C:\Users\dev\my game\src\init.luau")
        );
    }

    #[test]
    fn posix_paths_round_trip() {
        let uri = from_path(Path::new("/home/dev/src/init.luau")).unwrap();
        assert_eq!(uri, "file:///home/dev/src/init.luau");
        assert_eq!(
            to_path(&uri).unwrap(),
            PathBuf::from("/home/dev/src/init.luau")
        );
    }

    #[test]
    fn rejects_non_file_uris() {
        assert!(to_path("https://example.com/a").is_err());
    }

    #[test]
    fn unc_paths_keep_their_leading_double_slash() {
        assert_eq!(
            from_path(Path::new(r"\\build\share\src\init.luau")).unwrap(),
            "file://build/share/src/init.luau"
        );
        assert_eq!(
            from_path(Path::new("//build/share/src/init.luau")).unwrap(),
            "file://build/share/src/init.luau"
        );
    }

    #[test]
    fn every_reserved_byte_is_percent_encoded_in_upper_case() {
        let uri = from_path(Path::new("/tmp/a b#c?d%e[f]/ünïcode.luau")).unwrap();
        assert_eq!(
            uri,
            "file:///tmp/a%20b%23c%3Fd%25e%5Bf%5D/%C3%BCn%C3%AFcode.luau"
        );
        assert_eq!(
            to_path(&uri).unwrap(),
            PathBuf::from("/tmp/a b#c?d%e[f]/ünïcode.luau")
        );
    }
}
