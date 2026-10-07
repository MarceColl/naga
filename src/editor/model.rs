//! Asking a model for a hole's fill: DeepSeek V4.1 Flash over its OpenAI-style chat API.
//! It is told the language (the cheat sheet `?` opens), the program and the hole, and gets
//! tools to look things up and to try a fill before it answers with one.
use crate::editor::analysis::Analysis;
use crate::editor::{agent, help, ops};
use serde_json::{Value, json};
use std::time::Duration;

const URL: &str = "https://api.deepseek.com/chat/completions";
const MODEL: &str = "deepseek-flash";
/// Requests before it must have answered.
const TURNS: usize = 16;

/// What the model proposes for a hole: the code that replaces it, any new words it needs,
/// and one line on why.
#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    pub code: String,
    pub helpers: Option<String>,
    pub why: String,
}

/// What the model works on: the program, its hole (by step id) and what it's asked.
#[derive(Clone, Debug)]
pub struct Question {
    pub text: String,
    pub hole: usize,
    pub prompt: String,
}

const EXAMPLES: [(&str, &str); 11] = [
    ("accounts", include_str!("../../examples/accounts.nag")),
    ("countdown", include_str!("../../examples/countdown.nag")),
    ("counter", include_str!("../../examples/counter.nag")),
    ("customers", include_str!("../../examples/customers.nag")),
    ("invoice", include_str!("../../examples/invoice.nag")),
    ("ledger", include_str!("../../examples/ledger.nag")),
    ("ledger-server", include_str!("../../examples/ledger-server.nag")),
    ("order", include_str!("../../examples/order.nag")),
    ("quote", include_str!("../../examples/quote.nag")),
    ("shape", include_str!("../../examples/shape.nag")),
    ("todos", include_str!("../../examples/todos.nag")),
];

fn system() -> String {
    format!(
        "You fill holes in programs written in naga. Its reference follows; the program, the hole and what \
         reaches it come in the first message.\n\nLook before you write: `words` and `word` show what the \
         program has and the record each word takes and leaves; `help` and `examples` show the language. \
         Then `try_fill` your code: it parses it, runs the checker and the program's runs. When it passes, \
         answer with `fill`. Use only fields the record has. Keep it short and in the program's style.\n\n{}",
        &*help::TEXT
    )
}

fn tool(name: &str, what: &str, props: Value, required: &[&str]) -> Value {
    json!({"type": "function", "function": {"name": name, "description": what,
        "parameters": {"type": "object", "properties": props, "required": required, "additionalProperties": false}}})
}

fn tools() -> Value {
    let s = |d: &str| json!({"type": "string", "description": d});
    json!([
        tool("help", "A section of the naga reference, by heading (or none for the list of headings).", json!({"topic": s("A heading, or part of one: steps, tables, live pages, runs…")}), &[]),
        tool("words", "Every word in the program, with the record each clause takes and leaves.", json!({}), &[]),
        tool("word", "One word: its clauses as written, its signatures, and which words call it.", json!({"name": s("The word")}), &["name"]),
        tool("overview", "The program's tables (columns, seeds), routes, live pages and runs with how each run does now.", json!({}), &[]),
        tool("examples", "A whole example program by name, or none for the list.", json!({"name": s("An example's name")}), &[]),
        tool("try_fill", "Puts the code in the hole and reports whether it parses, what the checker says and how the runs do. Changes nothing.",
            json!({"code": s("What replaces the hole. Several lines are fine."), "helpers": s("New word definitions it needs, or empty.")}), &["code"]),
        tool("fill", "Your answer: the code that replaces the hole.",
            json!({"code": s("What replaces the hole."), "helpers": s("New word definitions it needs, or empty."), "why": s("One short line on the approach.")}), &["code", "why"]),
    ])
}

