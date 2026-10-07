//! The REPL: a record you keep, and lines of steps run on it with the program's words. A
//! line is the body of a word; one starting with `=` is an expression, shown and not kept.
use crate::serve::run_word;
use crate::store::Store;
use crate::syntax::parse;
use crate::value::*;
use std::rc::Rc;
use std::sync::Arc;

/// The word a line becomes, and the field an expression's value goes in.
const WORD: &str = "__repl-line";
const VALUE: &str = "__repl-value";

/// One line entered: what it said, and the record it left or why it stopped. An
/// expression's value is shown; the record stays as it was.
#[derive(Clone, Debug)]
pub struct Entry {
    pub line: String,
    pub said: Vec<String>,
    pub out: Result<Arc<Rec>, String>,
    pub value: Option<Value>,
}

impl PartialEq for Entry {
    fn eq(&self, o: &Entry) -> bool {
        let shown = |e: &Entry| (e.out.as_ref().map(|r| show_rec(r)).map_err(Clone::clone), e.value.as_ref().map(Value::show));
        self.line == o.line && self.said == o.said && shown(self) == shown(o)
    }
}

/// A line waiting to run: on `rec`, with the words of `text`.
#[derive(Clone, Debug)]
pub struct Ask {
    pub id: u64,
    pub line: String,
    pub rec: Arc<Rec>,
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct Repl {
    pub open: bool,
    /// Keys go to the input line.
    pub focus: bool,
    pub entries: Rc<Vec<Entry>>,
    /// The record the next line runs on.
    pub rec: Arc<Rec>,
    pub input: String,
    /// The caret, in characters.
    pub col: usize,
    /// Which earlier line up and down brought back, counting from the last.
    pub recall: Option<usize>,
    /// The line running now.
    pub waiting: Option<u64>,
    pub asked: u64,
}

/// Runs `line` on `rec` against `store`, with the words of `text`. True when it wrote to a
/// table.
pub fn eval(text: &str, line: &str, rec: Arc<Rec>, store: &mut Store) -> (Entry, bool) {
    let entry = |out, said, value| Entry { line: line.into(), said, out, value };
    let (expr, steps) = match line.trim().strip_prefix('=') {
        Some(e) => (true, format!("+{{{VALUE}: {e}}}")),
        None => (false, line.trim().to_string()),
    };
    let head = format!("{text}\n\n{WORD} = ");
    let p = parse(&format!("{head}{steps}\n"));
    let at = head.matches('\n').count() + 1;
    if let Some(e) = p.errors.iter().find(|e| e.line >= at) {
        return (entry(Err(e.msg.clone()), vec![], None), false);
    }
    let ran = run_word(&p, store, &WORD.into(), (*rec).clone(), crate::funcs::Clock::now());
    let wrote = ran.sql.iter().any(|q| ["INSERT", "UPDATE", "DELETE"].iter().any(|w| q.starts_with(w)));
    let e = match ran.out {
        Ok(out) if expr => entry(Ok(rec), ran.said, out.get(VALUE).cloned()),
        Ok(out) => entry(Ok(out), ran.said, None),
        Err(msg) => entry(Err(msg), ran.said, None),
    };
    (e, wrote)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "table notes :: {text: Text}\nseed notes = [{text: \"hi\"}]\ndouble = +{twice: n * 2}\n";

    fn store() -> Store {
        let p = parse(SRC);
        let mut s = Store::new(&p).unwrap();
        s.seed(&p, &["notes".into()]).unwrap();
        s
    }

    #[test]
    fn lines_run_on_the_record_with_the_programs_words_and_tables() {
        let mut s = store();
        let (e, _) = eval(SRC, "+{n: 3} n+1", Arc::default(), &mut s);
        let rec = e.out.clone().unwrap();
        assert_eq!(show_rec(&rec), "{n: 4}");
        let (e, _) = eval(SRC, "double", rec.clone(), &mut s);
        assert_eq!(show_rec(&e.out.unwrap()), "{n: 4, twice: 8}");
        // an expression is shown, and the record stays
        let (e, _) = eval(SRC, "= n * 10", rec.clone(), &mut s);
        assert_eq!((e.value.map(|v| v.show()), show_rec(&e.out.unwrap())), (Some("40".into()), "{n: 4}".into()));
        // tables are read and written
        let (e, wrote) = eval(SRC, "+{note: {text: \"yo\"}} notes.insert say \"#{note.id}\" -{note} notes.all", rec, &mut s);
        assert!(wrote);
        assert_eq!(e.said, ["#2"]);
        assert_eq!(e.out.unwrap().get("notes").map(|v| v.show()), Some(r#"[{id: 1, text: "hi"}, {id: 2, text: "yo"}]"#.into()));
    }

    #[test]
    fn a_line_that_fails_keeps_the_record_and_says_why() {
        let mut s = store();
        let (e, _) = eval(SRC, "+{n: ", Arc::default(), &mut s);
        assert!(e.out.is_err());
        let (e, _) = eval(SRC, "nope", Arc::default(), &mut s);
        assert_eq!(e.out.err(), Some("nope isn't defined yet — write nope = ? to leave it open".into()));
    }
}
