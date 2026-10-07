//! Asking Claude for a hole's fill: one Messages API request over raw HTTP (Rust has no
//! official SDK), with structured output so the answer is just the code.
use serde_json::{Value, json};
use std::time::Duration;

const MODEL: &str = "claude-opus-5-5";

/// What Claude proposes for a hole: the code that replaces it, any new words it needs, and
/// one line on why.
#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    pub code: String,
    pub helpers: Option<String>,
    pub why: String,
}

/// The language, briefly; the program itself is the best teacher.
const SYSTEM: &str = r#"You write code in naga, a Forth where a record flows through words.

- A word is `name = steps`, or several clauses `name {pattern} when guard = steps` tried top to bottom; the first that matches runs.
- Patterns: `{f}` needs field f, `{-f}` needs it and removes it, `{¬f}` needs it absent, `{f: Lit}` matches a value, nested `{f: {g}}`.
- Steps run left to right on the record: `+{f: expr}` adds (f must be absent), `-{f, g}` removes, `.{f g}` keeps only those, `f=expr` sets, `f+1` bumps, `a→b` renames, `⊃f` lifts f's fields up, `⊂f{a b}` packs fields into f, `⫽{…}` overrides, `⊔{…}` fills absent fields.
- `@f( steps )` runs steps inside record f, or on every item when f is a list. `( a ‖ b )` runs both on the same record and merges what they add.
- `^x` reads x from the record one focus out, `^^x` two out. Expressions: + - * / %, == ≠ < > ≤ ≥, and or not, ++ for lists and text, len sum min max head tail last str, "text {expr}" interpolation.
- Tables (table accounts: its row field is the singular, account): `accounts.get` reads `id` and leaves `account` or `missing`; `accounts.where {col: v} when guard` leaves the list in `accounts` (the guard sees columns bare and the record as ^x); `accounts.all` likewise; `accounts.insert` stores the `account` field and sets its id; `accounts.update` reads `id` and `changes` and leaves `account` or `missing`; `accounts.delete` reads `id` and leaves `deleted` or `missing`.
- Route words leave {status, json}.

Answer with the code that replaces the hole: steps, or whole clauses starting with the word's name when the word needs several. Put any new words it needs in helpers. Use only fields the record has. Keep it short and in the program's style."#;

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "code": {"type": "string", "description": "What replaces the hole. Several lines are fine."},
            "helpers": {"type": "string", "description": "New word definitions it needs, or empty."},
            "why": {"type": "string", "description": "One short line on the approach."}
        },
        "required": ["code", "helpers", "why"],
        "additionalProperties": false
    })
}

/// Sends `prompt` and returns Claude's fill. Blocks; call it off the UI thread.
pub fn ask(prompt: &str) -> Result<Fill, String> {
    let key = std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.is_empty());
    let token = std::env::var("ANTHROPIC_AUTH_TOKEN").ok().filter(|k| !k.is_empty());
    let body = json!({
        "model": MODEL,
        "max_tokens": 16000,
        "fallbacks": "default",
        "output_config": {"effort": "high", "format": {"type": "json_schema", "schema": schema()}},
        "system": SYSTEM,
        "messages": [{"role": "user", "content": prompt}],
    });
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(600))).http_status_as_error(false).build().into();
    let mut req = agent.post("https://api.anthropic.com/v1/messages").header("content-type", "application/json").header("anthropic-version", "2023-06-01");
    req = match (key, token) {
        (Some(k), _) => req.header("x-api-key", &k).header("anthropic-beta", "server-side-fallback-2026-07-01"),
        (None, Some(t)) => req.header("authorization", &format!("Bearer {t}")).header("anthropic-beta", "server-side-fallback-2026-07-01,oauth-2025-04-20"),
        (None, None) => return Err("set ANTHROPIC_API_KEY to let Claude fill holes".into()),
    };
    let mut res = req.send_json(&body).map_err(|e| format!("couldn't reach Claude: {e}"))?;
    let status = res.status().as_u16();
    let v: Value = res.body_mut().read_json().map_err(|e| format!("Claude's answer isn't JSON: {e}"))?;
    if status != 200 {
        let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("no message");
        return Err(format!("Claude answered {status}: {msg}"));
    }
    if v["stop_reason"] == "refusal" {
        return Err("Claude declined to fill this hole".into());
    }
    let text: String = v["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
    let fill: Value = serde_json::from_str(&text).map_err(|e| format!("Claude's fill isn't the expected JSON: {e}"))?;
    let s = |k: &str| fill[k].as_str().unwrap_or("").trim().to_string();
    Ok(Fill { code: s("code"), helpers: Some(s("helpers")).filter(|h| !h.is_empty()), why: s("why") })
}

#[cfg(test)]
mod tests {
    use crate::editor::analysis::Analysis;
    use crate::editor::{agent, ops};

    /// One real request: `ANTHROPIC_API_KEY=… cargo test -p naga-editor -- --ignored live`
    #[test]
    #[ignore]
    fn live_claude_fills_a_hole_that_passes() {
        let src = "route GET /x/:n:Int = double\ndouble = ? answer 200 with twice n\nrun \"x\"\n  GET /x/4 → 200 8\n";
        let an = Analysis::new(src);
        let h = an.holes().next().unwrap().clone();
        let a = agent::Agent { id: 1, hole: h.id, label: "double".into(), tries: 0, fill: None, shown: 0, base: std::rc::Rc::new(Analysis::new(src)), original: None, rejected: None };
        let fill = super::ask(&agent::prompt(&an, &h, &a)).expect("Claude answers");
        let cand = Analysis::new(&ops::fill(&an, &h, &fill.code, fill.helpers.as_deref()));
        assert_eq!(agent::judge(&an, &cand), Ok(()), "{}", cand.text);
        assert_eq!(cand.passing(), 1, "{}", cand.text);
    }
}
