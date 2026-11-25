//! Values of string and character literals as written in source (jtk keeps only their position).

/// The value of a string literal or text block, including its quotes.
pub fn string(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if text.starts_with("\"\"\"") {
        return text_block(&chars);
    }
    unescape(&chars[1..chars.len() - 1])
}

/// The value of a character literal, including its quotes.
pub fn char(text: &str) -> char {
    let chars: Vec<char> = text.chars().collect();
    unescape(&chars[1..chars.len() - 1])
        .chars()
        .next()
        .unwrap_or('\0')
}

fn unescape(chars: &[char]) -> String {
    let mut value = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' {
            let (ch, len) = escape(chars, i);
            value.push(ch);
            i += len;
        } else {
            value.push(chars[i]);
            i += 1;
        }
    }
    value
}

/// Decodes the escape sequence starting with the backslash at `i`: the character and the length of the sequence.
fn escape(chars: &[char], i: usize) -> (char, usize) {
    let n = chars.get(i + 1).copied().unwrap_or('\\');
    let simple = match n {
        'n' => Some('\n'),
        't' => Some('\t'),
        'r' => Some('\r'),
        'b' => Some('\u{8}'),
        'f' => Some('\u{c}'),
        's' => Some(' '),
        '0'..='7' | 'u' => None,
        other => Some(other),
    };
    if let Some(ch) = simple {
        return (ch, 2);
    }
    if n == 'u' {
        let mut j = i + 1;
        while chars.get(j) == Some(&'u') {
            j += 1;
        }
        let hex: String = chars[j..(j + 4).min(chars.len())].iter().collect();
        let ch = u32::from_str_radix(&hex, 16)
            .ok()
            .and_then(char::from_u32)
            .unwrap_or('\u{fffd}');
        return (ch, j + 4 - i);
    }
    let mut j = i + 1;
    let mut value = 0u32;
    while j < chars.len() && j < i + 4 && ('0'..='7').contains(&chars[j]) {
        value = value * 8 + chars[j].to_digit(8).unwrap();
        j += 1;
    }
    (char::from_u32(value).unwrap_or('\0'), j - i)
}

/// The value of a text block: incidental indentation and trailing spaces removed, then escapes decoded.
fn text_block(chars: &[char]) -> String {
    let open_end = chars
        .iter()
        .position(|c| *c == '\n')
        .map_or(chars.len(), |p| p + 1);
    let content_end = chars.len().saturating_sub(3).max(open_end);
    let raw: String = chars[open_end..content_end].iter().collect();
    let lines: Vec<&str> = raw.split('\n').collect();
    let indent = lines
        .iter()
        .enumerate()
        .filter(|(idx, l)| !l.trim().is_empty() || *idx == lines.len() - 1)
        .map(|(_, l)| l.chars().take_while(|c| c.is_whitespace()).count())
        .min()
        .unwrap_or(0);
    let mut stripped = String::new();
    for (idx, l) in lines.iter().enumerate() {
        let line: String = l.chars().skip(indent).collect();
        stripped.push_str(line.trim_end());
        if idx + 1 < lines.len() {
            stripped.push('\n');
        }
    }
    let chars: Vec<char> = stripped.chars().collect();
    let mut value = String::new();
    let mut k = 0;
    while k < chars.len() {
        if chars[k] == '\\' {
            if chars.get(k + 1) == Some(&'\n') {
                k += 2;
                continue;
            }
            let (ch, len) = escape(&chars, k);
            value.push(ch);
            k += len;
        } else {
            value.push(chars[k]);
            k += 1;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_escapes() {
        assert_eq!(string(r#""a\"b\nä\101""#), "a\"b\näA");
        assert_eq!(char(r"'\''"), '\'');
    }

    #[test]
    fn strips_text_block_indentation() {
        let text = "\"\"\"\n    first\n      second  \n    \"\"\"";
        assert_eq!(string(text), "first\n  second\n");
    }
}
