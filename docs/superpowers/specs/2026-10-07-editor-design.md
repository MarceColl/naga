# The naga editor

A GPU-rendered port of the "Forthy Holes" editor (reference: `~/Downloads/Forthy Holes.html`,
whose engine is `spec/lang.js`): one screen of colorForth-style code, steps as the unit of
selection, typed keyword operations, holes filled by hand or by Claude, runs checked on every
edit, a debugger and a data view.

`naga-edit file.nag` (its own binary, so the GPU stack stays out of `naga`) opens a file and writes it on every successful edit. `--shot out.png` draws one frame offscreen after replaying `--keys`, `--click` and `--scroll`.

## Runs: route requests declared in the program

```
run "POST, then GET it back"
  POST /transactions {date: "2026-10-02", payee: "Mercadona", postings: [...]} → 201
  GET  /transactions/4 → 200 {id: 4, date: "2026-10-02", payee: "Mercadona", lines: [...]}
```

- A run is a name plus request lines: `VERB /path?query [body] → status [json]`. The body
  and the expected json are naga literals; the body goes through the router as JSON.
- Each run starts from a fresh in-memory database holding the seeds. Its requests share it,
  in order, and go through `serve`'s own path (decode → word → reply).
- The verdict:
  - **pass:** every status matches, and every json given matches (key order ignored);
  - **fail:** a status or json differs;
  - **error:** a 500 or a runtime error;
  - **waits:** a request hit a hole.
- `naga run` prints each run's verdict.

## Shape

```
naga lib ── analyze(text) ─▶ Analysis { program, steps + ranges, holes, errors, before/after shapes, runs }
editor/ (own crate; GPU dependencies stay out of naga)
  core   pure text ops over an Analysis: extract inline hole drop swap rename edit fill → Result<String, String>
  state  State + update(State, Event) → State: mode, selection, hover, command line, edit buffer, undo (texts), debug, data view
  view   view(&State) → Frame { coloured runs per line, rects, underlines, bar }   (pure, unit-tested)
  gpu    winit events → Event; Frame → one quad pass + one glyphon text pass (wgpu)
```

- **The program text is the only state.** Everything else is recomputed from it, and undo is
  a stack of texts.
- **Look:** the reference's palette, uppercase text and weights 500/700/800, in Martian Mono
  Wide (static TTFs, OFL, bundled).
- **The agent** asks the Claude API (`ANTHROPIC_API_KEY`) for candidates on a background
  thread. Its loop is the reference's: write a candidate; keep it if it adds no checker error
  and breaks no run; otherwise retry with the reason.

## What the engine gains

These were dropped as playground-only during the port:

1. **Statement end positions** (`end: Pos`), for highlighting, selection and text edits.
2. **The checker's shapes before and after each statement**, for the status bar.
3. **Table snapshots** (all rows of every table), for the data view: the seeded rows and the
   rows at the end of a run.
4. **`run` declarations and their runner.**

## Slices

0. Engine: end positions, before/after shapes, snapshots, runs.
1. Window and view: rendering, colours, folded seeds, holes, hover, step selection and
   navigation, the status bar.
2. Commands: the command line, the seven text ops, in-place editing, filling holes, undo,
   saving.
3. Runs and the data view.
4. Debugger.
5. Agent: filling holes and `rework` through Claude.
