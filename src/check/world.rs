//! Worlds: one possible situation inside a word, with what it assumes about the input.
use super::ty::*;
use super::{Cx, Sig};
use crate::num::text_cmp;
use crate::value::{Name, Value};
use indexmap::{IndexMap, IndexSet};
use std::rc::Rc;

/// A place in a clause, with the line relative to the clause's chunk.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct At {
    pub word: Name,
    pub ci: usize,
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Src {
    /// Must hold, or the caller is wrong.
    Pre,
    /// The clause pattern.
    Cond,
    /// A callee dispatches on it.
    Lift,
    Assume,
}

impl Src {
    pub fn name(self) -> &'static str {
        match self {
            Src::Pre => "pre",
            Src::Cond => "cond",
            Src::Lift => "lift",
            Src::Assume => "assume",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    List,
    Rec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Stored by insert.
    Row,
    /// Given to update.
    Changes,
    /// The old side of a change set given to update.
    Old,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Apart {
    pub other: Key,
    pub what: String,
    pub except: Vec<Name>,
}

/// Promises about fields nobody has named yet, checked where the record's shape is known.
#[derive(Clone, Debug, PartialEq)]
pub enum Obl {
    /// The record may only have these fields (it is stored as a table row).
    Only { only: Vec<Name>, table: Name, role: Role },
    /// The record shares no fields with others (they are spliced together).
    Apart(Vec<Apart>),
}

#[derive(Clone, Debug)]
pub struct Fact {
    pub has: bool,
    pub ty: Option<Ty>,
    pub src: Src,
    pub lit: Option<Value>,
    pub soft: bool,
    pub kind: Option<Kind>,
    pub via: Vec<At>,
    pub obl: Option<Obl>,
}

impl Fact {
    pub fn new(has: bool, ty: Option<Ty>, src: Src) -> Fact {
        Fact { has, ty, src, lit: None, soft: false, kind: None, via: vec![], obl: None }
    }
}

#[derive(Clone, Debug)]
pub struct Fail {
    pub msg: String,
    pub at: Option<At>,
    pub reported: bool,
}

/// A place where a world's path forked: what forked it, its branches (a label and the field
/// each leaves), and which one this world took.
#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    pub at: At,
    pub end: At,
    /// `notes.get`
    pub what: String,
    /// `may or may not find a row with that id`
    pub may: String,
    pub branches: std::rc::Rc<Vec<(String, Name)>>,
    pub took: usize,
}

#[derive(Clone, Debug)]
pub struct World {
    pub shape: Shape,
    /// The splits that made this world, oldest first.
    pub trail: Vec<Split>,
    pub facts: IndexMap<Key, Fact>,
    pub writes: IndexSet<Name>,
    pub lifted: bool,
    pub fail: Option<Fail>,
    pub diverge: bool,
    pub outer: Vec<Shape>,
    pub guard_mode: bool,
}

impl World {
    pub fn new(shape: Shape) -> World {
        World { shape, trail: vec![], facts: IndexMap::new(), writes: IndexSet::new(), lifted: false, fail: None, diverge: false, outer: vec![], guard_mode: false }
    }
    pub fn with_shape(&self, shape: Shape) -> World {
        World { shape, ..self.clone() }
    }
    pub fn lifted(&self) -> World {
        World { lifted: true, ..self.clone() }
    }
    pub fn failed(&self, f: Fail) -> World {
        World { fail: Some(f), ..self.clone() }
    }
    pub fn diverged(&self) -> World {
        World { diverge: true, ..self.clone() }
    }
    pub fn live(&self) -> bool {
        self.fail.is_none() && !self.diverge
    }
    pub fn set(&mut self, k: &Key, f: Fact) {
        self.facts.insert(k.clone(), f);
    }
    pub fn write(&mut self, n: &Name) {
        self.writes.insert(n.clone());
    }
}

/// A type error. `at`/`top_msg` say where to report it when it surfaces at the entry.
#[derive(Clone, Debug)]
pub struct TErr {
    pub msg: String,
    pub at: Option<At>,
    pub top_msg: Option<String>,
}

pub type R<T> = Result<T, TErr>;

pub fn terr<T>(msg: impl Into<String>) -> R<T> {
    Err(TErr { msg: msg.into(), at: None, top_msg: None })
}

/// Follow variables to what the input facts say they are.
pub fn rt(w: &World, t: &Ty) -> Ty {
    let mut t = t.clone();
    for _ in 0..20 {
        let Ty::Var(k) = &t else { break };
        match w.facts.get(k) {
            Some(Fact { has: true, ty: Some(ty), .. }) if !ty.is("any") => t = ty.clone(),
            _ => return Ty::Any,
        }
    }
    t
}

#[derive(Clone, Debug)]
pub enum Look {
    Present(Ty, Option<Key>),
    Opt(Ty),
    Absent,
    Unknown(Key),
}

pub fn lookup(w: &World, s: &Shape, name: &str) -> Look {
    if let Some(fd) = s.fields.get(name) {
        return if fd.opt { Look::Opt(fd.ty.clone()) } else { Look::Present(fd.ty.clone(), None) };
    }
    let origin = match &s.origin {
        _ if s.absent.contains(name) => return Look::Absent,
        Origin::Closed => return Look::Absent,
        Origin::Unknown => return Look::Present(Ty::Any, None),
        Origin::Input(o) => o,
    };
    let key = child_key(origin, name);
    match w.facts.get(&key) {
        Some(f) if f.has => Look::Present(Ty::Var(key.clone()), Some(key)),
        Some(_) => Look::Absent,
        None => Look::Unknown(key),
    }
}

impl Cx<'_> {
    pub fn need_in(&self, w: &mut World, s: &Shape, name: &str, place: &str) -> R<Ty> {
        match lookup(w, s, name) {
            Look::Present(t, _) => Ok(t),
            Look::Unknown(key) => {
                let f = if w.guard_mode {
                    Fact { soft: true, ..Fact::new(true, Some(Ty::Any), Src::Cond) }
                } else {
                    Fact { via: self.here.iter().cloned().collect(), ..Fact::new(true, Some(Ty::Any), Src::Pre) }
                };
                w.set(&key, f);
                Ok(Ty::Var(key))
            }
            Look::Opt(_) => terr(format!("{name} might be missing — {place} is {}; match on {{{name}}} first", fmt_w(Some(w), s, 0))),
            Look::Absent => terr(format!("needs {name} — {place} is {}", fmt_w(Some(w), s, 0))),
        }
    }

