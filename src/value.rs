//! Runtime values, how they print, and the record algebra (∆ ⍎ ⊔ ⊓ ⊑ .{}).
use crate::num;
use indexmap::IndexMap;
use std::rc::Rc;

pub type Name = Rc<str>;
/// Records keep insertion order; output order depends on it.
pub type Rec = IndexMap<Name, Value>;

#[derive(Clone, Debug)]
pub enum Value {
    Nil,
    Bool(bool),
    Num(f64),
    Text(Rc<str>),
    Const(Name),
    List(Rc<Vec<Value>>),
    Rec(Rc<Rec>),
}

#[derive(Debug, Clone)]
pub struct RunError {
    pub msg: String,
    pub details: Vec<String>,
}

pub fn fail<T>(msg: impl Into<String>) -> Result<T, RunError> {
    Err(RunError { msg: msg.into(), details: vec![] })
}

pub fn list(items: Vec<Value>) -> Value {
    Value::List(Rc::new(items))
}

pub fn rec(r: Rec) -> Value {
    Value::Rec(Rc::new(r))
}

pub fn rec1(k: &str, v: Value) -> Rec {
    Rec::from([(k.into(), v)])
}

impl Value {
    pub fn show(&self) -> String {
        match self {
            Value::Nil => "nil".into(),
            Value::Bool(b) => b.to_string(),
            Value::Num(n) => num::show(*n),
            Value::Text(s) => num::json_str(s),
            Value::Const(c) => c.to_string(),
            Value::List(xs) => format!("[{}]", xs.iter().map(Value::show).collect::<Vec<_>>().join(", ")),
            Value::Rec(r) => show_rec(r),
        }
    }

    /// Strings print bare; everything else as `show`.
    pub fn text(&self) -> String {
        match self {
            Value::Text(s) => s.to_string(),
            v => v.show(),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "a boolean",
            Value::Num(_) => "a number",
            Value::Text(_) => "a string",
            Value::Const(_) => "a constant",
            Value::List(_) => "a list",
            Value::Rec(_) => "a record",
        }
    }

    pub fn truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    pub fn as_rec(&self) -> Option<&Rc<Rec>> {
        if let Value::Rec(r) = self { Some(r) } else { None }
    }

    /// `type name + shown value`, as in most runtime errors.
    pub fn described(&self) -> String {
        format!("{} {}", self.type_name(), self.show())
    }
}

pub fn show_rec(r: &Rec) -> String {
    format!("{{{}}}", r.iter().map(|(k, v)| format!("{k}: {}", v.show())).collect::<Vec<_>>().join(", "))
}

pub fn eqv(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Num(x), Value::Num(y)) => x == y,
        (Value::Text(x), Value::Text(y)) => x == y,
        (Value::Const(x), Value::Const(y)) => x == y,
        (Value::List(x), Value::List(y)) => x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| eqv(p, q)),
        (Value::Rec(x), Value::Rec(y)) => eqv_rec(x, y),
        _ => false,
    }
}

pub fn eqv_rec(x: &Rec, y: &Rec) -> bool {
    x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| eqv(v, w)))
}

// ---- record algebra ----
// A change set (a ∆ b) is a plain record: each field holds one tag record,
// {same: v} | {set: new, was: old} | {add: v} | {del: old}, so clauses can match on it.
const TAGS: [&str; 4] = ["same", "set", "add", "del"];

fn tag(k: &str, v: &Value, was: Option<&Value>) -> Value {
    let mut m = rec1(k, v.clone());
    if let Some(w) = was {
        m.insert("was".into(), w.clone());
    }
    rec(m)
}

pub fn rec_args<'a>(a: &'a Value, b: &'a Value, op: &str) -> Result<(&'a Rec, &'a Rec), RunError> {
    match (a, b) {
        (Value::Rec(x), Value::Rec(y)) => Ok((x, y)),
        _ => fail(format!("{op} needs two records, got {} and {}", a.described(), b.described())),
    }
}

