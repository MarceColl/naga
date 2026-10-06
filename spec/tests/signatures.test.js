const L = require("../lang.js");
const EX = require("../lang-examples.js");
const CK = L.makeChecker();
const run = (src, entry = 'main', quiet) => {
  const P = L.parseProgram(src);
  const R = CK.check(P, P.words.has(entry) ? entry : P.order[0]);
  return { P, R };
};
const printSigs = (P, R) => {
  for (const n of P.order) {
    const s = R.signatures.get(n) || [];
    s.forEach((c, i) => console.log(`   ${i ? ' '.repeat(n.length) : n} : ${c.in} → ${c.fail ? '✗ ' + c.fail : c.out.join(' | ') || 'never returns'}${c.guard ? '   (guarded)' : ''}`));
    if (!s.length) console.log(`   ${n} : (no signature)`);
  }
};
for (const ex of EX) {
  const { P, R } = run(ex.src);
  console.log(`\n=== ${ex.title}: ${R.errors.length} errors`);
  R.errors.forEach(e => console.log(`  ✗ line ${e.line}: ${e.msg}`));
  printSigs(P, R);
}
