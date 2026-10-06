const fs = require('fs');
const L = require("../lang.js");
const base = fs.readFileSync(require('path').join(__dirname, '../../examples/ledger.nag'), 'utf8');
const muts = [
  ['404 path forgets its status', 'account-found {-missing} = +{status: 404, json: "no such account"}', 'account-found {-missing} = +{json: "no such account"}'],
  ['typo in a column', '+{balance: sum(postings.amount)}', '+{balance: sum(postings.amnt)}'],
  ['^ points at a field that is not there', 'date: ^date}}', 'date: ^day}}'],
  ['id not cleared before reuse', 'with-payee = -{id} +{id: tx-id}', 'with-payee = +{id: tx-id}'],
  ['wrong number of ^', 'date ≥ ^^^from and date < ^^^to', 'date ≥ ^^from and date < ^^to'],
  ['kind-ok without a fallback', 'kind-ok                   = +{problem: "kind must be Asset, Liability, Income, Expense or Equity"}\n', ''],
  ['both sides of ‖ write income', '(@income( kind-flow ) ‖ @expenses( kind-flow ))', '(@income( kind-flow ) ‖ @income( kind-flow ))'],
  ['delete ignores the missing case', '@postings( postings.delete .{id} )', '@postings( postings.delete -{deleted} )'],
];
const CK = L.makeChecker();
for (const [label, from, to] of muts) {
  if (!base.includes(from)) { console.log('!! pattern not found:', label); continue; }
  const P = L.parseProgram(base.replace(from, to));
  const R = CK.check(P, 'main');
  console.log(`\n-- ${label}  (rechecked ${R.stats.recomputed.length}/${R.stats.recomputed.length + R.stats.reused.length})`);
  if (!R.errors.length) console.log('  (no errors)');
  R.errors.slice(0, 3).forEach(e => console.log(`  ✗ line ${e.line}: ${e.msg.slice(0, 220)}`));
}
