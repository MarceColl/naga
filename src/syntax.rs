//! Lexer and parser. A program is a list of chunks: a line at column 0 starts one,
//! indented lines continue it. Each chunk is a table, a seed, or one clause of a word.
use crate::ast::*;
use crate::num::slice16;
use crate::store::compile_where;
use crate::value::{Name, Value, list};
use std::rc::Rc;
use unicode_general_category::{GeneralCategory as G, get_general_category};

pub const KEYWORDS: [&str; 15] = ["when", "say", "show", "raise", "break", "nop", "true", "false", "nil", "now", "and", "or", "not", "wrap", "unwrap"];
const OPS: [&str; 51] = [
    "->", "//", "\\/", "/\\", "<:", "+{", "-{", ".{", "||", "==", "!=", "<=", ">=", "&&", "++", "‖", "≤", "≥", "≠", "¬", "∧", "∨", "→", "⊂", "⊃",
    "⫽", "⊔", "⊓", "⊑", "∆", "⍎", "(", ")", "[", "]", "{", "}", ",", "+", "-", "*", "/", "%", "<", ">", "=", "!", ":", ".", "@", "^",
];
const OP_SEMI: &str = ";";

fn canon(op: &'static str) -> &'static str {
    match op {
        "||" => "‖", "!=" => "≠", "<=" => "≤", ">=" => "≥", "!" => "¬", "&&" | "∧" => "and", "∨" => "or",
        "->" => "→", "//" => "⫽", "\\/" => "⊔", "/\\" => "⊓", "<:" => "⊑", o => o,
    }
}

pub fn is_keyword(s: &str) -> bool {
    KEYWORDS.contains(&s)
}

fn is_letter(c: char) -> bool {
    matches!(get_general_category(c), G::UppercaseLetter | G::LowercaseLetter | G::TitlecaseLetter | G::ModifierLetter | G::OtherLetter)
}
fn is_digit(c: char) -> bool {
    get_general_category(c) == G::DecimalNumber
}
fn id_start(c: char) -> bool {
    is_letter(c) || c == '_'
}
fn id_cont(c: char) -> bool {
    id_start(c) || is_digit(c)
}
pub fn is_upper(s: &str) -> bool {
    s.chars().next().is_some_and(|c| get_general_category(c) == G::UppercaseLetter)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Tk {
    Num(f64),
    Str(String),
    Id(Name),
    Op(&'static str),
    Hole(String),
}

#[derive(Clone, Debug)]
pub struct Token {
    pub t: Tk,
    pub raw: String,
    pub line: usize,
    pub col: usize,
    pub end: usize,
}

#[derive(Debug)]
pub struct SyntaxErr {
    pub msg: String,
    pub at: Option<(usize, usize)>,
}

type R<T> = Result<T, SyntaxErr>;

fn serr<T>(msg: impl Into<String>, at: Option<(usize, usize)>) -> R<T> {
    Err(SyntaxErr { msg: msg.into(), at })
}

/// Everything before `--`, outside strings.
pub fn strip_comment(line: &str) -> &str {
    let b = line.as_bytes();
    let (mut i, mut in_str) = (0, false);
    while i < b.len() {
        let c = b[i];
        if in_str {
            if c == b'\\' {
                i += 1;
            } else if c == b'"' {
                in_str = false;
            }
        } else if c == b'"' {
            in_str = true;
        } else if c == b'-' && b.get(i + 1) == Some(&b'-') {
            return &line[..i];
        }
        i += 1;
    }
    line
}

pub fn lex_line(src: &str, line: usize) -> R<Vec<Token>> {
    lex(src, line, false)
}

/// `types`: a route's field types, where `?` glued to `]` or `}` marks an optional field
/// (elsewhere it opens a hole).
fn lex(src: &str, line: usize, types: bool) -> R<Vec<Token>> {
    let cs: Vec<char> = src.chars().collect();
    // UTF-16 column of each char index
    let mut col = vec![0; cs.len() + 1];
    for (k, c) in cs.iter().enumerate() {
        col[k + 1] = col[k] + c.len_utf16();
    }
    let raw = |a: usize, b: usize| cs[a..b].iter().collect::<String>();
    let mut toks = vec![];
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c == ' ' || c == '\t' || c == '\r' {
            i += 1;
            continue;
        }
        let mut push = |t: Tk, len: usize, i: &mut usize| {
            toks.push(Token { t, raw: raw(*i, *i + len), line, col: col[*i], end: col[*i + len] });
            *i += len;
        };
        if c.is_ascii_digit() {
            let mut j = i;
            while j < cs.len() && cs[j].is_ascii_digit() {
                j += 1;
            }
            if j + 1 < cs.len() && cs[j] == '.' && cs[j + 1].is_ascii_digit() {
                j += 1;
                while j < cs.len() && cs[j].is_ascii_digit() {
                    j += 1;
                }
            }
            push(Tk::Num(raw(i, j).parse().unwrap()), j - i, &mut i);
            continue;
        }
        if c == '"' {
            let (mut j, mut s) = (i + 1, String::new());
            while j < cs.len() && cs[j] != '"' {
                if cs[j] == '\\' && j + 1 < cs.len() {
                    s.push(cs[j]);
                    s.push(cs[j + 1]);
                    j += 2;
                } else {
                    s.push(cs[j]);
                    j += 1;
                }
            }
            if j >= cs.len() {
                return serr("this string is missing its closing \"", Some((line, col[i])));
            }
            push(Tk::Str(s), j + 1 - i, &mut i);
            continue;
        }
        if c == '?' && i > 0 && (id_cont(cs[i - 1]) || types && matches!(cs[i - 1], ']' | '}')) {
            push(Tk::Op("?"), 1, &mut i);
            continue;
        }
        if c == '?' {
            let rest = raw(i + 1, cs.len()).trim().to_string();
            if rest.is_empty() || rest.starts_with(')') || rest.starts_with('‖') || rest.starts_with("||") {
                push(Tk::Hole(String::new()), 1, &mut i);
                continue;
            }
            toks.push(Token { t: Tk::Hole(rest), raw: raw(i, cs.len()), line, col: col[i], end: col[cs.len()] });
            break;
        }
        if id_start(c) {
            let mut j = i + 1;
            while j < cs.len() && id_cont(cs[j]) {
                j += 1;
            }
            while j + 1 < cs.len() && cs[j] == '-' && id_start(cs[j + 1]) {
                j += 2;
                while j < cs.len() && id_cont(cs[j]) {
                    j += 1;
                }
            }
            push(Tk::Id(raw(i, j).into()), j - i, &mut i);
            continue;
        }
        let at = |o: &str| o.chars().enumerate().all(|(k, oc)| cs.get(i + k) == Some(&oc));
        let Some(op) = OPS.iter().copied().chain([OP_SEMI]).find(|o| at(o)) else {
            return serr(format!("unexpected character “{c}”"), Some((line, col[i])));
        };
        push(Tk::Op(canon(op)), op.chars().count(), &mut i);
    }
    Ok(toks)
}

struct Ts<'a> {
    t: Vec<Token>,
    i: usize,
    lines: &'a [String],
}

impl Ts<'_> {
    fn peek(&self, k: usize) -> Option<&Token> {
        self.t.get(self.i + k)
    }
    fn next(&mut self) -> Option<Token> {
        let t = self.t.get(self.i).cloned();
        self.i += 1;
        t
    }
    fn last(&self) -> Option<&Token> {
        self.i.checked_sub(1).and_then(|k| self.t.get(k))
    }
    fn at_end(&self) -> bool {
        self.i >= self.t.len()
    }
    fn is_op(&self, v: &str, k: usize) -> bool {
        matches!(self.peek(k), Some(Token { t: Tk::Op(o), .. }) if *o == v)
    }
    fn is_id(&self, v: &str) -> bool {
        matches!(self.peek(0), Some(Token { t: Tk::Id(o), .. }) if &**o == v)
    }
    fn err<T>(&self, msg: impl Into<String>, tok: Option<&Token>) -> R<T> {
        let t = tok.or(self.peek(0)).or(self.last());
        serr(msg, t.map(|t| (t.line, t.col)))
    }
    fn expect_op(&mut self, v: &str, what: &str) -> R<Token> {
        let x = self.next();
        match &x {
            Some(Token { t: Tk::Op(o), .. }) if *o == v => Ok(x.unwrap()),
            _ => {
                let found = x.as_ref().map(|t| format!(", found “{}”", t.raw)).unwrap_or_default();
                let what = if what.is_empty() { String::new() } else { format!(" {what}") };
                self.err(format!("expected “{v}”{what}{found}"), x.as_ref())
            }
        }
    }
    fn pos(&self, start: &Token) -> Pos {
        Pos { line: start.line, col: start.col }
    }
    /// Just past the last token taken.
    fn end(&self) -> Pos {
        self.last().map_or_else(Pos::default, |t| Pos { line: t.line, col: t.end })
    }
}

