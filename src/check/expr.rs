//! Expression types. Every record-algebra result shape comes from its input shapes.
use super::Cx;
use super::ty::*;
use super::world::*;
use crate::ast::{BinOp, Builtin, Expr, Part};
use crate::value::Name;
use indexmap::{IndexMap, IndexSet};
use std::rc::Rc;

impl Cx<'_> {
    pub fn ty_of(&self, w: &mut World, e: &Expr, s: &Shape) -> R<Ty> {
        Ok(match e {
            Expr::Lit(v) => lit_type(v),
            Expr::Now => Ty::Num,
            Expr::Field(n) => self.need(w, s, n)?,
            Expr::Outer(up, n) => {
                let Some(o) = w.outer.get(up - 1).cloned() else { return terr(format!("^{n} only works inside @field( … )")) };
                self.need_in(w, &o, n, "the enclosing record")?
            }
            Expr::Get(o, k) => {
                let t = self.ty_of(w, o, s)?;
                self.get_key_t(w, &t, k)?
            }
            Expr::Proj(o, names) => {
                let t = self.ty_of(w, o, s)?;
                proj_t(w, &t, names)?
            }
            Expr::List(xs) => {
                let mut t = Ty::Never;
                for x in xs {
                    t = join(&t, &self.ty_of(w, x, s)?);
                }
                list(t)
            }
            Expr::Rec(fs) => {
                let mut f = IndexMap::new();
                for (k, x) in fs {
                    f.insert(k.clone(), Field { ty: self.ty_of(w, x, s)?, opt: false });
                }
                rec(Shape::closed(f))
            }
            Expr::Interp(parts) => {
                for p in parts {
                    if let Part::Expr(x) = p {
                        self.ty_of(w, x, s)?;
                    }
                }
                Ty::Text
            }
            Expr::Neg(a) => {
                let t = self.ty_of(w, a, s)?;
                constrain(w, &t, &Ty::Num, "minus")?;
                Ty::Num
            }
            Expr::Not(a) => {
                self.ty_of(w, a, s)?;
                Ty::Bool
            }
            Expr::And(a, b) | Expr::Or(a, b) => {
                self.ty_of(w, a, s)?;
                self.ty_of(w, b, s)?;
                Ty::Bool
            }
            Expr::Call(f, args) => {
                use Builtin::*;
                let list_arg = matches!(f, Sum | Head | Last | Tail | Reverse | Push) || (matches!(f, Min | Max) && args.len() == 1);
                let mut ts = vec![];
                for (i, a) in args.iter().enumerate() {
                    ts.push(if i == 0 && list_arg { self.ty_of_listy(w, a, s)? } else { self.ty_of(w, a, s)? });
                }
                fn_t(w, *f, &ts)?
            }
            Expr::Bin(op, a, b) => {
                let (a, b) = (self.ty_of(w, a, s)?, self.ty_of(w, b, s)?);
                bin_t(w, *op, &a, &b)?
            }
        })
    }

    /// In `sum(xs.amount)` an unknown `xs` is a list whose items have `amount`.
    fn ty_of_listy(&self, w: &mut World, e: &Expr, s: &Shape) -> R<Ty> {
        if let Expr::Get(o, k) = e {
            let base = self.ty_of(w, o, s)?;
            if base.is("var") && rt(w, &base).is("any") {
                let el = as_list(w, &base, "")?;
                return Ok(list(self.get_key_t(w, &el, k)?));
            }
            return self.get_key_t(w, &base, k);
        }
        self.ty_of(w, e, s)
    }

    pub fn get_key_t(&self, w: &mut World, t: &Ty, key: &str) -> R<Ty> {
        let r = rt(w, t);
        match &r {
            Ty::Never => Ok(Ty::Never),
            Ty::List(of) => Ok(list(self.get_key_t(w, of, key)?)),
            _ => {
                let sh = as_rec(w, if t.is("var") { t } else { &r }, &format!(".{key}"))?;
                self.need(w, &sh, key)
            }
        }
    }
}

