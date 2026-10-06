const fs = require('fs');
const L = require("../lang.js");
const base = fs.readFileSync(require('path').join(__dirname, '../../examples/customers.nag'), 'utf8');
const muts = [
  ['wrap before state exists', 'state: Active} ⊂customer{name email plan state}', 'state: Active} ⊂customer{name email plan state tier}'],
  ['drop the 403 clause', "owned {body: {conflict}} = +{status: 403, json: \"that customer isn't you\"} reply\n", ''],
  ['⊔ as an expression on overlapping records', '@body( ⊔{customer-id: ^auth.customer-id} ) owned', '+{merged: body ⊔ auth} owned'],
  ['rename onto a field that exists', 'owned = body→event', 'owned = +{event: 1} body→event'],
  ['unwrap into a record that already has the field', 'post-customer {body: {name, email, plan}} =\n  ⊃body', 'post-customer {body: {name, email, plan}} =\n  +{plan: Free} ⊃body'],
  ['change set with a wrongly typed value', 'customer ⫽ body.{name email plan state}', 'customer ⫽ {plan: "Pro"}'],
  ['change set that deletes a required column', 'customer ∆ (customer ⫽ body.{name email plan state})', 'customer ∆ customer.{id name email plan}'],
];
let caught = 0;
for (const [label, from, to] of muts) {
  if (!base.includes(from)) { console.log('!! pattern not found:', label); continue; }
  const P = L.parseProgram(base.replace(from, to));
  const R = L.makeChecker().check(P, 'main');
  const errs = [...P.errors, ...R.errors];
  if (errs.length) caught++;
  console.log(`-- ${label}: ${errs.length ? '' : 'NOT CAUGHT'}`);
  errs.slice(0, 2).forEach(e => console.log(`   ✗ ${e.line}: ${e.msg}`));
}
console.log(`${caught}/${muts.length} caught`);