pub fn is_delta(v: &Value) -> bool {
    let Value::Rec(d) = v else { return false };
    d.values().all(|e| {
        let Value::Rec(e) = e else { return false };
        e.keys().all(|k| TAGS.contains(&&**k) || &**k == "was") && e.keys().filter(|k| TAGS.contains(&&***k)).count() == 1
    })
}

pub fn diff(a: &Rec, b: &Rec) -> Rec {
    let mut d = Rec::new();
    for (k, x) in a {
        let e = match b.get(k) {
            None => tag("del", x, None),
            Some(y) if eqv(x, y) => tag("same", x, None),
            Some(y) => tag("set", y, Some(x)),
        };
        d.insert(k.clone(), e);
    }
    for (k, y) in b {
        if !a.contains_key(k) {
            d.insert(k.clone(), tag("add", y, None));
        }
    }
    d
}

fn tag_of<'a>(e: &'a Value, t: &str) -> Option<&'a Value> {
    e.as_rec().and_then(|r| r.get(t))
}

pub fn apply_delta(x: &Value, d: &Value) -> Result<Value, RunError> {
    let Value::Rec(x) = x else { return fail(format!("⍎ applies a change set to a record, got {}", x.described())) };
    if !is_delta(d) {
        return fail(format!("⍎ needs a change set made by ∆, got {}", d.show()));
    }
    let mut n = (**x).clone();
    for (k, e) in d.as_rec().unwrap().iter() {
        if tag_of(e, "del").is_some() {
            n.shift_remove(k);
        } else if let Some(v) = tag_of(e, "set").or_else(|| tag_of(e, "add")) {
            n.insert(k.clone(), v.clone());
        }
    }
    Ok(rec(n))
}

/// What `.update` needs from a change set: new values, and nil for a deleted column.
pub fn delta_changes(d: &Rec) -> Rec {
    d.iter()
        .filter(|(_, e)| tag_of(e, "same").is_none())
        .map(|(k, e)| {
            let v = if tag_of(e, "del").is_some() { Value::Nil } else { tag_of(e, "set").or_else(|| tag_of(e, "add")).unwrap().clone() };
            (k.clone(), v)
        })
        .collect()
}

/// a ⊔ b: merged record, and the fields where they disagree.
pub fn unify(a: &Rec, b: &Rec) -> (Rec, Rec) {
    let mut merged = a.clone();
    let mut conflict = Rec::new();
    for (k, y) in b {
        match a.get(k) {
            Some(x) if !eqv(x, y) => {
                conflict.insert(k.clone(), rec(Rec::from([("left".into(), x.clone()), ("right".into(), y.clone())])));
            }
            _ => {
                merged.insert(k.clone(), y.clone());
            }
        }
    }
    (merged, conflict)
}

pub fn meet(a: &Rec, b: &Rec) -> Rec {
    a.iter()
        .filter_map(|(k, x)| {
            let y = b.get(k)?;
            let v = if eqv(x, y) {
                rec1("agree", x.clone())
            } else {
                rec1("differ", rec(Rec::from([("left".into(), x.clone()), ("right".into(), y.clone())])))
            };
            Some((k.clone(), rec(v)))
        })
        .collect()
}

pub fn subsumes(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Rec(x), Value::Rec(y)) => x.iter().all(|(k, v)| y.get(k).is_some_and(|w| subsumes(v, w))),
        _ => eqv(a, b),
    }
}

pub fn project(v: &Value, names: &[Name]) -> Result<Value, RunError> {
    match v {
        Value::List(xs) => Ok(list(xs.iter().map(|x| project(x, names)).collect::<Result<_, _>>()?)),
        Value::Rec(r) => Ok(rec(names.iter().filter_map(|n| Some((n.clone(), r.get(n)?.clone()))).collect())),
        _ => fail(format!(
            ".{{{}}} needs a record or a list of records, got {}",
            names.iter().map(|n| &**n).collect::<Vec<_>>().join(" "),
            v.described()
        )),
    }
}

// ---- JSON, for Json columns: constants travel as {"$sym": name} ----
pub fn to_json(v: &Value) -> String {
    json(v, false)
}

