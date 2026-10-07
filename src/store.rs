//! Tables live in SQLite. Every statement is logged with its parameters inlined.
use crate::ast::*;
use crate::eval::{Env, eval};
use crate::num;
use crate::value::*;
use indexmap::IndexMap;
use rusqlite::{Connection, types::ValueRef};

fn qi(n: &str) -> String {
    format!("\"{}\"", n.replace('"', "\"\""))
}

// ---------- where → SQL ----------
/// `^x` (or `^x.y.z`) inside a where is a value from the record being processed, passed as a parameter.
fn outer_param(e: &Expr) -> Option<Expr> {
    match e {
        Expr::Outer(1, n) => Some(Expr::Field(n.clone())),
        Expr::Outer(up, n) => Some(Expr::Outer(up - 1, n.clone())),
        Expr::Get(o, k) => Some(Expr::Get(Box::new(outer_param(o)?), k.clone())),
        _ => None,
    }
}

/// A where pattern and guard become one SQL condition. Bare names are columns; ^x reads the current record.
pub fn compile_where(t: &Table, pattern: Option<&[PatEntry]>, guard: Option<&Expr>) -> Result<Where, String> {
    struct C<'a> {
        t: &'a Table,
        params: Vec<Param>,
    }
    impl C<'_> {
        fn col(&self, n: &str) -> Result<String, String> {
            if n != "id" && self.t.col(n).is_none() {
                return Err(format!("{} has no column {n}", self.t.name));
            }
            Ok(qi(n))
        }
        fn boolean(&mut self, e: &Expr) -> Result<String, String> {
            use BinOp::*;
            match e {
                Expr::And(..) | Expr::Or(..) | Expr::Not(_) | Expr::Bin(Eq | Ne | Lt | Gt | Le | Ge, ..) => self.expr(e),
                _ => Ok(format!("coalesce({}, 0) IS NOT 0", self.expr(e)?)),
            }
        }
        fn expr(&mut self, e: &Expr) -> Result<String, String> {
            let nil = |e: &Expr| matches!(e, Expr::Lit(Value::Nil));
            match e {
                Expr::Lit(Value::Nil) => return Ok("NULL".into()),
                Expr::Lit(v) => {
                    self.params.push(Param::Lit(v.clone()));
                    return Ok("?".into());
                }
                Expr::Field(n) => return self.col(n),
                Expr::Outer(..) | Expr::Get(..) => {
                    if let Some(p) = outer_param(e) {
                        self.params.push(Param::Expr(p));
                        return Ok("?".into());
                    }
                }
                Expr::Not(a) => return Ok(format!("(NOT {})", self.boolean(a)?)),
                Expr::And(a, b) => return Ok(format!("({} AND {})", self.boolean(a)?, self.boolean(b)?)),
                Expr::Or(a, b) => return Ok(format!("({} OR {})", self.boolean(a)?, self.boolean(b)?)),
                Expr::Neg(a) => return Ok(format!("(-{})", self.expr(a)?)),
                Expr::Bin(op @ (BinOp::Eq | BinOp::Ne), a, b) if nil(a) || nil(b) => {
                    let other = if nil(a) { b } else { a };
                    return Ok(format!("({} {} NULL)", self.expr(other)?, if *op == BinOp::Eq { "IS" } else { "IS NOT" }));
                }
                Expr::Bin(op, a, b) => {
                    let (a, b) = (self.expr(a)?, self.expr(b)?);
                    use BinOp::*;
                    let sql = match op {
                        Div => return Ok(format!("(CAST({a} AS REAL) / {b})")),
                        Eq => "=", Ne => "<>", Lt => "<", Gt => ">", Le => "<=", Ge => ">=", Add => "+", Sub => "-", Mul => "*", Rem => "%", Cat => "||",
                        _ => return Err(format!("{} can't be used inside a where", op.glyph())),
                    };
                    return Ok(format!("({a} {sql} {b})"));
                }
                Expr::Call(f, args) => {
                    let m = match f {
                        Builtin::Len => Some("length"),
                        Builtin::Abs => Some("abs"),
                        Builtin::Round => Some("round"),
                        Builtin::Min | Builtin::Max if args.len() >= 2 => Some(if *f == Builtin::Min { "min" } else { "max" }),
                        _ => None,
                    };
                    let Some(m) = m else { return Err(format!("{}() can't be used inside a where", f.name())) };
                    let args = args.iter().map(|a| self.expr(a)).collect::<Result<Vec<_>, _>>()?;
                    return Ok(format!("{m}({})", args.join(", ")));
                }
                _ => {}
            }
            Err("this part of the where can’t be turned into SQL (use fields, ^fields, constants, comparisons, arithmetic, and/or/not, len/abs/round)".into())
        }
    }
    let mut c = C { t, params: vec![] };
    let mut parts = vec![];
    for e in pattern.unwrap_or_default() {
        if e.sub.is_some() {
            return Err(format!("a where pattern can't look inside {}", e.name));
        }
        let col = c.col(&e.name)?;
        match &e.lit {
            _ if e.mode == Mode::Absent => parts.push(format!("{col} IS NULL")),
            Some(Value::Nil) => parts.push(format!("{col} IS NULL")),
            Some(v) => {
                c.params.push(Param::Lit(v.clone()));
                parts.push(format!("{col} = ?"));
            }
            None => parts.push(format!("{col} IS NOT NULL")),
        }
    }
    if let Some(g) = guard {
        let b = c.boolean(g)?;
        parts.push(b);
    }
    Ok(Where { sql: if parts.is_empty() { "1".into() } else { parts.join(" AND ") }, params: c.params })
}

