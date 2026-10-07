//! The functions expressions can call, each declared once: its name and signature, a line for
//! the cheat sheet, how it runs and how the checker types it. Effectful ones read the clock
//! or randomness from the run's `Clock`: real for requests, fixed for runs.
use crate::check::ty::{Ty, list as list_t};
use crate::check::world::{R, World, as_list, as_rec, constrain, fmt_t, rt, terr};
use crate::num;
use crate::value::*;
use base64::Engine;
use sha2::Digest;
use std::cell::Cell;
use std::sync::Arc;

/// What a run reads for time and randomness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    /// Seconds since 1970.
    pub at: f64,
    /// Randomness drawn from this seed, or, without one, from the system.
    pub seed: Option<u64>,
}

/// Runs start at this time unless they say otherwise: `run "x" at "…"`.
pub const RUN_START: f64 = 1_767_225_600.0; // 2026-01-01T00:00:00Z

impl Clock {
    pub fn now() -> Clock {
        let at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs() as f64);
        Clock { at, seed: None }
    }
    pub fn fixed(at: f64) -> Clock {
        Clock { at, seed: Some(at as u64 ^ 0x6e61_6761) }
    }
    /// The clock for step `i` of a run: the same time, its own draws.
    pub fn step(self, i: usize) -> Clock {
        Clock { seed: self.seed.map(|s| s.wrapping_add((i as u64 + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15))), ..self }
    }
}

impl Default for Clock {
    fn default() -> Clock {
        Clock::fixed(RUN_START)
    }
}

/// One evaluation's view of the clock: random bytes come from the seed, the step and how
/// many effectful calls came before in the same step, so a replay draws the same.
pub struct Ctx {
    pub clock: Clock,
    pub tick: usize,
    pub calls: Cell<u64>,
}

impl Ctx {
    pub fn new(clock: Clock, tick: usize) -> Ctx {
        Ctx { clock, tick, calls: Cell::new(0) }
    }
    fn bytes<const N: usize>(&self) -> [u8; N] {
        let mut out = [0u8; N];
        match self.clock.seed {
            Some(seed) => {
                let n = self.calls.replace(self.calls.get() + 1);
                let h = sha2::Sha256::digest([seed.to_le_bytes(), (self.tick as u64).to_le_bytes(), n.to_le_bytes()].concat());
                out.iter_mut().zip(h.iter().cycle()).for_each(|(o, b)| *o = *b);
            }
            None => getrandom::fill(&mut out).expect("system randomness"),
        }
        out
    }
}

impl Default for Ctx {
    fn default() -> Ctx {
        Ctx::new(Clock::default(), 0)
    }
}

pub struct Func {
    pub name: &'static str,
    /// How it's called and what it gives: `sha256(x) → Text`.
    pub sig: &'static str,
    pub note: &'static str,
    /// Reads the clock or randomness.
    pub effect: bool,
    /// The fewest and most arguments it takes.
    pub args: (usize, usize),
    /// Its first argument may be a list read off a list: `sum(lines.price)`.
    pub listy: bool,
    pub run: fn(&[Value], &Ctx) -> Result<Value, RunError>,
    pub ty: fn(&mut World, &[Ty]) -> R<Ty>,
}

impl std::fmt::Debug for Func {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}()", self.name)
    }
}

pub fn named(n: &str) -> Option<&'static Func> {
    FUNCS.iter().find(|f| f.name == n)
}

