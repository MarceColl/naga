//! Signatures. A word's signature is a list of groups, one per clause; each group has the
//! clause's dispatch pattern and its cases: conditions callees dispatch on, preconditions
//! (must hold or it's an error), outputs, the fields written, or a failure.
use super::ty::*;
use super::world::*;
use crate::num::cmp16;
use crate::value::{Name, Value};
use indexmap::IndexMap;

#[derive(Clone, Debug)]
pub struct CondItem {
    pub key: Key,
    pub has: bool,
    pub lit: Option<Value>,
    pub soft: bool,
    pub kind: Option<Kind>,
    pub ty: Option<Ty>,
}

#[derive(Clone, Debug)]
pub struct Pre {
    pub key: Key,
    pub ty: Ty,
    pub via: Vec<At>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OblItem {
    pub key: Key,
    pub obl: Obl,
}

impl OblItem {
    fn kind(&self) -> &'static str {
        if matches!(self.obl, Obl::Only { .. }) { "only" } else { "apart" }
    }
}

#[derive(Clone, Debug)]
pub struct Case {
    pub cond: Vec<CondItem>,
    pub pre: Vec<Pre>,
    pub lacks: Vec<Key>,
    pub obl: Vec<OblItem>,
    pub outs: Vec<Shape>,
    pub out_text: Vec<String>,
    pub writes: Vec<Name>,
    pub fail: Option<String>,
    pub fail_reported: bool,
    pub fail_at: Option<At>,
    pub pre_text: Vec<(Key, String)>,
    pub in_text: String,
    pub id: String,
}

#[derive(Clone, Debug)]
pub struct Group {
    pub guard: bool,
    pub pattern: Vec<CondItem>,
    pub cases: Vec<Case>,
}

#[derive(Debug)]
pub enum Sig {
    /// Not known yet (a recursive group before its fixpoint).
    Bottom,
    Groups(Vec<Group>),
}

struct Facts {
    cond: Vec<(Key, Src, Fact)>,
    pre: Vec<(Key, Ty)>,
    lacks: Vec<Key>,
    obl: Vec<OblItem>,
}

fn depth(k: &str) -> usize {
    k.split('.').count()
}

fn facts_of(w: &World) -> Facts {
    let mut fx = Facts { cond: vec![], pre: vec![], lacks: vec![], obl: vec![] };
    for (key, f) in &w.facts {
        if f.src == Src::Assume {
            continue;
        }
        if let Some(o) = &f.obl {
            fx.obl.push(OblItem { key: key[..key.rfind('#').unwrap()].into(), obl: o.clone() });
            continue;
        }
        match f.src {
            Src::Cond | Src::Lift => fx.cond.push((key.clone(), f.src, f.clone())),
            _ if f.has => fx.pre.push((key.clone(), f.ty.clone().unwrap_or(Ty::Any))),
            _ => fx.lacks.push(key.clone()),
        }
    }
    let by_depth = |a: &str, b: &str| depth(a).cmp(&depth(b)).then(cmp16(a, b));
    fx.cond.sort_by(|a, b| by_depth(&a.0, &b.0));
    fx.pre.sort_by(|a, b| by_depth(&a.0, &b.0));
    sort16(&mut fx.lacks);
    fx.obl.sort_by(|a, b| cmp16(&format!("{}{}", a.key, a.kind()), &format!("{}{}", b.key, b.kind())));
    fx
}

/// A precondition's type, without references to this word's input.
fn plain_type(w: &World, t: &Ty, depth: usize) -> Ty {
    let r = rt(w, t);
    if depth > 4 {
        return Ty::Any;
    }
    match r {
        Ty::Rec(..) => rec(Shape::open(Origin::Unknown)),
        Ty::List(of) => list(plain_type(w, &of, depth + 1)),
        Ty::Any | Ty::Var(_) => Ty::Any,
        r => r,
    }
}

fn cond_item(w: &World, key: &Key, f: &Fact) -> CondItem {
    CondItem {
        key: key.clone(),
        has: f.has,
        lit: f.lit.clone(),
        soft: f.soft,
        kind: f.kind,
        ty: if f.has { f.ty.as_ref().map(|t| plain_type(w, t, 0)) } else { None },
    }
}

pub fn pattern_of(w: &World) -> Vec<CondItem> {
    facts_of(w).cond.iter().filter(|(_, s, _)| *s == Src::Cond).map(|(k, _, f)| cond_item(w, k, f)).collect()
}

