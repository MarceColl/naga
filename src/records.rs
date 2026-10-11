//! A program as naga values: `Program` → records → `Program`, losslessly, so a naga word can hold
//! code, walk it with `keys`/`head`/`tail`/`len`, and rewrite it before the checker sees it.
//!
//! Every node is a record. Statements and expressions say what they are in `k` (`{k: :call, name: :greet}`);
//! names are constants, source text is text, lines and columns are numbers. An optional value is nil when
//! absent, except a pattern literal, which can itself be nil: there the field is left out.
use crate::ast::*;
use crate::value::{Name, Rec, Value, list, rec};
use indexmap::IndexMap;

pub type R<T> = Result<T, String>;

// ---------- encoding ----------
fn r<const N: usize>(fields: [(&str, Value); N]) -> Value {
    rec(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
}

fn tagged<const N: usize>(k: &str, fields: [(&str, Value); N]) -> Value {
    let mut m = Rec::from([("k".into(), c(k))]);
    m.extend(fields.into_iter().map(|(k, v)| (k.into(), v)));
    rec(m)
}

fn c(n: &str) -> Value {
    Value::Const(n.into())
}

fn t(s: &str) -> Value {
    Value::Text(s.into())
}

fn n(x: usize) -> Value {
    Value::Num(x as f64)
}

fn names(ns: &[Name]) -> Value {
    list(ns.iter().map(|x| c(x)).collect())
}

fn opt<T>(x: &Option<T>, f: impl Fn(&T) -> Value) -> Value {
    x.as_ref().map_or(Value::Nil, f)
}

fn many<T>(xs: &[T], f: impl Fn(&T) -> Value) -> Value {
    list(xs.iter().map(f).collect())
}

fn pos(p: Pos) -> Value {
    r([("line", n(p.line)), ("col", n(p.col))])
}

const OPS: [(BinOp, &str); 18] = [
    (BinOp::Eq, "eq"), (BinOp::Ne, "ne"), (BinOp::Lt, "lt"), (BinOp::Gt, "gt"), (BinOp::Le, "le"), (BinOp::Ge, "ge"),
    (BinOp::Within, "within"), (BinOp::Cat, "cat"), (BinOp::Add, "add"), (BinOp::Sub, "sub"), (BinOp::Over, "over"),
    (BinOp::Join, "join"), (BinOp::Meet, "meet"), (BinOp::Diff, "diff"), (BinOp::Apply, "apply"), (BinOp::Mul, "mul"),
    (BinOp::Div, "div"), (BinOp::Rem, "rem"),
];

pub fn expr(e: &Expr) -> Value {
    match e {
        Expr::Lit(v) => tagged("lit", [("v", v.clone())]),
        Expr::Now => tagged("now", []),
        Expr::Field(x) => tagged("field", [("name", c(x))]),
        Expr::Outer(up, x) => tagged("outer", [("up", n(*up)), ("name", c(x))]),
        Expr::Get(of, x) => tagged("get", [("of", expr(of)), ("name", c(x))]),
        Expr::Proj(of, xs) => tagged("proj", [("of", expr(of)), ("names", names(xs))]),
        Expr::List(xs) => tagged("list", [("items", many(xs, expr))]),
        Expr::Rec(fs) => tagged("rec", [("fields", many(fs, |(k, v)| r([("name", c(k)), ("expr", expr(v))])))]),
        Expr::Interp(ps) => tagged("interp", [("parts", many(ps, |p| match p {
            Part::Text(s) => t(s),
            Part::Expr(e) => expr(e),
        }))]),
        Expr::Neg(of) => tagged("neg", [("of", expr(of))]),
        Expr::Not(of) => tagged("not", [("of", expr(of))]),
        Expr::And(a, b) => tagged("and", [("l", expr(a)), ("r", expr(b))]),
        Expr::Or(a, b) => tagged("or", [("l", expr(a)), ("r", expr(b))]),
        Expr::Call(f, args) => tagged("call", [("fn", c(f.name)), ("args", many(args, expr))]),
        Expr::Bin(op, a, b) => {
            let op = OPS.iter().find(|(o, _)| o == op).unwrap().1;
            tagged("bin", [("op", c(op)), ("l", expr(a)), ("r", expr(b))])
        }
    }
}

const MODES: [(Mode, &str); 3] = [(Mode::Has, "has"), (Mode::Take, "take"), (Mode::Absent, "absent")];

fn pattern(ps: &[PatEntry]) -> Value {
    many(ps, |p| {
        let mut m = Rec::from([
            ("name".into(), c(&p.name)),
            ("mode".into(), c(MODES.iter().find(|(m, _)| *m == p.mode).unwrap().1)),
            ("sub".into(), opt(&p.sub, |s| pattern(s))),
        ]);
        if let Some(v) = &p.lit {
            m.insert("lit".into(), v.clone());
        }
        rec(m)
    })
}

fn inits(fs: &[FieldInit]) -> Value {
    many(fs, |f| r([("name", c(&f.name)), ("expr", opt(&f.expr, expr))]))
}

fn body(b: &[Stmt]) -> Value {
    many(b, stmt)
}

pub fn stmt(s: &Stmt) -> Value {
    use StmtKind::*;
    let (k, fields): (&str, Vec<(&str, Value)>) = match &s.kind {
        Call(x) => ("call", vec![("name", c(x))]),
        Add(fs) => ("add", vec![("fields", inits(fs))]),
        Del(xs) => ("del", vec![("names", names(xs))]),
        Pick(xs) => ("pick", vec![("names", names(xs))]),
        Set(x, e) => ("set", vec![("name", c(x)), ("expr", expr(e))]),
        Inc(x, by) => ("inc", vec![("name", c(x)), ("by", Value::Num(*by))]),
        Rename(a, b) => ("rename", vec![("from", c(a)), ("to", c(b))]),
        Wrap(x, xs) => ("wrap", vec![("name", c(x)), ("names", names(xs))]),
        Unwrap(x) => ("unwrap", vec![("name", c(x))]),
        Over(fs) => ("over", vec![("fields", inits(fs))]),
        Unify(fs) => ("unify", vec![("fields", inits(fs))]),
        Focus(x, b) => ("focus", vec![("name", c(x)), ("body", body(b))]),
        Par(bs) => ("par", vec![("bodies", many(bs, |b| body(b)))]),
        Say(e) => ("say", vec![("expr", expr(e))]),
        Raise(e) => ("raise", vec![("expr", expr(e))]),
        Show => ("show", vec![]),
        Break => ("break", vec![]),
        Hole(note) => ("hole", vec![("note", t(note))]),
        Table { table, op, pattern: pat, guard, compiled } => (
            "table",
            vec![
                ("table", c(table)),
                ("op", c(op)),
                ("pattern", opt(pat, |p| pattern(p))),
                ("guard", opt(guard, expr)),
                ("compiled", opt(compiled, |w| r([("sql", t(&w.sql)), ("params", many(&w.params, |p| match p {
                    Param::Lit(v) => tagged("lit", [("v", v.clone())]),
                    Param::Expr(e) => tagged("expr", [("expr", expr(e))]),
                }))]))),
            ],
        ),
    };
    let mut m = Rec::from([("k".into(), c(k)), ("pos".into(), pos(s.pos)), ("end".into(), pos(s.end))]);
    m.extend(fields.into_iter().map(|(k, v)| (k.into(), v)));
    rec(m)
}

/// `pattern` nil is an open head (`greet = …`), a list a closed one (`greet {} = …` is the empty list);
/// `head`, `guard_src` and `chunk` are the source text diagnostics quote and the checker caches by.
pub fn clause(cl: &Clause) -> Value {
    r([
        ("line", n(cl.line)),
        ("head", t(&cl.head_src)),
        ("pattern", opt(&cl.pattern, |p| pattern(p))),
        ("guard", opt(&cl.guard, expr)),
        ("guard_src", opt(&cl.guard_src, |s| t(s))),
        ("chunk", r([("line", n(cl.chunk_line)), ("text", t(&cl.chunk_text))])),
        ("body", body(&cl.body)),
    ])
}

pub fn word(w: &Word) -> Value {
    r([("name", c(&w.name)), ("line", n(w.line)), ("clauses", many(&w.clauses, clause))])
}

fn field_ty(ty: &FieldTy) -> Value {
    match ty {
        FieldTy::Col(t) => tagged("col", [("ty", c(t.name()))]),
        FieldTy::List(of) => tagged("list", [("of", field_ty(of))]),
        FieldTy::Rec(fs) => tagged("rec", [("fields", many(fs, field_decl))]),
    }
}

fn field_decl(d: &FieldDecl) -> Value {
    r([("name", c(&d.name)), ("ty", field_ty(&d.ty)), ("opt", Value::Bool(d.opt))])
}

fn route(rt: &Route) -> Value {
    r([
        ("verb", c(&rt.verb)),
        ("path", t(&rt.path)),
        ("segs", many(&rt.segs, |s| match s {
            Seg::Lit(l) => t(l),
            Seg::Param(d) => field_decl(d),
        })),
        ("query", many(&rt.query, field_decl)),
        ("body", opt(&rt.body, |fs| many(fs, field_decl))),
        ("word", c(&rt.word)),
        ("line", n(rt.line)),
    ])
}

fn diag(d: &Diag) -> Value {
    r([("line", n(d.line)), ("col", n(d.col)), ("msg", t(&d.msg))])
}

pub fn program(p: &Program) -> Value {
    let by_name = |m: Vec<(Name, Value)>| rec(m.into_iter().collect());
    r([
        ("words", by_name(p.words.iter().map(|(k, w)| (k.clone(), word(w))).collect())),
        ("tables", by_name(p.tables.iter().map(|(k, tb)| {
            let cols = many(&tb.cols, |col| r([("name", c(&col.name)), ("ty", c(col.ty.name())), ("opt", Value::Bool(col.opt))]));
            (k.clone(), r([("name", c(&tb.name)), ("single", c(&tb.single)), ("cols", cols), ("line", n(tb.line))]))
        }).collect())),
        ("seeds", many(&p.seeds, |s| r([("table", c(&s.table)), ("expr", expr(&s.expr)), ("line", n(s.line))]))),
        ("routes", many(&p.routes, route)),
        ("lives", many(&p.lives, route)),
        ("runs", many(&p.runs, |run| r([
            ("name", t(&run.name)),
            ("at", opt(&run.at, |x| Value::Num(*x))),
            ("requests", many(&run.requests, |q| r([
                ("verb", c(&q.verb)),
                ("path", t(&q.path)),
                ("body", opt(&q.body, expr)),
                ("status", n(q.status as usize)),
                ("json", opt(&q.json, expr)),
                ("line", n(q.line)),
            ]))),
            ("line", n(run.line)),
        ]))),
        ("errors", many(&p.errors, diag)),
        ("warnings", many(&p.warnings, diag)),
        ("holes", many(&p.holes, |h| r([("line", n(h.line)), ("note", t(&h.note)), ("word", c(&h.word))]))),
        ("lines", many(&p.lines, |l| t(l))),
    ])
}

// ---------- decoding ----------
fn as_rec<'a>(v: &'a Value, what: &str) -> R<&'a Rec> {
    v.as_rec().map(|r| &**r).ok_or_else(|| format!("{what} should be a record, got {}", v.show()))
}

