use std::borrow::Cow;

/// Safely returns a prefix of the string with at most `max_chars` characters.
/// This respects UTF-8 character boundaries.
pub fn prefix_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Safely returns a suffix of the string with at most `max_chars` characters.
/// This respects UTF-8 character boundaries.
pub fn suffix_chars(s: &str, max_chars: usize) -> &str {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s;
    }
    match s.char_indices().nth(char_count - max_chars) {
        Some((idx, _)) => &s[idx..],
        None => s,
    }
}

/// Returns the first `n` characters as a Cow<str>, avoiding allocation if possible.
pub fn first_n_chars_lossy(s: &str, n: usize) -> Cow<'_, str> {
    if s.chars().count() <= n {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(prefix_chars(s, n).to_string())
    }
}

/// Safely slices a string using byte offsets if they fall on character boundaries.
/// Returns None if the offsets are invalid or not on boundaries.
pub fn slice_bytes_safe(s: &str, start: usize, end: usize) -> Option<&str> {
    if start <= end && end <= s.len() && s.is_char_boundary(start) && s.is_char_boundary(end) {
        Some(&s[start..end])
    } else {
        None
    }
}