// ---------- values ↔ SQL ----------
#[derive(Clone, Debug)]
enum Sql {
    Null,
    Num(f64),
    Text(String),
}

impl rusqlite::ToSql for Sql {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, Value as V};
        Ok(ToSqlOutput::Owned(match self {
            Sql::Null => V::Null,
            Sql::Num(n) if num::is_int(*n) && n.abs() < 9e15 => V::Integer(*n as i64),
            Sql::Num(n) => V::Real(*n),
            Sql::Text(s) => V::Text(s.clone()),
        }))
    }
}

fn to_sql(v: &Value) -> Sql {
    match v {
        Value::Nil => Sql::Null,
        Value::Bool(b) => Sql::Num(if *b { 1.0 } else { 0.0 }),
        Value::Num(n) => Sql::Num(*n),
        Value::Text(s) => Sql::Text(s.to_string()),
        Value::Const(c) => Sql::Text(c.to_string()),
        v => Sql::Text(to_json(v)),
    }
}

fn to_sql_col(t: &Table, k: &str, v: &Value) -> Sql {
    match v {
        Value::Nil => Sql::Null,
        v if t.col(k).is_some_and(|c| c.ty == ColType::Json) => Sql::Text(to_json(v)),
        v => to_sql(v),
    }
}

fn inline(sql: &str, params: &[Sql]) -> String {
    let mut it = params.iter();
    let mut out = String::new();
    for c in sql.chars() {
        if c != '?' {
            out.push(c);
            continue;
        }
        match it.next() {
            None | Some(Sql::Null) => out.push_str("NULL"),
            Some(Sql::Num(n)) => out.push_str(&num::js(*n)),
            Some(Sql::Text(s)) => out.push_str(&format!("'{}'", s.replace('\'', "''"))),
        }
    }
    out
}

fn from_sql(ty: ColType, v: &Sql) -> Option<Value> {
    let number = |v: &Sql| match v {
        Sql::Num(n) => *n,
        Sql::Text(s) if s.trim().is_empty() => 0.0,
        Sql::Text(s) => s.trim().parse().unwrap_or(f64::NAN),
        Sql::Null => 0.0,
    };
    let string = |v: &Sql| match v {
        Sql::Num(n) => num::js(*n),
        Sql::Text(s) => s.clone(),
        Sql::Null => "null".into(),
    };
    Some(match (ty, v) {
        (_, Sql::Null) => return None,
        (ColType::Int | ColType::Num, v) => Value::Num(number(v)),
        (ColType::Text, v) => Value::Text(string(v).into()),
        (ColType::Bool, Sql::Num(n)) => Value::Bool(*n != 0.0),
        (ColType::Bool, Sql::Text(s)) => Value::Bool(s != "0"),
        (ColType::Const, v) => Value::Const(string(v).into()),
        (ColType::Json, v) => from_json(&string(v)).unwrap_or_else(|| match v {
            Sql::Num(n) => Value::Num(*n),
            _ => Value::Text(string(v).into()),
        }),
    })
}

