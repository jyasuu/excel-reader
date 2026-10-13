//! A small SQL dialect: `SELECT ... FROM ... WHERE ... GROUP BY ... ORDER BY ... LIMIT`.

use anyhow::{Result, bail};

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub select: Vec<SelectItem>,
    pub from: Option<String>,
    pub filter: Option<Expr>,
    pub group_by: Vec<String>,
    pub order_by: Vec<OrderKey>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectItem {
    pub expr: SelectExpr,
    /// Explicit `AS alias`, if any.
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectExpr {
    /// `*`
    Star,
    /// A bare column reference.
    Column(String),
    /// An aggregate call such as `SUM(D)` or `COUNT(*)`.
    Aggregate { func: AggFunc, arg: AggArg },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFunc {
    Sum,
    Count,
    Avg,
    Min,
    Max,
}

impl AggFunc {
    fn parse(s: &str) -> Option<AggFunc> {
        match s.to_ascii_uppercase().as_str() {
            "SUM" => Some(AggFunc::Sum),
            "COUNT" => Some(AggFunc::Count),
            "AVG" | "AVERAGE" => Some(AggFunc::Avg),
            "MIN" => Some(AggFunc::Min),
            "MAX" => Some(AggFunc::Max),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            AggFunc::Sum => "SUM",
            AggFunc::Count => "COUNT",
            AggFunc::Avg => "AVG",
            AggFunc::Min => "MIN",
            AggFunc::Max => "MAX",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AggArg {
    Star,
    Column(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderKey {
    pub column: String,
    pub desc: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Comparison {
        left: Operand,
        op: CmpOp,
        right: Operand,
    },
    IsNull {
        operand: Operand,
        negated: bool,
    },
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Column(String),
    Literal(Literal),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Number(f64),
    Text(String),
    Bool(bool),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Number(f64),
    Star,
    Comma,
    LParen,
    RParen,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Eof,
}

impl Token {
    fn describe(&self) -> String {
        match self {
            Token::Ident(s) => format!("`{s}`"),
            Token::Str(s) => format!("string '{s}'"),
            Token::Number(n) => format!("number `{n}`"),
            Token::Star => "`*`".into(),
            Token::Comma => "`,`".into(),
            Token::LParen => "`(`".into(),
            Token::RParen => "`)`".into(),
            Token::Eq => "`=`".into(),
            Token::Ne => "`<>`".into(),
            Token::Lt => "`<`".into(),
            Token::Le => "`<=`".into(),
            Token::Gt => "`>`".into(),
            Token::Ge => "`>=`".into(),
            Token::Eof => "end of query".into(),
        }
    }
}

#[derive(Debug, Clone)]
struct Spanned {
    token: Token,
    at: usize,
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Lexer { src: src.as_bytes(), pos: 0 }
    }

    fn tokenize(mut self) -> Result<Vec<Spanned>> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace_and_comments();
            let at = self.pos;
            let Some(c) = self.peek() else {
                tokens.push(Spanned { token: Token::Eof, at });
                return Ok(tokens);
            };
            let token = match c {
                b'*' => {
                    self.bump();
                    Token::Star
                }
                b',' => {
                    self.bump();
                    Token::Comma
                }
                b'(' => {
                    self.bump();
                    Token::LParen
                }
                b')' => {
                    self.bump();
                    Token::RParen
                }
                b'=' => {
                    self.bump();
                    Token::Eq
                }
                b'<' => {
                    self.bump();
                    match self.peek() {
                        Some(b'=') => {
                            self.bump();
                            Token::Le
                        }
                        Some(b'>') => {
                            self.bump();
                            Token::Ne
                        }
                        _ => Token::Lt,
                    }
                }
                b'>' => {
                    self.bump();
                    if self.peek() == Some(b'=') {
                        self.bump();
                        Token::Ge
                    } else {
                        Token::Gt
                    }
                }
                b'!' => {
                    self.bump();
                    if self.peek() == Some(b'=') {
                        self.bump();
                        Token::Ne
                    } else {
                        bail!("unexpected character '!' at offset {at} (did you mean `!=`?)");
                    }
                }
                b'\'' => Token::Str(self.read_quoted(b'\'')?),
                b'"' => Token::Ident(self.read_quoted(b'"')?),
                b'`' => Token::Ident(self.read_quoted(b'`')?),
                b'[' => Token::Ident(self.read_bracketed()?),
                b if b.is_ascii_digit()
                    || (b == b'.' && self.peek_at(1).is_some_and(|n| n.is_ascii_digit())) =>
                {
                    Token::Number(self.read_number(at)?)
                }
                b if is_ident_start(b) => Token::Ident(self.read_ident()),
                other => bail!("unexpected character '{}' at offset {at}", other as char),
            };
            tokens.push(Spanned { token, at });
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, n: usize) -> Option<u8> {
        self.src.get(self.pos + n).copied()
    }

    fn bump(&mut self) {
        self.pos += 1;
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.bump();
            }
            if self.peek() == Some(b'-') && self.peek_at(1) == Some(b'-') {
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.bump();
                }
                continue;
            }
            break;
        }
    }

    fn read_ident(&mut self) -> String {
        let start = self.pos;
        while self.peek().is_some_and(is_ident_continue) {
            self.bump();
        }
        String::from_utf8_lossy(&self.src[start..self.pos]).into_owned()
    }

    fn read_quoted(&mut self, quote: u8) -> Result<String> {
        let at = self.pos;
        self.bump(); // opening quote
        let mut out = String::new();
        loop {
            match self.peek() {
                None => bail!("unterminated quoted value starting at offset {at}"),
                Some(c) if c == quote => {
                    self.bump();
                    // Doubled quote is an escaped quote.
                    if self.peek() == Some(quote) {
                        self.bump();
                        out.push(quote as char);
                        continue;
                    }
                    return Ok(out);
                }
                Some(_) => {
                    // Copy one UTF-8 character.
                    let start = self.pos;
                    self.bump();
                    while self.peek().is_some_and(|c| (c & 0xC0) == 0x80) {
                        self.bump();
                    }
                    out.push_str(&String::from_utf8_lossy(&self.src[start..self.pos]));
                }
            }
        }
    }

    fn read_bracketed(&mut self) -> Result<String> {
        let at = self.pos;
        self.bump(); // `[`
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c == b']' {
                let text = String::from_utf8_lossy(&self.src[start..self.pos]).into_owned();
                self.bump();
                return Ok(text);
            }
            self.bump();
        }
        bail!("unterminated `[identifier]` starting at offset {at}")
    }

    fn read_number(&mut self, at: usize) -> Result<f64> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
        }
        if self.peek() == Some(b'.') {
            self.bump();
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.bump();
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let save = self.pos;
            self.bump();
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.bump();
            }
            if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.bump();
                }
            } else {
                self.pos = save;
            }
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("");
        text.parse::<f64>()
            .map_err(|_| anyhow::anyhow!("invalid number `{text}` at offset {at}"))
    }
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

