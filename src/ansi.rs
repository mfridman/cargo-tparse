//! Remove terminal escape sequences from text.

const ESC: char = '\x1b';
const BEL: char = '\x07';

/// Remove ANSI escape sequences, for example colors from
/// CARGO_TERM_COLOR=always or hyperlinks.
pub fn strip(text: &str) -> String {
    if !text.contains(ESC) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != ESC {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI, for example "ESC[1;32m": parameters up to a final byte
            // from '@' to '~'.
            Some('[') => {
                while let Some(&c) = chars.peek() {
                    if !(' '..='?').contains(&c) {
                        if ('@'..='~').contains(&c) {
                            chars.next();
                        }
                        break;
                    }
                    chars.next();
                }
            }
            // OSC, for example a hyperlink: up to BEL or "ESC\".
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == BEL {
                        break;
                    }
                    if c == ESC {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            // Other sequences have one more character.
            _ => {}
        }
    }
    out
}

/// Remove escape sequences and control characters, except new lines and
/// tabs. A terminal or a stream filter can hide control characters, so text
/// that must keep its shape, such as Markdown, must not have them.
pub fn printable(text: &str) -> String {
    strip(text)
        .chars()
        .filter(|&c| !c.is_control() || c == '\n' || c == '\t')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::strip;

    #[test]
    fn sequences() {
        let cases = [
            (
                "\x1b[1m\x1b[32m     Running\x1b[0m tests",
                "     Running tests",
            ),
            ("a\x1b[38;5;196mred\x1b[0m", "ared"),
            (
                "\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\",
                "link",
            ),
            ("\x1b]0;title\x07text", "text"),
            ("plain", "plain"),
        ];
        for (input, want) in cases {
            assert_eq!(strip(input), want, "{input:?}");
        }
    }
}