fn get<'a>(m: &'a Rec, k: &str) -> R<&'a Value> {
    m.get(k).ok_or_else(|| format!("{} has no {k}", crate::value::show_rec(m)))
}

fn name_of(v: &Value) -> R<Name> {
    match v {
        Value::Const(x) | Value::Text(x) => Ok(x.clone()),
        v => Err(format!("expected a name, got {}", v.show())),
    }
}

fn name(m: &Rec, k: &str) -> R<Name> {
    name_of(get(m, k)?)
}

fn text(m: &Rec, k: &str) -> R<String> {
    match get(m, k)? {
        Value::Text(s) => Ok(s.to_string()),
        v => Err(format!("{k} should be text, got {}", v.show())),
    }
}

fn num(m: &Rec, k: &str) -> R<f64> {
    match get(m, k)? {
        Value::Num(x) => Ok(*x),
        v => Err(format!("{k} should be a number, got {}", v.show())),
    }
}

fn int(m: &Rec, k: &str) -> R<usize> {
    let x = num(m, k)?;
    (x >= 0.0 && x.fract() == 0.0).then_some(x as usize).ok_or_else(|| format!("{k} should be a whole number, got {x}"))
}

fn flag(m: &Rec, k: &str) -> R<bool> {
    match get(m, k)? {
        Value::Bool(b) => Ok(*b),
        v => Err(format!("{k} should be true or false, got {}", v.show())),
    }
}

