const fs = require('fs');
const L = require("../lang.js");
const { DatabaseSync } = require('node:sqlite');
const nodeDriver = () => { const db = new DatabaseSync(':memory:'); return {
  all: (sql, ps) => db.prepare(sql).all(...ps).map(o => ({ ...o })),
  run: (sql, ps) => { const r = db.prepare(sql).run(...ps); return { changes: Number(r.changes), lastId: Number(r.lastInsertRowid) }; },
  close: () => db.close() }; };
const src = fs.readFileSync(process.argv[2] || require('path').join(__dirname, '../../examples/ledger.nag'), 'utf8');
const P = L.parseProgram(src);
console.log('parse errors:', P.errors, 'warnings:', P.warnings.map(w => w.line + ' ' + w.msg));
for (const eng of [{ kind: 'sql', driver: nodeDriver, label: 'sqlite' }, { kind: 'memory' }]) {
  const S = new L.Session(P, 'main', eng);
  S.run('continue', new Set());
  const st = S.state;
  console.log(`\n== ${eng.kind}: ${st.status} after ${st.tick} steps ${st.error ? '— ' + st.error.msg + ' @' + st.error.pos.line + ' ' + JSON.stringify(st.error.details) : ''}`);
  S.log.slice(0, st.outLen).forEach(l => console.log('  ' + l.text));
  if (eng.kind === 'sql') console.log('  sql statements:', S.store.log.length);
}
const R = L.makeChecker().check(P, 'main');
console.log(`\n== checker: ${R.errors.length} errors, ${R.stats.recomputed.length} words`);
R.errors.forEach(e => console.log(`  ✗ line ${e.line}: ${e.msg}`));
if (process.argv[3]) for (const n of P.order) (R.signatures.get(n) || []).forEach(c => console.log(`   ${n} : ${c.in} → ${c.fail ? '✗ ' + c.fail : c.out.join(' | ') || 'never returns'}`));