fn id_of(t: &Option<Token>) -> Option<&Name> {
    match t {
        Some(Token { t: Tk::Id(v), .. }) => Some(v),
        _ => None,
    }
}

// ---------- expressions ----------
fn bin_op(tok: Option<&Token>) -> Option<(&'static str, u8)> {
    let op = match &tok?.t {
        Tk::Op(o) => *o,
        Tk::Id(v) => match &**v {
            "and" => "and",
            "or" => "or",
            "diff" => "∆",
            "apply" => "⍎",
            _ => return None,
        },
        _ => return None,
    };
    let bp = match op {
        "or" => 2,
        "and" => 3,
        "==" | "≠" | "<" | ">" | "≤" | "≥" | "⊑" => 5,
        "++" | "+" | "-" | "⫽" | "⊔" | "⊓" | "∆" | "⍎" => 6,
        "*" | "/" | "%" => 7,
        _ => return None,
    };
    Some((op, bp))
}

fn bin(op: &str) -> BinOp {
    use BinOp::*;
    match op {
        "==" => Eq, "≠" => Ne, "<" => Lt, ">" => Gt, "≤" => Le, "≥" => Ge, "⊑" => Within, "++" => Cat, "+" => Add,
        "-" => Sub, "⫽" => Over, "⊔" => Join, "⊓" => Meet, "∆" => Diff, "⍎" => Apply, "*" => Mul, "/" => Div, _ => Rem,
    }
}

fn adjacent(a: Option<&Token>, b: Option<&Token>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a.line == b.line && b.col == a.end)
}

fn expr(ts: &mut Ts, min_bp: u8) -> R<Expr> {
    let mut left = nud(ts)?;
    while let Some((op, bp)) = bin_op(ts.peek(0)) {
        // ⫽{…} and ⊔{…} glued to their brace start a new step, not an infix operator
        if (op == "⫽" || op == "⊔") && ts.is_op("{", 1) && adjacent(ts.peek(0), ts.peek(1)) {
            break;
        }
        if bp <= min_bp {
            break;
        }
        ts.next();
        let right = Box::new(expr(ts, bp)?);
        let l = Box::new(left);
        left = match op {
            "and" => Expr::And(l, right),
            "or" => Expr::Or(l, right),
            _ => Expr::Bin(bin(op), l, right),
        };
    }
    Ok(left)
}

fn postfix(ts: &mut Ts, mut e: Expr) -> R<Expr> {
    loop {
        if ts.is_op(".", 0) && matches!(ts.peek(1), Some(Token { t: Tk::Id(_), .. })) {
            ts.next();
            let key = id_of(&ts.next()).unwrap().clone();
            e = Expr::Get(Box::new(e), key);
            continue;
        }
        // body.{date payee}: projection, written without a space
        if ts.is_op(".{", 0) && adjacent(ts.last(), ts.peek(0)) {
            ts.next();
            e = Expr::Proj(Box::new(e), fields(ts, false, ".{…}")?.into_iter().map(|f| f.name).collect());
            continue;
        }
        return Ok(e);
    }
}

