//! Expressions: evaluated against the subject record, with `^` reaching enclosing records.
use crate::ast::*;
use crate::num;
use crate::value::*;
use std::sync::Arc;

pub struct Env<'a> {
    pub subj: &'a Rec,
    pub now: f64,
    /// Enclosing records, innermost first.
    pub outer: &'a [Arc<Rec>],
}

fn number(v: &Value, what: &str) -> Result<f64, RunError> {
    match v {
        Value::Num(n) => Ok(*n),
        v => fail(format!("{what} needs a number, got {}", v.described())),
    }
}

fn items<'a>(v: &'a Value, what: &str) -> Result<&'a Arc<Vec<Value>>, RunError> {
    match v {
        Value::List(xs) => Ok(xs),
        v => fail(format!("{what} needs a list, got {}", v.described())),
    }
}

fn builtin(f: Builtin, a: Vec<Value>) -> Result<Value, RunError> {
    use Builtin::*;
    let arg = |i: usize| a.get(i).cloned().unwrap_or(Value::Nil);
    let a0 = arg(0);
    Ok(match f {
        Len => Value::Num(match &a0 {
            Value::List(xs) => xs.len(),
            Value::Text(s) => num::len16(s),
            Value::Rec(r) => r.len(),
            v => return fail(format!("len() needs a list, string or record, got {}", v.type_name())),
        } as f64),
        Sum => Value::Num(items(&a0, "sum()")?.iter().try_fold(0.0, |s, x| Ok::<_, RunError>(s + number(x, "sum()")?))?),
        Min | Max => {
            let what = if f == Min { "min()" } else { "max()" };
            let xs: Vec<Value> = match (&a[..], &a0) {
                ([_], Value::List(xs)) => xs.to_vec(),
                _ => a.clone(),
            };
            if xs.is_empty() {
                return fail(format!("{} of nothing", what));
            }
            let ns = xs.iter().map(|x| number(x, what)).collect::<Result<Vec<_>, _>>()?;
            Value::Num(ns.into_iter().reduce(|p, q| if p.is_nan() || q.is_nan() { f64::NAN } else if f == Min { p.min(q) } else { p.max(q) }).unwrap())
        }
        Abs => Value::Num(number(&a0, "abs()")?.abs()),
        Round => {
            let d = if a.len() > 1 { number(&arg(1), "round()")? } else { 0.0 };
            let p = 10f64.powf(d);
            Value::Num(num::round(number(&a0, "round()")? * p) / p)
        }
        Floor => Value::Num(number(&a0, "floor()")?.floor()),
        Ceil => Value::Num(number(&a0, "ceil()")?.ceil()),
        Head => items(&a0, "head()")?.first().cloned().map_or_else(|| fail("head() of an empty list"), Ok)?,
        Tail => list(items(&a0, "tail()")?.iter().skip(1).cloned().collect()),
        Last => items(&a0, "last()")?.last().cloned().map_or_else(|| fail("last() of an empty list"), Ok)?,
        Push => {
            let mut xs = items(&a0, "push()")?.to_vec();
            xs.push(arg(1));
            list(xs)
        }
        Reverse => list(items(&a0, "reverse()")?.iter().rev().cloned().collect()),
        Range => {
            let (lo, hi) = if a.len() < 2 { (Value::Num(0.0), a0) } else { (a0, arg(1)) };
            let (lo, hi) = (number(&lo, "range()")?, number(&hi, "range()")?);
            let mut r = vec![];
            let mut i = lo;
            while i < hi && r.len() < 100000 {
                r.push(Value::Num(i));
                i += 1.0;
            }
            list(r)
        }
        Str => Value::Text(a0.text().into()),
        Keys => match &a0 {
            Value::Rec(r) => list(r.keys().map(|k| Value::Const(k.clone())).collect()),
            v => return fail(format!("keys() needs a record, got {}", v.type_name())),
        },
    })
}

pub fn get_key(o: &Value, key: &str) -> Result<Value, RunError> {
    match o {
        Value::Rec(r) => r.get(key).cloned().map_or_else(|| fail(format!("{} has no field {key}", o.show())), Ok),
        Value::List(xs) => Ok(list(xs.iter().map(|x| get_key(x, key)).collect::<Result<_, _>>()?)),
        o => fail(format!("can't read .{key} from {}", o.described())),
    }
}

