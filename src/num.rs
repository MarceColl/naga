//! JavaScript number semantics, where the reference implementation's output depends on them.

/// `String(n)` in JavaScript.
pub fn js(n: f64) -> String {
    ryu_js::Buffer::new().format(n).to_string()
}

pub fn is_int(n: f64) -> bool {
    n.is_finite() && n.fract() == 0.0
}

/// How values print: whole numbers as-is, others rounded to 12 significant digits.
pub fn show(n: f64) -> String {
    if is_int(n) || !n.is_finite() {
        return js(n);
    }
    js(format!("{n:.11e}").parse().unwrap())
}

/// `Math.round`: halves go up.
pub fn round(x: f64) -> f64 {
    let f = x.floor();
    if x - f >= 0.5 { f + 1.0 } else { f }
}

/// `JSON.stringify` of a string.
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// JavaScript string length and ordering count UTF-16 code units.
pub fn len16(s: &str) -> usize {
    s.encode_utf16().count()
}

pub fn cmp16(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// `s.slice(a, b)` with UTF-16 offsets.
pub fn slice16(s: &str, a: usize, b: usize) -> String {
    String::from_utf16_lossy(&s.encode_utf16().skip(a).take(b.saturating_sub(a)).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_like_js() {
        assert_eq!(show(0.1 + 0.2), "0.3");
        assert_eq!(show(1.0 / 3.0), "0.333333333333");
        assert_eq!(show(1e21), "1e+21");
        assert_eq!(show(1e-7), "1e-7");
        assert_eq!(show(-0.0), "0");
        assert_eq!(round(2.5), 3.0);
        assert_eq!(round(-2.5), -2.0);
    }
}
