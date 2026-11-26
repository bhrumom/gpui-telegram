//! Lexer for `CodeSceneScript` (`cs1`).
//!
//! Supports decimal/scientific/0x-hex numbers, UTF-8 strings with escapes,
//! `//` line and `/* */` block comments, compound assignment operators,
//! ranges (`..`, `..=`), and fail-closed `max_source_bytes` / `max_str_bytes`.

use std::sync::Arc;

use crate::lang::limits::VmBudgets;
use crate::model::{CompileDiagnostic, DiagnosticStage, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Keywords
    Let,
    Fn,
    If,
    Else,
    While,
    For,
    In,
    Break,
    Continue,
    Return,
    True,
    False,
    Null,
    // Literals & identifiers
    Num(f64),
    Str(Arc<str>),
    Ident(String),
    // Single-char punctuation & operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    Eq,
    Lt,
    Gt,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Semicolon,
    Dot,
    // Multi-char operators
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    EqEq,
    BangEq,
    LtEq,
    GtEq,
    AndAnd,
    OrOr,
    DotDot,
    DotDotEq,
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    /// True if one or more newlines preceded this token (supports optional semicolons).
    pub after_newline: bool,
}

pub fn tokenize(source: &str, budgets: &VmBudgets) -> Result<Vec<Token>, CompileDiagnostic> {
    if source.len() > budgets.max_source_bytes {
        return Err(CompileDiagnostic::new(
            DiagnosticStage::Lex,
            "E_SOURCE_TOO_LARGE",
            format!(
                "source size {} bytes exceeds limit {} bytes",
                source.len(),
                budgets.max_source_bytes
            ),
            Some(Span { line: 1, col: 1 }),
        ));
    }

    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut line = 1usize;
    let mut col = 1usize;
    let mut tokens = Vec::with_capacity((len / 4).max(16));
    let mut saw_newline = false;

    while i < len {
        let b = bytes[i];
        // Whitespace
        if b == b' ' || b == b'\t' || b == b'\r' {
            i += 1;
            col += 1;
            continue;
        }
        if b == b'\n' {
            i += 1;
            line += 1;
            col = 1;
            saw_newline = true;
            continue;
        }
        // Line comment //
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'/' {
            i += 2;
            col += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
                col += 1;
            }
            continue;
        }
        // Block comment /* ... */
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            let start_span = Span { line, col };
            i += 2;
            col += 2;
            let mut closed = false;
            while i + 1 < len {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    col += 2;
                    closed = true;
                    break;
                }
                if bytes[i] == b'\n' {
                    line += 1;
                    col = 1;
                    saw_newline = true;
                } else {
                    col += 1;
                }
                i += 1;
            }
            if !closed {
                return Err(CompileDiagnostic::new(
                    DiagnosticStage::Lex,
                    "E_UNTERMINATED_COMMENT",
                    "unterminated block comment `/*`",
                    Some(start_span),
                ));
            }
            continue;
        }

        let span = Span { line, col };
        let after_newline = saw_newline;
        saw_newline = false;

        // String literal
        if b == b'"' {
            let (s, next_i, next_line, next_col) = lex_string(source, i, line, col, budgets)?;
            i = next_i;
            line = next_line;
            col = next_col;
            tokens.push(Token {
                kind: TokenKind::Str(Arc::from(s)),
                span,
                after_newline,
            });
            continue;
        }

        // Number literal (decimal, scientific, or 0x hex)
        if b.is_ascii_digit() {
            let (n, next_i, next_col) = lex_number(source, i, line, col)?;
            i = next_i;
            col = next_col;
            tokens.push(Token {
                kind: TokenKind::Num(n),
                span,
                after_newline,
            });
            continue;
        }

        // Identifier / keyword
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
                col += 1;
            }
            let ident = &source[start..i];
            let kind = match ident {
                "let" => TokenKind::Let,
                "fn" => TokenKind::Fn,
                "if" => TokenKind::If,
                "else" => TokenKind::Else,
                "while" => TokenKind::While,
                "for" => TokenKind::For,
                "in" => TokenKind::In,
                "break" => TokenKind::Break,
                "continue" => TokenKind::Continue,
                "return" => TokenKind::Return,
                "true" => TokenKind::True,
                "false" => TokenKind::False,
                "null" => TokenKind::Null,
                _ => {
                    if ident.len() > budgets.max_str_bytes {
                        return Err(CompileDiagnostic::new(
                            DiagnosticStage::Lex,
                            "E_STR_TOO_LONG",
                            format!(
                                "identifier exceeds max length {} bytes",
                                budgets.max_str_bytes
                            ),
                            Some(span),
                        ));
                    }
                    TokenKind::Ident(ident.to_string())
                }
            };
            tokens.push(Token {
                kind,
                span,
                after_newline,
            });
            continue;
        }

        // Operators & punctuation
        let (kind, advance) = lex_punct(bytes, i, span)?;
        i += advance;
        col += advance;
        tokens.push(Token {
            kind,
            span,
            after_newline,
        });
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        span: Span { line, col },
        after_newline: saw_newline,
    });
    Ok(tokens)
}

