//! Applying a callee's signature at a call site: split the caller's worlds by what the callee
//! dispatches on, check its preconditions, and lay its outputs over the caller's shape.
use super::sig::*;
use super::ty::*;
use super::world::*;
use super::Cx;
use crate::ast::Pos;
use crate::value::{Name, Value};
use indexmap::{IndexMap, IndexSet};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum St {
    Present,
    Opt,
    Absent,
    Unknown,
    UnknownParent,
}

pub struct Loc {
    pub st: St,
    pub ty: Option<Ty>,
    pub key: Option<Key>,
    /// The shape holding the last segment.
    pub shape: Option<Shape>,
    pub name: Name,
}

fn loc_of(l: Look, shape: Option<Shape>, name: &str) -> Loc {
    let (st, ty, key) = match l {
        Look::Present(t, k) => (St::Present, Some(t), k),
        Look::Opt(t) => (St::Opt, Some(t), None),
        Look::Absent => (St::Absent, None, None),
        Look::Unknown(k) => (St::Unknown, None, Some(k)),
    };
    Loc { st, ty, key, shape, name: name.into() }
}

fn absent_loc() -> Loc {
    Loc { st: St::Absent, ty: None, key: None, shape: None, name: "".into() }
}

/// `^n.rest` paths start at an enclosing record.
fn outer_base<'p>(w: &World, s: &Shape, path: &'p [&'p str]) -> Option<(Shape, &'p [&'p str])> {
    let p0 = path.first().copied().unwrap_or("");
    if p0.len() > 1 && p0.starts_with('^') && p0[1..].bytes().all(|b| b.is_ascii_digit()) {
        let n: usize = p0[1..].parse().ok()?;
        return w.outer.get(n.checked_sub(1)?).map(|o| (o.clone(), &path[1..]));
    }
    Some((s.clone(), path))
}

impl Cx<'_> {
    /// Find a (possibly nested) input path inside the caller's current shape.
    pub fn locate(&self, w: &mut World, s0: &Shape, path0: &[&str], create: bool) -> R<Loc> {
        let Some((s, path)) = outer_base(w, s0, path0) else { return Ok(absent_loc()) };
        let mut t = rec(s);
        for i in 0..path.len().saturating_sub(1) {
            let seg = path[i];
            if seg == "[]" {
                t = as_list(w, &t, &format!("reading {}", key_name(&path[..i].join("."))))?;
                continue;
            }
            let sh = as_rec(w, &t, &format!("reading {}", key_name(&path.join("."))))?;
            match lookup(w, &sh, seg) {
                Look::Unknown(key) if create => {
                    w.set(&key, Fact::new(true, Some(Ty::Any), Src::Pre));
                    t = Ty::Var(key);
                }
                Look::Present(ty, _) => t = ty,
                Look::Unknown(_) => return Ok(Loc { st: St::UnknownParent, ..absent_loc() }),
                _ => return Ok(absent_loc()),
            }
        }
        let name = path.last().copied().unwrap_or("");
        if name == "[]" {
            let el = as_list(w, &t, &format!("reading {}", key_name(&path.join("."))))?;
            return Ok(Loc { st: St::Present, ty: Some(el), key: None, shape: None, name: name.into() });
        }
        let sh = as_rec(w, &t, &format!("reading {}", key_name(&path.join("."))))?;
        Ok(loc_of(lookup(w, &sh, name), Some(sh), name))
    }
}

/// Apply `f` to the shape holding the last path segment, rebuilding local records on the way.
fn rebuild_at(w: &World, s: &Shape, path: &[&str], f: &dyn Fn(&Shape) -> Shape) -> Shape {
    if path.len() == 1 {
        return f(s);
    }
    let Some(r) = s.fields.get(path[0]) else { return s.clone() };
    let Ty::Rec(inner, _) = rt(w, &r.ty) else { return s.clone() };
    s.set(&path[0].into(), rec(rebuild_at(w, &inner, &path[1..], f)), r.opt)
}

/// One world of a split: `w` is None for the world being split itself, which the split
/// updates in place (the caller keeps using it), or a copy.
pub struct Split {
    pub w: Option<World>,
    pub yes: bool,
    pub exact: bool,
}