fn nud(ts: &mut Ts) -> R<Expr> {
    let Some(tok) = ts.next() else { return ts.err("this expression ends too early", None) };
    match &tok.t {
        Tk::Num(n) => Ok(Expr::Lit(Value::Num(*n))),
        Tk::Str(_) => {
            let e = string(&tok)?;
            postfix(ts, e)
        }
        Tk::Id(v) => {
            match &**v {
                "true" => return Ok(Expr::Lit(Value::Bool(true))),
                "false" => return Ok(Expr::Lit(Value::Bool(false))),
                "nil" => return Ok(Expr::Lit(Value::Nil)),
                "now" => return Ok(Expr::Now),
                "not" => return Ok(Expr::Not(Box::new(expr(ts, 4)?))),
                _ => {}
            }
            if is_keyword(v) {
                return ts.err(format!("“{v}” can't be used inside an expression"), Some(&tok));
            }
            if is_upper(v) {
                return Ok(Expr::Lit(Value::Const(v.clone())));
            }
            if ts.is_op("(", 0) {
                let Some(f) = Builtin::named(v) else { return ts.err(format!("there's no function called {v}()"), Some(&tok)) };
                ts.next();
                let mut args = vec![];
                while !ts.is_op(")", 0) {
                    if ts.at_end() {
                        return ts.err(format!("{v}( is missing its “)”"), Some(&tok));
                    }
                    args.push(expr(ts, 0)?);
                    if ts.is_op(",", 0) {
                        ts.next();
                    } else if !ts.is_op(")", 0) {
                        return ts.err("expected “,” or “)”", None);
                    }
                }
                ts.next();
                return postfix(ts, Expr::Call(f, args));
            }
            postfix(ts, Expr::Field(v.clone()))
        }
        Tk::Op("(") => {
            let e = expr(ts, 0)?;
            ts.expect_op(")", "")?;
            postfix(ts, e)
        }
        Tk::Op("[") => {
            let mut items = vec![];
            while !ts.is_op("]", 0) {
                if ts.at_end() {
                    return ts.err("this list is missing its “]”", Some(&tok));
                }
                items.push(expr(ts, 0)?);
                if ts.is_op(",", 0) {
                    ts.next();
                } else if !ts.is_op("]", 0) {
                    return ts.err("expected “,” or “]” in the list", None);
                }
            }
            ts.next();
            postfix(ts, Expr::List(items))
        }
        Tk::Op("{") => {
            let mut fs = vec![];
            while !ts.is_op("}", 0) {
                if ts.at_end() {
                    return ts.err("this record is missing its “}”", Some(&tok));
                }
                if ts.is_op(",", 0) {
                    ts.next();
                    continue;
                }
                let id = ts.next();
                let Some(name) = id_of(&id).cloned() else { return ts.err("expected a field name in the record", id.as_ref()) };
                let mut e = Expr::Lit(Value::Bool(true));
                if ts.is_op(":", 0) {
                    ts.next();
                    e = expr(ts, 0)?;
                }
                fs.push((name, e));
            }
            ts.next();
            postfix(ts, Expr::Rec(fs))
        }
        Tk::Op("^") => {
            let mut up = 1;
            while ts.is_op("^", 0) {
                ts.next();
                up += 1;
            }
            let id = ts.next();
            let Some(name) = id_of(&id).cloned() else { return ts.err("^ needs a field name after it, like ^total", id.as_ref()) };
            postfix(ts, Expr::Outer(up, name))
        }
        Tk::Op("-") => Ok(Expr::Neg(Box::new(expr(ts, 8)?))),
        Tk::Op("¬") => Ok(Expr::Not(Box::new(expr(ts, 4)?))),
        _ => ts.err(format!("unexpected “{}”", tok.raw), Some(&tok)),
    }
}

/// A string literal; `{expr}` inside interpolates.
fn string(tok: &Token) -> R<Expr> {
    let Tk::Str(raw) = &tok.t else { unreachable!() };
    let cs: Vec<char> = raw.chars().collect();
    let (mut parts, mut buf, mut i) = (vec![], String::new(), 0);
    let at = Some((tok.line, tok.col));
    while i < cs.len() {
        let c = cs[i];
        if c == '\\' {
            match cs.get(i + 1) {
                Some('n') => buf.push('\n'),
                Some('t') => buf.push('\t'),
                Some(&n) => buf.push(n),
                None => buf.push_str("undefined"),
            }
            i += 2;
            continue;
        }
        if c == '{' {
            let Some(j) = (i..cs.len()).find(|&j| cs[j] == '}') else {
                return serr("a “{” in this string has no matching “}” (write \\{ for a literal brace)", at);
            };
            if !buf.is_empty() {
                parts.push(Part::Text(std::mem::take(&mut buf)));
            }
            let inner: String = cs[i + 1..j].iter().collect();
            let mut its = Ts { t: lex_line(&inner, tok.line)?, i: 0, lines: &[] };
            let e = expr(&mut its, 0)?;
            if let Some(t) = its.peek(0) {
                return serr(format!("unexpected “{}” inside {{…}} in a string", t.raw), at);
            }
            parts.push(Part::Expr(e));
            i = j + 1;
            continue;
        }
        buf.push(c);
        i += 1;
    }
    if !buf.is_empty() || parts.is_empty() {
        parts.push(Part::Text(buf));
    }
    Ok(match parts.as_slice() {
        [Part::Text(s)] => Expr::Lit(Value::Text(s.as_str().into())),
        _ => Expr::Interp(parts),
    })
}

