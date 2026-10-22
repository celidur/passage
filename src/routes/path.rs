/// Decodes a request path and collapses empty segments so prefix matching sees what
/// upstreams will see. Returns `None` for dot segments and backslashes, which could
/// otherwise reach a different route than the one matched (e.g. bypass forward-auth).
pub fn canonical_path(raw: &str) -> Option<String> {
    let decoded = percent_encoding::percent_decode_str(raw).decode_utf8_lossy();
    if decoded.contains('\\') {
        return None;
    }
    let mut path = String::with_capacity(decoded.len());
    for segment in decoded.split('/').filter(|s| !s.is_empty()) {
        if segment == "." || segment == ".." {
            return None;
        }
        path.push('/');
        path.push_str(segment);
    }
    if path.is_empty() {
        path.push('/');
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_and_collapses() {
        assert_eq!(canonical_path("//api//users/").unwrap(), "/api/users");
        assert_eq!(canonical_path("/%61pi/x").unwrap(), "/api/x");
        assert_eq!(canonical_path("").unwrap(), "/");
    }

    #[test]
    fn rejects_ambiguous_paths() {
        for path in [
            "/public/../api",
            "/%2e%2e/api",
            "/./api",
            "/api\\x",
            "/a%5Cb",
        ] {
            assert!(canonical_path(path).is_none(), "{path}");
        }
    }
}
