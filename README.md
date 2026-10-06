# naga

A Forth where the subject record is the dataflow: words compose by juxtaposition, clauses
match on record shape, `+{}` / `-{}` widen and narrow, `@f( … )` focuses, `( a ‖ b )` forks
and merges, tables are SQLite, and a compositional checker gives every word a signature.

```
cargo run --release -- run examples/ledger.nag
```

prints parse diagnostics, the run (output, final record, every SQL statement) and the
checker's errors and signatures.

## Parity with the reference

`spec/` holds the JavaScript reference (`lang.js`) this port matches byte for byte.

| command | checks |
| --- | --- |
| `cargo test` | `tests/cases/*.nag` against `tests/golden/*.out`; stepping/time travel; incremental rechecks |
| `node spec/report.js --golden` | regenerates the goldens from the reference |
| `node spec/fuzz.js 500 1` | mutates examples and cases, diffs `naga run` against the reference (needs a release build) |

## Layout

| module | holds |
| --- | --- |
| `syntax`, `ast` | lexer (UTF-16 columns, like the reference), parser, program |
| `value`, `num` | values, printing, record algebra, JS number semantics |
| `eval` | expressions and builtins |
| `store` | SQLite tables, where → SQL |
| `machine` | immutable states, stepping, sessions with history |
| `check` | signature checker: `ty` shapes, `world` facts and obligations, `expr`, `stmt`, `apply` (call sites), `sig` (cases), `mod` (SCC driver + memo) |
| `report` | what `naga run` prints |
