const L = require("../lang.js");
const EX = require("../lang-examples.js");
const { DatabaseSync } = require('node:sqlite');
const nodeDriver = () => { const db = new DatabaseSync(':memory:'); return {
  all: (sql, ps) => db.prepare(sql).all(...ps).map(o => ({ ...o })),
  run: (sql, ps) => { const r = db.prepare(sql).run(...ps); return { changes: Number(r.changes), lastId: Number(r.lastInsertRowid) }; },
  close: () => db.close() }; };
const engines = { sql: { kind: 'sql', driver: nodeDriver, label: 'node sqlite' }, mem: { kind: 'memory' } };
function run(src, eng, quiet) {
  const P = L.parseProgram(src);
  if (P.errors.length) console.log('  parse errors', P.errors);
  const S = new L.Session(P, P.words.has('main') ? 'main' : P.order[0], engines[eng]);
  S.run('continue', new Set());
  const st = S.state;
  return { P, S, st, out: S.log.slice(0, st.outLen).map(l => l.text), err: st.error && st.error.msg };
}
const todos = EX.find(e => e.id === 'todos').src;
for (const eng of ['sql', 'mem']) {
  const r = run(todos, eng);
  console.log(`\n== ${eng}: ${r.st.status} ${r.err || ''}`);
  console.log(r.out.join('\n'));
  console.log('rows:', r.st.db.todos.map(x => L.show(x)).join(' '));
  console.log('log:'); r.S.store.log.forEach(e => console.log(`  [${e.tick}] ${e.text}`));
}
// errors and edges
const base = `table todos :: {title: Text, done: Bool, tags, status: Const}\nseed todos = [{title: "a", done: false, tags: [1, "x"], status: Open}]\n`;
for (const [label, body] of [
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
]) {
  for (const eng of ['sql', 'mem']) {
    const r = run(base + body, eng);
    console.log(`${label} [${eng}] →`, r.P.errors.map(e => e.msg).join(' | ') || '', r.st.status, r.err || '', r.out.join(' / '));
  }
}
// time travel: snapshots differ per step
const r = run(todos, 'sql');
const S = r.S; const counts = S.hist.map(h => h.db.todos.length);
console.log('row counts over time:', [...new Set(counts.map((c, i) => i === 0 || c !== counts[i - 1] ? `${i}:${c}` : null).filter(Boolean))].join(' '));
console.log('preview where:', L.previewSql(r.P, r.P.words.get('list-open').clauses[0].body[0]));
