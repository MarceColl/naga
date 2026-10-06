// Differential fuzz: mutate the examples and test cases, and require `naga run` to print
// exactly what the reference prints. Needs `cargo build --release`.
// node spec/fuzz.js [count=500] [seed=1]
const fs = require('fs'), path = require('path'), os = require('os');
const { execFileSync } = require('child_process');
const root = path.join(__dirname, '..');
const N = +process.argv[2] || 500;
let seed = +process.argv[3] || 1;
const rnd = n => { seed = (seed * 1103515245 + 12345) & 0x7fffffff; return seed % n; };
// only what the reference parses: .nag sources without naga-only `route` lines
const read = d => fs.readdirSync(path.join(root, d)).filter(f => f.endsWith('.nag'))
  .map(f => fs.readFileSync(path.join(root, d, f), 'utf8')).filter(s => !/^route /m.test(s));
const pool = [...read('examples'), ...read('tests/cases')];
const steps = ['+{x: 1}', '-{id}', '.{id}', 'say "{x}"', '@body( nop )', '(nop ‖ +{y: 2})', 'show', '⊃body', 'id→key', 'n+1', '⊔{a: 1}', '⫽{a: 2}'];
const lines = s => s.split('\n'), words = s => s.split(/( +)/);
const muts = [
  s => { const l = lines(s); l.splice(rnd(l.length), 1); return l.join('\n'); },
  s => { const l = lines(s); l.splice(rnd(l.length), 0, l[rnd(l.length)]); return l.join('\n'); },
  s => { const t = words(s); t.splice(rnd(t.length), 1); return t.join(''); },
  s => { const t = words(s); const i = rnd(t.length), j = rnd(t.length); [t[i], t[j]] = [t[j], t[i]]; return t.join(''); },
  s => s.replace(/\d+/g, m => (rnd(4) ? m : String(rnd(3)))),
  s => { const ws = s.match(/[a-z][a-z-]*/g) || ['x']; return s.replace(ws[rnd(ws.length)], ws[rnd(ws.length)]); },
  s => { const l = lines(s); const i = rnd(l.length); l[i] += ' ' + steps[rnd(steps.length)]; return l.join('\n'); },
  s => s.replace(/(Asset|Free|Pro|Visa|Open|Done|true|false)/, ['Liability', 'nil', '0', '"x"', 'Free'][rnd(5)]),
];
const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'naga-fuzz-'));
const naga = path.join(root, 'target/release/naga');
let same = 0;
for (let k = 0; k < N; k++) {
  let s = pool[rnd(pool.length)];
  for (let i = 1 + rnd(3); i > 0; i--) s = muts[rnd(muts.length)](s);
  const f = path.join(dir, `f${k}.nag`);
  fs.writeFileSync(f, s);
  const run = (cmd, args) => execFileSync(cmd, args, { stdio: ['ignore', 'pipe', 'ignore'], timeout: 20000 }).toString();
  if (run('node', [path.join(__dirname, 'report.js'), f]) === run(naga, ['run', f])) same++;
  else console.log('differs:', f);
}
console.log(`${same}/${N} identical`);