fn type_ok(ty: ColType, v: &Value) -> bool {
    match ty {
        ColType::Int => matches!(v, Value::Num(n) if num::is_int(*n)),
        ColType::Num => matches!(v, Value::Num(_)),
        ColType::Text => matches!(v, Value::Text(_)),
        ColType::Bool => matches!(v, Value::Bool(_)),
        ColType::Const => matches!(v, Value::Const(_)),
        ColType::Json => true,
    }
}

fn check_row<'a>(t: &Table, v: &'a Value, what: &str) -> Result<&'a Rec, RunError> {
    let Value::Rec(r) = v else { return fail(format!("{what} must be a record, got {}", v.described())) };
    for (k, v) in r.iter() {
        if &**k == "id" {
            if !matches!(v, Value::Num(n) if num::is_int(*n)) {
                return fail(format!("id must be a whole number, got {}", v.show()));
            }
            continue;
        }
        let Some(c) = t.col(k) else {
            let cols: String = t.cols.iter().map(|c| format!(", {}", c.name)).collect();
            return fail(format!("{} has no column {k} (it has id{cols})", t.name));
        };
        if !matches!(v, Value::Nil) && !type_ok(c.ty, v) {
            return fail(format!("{}.{k} is {}, but got {}", t.name, c.ty.name(), v.described()));
        }
    }
    Ok(r)
}

/// A column as SQLite has it: name, SQL type, NOT NULL.
type SqlCol = (String, String, bool);

/// What making a database fit the program did, and what it left because data could be lost.
#[derive(Debug, Default, PartialEq)]
pub struct Migration {
    pub applied: Vec<String>,
    pub pending: Vec<String>,
}

pub struct Store {
    db: Connection,
    /// Every statement run, with the step it ran at (-1 before the run starts).
    pub log: Vec<(i64, String)>,
    pub tick: i64,
}

enum Out {
    Rows(Vec<Vec<Sql>>),
    Ran { changes: usize, last_id: i64 },
}

impl Store {
    pub fn new(p: &Program) -> Result<Store, RunError> {
        let mut s = Store { db: Connection::open_in_memory().map_err(sql_err)?, log: vec![], tick: -1 };
        for t in p.tables.values() {
            s.create(t)?;
        }
        Ok(s)
    }

    /// A database file that outlives the run, made to fit the program (see `migrate`).
    pub fn open(p: &Program, path: &std::path::Path) -> Result<(Store, Migration), RunError> {
        let mut s = Store { db: Connection::open(path).map_err(sql_err)?, log: vec![], tick: -1 };
        let m = s.migrate(p)?;
        Ok((s, m))
    }

    fn create(&mut self, t: &Table) -> Result<(), RunError> {
        let cols: Vec<String> = std::iter::once("\"id\" INTEGER PRIMARY KEY".to_string())
            .chain(t.cols.iter().map(|c| format!("{} {}{}", qi(&c.name), c.ty.sql(), if c.opt { "" } else { " NOT NULL" })))
            .collect();
        self.q(&format!("CREATE TABLE {} ({})", qi(&t.name), cols.join(", ")), vec![], true, false)?;
        Ok(())
    }

