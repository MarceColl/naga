//! Analysing a word's clauses: every statement maps each world to the worlds after it.
use super::expr::delta_changes_shape;
use super::sig::*;
use super::ty::*;
use super::world::*;
use super::Cx;
use crate::ast::*;
use crate::num::js;
use crate::value::Name;
use indexmap::{IndexMap, IndexSet};

/// Same shape, facts, failure and writes: one world.
fn widen(ws: Vec<World>) -> R<Vec<World>> {
    let mut seen: IndexMap<String, World> = IndexMap::new();
    for w in ws {
        let facts: Vec<String> =
            w.facts.iter().map(|(k, f)| format!("{k}\u{1}{}\u{1}{}\u{1}{}", f.has, f.src.name(), f.ty.as_ref().map(type_sig).unwrap_or_else(|| "null".into()))).collect();
        let mut writes: Vec<&Name> = w.writes.iter().collect();
        sort16(&mut writes);
        let key = format!(
            "{}#{}#{}#{}#{}",
            shape_sig(&w.shape),
            facts.join("\u{2}"),
            w.fail.as_ref().map_or("", |f| &f.msg),
            if w.diverge { "D" } else { "" },
            writes.iter().map(|w| &***w).collect::<Vec<_>>().join(",")
        );
        seen.entry(key).or_insert(w);
    }
    if seen.len() > 64 {
        return terr("too many cases to follow here — split this word up");
    }
    Ok(seen.into_values().collect())
}

fn row_shape(t: &Table) -> Shape {
    let mut m = IndexMap::new();
    m.insert("id".into(), Field { ty: Ty::Num, opt: false });
    for c in &t.cols {
        m.insert(c.name.clone(), Field { ty: col_type(c.ty), opt: c.opt });
    }
    Shape::closed(m)
}

fn check_row_shape(w: &mut World, t: &Table, sh: &Shape, partial: bool) -> R<()> {
    if let Origin::Input(o) = &sh.origin {
        let allowed: Vec<Name> = (!partial).then(|| "id".into()).into_iter().chain(t.cols.iter().map(|c| c.name.clone())).collect();
        add_only(w, o, &allowed, &t.name, if partial { Role::Changes } else { Role::Row });
    }
    let v = view(w, sh);
    for (k, f) in &v.present {
        if &**k == "id" {
            if partial {
                return terr("changes can’t include id");
            }
            continue;
        }
        let Some(c) = t.col(k) else { return terr(format!("{} has no column {k}", t.name)) };
        if rt(w, &f.ty).is("nil") {
            if !c.opt {
                return terr(format!("{}.{k} is required, so it can't be nil", t.name));
            }
            continue;
        }
        let want = col_type(c.ty);
        if !want.is("any") {
            constrain(w, &f.ty, &want, &format!("{}.{k} is {}, so {k}", t.name, c.ty.name()))?;
        }
    }
    if !partial && sh.origin == Origin::Closed {
        for c in t.cols.iter().filter(|c| !c.opt) {
            match v.present.get(&c.name) {
                Some(f) if !f.opt => {}
                f => return terr(format!("{} {} {} — {}.{} is required", t.single, if f.is_some() { "might be missing" } else { "needs" }, c.name, t.name, c.name)),
            }
        }
    }
    Ok(())
}

fn pattern_facts(w: &mut World, entries: &[PatEntry], origin: &str) {
    for e in entries {
        let key = child_key(origin, &e.name);
        if e.mode == Mode::Absent {
            w.set(&key, Fact::new(false, None, Src::Cond));
            continue;
        }
        let mut f = Fact::new(true, Some(Ty::Any), Src::Cond);
        if let Some(l) = &e.lit {
            f.lit = Some(l.clone());
            f.ty = Some(lit_type(l));
        }
        if let Some(sub) = &e.sub {
            w.set(&key, Fact { ty: Some(rec(Shape::open(Origin::Input(key.clone())))), ..f });
            pattern_facts(w, sub, &key);
        } else {
            w.set(&key, f);
        }
    }
}

