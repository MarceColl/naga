// Reference report: the text `naga run <file>` must print, byte for byte.
// node spec/report.js <file.nag>    → prints the report
// node spec/report.js --golden      → regenerates tests/cases/*.nag and tests/golden/*.out
const fs = require('fs');
const path = require('path');
const L = require('./lang.js');
const { DatabaseSync } = require('node:sqlite');

const driver = () => {
  const db = new DatabaseSync(':memory:');
  return {
    all: (sql, ps) => db.prepare(sql).all(...ps).map(o => ({ ...o })),
    run: (sql, ps) => { const r = db.prepare(sql).run(...ps); return { changes: Number(r.changes), lastId: Number(r.lastInsertRowid) }; },
    close: () => db.close(),
  };
};

function report(src) {
  const out = [];
  const P = L.parseProgram(src);
  P.errors.forEach(e => out.push(`parse ${e.line}:${e.col} ${e.msg}`));
  P.warnings.forEach(w => out.push(`warn ${w.line} ${w.msg}`));
  P.holes.forEach(h => out.push(`hole ${h.line} ${h.word}: ${h.note}`));
  const entry = P.words.has('main') ? 'main' : P.order[0];
  if (entry) {
    const S = new L.Session(P, entry, { kind: 'sql', driver });
    S.run('continue', new Set());
    const st = S.state;
    out.push(`run ${st.status} after ${S.index} steps` + (st.error ? `: ${st.error.msg} @${st.error.pos.line}` : ''));
    (st.error && st.error.details || []).forEach(d => out.push(`  why ${d}`));
    if (st.hole) out.push(`  hole ${st.hole.note}`);
    S.log.slice(0, st.outLen).forEach(l => out.push(`> ${l.text}`));
    out.push(`subj ${L.show(st.subj)}`);
    out.push(`pause ${S.pause.reason}`);
    if (S.store) S.store.log.forEach(l => out.push(`sql [${l.tick}] ${l.text}`));
    S.dispose();
  }
  const R = L.makeChecker().check(P, entry);
  out.push(`check ${R.errors.length} errors`);
  R.errors.forEach(e => out.push(`  ✗ ${e.line}:${e.col} ${e.msg}`));
  for (const n of P.order) {
    const s = R.signatures.get(n) || [];
    if (!s.length) out.push(`sig ${n} : (none)`);
    s.forEach(c => out.push(`sig ${n} : ${c.in} → ${c.fail ? '✗ ' + c.fail : c.out.join(' | ') || 'never returns'}${c.guard ? '   (guarded)' : ''}`));
  }
  return out.join('\n') + '\n';
}

// Stepping and time travel on the invoice sheet (from machine.test.js).
function stepping(src) {
  const out = [];
  const P = L.parseProgram(src);
  const S = new L.Session(P, 'main');
  const desc = s => {
    const n = L.nextStmt(s);
    return `${s.status} lvl${L.topLvl(s)} next=${n ? n.k + (n.name ? ':' + n.name : '') + '@' + n.pos.line : '-'} ctx=${L.contextOf(s.k).join('>')} at=${S.index}`;
  };
  const plan = [['into'], ['into'], ['into'], ['into'], ['over'], ['into'], ['into'], ['into'], ['out'], ['over'], ['out'],
    ['restart'], ['continue', [21]], ['continue', [21]], ['into'], ['out'], ['continue', [12, 25]], ['continue', [12, 25]], ['continue']];
  for (const [mode, bps] of plan) {
    if (mode === 'restart') { S.restart(); out.push(`restart ${desc(S.state)}`); continue; }
    S.run(mode, new Set(bps || []));
    out.push(`${mode}${bps ? ' bp' + bps : ''} ${desc(S.state)} pause=${S.pause.reason}`);
  }
  S.seek(10); out.push(`seek ${desc(S.state)}`);
  S.back(); out.push(`back ${desc(S.state)}`);
  S.restart(); out.push(`restart ${desc(S.state)}`);
  S.run('continue', new Set()); out.push(`again ${desc(S.state)}`);
  return out.join('\n') + '\n';
}