fn items<'a>(m: &'a Rec, k: &str) -> R<&'a [Value]> {
    match get(m, k)? {
        Value::List(xs) => Ok(xs),
        v => Err(format!("{k} should be a list, got {}", v.show())),
    }
}

fn each<T>(m: &Rec, k: &str, f: impl Fn(&Value) -> R<T>) -> R<Vec<T>> {
    items(m, k)?.iter().map(f).collect()
}

fn maybe<T>(m: &Rec, k: &str, f: impl Fn(&Value) -> R<T>) -> R<Option<T>> {
    match get(m, k)? {
        Value::Nil => Ok(None),
        v => f(v).map(Some),
    }
}

fn kind(m: &Rec) -> R<Name> {
    name(m, "k")
}

fn un_pos(v: &Value) -> R<Pos> {
    let m = as_rec(v, "a position")?;
    Ok(Pos { line: int(m, "line")?, col: int(m, "col")? })
}

fn boxed(m: &Rec, k: &str) -> R<Box<Expr>> {
    un_expr(get(m, k)?).map(Box::new)
}

pub fn un_expr(v: &Value) -> R<Expr> {
    let m = as_rec(v, "an expression")?;
    let k = kind(m)?;
    Ok(match &*k {
        "lit" => Expr::Lit(get(m, "v")?.clone()),
        "now" => Expr::Now,
        "field" => Expr::Field(name(m, "name")?),
        "outer" => Expr::Outer(int(m, "up")?, name(m, "name")?),
        "get" => Expr::Get(boxed(m, "of")?, name(m, "name")?),
        "proj" => Expr::Proj(boxed(m, "of")?, each(m, "names", name_of)?),
        "list" => Expr::List(each(m, "items", un_expr)?),
        "rec" => Expr::Rec(each(m, "fields", |f| {
            let f = as_rec(f, "a record field")?;
            Ok((name(f, "name")?, un_expr(get(f, "expr")?)?))
        })?),
        "interp" => Expr::Interp(each(m, "parts", |p| match p {
            Value::Text(s) => Ok(Part::Text(s.to_string())),
            p => un_expr(p).map(Part::Expr),
        })?),
        "neg" => Expr::Neg(boxed(m, "of")?),
        "not" => Expr::Not(boxed(m, "of")?),
        "and" => Expr::And(boxed(m, "l")?, boxed(m, "r")?),
        "or" => Expr::Or(boxed(m, "l")?, boxed(m, "r")?),
        "call" => {
            let f = name(m, "fn")?;
            let func = crate::funcs::named(&f).ok_or_else(|| format!("no function {f}"))?;
            Expr::Call(func, each(m, "args", un_expr)?)
        }
        "bin" => {
            let op = name(m, "op")?;
            let op = OPS.iter().find(|(_, x)| *x == &*op).ok_or_else(|| format!("no operator {op}"))?.0;
            Expr::Bin(op, boxed(m, "l")?, boxed(m, "r")?)
        }
        k => return Err(format!("no expression kind {k}")),
    })
}

