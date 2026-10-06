const L = require("../lang.js");
const EX = require("../lang-examples.js");
const CK = L.makeChecker();
const check = src => { const P = L.parseProgram(src); return { P, R: CK.check(P, P.words.has('main') ? 'main' : P.order[0]) }; };
const cases = {
  'two call sites, one missing id': 'table todos :: {title: Text, done: Bool}\nmain = (+{id: 2} finish) finish\nfinish = +{changes: {done: true}} todos.update finished -{id}\nfinished {-missing} = nop\nfinished {-todo} = nop',
  'no fallback': EX.find(e => e.id === 'invoice').src.replace('settle {-declined}                       = void\n', ''),
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
for (const [label, src] of Object.entries(cases)) {
  const { P, R } = check(src);
  console.log(`\n-- ${label}`);
  if (!R.errors.length) console.log('  no errors');
  R.errors.forEach(e => console.log(`  ✗ line ${e.line}: ${e.msg}`));
}
// signatures shared across call sites
const { R: R1 } = check(cases['two call sites, one missing id']);
console.log('\nfinish sig:', JSON.stringify(R1.signatures.get('finish')));

// incrementality
console.log('\n== incremental');
const inv = EX.find(e => e.id === 'invoice').src;
const C2 = L.makeChecker();
const go = (label, src) => { const P = L.parseProgram(src); const R = C2.check(P, 'main'); console.log(`${label}: recomputed [${R.stats.recomputed.join(', ')}] reused ${R.stats.reused.length} · errors ${R.errors.length}`); return R; };
go('first run', inv);
go('same text', inv);
go('edit report message', inv.replace('voided"', 'was voided"'));
go('edit fail body, same signature', inv.replace('fail = tries+1', 'fail = tries+2'));
go('add a blank line at the top', '\n' + inv);
go('change finalize signature', inv.replace('finalize = +{due: now + 30, tries: 0}', 'finalize = +{due: now + 30, tries: 0, note: "x"}'));