fn bin_t(w: &mut World, op: BinOp, a: &Ty, b: &Ty) -> R<Ty> {
    use BinOp::*;
    Ok(match op {
        Eq | Ne | Within => Ty::Bool,
        Over => {
            let (x, y) = (as_rec(w, a, "⫽")?, as_rec(w, b, "⫽")?);
            rec(merge_shapes(w, &x, &y))
        }
        Join => {
            let (x, y) = (as_rec(w, a, "⊔")?, as_rec(w, b, "⊔")?);
            if let Err(e) = require_apart(w, &x, &y, "a ⊔ b", &[]) {
                return terr(format!("{}, so ⊔ fails if they disagree — use the ⊔{{…}} step, which adds conflict for a clause to handle", e.msg));
            }
            rec(merge_shapes(w, &x, &y))
        }
        Meet => {
            let (x, y) = (as_rec(w, a, "⊓")?, as_rec(w, b, "⊓")?);
            meet_shape_t(w, &x, &y)
        }
        Diff => {
            let (x, y) = (as_rec(w, a, "∆")?, as_rec(w, b, "∆")?);
            delta_t(w, &x, &y)
        }
        Apply => {
            let x = as_rec(w, a, "⍎")?;
            apply_t(w, &x, &rt(w, b))
        }
        Cat => match (rt(w, a), rt(w, b)) {
            (Ty::List(x), Ty::List(y)) => list(join(&x, &y)),
            _ => Ty::Text,
        },
        Lt | Gt | Le | Ge => {
            let what = op.glyph();
            if rt(w, a).is("num") || rt(w, b).is("num") {
                constrain(w, a, &Ty::Num, what)?;
                constrain(w, b, &Ty::Num, what)?;
            } else if rt(w, a).is("text") || rt(w, b).is("text") {
                constrain(w, a, &Ty::Text, what)?;
                constrain(w, b, &Ty::Text, what)?;
            } else {
                let (ra, rb) = (rt(w, a), rt(w, b));
                if !ra.is("any") || !rb.is("any") {
                    return terr(format!("can't compare {} with {}", fmt_t(Some(w), &ra, 0), fmt_t(Some(w), &rb, 0)));
                }
            }
            Ty::Bool
        }
        Add | Sub | Mul | Div | Rem => {
            let (ra, rb) = (rt(w, a), rt(w, b));
            if op == Add && (ra.is("text") || rb.is("text")) {
                return terr(format!("can't use + on {} and {} — use ++ to join text", fmt_t(Some(w), &ra, 0), fmt_t(Some(w), &rb, 0)));
            }
            constrain(w, a, &Ty::Num, op.glyph())?;
            constrain(w, b, &Ty::Num, op.glyph())?;
            Ty::Num
        }
    })
}

fn fn_t(w: &mut World, f: Builtin, args: &[Ty]) -> R<Ty> {
    use Builtin::*;
    let a0 = args.first().cloned().unwrap_or(Ty::Any);
    let what = format!("{}()", f.name());
    let num = |w: &mut World, t: &Ty| constrain(w, t, &Ty::Num, &what).map(|_| ());
    Ok(match f {
        Len => {
            let r = rt(w, &a0);
            if !["list", "text", "rec", "any"].contains(&r.tag()) {
                return terr(format!("len() needs a list, text or record, got {}", fmt_t(Some(w), &r, 0)));
            }
            Ty::Num
        }
        Sum => {
            let el = as_list(w, &a0, "sum()")?;
            num(w, &el)?;
            Ty::Num
        }
        Min | Max => {
            if args.len() == 1 {
                let el = as_list(w, &a0, &what)?;
                num(w, &el)?;
            } else {
                for a in args {
                    num(w, a)?;
                }
            }
            Ty::Num
        }
        Abs | Floor | Ceil | Round => {
            for a in args {
                num(w, a)?;
            }
            Ty::Num
        }
        Head | Last => as_list(w, &a0, &what)?,
        Tail | Reverse => {
            as_list(w, &a0, &what)?;
            a0
        }
        Push => {
            let el = as_list(w, &a0, "push()")?;
            list(join(&el, args.get(1).unwrap_or(&Ty::Any)))
        }
        Range => {
            for a in args {
                num(w, a)?;
            }
            list(Ty::Num)
        }
        Str => Ty::Text,
        Keys => {
            as_rec(w, &a0, "keys()")?;
            list(Ty::Const(None))
        }
    })
}

// ---- record algebra ----
fn absent_in(s: &Shape, v: &View, k: &str) -> bool {
    v.gone.contains(k) || (s.origin == Origin::Closed && !v.present.contains_key(k))
}

pub fn merge_shapes(w: &World, a: &Shape, b: &Shape) -> Shape {
    let (va, vb) = (view(w, a), view(w, b));
    let mut f = va.present.clone();
    for (k, fd) in &vb.present {
        let v = match f.get(k) {
            Some(old) if fd.opt => Field { ty: join(&rt(w, &old.ty), &rt(w, &fd.ty)), opt: old.opt },
            _ => fd.clone(),
        };
        f.insert(k.clone(), v);
    }
    let absent: IndexSet<Name> = va.gone.iter().chain(vb.gone.iter()).filter(|k| !f.contains_key(*k) && absent_in(a, &va, k) && absent_in(b, &vb, k)).cloned().collect();
    let origin = if a.origin != Origin::Closed { a.origin.clone() } else { b.origin.clone() };
    Shape::new(f, absent, origin)
}