// ---------- patterns ----------
fn literal(ts: &mut Ts) -> R<Value> {
    let Some(t) = ts.next() else { return ts.err("expected a value", None) };
    match &t.t {
        Tk::Num(n) => return Ok(Value::Num(*n)),
        Tk::Op("-") if matches!(ts.peek(0), Some(Token { t: Tk::Num(_), .. })) => {
            let Some(Token { t: Tk::Num(n), .. }) = ts.next() else { unreachable!() };
            return Ok(Value::Num(-n));
        }
        Tk::Str(_) => {
            return match string(&t)? {
                Expr::Lit(v) => Ok(v),
                _ => ts.err("strings in patterns can’t use {…}", Some(&t)),
            };
        }
        Tk::Id(v) => match &**v {
            "true" => return Ok(Value::Bool(true)),
            "false" => return Ok(Value::Bool(false)),
            "nil" => return Ok(Value::Nil),
            v if is_upper(v) => return Ok(Value::Const(v.into())),
            _ => {}
        },
        Tk::Op("[") => {
            let mut items = vec![];
            while !ts.is_op("]", 0) {
                if ts.at_end() {
                    return ts.err("missing “]”", None);
                }
                items.push(literal(ts)?);
                if ts.is_op(",", 0) {
                    ts.next();
                }
            }
            ts.next();
            return Ok(list(items));
        }
        _ => {}
    }
    ts.err(
        format!("a pattern can only compare against a fixed value (a number, \"text\", a Constant, true/false/nil or []); “{}” isn't one — use a when guard instead", t.raw),
        Some(&t),
    )
}

#[derive(Clone, Copy, PartialEq)]
enum Nest {
    Top,
    Inner,
    Query,
}

fn pattern(ts: &mut Ts, nested: Nest) -> R<Vec<PatEntry>> {
    let open = ts.expect_op("{", "")?;
    let mut entries = vec![];
    loop {
        if ts.at_end() {
            return ts.err("this pattern is missing its “}”", Some(&open));
        }
        if ts.is_op("}", 0) {
            ts.next();
            return Ok(entries);
        }
        if ts.is_op(",", 0) {
            ts.next();
            continue;
        }
        let mut mode = Mode::Has;
        if ts.is_op("-", 0) {
            ts.next();
            mode = Mode::Take;
        } else if ts.is_op("¬", 0) {
            ts.next();
            mode = Mode::Absent;
        }
        if nested != Nest::Top && mode == Mode::Take {
            let msg = if nested == Nest::Query { "a where pattern can’t take fields out (-x)" } else { "-field only works at the top level of a pattern" };
            return ts.err(msg, None);
        }
        let id = ts.next();
        let name = match id_of(&id) {
            Some(n) if !is_keyword(n) => n.clone(),
            _ => return ts.err("expected a field name in the pattern", id.as_ref()),
        };
        let mut ent = PatEntry { name, mode, sub: None, lit: None };
        if ts.is_op(":", 0) {
            if mode == Mode::Absent {
                return ts.err(format!("!{} means “no {}”, so it can't have a value", ent.name, ent.name), None);
            }
            ts.next();
            if ts.is_op("{", 0) {
                ent.sub = Some(pattern(ts, if nested == Nest::Top { Nest::Inner } else { nested })?);
            } else {
                ent.lit = Some(literal(ts)?);
            }
        }
        entries.push(ent);
    }
}

// ---------- statements and definitions ----------
fn fields(ts: &mut Ts, with_values: bool, what: &str) -> R<Vec<FieldInit>> {
    let mut out = vec![];
    loop {
        if ts.at_end() {
            return ts.err(format!("{what} is missing its “}}”"), None);
        }
        if ts.is_op("}", 0) {
            ts.next();
            return Ok(out);
        }
        if ts.is_op(",", 0) {
            ts.next();
            continue;
        }
        let id = ts.next();
        let name = match id_of(&id) {
            Some(n) if !is_keyword(n) => n.clone(),
            _ => return ts.err(format!("expected a field name in {what}"), id.as_ref()),
        };
        let mut e = None;
        if ts.is_op(":", 0) {
            if !with_values {
                return ts.err(format!("{what} takes field names only"), None);
            }
            ts.next();
            e = Some(expr(ts, 0)?);
        }
        out.push(FieldInit { name, expr: e });
    }
}

fn names(ts: &mut Ts, what: &str) -> R<Vec<Name>> {
    Ok(fields(ts, false, what)?.into_iter().map(|f| f.name).collect())
}

fn body(ts: &mut Ts, closer: Option<&str>) -> R<Vec<Stmt>> {
    let mut branches: Vec<Vec<Stmt>> = vec![vec![]];
    let start = ts.peek(0).cloned();
    while !ts.at_end() {
        if closer.is_some_and(|c| ts.is_op(c, 0)) {
            break;
        }
        if ts.is_op("‖", 0) {
            let bar = ts.next();
            if branches.last().unwrap().is_empty() {
                return ts.err("‖ needs steps on both sides", bar.as_ref());
            }
            branches.push(vec![]);
            continue;
        }
        if ts.is_op(";", 0) {
            ts.next();
            continue;
        }
        let s = stmt(ts)?;
        branches.last_mut().unwrap().extend(s);
    }
    if branches.len() > 1 {
        if branches.last().unwrap().is_empty() {
            return ts.err("‖ needs steps on both sides", None);
        }
        let pos = ts.pos(start.as_ref().unwrap());
        return Ok(vec![Stmt { kind: StmtKind::Par(branches.into_iter().map(Rc::from).collect()), pos, end: ts.end() }]);
    }
    Ok(branches.pop().unwrap())
}

/// A statement ends where its last token does; one already given an end (inside `( … )`) keeps it.
fn stmt(ts: &mut Ts) -> R<Vec<Stmt>> {
    let mut v = stmt_at(ts)?;
    let end = ts.end();
    for s in v.iter_mut().filter(|s| s.end == Pos::default()) {
        s.end = end;
    }
    Ok(v)
}