/// JSON for the outside world: a constant is just its name.
pub fn to_plain_json(v: &Value) -> String {
    json(v, true)
}

fn json(v: &Value, plain: bool) -> String {
    match v {
        Value::Nil => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Num(n) if n.is_finite() => num::js(*n),
        Value::Num(_) => "null".into(),
        Value::Text(s) => num::json_str(s),
        Value::Const(c) if plain => num::json_str(c),
        Value::Const(c) => format!("{{\"$sym\":{}}}", num::json_str(c)),
        Value::List(xs) => format!("[{}]", xs.iter().map(|x| json(x, plain)).collect::<Vec<_>>().join(",")),
        Value::Rec(r) => format!("{{{}}}", r.iter().map(|(k, x)| format!("{}:{}", num::json_str(k), json(x, plain))).collect::<Vec<_>>().join(",")),
    }
}

pub fn from_json(s: &str) -> Option<Value> {
    let mut p = Json { s: s.as_bytes(), i: 0 };
    let v = p.value()?;
    p.ws();
    (p.i == s.len()).then_some(v)
}

struct Json<'a> {
    s: &'a [u8],
    i: usize,
}

impl Json<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        let ok = self.s.get(self.i) == Some(&c);
        if ok {
            self.i += 1;
        }
        ok
    }
    fn lit(&mut self, w: &str, v: Value) -> Option<Value> {
        self.s[self.i..].starts_with(w.as_bytes()).then(|| {
            self.i += w.len();
            v
        })
    }
    fn value(&mut self) -> Option<Value> {
        self.ws();
        match *self.s.get(self.i)? {
            b'n' => self.lit("null", Value::Nil),
            b't' => self.lit("true", Value::Bool(true)),
            b'f' => self.lit("false", Value::Bool(false)),
            b'"' => Some(Value::Text(self.string()?.into())),
            b'[' => {
                self.i += 1;
                let mut xs = vec![];
                if !self.eat(b']') {
                    loop {
                        xs.push(self.value()?);
                        if self.eat(b']') {
                            break;
                        }
                        if !self.eat(b',') {
                            return None;
                        }
                    }
                }
                Some(list(xs))
            }
            b'{' => {
                self.i += 1;
                let mut r = Rec::new();
                if !self.eat(b'}') {
                    loop {
                        self.ws();
                        let k = self.string()?;
                        if !self.eat(b':') {
                            return None;
                        }
                        r.insert(k.into(), self.value()?);
                        if self.eat(b'}') {
                            break;
                        }
                        if !self.eat(b',') {
                            return None;
                        }
                    }
                }
                if r.len() == 1 && r.contains_key("$sym")
                    && let Value::Text(s) = &r["$sym"] {
                        return Some(Value::Const(s.clone()));
                    }
                Some(rec(r))
            }
            _ => {
                let st = self.i;
                while self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[st..self.i]).ok()?.parse().ok().map(Value::Num)
            }
        }
    }
    fn string(&mut self) -> Option<String> {
        if self.s.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        let mut units: Vec<u16> = vec![];
        loop {
            let c = *self.s.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return Some(String::from_utf16_lossy(&units)),
                b'\\' => {
                    let e = *self.s.get(self.i)?;
                    self.i += 1;
                    match e {
                        b'n' => units.push(10),
                        b't' => units.push(9),
                        b'r' => units.push(13),
                        b'b' => units.push(8),
                        b'f' => units.push(12),
                        b'u' => {
                            let h = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
                            units.push(u16::from_str_radix(h, 16).ok()?);
                            self.i += 4;
                        }
                        c => units.push(c as u16),
                    }
                }
                _ => {
                    // copy one UTF-8 character
                    let st = self.i - 1;
                    let w = match c {
                        0xF0.. => 4,
                        0xE0.. => 3,
                        0xC0.. => 2,
                        _ => 1,
                    };
                    self.i = st + w;
                    units.extend(std::str::from_utf8(self.s.get(st..self.i)?).ok()?.encode_utf16());
                }
            }
        }
    }
}