fn lex_string(
    source: &str,
    start_idx: usize,
    mut line: usize,
    mut col: usize,
    budgets: &VmBudgets,
) -> Result<(String, usize, usize, usize), CompileDiagnostic> {
    let start_span = Span { line, col };
    let mut out = String::new();
    let mut chars = source[start_idx + 1..].char_indices();
    col += 1;

    while let Some((rel, ch)) = chars.next() {
        let abs = start_idx + 1 + rel;
        match ch {
            '"' => {
                col += 1;
                return Ok((out, abs + 1, line, col));
            }
            '\\' => {
                col += 1;
                let Some((_, esc)) = chars.next() else {
                    return Err(CompileDiagnostic::new(
                        DiagnosticStage::Lex,
                        "E_UNTERMINATED_STR",
                        "unterminated escape sequence in string literal",
                        Some(start_span),
                    ));
                };
                col += 1;
                let resolved = match esc {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    '\\' => '\\',
                    '"' => '"',
                    '\'' => '\'',
                    '0' => '\0',
                    other => other,
                };
                out.push(resolved);
            }
            '\n' => {
                line += 1;
                col = 1;
                out.push('\n');
            }
            other => {
                col += 1;
                out.push(other);
            }
        }
        if out.len() > budgets.max_str_bytes {
            return Err(CompileDiagnostic::new(
                DiagnosticStage::Lex,
                "E_STR_TOO_LONG",
                format!(
                    "string literal exceeds max length {} bytes",
                    budgets.max_str_bytes
                ),
                Some(start_span),
            ));
        }
    }

    Err(CompileDiagnostic::new(
        DiagnosticStage::Lex,
        "E_UNTERMINATED_STR",
        "unterminated string literal",
        Some(start_span),
    ))
}

fn lex_number(
    source: &str,
    start_idx: usize,
    line: usize,
    mut col: usize,
) -> Result<(f64, usize, usize), CompileDiagnostic> {
    let span = Span { line, col };
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut i = start_idx;

    // Hex literal: 0x... / 0X...
    if bytes[i] == b'0' && i + 1 < len && (bytes[i + 1] == b'x' || bytes[i + 1] == b'X') {
        i += 2;
        col += 2;
        let hex_start = i;
        while i < len && (bytes[i].is_ascii_hexdigit() || bytes[i] == b'_') {
            i += 1;
            col += 1;
        }
        let hex_clean: String = source[hex_start..i].chars().filter(|&c| c != '_').collect();
        if hex_clean.is_empty() {
            return Err(CompileDiagnostic::new(
                DiagnosticStage::Lex,
                "E_INVALID_NUMBER",
                "expected hexadecimal digits after `0x`",
                Some(span),
            ));
        }
        let val = u64::from_str_radix(&hex_clean, 16).map_err(|_| {
            CompileDiagnostic::new(
                DiagnosticStage::Lex,
                "E_INVALID_NUMBER",
                format!("invalid hexadecimal literal `0x{hex_clean}`"),
                Some(span),
            )
        })?;
        return Ok((val as f64, i, col));
    }

    while i < len && bytes[i].is_ascii_digit() {
        i += 1;
        col += 1;
    }
    // Fractional part only if `.` is NOT followed by another `.` (range operator `0..10`)
    if i < len && bytes[i] == b'.' && !(i + 1 < len && bytes[i + 1] == b'.') {
        if i + 1 < len && bytes[i + 1].is_ascii_digit() {
            i += 1;
            col += 1;
            while i < len && bytes[i].is_ascii_digit() {
                i += 1;
                col += 1;
            }
        }
    }
    // Scientific notation
    if i < len && (bytes[i] == b'e' || bytes[i] == b'E') {
        let save_i = i;
        let save_col = col;
        i += 1;
        col += 1;
        if i < len && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
            col += 1;
        }
        let exp_start = i;
        while i < len && bytes[i].is_ascii_digit() {
            i += 1;
            col += 1;
        }
        if i == exp_start {
            i = save_i;
            col = save_col;
        }
    }

    let raw = &source[start_idx..i];
    let parsed = raw.parse::<f64>().map_err(|_| {
        CompileDiagnostic::new(
            DiagnosticStage::Lex,
            "E_INVALID_NUMBER",
            format!("invalid numeric literal `{raw}`"),
            Some(span),
        )
    })?;
    Ok((parsed, i, col))
}