fn is_ident_continue(c: u8) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser {
    tokens: Vec<Spanned>,
    pos: usize,
}

impl Parser {
    /// Parse a complete query string.
    pub fn parse(src: &str) -> Result<Query> {
        let tokens = Lexer::new(src).tokenize()?;
        let mut parser = Parser { tokens, pos: 0 };
        let query = parser.parse_query()?;
        parser.expect_eof()?;
        Ok(query)
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos].token
    }

    fn at(&self) -> usize {
        self.tokens[self.pos].at
    }

    fn bump(&mut self) -> Token {
        let token = self.tokens[self.pos].token.clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        token
    }

    fn eat_keyword(&mut self, kw: &str) -> bool {
        if let Token::Ident(s) = self.peek()
            && s.eq_ignore_ascii_case(kw)
        {
            self.bump();
            return true;
        }
        false
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<()> {
        if self.eat_keyword(kw) {
            Ok(())
        } else {
            bail!("expected `{kw}` but found {} at offset {}", self.peek().describe(), self.at())
        }
    }

    fn eat(&mut self, want: &Token) -> bool {
        if self.peek() == want {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, want: &Token) -> Result<()> {
        if self.eat(want) {
            Ok(())
        } else {
            bail!("expected {} but found {} at offset {}", want.describe(), self.peek().describe(), self.at())
        }
    }

    fn expect_eof(&mut self) -> Result<()> {
        if self.peek() == &Token::Eof {
            Ok(())
        } else {
            bail!("unexpected {} at offset {} after end of query", self.peek().describe(), self.at())
        }
    }

    fn expect_ident(&mut self) -> Result<String> {
        match self.peek().clone() {
            Token::Ident(s) => {
                self.bump();
                Ok(s)
            }
            other => bail!("expected an identifier but found {} at offset {}", other.describe(), self.at()),
        }
    }

    fn parse_query(&mut self) -> Result<Query> {
        self.expect_keyword("select")?;
        let select = self.parse_select_list()?;

        let from = if self.eat_keyword("from") {
            let name = self.expect_ident()?;
            // Optional `AS alias` / bare alias after the table name.
            if self.eat_keyword("as") {
                let _ = self.expect_ident()?;
            } else if matches!(self.peek(), Token::Ident(s) if !is_clause_keyword(s)) {
                self.bump();
            }
            Some(name)
        } else {
            None
        };

        let filter = if self.eat_keyword("where") {
            Some(self.parse_expr()?)
        } else {
            None
        };

        let mut group_by = Vec::new();
        if self.eat_keyword("group") {
            self.expect_keyword("by")?;
            group_by = self.parse_ident_list()?;
        }

        // HAVING is accepted and ignored only if it is followed by nothing we
        // understand; better to give a clear error than silently ignore.
        if self.eat_keyword("having") {
            bail!("HAVING is not supported; filter with WHERE instead");
        }

        let mut order_by = Vec::new();
        if self.eat_keyword("order") {
            self.expect_keyword("by")?;
            order_by = self.parse_order_list()?;
        }

        let limit = if self.eat_keyword("limit") {
            match self.bump() {
                Token::Number(n) if n >= 0.0 => Some(n as usize),
                other => bail!("LIMIT expects a non-negative integer but found {} at offset {}", other.describe(), self.at()),
            }
        } else {
            None
        };

        Ok(Query { select, from, filter, group_by, order_by, limit })
    }

    fn parse_select_list(&mut self) -> Result<Vec<SelectItem>> {
        let mut items = Vec::new();
        loop {
            items.push(self.parse_select_item()?);
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        Ok(items)
    }

    fn parse_select_item(&mut self) -> Result<SelectItem> {
        if self.eat(&Token::Star) {
            return Ok(SelectItem { expr: SelectExpr::Star, alias: None });
        }

        let expr = match self.peek().clone() {
            Token::Ident(name) => {
                if let Some(func) = AggFunc::parse(&name) {
                    // Only treat as a call when followed by `(`.
                    if self.tokens.get(self.pos + 1).map(|t| &t.token) == Some(&Token::LParen) {
                        self.bump(); // function name
                        self.bump(); // `(`
                        let arg = if self.eat(&Token::Star) {
                            AggArg::Star
                        } else {
                            AggArg::Column(self.expect_ident()?)
                        };
                        self.expect(&Token::RParen)?;
                        SelectExpr::Aggregate { func, arg }
                    } else {
                        self.bump();
                        SelectExpr::Column(name)
                    }
                } else {
                    self.bump();
                    SelectExpr::Column(name)
                }
            }
            other => {
                bail!("expected a column or aggregate function but found {} at offset {}", other.describe(), self.at())
            }
        };

        let alias = if self.eat_keyword("as") {
            Some(self.expect_ident()?)
        } else if matches!(self.peek(), Token::Ident(s) if !is_clause_keyword(s)) {
            match self.bump() {
                Token::Ident(s) => Some(s),
                _ => unreachable!(),
            }
        } else {
            None
        };

        Ok(SelectItem { expr, alias })
    }

    fn parse_ident_list(&mut self) -> Result<Vec<String>> {
        let mut names = vec![self.expect_ident()?];
        while self.eat(&Token::Comma) {
            names.push(self.expect_ident()?);
        }
        Ok(names)
    }

    fn parse_order_list(&mut self) -> Result<Vec<OrderKey>> {
        let mut keys = Vec::new();
        loop {
            let column = self.expect_ident()?;
            let desc = if self.eat_keyword("desc") {
                true
            } else {
                self.eat_keyword("asc");
                false
            };
            keys.push(OrderKey { column, desc });
            if !self.eat(&Token::Comma) {
                break;
            }
        }
        Ok(keys)
    }

    // -- expression grammar: OR < AND < NOT < primary -----------------------

    fn parse_expr(&mut self) -> Result<Expr> {
        let mut left = self.parse_and()?;
        while self.eat_keyword("or") {
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr> {
        let mut left = self.parse_unary()?;
        while self.eat_keyword("and") {
            let right = self.parse_unary()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr> {
        if self.eat_keyword("not") {
            return Ok(Expr::Not(Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        if self.eat(&Token::LParen) {
            let expr = self.parse_expr()?;
            self.expect(&Token::RParen)?;
            return Ok(expr);
        }

        let left = self.parse_operand()?;

        if self.eat_keyword("is") {
            let negated = self.eat_keyword("not");
            self.expect_keyword("null")?;
            return Ok(Expr::IsNull { operand: left, negated });
        }

        let op = match self.peek() {
            Token::Eq => CmpOp::Eq,
            Token::Ne => CmpOp::Ne,
            Token::Lt => CmpOp::Lt,
            Token::Le => CmpOp::Le,
            Token::Gt => CmpOp::Gt,
            Token::Ge => CmpOp::Ge,
            other => {
                bail!("expected a comparison operator but found {} at offset {}", other.describe(), self.at())
            }
        };
        self.bump();
        let right = self.parse_operand()?;
        Ok(Expr::Comparison { left, op, right })
    }

    fn parse_operand(&mut self) -> Result<Operand> {
        let operand = match self.peek().clone() {
            Token::Number(n) => {
                self.bump();
                Operand::Literal(Literal::Number(n))
            }
            Token::Str(s) => {
                self.bump();
                Operand::Literal(Literal::Text(s))
            }
            Token::Ident(name) => {
                self.bump();
                match name.to_ascii_uppercase().as_str() {
                    "TRUE" => Operand::Literal(Literal::Bool(true)),
                    "FALSE" => Operand::Literal(Literal::Bool(false)),
                    "NULL" => Operand::Literal(Literal::Null),
                    _ => Operand::Column(name),
                }
            }
            other => {
                bail!("expected a column or literal but found {} at offset {}", other.describe(), self.at())
            }
        };
        Ok(operand)
    }
}

/// Keywords that may legally follow a select item / table name, so the parser
/// does not misinterpret them as an alias.
fn is_clause_keyword(word: &str) -> bool {
    matches!(
        word.to_ascii_uppercase().as_str(),
        "FROM" | "WHERE" | "GROUP" | "ORDER" | "LIMIT" | "HAVING" | "ASC" | "DESC"
            | "AND" | "OR" | "NOT" | "IS" | "AS" | "BY"
    )
}

/// Parse a query string into a [`Query`].
pub fn parse(src: &str) -> Result<Query> {
    Parser::parse(src)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_readme_query() {
        let q = parse("select A,B,C,SUM(D) from data group by A,B,C").unwrap();
        assert_eq!(q.from.as_deref(), Some("data"));
        assert_eq!(q.group_by, vec!["A", "B", "C"]);
        assert_eq!(q.select.len(), 4);
        assert_eq!(
            q.select[3].expr,
            SelectExpr::Aggregate { func: AggFunc::Sum, arg: AggArg::Column("D".into()) }
        );
    }

    #[test]
    fn parses_where_order_limit() {
        let q = parse(
            "SELECT region, COUNT(*) AS n FROM data \
             WHERE amount > 100 AND region <> 'x' ORDER BY n DESC LIMIT 5",
        )
        .unwrap();
        assert!(q.filter.is_some());
        assert_eq!(q.limit, Some(5));
        assert_eq!(q.order_by[0].column, "n");
        assert!(q.order_by[0].desc);
        assert_eq!(q.select[1].alias.as_deref(), Some("n"));
    }

    #[test]
    fn parses_parenthesised_boolean_logic() {
        let q = parse("select * from t where (a = 1 or b = 2) and c is not null").unwrap();
        assert!(matches!(q.filter, Some(Expr::And(_, _))));
    }

    #[test]
    fn rejects_incomplete_queries() {
        assert!(parse("select a from t where").is_err());
        assert!(parse("select a from t order by").is_err());
        assert!(parse("select a from t limit abc").is_err());
    }
}