fn meet_shape_t(w: &World, a: &Shape, b: &Shape) -> Ty {
    let (va, vb) = (view(w, a), view(w, b));
    let mut f = IndexMap::new();
    for (k, x) in &va.present {
        let Some(y) = vb.present.get(k) else { continue };
        let ty = rec_of(vec![("agree", join(&rt(w, &x.ty), &rt(w, &y.ty)), true), ("differ", rec_of(vec![("left", x.ty.clone(), false), ("right", y.ty.clone(), false)]), true)]);
        f.insert(k.clone(), Field { ty, opt: x.opt || y.opt });
    }
    rec(Shape::closed(f))
}

/// Δ(A → B): fields only in B are add, only in A are del, shared ones same or set.
fn delta_t(w: &World, a: &Shape, b: &Shape) -> Ty {
    let (va, vb) = (view(w, a), view(w, b));
    let mut f = IndexMap::new();
    let keys: IndexSet<&Name> = va.present.keys().chain(vb.present.keys()).collect();
    for k in keys {
        let (x, y) = (va.present.get(k), vb.present.get(k));
        let (a_sure, b_sure) = (x.is_some_and(|x| !x.opt), y.is_some_and(|y| !y.opt));
        let ta = x.map_or(Ty::Any, |x| x.ty.clone());
        let tb = y.map_or(Ty::Any, |y| y.ty.clone());
        let e = if a_sure && y.is_none() && absent_in(b, &vb, k) {
            rec_of(vec![("del", ta, false)])
        } else if b_sure && x.is_none() && absent_in(a, &va, k) {
            rec_of(vec![("add", tb, false)])
        } else if a_sure && b_sure {
            rec_of(vec![("same", ta.clone(), true), ("set", tb, true), ("was", ta, true)])
        } else {
            rec_of(vec![("same", Ty::Any, true), ("set", tb.clone(), true), ("was", ta.clone(), true), ("add", tb, true), ("del", ta, true)])
        };
        f.insert(k.clone(), Field { ty: e, opt: !(a_sure || b_sure) });
    }
    let origin = if a.origin == Origin::Closed && b.origin == Origin::Closed { Origin::Closed } else { Origin::Unknown };
    Ty::Rec(Rc::new(Shape::new(f, IndexSet::new(), origin)), Some(Rc::new((a.clone(), b.clone()))))
}

fn apply_t(w: &World, x: &Shape, d: &Ty) -> Ty {
    let Ty::Rec(_, Some(delta)) = d else { return rec(Shape::open(Origin::Unknown)) };
    let (a, b) = &**delta;
    let (va, vb) = (view(w, a), view(w, b));
    let mut s = x.clone();
    for (k, fa) in &va.present {
        if !fa.opt && !vb.present.contains_key(k) && absent_in(b, &vb, k) {
            s = s.del(k);
        }
    }
    for (k, fb) in &vb.present {
        s = s.set(k, fb.ty.clone(), fb.opt);
    }
    rec(s)
}

/// What `.update` receives from a change set: optional new values, nil for removed columns.
pub fn delta_changes_shape(w: &World, delta: &(Shape, Shape)) -> Shape {
    let (a, b) = delta;
    let (va, vb) = (view(w, a), view(w, b));
    let mut f = IndexMap::new();
    // id is left out: an unchanged id is dropped at run time, and a changed one is refused there
    for (k, fb) in &vb.present {
        if &**k != "id" {
            f.insert(k.clone(), Field { ty: fb.ty.clone(), opt: true });
        }
    }
    for k in va.present.keys() {
        if !vb.present.contains_key(k) && absent_in(b, &vb, k) {
            f.insert(k.clone(), Field { ty: Ty::Nil, opt: true });
        }
    }
    Shape::closed(f)
}

fn proj_t(w: &mut World, t: &Ty, names: &[Name]) -> R<Ty> {
    let one = |w: &World, sh: &Shape| {
        let mut f = IndexMap::new();
        for n in names {
            let (ty, opt) = match lookup(w, sh, n) {
                Look::Absent => continue,
                Look::Unknown(_) => (Ty::Any, true),
                Look::Present(t, _) => (t, false),
                Look::Opt(t) => (t, true),
            };
            f.insert(n.clone(), Field { ty, opt });
        }
        rec(Shape::closed(f))
    };
    if let Ty::List(of) = rt(w, t) {
        return Ok(list(match rt(w, &of) {
            Ty::Rec(s, _) => one(w, &s),
            Ty::Never => Ty::Never,
            _ => rec(Shape::open(Origin::Unknown)),
        }));
    }
    let what = format!(".{{{}}}", names.iter().map(|n| &**n).collect::<Vec<_>>().join(" "));
    let sh = as_rec(w, t, &what)?;
    Ok(one(w, &sh))
}