fn lex_punct(bytes: &[u8], i: usize, span: Span) -> Result<(TokenKind, usize), CompileDiagnostic> {
    let b0 = bytes[i];
    let b1 = bytes.get(i + 1).copied().unwrap_or(0);
    let b2 = bytes.get(i + 2).copied().unwrap_or(0);

    match (b0, b1, b2) {
        (b'.', b'.', b'=') => Ok((TokenKind::DotDotEq, 3)),
        (b'.', b'.', _) => Ok((TokenKind::DotDot, 2)),
        (b'+', b'=', _) => Ok((TokenKind::PlusEq, 2)),
        (b'-', b'=', _) => Ok((TokenKind::MinusEq, 2)),
        (b'*', b'=', _) => Ok((TokenKind::StarEq, 2)),
        (b'/', b'=', _) => Ok((TokenKind::SlashEq, 2)),
        (b'%', b'=', _) => Ok((TokenKind::PercentEq, 2)),
        (b'=', b'=', _) => Ok((TokenKind::EqEq, 2)),
        (b'!', b'=', _) => Ok((TokenKind::BangEq, 2)),
        (b'<', b'=', _) => Ok((TokenKind::LtEq, 2)),
        (b'>', b'=', _) => Ok((TokenKind::GtEq, 2)),
        (b'&', b'&', _) => Ok((TokenKind::AndAnd, 2)),
        (b'|', b'|', _) => Ok((TokenKind::OrOr, 2)),
        (b'+', _, _) => Ok((TokenKind::Plus, 1)),
        (b'-', _, _) => Ok((TokenKind::Minus, 1)),
        (b'*', _, _) => Ok((TokenKind::Star, 1)),
        (b'/', _, _) => Ok((TokenKind::Slash, 1)),
        (b'%', _, _) => Ok((TokenKind::Percent, 1)),
        (b'!', _, _) => Ok((TokenKind::Bang, 1)),
        (b'=', _, _) => Ok((TokenKind::Eq, 1)),
        (b'<', _, _) => Ok((TokenKind::Lt, 1)),
        (b'>', _, _) => Ok((TokenKind::Gt, 1)),
        (b'(', _, _) => Ok((TokenKind::LParen, 1)),
        (b')', _, _) => Ok((TokenKind::RParen, 1)),
        (b'[', _, _) => Ok((TokenKind::LBracket, 1)),
        (b']', _, _) => Ok((TokenKind::RBracket, 1)),
        (b'{', _, _) => Ok((TokenKind::LBrace, 1)),
        (b'}', _, _) => Ok((TokenKind::RBrace, 1)),
        (b',', _, _) => Ok((TokenKind::Comma, 1)),
        (b':', _, _) => Ok((TokenKind::Colon, 1)),
        (b';', _, _) => Ok((TokenKind::Semicolon, 1)),
        (b'.', _, _) => Ok((TokenKind::Dot, 1)),
        _ => Err(CompileDiagnostic::new(
            DiagnosticStage::Lex,
            "E_UNEXPECTED_CHAR",
            format!("unexpected character `{}`", b0 as char),
            Some(span),
        )),
    }
}
