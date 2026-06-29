/// S-expression lexer and parser for the zscheme evaluator.
/// Ported from ma-agent/src/scheme/parser.rs — no external dependencies.

// ── AST ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum SchemeExpr {
    /// `()` / `nil` — the empty list.
    #[allow(dead_code)]
    Nil,
    /// A quoted string literal: `"hello"`.
    Str(String),
    /// Any other token: symbol, number, keyword, path, actor target, …
    Atom(String),
    /// A parenthesised form: `(f a b …)`.
    List(Vec<SchemeExpr>),
}

// ── Errors ─────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct LexError(pub String);

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── Lexer ──────────────────────────────────────────────────────────────────

/// Tokenise a Scheme source string.
pub fn tokenize(input: &str) -> Result<Vec<String>, LexError> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(&ch) = chars.peek() {
        match ch {
            ' ' | '\t' | '\n' | '\r' => {
                chars.next();
            }
            '(' => {
                chars.next();
                tokens.push("(".to_string());
            }
            ')' => {
                chars.next();
                tokens.push(")".to_string());
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                let mut escaped = false;
                loop {
                    match chars.next() {
                        None => return Err(LexError("unterminated string literal".to_string())),
                        Some('\\') if !escaped => {
                            escaped = true;
                        }
                        Some('"') if !escaped => break,
                        Some(c) => {
                            if escaped {
                                match c {
                                    'n' => s.push('\n'),
                                    't' => s.push('\t'),
                                    'r' => s.push('\r'),
                                    '\\' => s.push('\\'),
                                    '"' => s.push('"'),
                                    other => {
                                        s.push('\\');
                                        s.push(other);
                                    }
                                }
                                escaped = false;
                            } else {
                                s.push(c);
                            }
                        }
                    }
                }
                tokens.push(format!("\"{s}\""));
            }
            ';' => while chars.next().map(|c| c != '\n').unwrap_or(false) {},
            '|' => {
                chars.next();
                tokens.push("|".to_string());
            }
            '\'' => {
                chars.next();
                tokens.push("'QUOTE".to_string());
            }
            _ => {
                let mut atom = String::new();
                while let Some(&c) = chars.peek() {
                    if c == ' '
                        || c == '\t'
                        || c == '\n'
                        || c == '\r'
                        || c == '('
                        || c == ')'
                        || c == ';'
                        || c == '|'
                    {
                        break;
                    }
                    atom.push(c);
                    chars.next();
                }
                if !atom.is_empty() {
                    tokens.push(atom);
                }
            }
        }
    }

    Ok(tokens)
}

// ── Parser ─────────────────────────────────────────────────────────────────

/// Parse one S-expression from `tokens` starting at `pos`.
/// Returns `(expr, next_pos)` on success.
pub fn parse_expr(tokens: &[String], pos: usize) -> Result<(SchemeExpr, usize), LexError> {
    if pos >= tokens.len() {
        return Err(LexError("unexpected end of input".to_string()));
    }

    let token = &tokens[pos];

    if token == "(" {
        let mut forms = Vec::new();
        let mut i = pos + 1;
        loop {
            if i >= tokens.len() {
                return Err(LexError("missing closing ')'".to_string()));
            }
            if tokens[i] == ")" {
                return Ok((SchemeExpr::List(forms), i + 1));
            }
            let (expr, next) = parse_expr(tokens, i)?;
            forms.push(expr);
            i = next;
        }
    } else if token == "'QUOTE" {
        let (inner, next_pos) = parse_expr(tokens, pos + 1)?;
        Ok((
            SchemeExpr::List(vec![SchemeExpr::Atom("quote".to_string()), inner]),
            next_pos,
        ))
    } else if token == ")" {
        Err(LexError("unexpected ')'".to_string()))
    } else if token.starts_with('"') && token.ends_with('"') && token.len() >= 2 {
        let inner = token[1..token.len() - 1].to_string();
        Ok((SchemeExpr::Str(inner), pos + 1))
    } else {
        Ok((SchemeExpr::Atom(token.clone()), pos + 1))
    }
}
