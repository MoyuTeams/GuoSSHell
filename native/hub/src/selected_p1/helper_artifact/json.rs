//! 受控构建记录的借用解析器；没有 String、Vec、serde scratch 或动态节点。

use super::Reject;

#[derive(Clone, Copy)]
pub(super) struct Value<'a>(pub &'a str);

impl<'a> Value<'a> {
    pub fn document(bytes: &'a [u8]) -> Result<Self, Reject> {
        let text = std::str::from_utf8(bytes).map_err(|_| Reject::Record)?;
        let value = Self(text.trim());
        if scan(value.0, 0)? != value.0.len() {
            return Err(Reject::Record);
        }
        Ok(value)
    }

    pub fn get(self, wanted: &str) -> Result<Self, Reject> {
        let mut rest = self.0.strip_prefix('{').ok_or(Reject::Record)?.trim_start();
        let mut found = None;
        while !rest.starts_with('}') {
            let end = string_end(rest)?;
            let key = Self(&rest[..end]);
            rest = rest[end..]
                .trim_start()
                .strip_prefix(':')
                .ok_or(Reject::Record)?
                .trim_start();
            let end = scan(rest, 0)?;
            if key.equals(wanted)? {
                if found.is_some() {
                    return Err(Reject::Record);
                }
                found = Some(Self(&rest[..end]));
            }
            rest = rest[end..].trim_start();
            if let Some(next) = rest.strip_prefix(',') {
                rest = next.trim_start();
            } else {
                break;
            }
        }
        found.ok_or(Reject::Record)
    }

    pub fn array(self) -> Result<Array<'a>, Reject> {
        Ok(Array {
            rest: self.0.strip_prefix('[').ok_or(Reject::Record)?.trim_start(),
        })
    }

    pub fn number(self) -> Result<usize, Reject> {
        self.0.parse().map_err(|_| Reject::Record)
    }

    pub fn is(self, literal: &str) -> bool {
        self.0 == literal
    }

    pub fn chars(self) -> Result<Chars<'a>, Reject> {
        let text = self
            .0
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .ok_or(Reject::Record)?;
        Ok(Chars { rest: text })
    }

    pub fn equals(self, text: &str) -> Result<bool, Reject> {
        let mut actual = self.chars()?;
        for expected in text.chars() {
            if actual.next().transpose()? != Some(expected) {
                return Ok(false);
            }
        }
        Ok(actual.next().transpose()?.is_none())
    }

    pub fn plain(self) -> Result<&'a str, Reject> {
        let text = self
            .0
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .ok_or(Reject::Record)?;
        if text.contains('\\') {
            Err(Reject::Record)
        } else {
            Ok(text)
        }
    }
}

pub(super) struct Array<'a> {
    rest: &'a str,
}
impl<'a> Iterator for Array<'a> {
    type Item = Result<Value<'a>, Reject>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.starts_with(']') || self.rest.is_empty() {
            return None;
        }
        let end = match scan(self.rest, 0) {
            Ok(end) => end,
            Err(error) => {
                self.rest = "";
                return Some(Err(error));
            }
        };
        let value = Value(&self.rest[..end]);
        self.rest = self.rest[end..].trim_start();
        if let Some(next) = self.rest.strip_prefix(',') {
            self.rest = next.trim_start();
        }
        Some(Ok(value))
    }
}

pub(super) struct Chars<'a> {
    rest: &'a str,
}
impl Iterator for Chars<'_> {
    type Item = Result<char, Reject>;
    fn next(&mut self) -> Option<Self::Item> {
        let first = self.rest.chars().next()?;
        self.rest = &self.rest[first.len_utf8()..];
        if first != '\\' {
            return Some(Ok(first));
        }
        let escaped = self.rest.as_bytes().first().copied()?;
        if !escaped.is_ascii() {
            return Some(Err(Reject::Record));
        }
        self.rest = &self.rest[1..];
        Some(match escaped {
            b'"' => Ok('"'),
            b'\\' => Ok('\\'),
            b'/' => Ok('/'),
            b'b' => Ok('\u{8}'),
            b'f' => Ok('\u{c}'),
            b'n' => Ok('\n'),
            b'r' => Ok('\r'),
            b't' => Ok('\t'),
            b'u' => self.unicode(),
            _ => Err(Reject::Record),
        })
    }
}
impl Chars<'_> {
    fn hex(&mut self) -> Result<u32, Reject> {
        let text = self.rest.get(..4).ok_or(Reject::Record)?;
        let value = u32::from_str_radix(text, 16).map_err(|_| Reject::Record)?;
        self.rest = &self.rest[4..];
        Ok(value)
    }
    fn unicode(&mut self) -> Result<char, Reject> {
        let high = self.hex()?;
        let scalar = if (0xd800..=0xdbff).contains(&high) {
            self.rest = self.rest.strip_prefix("\\u").ok_or(Reject::Record)?;
            let low = self.hex()?;
            if !(0xdc00..=0xdfff).contains(&low) {
                return Err(Reject::Record);
            }
            0x10000 + ((high - 0xd800) << 10) + low - 0xdc00
        } else {
            high
        };
        char::from_u32(scalar).ok_or(Reject::Record)
    }
}

fn string_end(text: &str) -> Result<usize, Reject> {
    if !text.starts_with('"') {
        return Err(Reject::Record);
    }
    let mut escaped = false;
    for (index, byte) in text.bytes().enumerate().skip(1) {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            for scalar in Value(&text[..=index]).chars()? {
                scalar?;
            }
            return Ok(index + 1);
        } else if byte < 32 {
            return Err(Reject::Record);
        }
    }
    Err(Reject::Record)
}

fn scan(text: &str, depth: usize) -> Result<usize, Reject> {
    if depth > 32 {
        return Err(Reject::Record);
    }
    let bytes = text.as_bytes();
    match bytes.first().copied().ok_or(Reject::Record)? {
        b'"' => string_end(text),
        b'{' | b'[' => {
            let object = bytes[0] == b'{';
            let closing = if object { b'}' } else { b']' };
            let mut at = 1;
            skip_space(bytes, &mut at);
            if bytes.get(at) == Some(&closing) {
                return Ok(at + 1);
            }
            loop {
                if object {
                    at += string_end(&text[at..])?;
                    skip_space(bytes, &mut at);
                    if bytes.get(at) != Some(&b':') {
                        return Err(Reject::Record);
                    }
                    at += 1;
                    skip_space(bytes, &mut at);
                }
                at += scan(&text[at..], depth + 1)?;
                skip_space(bytes, &mut at);
                match bytes.get(at) {
                    Some(b',') => {
                        at += 1;
                        skip_space(bytes, &mut at);
                    }
                    Some(byte) if *byte == closing => return Ok(at + 1),
                    _ => return Err(Reject::Record),
                }
            }
        }
        b't' if text.starts_with("true") => Ok(4),
        b'f' if text.starts_with("false") => Ok(5),
        b'n' if text.starts_with("null") => Ok(4),
        b'0'..=b'9' => {
            let end = bytes
                .iter()
                .position(|b| !b.is_ascii_digit())
                .unwrap_or(bytes.len());
            if end > 1 && bytes[0] == b'0' {
                return Err(Reject::Record);
            }
            Ok(end)
        }
        _ => Err(Reject::Record),
    }
}
fn skip_space(bytes: &[u8], at: &mut usize) {
    while bytes
        .get(*at)
        .is_some_and(|b| matches!(b, b' ' | b'\r' | b'\n' | b'\t'))
    {
        *at += 1;
    }
}
