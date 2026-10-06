const L = require("../lang.js");
function check(src, label) {
  const P = L.parseProgram(src);
  if (P.errors.length) { console.log(label, 'PARSE', JSON.stringify(P.errors)); return; }
  const R = L.makeChecker().check(P, 'main');
  console.log(`== ${label}: ${R.errors.length} errors`);
  R.errors.forEach(e => console.log(`   ✗ ${e.line}: ${e.msg}`));
  for (const n of P.order) (R.signatures.get(n) || []).forEach(c => console.log(`   ${n} : ${c.in} → ${c.fail ? '✗ ' + c.fail : c.out.join(' | ') || 'never returns'}`));
}
check(`main = +{a: 1, b: "x"} go
go = a→c say c + 1`, 'rename');
check(`main = +{a: 1, b: 2} b→a`, 'rename onto existing');
check(`main = +{a: 1, b: 2, c: 3} ⊂t{a b} say t.a + c ⊃t say a`, 'wrap/unwrap');
check(`main = +{a: 1} ⊂t{a b}`, 'wrap missing field');
check(`main = +{t: {a: 1}, a: 2} ⊃t`, 'unwrap collision');
check(`main = +{body: {name: "x", kind: Asset}} post
post {body: {name, kind}} = ⊃body say name`, 'unwrap open body');
check(`main = +{x: {a: 1, b: 2}, y: {b: "s", c: 4}} +{m: x ⫽ y, n: x ⊓ y, d: x ∆ y} +{z: x ⍎ d} say z.c + 1 say m.b ++ "!"`, 'expr ops');
check(`main = +{x: {a: 1}, y: {a: 2}} +{u: x ⊔ y}`, 'expr ⊔ overlap');
check(`main = +{x: {a: 1}, y: {b: 2}} +{u: x ⊔ y} say u.b`, 'expr ⊔ disjoint');
check(`main = +{auth: {cid: 1}, body: {cid: 2}} go
go = @body( ⊔{cid: ^auth.cid} ) after
after {body: {conflict}} = say "conflict"
after = say body.cid`, 'unify step');
check(`main = +{x: {a: 1}} say x.{a b} say x.{a}.a + 1 say [x].{a}`, 'projection');
check(`table things :: {name: Text, n: Int, note: Text?}
main = +{id: 1} things.get go
go {missing} = nop
go {thing} = +{changes: thing ∆ (thing ⫽ {n: 3})} -{thing} things.update`, 'update with delta');
check(`table things :: {name: Text, n: Int, note: Text?}
main = +{id: 1} things.get go
go {missing} = nop
go {thing} = +{changes: thing ∆ (thing ⫽ {n: "three"})} -{thing} things.update`, 'update with bad delta');
check(`table things :: {name: Text, n: Int, note: Text?}
main = +{id: 1} things.get go
go {missing} = nop
go {thing} = +{changes: thing ∆ thing.{id note}} -{thing} things.update`, 'update deleting required');