fn stmt_at(ts: &mut Ts) -> R<Vec<Stmt>> {
    let start = ts.peek(0).unwrap().clone();
    let pos = ts.pos(&start);
    let mk = |kind| Ok(vec![Stmt { kind, pos, end: Pos::default() }]);
    match &start.t {
        Tk::Hole(note) => {
            ts.next();
            mk(StmtKind::Hole(note.clone()))
        }
        Tk::Op(op) => {
            match *op {
                "+{" => {
                    ts.next();
                    return mk(StmtKind::Add(fields(ts, true, "+{…}")?));
                }
                "-{" => {
                    ts.next();
                    return mk(StmtKind::Del(names(ts, "-{…}")?));
                }
                ".{" => {
                    ts.next();
                    return mk(StmtKind::Pick(names(ts, ".{…}")?));
                }
                "@" => {
                    ts.next();
                    let id = ts.next();
                    let Some(field) = id_of(&id).cloned() else { return ts.err("@ needs a field name, like @card( … )", id.as_ref()) };
                    ts.expect_op("(", &format!("after @{field}"))?;
                    let b = body(ts, Some(")"))?;
                    ts.expect_op(")", &format!("to close @{field}("))?;
                    return mk(StmtKind::Focus(field, b.into()));
                }
                "(" => {
                    ts.next();
                    let b = body(ts, Some(")"))?;
                    ts.expect_op(")", "")?;
                    return Ok(b);
                }
                "⊂" => {
                    ts.next();
                    return mk(wrap(ts, "⊂")?);
                }
                "⊃" => {
                    ts.next();
                    return mk(unwrap(ts, "⊃")?);
                }
                "⫽" | "⊔" => {
                    ts.next();
                    ts.expect_op("{", &format!("after {op}"))?;
                    let fs = fields(ts, true, &format!("{op}{{…}}"))?;
                    return mk(if *op == "⫽" { StmtKind::Over(fs) } else { StmtKind::Unify(fs) });
                }
                "→" => return ts.err("→ renames a field: write old→new", Some(&start)),
                "‖" => return ts.err("‖ needs steps on both sides", Some(&start)),
                _ => {}
            }
            ts.err(format!("unexpected “{}”", start.raw), Some(&start))
        }
        Tk::Id(v) => {
            ts.next();
            match &**v {
                "say" => return mk(StmtKind::Say(expr(ts, 0)?)),
                "raise" => return mk(StmtKind::Raise(expr(ts, 0)?)),
                "show" => return mk(StmtKind::Show),
                "break" => return mk(StmtKind::Break),
                "nop" | "ε" => return Ok(vec![]),
                "wrap" => return mk(wrap(ts, "wrap ")?),
                "unwrap" => return mk(unwrap(ts, "unwrap ")?),
                _ => {}
            }
            if is_keyword(v) {
                return ts.err(format!("“{v}” can't start a step"), Some(&start));
            }
            if is_upper(v) {
                return ts.err(format!("{v} is a constant, not a word — put it in a field, like +{{status: {v}}}"), Some(&start));
            }
            let (n1, n2) = (ts.peek(0).map(|t| t.t.clone()), ts.peek(1).map(|t| t.t.clone()));
            match (n1, n2) {
                (Some(Tk::Op(".")), Some(Tk::Id(op))) => {
                    ts.next();
                    ts.next();
                    let (mut pat, mut guard) = (None, None);
                    if &*op == "where" {
                        if ts.is_op("{", 0) {
                            pat = Some(pattern(ts, Nest::Query)?);
                        }
                        if ts.is_id("when") {
                            ts.next();
                            guard = Some(expr(ts, 0)?);
                        }
                    }
                    mk(StmtKind::Table { table: v.clone(), op, pattern: pat, guard, compiled: None })
                }
                (Some(Tk::Op(sign @ ("+" | "-"))), Some(Tk::Num(n))) => {
                    ts.next();
                    ts.next();
                    mk(StmtKind::Inc(v.clone(), if sign == "+" { n } else { -n }))
                }
                (Some(Tk::Op("=")), _) => {
                    ts.next();
                    mk(StmtKind::Set(v.clone(), expr(ts, 0)?))
                }
                (Some(Tk::Op("→")), _) => {
                    ts.next();
                    let to = ts.next();
                    match id_of(&to) {
                        Some(n) if !is_keyword(n) && !is_upper(n) => mk(StmtKind::Rename(v.clone(), n.clone())),
                        _ => ts.err(format!("→ needs the new field name, like {v}→other"), to.as_ref()),
                    }
                }
                _ => mk(StmtKind::Call(v.clone())),
            }
        }
        Tk::Str(_) | Tk::Num(_) => ts.err(format!("a bare value can't be a step — did you mean say {}?", start.raw), Some(&start)),
    }
}

fn wrap(ts: &mut Ts, how: &str) -> R<StmtKind> {
    let id = ts.next();
    let field = match id_of(&id) {
        Some(n) if !is_keyword(n) => n.clone(),
        _ => return ts.err(format!("{} needs the new field's name, like {how}account{{name kind}}", how.trim()), id.as_ref()),
    };
    ts.expect_op("{", &format!("after {how}{field}"))?;
    Ok(StmtKind::Wrap(field.clone(), names(ts, &format!("{how}{field}{{…}}"))?))
}

fn unwrap(ts: &mut Ts, how: &str) -> R<StmtKind> {
    let id = ts.next();
    match id_of(&id) {
        Some(n) if !is_keyword(n) => Ok(StmtKind::Unwrap(n.clone())),
        _ => ts.err(format!("{} needs a field name, like {how}body", how.trim()), id.as_ref()),
    }
}

