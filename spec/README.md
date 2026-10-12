# Forthy dataflow — reference implementation

A Forth where the subject record is the dataflow: words compose by juxtaposition, clauses
match on record shape (first match wins), `+{}` / `-{}` widen and narrow, `@f( … )` focuses,
`( a ‖ b )` forks and merges, tables are SQLite, and a compositional checker builds each word's
signature from its body and its callees' signatures (Salsa-style cache).

This JS version is the executable spec for the Rust port.

## Layout

```
core/lang.js                 lexer, parser, runtime, tables, machine, sessions, checker (one file, no deps)
examples/*.fy                every sheet from the playground; ledger.fy and customers.fy are the big ones
examples/lang-examples.js    the same sheets as the playground loads them
playground/debugger.tpl.html the page (debugger, Data, Flow, Notes, Syntax tabs)
playground/build.js          inlines core + examples → playground/forthy-dataflow.html
tests/                       plain node scripts; no framework
```

## Running

Node 22+ (the table tests use the built-in `node:sqlite`).

```
node tests/run.js examples/ledger.fy        # both engines (SQLite, in-memory) + checker
node tests/run.js examples/customers.fy x   # any second arg also prints signatures
node playground/build.js
```

| Test | Covers | Expected |
| --- | --- | --- |
| machine.test.js | stepping, time travel, runtime errors, holes, breakpoints | prints traces; no exceptions |
| tables.test.js | SQLite vs in-memory engines give the same rows and log | both columns agree |
| signatures.test.js | signatures for every sheet | 0 errors except the "Shape error" sheet (1, on purpose) |
| checker-errors.test.js | error placement, incremental recheck counts | see the `== incremental` block |
| ledger-mutations.test.js | 8 injected ledger bugs | every one reported |
| algebra-mutations.test.js | 7 injected record-algebra bugs | `7/7 caught` |
| algebra-checker.test.js | static shapes of each algebra operator | read the printed signatures |

## Where things live in core/lang.js

| Section | What to know for a port |
| --- | --- |
| values | records are insertion-ordered maps (output order depends on it); constants are `{sym}` |
| text | the reference counts UTF-16 units (`len`, columns, `<`, slices); naga counts Unicode scalars, a deliberate parity break (see PLAN.md) |
| lexer | identifiers may contain `-` between letters; ASCII spellings are canonicalised (`->` → `→`, `//` → `⫽`, …) |
| expressions | Pratt parser; `.f` and `.{…}` postfix (`.{` only when glued to the left token) |
| record algebra (runtime) | change sets are plain records: `{same: v}`, `{set: new, was: old}`, `{add: v}`, `{del: old}` |
| patterns / statements | clause heads: `{f}`, `{-f}` take, `{¬f}`, `{f: Lit}`, nested, `when` guard sees the record before takes |
| tables | `.all .get .where .insert .update .delete`; NULL is an absent field; `where` compiles to one SQL WHERE |
| machine | immutable states; frames `seq`, `focus`, `map`, `par`; a tail call replaces its caller's frame |
| sessions | full history for stepping back; `seek`, `restart` |
| checker | shapes `{fields, absent, origin}`; worlds carry facts about the input; signatures are cases per clause |
| obligations | `x#only` (record carries only these fields) and `x#apart` (two records share no fields) travel up signatures like needs/lacks and are checked at the first call site where the shape is known |
| Salsa driver | Tarjan SCCs, fixpoint per SCC from ⊥, memo keyed by clause text + callee signature keys |

## Open issues

- Projection in expressions is lenient (`body.{date}` makes `date` optional). Planned: strict by default, `?` per field where absence is fine.
- After `⊃body` with both records open, a read of an unnamed field is charged to the outer record. Planned: split the case in two (one per origin), like `.get` → found / missing.
- A Δ type passed from one word to another loses its from/to shapes, so `⍎` there yields an open record.
- Not built yet: `::` decoding, dictionaries (`Map K V`, `⌸`, pointwise math, compress), the mask `a ≠ b`, list operators (join, filter, sort, group), seeded fold.
- Operator precedence for one-sided glyphs is still undecided.