pub fn case_of(w: &World, group: &Group) -> Case {
    let fx = facts_of(w);
    let ends = w.fail.is_some() || w.diverge;
    let mut writes: Vec<Name> = w.writes.iter().cloned().collect();
    sort16(&mut writes);
    let mut c = Case {
        cond: fx.cond.iter().filter(|(_, s, _)| *s == Src::Lift).map(|(k, _, f)| cond_item(w, k, f)).collect(),
        pre: fx.pre.iter().map(|(k, t)| Pre { key: k.clone(), ty: plain_type(w, t, 0), via: w.facts[k].via.clone() }).collect(),
        lacks: fx.lacks,
        obl: fx.obl,
        outs: if ends { vec![] } else { vec![w.shape.clone()] },
        out_text: if ends { vec![] } else { vec![fmt_out(w, &w.shape)] },
        writes,
        fail: w.fail.as_ref().map(|f| f.msg.clone()),
        fail_reported: w.fail.as_ref().is_some_and(|f| f.reported),
        fail_at: w.fail.as_ref().and_then(|f| f.at.clone()),
        pre_text: fx.pre.iter().map(|(k, t)| (k.clone(), fmt_t(Some(w), t, 0))).collect(),
        in_text: String::new(),
        id: String::new(),
    };
    finish_case(&mut c, group);
    c
}

fn cond_sig(cs: &[CondItem]) -> String {
    let items: Vec<String> = cs
        .iter()
        .map(|x| format!("{:?}", (&x.key, x.has, x.lit.as_ref().map(Value::show), x.soft, x.kind, x.ty.as_ref().map(type_sig))))
        .collect();
    items.join(";")
}

fn pre_sig(c: &Case) -> String {
    format!("{:?}", c.pre.iter().map(|p| (&p.key, type_sig(&p.ty))).collect::<Vec<_>>())
}

fn finish_case(c: &mut Case, group: &Group) {
    let cond: Vec<CondItem> = group.pattern.iter().chain(c.cond.iter()).cloned().collect();
    c.in_text = fmt_in(&cond, &c.pre_text, &c.lacks, &c.obl);
    c.id = format!("{}#{}#{:?}#{:?}#{:?}", cond_sig(&c.cond), pre_sig(c), c.lacks, c.fail, c.obl);
}

/// Input requirements as a nested record: `{..r, card: {..c, balance: Num}, ok, ¬paidAt}`.
fn fmt_in(cond: &[CondItem], pre_text: &[(Key, String)], lacks: &[Key], obl: &[OblItem]) -> String {
    #[derive(Default)]
    struct Node {
        kids: IndexMap<String, Node>,
        present: bool,
        absent: bool,
        lit: Option<String>,
        ty: Option<String>,
        kind: Option<Kind>,
        only: Option<Name>,
    }
    fn node<'a>(root: &'a mut Node, key: &str) -> &'a mut Node {
        key.split('.').fold(root, |n, seg| n.kids.entry(seg.to_string()).or_default())
    }
    fn render(n: &Node, name: &str) -> String {
        let mut parts = vec![format!("{name}{}", n.only.as_ref().map(|t| format!(" ⊆ {t}")).unwrap_or_default())];
        for (k, ch) in &n.kids {
            if ch.absent && !ch.present {
                parts.push(format!("¬{k}"));
                continue;
            }
            if k == "[]" {
                continue;
            }
            match ch.kids.get("[]") {
                Some(elem) if !elem.kids.is_empty() => parts.push(format!("{k}: [{}]", render(elem, ".."))),
                _ if !ch.kids.is_empty() || ch.only.is_some() => parts.push(format!("{k}: {}", render(ch, &format!("..{}", k.chars().next().unwrap())))),
                _ => {
                    let suffix = match (&ch.lit, &ch.ty, ch.kind) {
                        (Some(l), _, _) => format!(": {l}"),
                        (None, Some(t), _) => format!(": {t}"),
                        (None, None, Some(Kind::List)) => ": […]".into(),
                        _ => String::new(),
                    };
                    parts.push(format!("{k}{suffix}"));
                }
            }
        }
        format!("{{{}}}", parts.join(", "))
    }
    let mut root = Node::default();
    for x in cond {
        if let Some(base) = x.key.strip_suffix("#kind") {
            node(&mut root, base).kind = x.kind;
            continue;
        }
        let n = node(&mut root, &x.key);
        if !x.has {
            n.absent = true;
        } else {
            n.present = true;
            if let Some(l) = &x.lit {
                n.lit = Some(l.show());
            }
        }
    }
    for (k, ty) in pre_text {
        let n = node(&mut root, k);
        n.present = true;
        if ty != "Any" && !ty.starts_with('{') {
            n.ty = Some(ty.clone());
        }
    }
    for k in lacks {
        node(&mut root, k).absent = true;
    }
    for o in obl {
        if let Obl::Only { table, .. } = &o.obl {
            let n = if o.key.is_empty() { &mut root } else { node(&mut root, &o.key) };
            n.only = Some(table.clone());
        }
    }
    let outer: Vec<String> = root.kids.keys().filter(|k| k.len() > 1 && k.starts_with('^') && k[1..].bytes().all(|b| b.is_ascii_digit())).cloned().collect();
    let outer_text: Vec<String> = outer
        .iter()
        .map(|k| {
            let n = root.kids.shift_remove(k).unwrap();
            format!("{}{}", "^".repeat(k[1..].parse().unwrap()), render(&n, ".."))
        })
        .collect();
    let mut s = render(&root, "..r");
    if !outer_text.is_empty() {
        s.push_str(" inside ");
        s.push_str(&outer_text.join(" inside "));
    }
    s
}