impl Cx<'_> {
    /// Split `root` by conditions: worlds where they hold (yes) and where they don't.
    /// `exact` is false when a "no" world might still satisfy them.
    pub fn cond_split(&self, root: &mut World, conds: &[CondItem]) -> Vec<Split> {
        let mut parts = vec![Split { w: None, yes: true, exact: true }];
        for c in conds {
            let base = c.key.strip_suffix("#kind");
            let path: Vec<&str> = base.unwrap_or(&c.key).split('.').collect();
            let mut next = vec![];
            for p in parts {
                if !p.yes {
                    next.push(p);
                    continue;
                }
                match p.w {
                    None => next.extend(self.split_one(root, c, &path, base.is_some(), p.exact)),
                    Some(mut w) => {
                        let r = self.split_one(&mut w, c, &path, base.is_some(), p.exact);
                        next.extend(r.into_iter().map(|q| Split { w: q.w.or_else(|| Some(w.clone())), ..q }));
                    }
                }
            }
            parts = next;
        }
        parts
    }

    /// Split one world by one condition. Results that keep the world itself have no `w`.
    fn split_one(&self, w: &mut World, c: &CondItem, path: &[&str], kind_of: bool, exact: bool) -> Vec<Split> {
        let same = |yes: bool, exact: bool| Split { w: None, yes, exact };
        let own = |w: World, yes: bool, exact: bool| Split { w: Some(w), yes, exact };
        let shape = w.shape.clone();
        let loc = match self.locate(w, &shape, path, false) {
            Ok(l) => l,
            Err(_) => return vec![same(false, exact)],
        };
        let wants_list = c.kind == Some(Kind::List);
        let wants_rec = c.kind == Some(Kind::Rec);
        let mut out = vec![];
        if kind_of {
            if loc.st == St::Unknown {
                let key = loc.key.unwrap();
                for kind in [Kind::List, Kind::Rec] {
                    let mut w2 = w.lifted();
                    w2.set(&key, Fact::new(true, Some(Ty::Any), Src::Lift));
                    let v = Ty::Var(key.clone());
                    let _ = if kind == Kind::List { as_list(&mut w2, &v, "").map(|_| ()) } else { as_rec(&mut w2, &v, "").map(|_| ()) };
                    w2.set(&format!("{key}#kind").into(), Fact { kind: Some(kind), ..Fact::new(true, None, Src::Lift) });
                    out.push(own(w2, Some(kind) == c.kind, exact));
                }
                let mut w3 = w.lifted();
                w3.set(&key, Fact::new(false, None, Src::Lift));
                out.push(own(w3, wants_rec, exact));
                return out;
            }
            if loc.st != St::Present {
                return vec![same(wants_rec, exact)];
            }
            let ty = loc.ty.unwrap();
            let ft = rt(w, &ty);
            if ft.is("list") || ft.is("rec") {
                return vec![same(ft.is("list") == wants_list, exact)];
            }
            if let Ty::Var(k) = &ty {
                for kind in [Kind::List, Kind::Rec] {
                    let mut w2 = w.lifted();
                    let _ = if kind == Kind::List { as_list(&mut w2, &ty, "").map(|_| ()) } else { as_rec(&mut w2, &ty, "").map(|_| ()) };
                    w2.set(&format!("{k}#kind").into(), Fact { kind: Some(kind), ..Fact::new(true, None, Src::Lift) });
                    out.push(own(w2, Some(kind) == c.kind, exact));
                }
                return out;
            }
            return vec![same(wants_rec, false)];
        }
        if c.soft && loc.st == St::Unknown {
            let mut w2 = w.clone();
            w2.set(&loc.key.unwrap(), Fact::new(true, Some(Ty::Any), Src::Assume));
            return vec![own(w2, true, false)];
        }
        match loc.st {
            St::Present => {
                if !c.has {
                    return vec![same(false, exact)];
                }
                let ty = loc.ty.unwrap();
                if let Some(lit) = &c.lit {
                    let ft = rt(w, &ty);
                    if !ft.is("any") && ft.tag() != lit_type(lit).tag() && !matches!(lit, Value::Nil) {
                        return vec![same(false, exact)];
                    }
                    if let (Ty::Const(Some(names)), Value::Const(sym), Some(sh)) = (&ft, lit, &loc.shape)
                        && sh.fields.contains_key(&loc.name) {
                            if !names.contains(sym) {
                                return vec![same(false, exact)];
                            }
                            let name = loc.name.clone();
                            let mut w1 = w.clone();
                            let only = konst(vec![sym.clone()]);
                            w1.shape = rebuild_at(&w1, &w1.shape, path, &|sh| sh.set(&name, only.clone(), false));
                            out.push(own(w1, true, exact));
                            let rest: Vec<Name> = names.iter().filter(|n| *n != sym).cloned().collect();
                            if !rest.is_empty() {
                                let mut w2 = w.clone();
                                let others = konst(rest);
                                w2.shape = rebuild_at(&w2, &w2.shape, path, &|sh| sh.set(&name, others.clone(), false));
                                out.push(own(w2, false, true));
                            }
                            return out;
                        }
                    return vec![same(true, false)];
                }
                if c.ty.as_ref().is_some_and(|t| t.is("rec")) {
                    let ft = rt(w, &ty);
                    if !ft.is("rec") && !ft.is("any") {
                        return vec![same(false, exact)];
                    }
                }
                vec![same(true, exact)]
            }
            St::Absent => vec![same(!c.has, exact)],
            St::Opt => {
                let name = loc.name.clone();
                let mut w1 = w.clone();
                w1.shape = rebuild_at(&w1, &w1.shape, path, &|sh| sh.set(&name, sh.fields[&name].ty.clone(), false));
                let mut w2 = w.clone();
                w2.shape = rebuild_at(&w2, &w2.shape, path, &|sh| sh.del(&name));
                vec![own(w1, c.has, exact), own(w2, !c.has, exact)]
            }
            St::Unknown => {
                let key = loc.key.unwrap();
                [true, false]
                    .into_iter()
                    .map(|has| {
                        let mut w2 = w.lifted();
                        w2.set(&key, Fact::new(has, Some(Ty::Any), Src::Lift));
                        if has && c.lit.is_some() { own(w2, true, false) } else { own(w2, has == c.has, exact) }
                    })
                    .collect()
            }
            St::UnknownParent => vec![same(false, exact)],
        }
    }

    fn type_at_path(&self, w: &mut World, base0: &Shape, path0: &[&str]) -> R<Ty> {
        let Some((base, path)) = outer_base(w, base0, path0) else { return Ok(Ty::Any) };
        let mut t = rec(base);
        for seg in path {
            if *seg == "[]" {
                t = as_list(w, &t, "")?;
                continue;
            }
            let sh = as_rec(w, &t, "")?;
            t = match lookup(w, &sh, seg) {
                Look::Present(ty, _) | Look::Opt(ty) => ty,
                Look::Unknown(key) => {
                    w.set(&key, Fact::new(true, Some(Ty::Any), Src::Pre));
                    Ty::Var(key)
                }
                Look::Absent => return Ok(Ty::Any),
            };
        }
        Ok(t)
    }

    /// A callee's output type, with its references to its input resolved against `base`.
    fn inst_type(&self, w: &mut World, base: &Shape, t: &Ty) -> R<Ty> {
        Ok(match t {
            Ty::Var(k) => self.type_at_path(w, base, &k.split('.').collect::<Vec<_>>())?,
            Ty::Rec(sh, _) => match &sh.origin {
                Origin::Input(o) => {
                    let path: Vec<&str> = if o.is_empty() { vec![] } else { o.split('.').collect() };
                    let bt = self.type_at_path(w, base, &path)?;
                    let bs = match rt(w, &bt) {
                        Ty::Rec(s, _) => (*s).clone(),
                        _ => as_rec(w, &bt, "")?,
                    };
                    rec(self.overlay(w, base, &bs, sh)?)
                }
                _ => {
                    let mut f = IndexMap::new();
                    for (k, fd) in &sh.fields {
                        f.insert(k.clone(), Field { ty: self.inst_type(w, base, &fd.ty)?, opt: fd.opt });
                    }
                    rec(Shape::new(f, sh.absent.clone(), sh.origin.clone()))
                }
            },
            Ty::List(of) => list(self.inst_type(w, base, of)?),
            t => t.clone(),
        })
    }

    pub fn overlay(&self, w: &mut World, base: &Shape, bs: &Shape, out: &Shape) -> R<Shape> {
        if out.origin == Origin::Closed {
            let mut f = IndexMap::new();
            for (k, fd) in &out.fields {
                f.insert(k.clone(), Field { ty: self.inst_type(w, base, &fd.ty)?, opt: fd.opt });
            }
            return Ok(Shape::new(f, IndexSet::new(), Origin::Closed));
        }
        let (mut fields, mut absent) = (bs.fields.clone(), bs.absent.clone());
        for k in &out.absent {
            fields.shift_remove(k);
            absent.insert(k.clone());
        }
        for (k, fd) in &out.fields {
            fields.insert(k.clone(), Field { ty: self.inst_type(w, base, &fd.ty)?, opt: fd.opt });
            absent.shift_remove(k);
        }
        Ok(Shape::new(fields, absent, bs.origin.clone()))
    }

    fn shape_at_key(&self, w: &mut World, key: &str) -> R<Option<Shape>> {
        if key.is_empty() {
            return Ok(Some(w.shape.clone()));
        }
        let shape = w.shape.clone();
        let loc = self.locate(w, &shape, &key.split('.').collect::<Vec<_>>(), true)?;
        match loc.st {
            St::Present | St::Opt => Ok(Some(as_rec(w, &loc.ty.unwrap(), "")?)),
            _ => Ok(None),
        }
    }

    fn check_obligations(&self, w: &mut World, c: &Case, name: &str) -> R<()> {
        for o in &c.obl {
            match &o.obl {
                Obl::Only { only, table, role } => {
                    let Some(sh) = self.shape_at_key(w, &o.key)? else { continue };
                    let v = view(w, &sh);
                    let extra: Vec<Name> = v.present.keys().filter(|k| !only.contains(k)).cloned().collect();
                    if !extra.is_empty() {
                        let what = if o.key.is_empty() { "the record".to_string() } else { key_name(&o.key) };
                        return terr(only_msg(self, name, table, *role, &what, &extra));
                    }
                    if let Origin::Input(origin) = &sh.origin {
                        add_only(w, origin, only, table, *role);
                    }
                }
                Obl::Apart(list) => {
                    let a = self.shape_at_key(w, &o.key)?;
                    for x in list {
                        let b = self.shape_at_key(w, &x.other)?;
                        if let (Some(a), Some(b)) = (&a, b) {
                            require_apart(w, a, &b, &format!("{name} ({})", x.what), &x.except)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn check_pre(&self, w: &mut World, c: &Case, name: &str) -> R<()> {
        for p in &c.pre {
            let path: Vec<&str> = p.key.split('.').collect();
            let shape = w.shape.clone();
            let loc = self.locate(w, &shape, &path, true)?;
            let label = key_name(&p.key);
            match loc.st {
                St::Present => {
                    let lt = loc.ty.unwrap();
                    match &p.ty {
                        Ty::Rec(..) => {
                            as_rec(w, &lt, &format!("{name} uses {label} as a record, so it"))?;
                        }
                        Ty::List(_) => {
                            as_list(w, &lt, &format!("{name} uses {label} as a list, so it"))?;
                        }
                        Ty::Any => {}
                        pt => {
                            if constrain(w, &lt, pt, "").is_err() {
                                return terr(format!("{name} needs {label}: {}, but it's {}", fmt_t(None, pt, 0), fmt_t(Some(w), &lt, 0)));
                            }
                        }
                    }
                }
                St::Unknown => {
                    let key = loc.key.unwrap();
                    let is_rec = p.ty.is("rec");
                    let via = p.via.iter().cloned().chain(self.here.clone()).collect();
                    w.set(&key, Fact { via, ..Fact::new(true, Some(if is_rec { Ty::Any } else { p.ty.clone() }), Src::Pre) });
                    if is_rec {
                        as_rec(w, &Ty::Var(key), "")?;
                    }
                }
                st => {
                    let reader = match p.via.first() {
                        Some(v) if &*v.word != name => format!(" ({} reads it)", v.word),
                        _ => String::new(),
                    };
                    let here = fmt_w(Some(w), &w.shape, 0);
                    let msg = if st == St::Opt {
                        format!("{name} needs {label}, but it might be missing — the record here is {here}{reader}")
                    } else {
                        format!("{name} needs {label} — the record here is {here}{reader}")
                    };
                    let mut e = TErr { msg, at: None, top_msg: None };
                    if !p.via.is_empty() {
                        e.at = Some(p.via[1.min(p.via.len() - 1)].clone());
                        let mut chain: Vec<&str> = p.via.iter().map(|v| &*v.word).collect();
                        chain.push(name);
                        let mut seen = IndexSet::new();
                        chain.retain(|x| seen.insert(*x));
                        e.top_msg = Some(format!("{} needs {label}, but nothing on this path provides it ({})", p.via[0].word, chain.join(" ← ")));
                    }
                    return Err(e);
                }
            }
        }
        for k in &c.lacks {
            let shape = w.shape.clone();
            let loc = self.locate(w, &shape, &k.split('.').collect::<Vec<_>>(), true)?;
            match loc.st {
                St::Unknown => w.set(&loc.key.unwrap(), Fact::new(false, None, Src::Pre)),
                St::Present | St::Opt => {
                    let how = if loc.st == St::Opt { "might already be" } else { "is already" };
                    return terr(format!("{name} adds {}, but it {how} there — the record here is {}", key_name(k), fmt_w(Some(w), &w.shape, 0)));
                }
                _ => {}
            }
        }
        self.check_obligations(w, c, name)
    }

    /// A failure that depends on assumptions about the input becomes part of the signature;
    /// once it's certain, it's reported where it happens (`origin`), not where it was discovered.
    pub fn oops(&mut self, w: &World, pos: Option<Pos>, msg: String, origin: Option<At>) -> Vec<World> {
        let here = pos.and_then(|p| self.at(p));
        let at = origin.clone().or(here);
        if w.lifted {
            return vec![w.failed(Fail { msg, at, reported: false })];
        }
        if let Some(o) = origin {
            let m = match &self.word {
                Some(word) => format!("{msg} (when called from {word})"),
                None => msg.clone(),
            };
            self.errors.push((Some(o), m));
        } else if let Some(p) = pos {
            self.err(p, msg.clone());
        }
        vec![w.failed(Fail { msg, at, reported: true })]
    }

    pub fn apply_sig(&mut self, w: World, sig: Option<Rc<Sig>>, name: &str, pos: Pos) -> Vec<World> {
        let Some(sig) = sig else { return vec![] };
        let Sig::Groups(groups) = &*sig else { return vec![w.diverged()] };
        let mut remaining = vec![w];
        let mut out = vec![];
        for g in groups {
            let mut next_rem = vec![];
            for mut r in remaining {
                for part in self.cond_split(&mut r, &g.pattern) {
                    let mut pw = part.w.unwrap_or_else(|| r.clone());
                    if !part.yes {
                        next_rem.push(pw);
                        continue;
                    }
                    // the world goes on to later clauses as the case splits below leave it
                    let rem_at = (g.guard || !part.exact).then(|| {
                        next_rem.push(World::new(Shape::open(Origin::Closed)));
                        next_rem.len() - 1
                    });
                    for c in &g.cases {
                        for q in self.cond_split(&mut pw, &c.cond) {
                            if !q.yes {
                                continue;
                            }
                            let qw = q.w.unwrap_or_else(|| pw.clone());
                            if let Some(fail) = &c.fail {
                                if c.fail_reported {
                                    if let (Some(at), true) = (&c.fail_at, self.report_again) {
                                        self.errors.push((Some(at.clone()), fail.clone()));
                                    }
                                    out.push(qw.failed(Fail { msg: fail.clone(), at: c.fail_at.clone(), reported: true }));
                                } else {
                                    out.extend(self.oops(&qw, Some(pos), fail.clone(), c.fail_at.clone()));
                                }
                                continue;
                            }
                            let mut w = qw.clone();
                            let res = self.check_pre(&mut w, c, name).and_then(|_| {
                                let mut ws = vec![];
                                if c.outs.is_empty() {
                                    ws.push(w.diverged());
                                }
                                for o in &c.outs {
                                    let mut w2 = w.clone();
                                    w2.shape = self.overlay(&mut w2, &w.shape, &w.shape, o)?;
                                    for x in &c.writes {
                                        w2.write(x);
                                    }
                                    ws.push(w2);
                                }
                                Ok(ws)
                            });
                            match res {
                                Ok(ws) => out.extend(ws),
                                Err(e) => {
                                    let top = self.word.is_none();
                                    match (top, e.at, e.top_msg) {
                                        (true, Some(at), Some(m)) => out.extend(self.oops(&qw, Some(pos), m, Some(at))),
                                        _ => out.extend(self.oops(&qw, Some(pos), e.msg, None)),
                                    }
                                }
                            }
                        }
                    }
                    if let Some(i) = rem_at {
                        next_rem[i] = pw;
                    }
                }
            }
            remaining = next_rem;
        }
        for r in remaining {
            let msg = format!("no clause of {name} matches {}", fmt_w(Some(&r), &r.shape, 0));
            out.extend(self.oops(&r, Some(pos), msg, None));
        }
        out
    }
}