fn un_pattern(v: &Value) -> R<Vec<PatEntry>> {
    let Value::List(xs) = v else { return Err(format!("a pattern should be a list, got {}", v.show())) };
    xs.iter()
        .map(|p| {
            let m = as_rec(p, "a pattern entry")?;
            let mode = name(m, "mode")?;
            let mode = MODES.iter().find(|(_, x)| *x == &*mode).ok_or_else(|| format!("no pattern mode {mode}"))?.0;
            Ok(PatEntry { name: name(m, "name")?, mode, sub: maybe(m, "sub", un_pattern)?, lit: m.get("lit").cloned() })
        })
        .collect()
}

fn un_inits(m: &Rec) -> R<Vec<FieldInit>> {
    each(m, "fields", |f| {
        let f = as_rec(f, "a field")?;
        Ok(FieldInit { name: name(f, "name")?, expr: maybe(f, "expr", un_expr)? })
    })
}

fn un_body(v: &Value) -> R<Body> {
    let Value::List(xs) = v else { return Err(format!("a body should be a list, got {}", v.show())) };
    Ok(xs.iter().map(un_stmt).collect::<R<Vec<_>>>()?.into())
}

pub fn un_stmt(v: &Value) -> R<Stmt> {
    use StmtKind::*;
    let m = as_rec(v, "a statement")?;
    let k = kind(m)?;
    let kind = match &*k {
        "call" => Call(name(m, "name")?),
        "add" => Add(un_inits(m)?),
        "del" => Del(each(m, "names", name_of)?),
        "pick" => Pick(each(m, "names", name_of)?),
        "set" => Set(name(m, "name")?, un_expr(get(m, "expr")?)?),
        "inc" => Inc(name(m, "name")?, num(m, "by")?),
        "rename" => Rename(name(m, "from")?, name(m, "to")?),
        "wrap" => Wrap(name(m, "name")?, each(m, "names", name_of)?),
        "unwrap" => Unwrap(name(m, "name")?),
        "over" => Over(un_inits(m)?),
        "unify" => Unify(un_inits(m)?),
        "focus" => Focus(name(m, "name")?, un_body(get(m, "body")?)?),
        "par" => Par(each(m, "bodies", un_body)?),
        "say" => Say(un_expr(get(m, "expr")?)?),
        "raise" => Raise(un_expr(get(m, "expr")?)?),
        "show" => Show,
        "break" => Break,
        "hole" => Hole(text(m, "note")?),
        "table" => Table {
            table: name(m, "table")?,
            op: name(m, "op")?,
            pattern: maybe(m, "pattern", un_pattern)?,
            guard: maybe(m, "guard", un_expr)?,
            compiled: maybe(m, "compiled", |w| {
                let w = as_rec(w, "a compiled where")?;
                let params = each(w, "params", |p| {
                    let p = as_rec(p, "a where param")?;
                    match &*kind(p)? {
                        "lit" => Ok(Param::Lit(get(p, "v")?.clone())),
                        "expr" => un_expr(get(p, "expr")?).map(Param::Expr),
                        k => Err(format!("no where param kind {k}")),
                    }
                })?;
                Ok(Where { sql: text(w, "sql")?, params })
            })?,
        },
        k => return Err(format!("no statement kind {k}")),
    };
    Ok(Stmt { kind, pos: un_pos(get(m, "pos")?)?, end: un_pos(get(m, "end")?)? })
}