/// A tool's answer, as text for the model.
fn run_tool(q: &Question, name: &str, args: &Value) -> String {
    let an = Analysis::new(&q.text);
    let arg = |k: &str| args[k].as_str().unwrap_or("").trim().to_string();
    let sigs = |w: &str| {
        let cases = an.checked.as_ref().and_then(|c| c.signatures.get(w));
        cases.into_iter().flatten().map(|c| format!("  {w} : {} → {}", c.input, c.fail.clone().unwrap_or_else(|| c.out.join("  |  ")))).collect::<Vec<_>>().join("\n")
    };
    match name {
        "help" => {
            let topic = arg("topic").to_lowercase();
            let sections: Vec<&str> = help::TEXT.split("\n# ").collect();
            match sections.iter().find(|s| !topic.is_empty() && s.lines().next().unwrap_or("").to_lowercase().contains(&topic)) {
                Some(s) => format!("# {}", s.trim_start_matches("# ")),
                None => format!("Headings: {}", sections.iter().map(|s| s.lines().next().unwrap_or("").trim_start_matches("# ")).collect::<Vec<_>>().join(", ")),
            }
        }
        "words" => {
            let mut names: Vec<_> = an.program.words.keys().collect();
            names.sort();
            names.iter().map(|w| sigs(w)).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n")
        }
        "word" => {
            let w = arg("name").to_lowercase();
            let Some(word) = an.program.words.get(w.as_str()) else { return format!("there's no word {w}") };
            let src: Vec<&str> = word.clauses.iter().map(|c| c.chunk_text.trim_end()).collect();
            let mut callers: Vec<String> = an.steps.iter().filter(|s| s.call().is_some_and(|c| **c == *w)).map(|s| s.word.to_string()).collect();
            callers.dedup();
            format!("{}\n\nsignatures:\n{}\n\ncalled by: {}", src.join("\n"), sigs(&w), if callers.is_empty() { "nothing".into() } else { callers.join(", ") })
        }
        "overview" => {
            let p = &an.program;
            let mut out = vec![];
            for t in p.tables.values() {
                let cols: Vec<String> = t.cols.iter().map(|c| format!("{}: {}{}", c.name, c.ty.name(), if c.opt { "?" } else { "" })).collect();
                let seeded = p.seeds.iter().any(|s| s.table == t.name);
                out.push(format!("table {} :: {{{}}}{}  (a row is `{}`)", t.name, cols.join(", "), if seeded { ", seeded" } else { "" }, t.single));
            }
            out.extend(p.routes.iter().map(|r| format!("route {} {} = {}", r.verb, r.path, r.word)));
            out.extend(p.lives.iter().map(|l| format!("live {} = {}", l.path, l.word)));
            for (r, o) in p.runs.iter().zip(crate::runs::all(p)) {
                out.push(format!("run \"{}\": {}{}", r.name, o.verdict.name(), o.detail.map(|d| format!(" — {d}")).unwrap_or_default()));
            }
            if out.is_empty() { "no tables, routes, live pages or runs".into() } else { out.join("\n") }
        }
        "examples" => match EXAMPLES.iter().find(|(n, _)| *n == arg("name")) {
            Some((_, src)) => src.to_string(),
            None => format!("Examples: {}", EXAMPLES.iter().map(|(n, src)| format!("{n} ({})", src.lines().next().unwrap_or("").trim_start_matches("-- "))).collect::<Vec<_>>().join("; ")),
        },
        "try_fill" => {
            let Some(h) = an.step(q.hole).cloned() else { return "the hole is gone".into() };
            let helpers = Some(arg("helpers")).filter(|h| !h.is_empty());
            let cand = Analysis::new(&ops::fill(&an, &h, &arg("code"), helpers.as_deref()));
            let runs = crate::runs::all(&cand.program);
            let verdicts: Vec<String> = cand.program.runs.iter().zip(&runs).map(|(r, o)| format!("run \"{}\": {}", r.name, o.verdict.name())).collect();
            match agent::judge(&an, &cand) {
                Ok(()) => format!("passes: it parses, adds no checker error and breaks no run.\n{}", verdicts.join("\n")),
                Err(why) => format!("fails: {why}\n{}", verdicts.join("\n")),
            }
        }
        _ => format!("there's no tool {name}"),
    }
}

