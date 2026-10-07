# Errors that explain the flow

naga's checker follows every path a record can take. Its errors should show those paths:
which step split them, what each branch looks like, and which branch fails and why. They
appear inline in the editor, with the code they name highlighted in matching colours.

## Today

Every error leaves the checker as a flat string with a position. Worlds don't remember how
they were split, so "why is `note` missing here" is lost by the time `complete` fails.

## Worlds remember their splits

A world carries a trail of the splits that made it. An entry records:
- **where** the split happened;
- **what** split: `notes.get`;
- **its branches**, each a label with the fields it leaves: "finds a row → note", "finds no
  row → missing";
- **which branch** this world took.

Splits are recorded where alternatives are made: table get, update and delete first, then
(slice 3) a callee's cases and optional fields. Merging identical worlds keeps one trail,
since any one path is a valid explanation. Signatures don't carry trails, so the memo and
incremental rechecks are unaffected.

## Failures are merged per split

The checker already runs a statement once per world. When some worlds fail at a statement
and their siblings from the same split don't, those failures become one diagnostic listing
every branch with its real outcome: ✓ if the step takes that world, ✗ with the reason if not.
✓ claims only "this step takes it", nothing about later steps.

```
NOTES.GET MAY FIND NO ROW, AND THEN COMPLETE NEEDS NOTE              app.nag:33
  33 │ complete-note = notes.get complete reply
  [notes.get] may or may not find a row with that id:
     finds a row   {..r, id, note}      → [complete] takes it ✓
     finds no row  {..r, id, missing}   → [complete] needs note ✗
  complete has no clause for that. Add one:
      complete {-missing} = ? when there is no note
```

The fix proposes a clause with a hole: naga's own way of saying "decide this".

## Diagnostics

```rust
Diagnostic { title, at, lines: Vec<Vec<Part>>, fix: Option<String>, refs: Vec<Ref> }
Part = Text | Ref(i) | Shape | Ok | No        Ref = { span, label }
```

Every other error is wrapped as a plain diagnostic (its title is the old message), so
nothing regresses. Rich kinds are added slice by slice.

## Rendering

- **Editor:** an inline block under the error's line, expanded while the cursor is on that
  line and folded to its title otherwise. Each ref has a colour: a chip in the message and
  the same colour behind its code.
- **Terminal:** `naga run` prints the Elm-style text when it writes to a terminal.
- **Goldens:** one line per error, `✗ line:col title`, so plain errors keep their goldens.

## Slices

1. The plumbing: the `Diagnostic` type through the checker, report and editor, the inline
   block with coloured refs, terminal output. Trails at table-op splits, and failures
   merged per split.
2. More kinds: "no clause matches" (what each clause wants), "already there", "might be
   missing", table rows and columns.
3. Trails through a callee's cases and optional-field splits.
4. Type mismatches and the rare kinds.
