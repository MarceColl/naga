# naga as a live system

You go into a folder, run `naga init`, then `naga start`: the editor opens and the program
is already serving. You change it while it runs, Lisp-image style. One binary does it all.

```
naga init            app.nag (a starter: table, seed, routes, words, a run) + .gitignore for *.db
naga start [file]    the editor + the server on :4170, on one program
naga run | serve     as today
```

## The editor is the image

```
keystroke → text → Analysis { program, checked, runs }   ← the one program there is
                                │
HTTP request → axum catch-all ──┴→ the UI thread runs the route's word on Analysis.program
                                │   and the live store → reply
                                └→ the file is written behind it (persistence, not a consumer)
```

- **One parse, one program.** The server has no program of its own. naga's values are `Rc`,
  so they stay on the UI thread. axum hands each request to that thread through the
  event-loop proxy (the same path the agent's replies take) and waits for the answer.
- **A swap is just the analysis changing.** The next request runs on the new analysis.
  Requests and edits are serialized on the UI thread, so a swap is atomic.
- **The text goes live as you type**, whenever it parses. Checker errors don't block it.
- **Routes are matched by the program** (`Program::route_for`), not registered with axum, so
  adding a route needs no restart. `naga serve` uses the same catch-all and the same
  `serve::respond`.
- **A request runs on the UI thread**, so a slow one stalls a frame. The step budget bounds it.
- **The bar** shows the server (`LIVE :4170`) and the last request with its status.

## The data lives on

The live database is `app.db` beside the file. When the analysis changes:

- **Applied at once:** a new table (created and seeded), and a new optional column
  (`ALTER TABLE … ADD COLUMN`).
- **Pending:** anything that could lose or break data — a dropped column, a type change, a
  new required column. The program keeps the old table and the bar lists what's pending.
- **`:reset t`** drops table `t`, then recreates and reseeds it.

## Editing: text first, structure on top

- **Plain editing:** a cursor, typing, and selection with the keyboard and mouse.
- **Vim:**
  - Modes: normal, insert, visual.
  - Moving: `hjkl w b e 0 $ gg G`.
  - Editing: `x dd yy p`, the operators `d c y` with a motion, `i a o O A I`, `u` and `⌃R`.
  - Commands after `:`.
- **The structure overlay:**
  - The bar follows the step under the cursor (shapes, signature).
  - Holes stay red, and errors are underlined as you type.
- **The old commands** become `:extract name`, `:hole`, `:inline`, `:rename x`, `:rework`,
  `:data`, `:debug N` and `:reset t`. They work on the visual selection, widened to whole
  steps.
- **`ga`** on a hole hands it to the agent.
- **Undo** is per edit.

## Sections

- **Headings:** `*` and `**` lines at column 0. The parser skips them the way it skips
  comments; the fuzzer skips files that contain them.
- **Folding:**
  - Tab on a heading (normal mode) folds or opens that section.
  - Shift-Tab cycles the whole outline: everything folded, headings only, everything open.
  - `za`, `zM` and `zR` work too. In insert mode, Tab inserts two spaces.
- **A folded heading shows what it holds:** `* Transactions  … 9 words, 4 runs`.
- **Moving:** `[[` and `]]` jump between headings.

## Slices

1. **One binary.** The editor moves into the `naga` crate. `init`, `start`, the catch-all
   server, requests run on the editor's program, and the live segment in the bar.
2. **Migrations and `:reset`.**
3. **The text editor:** the buffer, the cursor, plain editing, the live analysis as you type,
   the overlay following the cursor, and writing the file.
4. **Vim mode.**
5. **Sections and folding.**
6. **The old commands as `:` commands, plus `ga`.**

To measure in slice 3: the cost of a full analysis (check plus every run) on each keystroke.
If it grows, the runs move to a debounce while parse and check stay immediate.
