# HTTP routes

Goal: a `.nag` file declares its HTTP routes and `naga serve` answers them. Today the
router is a convention (`ledger.nag`, `customers.nag`): route words carry a
`-- GET /accounts/:id` comment, params sit at the top level of the record, the body in
`body`, and every route leaves `{status, json}`. This makes that router real, typed and
checked.

First divergence from the JS reference: `spec/` does not know `route`.

## How it fits

```
app.nag ─▶ parse ─▶ Program { words, tables, seeds, routes } ─┬─▶ check: every route is a call site
                                                              └─▶ naga serve app.nag
  axum (tokio) ── (route index, raw params, body text) ──▶ interpreter thread (Program + Store on app.db)
               ◀────────────── (status, json text) ──────── decode → initial record → step … → encode
```

## 1. Syntax

```
route VERB PATH[?QUERY] [BODY] = word

PATH  = /seg/… where seg is a literal or :name:Type
QUERY = name:Type[?] joined by &
BODY  = {name: Type[?], …}           -- the table-column syntax
```

- `VERB` ∈ `GET POST PUT PATCH DELETE`. `Type` ∈ the `ColType`s: `Int Num Text Bool Const Json`.
- Path params are required. Query and body fields are required unless they end in `?`.
- Undeclared query keys and body fields are dropped.

```
route GET    /accounts/:id:Int                 = get-account
route POST   /accounts {name: Text, kind: Const} = post-account
route PATCH  /accounts/:id:Int {name: Text?, archived: Bool?} = patch-account
route GET    /transactions?account:Int?         = get-transactions
route GET    /reports/income?from:Text&to:Text  = get-income
```

AST (`ast.rs`):

```rust
pub enum Seg { Lit(Name), Param(Col) }
pub struct Route { pub verb: Name, pub segs: Vec<Seg>, pub query: Vec<Col>, pub body: Option<Vec<Col>>, pub word: Name, pub line: usize }
// Program gains: pub routes: Vec<Route>
```

Parsing (`syntax.rs`): a `route` branch in `chunk`, beside `table` and `seed`. The verb
and path come from the raw line text up to whitespace, because the lexer would turn `//`
and `:` into operators. The optional `{…}` reuses the table-column parser, then `= word`.

`check_refs` reports each of these as an error:

- an unknown verb or type;
- an unknown word;
- the same verb and path declared twice;
- one name used twice across path, query and body;
- `?` on a path param.

## 2. Runtime: `naga serve app.nag [--port 4170]`

**Threads.** `Value`, `Program` and `Body` use `Rc`, which is not `Send`. So one
interpreter thread owns `Program` and `Store`. Axum handlers send it only `Send` data
over a `std::sync::mpsc` channel: route index, path params, query pairs, body text and
time, all as strings, plus a `tokio::sync::oneshot` for the answer `(u16, String)`.
Decoding and encoding happen on the interpreter thread. Requests run one at a time, the
same order SQLite would impose on writes.

**Routing.** Each `Route` becomes an axum route, with `:id:Int` turned into `{id}`. A
handler captures its route index. Unknown paths answer 404 and wrong methods 405, both
from axum. The server binds to `127.0.0.1`.

**The request record.**

- Path and query params sit at the top level.
- `body: {…}` is present when the route declares a body.
- `time` is the request's UTC time as text, `2026-10-06T21:03:00Z` (std only, no date crate).

One `decode(col, raw) -> Result<Value, String>` serves path segments, query values and
body fields:

| Type | Accepts |
| --- | --- |
| `Int` | an integral number |
| `Num` | a number |
| `Text` | text |
| `Bool` | `true` / `false` |
| `Const` | text, made a constant |
| `Json` | any JSON value, via `value::from_json` |

A body that isn't a JSON object, a missing required field or a value of the wrong type
answers 400 `{"error": "id must be Int"}` without running the word.

**Running.** There is no `Session`, since no history is needed:

1. `machine::initial(p, word, subj)`, which today always starts from an empty record and
   gains the `subj` argument; `Session::new` passes an empty one.
2. Call `step` until the status isn't `Ready`, within a step budget (200,000, the same as
   `naga run`).
3. Print `say` output to stdout, prefixed by `VERB path`. The store's SQL log is cleared
   after each request.

**The response.**

- `status` must be an integer from 100 to 599 and `json` must be present; otherwise 500.
- `json` is encoded by `to_json` with constants written as plain strings (a flag on the
  encoder; `Json` columns keep `{"$sym"}`).
- A 204 has an empty body.
- A runtime error, a hole or a spent budget answers 500 `{"error": msg}`.

**The store.** `Store::open(p, path)` opens `app.db` beside the source file and creates
every table with `CREATE TABLE IF NOT EXISTS`. Seeds go only into tables it just created.
`Store::new` (in-memory, `naga run`) stays. Changing the schema of an existing database is
out of scope; SQLite's error surfaces at startup.

## 3. Checker

`Checker::check` applies `main` to an empty closed shape (`check/mod.rs`, "the run starts
from an empty record"). That becomes one `call_site(shape, word, label, line)` with two
callers:

- **The entry:** the empty record, labelled `main`. Output is unchanged.
- **Each route:** the closed shape it declares. Params get `ty::col_type` with their `?`,
  `body` gets `rec_of(cols)`, and `time` is `Text`. Then every live output world must have
  `status` and `json` as required fields. When one doesn't, the error is reported at the
  route's line:

```
✗ 3:0 route GET /accounts/:id: get-account can finish without status
```

Errors inside the word end with `(when route GET /accounts/:id runs)`, in the same place
`(when main runs)` goes today.

## 4. Examples

`examples/ledger.nag` and `customers.nag` stay as they are, to keep parity.
`examples/ledger-server.nag` is the ledger with a `route` line for each of its 11
endpoints. Its 422 fallbacks for missing fields become dead code, because the router
answers 400 first. They are dropped there.

## 5. Tests

- The goldens and the fuzzer are untouched: no reference case contains `route`.
- `tests/http.rs` (naga only) covers:
  - parsing: the `Route` values, and every `check_refs` error above;
  - decoding: one 400 per type and for a missing field; undeclared keys dropped;
  - end to end: `serve::router(...)` with `tower::ServiceExt::oneshot` against
    `ledger-server.nag` on a temporary database: 200, 201, 204, 404 from the word, 404
    and 405 from axum, 422 from the word, 400 from decoding;
  - the store: seeding happens once, and a second `Store::open` keeps the rows;
  - the checker: a route whose word can leave without `status`, and a word needing a
    field its route doesn't give.

## Dependencies

`axum 0.8`, `tokio` (`rt-multi-thread`, `macros`, `net`, `sync`). For tests: `tower` and
`http-body-util`.

## Out of scope

Auth, middleware, response headers, hot reload, schema migration, concurrent
interpreters.