    /// The table's columns as SQLite has them, or None when it doesn't exist.
    fn columns(&self, table: &str) -> Result<Option<Vec<SqlCol>>, RunError> {
        let mut st = self.db.prepare(&format!("PRAGMA table_info({})", qi(table))).map_err(sql_err)?;
        let cols = st.query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)? != 0))).map_err(sql_err)?;
        let cols: Vec<_> = cols.collect::<Result<_, _>>().map_err(sql_err)?;
        Ok((!cols.is_empty()).then_some(cols))
    }

    /// Makes the tables fit the program where no data can be lost: new tables are created and
    /// seeded, new optional columns added. Everything else is left as it is and reported.
    pub fn migrate(&mut self, p: &Program) -> Result<Migration, RunError> {
        let mut m = Migration::default();
        for t in p.tables.values() {
            let Some(have) = self.columns(&t.name)? else {
                self.create(t)?;
                self.seed(p, std::slice::from_ref(&t.name)).map_err(|(_, e)| e)?;
                m.applied.push(format!("created {}", t.name));
                continue;
            };
            for c in &t.cols {
                match have.iter().find(|h| h.0 == *c.name) {
                    None if c.opt => {
                        self.q(&format!("ALTER TABLE {} ADD COLUMN {} {}", qi(&t.name), qi(&c.name), c.ty.sql()), vec![], true, false)?;
                        m.applied.push(format!("added {}.{}", t.name, c.name));
                    }
                    None => m.pending.push(format!("{}.{} is new and required", t.name, c.name)),
                    Some((_, ty, _)) if ty != c.ty.sql() => m.pending.push(format!("{}.{} was {ty}, is now {}", t.name, c.name, c.ty.sql())),
                    Some((_, _, notnull)) if *notnull == c.opt => m.pending.push(format!("{}.{} is {} optional", t.name, c.name, if c.opt { "now" } else { "no longer" })),
                    Some(_) => {}
                }
            }
            for h in have.iter().filter(|h| h.0 != "id" && t.col(&h.0).is_none()) {
                m.pending.push(format!("{}.{} is gone from the program", t.name, h.0));
            }
        }
        self.log.clear();
        Ok(m)
    }

    /// Drops the table, then creates and seeds it as the program declares it.
    pub fn reset(&mut self, p: &Program, table: &str) -> Result<(), RunError> {
        let Some(t) = p.tables.get(table) else { return fail(format!("there's no table called {table}")) };
        self.q(&format!("DROP TABLE IF EXISTS {}", qi(table)), vec![], true, false)?;
        self.create(t)?;
        self.seed(p, std::slice::from_ref(&t.name)).map_err(|(_, e)| e)?;
        self.log.clear();
        Ok(())
    }

    /// Runs the seeds of the tables in `only`; a failure carries the seed's line.
    pub fn seed(&mut self, p: &Program, only: &[Name]) -> Result<(), (usize, RunError)> {
        for sd in p.seeds.iter().filter(|sd| only.contains(&sd.table)) {
            let Some(t) = p.tables.get(&sd.table) else { continue };
            let at = |e| (sd.line, e);
            let rows = eval(&sd.expr, &Env { subj: &Rec::new(), now: 0.0, outer: &[] }).map_err(at)?;
            let Value::List(rows) = rows else {
                return Err(at(RunError { msg: format!("seed {} needs a list of records, got {}", sd.table, rows.type_name()), details: vec![] }));
            };
            for r in rows.iter() {
                self.insert(t, r).map_err(at)?;
            }
        }
        Ok(())
    }

    fn q(&mut self, sql: &str, params: Vec<Sql>, run: bool, silent: bool) -> Result<Out, RunError> {
        if !silent {
            self.log.push((self.tick, inline(sql, &params)));
        }
        let ps = rusqlite::params_from_iter(params.iter());
        if run {
            let changes = self.db.execute(sql, ps).map_err(sql_err)?;
            return Ok(Out::Ran { changes, last_id: self.db.last_insert_rowid() });
        }
        let mut st = self.db.prepare(sql).map_err(sql_err)?;
        let n = st.column_count();
        let rows = st
            .query_map(ps, |r| {
                (0..n)
                    .map(|i| {
                        Ok(match r.get_ref(i)? {
                            ValueRef::Null => Sql::Null,
                            ValueRef::Integer(x) => Sql::Num(x as f64),
                            ValueRef::Real(x) => Sql::Num(x),
                            ValueRef::Text(x) | ValueRef::Blob(x) => Sql::Text(String::from_utf8_lossy(x).into()),
                        })
                    })
                    .collect()
            })
            .map_err(sql_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql_err)?;
        Ok(Out::Rows(rows))
    }

    fn rows(&mut self, t: &Table, sql: &str, params: Vec<Sql>, silent: bool) -> Result<Vec<Value>, RunError> {
        let Out::Rows(rows) = self.q(sql, params, false, silent)? else { unreachable!() };
        Ok(rows
            .into_iter()
            .map(|r| {
                let mut m = rec1("id", Value::Num(if let Sql::Num(n) = r[0] { n } else { f64::NAN }));
                for (c, v) in t.cols.iter().zip(&r[1..]) {
                    if let Some(v) = from_sql(c.ty, v) {
                        m.insert(c.name.clone(), v);
                    }
                }
                rec(m)
            })
            .collect())
    }

    fn ran(&mut self, sql: &str, params: Vec<Sql>) -> Result<(usize, i64), RunError> {
        let Out::Ran { changes, last_id } = self.q(sql, params, true, false)? else { unreachable!() };
        Ok((changes, last_id))
    }

    pub fn all(&mut self, t: &Table) -> Result<Vec<Value>, RunError> {
        self.rows(t, &format!("SELECT * FROM {} ORDER BY \"id\"", qi(&t.name)), vec![], false)
    }

    /// Every row of every table, without logging: what the data view shows.
    pub fn snapshot(&mut self, p: &Program) -> Result<IndexMap<Name, Vec<Value>>, RunError> {
        let all = |s: &mut Store, t: &Table| s.rows(t, &format!("SELECT * FROM {} ORDER BY \"id\"", qi(&t.name)), vec![], true);
        p.tables.values().map(|t| Ok((t.name.clone(), all(self, t)?))).collect()
    }

    pub fn get(&mut self, t: &Table, id: f64, silent: bool) -> Result<Option<Value>, RunError> {
        Ok(self.rows(t, &format!("SELECT * FROM {} WHERE \"id\" = ?", qi(&t.name)), vec![Sql::Num(id)], silent)?.into_iter().next())
    }

    pub fn find(&mut self, t: &Table, w: &Where, env: &Env) -> Result<Vec<Value>, RunError> {
        let ps = w
            .params
            .iter()
            .map(|p| match p {
                Param::Lit(v) => Ok(to_sql(v)),
                Param::Expr(e) => Ok(to_sql(&eval(e, env)?)),
            })
            .collect::<Result<_, RunError>>()?;
        self.rows(t, &format!("SELECT * FROM {} WHERE {} ORDER BY \"id\"", qi(&t.name), w.sql), ps, false)
    }

    pub fn insert(&mut self, t: &Table, v: &Value) -> Result<Option<Value>, RunError> {
        let r = check_row(t, v, &t.single)?;
        for c in t.cols.iter().filter(|c| !c.opt) {
            if r.get(&c.name).is_none_or(|v| matches!(v, Value::Nil)) {
                return fail(format!(
                    "{} needs {} — {}.{} is required (write {}? in the table to make it optional)",
                    t.single, c.name, t.name, c.name, c.ty.name()
                ));
            }
        }
        let keys: Vec<&Name> = r.iter().filter(|(_, v)| !matches!(v, Value::Nil)).map(|(k, _)| k).collect();
        let sql = if keys.is_empty() {
            format!("INSERT INTO {} DEFAULT VALUES", qi(&t.name))
        } else {
            format!("INSERT INTO {} ({}) VALUES ({})", qi(&t.name), keys.iter().map(|k| qi(k)).collect::<Vec<_>>().join(", "), vec!["?"; keys.len()].join(", "))
        };
        let (_, last_id) = self.ran(&sql, keys.iter().map(|k| to_sql_col(t, k, &r[*k])).collect())?;
        let id = match r.get("id") {
            Some(Value::Num(n)) => *n,
            _ => last_id as f64,
        };
        self.get(t, id, true)
    }

    pub fn update(&mut self, t: &Table, id: f64, changes: &Value) -> Result<Option<Value>, RunError> {
        let ch = check_row(t, changes, "changes")?;
        if ch.contains_key("id") {
            return fail("changes can’t include id");
        }
        for c in t.cols.iter().filter(|c| !c.opt) {
            if matches!(ch.get(&c.name), Some(Value::Nil)) {
                return fail(format!("{}.{} is required, so it can't be set to nil", t.name, c.name));
            }
        }
        if ch.is_empty() {
            return self.get(t, id, false);
        }
        let set: Vec<String> = ch.keys().map(|k| format!("{} = ?", qi(k))).collect();
        let mut ps: Vec<Sql> = ch.iter().map(|(k, v)| to_sql_col(t, k, v)).collect();
        ps.push(Sql::Num(id));
        let (changed, _) = self.ran(&format!("UPDATE {} SET {} WHERE \"id\" = ?", qi(&t.name), set.join(", ")), ps)?;
        if changed > 0 { self.get(t, id, true) } else { Ok(None) }
    }

    pub fn delete(&mut self, t: &Table, id: f64) -> Result<bool, RunError> {
        Ok(self.ran(&format!("DELETE FROM {} WHERE \"id\" = ?", qi(&t.name)), vec![Sql::Num(id)])?.0 > 0)
    }
}

fn sql_err(e: rusqlite::Error) -> RunError {
    let msg = match &e {
        rusqlite::Error::SqliteFailure(_, Some(m)) => m.clone(),
        e => e.to_string(),
    };
    RunError { msg: format!("SQLite: {msg}"), details: vec![] }
}
