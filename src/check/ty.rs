//! Static types and record shapes.
use crate::num::text_cmp;
use crate::value::{Name, Value};
use indexmap::{IndexMap, IndexSet};
use std::rc::Rc;

/// A path into the word's input: `card.balance`, `lines.[]`, `^1.total`, `body#only`.
pub type Key = Rc<str>;

#[derive(Clone, Debug)]
pub enum Ty {
    Num,
    Text,
    Bool,
    Nil,
    Any,
    Never,
    /// Known constants (sorted), or any constant.
    Const(Option<Rc<Vec<Name>>>),
    List(Rc<Ty>),
    /// A record; a change set made by ∆ also remembers its two sides.
    Rec(Rc<Shape>, Option<Rc<(Shape, Shape)>>),
    /// Whatever the input has at this key.
    Var(Key),
}

#[derive(Clone, Debug)]
pub struct Field {
    pub ty: Ty,
    pub opt: bool,
}

/// Where a shape's unnamed fields come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Nothing else: the record has exactly these fields.
    Closed,
    /// The input at this key ("" is the word's input row).
    Input(Key),
    /// Can't tell.
    Unknown,
}

impl Origin {
    pub fn input(&self) -> Option<&Key> {
        if let Origin::Input(k) = self { Some(k) } else { None }
    }
    pub fn sig(&self) -> String {
        match self {
            Origin::Closed => "null".into(),
            Origin::Input(k) => k.to_string(),
            Origin::Unknown => "*".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Shape {
    pub fields: IndexMap<Name, Field>,
    pub absent: IndexSet<Name>,
    pub origin: Origin,
}

impl Shape {
    pub fn new(fields: IndexMap<Name, Field>, absent: IndexSet<Name>, origin: Origin) -> Shape {
        Shape { fields, absent, origin }
    }
    pub fn closed(fields: IndexMap<Name, Field>) -> Shape {
        Shape::new(fields, IndexSet::new(), Origin::Closed)
    }
    pub fn open(origin: Origin) -> Shape {
        Shape::new(IndexMap::new(), IndexSet::new(), origin)
    }
    pub fn set(&self, name: &Name, ty: Ty, opt: bool) -> Shape {
        let mut s = self.clone();
        s.fields.insert(name.clone(), Field { ty, opt });
        s.absent.shift_remove(name);
        s
    }
    pub fn del(&self, name: &Name) -> Shape {
        let mut s = self.clone();
        s.fields.shift_remove(name);
        s.absent.insert(name.clone());
        s
    }
}

pub fn rec(s: Shape) -> Ty {
    Ty::Rec(Rc::new(s), None)
}

pub fn list(t: Ty) -> Ty {
    Ty::List(Rc::new(t))
}

pub fn konst(names: Vec<Name>) -> Ty {
    let mut n: Vec<Name> = names.into_iter().collect::<IndexSet<_>>().into_iter().collect();
    sort16(&mut n);
    Ty::Const(Some(Rc::new(n)))
}

/// A closed record from `(name, type, optional)`.
pub fn rec_of(entries: Vec<(&str, Ty, bool)>) -> Ty {
    rec(Shape::closed(entries.into_iter().map(|(k, ty, opt)| (k.into(), Field { ty, opt })).collect()))
}

pub fn sort16<S: AsRef<str>>(v: &mut [S]) {
    v.sort_by(|a, b| text_cmp(a.as_ref(), b.as_ref()));
}

pub fn child_key(origin: &str, f: &str) -> Key {
    if origin.is_empty() { f.into() } else { format!("{origin}.{f}").into() }
}

/// `lines.[].amount` → `lines.amount`; `lines.[]` → `lines[]`.
pub fn key_name(k: &str) -> String {
    let base: Vec<&str> = k.split('.').filter(|x| *x != "[]").collect();
    format!("{}{}", base.join("."), if k.ends_with(".[]") { "[]" } else { "" })
}

impl Ty {
    pub fn tag(&self) -> &'static str {
        match self {
            Ty::Num => "num",
            Ty::Text => "text",
            Ty::Bool => "bool",
            Ty::Nil => "nil",
            Ty::Any => "any",
            Ty::Never => "never",
            Ty::Const(_) => "const",
            Ty::List(_) => "list",
            Ty::Rec(..) => "rec",
            Ty::Var(_) => "var",
        }
    }
    pub fn is(&self, tag: &str) -> bool {
        self.tag() == tag
    }
}

pub fn lit_type(v: &Value) -> Ty {
    match v {
        Value::Nil => Ty::Nil,
        Value::Num(_) => Ty::Num,
        Value::Text(_) => Ty::Text,
        Value::Bool(_) => Ty::Bool,
        Value::Const(c) => konst(vec![c.clone()]),
        Value::List(xs) => list(xs.iter().fold(Ty::Never, |t, x| join(&t, &lit_type(x)))),
        Value::Rec(r) => rec(Shape::closed(r.iter().map(|(k, x)| (k.clone(), Field { ty: lit_type(x), opt: false })).collect())),
    }
}

pub fn join(a: &Ty, b: &Ty) -> Ty {
    match (a, b) {
        (Ty::Never, _) => b.clone(),
        (_, Ty::Never) => a.clone(),
        (Ty::Var(x), Ty::Var(y)) if x == y => a.clone(),
        _ if a.tag() != b.tag() || a.is("any") || a.is("var") => Ty::Any,
        (Ty::Const(x), Ty::Const(y)) => match (x, y) {
            (Some(x), Some(y)) => {
                let n: IndexSet<Name> = x.iter().chain(y.iter()).cloned().collect();
                if n.len() > 8 { Ty::Const(None) } else { konst(n.into_iter().collect()) }
            }
            _ => Ty::Const(None),
        },
        (Ty::List(x), Ty::List(y)) => list(join(x, y)),
        (Ty::Rec(x, _), Ty::Rec(y, _)) => rec(join_shapes(&[(**x).clone(), (**y).clone()])),
        _ => a.clone(),
    }
}

pub fn join_shapes(shapes: &[Shape]) -> Shape {
    if shapes.len() == 1 {
        return shapes[0].clone();
    }
    let names: IndexSet<Name> = shapes.iter().flat_map(|s| s.fields.keys().cloned()).collect();
    let fields = names
        .into_iter()
        .map(|k| {
            let (mut ty, mut opt) = (Ty::Never, false);
            for s in shapes {
                match s.fields.get(&k) {
                    None => opt = true,
                    Some(f) => {
                        ty = join(&ty, &f.ty);
                        opt |= f.opt;
                    }
                }
            }
            (k, Field { ty, opt })
        })
        .collect();
    let absent = shapes[0].absent.iter().filter(|k| shapes.iter().all(|s| s.absent.contains(*k))).cloned().collect();
    let origin = if shapes.iter().all(|s| s.origin == shapes[0].origin) { shapes[0].origin.clone() } else { Origin::Closed };
    Shape::new(fields, absent, origin)
}

pub fn meet(a: &Ty, b: &Ty) -> Option<Ty> {
    match (a, b) {
        (_, Ty::Any) => Some(a.clone()),
        (Ty::Any, _) => Some(b.clone()),
        _ if a.tag() != b.tag() => None,
        (Ty::Const(x), Ty::Const(y)) => match (x, y) {
            (None, _) => Some(b.clone()),
            (_, None) => Some(a.clone()),
            (Some(x), Some(y)) => {
                let n: Vec<Name> = x.iter().filter(|n| y.contains(n)).cloned().collect();
                (!n.is_empty()).then(|| konst(n))
            }
        },
        (Ty::List(x), Ty::List(y)) => meet(x, y).map(list),
        _ => Some(a.clone()),
    }
}

// ---- keys that identify types and shapes, for caching and deduplication ----
pub fn type_sig(t: &Ty) -> String {
    match t {
        Ty::Const(Some(n)) => format!("C:{}", n.join("|")),
        Ty::Const(None) => "C".into(),
        Ty::List(of) => format!("[{}]", type_sig(of)),
        Ty::Rec(s, _) => shape_sig(s),
        Ty::Var(k) => format!("${k}"),
        t => t.tag().into(),
    }
}

pub fn shape_sig(s: &Shape) -> String {
    let mut ks: Vec<&Name> = s.fields.keys().collect();
    sort16(&mut ks);
    let mut ab: Vec<&Name> = s.absent.iter().collect();
    sort16(&mut ab);
    let fs: Vec<String> = ks.iter().map(|k| format!("{k}{}:{}", if s.fields[*k].opt { "?" } else { "" }, type_sig(&s.fields[*k].ty))).collect();
    format!("{{{}|¬{}|{}}}", fs.join(","), ab.iter().map(|k| &***k).collect::<Vec<_>>().join(","), s.origin.sig())
}

pub fn col_type(c: crate::ast::ColType) -> Ty {
    use crate::ast::ColType::*;
    match c {
        Int | Num => Ty::Num,
        Text => Ty::Text,
        Bool => Ty::Bool,
        Const => Ty::Const(None),
        Json => Ty::Any,
    }
}