// ---------- running ----------
fn arg(a: &[Value], i: usize) -> Value {
    a.get(i).cloned().unwrap_or(Value::Nil)
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

fn text(s: impl Into<Arc<str>>) -> Value {
    Value::Text(s.into())
}

fn hex(b: &[u8]) -> Value {
    text(b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

fn min_max(a: &[Value], what: &str, min: bool) -> Result<Value, RunError> {
    let xs: Vec<Value> = match (a, &arg(a, 0)) {
        ([_], Value::List(xs)) => xs.to_vec(),
        _ => a.to_vec(),
    };
    if xs.is_empty() {
        return fail(format!("{what} of nothing"));
    }
    let ns = xs.iter().map(|x| number(x, what)).collect::<Result<Vec<_>, _>>()?;
    Ok(Value::Num(ns.into_iter().reduce(|p, q| if p.is_nan() || q.is_nan() { f64::NAN } else if min { p.min(q) } else { p.max(q) }).unwrap()))
}

/// `2026-10-07T12:00:00Z` for seconds since 1970.
pub fn iso(s: f64) -> String {
    let s = s.floor() as i64;
    let (days, secs) = (s.div_euclid(86400), s.rem_euclid(86400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

/// Seconds since 1970 for `2026-10-07`, `2026-10-07T12:00:00Z` or `…T12:00Z`.
pub fn parse_iso(t: &str) -> Option<f64> {
    let (date, time) = t.trim().trim_end_matches('Z').split_once('T').unwrap_or((t.trim(), "00:00:00"));
    let d: Vec<i64> = date.split('-').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let hms: Vec<f64> = time.split(':').map(|x| x.parse().ok()).collect::<Option<_>>()?;
    let (&[y, m, d], &[h, mi, ref rest @ ..]) = (&d[..], &hms[..]) else { return None };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    Some(days as f64 * 86400.0 + h * 3600.0 + mi * 60.0 + rest.first().copied().unwrap_or(0.0))
}

fn random(c: &Ctx) -> f64 {
    (u64::from_le_bytes(c.bytes::<8>()) >> 11) as f64 / (1u64 << 53) as f64
}

// ---------- typing ----------
fn num_args(w: &mut World, a: &[Ty], what: &str) -> R<()> {
    a.iter().try_for_each(|t| constrain(w, t, &Ty::Num, what).map(|_| ()))
}

fn text_args(w: &mut World, a: &[Ty], what: &str) -> R<()> {
    a.iter().try_for_each(|t| constrain(w, t, &Ty::Text, what).map(|_| ()))
}

fn min_max_t(w: &mut World, a: &[Ty], what: &str) -> R<Ty> {
    match a {
        [one] => {
            let el = as_list(w, one, what)?;
            constrain(w, &el, &Ty::Num, what)?;
        }
        _ => num_args(w, a, what)?,
    }
    Ok(Ty::Num)
}

pub static FUNCS: &[Func] = &[
    // lists, numbers, text
    Func { name: "len", sig: "len(x) → Num", note: "items in a list, characters in text, fields in a record", effect: false, args: (1, 1), listy: false,
        run: |a, _| Ok(Value::Num(match &arg(a, 0) {
            Value::List(xs) => xs.len(),
            Value::Text(s) => num::len16(s),
            Value::Rec(r) => r.len(),
            v => return fail(format!("len() needs a list, string or record, got {}", v.type_name())),
        } as f64)),
        ty: |w, a| {
            let r = rt(w, &a[0]);
            if !["list", "text", "rec", "any"].contains(&r.tag()) {
                return terr(format!("len() needs a list, text or record, got {}", fmt_t(Some(w), &r, 0)));
            }
            Ok(Ty::Num)
        } },
    Func { name: "sum", sig: "sum(xs) → Num", note: "the numbers added up", effect: false, args: (1, 1), listy: true,
        run: |a, _| Ok(Value::Num(items(&arg(a, 0), "sum()")?.iter().try_fold(0.0, |s, x| Ok::<_, RunError>(s + number(x, "sum()")?))?)),
        ty: |w, a| {
            let el = as_list(w, &a[0], "sum()")?;
            constrain(w, &el, &Ty::Num, "sum()")?;
            Ok(Ty::Num)
        } },
    Func { name: "min", sig: "min(xs) or min(a, b, …) → Num", note: "the smallest", effect: false, args: (1, 99), listy: true, run: |a, _| min_max(a, "min()", true), ty: |w, a| min_max_t(w, a, "min()") },
    Func { name: "max", sig: "max(xs) or max(a, b, …) → Num", note: "the largest", effect: false, args: (1, 99), listy: true, run: |a, _| min_max(a, "max()", false), ty: |w, a| min_max_t(w, a, "max()") },
    Func { name: "abs", sig: "abs(n) → Num", note: "without its sign", effect: false, args: (1, 1), listy: false,
        run: |a, _| Ok(Value::Num(number(&arg(a, 0), "abs()")?.abs())), ty: |w, a| num_args(w, a, "abs()").map(|_| Ty::Num) },
    Func { name: "round", sig: "round(n, places?) → Num", note: "to the nearest, or to that many decimals", effect: false, args: (1, 2), listy: false,
        run: |a, _| {
            let d = if a.len() > 1 { number(&arg(a, 1), "round()")? } else { 0.0 };
            let p = 10f64.powf(d);
            Ok(Value::Num(num::round(number(&arg(a, 0), "round()")? * p) / p))
        },
        ty: |w, a| num_args(w, a, "round()").map(|_| Ty::Num) },
    Func { name: "floor", sig: "floor(n) → Num", note: "down to a whole number", effect: false, args: (1, 1), listy: false,
        run: |a, _| Ok(Value::Num(number(&arg(a, 0), "floor()")?.floor())), ty: |w, a| num_args(w, a, "floor()").map(|_| Ty::Num) },
    Func { name: "ceil", sig: "ceil(n) → Num", note: "up to a whole number", effect: false, args: (1, 1), listy: false,
        run: |a, _| Ok(Value::Num(number(&arg(a, 0), "ceil()")?.ceil())), ty: |w, a| num_args(w, a, "ceil()").map(|_| Ty::Num) },
    Func { name: "head", sig: "head(xs)", note: "the first item", effect: false, args: (1, 1), listy: true,
        run: |a, _| items(&arg(a, 0), "head()")?.first().cloned().map_or_else(|| fail("head() of an empty list"), Ok), ty: |w, a| as_list(w, &a[0], "head()") },
    Func { name: "tail", sig: "tail(xs)", note: "all but the first", effect: false, args: (1, 1), listy: true,
        run: |a, _| Ok(list(items(&arg(a, 0), "tail()")?.iter().skip(1).cloned().collect())),
        ty: |w, a| as_list(w, &a[0], "tail()").map(|_| a[0].clone()) },
    Func { name: "last", sig: "last(xs)", note: "the last item", effect: false, args: (1, 1), listy: true,
        run: |a, _| items(&arg(a, 0), "last()")?.last().cloned().map_or_else(|| fail("last() of an empty list"), Ok), ty: |w, a| as_list(w, &a[0], "last()") },
    Func { name: "push", sig: "push(xs, x)", note: "the list with x at the end", effect: false, args: (2, 2), listy: true,
        run: |a, _| {
            let mut xs = items(&arg(a, 0), "push()")?.to_vec();
            xs.push(arg(a, 1));
            Ok(list(xs))
        },
        ty: |w, a| {
            let el = as_list(w, &a[0], "push()")?;
            Ok(list_t(crate::check::ty::join(&el, a.get(1).unwrap_or(&Ty::Any))))
        } },
    Func { name: "reverse", sig: "reverse(xs)", note: "the list backwards", effect: false, args: (1, 1), listy: true,
        run: |a, _| Ok(list(items(&arg(a, 0), "reverse()")?.iter().rev().cloned().collect())),
        ty: |w, a| as_list(w, &a[0], "reverse()").map(|_| a[0].clone()) },
    Func { name: "range", sig: "range(n) or range(from, to) → [Num]", note: "whole numbers up to, not including, the end", effect: false, args: (1, 2), listy: false,
        run: |a, _| {
            let (lo, hi) = if a.len() < 2 { (Value::Num(0.0), arg(a, 0)) } else { (arg(a, 0), arg(a, 1)) };
            let (lo, hi) = (number(&lo, "range()")?, number(&hi, "range()")?);
            let mut r = vec![];
            let mut i = lo;
            while i < hi && r.len() < 100000 {
                r.push(Value::Num(i));
                i += 1.0;
            }
            Ok(list(r))
        },
        ty: |w, a| num_args(w, a, "range()").map(|_| list_t(Ty::Num)) },
    Func { name: "str", sig: "str(x) → Text", note: "as text", effect: false, args: (1, 1), listy: false, run: |a, _| Ok(text(arg(a, 0).text())), ty: |_, _| Ok(Ty::Text) },
    Func { name: "keys", sig: "keys(r) → [Const]", note: "a record's field names", effect: false, args: (1, 1), listy: false,
        run: |a, _| match &arg(a, 0) {
            Value::Rec(r) => Ok(list(r.keys().map(|k| Value::Const(k.clone())).collect())),
            v => fail(format!("keys() needs a record, got {}", v.type_name())),
        },
        ty: |w, a| as_rec(w, &a[0], "keys()").map(|_| list_t(Ty::Const(None))) },
    // time
    Func { name: "time", sig: "time() → Text", note: "now, as 2026-10-07T12:00:00Z (fixed in runs)", effect: true, args: (0, 0), listy: false,
        run: |_, c| Ok(text(iso(c.clock.at))), ty: |_, _| Ok(Ty::Text) },
    Func { name: "unix", sig: "unix(t?) → Num", note: "seconds since 1970: now, or of a time like 2026-10-07T12:00:00Z", effect: true, args: (0, 1), listy: false,
        run: |a, c| match a.first() {
            None => Ok(Value::Num(c.clock.at)),
            Some(t) => parse_iso(&t.text()).map(Value::Num).map_or_else(|| fail(format!("unix() needs a time like 2026-10-07T12:00:00Z, got {}", t.show())), Ok),
        },
        ty: |w, a| text_args(w, a, "unix()").map(|_| Ty::Num) },
    Func { name: "iso", sig: "iso(n) → Text", note: "seconds since 1970 as a time: 2026-10-07T12:00:00Z", effect: false, args: (1, 1), listy: false,
        run: |a, _| Ok(text(iso(number(&arg(a, 0), "iso()")?))), ty: |w, a| num_args(w, a, "iso()").map(|_| Ty::Text) },
    // hashing
    Func { name: "sha256", sig: "sha256(x) → Text", note: "SHA-256 of x's text, in hex", effect: false, args: (1, 1), listy: false,
        run: |a, _| Ok(hex(&sha2::Sha256::digest(arg(a, 0).text().as_bytes()))), ty: |_, _| Ok(Ty::Text) },
    Func { name: "sha1", sig: "sha1(x) → Text", note: "SHA-1 of x's text, in hex", effect: false, args: (1, 1), listy: false,
        run: |a, _| Ok(hex(&sha1::Sha1::digest(arg(a, 0).text().as_bytes()))), ty: |_, _| Ok(Ty::Text) },
    Func { name: "md5", sig: "md5(x) → Text", note: "MD5 of x's text, in hex", effect: false, args: (1, 1), listy: false,
        run: |a, _| Ok(hex(&md5::Md5::digest(arg(a, 0).text().as_bytes()))), ty: |_, _| Ok(Ty::Text) },
    Func { name: "hmac", sig: "hmac(key, msg) → Text", note: "HMAC-SHA256 of msg with key, in hex", effect: false, args: (2, 2), listy: false,
        run: |a, _| {
            use hmac::{KeyInit, Mac};
            let mut m = hmac::Hmac::<sha2::Sha256>::new_from_slice(arg(a, 0).text().as_bytes()).expect("any key length");
            m.update(arg(a, 1).text().as_bytes());
            Ok(hex(&m.finalize().into_bytes()))
        },
        ty: |_, _| Ok(Ty::Text) },
    Func { name: "password", sig: "password(pw) → Text", note: "an Argon2id hash to store, salt included (verify checks it)", effect: true, args: (1, 1), listy: false,
        run: |a, c| {
            use argon2::password_hash::PasswordHasher;
            let h = argon2::Argon2::default().hash_password_with_salt(arg(a, 0).text().as_bytes(), &c.bytes::<16>()).map_err(|e| RunError { msg: format!("password(): {e}"), details: vec![] })?;
            Ok(text(h.to_string()))
        },
        ty: |w, a| text_args(w, a, "password()").map(|_| Ty::Text) },
    Func { name: "verify", sig: "verify(pw, hash) → Bool", note: "whether pw is the password hash was made from", effect: false, args: (2, 2), listy: false,
        run: |a, _| {
            use argon2::password_hash::{PasswordVerifier, phc::PasswordHash};
            let hash = arg(a, 1).text();
            let Ok(h) = PasswordHash::new(&hash) else { return Ok(Value::Bool(false)) };
            Ok(Value::Bool(argon2::Argon2::default().verify_password(arg(a, 0).text().as_bytes(), &h).is_ok()))
        },
        ty: |w, a| text_args(w, a, "verify()").map(|_| Ty::Bool) },
    // ids and randomness
    Func { name: "uuid", sig: "uuid() → Text", note: "a random v4 UUID (repeatable in runs)", effect: true, args: (0, 0), listy: false,
        run: |_, c| Ok(text(uuid::Builder::from_random_bytes(c.bytes::<16>()).into_uuid().to_string())), ty: |_, _| Ok(Ty::Text) },
    Func { name: "random", sig: "random(n?) → Num", note: "a number in [0, 1), or a whole number below n (repeatable in runs)", effect: true, args: (0, 1), listy: false,
        run: |a, c| {
            let r = random(c);
            match a.first() {
                None => Ok(Value::Num(r)),
                Some(n) => Ok(Value::Num((r * number(n, "random()")?).floor())),
            }
        },
        ty: |w, a| num_args(w, a, "random()").map(|_| Ty::Num) },
    // encoding
    Func { name: "base64", sig: "base64(x) → Text", note: "x's text in base64", effect: false, args: (1, 1), listy: false,
        run: |a, _| Ok(text(base64::engine::general_purpose::STANDARD.encode(arg(a, 0).text().as_bytes()))), ty: |_, _| Ok(Ty::Text) },
    Func { name: "unbase64", sig: "unbase64(t) → Text", note: "the text base64 encodes", effect: false, args: (1, 1), listy: false,
        run: |a, _| {
            let bytes = base64::engine::general_purpose::STANDARD.decode(arg(a, 0).text().as_bytes()).map_err(|e| RunError { msg: format!("unbase64(): {e}"), details: vec![] })?;
            String::from_utf8(bytes).map(text).map_err(|_| RunError { msg: "unbase64(): that isn't text".into(), details: vec![] })
        },
        ty: |w, a| text_args(w, a, "unbase64()").map(|_| Ty::Text) },
    Func { name: "json", sig: "json(v) → Text", note: "v as JSON", effect: false, args: (1, 1), listy: false, run: |a, _| Ok(text(to_plain_json(&arg(a, 0)))), ty: |_, _| Ok(Ty::Text) },
    Func { name: "unjson", sig: "unjson(t)", note: "the value JSON text holds", effect: false, args: (1, 1), listy: false,
        run: |a, _| from_json(&arg(a, 0).text()).map_or_else(|| fail(format!("unjson(): {} isn't JSON", arg(a, 0).show())), Ok),
        ty: |w, a| text_args(w, a, "unjson()").map(|_| Ty::Any) },
    Func { name: "lower", sig: "lower(t) → Text", note: "in lowercase", effect: false, args: (1, 1), listy: false,
        run: |a, _| Ok(text(arg(a, 0).text().to_lowercase())), ty: |w, a| text_args(w, a, "lower()").map(|_| Ty::Text) },
    Func { name: "upper", sig: "upper(t) → Text", note: "in uppercase", effect: false, args: (1, 1), listy: false,
        run: |a, _| Ok(text(arg(a, 0).text().to_uppercase())), ty: |w, a| text_args(w, a, "upper()").map(|_| Ty::Text) },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, a: &[Value], c: &Ctx) -> Value {
        (named(name).unwrap().run)(a, c).unwrap()
    }
    fn t(s: &str) -> Value {
        text(s)
    }

    #[test]
    fn hashes_and_encodings_are_the_standard_ones() {
        let c = Ctx::default();
        assert_eq!(call("sha256", &[t("abc")], &c).text(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(call("sha1", &[t("abc")], &c).text(), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(call("md5", &[t("abc")], &c).text(), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(call("hmac", &[t("key"), t("The quick brown fox jumps over the lazy dog")], &c).text(), "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8");
        assert_eq!(call("base64", &[t("naga!")], &c).text(), "bmFnYSE=");
        assert_eq!(call("unbase64", &[t("bmFnYSE=")], &c).text(), "naga!");
    }

    #[test]
    fn passwords_hash_with_a_salt_and_verify() {
        let c = Ctx::default();
        let h = call("password", &[t("hunter2")], &c);
        assert!(h.text().starts_with("$argon2id$v=19$"), "{}", h.text());
        assert!(call("verify", &[t("hunter2"), h.clone()], &c).truthy());
        assert!(!call("verify", &[t("hunter3"), h.clone()], &c).truthy());
        assert!(!call("verify", &[t("hunter2"), t("not a hash")], &c).truthy());
        // two hashes of one password differ: each has its own salt
        assert_ne!(call("password", &[t("hunter2")], &c).text(), h.text());
    }

    #[test]
    fn time_and_randomness_are_fixed_in_runs_and_real_otherwise() {
        let run = || Ctx::new(Clock::fixed(parse_iso("2026-10-07T12:00:00Z").unwrap()), 3);
        assert_eq!(call("time", &[], &run()).text(), "2026-10-07T12:00:00Z");
        assert_eq!(call("unix", &[t("2026-10-07")], &run()).show(), "1791331200");
        assert_eq!(call("iso", &[Value::Num(1791331200.0)], &run()).text(), "2026-10-07T00:00:00Z");
        // the same run draws the same, call by call
        let (a, b) = (run(), run());
        assert_eq!(call("uuid", &[], &a).text(), call("uuid", &[], &b).text());
        assert_ne!(call("uuid", &[], &a).text(), call("uuid", &[], &a).text());
        let r = call("random", &[Value::Num(6.0)], &a);
        assert!(matches!(r, Value::Num(n) if (0.0..6.0).contains(&n) && n.fract() == 0.0));
        // a request draws from the system
        let real = Ctx::new(Clock::now(), 0);
        assert!(call("uuid", &[], &real).text().chars().nth(14) == Some('4'));
        assert!(call("unix", &[], &real).show().parse::<f64>().unwrap() > 1.7e9);
    }
}
