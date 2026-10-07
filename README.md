# naga

A Forth where the subject record is the dataflow: words compose by juxtaposition, clauses
match on record shape, `+{}` / `-{}` widen and narrow, `@f( … )` focuses, `( a ‖ b )` forks
and merges, tables are SQLite, and a compositional checker gives every word a signature.

```
cargo run --release -- run examples/ledger.nag
```

prints parse diagnostics, the run (output, final record, every SQL statement) and the
checker's errors and signatures.

## The reference, frozen

`spec/` holds the JavaScript reference (`lang.js`) this port matched byte for byte, until naga
made names case-insensitive and constants `:name`. The goldens in `tests/golden/` are now
naga's own output: the 92 cases were converted (`Asset` → `:asset`) and behave as they did
under the reference, differing only in spelling and in error columns.

| command | checks |
| --- | --- |
| `cargo test` | `tests/cases/*.nag` against `tests/golden/*.out`; stepping/time travel; incremental rechecks; routes, runs, the live database, the editor |

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