/// Asks the model to fill `q`'s hole, letting it use the tools first. Blocks; call it off the
/// UI thread.
pub fn ask(q: &Question) -> Result<Fill, String> {
    let key = std::env::var("DEEPSEEK_API_KEY").ok().filter(|k| !k.is_empty()).ok_or("set DEEPSEEK_API_KEY to let the agent fill holes")?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(300))).http_status_as_error(false).build().into();
    let mut msgs = vec![json!({"role": "system", "content": system()}), json!({"role": "user", "content": q.prompt})];
    for _ in 0..TURNS {
        let body = json!({"model": MODEL, "messages": msgs, "tools": tools()});
        let mut res = agent.post(URL).header("content-type", "application/json").header("authorization", &format!("Bearer {key}")).send_json(&body).map_err(|e| format!("couldn't reach DeepSeek: {e}"))?;
        let status = res.status().as_u16();
        let v: Value = res.body_mut().read_json().map_err(|e| format!("DeepSeek's answer isn't JSON: {e}"))?;
        if status != 200 {
            return Err(format!("DeepSeek answered {status}: {}", v.pointer("/error/message").and_then(Value::as_str).unwrap_or("no message")));
        }
        // the message goes back as it came, reasoning included
        let m = v["choices"][0]["message"].clone();
        msgs.push(m.clone());
        let calls = m["tool_calls"].as_array().cloned().unwrap_or_default();
        if calls.is_empty() {
            msgs.push(json!({"role": "user", "content": "Answer by calling fill."}));
            continue;
        }
        for c in calls {
            let name = c["function"]["name"].as_str().unwrap_or("");
            let args: Value = serde_json::from_str(c["function"]["arguments"].as_str().unwrap_or("{}")).unwrap_or(json!({}));
            if name == "fill" {
                let s = |k: &str| args[k].as_str().unwrap_or("").trim().to_string();
                return Ok(Fill { code: s("code"), helpers: Some(s("helpers")).filter(|h| !h.is_empty()), why: s("why") });
            }
            msgs.push(json!({"role": "tool", "tool_call_id": c["id"], "content": run_tool(q, name, &args)}));
        }
    }
    Err(format!("no fill after {TURNS} requests"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "table notes :: {title: Text}\nroute GET /x/:n:Int = double\ndouble = ? answer 200 with twice n\nrun \"x\"\n  GET /x/4 → 200 8\n";

    fn q() -> Question {
        let an = Analysis::new(SRC);
        Question { text: SRC.into(), hole: an.holes().next().unwrap().id, prompt: String::new() }
    }

    #[test]
    fn the_tools_show_the_program_and_try_fills_without_keeping_them() {
        let q = q();
        let t = |name: &str, args: Value| run_tool(&q, name, &args);
        assert!(t("help", json!({"topic": "tables"})).starts_with("# Tables"), "{}", t("help", json!({"topic": "tables"})));
        assert!(t("help", json!({})).starts_with("Headings: The idea, Words and clauses"));
        assert!(t("word", json!({"name": "DOUBLE"})).starts_with("double = ? answer"));
        let o = t("overview", json!({}));
        assert!(o.contains("table notes :: {title: Text}") && o.contains("route GET /x/:n:Int = double") && o.contains("run \"x\": waits"), "{o}");
        assert!(t("examples", json!({})).contains("countdown"));
        assert!(t("examples", json!({"name": "todos"})).contains("table todos"));
        assert!(t("try_fill", json!({"code": "+{status: 200, json: n * 2}"})).starts_with("passes"));
        let wrong = t("try_fill", json!({"code": "+{status: 200, json: n}"}));
        assert!(wrong.starts_with("fails: run “x” answers 200 4"), "{wrong}");
    }

    /// One real request: `DEEPSEEK_API_KEY=… cargo test -- --ignored live`
    #[test]
    #[ignore]
    fn live_deepseek_fills_a_hole_that_passes() {
        let an = Analysis::new(SRC);
        let h = an.holes().next().unwrap().clone();
        let a = agent::Agent { id: 1, hole: h.id, label: "double".into(), tries: 0, fill: None, shown: 0, base: std::rc::Rc::new(Analysis::new(SRC)), original: None, rejected: None };
        let fill = ask(&Question { prompt: agent::prompt(&an, &h, &a), ..q() }).expect("an answer");
        let cand = Analysis::new(&ops::fill(&an, &h, &fill.code, fill.helpers.as_deref()));
        assert_eq!(agent::judge(&an, &cand), Ok(()), "{}", cand.text);
        assert_eq!(crate::editor::state::Runs::of(&cand).passing(), 1, "{}", cand.text);
    }
}
