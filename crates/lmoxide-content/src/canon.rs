//! The canonical text of a content file: pretty JSON with two-space indents, fields in their
//! declared order, short arrays of plain values on one line, and a final newline. Everything
//! that writes content writes through here, so a diff only shows real changes.

use serde::Serialize;

/// Arrays of plain values up to this many characters stay on one line.
const INLINE: usize = 100;

pub fn to_string<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    let pretty = serde_json::to_string_pretty(value)?;
    let mut out = inline_arrays(&pretty);
    out.push('\n');
    Ok(out)
}

/// Joins `[\n  1,\n  2\n]` into `[1, 2]` where the array holds no objects or arrays.
fn inline_arrays(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' {
            let end = string_end(bytes, i);
            out.push_str(&text[i..end]);
            i = end;
            continue;
        }
        if c == b'['
            && let Some((joined, end)) = try_inline(text, i)
        {
            out.push_str(&joined);
            i = end;
            continue;
        }
        out.push(c as char);
        i += 1;
    }
    out
}

/// The index just past the string starting at `start` (a `"`).
fn string_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

fn try_inline(text: &str, start: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let mut items = Vec::new();
    let mut item = String::new();
    let mut i = start + 1;
    loop {
        let c = *bytes.get(i)?;
        match c {
            b'"' => {
                let end = string_end(bytes, i);
                item.push_str(&text[i..end]);
                i = end;
                continue;
            }
            b'{' | b'[' => return None,
            b',' => items.push(std::mem::take(&mut item)),
            b']' => {
                if !item.trim().is_empty() {
                    items.push(item);
                }
                let items: Vec<&str> = items.iter().map(|s| s.trim()).collect();
                let joined = format!("[{}]", items.join(", "));
                return (joined.len() <= INLINE).then_some((joined, i + 1));
            }
            b'\n' | b' ' => {}
            _ => item.push(c as char),
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn inlines_plain_arrays_only() {
        let v = serde_json::json!({ "pos": [1.5, 2, 3], "tags": ["a, b", "c"], "list": [{ "x": [1] }], "e": [] });
        let text = super::to_string(&v).unwrap();
        assert!(text.contains("\"pos\": [1.5, 2, 3]"), "{text}");
        assert!(text.contains("\"tags\": [\"a, b\", \"c\"]"), "{text}");
        assert!(text.contains("\"x\": [1]"), "{text}");
        assert!(text.contains("\"e\": []"), "{text}");
        let back: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back, v);
    }
}