fn def(ts: &mut Ts) -> R<(Name, Clause)> {
    let name_tok = ts.next();
    let Some(name) = id_of(&name_tok).cloned() else { return ts.err("a definition starts with a word name, like greet = …", name_tok.as_ref()) };
    let name_tok = name_tok.unwrap();
    if is_keyword(&name) || Builtin::named(&name).is_some() {
        return ts.err(format!("{name} is a built-in name"), Some(&name_tok));
    }
    if is_upper(&name) {
        return ts.err("word names start lowercase; Capitalized names are constants", Some(&name_tok));
    }
    let (mut pat, mut guard, mut guard_src) = (None, None, None);
    if ts.is_op("{", 0) {
        pat = Some(pattern(ts, Nest::Top)?);
    }
    if ts.is_id("when") {
        let w = ts.next().unwrap();
        guard = Some(expr(ts, 0)?);
        let g = ts.last().unwrap();
        guard_src = Some(if w.line == g.line { slice16(&ts.lines[w.line - 1], w.end, g.end).trim().to_string() } else { "guard".into() });
    }
    let eq = ts.next();
    if !matches!(&eq, Some(Token { t: Tk::Op("=" | ":"), .. })) {
        let what = if pat.is_some() { " and its pattern" } else { "" };
        return ts.err(format!("expected “=” after {name}{what}"), eq.as_ref());
    }
    let head_src = slice16(&ts.lines[name_tok.line - 1], name_tok.col, eq.unwrap().col).trim().to_string();
    let b = body(ts, None)?;
    let clause = Clause { pattern: pat, guard, guard_src, body: b.into(), line: name_tok.line, head_src, chunk_line: 0, chunk_text: String::new() };
    Ok((name, clause))
}

fn singular(name: &str) -> Option<String> {
    let n = name;
    if let Some(s) = n.strip_suffix("ies") {
        return Some(format!("{s}y"));
    }
    if ["sses", "shes", "ches", "xes", "zes"].iter().any(|e| n.ends_with(e)) {
        return Some(n[..n.len() - 2].to_string());
    }
    let cs: Vec<char> = n.chars().collect();
    if cs.len() >= 2 && cs[cs.len() - 1] == 's' && cs[cs.len() - 2] != 's' {
        return Some(cs[..cs.len() - 1].iter().collect());
    }
    None
}

/// `{name: Type, opt: Type?, …}`. In a route, types nest (`[Type]`, `{…}`); in a table,
/// they're column types and `id` is implicit, only checked.
fn field_decls(ts: &mut Ts, table: bool) -> R<Vec<FieldDecl>> {
    ts.expect_op("{", "to start the columns")?;
    let mut decls: Vec<FieldDecl> = vec![];
    loop {
        if ts.at_end() {
            return ts.err("the column list is missing its “}”", None);
        }
        if ts.is_op("}", 0) {
            ts.next();
            break;
        }
        if ts.is_op(",", 0) {
            ts.next();
            continue;
        }
        let id = ts.next();
        let cname = match id_of(&id) {
            Some(n) if !is_keyword(n) => n.clone(),
            _ => return ts.err("expected a column name", id.as_ref()),
        };
        let (mut ty, mut opt) = (FieldTy::Col(ColType::Json), true);
        if ts.is_op(":", 0) {
            ts.next();
            ty = field_ty(ts, !table, id.as_ref())?;
            opt = false;
            if ts.is_op("?", 0) {
                ts.next();
                opt = true;
            }
        }
        if table && &*cname == "id" {
            if !matches!(ty, FieldTy::Col(ColType::Int | ColType::Json)) {
                return ts.err("id is always an Int", id.as_ref());
            }
            continue;
        }
        if decls.iter().any(|c| c.name == cname) {
            return ts.err(format!("{cname} is listed twice"), id.as_ref());
        }
        decls.push(FieldDecl { name: cname, ty, opt });
    }
    Ok(decls)
}

fn field_ty(ts: &mut Ts, nested: bool, at: Option<&Token>) -> R<FieldTy> {
    if nested && ts.is_op("{", 0) {
        return Ok(FieldTy::Rec(field_decls(ts, false)?));
    }
    if nested && ts.is_op("[", 0) {
        ts.next();
        let t = field_ty(ts, true, at)?;
        ts.expect_op("]", "to close the list type")?;
        return Ok(FieldTy::List(Box::new(t)));
    }
    let t = ts.next();
    match id_of(&t).and_then(|v| ColType::ALL.iter().find(|(k, _)| *k == &**v)) {
        Some((_, c)) => Ok(FieldTy::Col(*c)),
        None if nested => ts.err("types are Int, Num, Text, Bool, Const, Json, [Type] and {field: Type} (add ? when it may be missing, like Text?)", t.as_ref().or(at)),
        None => ts.err("column types are Int, Num, Text, Bool, Const, Json (add ? for an optional column, like Text?)", t.as_ref().or(at)),
    }
}

fn table(ts: &mut Ts) -> R<Table> {
    ts.next();
    let name_tok = ts.next().unwrap();
    let Tk::Id(name) = name_tok.t.clone() else { unreachable!() };
    let single = if ts.is_id("of") {
        ts.next();
        let s = ts.next();
        match id_of(&s) {
            Some(v) => Some(v.to_string()),
            None => return ts.err("expected the name of one row after “of”", s.as_ref()),
        }
    } else {
        singular(&name)
    };
    let Some(single) = single else {
        return ts.err(format!("can't guess what one row of {name} is called — write table {name} of item :: {{…}}"), Some(&name_tok));
    };
    if *single == *name {
        return ts.err("the row name has to differ from the table name", Some(&name_tok));
    }
    if !ts.is_op(":", 0) {
        return ts.err(format!("expected “::” after table {name}"), None);
    }
    ts.next();
    if ts.is_op(":", 0) {
        ts.next();
    }
    let cols = field_decls(ts, true)?.into_iter().map(|f| match f.ty {
        FieldTy::Col(ty) => Col { name: f.name, ty, opt: f.opt },
        _ => unreachable!("a table's fields are column types"),
    });
    let cols = cols.collect();
    if let Some(t) = ts.peek(0) {
        return ts.err(format!("unexpected “{}” after the columns", t.raw), None);
    }
    Ok(Table { name, single: single.into(), cols, line: name_tok.line })
}