// Salsa reuse: what gets rechecked after each edit (from checker-errors.test.js).
function incremental(inv) {
  const C = L.makeChecker();
  const out = [];
  const go = (label, src) => {
    const R = C.check(L.parseProgram(src), 'main');
    out.push(`${label}: recomputed [${R.stats.recomputed.join(', ')}] reused ${R.stats.reused.length} · errors ${R.errors.length}`);
  };
  go('first run', inv);
  go('same text', inv);
  go('edit report message', inv.replace('voided"', 'was voided"'));
  go('edit fail body, same signature', inv.replace('fail = tries+1', 'fail = tries+2'));
  go('add a blank line at the top', '\n' + inv);
  go('change finalize signature', inv.replace('finalize = +{due: now + 30, tries: 0}', 'finalize = +{due: now + 30, tries: 0, note: "x"}'));
  go('back to the start', inv);
  return out.join('\n') + '\n';
}

function cases() {
  const root = path.join(__dirname, '..');
  const ex = n => fs.readFileSync(path.join(root, 'examples', n + '.nag'), 'utf8');
  const cs = [];
  const add = (group, label, src) => cs.push([`${group}-${label.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '')}`, src]);
  const mutate = (group, base, muts) => muts.forEach(([label, from, to]) => {
    if (!base.includes(from)) throw new Error('pattern not found: ' + label);
    add(group, label, base.replace(from, to));
  });
  for (const f of fs.readdirSync(path.join(root, 'examples')).sort()) add('example', f.replace(/\.nag$/, ''), ex(f.replace(/\.nag$/, '')));

  ['main = +{a: 1} +{a: 2}', 'main = foo', 'main = +{x: "a"} +{y: x + 1}', 'main = (+{a: 1} ‖ +{a: 2})', 'main = +{n: 1} f\nf {n: 2} = nop\nf {m} = nop',
    'f = \nmain = say "hi {', 'main = +{xs: [1,2,3]} @xs( it=it*2 ) show', 'main = +{c: {v: 1}, k: 5} @c( v=v + ^k ) show', 'main = ?  decide later',
    'main = say 1 ++ 2 break say "after"', 'g = nop\ng {a} = nop\nmain = g',
    'main = say 0.1 + 0.2 say 1 / 3', 'main = say round(2.5) say round(-2.5) say round(1.005, 2) say 10 % 3 say -7 % 3',
    'main = say 1000000 * 1000000 * 1000000 say 1000000 * 1000000 * 1000000 * 1000 say 0.000001 say 0.0000001 say 123456789.123456789',
    'main = say len("héllo 😀") say str([1, "a", {b: Nil}]) say keys({z: 1, a: 2}) say range(3) say range(1.5, 4)',
    'main = +{r: {a: 1, b: 2}} r=(r ⫽ {a: 3}) +{d: {a: 1, b: 2} ∆ {b: 3, c: 4}} show say {a: 1} ⊑ {a: 1, b: 2} say {a: 1} ⊓ {a: 2, b: 1}',
    'main = +{a: 1} -{a} +{a: 2} show',
  ].forEach((src, i) => add('machine', String(i + 1).padStart(2, '0'), src));

  const inv = ex('invoice');
  const checks = {
    'two call sites, one missing id': 'table todos :: {title: Text, done: Bool}\nmain = (+{id: 2} finish) finish\nfinish = +{changes: {done: true}} todos.update finished -{id}\nfinished {-missing} = nop\nfinished {-todo} = nop',
    'no fallback': inv.replace('settle {-declined}                       = void\n', ''),
    'par conflict': 'main = +{total: 1} (+{tax: 1} ‖ (+{x: 1} tax-again))\ntax-again = +{tax: 2}',
    'text plus num': 'main = +{x: "a"} +{y: x + 1}',
    'type flows into callee': 'main = +{tries: "many"} fail\nfail = tries+1',
    'double add': 'main = +{a: 1} +{a: 2}',
    'callee adds existing': 'main = +{paidAt: 1, due: 1, tries: 1} paid\npaid = -{due, tries} +{paidAt: now}',
    'loop adds field': 'main = loop\nloop = +{step: 1} loop',
    'optional column': 'table notes :: {body: Text, tag: Text?}\nmain = notes.all @notes( say "{tag}" )',
    'optional after match': 'table notes :: {body: Text, tag: Text?}\nmain = notes.all @notes( show-tag )\nshow-tag {tag} = say "{tag}"\nshow-tag = nop',
    'insert missing required': 'table todos :: {title: Text, done: Bool}\nmain = +{todo: {title: "x"}} todos.insert',
    'insert wrong type': 'table todos :: {title: Text, done: Bool}\nmain = +{todo: {title: "x", done: "no"}} todos.insert',
    'get union unhandled': 'table todos :: {title: Text, done: Bool}\nmain = +{id: 1} todos.get say "{todo.title}"',
    'const narrowing': 'main = +{s: Open} f g\nf {s: Open} = s=Done\nf {s: Done} = nop\ng {s: Done} = say "done"',
    'const not covered': 'main = +{s: Open} g\ng {s: Done} = nop',
    'outer missing': 'main = +{c: {v: 1}} @c( v=v + ^k )',
    'focus on number': 'main = +{n: 1} @n( nop )',
    'list of plain': 'main = +{xs: [1, 2]} @xs( it=it * 2 ) +{t: sum(xs)}',
    'guard missing field is fine': 'main = +{a: 1} f\nf when b > 1 = nop\nf = nop',
    'nested requirement through callee': 'main = +{card: {kind: Visa}} charge\ncharge = @card( balance-1 )',
    'unused word still checked': 'main = nop\nbroken = +{a: 1} -{b}',
  };
  for (const [label, src] of Object.entries(checks)) add('check', label, src);

  const algebra = [
    ['rename', 'main = +{a: 1, b: "x"} go\ngo = a→c say c + 1'],
    ['rename onto existing', 'main = +{a: 1, b: 2} b→a'],
    ['wrap/unwrap', 'main = +{a: 1, b: 2, c: 3} ⊂t{a b} say t.a + c ⊃t say a'],
    ['wrap missing field', 'main = +{a: 1} ⊂t{a b}'],
    ['unwrap collision', 'main = +{t: {a: 1}, a: 2} ⊃t'],
    ['unwrap open body', 'main = +{body: {name: "x", kind: Asset}} post\npost {body: {name, kind}} = ⊃body say name'],
    ['expr ops', 'main = +{x: {a: 1, b: 2}, y: {b: "s", c: 4}} +{m: x ⫽ y, n: x ⊓ y, d: x ∆ y} +{z: x ⍎ d} say z.c + 1 say m.b ++ "!"'],
    ['expr ⊔ overlap', 'main = +{x: {a: 1}, y: {a: 2}} +{u: x ⊔ y}'],
    ['expr ⊔ disjoint', 'main = +{x: {a: 1}, y: {b: 2}} +{u: x ⊔ y} say u.b'],
    ['unify step', 'main = +{auth: {cid: 1}, body: {cid: 2}} go\ngo = @body( ⊔{cid: ^auth.cid} ) after\nafter {body: {conflict}} = say "conflict"\nafter = say body.cid'],
    ['projection', 'main = +{x: {a: 1}} say x.{a b} say x.{a}.a + 1 say [x].{a}'],
    ['update with delta', 'table things :: {name: Text, n: Int, note: Text?}\nmain = +{id: 1} things.get go\ngo {missing} = nop\ngo {thing} = +{changes: thing ∆ (thing ⫽ {n: 3})} -{thing} things.update'],
    ['update with bad delta', 'table things :: {name: Text, n: Int, note: Text?}\nmain = +{id: 1} things.get go\ngo {missing} = nop\ngo {thing} = +{changes: thing ∆ (thing ⫽ {n: "three"})} -{thing} things.update'],
    ['update deleting required', 'table things :: {name: Text, n: Int, note: Text?}\nmain = +{id: 1} things.get go\ngo {missing} = nop\ngo {thing} = +{changes: thing ∆ thing.{id note}} -{thing} things.update'],
  ];
  algebra.forEach(([label, src]) => add('algebra', label, src));

  mutate('cmut', ex('customers'), [
    ['wrap before state exists', 'state: Active} ⊂customer{name email plan state}', 'state: Active} ⊂customer{name email plan state tier}'],
    ['drop the 403 clause', "owned {body: {conflict}} = +{status: 403, json: \"that customer isn't you\"} reply\n", ''],
    ['⊔ as an expression on overlapping records', '@body( ⊔{customer-id: ^auth.customer-id} ) owned', '+{merged: body ⊔ auth} owned'],
    ['rename onto a field that exists', 'owned = body→event', 'owned = +{event: 1} body→event'],
    ['unwrap into a record that already has the field', 'post-customer {body: {name, email, plan}} =\n  ⊃body', 'post-customer {body: {name, email, plan}} =\n  +{plan: Free} ⊃body'],
    ['change set with a wrongly typed value', 'customer ⫽ body.{name email plan state}', 'customer ⫽ {plan: "Pro"}'],
    ['change set that deletes a required column', 'customer ∆ (customer ⫽ body.{name email plan state})', 'customer ∆ customer.{id name email plan}'],
  ]);
  mutate('lmut', ex('ledger'), [
    ['404 path forgets its status', 'account-found {-missing} = +{status: 404, json: "no such account"}', 'account-found {-missing} = +{json: "no such account"}'],
    ['typo in a column', '+{balance: sum(postings.amount)}', '+{balance: sum(postings.amnt)}'],
    ['^ points at a field that is not there', 'date: ^date}}', 'date: ^day}}'],
    ['id not cleared before reuse', 'with-payee = -{id} +{id: tx-id}', 'with-payee = +{id: tx-id}'],
    ['wrong number of ^', 'date ≥ ^^^from and date < ^^^to', 'date ≥ ^^from and date < ^^to'],
    ['kind-ok without a fallback', 'kind-ok                   = +{problem: "kind must be Asset, Liability, Income, Expense or Equity"}\n', ''],
    ['both sides of ‖ write income', '(@income( kind-flow ) ‖ @expenses( kind-flow ))', '(@income( kind-flow ) ‖ @income( kind-flow ))'],
    ['delete ignores the missing case', '@postings( postings.delete .{id} )', '@postings( postings.delete -{deleted} )'],
  ]);

  const base = 'table todos :: {title: Text, done: Bool, tags, status: Const}\nseed todos = [{title: "a", done: false, tags: [1, "x"], status: Open}]\n';
  [
    ['type error', 'main = +{todo: {title: 5}} todos.insert'],
    ['unknown column', 'main = +{todo: {color: Red}} todos.insert'],
    ['json+const roundtrip', 'main = +{id: 1} todos.get show'],
    ['where ^ param', 'main = +{want: Open} todos.where when status == ^want show'],
    ['where nil/absent', 'main = +{todo: {title: "b"}} todos.insert -{todo} todos.where {!done} show'],
    ['update to nil removes', 'main = +{id: 1, changes: {tags: nil}} todos.update show'],
    ['strict field', 'main = todos.all todos.all'],
    ['bad op', 'main = todos.frob'],
    ['bad where col', 'main = todos.where {color: Red}'],
    ['bad where fn', 'main = todos.where when head(title) == "a"'],
    ['no table', 'main = users.all'],
    ['seed error', 'seed todos = [{title: 1}]\nmain = nop'],
    ['division real', 'main = todos.where when (len(title) / 2) > 0.4 show'],
    ['insert full row', 'main = +{todo: {title: "q\'s", done: true, tags: {n: 1.5, deep: [Open, nil]}, status: Done}} todos.insert show'],
    ['delete twice', 'main = +{id: 1} todos.delete -{deleted} todos.delete show'],
    ['update id missing', 'main = +{id: 7, changes: {done: true}} todos.update show'],
    ['insert explicit id clash', 'main = +{todo: {id: 1, title: "x", done: true, status: A}} todos.insert'],
  ].forEach(([label, body]) => add('table', label, base + body));
  return cs;
}

if (process.argv[2] === '--golden') {
  const root = path.join(__dirname, '..', 'tests');
  for (const d of ['cases', 'golden']) { fs.rmSync(path.join(root, d), { recursive: true, force: true }); fs.mkdirSync(path.join(root, d), { recursive: true }); }
  const cs = cases();
  for (const [name, src] of cs) {
    fs.writeFileSync(path.join(root, 'cases', name + '.nag'), src);
    fs.writeFileSync(path.join(root, 'golden', name + '.out'), report(src));
  }
  const inv = fs.readFileSync(path.join(__dirname, '..', 'examples', 'invoice.nag'), 'utf8');
  fs.writeFileSync(path.join(root, 'golden', 'stepping.out'), stepping(inv));
  fs.writeFileSync(path.join(root, 'golden', 'incremental.out'), incremental(inv));
  console.log(`${cs.length} cases + stepping + incremental`);
} else process.stdout.write(report(fs.readFileSync(process.argv[2], 'utf8')));
