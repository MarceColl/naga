const L = require("../lang.js");
const EX = require("../lang-examples.js");
for (const ex of EX) {
  console.log('\n=== ' + ex.title);
  const P = L.parseProgram(ex.src);
  if (P.errors.length || P.warnings.length || P.holes.length) console.log('errors', P.errors, 'warnings', P.warnings, 'holes', P.holes);
  const S = new L.Session(P, P.words.has('main') ? 'main' : P.order[0]);
  S.run('continue', new Set());
  const st = S.state;
  console.log('status', st.status, 'steps', S.index, 'pause', S.pause);
  console.log('out:', S.log.slice(0, st.outLen).map(l => l.text));
  console.log('subj', L.show(st.subj));
  if (st.error) console.log('ERROR', st.error);
}
// stepping modes on invoice
const P = L.parseProgram(EX[1].src);
const S = new L.Session(P, 'main');
const desc = s => { const n = L.nextStmt(s); return `${s.status} lvl${L.topLvl(s)} next=${n ? n.k + (n.name ? ':' + n.name : '') + '@' + n.pos.line : '-'} ctx=${L.contextOf(s.k).join('>')}`; };
S.run('into', new Set()); console.log('into1', desc(S.state));
S.run('into', new Set()); console.log('into2', desc(S.state));
S.run('over', new Set()); console.log('over', desc(S.state));
S.run('into', new Set()); console.log('into', desc(S.state));
S.run('into', new Set()); console.log('into', desc(S.state));
S.run('out', new Set()); console.log('out', desc(S.state));
S.run('continue', new Set([22])); console.log('bp22', desc(S.state), S.pause);
console.log(L.explainCall(P, 'settle', { subj: S.state.subj, now: 0, outer: [] }));
S.back(); console.log('back', desc(S.state));
S.restart(); console.log('restart', desc(S.state), S.index);
// errors
for (const bad of ['main = +{a: 1} +{a: 2}', 'main = foo', 'main = +{x: "a"} +{y: x + 1}', 'main = (+{a: 1} ‖ +{a: 2})', 'main = +{n: 1} f\nf {n: 2} = nop\nf {m} = nop', 'f = \nmain = say "hi {', 'main = +{xs: [1,2,3]} @xs( it=it*2 ) show', 'main = +{c: {v: 1}, k: 5} @c( v=v + ^k ) show', 'main = ?  decide later', 'main = say 1 ++ 2 break say "after"', 'g = nop\ng {a} = nop\nmain = g']) {
  const P = L.parseProgram(bad);
  const S = new L.Session(P, P.words.has('main') ? 'main' : P.order[0]);
  if (P.words.size) S.run('continue', new Set());
  console.log(JSON.stringify(bad), '→', P.errors.map(e => e.line + ':' + e.msg).join(' | '), P.warnings.map(w => w.msg).join('|'), '|', S.state.status, S.state.error ? S.state.error.msg + ' ' + JSON.stringify(S.state.error.details) : '', S.log.map(l => l.text).join(' / '), S.state.hole ? 'hole:' + S.state.hole.note : '', S.pause.reason);
}