pub fn un_clause(v: &Value) -> R<Clause> {
    let m = as_rec(v, "a clause")?;
    let chunk = as_rec(get(m, "chunk")?, "a chunk")?;
    Ok(Clause {
        pattern: maybe(m, "pattern", un_pattern)?,
        guard: maybe(m, "guard", un_expr)?,
        guard_src: maybe(m, "guard_src", |s| match s {
            Value::Text(s) => Ok(s.to_string()),
            v => Err(format!("guard_src should be text, got {}", v.show())),
        })?,
        body: un_body(get(m, "body")?)?,
        line: int(m, "line")?,
        head_src: text(m, "head")?,
        chunk_line: int(chunk, "line")?,
        chunk_text: text(chunk, "text")?,
    })
}

pub fn un_word(v: &Value) -> R<Word> {
    let m = as_rec(v, "a word")?;
    Ok(Word { name: name(m, "name")?, clauses: each(m, "clauses", un_clause)?, line: int(m, "line")? })
}

fn col_type(v: &Value) -> R<ColType> {
    let x = name_of(v)?;
    ColType::ALL.iter().find(|(k, _)| *k == &*x).map(|(_, t)| *t).ok_or_else(|| format!("no column type {x}"))
}

fn un_field_ty(v: &Value) -> R<FieldTy> {
    let m = as_rec(v, "a field type")?;
    Ok(match &*kind(m)? {
        "col" => FieldTy::Col(col_type(get(m, "ty")?)?),
        "list" => FieldTy::List(Box::new(un_field_ty(get(m, "of")?)?)),
        "rec" => FieldTy::Rec(each(m, "fields", un_field_decl)?),
        k => return Err(format!("no field type kind {k}")),
    })
}