impl Cx<'_> {
    pub fn run_body(&mut self, worlds: Vec<World>, stmts: &[Stmt]) -> R<Vec<World>> {
        let mut cur = worlds;
        for st in stmts {
            if !cur.iter().any(World::live) {
                break;
            }
            let mut next = vec![];
            for w in cur {
                if !w.live() {
                    next.push(w);
                    continue;
                }
                self.here = self.at(st.pos);
                next.extend(self.run_stmt(w, st));
            }
            cur = widen(next)?;
        }
        Ok(cur)
    }

    fn run_stmt(&mut self, w0: World, st: &Stmt) -> Vec<World> {
        if let StmtKind::Call(name) = &st.kind {
            let sig = sig_of(self, name);
            return self.apply_sig(w0, sig, name, st.pos);
        }
        let mut w = w0.clone();
        match self.stmt(&mut w, &w0, st) {
            Ok(ws) => ws,
            Err(e) => self.oops(&w0, Some(st.pos), e.msg, None),
        }
    }

    fn values(&self, w: &mut World, fs: &[FieldInit], s: &Shape) -> R<Vec<Ty>> {
        fs.iter().map(|f| f.expr.as_ref().map_or(Ok(Ty::Bool), |e| self.ty_of(w, e, s))).collect()
    }

    fn stmt(&mut self, w: &mut World, w0: &World, st: &Stmt) -> R<Vec<World>> {
        match &st.kind {
            StmtKind::Add(fs) => {
                let vals = self.values(w, fs, &w0.shape)?;
                let mut s = w.shape.clone();
                for (f, t) in fs.iter().zip(vals) {
                    need_absent(w, &s, &f.name)?;
                    s = s.set(&f.name, t, false);
                    w.write(&f.name);
                }
                w.shape = s;
            }
            StmtKind::Del(names) => {
                let mut s = w.shape.clone();
                for n in names {
                    self.need(w, &s, n)?;
                    s = s.del(n);
                    w.write(n);
                }
                w.shape = s;
            }
            StmtKind::Pick(names) => {
                let mut f = IndexMap::new();
                for n in names {
                    f.insert(n.clone(), Field { ty: self.need_here(w, n)?, opt: false });
                }
                w.shape = Shape::closed(f);
                w.write(&"*".into());
            }
            StmtKind::Set(name, e) => {
                self.need_here(w, name)?;
                let t = self.ty_of(w, e, &w0.shape)?;
                w.shape = w.shape.set(name, t, false);
                w.write(name);
            }
            StmtKind::Inc(name, n) => {
                let t = self.need_here(w, name)?;
                constrain(w, &t, &Ty::Num, &format!("{name}{}{}", if *n < 0.0 { "" } else { "+" }, js(*n)))?;
                w.write(name);
            }
            StmtKind::Rename(from, to) => {
                let t = self.need_here(w, from)?;
                let s = w.shape.clone();
                need_absent(w, &s, to)?;
                w.shape = w.shape.del(from).set(to, t, false);
                w.write(from);
                w.write(to);
            }
            StmtKind::Wrap(field, names) => {
                let s0 = w.shape.clone();
                need_absent(w, &s0, field)?;
                let mut f = IndexMap::new();
                let mut s = w.shape.clone();
                for n in names {
                    f.insert(n.clone(), Field { ty: self.need(w, &s, n)?, opt: false });
                    s = s.del(n);
                    w.write(n);
                }
                w.shape = s.set(field, rec(Shape::closed(f)), false);
                w.write(field);
            }
            StmtKind::Unwrap(field) => {
                let t = self.need_here(w, field)?;
                let inner = as_rec(w, &t, &format!("⊃{field}"))?;
                let v = view(w, &inner);
                let mut s = w.shape.del(field);
                w.write(field);
                require_apart(w, &s, &inner, &format!("⊃{field}"), std::slice::from_ref(field))?;
                for (k, fd) in &v.present {
                    s = s.set(k, fd.ty.clone(), fd.opt);
                    w.write(k);
                }
                // a closed record that takes in an open one now has the same unknowns
                if s.origin == Origin::Closed && inner.origin != Origin::Closed {
                    s.origin = inner.origin.clone();
                }
                w.shape = s;
            }
            StmtKind::Over(fs) => {
                let vals = self.values(w, fs, &w0.shape)?;
                let mut s = w.shape.clone();
                for (f, t) in fs.iter().zip(vals) {
                    s = s.set(&f.name, t, false);
                    w.write(&f.name);
                }
                w.shape = s;
            }
            StmtKind::Unify(fs) => {
                // ⊔{…} either merges or, when a present field disagrees, adds conflict for the next clause
                let vals = self.values(w, fs, &w0.shape)?;
                let s0 = w.shape.clone();
                need_absent(w, &s0, "conflict")?;
                let mut s = w.shape.clone();
                let mut cf = IndexMap::new();
                for (f, t) in fs.iter().zip(vals) {
                    let l = lookup(w, &w.shape, &f.name);
                    let lt = match &l {
                        Look::Present(t, _) | Look::Opt(t) => Some(t.clone()),
                        _ => None,
                    };
                    if !matches!(l, Look::Absent) {
                        cf.insert(f.name.clone(), Field { ty: rec_of(vec![("left", lt.clone().unwrap_or(Ty::Any), false), ("right", t.clone(), false)]), opt: true });
                    }
                    s = s.set(&f.name, if matches!(l, Look::Present(..)) { lt.unwrap() } else { t }, false);
                    w.write(&f.name);
                }
                w.write(&"conflict".into());
                if cf.is_empty() {
                    w.shape = s;
                } else {
                    let with_conflict = w.shape.set(&"conflict".into(), rec(Shape::closed(cf)), false);
                    return Ok(vec![w.with_shape(s), w.with_shape(with_conflict)]);
                }
            }
            StmtKind::Say(e) => {
                let s = w.shape.clone();
                self.ty_of(w, e, &s)?;
            }
            StmtKind::Raise(e) => {
                let s = w.shape.clone();
                self.ty_of(w, e, &s)?;
                w.diverge = true;
            }
            StmtKind::Show | StmtKind::Break => {}
            StmtKind::Hole(_) => w.diverge = true,
            StmtKind::Focus(field, body) => return self.focus(w, field, body, st.pos),
            StmtKind::Par(branches) => return self.par(w, branches),
            StmtKind::Table { table, op, compiled, .. } => return self.table(w, table, op, compiled.as_ref()),
            StmtKind::Call(_) => unreachable!(),
        }
        Ok(vec![w.clone()])
    }

    fn focus(&mut self, w0: &mut World, field: &Name, body: &[Stmt], pos: Pos) -> R<Vec<World>> {
        let t0 = self.need_here(w0, field)?;
        if let (Ty::Var(key), true) = (&t0, rt(w0, &t0).is("any")) {
            let mut out = vec![];
            for kind in [Kind::List, Kind::Rec] {
                let mut w = w0.lifted();
                let _ = if kind == Kind::List { as_list(&mut w, &t0, "").map(|_| ()) } else { as_rec(&mut w, &t0, "").map(|_| ()) };
                w.set(&format!("{key}#kind").into(), Fact { kind: Some(kind), ..Fact::new(true, None, Src::Lift) });
                match self.focus_known(&mut w, field, body) {
                    Ok(ws) => out.extend(ws),
                    Err(e) => out.extend(self.oops(&w, Some(pos), e.msg, None)),
                }
            }
            return Ok(out);
        }
        self.focus_known(w0, field, body)
    }

    fn focus_known(&mut self, w: &mut World, field: &Name, body: &[Stmt]) -> R<Vec<World>> {
        let t = self.need_here(w, field)?;
        let r = rt(w, &t);
        let outer_sh = w.shape.clone();
        let inner = |w: &World, sh: Shape| {
            let mut outer = vec![outer_sh.clone()];
            outer.extend(w.outer.iter().cloned());
            World { shape: sh, outer, writes: IndexSet::new(), ..w.clone() }
        };
        let back = |w: &World, r: &World, ty: Ty| {
            let mut writes = w.writes.clone();
            writes.insert(field.clone());
            World { shape: outer_sh.set(field, ty, false), outer: w.outer.clone(), writes, ..r.clone() }
        };
        let restore = |w: &World, r: &World| World { outer: w.outer.clone(), ..r.clone() };
        if r.is("list") {
            let el = as_list(w, &t, &format!("@{field}"))?;
            let er = rt(w, &el);
            if er.is("never") {
                return Ok(vec![w.clone()]);
            }
            if er.is("rec") || er.is("any") {
                let sh = match &er {
                    Ty::Rec(s, _) => (**s).clone(),
                    _ => as_rec(w, &el, &format!("@{field}"))?,
                };
                let res = self.run_body(vec![inner(w, sh)], body)?;
                if res.iter().any(|x| x.fail.is_some()) {
                    return Ok(res.iter().filter(|x| x.fail.is_some()).map(|x| restore(w, x)).collect());
                }
                if res.is_empty() {
                    return Ok(vec![]);
                }
                let mut merged = res[0].clone();
                for o in &res[1..] {
                    for (k, f) in &o.facts {
                        if !merged.facts.contains_key(k) {
                            merged.set(k, f.clone());
                        }
                    }
                }
                let shapes: Vec<Shape> = res.iter().map(|o| o.shape.clone()).collect();
                return Ok(vec![back(w, &merged, list(rec(join_shapes(&shapes))))]);
            }
            let it = Shape::closed(IndexMap::from([("it".into(), Field { ty: el, opt: false })]));
            let res: Vec<World> = self.run_body(vec![inner(w, it)], body)?.into_iter().filter(|x| x.fail.is_none()).collect();
            if res.is_empty() {
                return Ok(vec![]);
            }
            let mut et = Ty::Never;
            for o in &res {
                match o.shape.fields.get("it") {
                    Some(f) if !f.opt => et = join(&et, &rt(o, &f.ty)),
                    _ => return terr(format!("@{field}( … ) over plain values must leave a field named it")),
                }
            }
            return Ok(vec![back(w, &res[0], list(et))]);
        }
        if r.is("rec") || r.is("any") {
            let sh = as_rec(w, &t, &format!("@{field}"))?;
            let res = self.run_body(vec![inner(w, sh)], body)?;
            return Ok(res.iter().map(|x| if x.fail.is_some() { restore(w, x) } else { back(w, x, rec(x.shape.clone())) }).collect());
        }
        terr(format!("@{field}( … ) needs a record or a list, but {field} is {}", fmt_t(Some(w), &r, 0)))
    }

    fn par(&mut self, w: &World, branches: &[Body]) -> R<Vec<World>> {
        let mut res = vec![];
        for b in branches {
            res.push(self.run_body(vec![World { writes: IndexSet::new(), ..w.clone() }], b)?);
        }
        let ws: Vec<IndexSet<Name>> = res.iter().map(|rs| rs.iter().flat_map(|x| x.writes.iter().cloned()).collect()).collect();
        for i in 0..ws.len() {
            for j in i + 1..ws.len() {
                let both: Vec<&Name> = ws[i].iter().filter(|x| ws[j].contains(*x) || &***x == "*").collect();
                if !both.is_empty() || ws[j].contains("*") {
                    let what = match both.first() {
                        Some(x) if &***x != "*" => x.to_string(),
                        _ => "the whole record".into(),
                    };
                    return terr(format!("both sides of ‖ write {what} — side {} and side {}", i + 1, j + 1));
                }
            }
        }
        let mut combos = vec![w.clone()];
        for (bi, rs) in res.iter().enumerate() {
            let mut next = vec![];
            for c in &combos {
                for r in rs {
                    if r.fail.is_some() {
                        next.push(r.clone());
                        continue;
                    }
                    let mut x = c.clone();
                    let mut bad = false;
                    for (k, f) in &r.facts {
                        match x.facts.get(k) {
                            Some(g) if g.has != f.has => bad = true,
                            Some(_) => {}
                            None => x.set(k, f.clone()),
                        }
                    }
                    if bad {
                        continue;
                    }
                    let mut s = x.shape.clone();
                    for f in &ws[bi] {
                        if let Some(fd) = r.shape.fields.get(f) {
                            s = s.set(f, fd.ty.clone(), fd.opt);
                        } else if r.shape.absent.contains(f) {
                            s = s.del(f);
                        }
                    }
                    x.shape = s;
                    for f in &ws[bi] {
                        x.write(f);
                    }
                    x.lifted |= r.lifted;
                    next.push(x);
                }
            }
            combos = widen(next)?;
        }
        Ok(combos)
    }

    fn table(&mut self, w: &mut World, table: &Name, op: &str, compiled: Option<&Where>) -> R<Vec<World>> {
        let Some(t) = self.p.tables.get(table) else { return Ok(vec![]) };
        if !TABLE_OPS.contains(&op) {
            return Ok(vec![]);
        }
        let row = rec(row_shape(t));
        let s = w.shape.clone();
        let id_t = |cx: &Self, w: &mut World| -> R<()> {
            let id = cx.need(w, &s, "id")?;
            constrain(w, &id, &Ty::Num, &format!("{table}.{op}: id")).map(|_| ())
        };
        let writes = |w: &mut World, ks: &[&Name]| ks.iter().for_each(|k| w.write(k));
        let two = |w: &World, a: Shape, b: Shape| vec![w.with_shape(a), w.with_shape(b)];
        let (missing, deleted, changes): (Name, Name, Name) = ("missing".into(), "deleted".into(), "changes".into());
        match op {
            "all" | "where" => {
                need_absent(w, &s, &t.name)?;
                if let Some(c) = compiled {
                    for p in &c.params {
                        if let Param::Expr(e) = p {
                            self.ty_of(w, e, &s)?;
                        }
                    }
                }
                writes(w, &[&t.name]);
                w.shape = s.set(&t.name, list(row), false);
                Ok(vec![w.clone()])
            }
            "get" => {
                id_t(self, w)?;
                need_absent(w, &s, &t.single)?;
                need_absent(w, &s, &missing)?;
                writes(w, &[&t.single, &missing]);
                Ok(two(w, s.set(&t.single, row, false), s.set(&missing, Ty::Bool, false)))
            }
            "insert" => {
                let r = self.need(w, &s, &t.single)?;
                let sh = as_rec(w, &r, &format!("{table}.insert"))?;
                check_row_shape(w, t, &sh, false)?;
                writes(w, &[&t.single]);
                w.shape = s.set(&t.single, row, false);
                Ok(vec![w.clone()])
            }
            "update" => {
                id_t(self, w)?;
                let ct = self.need(w, &s, &changes)?;
                match rt(w, &ct) {
                    Ty::Rec(_, Some(delta)) => {
                        let shape = delta_changes_shape(w, &delta);
                        check_row_shape(w, t, &shape, true)?;
                        let (a, b) = &*delta;
                        if let (Origin::Input(o), Origin::Closed) = (&a.origin, &b.origin) {
                            let mut allowed: Vec<Name> = vec!["id".into()];
                            allowed.extend(view(w, b).present.keys().cloned());
                            allowed.extend(t.cols.iter().filter(|c| c.opt).map(|c| c.name.clone()));
                            add_only(w, o, &allowed, &t.name, Role::Old);
                        }
                    }
                    _ => {
                        let sh = as_rec(w, &ct, &format!("{table}.update"))?;
                        check_row_shape(w, t, &sh, true)?;
                    }
                }
                need_absent(w, &s, &t.single)?;
                need_absent(w, &s, &missing)?;
                writes(w, &[&changes, &t.single, &missing]);
                let b = s.del(&changes);
                Ok(two(w, b.set(&t.single, row, false), b.set(&missing, Ty::Bool, false)))
            }
            _ => {
                id_t(self, w)?;
                need_absent(w, &s, &deleted)?;
                need_absent(w, &s, &missing)?;
                writes(w, &[&deleted, &missing]);
                Ok(two(w, s.set(&deleted, Ty::Bool, false), s.set(&missing, Ty::Bool, false)))
            }
        }
    }

    pub fn analyze_word(&mut self, w: &Word) -> Sig {
        let mut groups = vec![];
        for (ci, c) in w.clauses.iter().enumerate() {
            self.ci = ci;
            self.chunk_line = c.chunk_line;
            let mut world = World::new(Shape::open(Origin::Input("".into())));
            world.outer = (1..=3).map(|n| Shape::open(Origin::Input(format!("^{n}").into()))).collect();
            pattern_facts(&mut world, c.pattern.as_deref().unwrap_or_default(), "");
            if let Some(g) = &c.guard {
                world.guard_mode = true;
                let s = world.shape.clone();
                if let Err(e) = self.ty_of(&mut world, g, &s) {
                    self.err(Pos { line: c.line, col: 0 }, format!("guard of {}: {}", w.name, e.msg));
                }
                world.guard_mode = false;
            }
            for e in c.pattern.iter().flatten().filter(|e| e.mode == Mode::Take) {
                world.shape = world.shape.del(&e.name);
                world.write(&e.name);
            }
            let mut group = Group { guard: c.guard.is_some(), pattern: pattern_of(&world), cases: vec![] };
            let res = match self.run_body(vec![world], &c.body) {
                Ok(r) => r,
                Err(e) => {
                    self.err(Pos { line: c.line, col: 0 }, e.msg);
                    vec![]
                }
            };
            let cases = group_cases(res.iter().map(|r| case_of(r, &group)).collect());
            group.cases = simplify(cases, &group);
            groups.push(group);
        }
        Sig::Groups(groups)
    }
}