    pub fn need(&self, w: &mut World, s: &Shape, name: &str) -> R<Ty> {
        self.need_in(w, s, name, "the record here")
    }

    /// `need` on the world's own shape.
    pub fn need_here(&self, w: &mut World, name: &str) -> R<Ty> {
        let s = w.shape.clone();
        self.need(w, &s, name)
    }
}

pub fn need_absent(w: &mut World, s: &Shape, name: &str) -> R<()> {
    match lookup(w, s, name) {
        Look::Absent => Ok(()),
        Look::Unknown(key) => {
            w.set(&key, Fact::new(false, None, Src::Pre));
            Ok(())
        }
        l => terr(format!(
            "{name} {} there — the record is {}; use {name}=… to change it",
            if matches!(l, Look::Opt(_)) { "might already be" } else { "is already" },
            fmt_w(Some(w), s, 0)
        )),
    }
}

pub fn constrain(w: &mut World, t: &Ty, want: &Ty, what: &str) -> R<Ty> {
    if let Ty::Var(key) = t {
        let f = w.facts.get(key).cloned();
        let cur = f.as_ref().and_then(|f| f.ty.clone()).unwrap_or(Ty::Any);
        let Some(m) = meet(&cur, want) else {
            return terr(format!("{what}: {} is used as {} here, but it's {}", key_name(key), fmt_t(Some(w), want, 0), fmt_t(Some(w), &cur, 0)));
        };
        if let Some(f) = f {
            w.set(key, Fact { ty: Some(m.clone()), ..f });
        }
        return Ok(m);
    }
    let r = rt(w, t);
    if r.is("any") {
        return Ok(want.clone());
    }
    if meet(&r, want).is_none() {
        return terr(format!("{what} needs {}, got {}", fmt_t(Some(w), want, 0), fmt_t(Some(w), &r, 0)));
    }
    Ok(r)
}