fn seed(ts: &mut Ts) -> R<Seed> {
    ts.next();
    let name_tok = ts.next().unwrap();
    let Tk::Id(table) = name_tok.t.clone() else { unreachable!() };
    ts.expect_op("=", &format!("after seed {table}"))?;
    let e = expr(ts, 0)?;
    if let Some(t) = ts.peek(0) {
        return ts.err(format!("unexpected “{}”", t.raw), None);
    }
    Ok(Seed { table, expr: e, line: name_tok.line })
}

pub fn parse(src: &str) -> Program {
    let lines: Vec<String> = src.split('\n').map(String::from).collect();
    let mut p = Program::default();
    let mut chunks: Vec<Vec<usize>> = vec![];
    for (idx, raw) in lines.iter().enumerate() {
        let code = strip_comment(raw);
        if code.trim().is_empty() {
            continue;
        }
        match chunks.last_mut() {
            Some(cur) if code.starts_with([' ', '\t']) => cur.push(idx),
            _ => chunks.push(vec![idx]),
        }
    }
    for ch in &chunks {
        if let Err(e) = chunk(&mut p, &lines, ch) {
            let (line, col) = e.at.unwrap_or((ch[0] + 1, 0));
            p.errors.push(Diag { line, col, msg: e.msg });
        }
    }
    check_refs(&mut p);
    p.errors.sort_by_key(|e| e.line);
    p.lines = lines;
    p
}

/// `name:Type`, with a trailing `?` when `opt` allows it.
fn param(src: &str, opt: bool) -> Result<FieldDecl, String> {
    let bad = || format!("“{src}”: write name:Type, with Type one of Int, Num, Text, Bool, Const, Json{}", if opt { " (add ? if it may be missing)" } else { "" });
    let (name, ty) = src.split_once(':').ok_or_else(bad)?;
    let (ty, is_opt) = match ty.strip_suffix('?') {
        Some(t) if opt => (t, true),
        _ => (ty, false),
    };
    let ty = ColType::ALL.iter().find(|(k, _)| *k == ty).ok_or_else(bad)?.1;
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return Err(bad());
    }
    Ok(FieldDecl { name: name.into(), ty: FieldTy::Col(ty), opt: is_opt })
}

/// `/lit/:name:Type…?name:Type[?]&…`
fn path(src: &str) -> Result<(Vec<Seg>, Vec<FieldDecl>), String> {
    let (path, query) = src.split_once('?').unwrap_or((src, ""));
    let Some(rest) = path.strip_prefix('/') else { return Err(format!("a route path starts with /, got “{path}”")) };
    let segs = rest.split('/').filter(|s| !rest.is_empty() || !s.is_empty()).map(|s| match s.strip_prefix(':') {
        Some(p) => param(p, false).map(Seg::Param),
        None if s.is_empty() || s.contains(['{', '}', ':', '*']) => Err(format!("“{s}” can't be a path segment in “{path}”")),
        None => Ok(Seg::Lit(s.into())),
    });
    let query = query.split('&').filter(|q| !q.is_empty()).map(|q| param(q, true));
    Ok((segs.collect::<Result<_, _>>()?, query.collect::<Result<_, _>>()?))
}

/// The path is read as raw text (the lexer would see `//` and `:` as operators); the rest is lexed with it blanked out.
fn route(lines: &[String], ch: &[usize]) -> R<Route> {
    let first = strip_comment(&lines[ch[0]]);
    let line = ch[0] + 1;
    let word_after = |from: usize| {
        let a = from + first[from..].find(|c: char| !c.is_whitespace()).unwrap_or(first.len() - from);
        (a, a + first[a..].find(char::is_whitespace).unwrap_or(first.len() - a))
    };
    let (_, verb_end) = word_after("route".len());
    let (pa, pb) = word_after(verb_end);
    let src = &first[pa..pb];
    let col = first[..pa].encode_utf16().count();
    let (segs, query) = path(src).or_else(|msg| serr(msg, Some((line, col))))?;
    let blanked = format!("{}{}{}", &first[..pa], " ".repeat(pb - pa), &first[pb..]);
    let mut toks = lex(&blanked, line, true)?;
    for &li in &ch[1..] {
        toks.extend(lex(strip_comment(&lines[li]), li + 1, true)?);
    }
    let mut ts = Ts { t: toks, i: 0, lines };
    ts.next();
    let v = ts.next();
    let verb = match id_of(&v) {
        Some(v) if VERBS.contains(&&**v) => v.clone(),
        _ => return ts.err(format!("a route starts with a verb: {}", VERBS.join(", ")), v.as_ref()),
    };
    if src.is_empty() {
        return serr(format!("route {verb} needs a path, like /accounts/:id:Int"), Some((line, col)));
    }
    let body = if ts.is_op("{", 0) { Some(field_decls(&mut ts, false)?) } else { None };
    ts.expect_op("=", "before the word that answers the route")?;
    let w = ts.next();
    let Some(word) = id_of(&w).filter(|n| !is_keyword(n)).cloned() else { return ts.err("expected the word that answers the route", w.as_ref()) };
    if let Some(t) = ts.peek(0) {
        return ts.err(format!("unexpected “{}” after route {verb} {src} = {word}", t.raw), None);
    }
    let r = Route { verb, path: src.split('?').next().unwrap().into(), segs, query, body, word, line };
    let mut seen: Vec<&str> = vec!["body", "time"];
    for c in r.params() {
        if seen.contains(&&*c.name) {
            let why = if ["body", "time"].contains(&&*c.name) { "the router sets it" } else { "it's already a param" };
            return serr(format!("route {} {}: {} can't be a param, {why}", r.verb, r.path, c.name), Some((line, col)));
        }
        seen.push(&c.name);
    }
    Ok(r)
}

