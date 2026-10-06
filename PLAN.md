# naga — Rust port of the Forthy reference

Goal: a `naga` CLI evaluator + signature checker whose output matches `spec/lang.js` byte for byte.
Parity first; language changes (README open issues) come after.

## How it fits

```
src .nag ─▶ syntax (lex, parse) ─▶ Program ─┬─▶ machine/session ─▶ store (rusqlite)
                                            └─▶ check (signatures, Salsa memo)
                     report = parse diagnostics + run trace + sql log + check result
```

The oracle is `spec/report.js`: it prints the same report from the JS engine and
writes `tests/cases/*.nag` + `tests/golden/*.out`. `cargo test` diffs every case.

## Modules

| module | holds |
|---|---|
| `num` | JS number semantics: `Number#toString`, `toPrecision(12)`, `Math.round`, JSON text |
| `value` | `Value` (Rc-shared, records are `IndexMap`), `show`, `text`, `eqv` |
| `syntax` | tokens with UTF-16 columns, Pratt expressions, patterns, statements, tables, seeds |
| `eval` | expressions, builtins, record algebra |
| `store` | SQLite tables: create/seed/all/get/where/insert/update/delete, where→SQL compiler |
| `machine` | immutable `State`, frames `Seq/Focus/Map/Par`, `step` |
| `session` | history, `run(mode, breakpoints)`, seek/back/restart |
| `check` | types/shapes, worlds, expression typing, record-algebra shapes, signatures, call-site application, statement analysis, SCC driver with memo |
| `report` | the text the CLI prints |

## Dropped (playground only)

Checker before/after/head points, `previewSql`, per-step DB snapshots, sql.js driver, in-memory store.

## Status

Parity reached: 92 golden cases, stepping and incremental traces, and differential fuzzing
(`spec/fuzz.js`; 2,900 mutants identical) all match the reference.

## Exactness hazards

JS number formatting · `Math.round` half-up · UTF-16 `len` and columns · Map order (delete+set moves to end:
always `shift_remove`) · checker world dedup keys depend on fact insertion order · sorting is UTF-16 order.