pub fn as_rec(w: &mut World, t: &Ty, what: &str) -> R<Shape> {
    match t {
        Ty::Var(key) => {
            let f = w.facts.get(key).cloned();
            let cur = f.as_ref().and_then(|f| f.ty.clone()).unwrap_or(Ty::Any);
            match cur {
                Ty::Any => {
                    let s = Shape::open(Origin::Input(key.clone()));
                    if let Some(f) = f {
                        w.set(key, Fact { ty: Some(rec(s.clone())), ..f });
                    }
                    Ok(s)
                }
                Ty::Rec(s, _) => Ok((*s).clone()),
                cur => terr(format!("{what} needs a record, but {} is {}", key_name(key), fmt_t(Some(w), &cur, 0))),
            }
        }
        Ty::Rec(s, _) => Ok((**s).clone()),
        Ty::Any => Ok(Shape::open(Origin::Unknown)),
        t => terr(format!("{what} needs a record, got {}", fmt_t(Some(w), t, 0))),
    }
}

pub fn as_list(w: &mut World, t: &Ty, what: &str) -> R<Ty> {
    match t {
        Ty::Var(key) => {
            let f = w.facts.get(key).cloned();
            let cur = f.as_ref().and_then(|f| f.ty.clone()).unwrap_or(Ty::Any);
            match cur {
                Ty::Any => {
                    let ek: Key = format!("{key}.[]").into();
                    if let Some(f) = f {
                        w.set(key, Fact { ty: Some(list(Ty::Var(ek.clone()))), ..f });
                    }
                    w.set(&ek, Fact::new(true, Some(Ty::Any), Src::Pre));
                    Ok(Ty::Var(ek))
                }
                Ty::List(of) => Ok((*of).clone()),
                cur => terr(format!("{what} needs a list, but {} is {}", key_name(key), fmt_t(Some(w), &cur, 0))),
            }
        }
        Ty::List(of) => Ok((**of).clone()),
        Ty::Any => Ok(Ty::Any),
        t => terr(format!("{what} needs a list, got {}", fmt_t(Some(w), t, 0))),
    }
}

// ---- display ----
pub fn fmt_t(w: Option<&World>, t: &Ty, d: usize) -> String {
    let t = match w {
        Some(w) => rt(w, t),
        None => t.clone(),
    };
    match &t {
        Ty::Num => "Num".into(),
        Ty::Text => "Text".into(),
        Ty::Bool => "Bool".into(),
        Ty::Nil => "nil".into(),
        Ty::Any | Ty::Var(_) => "Any".into(),
        Ty::Never => "Never".into(),
        Ty::Const(Some(n)) => n.iter().map(|c| format!(":{c}")).collect::<Vec<_>>().join("|"),
        Ty::Const(None) => "Const".into(),
        Ty::List(of) => format!("[{}]", fmt_t(w, of, d + 1)),
        Ty::Rec(_, _) if d > 3 => "{…}".into(),
        Ty::Rec(_, Some(delta)) => format!("Δ({} → {})", fmt_w(w, &delta.0, d + 1), fmt_w(w, &delta.1, d + 1)),
        Ty::Rec(s, None) => fmt_w(w, s, d + 1),
    }
}

/// The fields a shape is known to have or lack, including input facts.
pub struct View {
    pub present: IndexMap<Name, Field>,
    pub gone: IndexSet<Name>,
}

pub fn shape_view(facts: Option<&IndexMap<Key, Fact>>, s: &Shape) -> View {
    let mut present = s.fields.clone();
    let mut gone = s.absent.clone();
    if let (Some(facts), Origin::Input(o)) = (facts, &s.origin) {
        let pre = if o.is_empty() { String::new() } else { format!("{o}.") };
        for (k, f) in facts {
            let Some(rest) = k.strip_prefix(pre.as_str()) else { continue };
            if rest.is_empty() || rest.contains('.') || rest.contains('#') || f.src == Src::Assume {
                continue;
            }
            if present.contains_key(rest) || gone.contains(rest) {
                continue;
            }
            if f.has {
                present.insert(rest.into(), Field { ty: Ty::Var(k.clone()), opt: false });
            } else {
                gone.insert(rest.into());
            }
        }
    }
    View { present, gone }
}

pub fn view(w: &World, s: &Shape) -> View {
    shape_view(Some(&w.facts), s)
}

pub fn fmt_w(w: Option<&World>, s: &Shape, d: usize) -> String {
    let v = shape_view(w.map(|w| &w.facts), s);
    let mut parts = vec![];
    match &s.origin {
        Origin::Input(o) if o.is_empty() => parts.push("..r".to_string()),
        Origin::Unknown => parts.push("..".into()),
        Origin::Input(o) => parts.push(format!("..{}", key_name(o).rsplit('.').next().unwrap().chars().next().map(String::from).unwrap_or_default())),
        Origin::Closed => {}
    }
    for (k, f) in &v.present {
        let ty = fmt_t(w, &f.ty, d);
        parts.push(format!("{k}{}{}", if f.opt { "?" } else { "" }, if ty == "Any" { String::new() } else { format!(": {ty}") }));
    }
    let mut gone: Vec<&Name> = v.gone.iter().collect();
    sort16(&mut gone);
    parts.extend(gone.iter().map(|k| format!("¬{k}")));
    format!("{{{}}}", parts.join(", "))
}

