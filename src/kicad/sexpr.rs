//! Lossless S-expression spans for merging libraries without reserializing geometry.
use std::ops::Range;

use crate::error::{AppError, Result};

#[derive(Debug)]
pub(crate) struct Node {
    pub span: Range<usize>,
    pub atom: Option<String>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn head(&self) -> Option<&str> {
        self.value(0)
    }

    pub fn value(&self, index: usize) -> Option<&str> {
        self.children.get(index)?.atom.as_deref()
    }
}

pub(crate) fn parse(text: &str) -> Result<Node> {
    let mut offset = 0;
    whitespace(text, &mut offset);
    let node = node(text, &mut offset, 0)?;
    whitespace(text, &mut offset);
    if offset != text.len() || node.atom.is_some() {
        return Err(AppError::Other("expected one S-expression document".into()));
    }
    Ok(node)
}

fn whitespace(text: &str, offset: &mut usize) {
    while text
        .as_bytes()
        .get(*offset)
        .is_some_and(u8::is_ascii_whitespace)
    {
        *offset += 1;
    }
}

fn node(text: &str, offset: &mut usize, depth: usize) -> Result<Node> {
    if depth > 256 {
        return Err(AppError::Other(
            "S-expression nesting exceeds 256 levels".into(),
        ));
    }
    whitespace(text, offset);
    let start = *offset;
    let bytes = text.as_bytes();
    let mut children = Vec::new();
    let atom = match bytes.get(*offset) {
        Some(b'(') => {
            *offset += 1;
            loop {
                whitespace(text, offset);
                match bytes.get(*offset) {
                    Some(b')') => {
                        *offset += 1;
                        break;
                    }
                    None => return Err(AppError::Other("unclosed S-expression".into())),
                    _ => children.push(node(text, offset, depth + 1)?),
                }
            }
            None
        }
        Some(b'"') => {
            *offset += 1;
            let mut value = String::new();
            loop {
                let ch = text[*offset..]
                    .chars()
                    .next()
                    .ok_or_else(|| AppError::Other("unclosed quoted string".into()))?;
                *offset += ch.len_utf8();
                match ch {
                    '"' => break,
                    '\\' => {
                        let escaped = text[*offset..]
                            .chars()
                            .next()
                            .ok_or_else(|| AppError::Other("incomplete escape".into()))?;
                        *offset += escaped.len_utf8();
                        match escaped {
                            'n' => value.push('\n'),
                            'r' => value.push('\r'),
                            't' => value.push('\t'),
                            '\\' | '"' => value.push(escaped),
                            _ => {
                                value.push('\\');
                                value.push(escaped);
                            }
                        }
                    }
                    _ => value.push(ch),
                }
            }
            Some(value)
        }
        None | Some(b')') => return Err(AppError::Other("unexpected end or ')'".into())),
        _ => {
            while bytes
                .get(*offset)
                .is_some_and(|b| !b.is_ascii_whitespace() && !matches!(b, b'(' | b')' | b'"'))
            {
                *offset += 1;
            }
            Some(text[start..*offset].to_owned())
        }
    };
    Ok(Node {
        span: start..*offset,
        atom,
        children,
    })
}

pub(crate) fn quote(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}

pub(crate) fn replace(text: &str, mut edits: Vec<(Range<usize>, String)>) -> String {
    edits.sort_by_key(|(span, _)| span.start);
    let mut result = text.to_owned();
    for (span, value) in edits.into_iter().rev() {
        result.replace_range(span, &value);
    }
    result
}