fn fmt_out(w: &World, s: &Shape) -> String {
    let facts: IndexMap<Key, Fact> = w.facts.iter().filter(|(_, f)| matches!(f.src, Src::Pre | Src::Cond)).map(|(k, f)| (k.clone(), f.clone())).collect();
    let v = shape_view(Some(&facts), s);
    let mut parts = vec![];
    match &s.origin {
        Origin::Input(o) if o.is_empty() => parts.push("..r".to_string()),
        Origin::Closed => {}
        _ => parts.push("..".into()),
    }
    for (k, f) in &v.present {
        let ty = fmt_t(Some(w), &f.ty, 1);
        parts.push(format!("{k}{}{}", if f.opt { "?" } else { "" }, if ty == "Any" { String::new() } else { format!(": {ty}") }));
    }
    format!("{{{}}}", parts.join(", "))
}

fn outs_sig(c: &Case) -> String {
    let mut o: Vec<String> = c.outs.iter().map(shape_sig).collect();
    sort16(&mut o);
    let mut w: Vec<&str> = c.writes.iter().map(|w| &**w).collect();
    sort16(&mut w);
    format!("{}#{}", o.join("|"), w.join(","))
}

/// Two cases that differ only in one condition, taken both ways, are one case.
pub fn simplify(mut cases: Vec<Case>, group: &Group) -> Vec<Case> {
    let mut changed = true;
    while changed {
        changed = false;
        'outer: for i in 0..cases.len() {
            for j in i + 1..cases.len() {
                let (a, b) = (&cases[i], &cases[j]);
                if a.fail != b.fail || a.cond.len() != b.cond.len() || pre_sig(a) != pre_sig(b) || a.lacks != b.lacks || outs_sig(a) != outs_sig(b) || a.obl != b.obl {
                    continue;
                }
                let diff: Vec<&CondItem> = a.cond.iter().filter(|x| !b.cond.iter().any(|y| y.key == x.key && y.has == x.has && y.kind == x.kind)).collect();
                if diff.len() != 1 {
                    continue;
                }
                let k = diff[0].key.clone();
                let Some(other) = b.cond.iter().find(|y| y.key == k) else { continue };
                if other.has == diff[0].has && other.kind == diff[0].kind {
                    continue;
                }
                let (dot, hash) = (format!("{k}."), format!("{k}#"));
                if a.cond.iter().any(|x| x.key != k && (x.key.starts_with(&dot) || x.key.starts_with(&hash))) {
                    continue;
                }
                let a = &mut cases[i];
                a.cond.retain(|x| x.key != k);
                finish_case(a, group);
                cases.remove(j);
                changed = true;
                break 'outer;
            }
        }
    }
    cases
}

/// Cases with the same inputs merge their outputs.
pub fn group_cases(cases: Vec<Case>) -> Vec<Case> {
    let mut out: Vec<Case> = vec![];
    for c in cases {
        let Some(g) = out.iter_mut().find(|x| x.id == c.id) else {
            out.push(c);
            continue;
        };
        for (o, t) in c.outs.iter().zip(c.out_text.iter()) {
            if !g.outs.iter().any(|x| shape_sig(x) == shape_sig(o)) {
                g.outs.push(o.clone());
                g.out_text.push(t.clone());
            }
        }
        for w in c.writes {
            if !g.writes.contains(&w) {
                g.writes.push(w);
            }
        }
    }
    out
}

/// What callers can observe of a signature; equal keys mean callers need no recheck.
pub fn sig_key(sig: Option<&Sig>) -> String {
    match sig {
        None => "undefined".into(),
        Some(Sig::Bottom) => "⊥".into(),
        Some(Sig::Groups(gs)) => gs
            .iter()
            .map(|g| {
                let cases: Vec<String> = g
                    .cases
                    .iter()
                    .map(|c| {
                        let mut o: Vec<String> = c.outs.iter().map(shape_sig).collect();
                        sort16(&mut o);
                        let mut w: Vec<&str> = c.writes.iter().map(|w| &**w).collect();
                        sort16(&mut w);
                        format!("{}@{}@{}", c.id, o.join("|"), w.join(","))
                    })
                    .collect();
                format!("{}/{}/{}", g.guard, cond_sig(&g.pattern), cases.join("&"))
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}