// ---- obligations about fields nobody has named yet ----
fn obl_key(origin: &str, kind: &str) -> Key {
    format!("{origin}#{kind}").into()
}

pub fn add_only(w: &mut World, origin: &str, allowed: &[Name], table: &Name, role: Role) {
    let key = obl_key(origin, "only");
    let obl = match w.facts.get(&key).and_then(|f| f.obl.clone()) {
        Some(Obl::Only { only, table, role }) => Obl::Only { only: only.into_iter().filter(|x| allowed.contains(x)).collect(), table, role },
        _ => Obl::Only { only: allowed.to_vec(), table: table.clone(), role },
    };
    w.set(&key, Fact { obl: Some(obl), ..Fact::new(true, None, Src::Pre) });
}

pub fn only_msg(cx: &Cx, name: &str, table: &str, role: Role, what: &str, extra: &[Name]) -> String {
    let t = cx.p.tables.get(table);
    let list = extra.iter().map(|k| &**k).collect::<Vec<_>>().join(", ");
    if role == Role::Old {
        let why: Vec<String> =
            extra.iter().map(|k| if t.is_some_and(|t| t.col(k).is_some()) { format!("{table}.{k} is required") } else { format!("{table} has no column {k}") }).collect();
        return format!(
            "{name} would clear {list} in {table}: the new side of the change set leaves {} out, and {}",
            if extra.len() > 1 { "them" } else { "it" },
            why.join("; ")
        );
    }
    let as_what = if role == Role::Row {
        let article = if table.chars().next().is_some_and(|c| "aeiouAEIOU".contains(c)) { "an" } else { "a" };
        format!("{article} {table} row")
    } else {
        format!("{table} changes")
    };
    let why: Vec<String> = extra.iter().map(|k| format!("{table} has no column {k}")).collect();
    format!("{name} stores {what} as {as_what}, but it has {list} — {}", why.join("; "))
}

pub fn add_apart(w: &mut World, a: &Key, b: &Key, what: &str, except: &[Name]) {
    let (a, b) = if text_cmp(a, b).is_gt() { (b, a) } else { (a, b) };
    let key = obl_key(a, "apart");
    let mut list = match w.facts.get(&key).and_then(|f| f.obl.clone()) {
        Some(Obl::Apart(l)) => l.into_iter().filter(|x| &x.other != b).collect(),
        _ => vec![],
    };
    list.push(Apart { other: b.clone(), what: what.into(), except: except.to_vec() });
    w.set(&key, Fact { obl: Some(Obl::Apart(list)), ..Fact::new(true, None, Src::Pre) });
}

/// A and B must share no fields; unknown parts become promises for the caller.
pub fn require_apart(w: &mut World, a: &Shape, b: &Shape, what: &str, except: &[Name]) -> R<()> {
    if a.origin == Origin::Unknown || b.origin == Origin::Unknown {
        return terr(format!("{what}: can't tell which fields these records have"));
    }
    let (va, vb) = (view(w, a), view(w, b));
    let ka: Vec<Name> = va.present.keys().filter(|k| !except.contains(k)).cloned().collect();
    let both: Vec<&str> = ka.iter().filter(|k| vb.present.contains_key(*k)).map(|k| &**k).collect();
    if !both.is_empty() {
        return terr(format!("{what}: both records have {}", both.join(", ")));
    }
    if b.origin != Origin::Closed {
        for k in &ka {
            need_absent(w, b, k)?;
        }
    }
    if a.origin != Origin::Closed {
        for k in vb.present.keys() {
            need_absent(w, a, k)?;
        }
    }
    if let (Origin::Input(x), Origin::Input(y)) = (&a.origin, &b.origin) {
        add_apart(w, x, y, what, except);
    }
    Ok(())
}

pub fn sig_of(cx: &Cx, name: &str) -> Option<Rc<Sig>> {
    cx.local.get(name).or_else(|| cx.sigs.get(name)).cloned()
}