pub fn eval(e: &Expr, env: &Env) -> Result<Value, RunError> {
    Ok(match e {
        Expr::Lit(v) => v.clone(),
        Expr::Now => Value::Num(env.now),
        Expr::Field(n) => env.subj.get(n).cloned().map_or_else(|| fail(format!("needs field {n}")), Ok)?,
        Expr::Outer(up, n) => {
            let Some(r) = env.outer.get(up - 1) else {
                return fail(format!("^{n}: there's no enclosing record here — ^ only works inside @field( … )"));
            };
            r.get(n).cloned().map_or_else(|| fail(format!("the enclosing record has no field {n}")), Ok)?
        }
        Expr::Get(o, k) => get_key(&eval(o, env)?, k)?,
        Expr::Proj(o, names) => project(&eval(o, env)?, names)?,
        Expr::List(xs) => list(xs.iter().map(|x| eval(x, env)).collect::<Result<_, _>>()?),
        Expr::Rec(fs) => {
            let mut m = Rec::new();
            for (k, x) in fs {
                m.insert(k.clone(), eval(x, env)?);
            }
            rec(m)
        }
        Expr::Interp(parts) => {
            let mut s = String::new();
            for p in parts {
                match p {
                    Part::Text(t) => s.push_str(t),
                    Part::Expr(x) => s.push_str(&eval(x, env)?.text()),
                }
            }
            Value::Text(s.into())
        }
        Expr::Neg(a) => Value::Num(-number(&eval(a, env)?, "minus")?),
        Expr::Not(a) => Value::Bool(!eval(a, env)?.truthy()),
        Expr::And(a, b) => Value::Bool(eval(a, env)?.truthy() && eval(b, env)?.truthy()),
        Expr::Or(a, b) => Value::Bool(eval(a, env)?.truthy() || eval(b, env)?.truthy()),
        Expr::Call(f, args) => builtin(*f, args.iter().map(|a| eval(a, env)).collect::<Result<_, _>>()?)?,
        Expr::Bin(op, a, b) => binary(*op, eval(a, env)?, eval(b, env)?)?,
    })
}

fn binary(op: BinOp, a: Value, b: Value) -> Result<Value, RunError> {
    use BinOp::*;
    Ok(match op {
        Eq => Value::Bool(eqv(&a, &b)),
        Ne => Value::Bool(!eqv(&a, &b)),
        Cat => match (&a, &b) {
            (Value::List(x), Value::List(y)) => list(x.iter().chain(y.iter()).cloned().collect()),
            _ => Value::Text(format!("{}{}", a.text(), b.text()).into()),
        },
        Within => Value::Bool(subsumes(&a, &b)),
        Over => {
            let (x, y) = rec_args(&a, &b, "⫽")?;
            let mut m = x.clone();
            m.extend(y.iter().map(|(k, v)| (k.clone(), v.clone())));
            rec(m)
        }
        Join => {
            let (x, y) = rec_args(&a, &b, "⊔")?;
            let (merged, conflict) = unify(x, y);
            if !conflict.is_empty() {
                let keys: Vec<&str> = conflict.keys().map(|k| &**k).collect();
                return Err(RunError {
                    msg: format!("⊔: the records disagree on {} — the ⊔{{…}} step adds conflict instead of failing", keys.join(", ")),
                    details: conflict
                        .iter()
                        .map(|(k, c)| {
                            let c = c.as_rec().unwrap();
                            format!("{k}: {} vs {}", c["left"].show(), c["right"].show())
                        })
                        .collect(),
                });
            }
            rec(merged)
        }
        Meet => {
            let (x, y) = rec_args(&a, &b, "⊓")?;
            rec(meet(x, y))
        }
        Diff => {
            let (x, y) = rec_args(&a, &b, "∆")?;
            rec(diff(x, y))
        }
        Apply => apply_delta(&a, &b)?,
        Lt | Gt | Le | Ge => {
            let ord = match (&a, &b) {
                (Value::Num(x), Value::Num(y)) => x.partial_cmp(y),
                (Value::Text(x), Value::Text(y)) => Some(num::cmp16(x, y)),
                _ => return fail(format!("can't compare {} with {} ({} {} {})", a.type_name(), b.type_name(), a.show(), op.glyph(), b.show())),
            };
            use std::cmp::Ordering::*;
            Value::Bool(match (op, ord) {
                (_, None) => false,
                (Lt, Some(o)) => o == Less,
                (Gt, Some(o)) => o == Greater,
                (Le, Some(o)) => o != Greater,
                (_, Some(o)) => o != Less,
            })
        }
        Add | Sub | Mul | Div | Rem => {
            let (Value::Num(x), Value::Num(y)) = (&a, &b) else {
                let hint = if op == Add && (matches!(a, Value::Text(_)) || matches!(b, Value::Text(_))) { " — use ++ to join text" } else { "" };
                return fail(format!("can't use {} on {} and {} ({} {} {}){hint}", op.glyph(), a.type_name(), b.type_name(), a.show(), op.glyph(), b.show()));
            };
            Value::Num(match op {
                Add => x + y,
                Sub => x - y,
                Mul => x * y,
                Div | Rem if *y == 0.0 => return fail("division by zero"),
                Div => x / y,
                _ => x % y,
            })
        }
    })
}