fn un_field_decl(v: &Value) -> R<FieldDecl> {
    let m = as_rec(v, "a field declaration")?;
    Ok(FieldDecl { name: name(m, "name")?, ty: un_field_ty(get(m, "ty")?)?, opt: flag(m, "opt")? })
}

fn un_route(v: &Value) -> R<Route> {
    let m = as_rec(v, "a route")?;
    Ok(Route {
        verb: name(m, "verb")?,
        path: text(m, "path")?,
        segs: each(m, "segs", |s| match s {
            Value::Text(l) => Ok(Seg::Lit(l.to_string())),
            s => un_field_decl(s).map(Seg::Param),
        })?,
        query: each(m, "query", un_field_decl)?,
        body: maybe(m, "body", |b| match b {
            Value::List(xs) => xs.iter().map(un_field_decl).collect(),
            b => Err(format!("a route body should be a list, got {}", b.show())),
        })?,
        word: name(m, "word")?,
        line: int(m, "line")?,
    })
}

fn un_diag(v: &Value) -> R<Diag> {
    let m = as_rec(v, "a diagnostic")?;
    Ok(Diag { line: int(m, "line")?, col: int(m, "col")?, msg: text(m, "msg")? })
}

fn by_name<T>(m: &Rec, k: &str, f: impl Fn(&Value) -> R<T>) -> R<IndexMap<Name, T>> {
    as_rec(get(m, k)?, k)?.iter().map(|(k, v)| Ok((k.clone(), f(v)?))).collect()
}

pub fn un_program(v: &Value) -> R<Program> {
    let m = as_rec(v, "a program")?;
    Ok(Program {
        words: by_name(m, "words", un_word)?,
        tables: by_name(m, "tables", |v| {
            let tb = as_rec(v, "a table")?;
            let cols = each(tb, "cols", |col| {
                let col = as_rec(col, "a column")?;
                Ok(Col { name: name(col, "name")?, ty: col_type(get(col, "ty")?)?, opt: flag(col, "opt")? })
            })?;
            Ok(Table { name: name(tb, "name")?, single: name(tb, "single")?, cols, line: int(tb, "line")? })
        })?,
        seeds: each(m, "seeds", |s| {
            let s = as_rec(s, "a seed")?;
            Ok(Seed { table: name(s, "table")?, expr: un_expr(get(s, "expr")?)?, line: int(s, "line")? })
        })?,
        routes: each(m, "routes", un_route)?,
        lives: each(m, "lives", un_route)?,
        runs: each(m, "runs", |run| {
            let run = as_rec(run, "a run")?;
            let requests = each(run, "requests", |q| {
                let q = as_rec(q, "a request")?;
                let status = int(q, "status")?;
                Ok(Request {
                    verb: name(q, "verb")?,
                    path: text(q, "path")?,
                    body: maybe(q, "body", un_expr)?,
                    status: u16::try_from(status).map_err(|_| format!("status {status} is out of range"))?,
                    json: maybe(q, "json", un_expr)?,
                    line: int(q, "line")?,
                })
            })?;
            Ok(Run { name: text(run, "name")?, at: maybe(run, "at", |x| match x {
                Value::Num(x) => Ok(*x),
                x => Err(format!("at should be a number, got {}", x.show())),
            })?, requests, line: int(run, "line")? })
        })?,
        errors: each(m, "errors", un_diag)?,
        warnings: each(m, "warnings", un_diag)?,
        holes: each(m, "holes", |h| {
            let h = as_rec(h, "a hole")?;
            Ok(Hole { line: int(h, "line")?, note: text(h, "note")?, word: name(h, "word")? })
        })?,
        lines: each(m, "lines", |l| match l {
            Value::Text(s) => Ok(s.to_string()),
            l => Err(format!("a line should be text, got {}", l.show())),
        })?,
    })
}