fn chunk(p: &mut Program, lines: &[String], ch: &[usize]) -> R<()> {
    let mut head = strip_comment(&lines[ch[0]]).split_whitespace();
    if head.next() == Some("route") && head.next().is_some_and(|v| v.chars().all(|c| c.is_ascii_uppercase())) {
        p.routes.push(route(lines, ch)?);
        return Ok(());
    }
    let mut toks = vec![];
    for &li in ch {
        toks.extend(lex_line(strip_comment(&lines[li]), li + 1)?);
    }
    let mut ts = Ts { t: toks, i: 0, lines };
    if let (Some(Tk::Id(a)), Some(Tk::Id(_))) = (ts.peek(0).map(|t| &t.t), ts.peek(1).map(|t| &t.t)) {
        if &**a == "table" {
            let t = table(&mut ts)?;
            if let Some(prev) = p.tables.get(&t.name) {
                let at = ts.t[1].clone();
                return ts.err(format!("table {} is already declared on line {}", t.name, prev.line), Some(&at));
            }
            p.tables.insert(t.name.clone(), t);
            return Ok(());
        }
        if &**a == "seed" {
            p.seeds.push(seed(&mut ts)?);
            return Ok(());
        }
    }
    let (name, mut c) = def(&mut ts)?;
    if let Some(t) = ts.peek(0) {
        return ts.err(format!("unexpected “{}”", t.raw), None);
    }
    c.chunk_line = ch[0] + 1;
    c.chunk_text = lines[ch[0]..=*ch.last().unwrap()].join("\n");
    let line = c.line;
    p.words.entry(name.clone()).or_insert_with(|| Word { name, clauses: vec![], line }).clauses.push(c);
    Ok(())
}

/// Unknown words and tables, holes, where clauses compiled to SQL, clauses that never run.
fn check_refs(p: &mut Program) {
    let mut cx = Refs { known: p.words.keys().cloned().collect(), tables: &p.tables, seen: Default::default(), errors: vec![], holes: vec![] };
    let mut warnings = vec![];
    for w in p.words.values_mut() {
        let n = w.clauses.len();
        for i in 0..n {
            let c = &mut w.clauses[i];
            let mut body = c.body.to_vec();
            cx.walk(&mut body, &w.name);
            c.body = body.into();
            let c = &w.clauses[i];
            let catch_all = c.pattern.as_ref().is_none_or(|p| p.is_empty()) && c.guard.is_none();
            if catch_all && i + 1 < n {
                warnings.push(Diag { line: w.clauses[i + 1].line, col: 0, msg: format!("this {} clause never runs: the one on line {} matches everything", w.name, c.line) });
            }
        }
    }
    let (mut errors, holes) = (cx.errors, cx.holes);
    for sd in &p.seeds {
        if !p.tables.contains_key(&sd.table) {
            errors.push(Diag { line: sd.line, col: 0, msg: format!("seed {0}: there's no table called {0}", sd.table) });
        }
    }
    for (i, r) in p.routes.iter().enumerate() {
        if !p.words.contains_key(&r.word) {
            errors.push(Diag { line: r.line, col: 0, msg: format!("route {} {}: {} isn't defined", r.verb, r.path, r.word) });
        }
        let same = p.routes[..i].iter().filter(|q| q.shape() == r.shape());
        let msg = (same.clone().find(|q| q.pattern() != r.pattern()))
            .map(|q| format!("route {} {}: name the params as on line {} ({})", r.verb, r.path, q.line, q.path))
            .or_else(|| same.clone().find(|q| q.verb == r.verb).map(|q| format!("route {} {} is already declared on line {}", r.verb, r.path, q.line)));
        errors.extend(msg.map(|msg| Diag { line: r.line, col: 0, msg }));
    }
    p.errors.extend(errors);
    p.warnings = warnings;
    p.holes = holes;
}

struct Refs<'a> {
    known: std::collections::HashSet<Name>,
    tables: &'a indexmap::IndexMap<Name, Table>,
    seen: std::collections::HashSet<(Name, usize)>,
    errors: Vec<Diag>,
    holes: Vec<Hole>,
}

impl Refs<'_> {
    fn walk(&mut self, body: &mut [Stmt], word: &Name) {
        for s in body.iter_mut() {
            let pos = s.pos;
            let err = |msg| Diag { line: pos.line, col: pos.col, msg };
            match &mut s.kind {
                StmtKind::Call(name) if !self.known.contains(name) => {
                    if self.seen.insert((name.clone(), pos.line)) {
                        self.errors.push(err(format!("{name} isn't defined yet — write {name} = ? to leave it open")));
                    }
                }
                StmtKind::Hole(note) => self.holes.push(Hole { line: pos.line, note: note.clone(), word: word.clone() }),
                StmtKind::Table { table, op, pattern, guard, compiled } => match self.tables.get(table) {
                    None => self.errors.push(err(format!("there's no table called {table} — declare it with table {table} :: {{…}}"))),
                    Some(_) if !TABLE_OPS.contains(&&**op) => {
                        self.errors.push(err(format!("{table}.{op} isn't a table word — use {}", TABLE_OPS.map(|o| format!("{table}.{o}")).join(", "))))
                    }
                    Some(t) if &**op == "where" => match compile_where(t, pattern.as_deref(), guard.as_ref()) {
                        Ok(w) => *compiled = Some(w),
                        Err(msg) => self.errors.push(err(msg)),
                    },
                    Some(_) => {}
                },
                StmtKind::Focus(_, b) => {
                    let mut v = b.to_vec();
                    self.walk(&mut v, word);
                    *b = v.into();
                }
                StmtKind::Par(bs) => {
                    for b in bs.iter_mut() {
                        let mut v = b.to_vec();
                        self.walk(&mut v, word);
                        *b = v.into();
                    }
                }
                _ => {}
            }
        }
    }
}
