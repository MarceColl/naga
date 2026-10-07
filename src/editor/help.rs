//! The cheat sheet `?` opens. `# ` lines are headings, lines indented two spaces are naga
//! (coloured like code), with an optional note after a tab; the rest is prose.

pub const TEXT: &str = r#"# The idea
A record flows through words. Each word takes the record and gives one back; words written
one after another run left to right. Names ignore case: GET-NOTE is get-note.
  main = +{n: 3} countdown

# Words and clauses
A word can have several clauses. The first whose pattern and guard match runs.
  countdown {n: 0} = say "liftoff"
  countdown        = say "{n}…" n-1 countdown
  name {pattern} when guard = steps
Patterns look at the record:
  {f}	f is there
  {-f}	f is there, and the clause takes it out
  {!f}	f isn't there  (also {¬f})
  {f: :asset}	f is that constant
  {f: {g}}	f is a record with g

# Steps
  +{f: expr}	add f; it must not be there yet
  -{f, g}	remove fields
  .{f g}	keep only these fields
  f=expr	set f
  n+1  n-1	bump a number
  a->b	rename a to b  (a→b)
  //{f: v}	override fields  (⫽)
  \/{f: v}	fill fields that aren't there  (⊔)
  unwrap f	lift f's fields up  (⊃f)
  wrap f{a b}	pack fields a b into f  (⊂f{a b})
  @f( steps )	run steps inside record f, or on every item of list f
  ( a || b )	both on the same record, then merge what they add  (‖)
  say "text {f}"	print
  show	print the whole record
  raise expr	stop with an error
  break	pause the debugger
  nop	do nothing
  ? note	a hole: work still to do

# Expressions
  + - * / %   == != < > <= >=   and or not   ++
++ joins lists and text.
  r.f	a field
  xs.f	on a list: each item's f, as a list
  r.{a b}	only those fields
  [a, b]   {a: 1, b: x}	a list, a record
  ^x   ^^x	x from the record one @focus out, two out
  "total: {sum(lines.price)}"	text with expressions inside
  a diff b	the change from a to b (∆)
  r apply d	apply a change (⍎)
  a \/ b	join: both records' fields (⊔)
  a /\ b	meet: the fields they share (⊓)
  a <: b	a is within b (⊑)
  a // b	a with b's fields over it (⫽)
  :asset	a constant: a name used as a value (any case: :Asset is :asset)
Functions: len sum min max abs round floor ceil head tail last push reverse range str keys

# Tables
  table accounts :: {name: Text, kind: Const, memo: Text?}
  seed accounts = [{name: "Cash", kind: :asset}]
Types: Int Num Text Bool Const Json, with ? for optional. Every row also has an id.
The row field is the table's name in the singular: accounts → account.
  accounts.all	all rows, in `accounts`
  accounts.get	by `id`: leaves `account`, or `missing`
  accounts.where {kind: :asset} when balance > 0	matching rows, in `accounts`
  accounts.insert	stores `account` and sets its id
  accounts.update	`id` and `changes`: leaves `account`, or `missing`
  accounts.delete	by `id`: leaves `deleted`, or `missing`
In a where, columns are bare and the record is ^x.

# Routes
  route GET    /accounts/:id:Int                     = get-account
  route POST   /accounts {name: Text, kind: Const}   = post-account
  route GET    /reports?from:Text&to:Text?           = get-report
Path and query params land at the top level, the JSON body in `body`, the time in `time`.
Nested bodies work: {lines: [{sku: Const, qty: Int}]}. A bad request gets a 400 before the
word runs. The word leaves {status, json}. A Const field takes a JSON string ("Asset" → :asset)
and goes back out as one ("asset").

# Live pages
  live /counter = counter
The word runs on the page's state, with `time`: on mount the state is the params; on a click
it also has event: {name: :inc}. It leaves `html`; the rest is the next state.
  [:button, {on-click: :inc}, "+"]        an element: [:tag, {attrs}?, child…]
  [:form, {on-submit: :add}, …]            event: {name: :add, form: {…fields}}
  [:input, {on-input: :typed}]             event: {name: :typed, value}
Pages refresh (no event) after a table write and after each change to the program.

# Runs
Requests the program must answer, checked on every change.
  run "add one, then read it back"
    POST /notes {title: "Ship it"} → 201 {id: 3, title: "Ship it", done: false}
    GET /notes/3 → 200
  run "counts"
    LIVE /counter → {count: 0}            a page's state has at least these fields
    EVENT {name: :inc} → {count: 1}

# Sections
  * Accounts          ** Reading
Tab folds or opens the section under the cursor, Shift-Tab cycles the whole outline.

# The editor
Normal:   h j k l   w b e   0 ^ $   gg G   12G      move
          i a I A o O        write              v V   select
          x dd yy cc D C p P   cut, copy, paste      u  ⌃R   undo, redo
          Tab Shift-Tab za zM zR   fold             [[ ]]   between headings
          enter on a hole: write it                 ga on a hole: hand it to the agent
          ]e [e   next, previous error              ?   this sheet
Commands: :extract name  :inline  :hole note  :rework  :drop  :swap  :rename name  :fix
          :data [table]  :debug n  :migrate  :reset table  :help  :w  :q
They act on the selected steps, or the step under the cursor.
Data:     h l   live, seed, runs       j k   tables          esc   back
Debugger: l into   j over   k out   h back   enter continue   0 restart
          click a line for a breakpoint      1–9 another run      esc   back
It's live: as soon as the program parses, the server runs it and the file holds it.
"#;
