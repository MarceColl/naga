'use strict';
// Forthy dataflow language: words are record → record, juxtaposition is composition,
// multi-clause words match on the record's shape, +{} / -{} widen and narrow.
(function (root) {

class SyntaxErr extends Error { constructor(msg, tok) { super(msg); this.tok = tok; } }
class RunError extends Error { constructor(msg, details) { super(msg); this.details = details || null; } }

// ---------- values ----------
const sym = n => ({ sym: n });
const isRec = v => v instanceof Map;
const isList = Array.isArray;
const isSym = v => v !== null && typeof v === 'object' && !isRec(v) && !isList(v) && 'sym' in v;

function fmtNum(n) { return Number.isInteger(n) ? String(n) : String(parseFloat(n.toPrecision(12))); }
function show(v) {
  if (v === undefined) return '∅';
  if (v === null) return 'nil';
  if (typeof v === 'number') return fmtNum(v);
  if (typeof v === 'string') return JSON.stringify(v);
  if (typeof v === 'boolean') return String(v);
  if (isSym(v)) return v.sym;
  if (isList(v)) return '[' + v.map(show).join(', ') + ']';
  if (isRec(v)) return '{' + [...v].map(([k, x]) => `${k}: ${show(x)}`).join(', ') + '}';
  return String(v);
}
const text = v => (typeof v === 'string' ? v : show(v));
function typeName(v) {
  if (v === null) return 'nil';
  if (typeof v === 'number') return 'a number';
  if (typeof v === 'string') return 'a string';
  if (typeof v === 'boolean') return 'a boolean';
  if (isSym(v)) return 'a constant';
  if (isList(v)) return 'a list';
  if (isRec(v)) return 'a record';
  return typeof v;
}
function eqv(a, b) {
  if (a === b) return true;
  if (isSym(a) && isSym(b)) return a.sym === b.sym;
  if (isList(a) && isList(b)) return a.length === b.length && a.every((x, i) => eqv(x, b[i]));
  if (isRec(a) && isRec(b)) {
    if (a.size !== b.size) return false;
    for (const [k, x] of a) if (!b.has(k) || !eqv(x, b.get(k))) return false;
    return true;
  }
  return false;
}
const truthy = v => !(v === false || v === null);

// ---------- lexer ----------
const KEYWORDS = new Set(['when', 'say', 'show', 'raise', 'break', 'nop', 'true', 'false', 'nil', 'now', 'and', 'or', 'not', 'wrap', 'unwrap']);
const ID_RE = /[\p{L}_][\p{L}\p{Nd}_]*(?:-[\p{L}_][\p{L}\p{Nd}_]*)*/uy;
const NUM_RE = /\d+(?:\.\d+)?/y;
const OPS = ['->', '//', '\\/', '/\\', '<:', '+{', '-{', '.{', '||', '==', '!=', '<=', '>=', '&&', '++',
  '‖', '≤', '≥', '≠', '¬', '∧', '∨', '→', '⊂', '⊃', '⫽', '⊔', '⊓', '⊑', '∆', '⍎',
  '(', ')', '[', ']', '{', '}', ',', '+', '-', '*', '/', '%', '<', '>',
  '=', '!', ':', '.', '@', '^', ';'];
const CANON = { '||': '‖', '!=': '≠', '<=': '≤', '>=': '≥', '!': '¬', '&&': 'and', '∧': 'and', '∨': 'or',
  '->': '→', '//': '⫽', '\\/': '⊔', '/\\': '⊓', '<:': '⊑' };

function stripComment(line) {
  let inStr = false;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (inStr) { if (c === '\\') i++; else if (c === '"') inStr = false; continue; }
    if (c === '"') inStr = true;
    else if (c === '-' && line[i + 1] === '-') return line.slice(0, i);
  }
  return line;
}

function lexLine(src, line) {
  const toks = [];
  let i = 0, m;
  while (i < src.length) {
    const c = src[i];
    if (c === ' ' || c === '\t' || c === '\r') { i++; continue; }
    const push = (t, v, len, extra) => { toks.push({ t, v, raw: src.slice(i, i + len), line, col: i, end: i + len, ...extra }); i += len; };
    NUM_RE.lastIndex = i;
    if ((m = NUM_RE.exec(src))) { push('num', parseFloat(m[0]), m[0].length); continue; }
    if (c === '"') {
      let j = i + 1, raw = '';
      while (j < src.length && src[j] !== '"') {
        if (src[j] === '\\' && j + 1 < src.length) { raw += src[j] + src[j + 1]; j += 2; } else raw += src[j++];
      }
      if (j >= src.length) throw new SyntaxErr('this string is missing its closing "', { line, col: i, end: src.length });
      push('str', raw, j + 1 - i); continue;
    }
    if (c === '?' && i > 0 && /[\p{L}\p{Nd}_]/u.test(src[i - 1])) { push('op', '?', 1); continue; }
    if (c === '?') {
      const rest = src.slice(i + 1).trim();
      if (!rest || rest[0] === ')' || rest[0] === '‖' || rest.startsWith('||')) { push('hole', '', 1, { note: '' }); continue; }
      toks.push({ t: 'hole', v: '', raw: src.slice(i), line, col: i, end: src.length, note: rest });
      break;
    }
    ID_RE.lastIndex = i;
    if ((m = ID_RE.exec(src))) { push('id', m[0], m[0].length); continue; }
    let op = null;
    for (const o of OPS) if (src.startsWith(o, i)) { op = o; break; }
    if (!op) throw new SyntaxErr(`unexpected character “${c}”`, { line, col: i, end: i + 1 });
    push('op', CANON[op] || op, op.length);
  }
  return toks;
}

class TS {
  constructor(toks) { this.t = toks; this.i = 0; }
  peek(k = 0) { return this.t[this.i + k]; }
  next() { return this.t[this.i++]; }
  last() { return this.t[this.i - 1]; }
  atEnd() { return this.i >= this.t.length; }
  isOp(v, k = 0) { const x = this.peek(k); return !!x && x.t === 'op' && x.v === v; }
  isId(v, k = 0) { const x = this.peek(k); return !!x && x.t === 'id' && (v === undefined || x.v === v); }
  err(msg, tok) { return new SyntaxErr(msg, tok || this.peek() || this.last()); }
  expectOp(v, what) {
    const x = this.next();
    if (!x || x.t !== 'op' || x.v !== v) throw this.err(`expected “${v}”${what ? ' ' + what : ''}` + (x ? `, found “${x.raw}”` : ''), x);
    return x;
  }
}

// ---------- expressions ----------
const BIN = { or: 2, and: 3, '==': 5, '≠': 5, '<': 5, '>': 5, '≤': 5, '≥': 5, '⊑': 5, '++': 6, '+': 6, '-': 6,
  '⫽': 6, '⊔': 6, '⊓': 6, '∆': 6, '⍎': 6, '*': 7, '/': 7, '%': 7 };
const WORD_OPS = { diff: '∆', apply: '⍎' };
function binOp(tok) {
  if (!tok) return null;
  if (tok.t === 'op' && BIN[tok.v] !== undefined) return tok.v;
  if (tok.t === 'id' && (tok.v === 'and' || tok.v === 'or')) return tok.v;
  if (tok.t === 'id' && WORD_OPS[tok.v]) return WORD_OPS[tok.v];
  return null;
}
const adjacent = (a, b) => !!a && !!b && a.line === b.line && b.col === a.end;
function parseExpr(ts, minBP = 0) {
  let left = nud(ts);
  for (;;) {
    const op = binOp(ts.peek());
    if (!op) break;
    // ⫽{…} and ⊔{…} glued to their brace start a new step, not an infix operator
    if ((op === '⫽' || op === '⊔') && ts.isOp('{', 1) && adjacent(ts.peek(), ts.peek(1))) break;
    const bp = BIN[op];
    if (bp <= minBP) break;
    ts.next();
    const right = parseExpr(ts, bp);
    left = op === 'and' ? { k: 'and', a: left, b: right } : op === 'or' ? { k: 'or', a: left, b: right } : { k: 'bin', op, a: left, b: right };
  }
  return left;
}
function postfix(ts, e) {
  for (;;) {
    if (ts.isOp('.') && ts.isId(undefined, 1)) { ts.next(); e = { k: 'get', obj: e, key: ts.next().v }; continue; }
    // body.{date payee} — projection, written without a space
    if (ts.isOp('.{') && adjacent(ts.last(), ts.peek())) { ts.next(); e = { k: 'proj', obj: e, names: parseFields(ts, false, '.{…}').map(f => f.name) }; continue; }
    return e;
  }
}
function nud(ts) {
  const tok = ts.next();
  if (!tok) throw ts.err('this expression ends too early');
  if (tok.t === 'num') return { k: 'lit', v: tok.v };
  if (tok.t === 'str') return postfix(ts, parseStr(tok));
  if (tok.t === 'id') {
    switch (tok.v) {
      case 'true': return { k: 'lit', v: true };
      case 'false': return { k: 'lit', v: false };
      case 'nil': return { k: 'lit', v: null };
      case 'now': return { k: 'now' };
      case 'not': return { k: 'not', a: parseExpr(ts, 4) };
    }
    if (KEYWORDS.has(tok.v)) throw ts.err(`“${tok.v}” can't be used inside an expression`, tok);
    if (/^\p{Lu}/u.test(tok.v)) return { k: 'lit', v: sym(tok.v) };
    if (ts.isOp('(')) {
      if (!FUNCS[tok.v]) throw ts.err(`there's no function called ${tok.v}()`, tok);
      ts.next();
      const args = [];
      while (!ts.isOp(')')) {
        if (ts.atEnd()) throw ts.err(`${tok.v}( is missing its “)”`, tok);
        args.push(parseExpr(ts, 0));
        if (ts.isOp(',')) ts.next(); else if (!ts.isOp(')')) throw ts.err('expected “,” or “)”');
      }
      ts.next();
      return postfix(ts, { k: 'fn', name: tok.v, args });
    }
    return postfix(ts, { k: 'field', name: tok.v });
  }
  if (tok.t === 'op') {
    switch (tok.v) {
      case '(': { const e = parseExpr(ts, 0); ts.expectOp(')'); return postfix(ts, e); }
      case '[': {
        const items = [];
        while (!ts.isOp(']')) {
          if (ts.atEnd()) throw ts.err('this list is missing its “]”', tok);
          items.push(parseExpr(ts, 0));
          if (ts.isOp(',')) ts.next(); else if (!ts.isOp(']')) throw ts.err('expected “,” or “]” in the list');
        }
        ts.next();
        return postfix(ts, { k: 'list', items });
      }
      case '{': {
        const fields = [];
        while (!ts.isOp('}')) {
          if (ts.atEnd()) throw ts.err('this record is missing its “}”', tok);
          if (ts.isOp(',')) { ts.next(); continue; }
          const id = ts.next();
          if (id.t !== 'id') throw ts.err('expected a field name in the record', id);
          let expr = { k: 'lit', v: true };
          if (ts.isOp(':')) { ts.next(); expr = parseExpr(ts, 0); }
          fields.push({ name: id.v, expr });
        }
        ts.next();
        return postfix(ts, { k: 'rec', fields });
      }
      case '^': {
        let up = 1;
        while (ts.isOp('^')) { ts.next(); up++; }
        const id = ts.next();
        if (!id || id.t !== 'id') throw ts.err('^ needs a field name after it, like ^total', id);
        return postfix(ts, { k: 'outer', up, name: id.v });
      }
      case '-': return { k: 'neg', a: parseExpr(ts, 8) };
      case '¬': return { k: 'not', a: parseExpr(ts, 4) };
    }
  }
  throw ts.err(`unexpected “${tok.raw}”`, tok);
}
function parseStr(tok) {
  const raw = tok.v, parts = [];
  let buf = '', i = 0;
  while (i < raw.length) {
    const c = raw[i];
    if (c === '\\') { const n = raw[i + 1]; buf += n === 'n' ? '\n' : n === 't' ? '\t' : n; i += 2; continue; }
    if (c === '{') {
      const j = raw.indexOf('}', i);
      if (j < 0) throw new SyntaxErr('a “{” in this string has no matching “}” (write \\{ for a literal brace)', tok);
      if (buf) { parts.push(buf); buf = ''; }
      const its = new TS(lexLine(raw.slice(i + 1, j), tok.line));
      const e = parseExpr(its, 0);
      if (!its.atEnd()) throw new SyntaxErr(`unexpected “${its.peek().raw}” inside {…} in a string`, tok);
      parts.push(e);
      i = j + 1; continue;
    }
    buf += c; i++;
  }
  if (buf || !parts.length) parts.push(buf);
  return parts.length === 1 && typeof parts[0] === 'string' ? { k: 'lit', v: parts[0] } : { k: 'interp', parts };
}

const num = (v, what) => { if (typeof v !== 'number') throw new RunError(`${what} needs a number, got ${typeName(v)} ${show(v)}`); return v; };
const list = (v, what) => { if (!isList(v)) throw new RunError(`${what} needs a list, got ${typeName(v)} ${show(v)}`); return v; };
const FUNCS = {
  len: x => { if (isList(x) || typeof x === 'string') return x.length; if (isRec(x)) return x.size; throw new RunError(`len() needs a list, string or record, got ${typeName(x)}`); },
  sum: xs => list(xs, 'sum()').reduce((a, x) => a + num(x, 'sum()'), 0),
  min: (...a) => { const xs = a.length === 1 && isList(a[0]) ? a[0] : a; if (!xs.length) throw new RunError('min() of nothing'); return Math.min(...xs.map(x => num(x, 'min()'))); },
  max: (...a) => { const xs = a.length === 1 && isList(a[0]) ? a[0] : a; if (!xs.length) throw new RunError('max() of nothing'); return Math.max(...xs.map(x => num(x, 'max()'))); },
  abs: x => Math.abs(num(x, 'abs()')),
  round: (x, d = 0) => { const f = 10 ** num(d, 'round()'); return Math.round(num(x, 'round()') * f) / f; },
  floor: x => Math.floor(num(x, 'floor()')),
  ceil: x => Math.ceil(num(x, 'ceil()')),
  head: xs => { if (!list(xs, 'head()').length) throw new RunError('head() of an empty list'); return xs[0]; },
  tail: xs => list(xs, 'tail()').slice(1),
  last: xs => { if (!list(xs, 'last()').length) throw new RunError('last() of an empty list'); return xs[xs.length - 1]; },
  push: (xs, x) => [...list(xs, 'push()'), x],
  reverse: xs => [...list(xs, 'reverse()')].reverse(),
  range: (a, b) => { if (b === undefined) { b = a; a = 0; } num(a, 'range()'); num(b, 'range()'); const r = []; for (let i = a; i < b && r.length < 100000; i++) r.push(i); return r; },
  str: x => text(x),
  keys: r => { if (!isRec(r)) throw new RunError(`keys() needs a record, got ${typeName(r)}`); return [...r.keys()].map(sym); },
};

function evalE(e, env) {
  const s = env.subj;
  switch (e.k) {
    case 'lit': return e.v;
    case 'now': return env.now;
    case 'field':
      if (!s.has(e.name)) throw new RunError(`needs field ${e.name}`);
      return s.get(e.name);
    case 'outer': {
      const r = env.outer[e.up - 1];
      if (!r) throw new RunError(`^${e.name}: there's no enclosing record here — ^ only works inside @field( … )`);
      if (!r.has(e.name)) throw new RunError(`the enclosing record has no field ${e.name}`);
      return r.get(e.name);
    }
    case 'get': return getKey(evalE(e.obj, env), e.key);
    case 'proj': return project(evalE(e.obj, env), e.names);
    case 'list': return e.items.map(x => evalE(x, env));
    case 'rec': { const m = new Map(); for (const f of e.fields) m.set(f.name, evalE(f.expr, env)); return m; }
    case 'interp': return e.parts.map(p => (typeof p === 'string' ? p : text(evalE(p, env)))).join('');
    case 'neg': return -num(evalE(e.a, env), 'minus');
    case 'not': return !truthy(evalE(e.a, env));
    case 'and': return truthy(evalE(e.a, env)) && truthy(evalE(e.b, env));
    case 'or': return truthy(evalE(e.a, env)) || truthy(evalE(e.b, env));
    case 'fn': return FUNCS[e.name](...e.args.map(a => evalE(a, env)));
    case 'bin': {
      const a = evalE(e.a, env), b = evalE(e.b, env);
      switch (e.op) {
        case '==': return eqv(a, b);
        case '≠': return !eqv(a, b);
        case '++': return isList(a) && isList(b) ? [...a, ...b] : text(a) + text(b);
        case '⊑': return subsumes(a, b);
        case '⫽': recArgs(a, b, '⫽'); return new Map([...a, ...b]);
        case '⊔': {
          recArgs(a, b, '⊔');
          const r = unifyRec(a, b);
          if (r.conflict.size) throw new RunError(`⊔: the records disagree on ${[...r.conflict.keys()].join(', ')} — the ⊔{…} step adds conflict instead of failing`,
            [...r.conflict].map(([k, c]) => `${k}: ${show(c.get('left'))} vs ${show(c.get('right'))}`));
          return r.merged;
        }
        case '⊓': recArgs(a, b, '⊓'); return meetRec(a, b);
        case '∆': recArgs(a, b, '∆'); return diffRec(a, b);
        case '⍎': return applyDelta(a, b);
        case '<': case '>': case '≤': case '≥': {
          if (!((typeof a === 'number' && typeof b === 'number') || (typeof a === 'string' && typeof b === 'string')))
            throw new RunError(`can't compare ${typeName(a)} with ${typeName(b)} (${show(a)} ${e.op} ${show(b)})`);
          return e.op === '<' ? a < b : e.op === '>' ? a > b : e.op === '≤' ? a <= b : a >= b;
        }
      }
      if (typeof a !== 'number' || typeof b !== 'number')
        throw new RunError(`can't use ${e.op} on ${typeName(a)} and ${typeName(b)} (${show(a)} ${e.op} ${show(b)})` + (e.op === '+' && (typeof a === 'string' || typeof b === 'string') ? ' — use ++ to join text' : ''));
      switch (e.op) {
        case '+': return a + b;
        case '-': return a - b;
        case '*': return a * b;
        case '/': if (b === 0) throw new RunError('division by zero'); return a / b;
        case '%': if (b === 0) throw new RunError('division by zero'); return a % b;
      }
    }
  }
  throw new Error('bad expression ' + e.k);
}
function getKey(o, key) {
  if (isRec(o)) { if (!o.has(key)) throw new RunError(`${show(o)} has no field ${key}`); return o.get(key); }
  if (isList(o)) return o.map(x => getKey(x, key));
  throw new RunError(`can't read .${key} from ${typeName(o)} ${show(o)}`);
}

// ---------- record algebra ----------
// A change set (from a ∆ b) is a plain record: each field holds one tag record,
// {same: v} | {set: new, was: old} | {add: v} | {del: old}, so clauses can match on it.
const TAGS = ['same', 'set', 'add', 'del'];
const tagRec = (k, v, was) => { const m = new Map([[k, v]]); if (was !== undefined) m.set('was', was); return m; };
function recArgs(a, b, op) {
  if (!isRec(a) || !isRec(b)) throw new RunError(`${op} needs two records, got ${typeName(a)} ${show(a)} and ${typeName(b)} ${show(b)}`);
}
function isDelta(v) {
  if (!isRec(v)) return false;
  for (const e of v.values()) {
    if (!isRec(e)) return false;
    let n = 0;
    for (const k of e.keys()) { if (TAGS.includes(k)) n++; else if (k !== 'was') return false; }
    if (n !== 1) return false;
  }
  return true;
}
function diffRec(a, b) {
  const d = new Map();
  for (const [k, x] of a) {
    if (!b.has(k)) d.set(k, tagRec('del', x));
    else if (eqv(x, b.get(k))) d.set(k, tagRec('same', x));
    else d.set(k, tagRec('set', b.get(k), x));
  }
  for (const [k, y] of b) if (!a.has(k)) d.set(k, tagRec('add', y));
  return d;
}
function applyDelta(x, d) {
  if (!isRec(x)) throw new RunError(`⍎ applies a change set to a record, got ${typeName(x)} ${show(x)}`);
  if (!isDelta(d)) throw new RunError(`⍎ needs a change set made by ∆, got ${show(d)}`);
  const n = new Map(x);
  for (const [k, e] of d) {
    if (e.has('del')) n.delete(k);
    else if (e.has('set')) n.set(k, e.get('set'));
    else if (e.has('add')) n.set(k, e.get('add'));
  }
  return n;
}
function deltaChanges(d) {
  // what .update needs: new values, and nil for a deleted column
  const ch = new Map();
  for (const [k, e] of d) {
    if (e.has('same')) continue;
    ch.set(k, e.has('del') ? null : e.has('set') ? e.get('set') : e.get('add'));
  }
  return ch;
}
function unifyRec(a, b) {
  const merged = new Map(a), conflict = new Map();
  for (const [k, y] of b) {
    if (a.has(k) && !eqv(a.get(k), y)) conflict.set(k, new Map([['left', a.get(k)], ['right', y]]));
    else merged.set(k, y);
  }
  return { merged, conflict };
}
function meetRec(a, b) {
  const m = new Map();
  for (const [k, x] of a) {
    if (!b.has(k)) continue;
    const y = b.get(k);
    m.set(k, eqv(x, y) ? new Map([['agree', x]]) : new Map([['differ', new Map([['left', x], ['right', y]])]]));
  }
  return m;
}
function subsumes(a, b) {
  if (isRec(a) && isRec(b)) { for (const [k, x] of a) if (!b.has(k) || !subsumes(x, b.get(k))) return false; return true; }
  return eqv(a, b);
}
function project(v, names) {
  if (isList(v)) return v.map(x => project(x, names));
  if (!isRec(v)) throw new RunError(`.{${names.join(' ')}} needs a record or a list of records, got ${typeName(v)} ${show(v)}`);
  const m = new Map();
  for (const n of names) if (v.has(n)) m.set(n, v.get(n));
  return m;
}

// ---------- patterns ----------
function parseLiteral(ts) {
  const t = ts.next();
  if (!t) throw ts.err('expected a value');
  if (t.t === 'num') return t.v;
  if (t.t === 'op' && t.v === '-' && ts.peek() && ts.peek().t === 'num') return -ts.next().v;
  if (t.t === 'str') { const e = parseStr(t); if (e.k !== 'lit') throw ts.err('strings in patterns can’t use {…}', t); return e.v; }
  if (t.t === 'id') {
    if (t.v === 'true') return true;
    if (t.v === 'false') return false;
    if (t.v === 'nil') return null;
    if (/^\p{Lu}/u.test(t.v)) return sym(t.v);
  }
  if (t.t === 'op' && t.v === '[') {
    const items = [];
    while (!ts.isOp(']')) { if (ts.atEnd()) throw ts.err('missing “]”'); items.push(parseLiteral(ts)); if (ts.isOp(',')) ts.next(); }
    ts.next();
    return items;
  }
  throw ts.err(`a pattern can only compare against a fixed value (a number, "text", a Constant, true/false/nil or []); “${t.raw}” isn't one — use a when guard instead`, t);
}
function parsePattern(ts, nested) {
  const open = ts.expectOp('{');
  const entries = [];
  for (;;) {
    if (ts.atEnd()) throw ts.err('this pattern is missing its “}”', open);
    if (ts.isOp('}')) { ts.next(); return entries; }
    if (ts.isOp(',')) { ts.next(); continue; }
    let mode = 'has';
    if (ts.isOp('-')) { ts.next(); mode = 'take'; } else if (ts.isOp('¬')) { ts.next(); mode = 'absent'; }
    if (nested && mode === 'take') throw ts.err(nested === 'query' ? 'a where pattern can’t take fields out (-x)' : '-field only works at the top level of a pattern');
    const id = ts.next();
    if (!id || id.t !== 'id' || KEYWORDS.has(id.v)) throw ts.err('expected a field name in the pattern', id);
    const ent = { name: id.v, mode };
    if (ts.isOp(':')) {
      if (mode === 'absent') throw ts.err(`!${id.v} means “no ${id.v}”, so it can't have a value`);
      ts.next();
      if (ts.isOp('{')) ent.sub = parsePattern(ts, nested || true); else ent.lit = parseLiteral(ts);
    }
    entries.push(ent);
  }
}
function matchEntries(entries, rec) {
  const take = [];
  for (const e of entries) {
    const has = rec.has(e.name);
    if (e.mode === 'absent') { if (has) return { ok: false, why: `has ${e.name}` }; continue; }
    if (!has) return { ok: false, why: `needs ${e.name}` };
    const v = rec.get(e.name);
    if (e.sub) {
      if (!isRec(v)) return { ok: false, why: `${e.name} is ${typeName(v)}, not a record` };
      const r = matchEntries(e.sub, v);
      if (!r.ok) return { ok: false, why: `${e.name}: ${r.why}` };
    }
    if ('lit' in e && !eqv(v, e.lit)) return { ok: false, why: `${e.name} is ${show(v)}, not ${show(e.lit)}` };
    if (e.mode === 'take') take.push(e.name);
  }
  return { ok: true, take };
}
function tryClause(c, env) {
  const r = c.pattern ? matchEntries(c.pattern, env.subj) : { ok: true, take: [] };
  if (!r.ok) return r;
  if (c.guard) {
    let g;
    try { g = truthy(evalE(c.guard, env)); } catch (e) {
      if (!(e instanceof RunError)) throw e;
      return { ok: false, why: `guard failed: ${e.message}` };
    }
    if (!g) return { ok: false, why: `when ${c.guardSrc} is false` };
  }
  return r;
}
function explainCall(P, name, env) {
  const w = P.words.get(name);
  if (!w) return null;
  const out = [];
  let found = false;
  w.clauses.forEach((c, i) => {
    if (found) { out.push({ idx: i, clause: c, state: 'skip' }); return; }
    const r = tryClause(c, env);
    if (r.ok) { found = true; out.push({ idx: i, clause: c, state: 'match', take: r.take }); }
    else out.push({ idx: i, clause: c, state: 'miss', why: r.why });
  });
  return out;
}

// ---------- statements and definitions ----------
function parseFields(ts, withValues, what) {
  const out = [];
  for (;;) {
    if (ts.atEnd()) throw ts.err(`${what} is missing its “}”`);
    if (ts.isOp('}')) { ts.next(); return out; }
    if (ts.isOp(',')) { ts.next(); continue; }
    const id = ts.next();
    if (id.t !== 'id' || KEYWORDS.has(id.v)) throw ts.err(`expected a field name in ${what}`, id);
    let expr = null;
    if (ts.isOp(':')) {
      if (!withValues) throw ts.err(`${what} takes field names only`);
      ts.next();
      expr = parseExpr(ts, 0);
    }
    out.push({ name: id.v, expr });
  }
}
function parseBody(ts, closer) {
  const branches = [[]];
  const start = ts.peek();
  while (!ts.atEnd()) {
    if (closer && ts.isOp(closer)) break;
    if (ts.isOp('‖')) {
      const bar = ts.next();
      if (!branches[branches.length - 1].length) throw ts.err('‖ needs steps on both sides', bar);
      branches.push([]);
      continue;
    }
    if (ts.isOp(';')) { ts.next(); continue; }
    branches[branches.length - 1].push(...parseStmt(ts));
  }
  if (branches.length > 1) {
    if (!branches[branches.length - 1].length) throw ts.err('‖ needs steps on both sides');
    const last = ts.last();
    return [{ k: 'par', branches, pos: { line: start.line, col: start.col, endLine: last.line, endCol: last.end } }];
  }
  return branches[0];
}
function parseStmt(ts) {
  const start = ts.peek();
  const mk = node => {
    const last = ts.last();
    node.pos = { line: start.line, col: start.col, endLine: last.line, endCol: last.end };
    return [node];
  };
  if (start.t === 'hole') { ts.next(); return mk({ k: 'hole', note: start.note }); }
  if (start.t === 'op') {
    switch (start.v) {
      case '+{': ts.next(); return mk({ k: 'add', fields: parseFields(ts, true, '+{…}') });
      case '-{': ts.next(); return mk({ k: 'del', names: parseFields(ts, false, '-{…}').map(f => f.name) });
      case '.{': ts.next(); return mk({ k: 'pick', names: parseFields(ts, false, '.{…}').map(f => f.name) });
      case '@': {
        ts.next();
        const id = ts.next();
        if (!id || id.t !== 'id') throw ts.err('@ needs a field name, like @card( … )', id);
        ts.expectOp('(', `after @${id.v}`);
        const body = parseBody(ts, ')');
        ts.expectOp(')', `to close @${id.v}(`);
        return mk({ k: 'focus', field: id.v, body });
      }
      case '(': {
        ts.next();
        const body = parseBody(ts, ')');
        ts.expectOp(')');
        return body;
      }
      case '⊂': ts.next(); return mk(parseWrap(ts, '⊂'));
      case '⊃': ts.next(); return mk(parseUnwrap(ts, '⊃'));
      case '⫽': case '⊔':
        ts.next();
        ts.expectOp('{', `after ${start.v}`);
        return mk({ k: start.v === '⫽' ? 'over' : 'unify', fields: parseFields(ts, true, `${start.v}{…}`) });
      case '→': throw ts.err('→ renames a field: write old→new', start);
      case '‖': throw ts.err('‖ needs steps on both sides', start);
    }
    throw ts.err(`unexpected “${start.raw}”`, start);
  }
  if (start.t === 'id') {
    ts.next();
    switch (start.v) {
      case 'say': return mk({ k: 'say', expr: parseExpr(ts, 0) });
      case 'raise': return mk({ k: 'raise', expr: parseExpr(ts, 0) });
      case 'show': return mk({ k: 'show' });
      case 'break': return mk({ k: 'break' });
      case 'nop': case 'ε': return [];
      case 'wrap': return mk(parseWrap(ts, 'wrap '));
      case 'unwrap': return mk(parseUnwrap(ts, 'unwrap '));
    }
    if (KEYWORDS.has(start.v)) throw ts.err(`“${start.v}” can't start a step`, start);
    if (/^\p{Lu}/u.test(start.v)) throw ts.err(`${start.v} is a constant, not a word — put it in a field, like +{status: ${start.v}}`, start);
    const n1 = ts.peek(), n2 = ts.peek(1);
    if (n1 && n1.t === 'op' && n1.v === '.' && n2 && n2.t === 'id') {
      ts.next(); ts.next();
      const node = { k: 'tcall', table: start.v, op: n2.v };
      if (node.op === 'where') {
        if (ts.isOp('{')) node.pattern = parsePattern(ts, 'query');
        if (ts.isId('when')) { ts.next(); node.guard = parseExpr(ts, 0); }
      }
      return mk(node);
    }
    if (n1 && n1.t === 'op' && (n1.v === '+' || n1.v === '-') && n2 && n2.t === 'num') {
      ts.next(); ts.next();
      return mk({ k: 'inc', name: start.v, n: n1.v === '+' ? n2.v : -n2.v });
    }
    if (n1 && n1.t === 'op' && n1.v === '=') { ts.next(); return mk({ k: 'set', name: start.v, expr: parseExpr(ts, 0) }); }
    if (n1 && n1.t === 'op' && n1.v === '→') {
      ts.next();
      const to = ts.next();
      if (!to || to.t !== 'id' || KEYWORDS.has(to.v) || /^\p{Lu}/u.test(to.v)) throw ts.err(`→ needs the new field name, like ${start.v}→other`, to);
      return mk({ k: 'rename', from: start.v, to: to.v });
    }
    return mk({ k: 'call', name: start.v });
  }
  if (start.t === 'str' || start.t === 'num') throw ts.err(`a bare value can't be a step — did you mean say ${start.raw}?`, start);
  throw ts.err(`unexpected “${start.raw}”`, start);
}
function parseWrap(ts, how) {
  const id = ts.next();
  if (!id || id.t !== 'id' || KEYWORDS.has(id.v)) throw ts.err(`${how.trim()} needs the new field's name, like ${how}account{name kind}`, id);
  ts.expectOp('{', `after ${how}${id.v}`);
  return { k: 'wrap', field: id.v, names: parseFields(ts, false, `${how}${id.v}{…}`).map(f => f.name) };
}
function parseUnwrap(ts, how) {
  const id = ts.next();
  if (!id || id.t !== 'id' || KEYWORDS.has(id.v)) throw ts.err(`${how.trim()} needs a field name, like ${how}body`, id);
  return { k: 'unwrap', field: id.v };
}
function parseDef(ts, lines) {
  const nameTok = ts.next();
  if (!nameTok || nameTok.t !== 'id') throw ts.err('a definition starts with a word name, like greet = …', nameTok);
  const name = nameTok.v;
  if (KEYWORDS.has(name) || FUNCS[name]) throw ts.err(`${name} is a built-in name`, nameTok);
  if (/^\p{Lu}/u.test(name)) throw ts.err('word names start lowercase; Capitalized names are constants', nameTok);
  let pattern = null, guard = null, guardSrc = null;
  if (ts.isOp('{')) pattern = parsePattern(ts, false);
  if (ts.isId('when')) {
    const w = ts.next();
    guard = parseExpr(ts, 0);
    const g = ts.last();
    guardSrc = w.line === g.line ? lines[w.line - 1].slice(w.end, g.end).trim() : 'guard';
  }
  const eq = ts.next();
  if (!eq || eq.t !== 'op' || (eq.v !== '=' && eq.v !== ':')) throw ts.err(`expected “=” after ${name}${pattern ? ' and its pattern' : ''}`, eq);
  const headSrc = lines[nameTok.line - 1].slice(nameTok.col, eq.col).trim();
  const body = parseBody(ts, null);
  return { name, pattern, guard, guardSrc, body, line: nameTok.line, headSrc };
}

function walkBody(body, fn) {
  for (const s of body) {
    fn(s);
    if (s.body) walkBody(s.body, fn);
    if (s.branches) s.branches.forEach(b => walkBody(b, fn));
  }
}
function parseProgram(src) {
  const lines = src.split('\n');
  const P = { words: new Map(), order: [], errors: [], warnings: [], holes: [], lines, tables: new Map(), seeds: [] };
  const chunks = [];
  let cur = null;
  lines.forEach((raw, idx) => {
    const code = stripComment(raw);
    if (!code.trim()) return;
    if (/^[ \t]/.test(code) && cur) cur.lines.push(idx);
    else { cur = { lines: [idx] }; chunks.push(cur); }
  });
  for (const ch of chunks) {
    try {
      const toks = [];
      for (const li of ch.lines) toks.push(...lexLine(stripComment(lines[li]), li + 1));
      const ts = new TS(toks);
      if (toks[0] && toks[0].t === 'id' && toks[1] && toks[1].t === 'id' && (toks[0].v === 'table' || toks[0].v === 'seed')) {
        if (toks[0].v === 'table') {
          const T = parseTable(ts);
          if (P.tables.has(T.name)) throw ts.err(`table ${T.name} is already declared on line ${P.tables.get(T.name).line}`, toks[1]);
          P.tables.set(T.name, T);
        } else P.seeds.push(parseSeed(ts));
        continue;
      }
      const def = parseDef(ts, lines);
      if (!ts.atEnd()) throw ts.err(`unexpected “${ts.peek().raw}”`);
      def.chunkLine = ch.lines[0] + 1;
      def.chunkText = lines.slice(ch.lines[0], ch.lines[ch.lines.length - 1] + 1).join('\n');
      let w = P.words.get(def.name);
      if (!w) { w = { name: def.name, clauses: [], line: def.line }; P.words.set(def.name, w); P.order.push(def.name); }
      w.clauses.push(def);
    } catch (e) {
      if (!(e instanceof SyntaxErr)) throw e;
      P.errors.push({ line: e.tok ? e.tok.line : ch.lines[0] + 1, col: e.tok ? e.tok.col : 0, msg: e.message });
    }
  }
  const seenUnknown = new Set();
  for (const w of P.words.values()) {
    w.clauses.forEach((c, i) => {
      walkBody(c.body, s => {
        if (s.k === 'call' && !P.words.has(s.name)) {
          const key = s.name + '@' + s.pos.line;
          if (!seenUnknown.has(key)) { seenUnknown.add(key); P.errors.push({ line: s.pos.line, col: s.pos.col, msg: `${s.name} isn't defined yet — write ${s.name} = ? to leave it open`, unknown: s.name }); }
        }
        if (s.k === 'hole') P.holes.push({ line: s.pos.line, note: s.note, word: w.name });
        if (s.k === 'tcall') {
          const T = P.tables.get(s.table);
          if (!T) P.errors.push({ line: s.pos.line, col: s.pos.col, msg: `there's no table called ${s.table} — declare it with table ${s.table} :: {…}` });
          else if (!TABLE_OPS.includes(s.op)) P.errors.push({ line: s.pos.line, col: s.pos.col, msg: `${s.table}.${s.op} isn't a table word — use ${TABLE_OPS.map(o => s.table + '.' + o).join(', ')}` });
          else if (s.op === 'where') {
            try { s.compiled = compileWhere(T, s.pattern, s.guard); }
            catch (e) { if (!(e instanceof SyntaxErr)) throw e; P.errors.push({ line: s.pos.line, col: s.pos.col, msg: e.message }); }
          }
        }
      });
      const catchAll = (!c.pattern || !c.pattern.length) && !c.guard;
      if (catchAll && i < w.clauses.length - 1)
        P.warnings.push({ line: w.clauses[i + 1].line, msg: `this ${w.name} clause never runs: the one on line ${c.line} matches everything` });
    });
  }
  for (const sd of P.seeds) if (!P.tables.has(sd.table)) P.errors.push({ line: sd.line, col: 0, msg: `seed ${sd.table}: there's no table called ${sd.table}` });
  P.errors.sort((a, b) => a.line - b.line);
  return P;
}

// ---------- tables ----------
const COL_TYPES = { Int: 'INTEGER', Num: 'REAL', Text: 'TEXT', Bool: 'INTEGER', Const: 'TEXT', Json: 'TEXT' };
const TABLE_OPS = ['all', 'get', 'where', 'insert', 'update', 'delete'];
const qi = n => '"' + String(n).replace(/"/g, '""') + '"';
function singular(name) {
  if (/ies$/.test(name)) return name.slice(0, -3) + 'y';
  if (/(ss|sh|ch|x|z)es$/.test(name)) return name.slice(0, -2);
  if (/[^s]s$/.test(name)) return name.slice(0, -1);
  return null;
}
function parseTable(ts) {
  ts.next();
  const nameTok = ts.next();
  let single = null;
  if (ts.isId('of')) {
    ts.next();
    const s = ts.next();
    if (!s || s.t !== 'id') throw ts.err('expected the name of one row after “of”', s);
    single = s.v;
  } else single = singular(nameTok.v);
  if (!single) throw ts.err(`can't guess what one row of ${nameTok.v} is called — write table ${nameTok.v} of item :: {…}`, nameTok);
  if (single === nameTok.v) throw ts.err('the row name has to differ from the table name', nameTok);
  if (!ts.isOp(':')) throw ts.err(`expected “::” after table ${nameTok.v}`);
  ts.next();
  if (ts.isOp(':')) ts.next();
  ts.expectOp('{', 'to start the columns');
  const cols = [];
  for (;;) {
    if (ts.atEnd()) throw ts.err('the column list is missing its “}”');
    if (ts.isOp('}')) { ts.next(); break; }
    if (ts.isOp(',')) { ts.next(); continue; }
    const id = ts.next();
    if (id.t !== 'id' || KEYWORDS.has(id.v)) throw ts.err('expected a column name', id);
    let type = 'Json', opt = true;
    if (ts.isOp(':')) {
      ts.next();
      const ty = ts.next();
      if (!ty || ty.t !== 'id' || !COL_TYPES[ty.v]) throw ts.err(`column types are ${Object.keys(COL_TYPES).join(', ')} (add ? for an optional column, like Text?)`, ty || id);
      type = ty.v;
      opt = false;
      if (ts.isOp('?')) { ts.next(); opt = true; }
    }
    if (id.v === 'id') { if (type !== 'Int' && type !== 'Json') throw ts.err('id is always an Int', id); continue; }
    if (cols.some(c => c.name === id.v)) throw ts.err(`${id.v} is listed twice`, id);
    cols.push({ name: id.v, type, opt });
  }
  if (!ts.atEnd()) throw ts.err(`unexpected “${ts.peek().raw}” after the columns`);
  return { name: nameTok.v, single, cols, colMap: new Map(cols.map(c => [c.name, c])), line: nameTok.line };
}
function parseSeed(ts) {
  ts.next();
  const nameTok = ts.next();
  ts.expectOp('=', `after seed ${nameTok.v}`);
  const expr = parseExpr(ts, 0);
  if (!ts.atEnd()) throw ts.err(`unexpected “${ts.peek().raw}”`);
  return { table: nameTok.v, expr, line: nameTok.line };
}

// ^x (or ^x.y.z) inside a where is a value from the record being processed, passed as a parameter
function outerParam(e) {
  if (e.k === 'outer') return e.up === 1 ? { k: 'field', name: e.name } : { k: 'outer', up: e.up - 1, name: e.name };
  if (e.k === 'get') { const o = outerParam(e.obj); return o && { k: 'get', obj: o, key: e.key }; }
  return null;
}
// A where pattern and guard become one SQL condition. Bare names are columns; ^x reads the current record.
function compileWhere(T, pattern, guard) {
  const params = [];
  const col = name => {
    if (name !== 'id' && !T.colMap.has(name)) throw new SyntaxErr(`${T.name} has no column ${name}`);
    return qi(name);
  };
  const isNil = e => e.k === 'lit' && e.v === null;
  const BOOLISH = ['==', '≠', '<', '>', '≤', '≥'];
  function cBool(e) {
    if (e.k === 'and' || e.k === 'or' || e.k === 'not' || (e.k === 'bin' && BOOLISH.includes(e.op))) return cExpr(e);
    return `coalesce(${cExpr(e)}, 0) IS NOT 0`;
  }
  function cExpr(e) {
    switch (e.k) {
      case 'lit': if (e.v === null) return 'NULL'; params.push({ lit: e.v }); return '?';
      case 'field': return col(e.name);
      case 'outer': params.push({ expr: outerParam(e) }); return '?';
      case 'get': { const ex = outerParam(e); if (ex) { params.push({ expr: ex }); return '?'; } break; }
      case 'not': return `(NOT ${cBool(e.a)})`;
      case 'and': return `(${cBool(e.a)} AND ${cBool(e.b)})`;
      case 'or': return `(${cBool(e.a)} OR ${cBool(e.b)})`;
      case 'neg': return `(-${cExpr(e.a)})`;
      case 'bin': {
        if ((e.op === '==' || e.op === '≠') && (isNil(e.a) || isNil(e.b))) return `(${cExpr(isNil(e.a) ? e.b : e.a)} ${e.op === '==' ? 'IS' : 'IS NOT'} NULL)`;
        const a = cExpr(e.a), b = cExpr(e.b);
        if (e.op === '/') return `(CAST(${a} AS REAL) / ${b})`;
        const op = { '==': '=', '≠': '<>', '<': '<', '>': '>', '≤': '<=', '≥': '>=', '+': '+', '-': '-', '*': '*', '%': '%', '++': '||' }[e.op];
        if (!op) throw new SyntaxErr(`${e.op} can't be used inside a where`);
        return `(${a} ${op} ${b})`;
      }
      case 'fn': {
        const m = { len: 'length', abs: 'abs', round: 'round', min: 'min', max: 'max' }[e.name];
        if (!m || ((e.name === 'min' || e.name === 'max') && e.args.length < 2)) throw new SyntaxErr(`${e.name}() can't be used inside a where`);
        return `${m}(${e.args.map(cExpr).join(', ')})`;
      }
    }
    throw new SyntaxErr('this part of the where can’t be turned into SQL (use fields, ^fields, constants, comparisons, arithmetic, and/or/not, len/abs/round)');
  }
  const parts = [];
  for (const e of pattern || []) {
    if (e.sub) throw new SyntaxErr(`a where pattern can't look inside ${e.name}`);
    const c = col(e.name);
    if (e.mode === 'absent' || ('lit' in e && e.lit === null)) parts.push(`${c} IS NULL`);
    else if ('lit' in e) { params.push({ lit: e.lit }); parts.push(`${c} = ?`); }
    else parts.push(`${c} IS NOT NULL`);
  }
  if (guard) parts.push(cBool(guard));
  return { sql: parts.length ? parts.join(' AND ') : '1', params };
}

function toJson(v) {
  if (isRec(v)) { const o = {}; for (const [k, x] of v) o[k] = toJson(x); return o; }
  if (isList(v)) return v.map(toJson);
  if (isSym(v)) return { $sym: v.sym };
  return v;
}
function fromJson(j) {
  if (Array.isArray(j)) return j.map(fromJson);
  if (j && typeof j === 'object') {
    const ks = Object.keys(j);
    if (ks.length === 1 && ks[0] === '$sym') return sym(j.$sym);
    return new Map(ks.map(k => [k, fromJson(j[k])]));
  }
  return j;
}
function toSql(v) {
  if (v === undefined || v === null) return null;
  if (typeof v === 'boolean') return v ? 1 : 0;
  if (typeof v === 'number' || typeof v === 'string') return v;
  if (isSym(v)) return v.sym;
  return JSON.stringify(toJson(v));
}
function toSqlCol(T, k, v) {
  if (v === null || v === undefined) return null;
  const c = T.colMap.get(k);
  return c && c.type === 'Json' ? JSON.stringify(toJson(v)) : toSql(v);
}
function fromSql(type, v) {
  if (v === null || v === undefined) return undefined;
  switch (type) {
    case 'Int': case 'Num': return Number(v);
    case 'Text': return String(v);
    case 'Bool': return v !== 0 && v !== '0';
    case 'Const': return sym(String(v));
    default: try { return fromJson(JSON.parse(v)); } catch (e) { return v; }
  }
}
const TYPE_OK = { Int: v => Number.isInteger(v), Num: v => typeof v === 'number', Text: v => typeof v === 'string', Bool: v => typeof v === 'boolean', Const: isSym, Json: () => true };
function checkRow(T, rec, what) {
  if (!isRec(rec)) throw new RunError(`${what} must be a record, got ${typeName(rec)} ${show(rec)}`);
  for (const [k, v] of rec) {
    if (k === 'id') { if (!Number.isInteger(v)) throw new RunError(`id must be a whole number, got ${show(v)}`); continue; }
    const c = T.colMap.get(k);
    if (!c) throw new RunError(`${T.name} has no column ${k} (it has id${T.cols.map(x => ', ' + x.name).join('')})`);
    if (v !== null && !TYPE_OK[c.type](v)) throw new RunError(`${T.name}.${k} is ${c.type}, but got ${typeName(v)} ${show(v)}`);
  }
}
function checkInsert(T, rec) {
  checkRow(T, rec, T.single);
  for (const c of T.cols) if (!c.opt && (!rec.has(c.name) || rec.get(c.name) === null))
    throw new RunError(`${T.single} needs ${c.name} — ${T.name}.${c.name} is required (write ${c.type}? in the table to make it optional)`);
}
function checkChanges(T, ch) {
  checkRow(T, ch, 'changes');
  if (ch.has('id')) throw new RunError('changes can’t include id');
  for (const c of T.cols) if (!c.opt && ch.has(c.name) && ch.get(c.name) === null)
    throw new RunError(`${T.name}.${c.name} is required, so it can't be set to nil`);
}
function inlineSql(sql, params) {
  let i = 0;
  return sql.replace(/\?/g, () => {
    const v = params[i++];
    if (v === null || v === undefined) return 'NULL';
    if (typeof v === 'number') return String(v);
    return "'" + String(v).replace(/'/g, "''") + "'";
  });
}
function rowOf(T, o) {
  const m = new Map([['id', Number(o.id)]]);
  for (const c of T.cols) { const v = fromSql(c.type, o[c.name]); if (v !== undefined) m.set(c.name, v); }
  return m;
}

// Runs the generated words as SQL through a driver: { all(sql, params) → rows, run(sql, params) → {changes, lastId} }.
class SqlStore {
  constructor(P, driver, label) {
    this.P = P; this.d = driver; this.label = label; this.log = []; this.tick = -1; this.sql = true;
    for (const T of P.tables.values()) {
      const cols = ['"id" INTEGER PRIMARY KEY', ...T.cols.map(c => `${qi(c.name)} ${COL_TYPES[c.type]}${c.opt ? '' : ' NOT NULL'}`)];
      this.q(`CREATE TABLE ${qi(T.name)} (${cols.join(', ')})`, [], 'run');
    }
  }
  q(sql, params, mode, silent) {
    if (!silent) this.log.push({ tick: this.tick, text: inlineSql(sql, params) });
    try { return mode === 'run' ? this.d.run(sql, params) : this.d.all(sql, params); }
    catch (e) { throw new RunError(`SQLite: ${e.message}`); }
  }
  all(T) { return this.q(`SELECT * FROM ${qi(T.name)} ORDER BY "id"`, []).map(o => rowOf(T, o)); }
  get(T, id, silent) {
    const r = this.q(`SELECT * FROM ${qi(T.name)} WHERE "id" = ?`, [id], 'all', silent);
    return r.length ? rowOf(T, r[0]) : null;
  }
  where(T, node, env) {
    const c = node.compiled;
    const ps = c.params.map(p => toSql('lit' in p ? p.lit : evalE(p.expr, env)));
    return this.q(`SELECT * FROM ${qi(T.name)} WHERE ${c.sql} ORDER BY "id"`, ps).map(o => rowOf(T, o));
  }
  insert(T, rec) {
    checkInsert(T, rec);
    const keys = [...rec.keys()].filter(k => rec.get(k) !== null);
    const sql = keys.length
      ? `INSERT INTO ${qi(T.name)} (${keys.map(qi).join(', ')}) VALUES (${keys.map(() => '?').join(', ')})`
      : `INSERT INTO ${qi(T.name)} DEFAULT VALUES`;
    const r = this.q(sql, keys.map(k => toSqlCol(T, k, rec.get(k))), 'run');
    return this.get(T, rec.has('id') ? rec.get('id') : r.lastId, true);
  }
  update(T, id, changes) {
    checkChanges(T, changes);
    const keys = [...changes.keys()];
    if (!keys.length) return this.get(T, id);
    const r = this.q(`UPDATE ${qi(T.name)} SET ${keys.map(k => `${qi(k)} = ?`).join(', ')} WHERE "id" = ?`,
      [...keys.map(k => toSqlCol(T, k, changes.get(k))), id], 'run');
    return r.changes ? this.get(T, id, true) : null;
  }
  delete(T, id) { return this.q(`DELETE FROM ${qi(T.name)} WHERE "id" = ?`, [id], 'run').changes > 0; }
  snapshot() {
    const o = {};
    for (const T of this.P.tables.values()) o[T.name] = this.q(`SELECT * FROM ${qi(T.name)} ORDER BY "id"`, [], 'all', true).map(r => rowOf(T, r));
    return o;
  }
  close() { if (this.d.close) this.d.close(); }
}

// Same words, same semantics, kept in plain records: used when SQLite can't load.
class MemoryStore {
  constructor(P) {
    this.P = P; this.label = 'in-memory engine'; this.log = []; this.tick = -1; this.sql = false;
    this.rows = new Map(); this.next = new Map();
    for (const T of P.tables.values()) { this.rows.set(T.name, []); this.next.set(T.name, 1); }
  }
  note(text) { this.log.push({ tick: this.tick, text }); }
  norm(T, id, rec) {
    const m = new Map([['id', id]]);
    for (const c of T.cols) if (rec.has(c.name) && rec.get(c.name) !== null) m.set(c.name, rec.get(c.name));
    return m;
  }
  all(T) { this.note(`${T.name}.all`); return [...this.rows.get(T.name)]; }
  get(T, id, silent) { if (!silent) this.note(`${T.name}.get id = ${id}`); return this.rows.get(T.name).find(r => r.get('id') === id) || null; }
  where(T, node, env) {
    this.note(`${T.name}.where`);
    return this.rows.get(T.name).filter(r => {
      if (node.pattern && !matchEntries(node.pattern, r).ok) return false;
      if (!node.guard) return true;
      try { return truthy(evalE(node.guard, { subj: r, outer: [env.subj, ...env.outer], now: env.now })); }
      catch (e) { if (e instanceof RunError) return false; throw e; }
    });
  }
  insert(T, rec) {
    checkInsert(T, rec);
    const list = this.rows.get(T.name);
    const id = rec.has('id') ? rec.get('id') : this.next.get(T.name);
    if (list.some(r => r.get('id') === id)) throw new RunError(`${T.name} already has a row with id ${id}`);
    this.next.set(T.name, Math.max(this.next.get(T.name), id + 1));
    const row = this.norm(T, id, rec);
    this.rows.set(T.name, [...list, row].sort((a, b) => a.get('id') - b.get('id')));
    this.note(`${T.name}.insert ${show(row)}`);
    return row;
  }
  update(T, id, changes) {
    checkChanges(T, changes);
    this.note(`${T.name}.update id = ${id} ${show(changes)}`);
    const list = this.rows.get(T.name);
    const old = list.find(r => r.get('id') === id);
    if (!old) return null;
    const merged = new Map(old);
    for (const [k, v] of changes) { if (v === null) merged.delete(k); else merged.set(k, v); }
    const row = this.norm(T, id, merged);
    this.rows.set(T.name, list.map(r => (r === old ? row : r)));
    return row;
  }
  delete(T, id) {
    this.note(`${T.name}.delete id = ${id}`);
    const list = this.rows.get(T.name);
    if (!list.some(r => r.get('id') === id)) return false;
    this.rows.set(T.name, list.filter(r => r.get('id') !== id));
    return true;
  }
  snapshot() { const o = {}; for (const [k, v] of this.rows) o[k] = v; return o; }
  close() {}
}
function previewSql(P, stmt) {
  const T = P.tables.get(stmt.table);
  if (!T || !TABLE_OPS.includes(stmt.op)) return null;
  const t = qi(T.name);
  switch (stmt.op) {
    case 'all': return `SELECT * FROM ${t} ORDER BY "id"`;
    case 'get': return `SELECT * FROM ${t} WHERE "id" = ?`;
    case 'where': return stmt.compiled ? `SELECT * FROM ${t} WHERE ${stmt.compiled.sql} ORDER BY "id"` : null;
    case 'insert': return `INSERT INTO ${t} (…${T.single}'s fields) VALUES (…)`;
    case 'update': return `UPDATE ${t} SET (…changes) WHERE "id" = ?`;
    case 'delete': return `DELETE FROM ${t} WHERE "id" = ?`;
  }
  return null;
}
function sqljsDriver(SQL) {
  const db = new SQL.Database();
  return {
    all(sql, params) {
      const st = db.prepare(sql);
      try { st.bind(params); const rows = []; while (st.step()) rows.push(st.getAsObject()); return rows; }
      finally { st.free(); }
    },
    run(sql, params) {
      db.run(sql, params);
      const changes = db.getRowsModified();
      const r = db.exec('SELECT last_insert_rowid()');
      return { changes, lastId: r.length ? Number(r[0].values[0][0]) : null };
    },
    version() { const r = db.exec('SELECT sqlite_version()'); return r.length ? r[0].values[0][0] : '?'; },
    close() { db.close(); },
  };
}

// ---------- machine ----------
let FID = 0; // frame ids, so a trace can tell one call from the next
// A state is immutable: subject record, continuation stack k, tick, output length, status.
function initial(P, entry, store) {
  const w = P.words.get(entry);
  const pos = w ? { line: w.line, col: 0, endLine: w.line, endCol: entry.length } : null;
  return { subj: new Map(), k: [{ fid: ++FID, kind: 'seq', body: [{ k: 'call', name: entry, pos, entry: true }], i: 0, word: null, lvl: 0 }],
    tick: 0, outLen: 0, status: 'ready', brk: false, last: null,
    db: store ? store.snapshot() : null, sqlLen: store ? store.log.length : 0 };
}
const nextStmt = s => { const top = s.k[s.k.length - 1]; return top && top.kind === 'seq' && s.status !== 'done' ? top.body[top.i] : null; };
const topLvl = s => { const top = s.k[s.k.length - 1]; return top && s.status !== 'done' ? top.lvl : -1; };
function outerChain(k) {
  const out = [];
  for (let i = k.length - 1; i >= 0; i--) if (k[i].kind === 'focus' || k[i].kind === 'map') out.push(k[i].saved);
  return out;
}
const wrapItem = x => (isRec(x) ? x : new Map([['it', x]]));

function settle(k, subj) {
  for (;;) {
    if (!k.length) return { k, subj };
    const top = k[k.length - 1];
    if (top.kind === 'seq') { if (top.i < top.body.length) return { k, subj }; k = k.slice(0, -1); continue; }
    if (top.kind === 'focus') {
      k = k.slice(0, -1);
      const outer = new Map(top.saved); outer.set(top.field, subj); subj = outer;
      continue;
    }
    if (top.kind === 'map') {
      const idx = top.results.length;
      let val = subj;
      if (!isRec(top.items[idx])) {
        if (!subj.has('it')) throw new RunError(`@${top.field}( … ) over plain values must leave a field named it`);
        val = subj.get('it');
      }
      const results = [...top.results, val];
      if (results.length < top.items.length) {
        k = [...k.slice(0, -1), { ...top, results }, { fid: ++FID, kind: 'seq', body: top.body, i: 0, word: null, ctx: `@${top.field}[${results.length}]`, lvl: top.lvl + 1 }];
        subj = wrapItem(top.items[results.length]);
        continue;
      }
      k = k.slice(0, -1);
      const outer = new Map(top.saved); outer.set(top.field, results); subj = outer;
      continue;
    }
    if (top.kind === 'par') {
      const results = [...top.results, subj];
      if (results.length < top.branches.length) {
        k = [...k.slice(0, -1), { ...top, results }, { fid: ++FID, kind: 'seq', body: top.branches[results.length], i: 0, word: null, ctx: `‖ ${results.length + 1}/${top.branches.length}`, lvl: top.lvl + 1 }];
        subj = top.base;
        continue;
      }
      k = k.slice(0, -1);
      subj = mergeBranches(top.base, results);
      continue;
    }
    throw new Error('bad frame ' + top.kind);
  }
}
function mergeBranches(base, results) {
  const changes = new Map();
  const note = (key, bi, ch) => {
    const prev = changes.get(key);
    if (prev && prev.bi !== bi) throw new RunError(`both sides of ‖ change ${key} (side ${prev.bi + 1} and side ${bi + 1})`);
    changes.set(key, { bi, ...ch });
  };
  results.forEach((r, bi) => {
    for (const [key, v] of r) if (!base.has(key) || !eqv(base.get(key), v)) note(key, bi, { v });
    for (const key of base.keys()) if (!r.has(key)) note(key, bi, { del: true });
  });
  const out = new Map(base);
  for (const [key, ch] of changes) { if (ch.del) out.delete(key); else out.set(key, ch.v); }
  return out;
}
function setMap(m, key, v) { const n = new Map(m); n.set(key, v); return n; }

function stepState(P, s, log, rt) {
  if (s.status !== 'ready') return s;
  let k = s.k.slice();
  const top = k[k.length - 1];
  const stmt = top.body[top.i];
  if (top.i + 1 < top.body.length) k[k.length - 1] = { ...top, i: top.i + 1 }; else k.pop();
  const lvl = top.lvl + 1;
  const env = { subj: s.subj, now: s.tick, outer: outerChain(s.k) };
  let subj = s.subj, outLen = s.outLen, brk = false, last = { kind: stmt.k }, db = s.db;
  const store = rt && rt.store;
  const need = n => { if (!subj.has(n)) throw new RunError(`needs field ${n}`); };
  try {
    switch (stmt.k) {
      case 'call': {
        const w = P.words.get(stmt.name);
        if (!w) throw new RunError(`${stmt.name} isn't defined`);
        const ex = explainCall(P, stmt.name, env);
        const hit = ex.find(x => x.state === 'match');
        if (!hit) throw new RunError(`no clause of ${w.name} matches ${show(subj)}`, ex.map(x => `${x.clause.headSrc} — ${x.why}`));
        if (hit.take.length) { subj = new Map(subj); hit.take.forEach(n => subj.delete(n)); }
        k.push({ fid: ++FID, kind: 'seq', body: hit.clause.body, i: 0, word: w.name, clause: hit.idx, head: hit.clause.headSrc, line: hit.clause.line, lvl });
        last = { kind: 'call', word: w.name, clause: hit.idx, take: hit.take };
        break;
      }
      case 'add': {
        const n = new Map(subj);
        for (const f of stmt.fields) {
          if (n.has(f.name)) throw new RunError(`${f.name} is already there — use ${f.name}=… to change it`);
          n.set(f.name, f.expr ? evalE(f.expr, env) : true);
        }
        subj = n; break;
      }
      case 'del': {
        const n = new Map(subj);
        for (const f of stmt.names) { if (!n.has(f)) throw new RunError(`needs field ${f}`); n.delete(f); }
        subj = n; break;
      }
      case 'pick': {
        stmt.names.forEach(need);
        const n = new Map();
        for (const [key, v] of subj) if (stmt.names.includes(key)) n.set(key, v);
        subj = n; break;
      }
      case 'set': need(stmt.name); subj = setMap(subj, stmt.name, evalE(stmt.expr, env)); break;
      case 'rename':
        need(stmt.from);
        if (subj.has(stmt.to)) throw new RunError(`${stmt.to} is already there — ${stmt.from}→${stmt.to} won't overwrite it`);
        subj = new Map([...subj].map(([key, v]) => [key === stmt.from ? stmt.to : key, v])); break;
      case 'wrap': {
        if (subj.has(stmt.field)) throw new RunError(`${stmt.field} is already there — ⊂${stmt.field}{…} makes a new field`);
        stmt.names.forEach(need);
        const n = new Map();
        for (const [key, v] of subj) if (!stmt.names.includes(key)) n.set(key, v);
        n.set(stmt.field, new Map(stmt.names.map(x => [x, subj.get(x)])));
        subj = n; break;
      }
      case 'unwrap': {
        need(stmt.field);
        const inner = subj.get(stmt.field);
        if (!isRec(inner)) throw new RunError(`⊃${stmt.field} needs a record, but ${stmt.field} is ${typeName(inner)} ${show(inner)}`);
        const n = new Map(subj); n.delete(stmt.field);
        for (const [key, v] of inner) { if (n.has(key)) throw new RunError(`⊃${stmt.field}: ${key} is already there`); n.set(key, v); }
        subj = n; break;
      }
      case 'over': { const n = new Map(subj); for (const f of stmt.fields) n.set(f.name, f.expr ? evalE(f.expr, env) : true); subj = n; break; }
      case 'unify': {
        if (subj.has('conflict')) throw new RunError('conflict is already there — handle it before the next ⊔{…}');
        const vals = new Map(stmt.fields.map(f => [f.name, f.expr ? evalE(f.expr, env) : true]));
        const r = unifyRec(subj, vals);
        subj = r.conflict.size ? setMap(subj, 'conflict', r.conflict) : r.merged; break;
      }
      case 'inc': {
        need(stmt.name);
        const v = subj.get(stmt.name);
        if (typeof v !== 'number') throw new RunError(`${stmt.name} is ${typeName(v)} ${show(v)}, not a number`);
        subj = setMap(subj, stmt.name, v + stmt.n); break;
      }
      case 'focus': {
        need(stmt.field);
        const v = subj.get(stmt.field);
        if (isRec(v)) {
          k.push({ fid: ++FID, kind: 'focus', field: stmt.field, saved: subj, lvl });
          k.push({ fid: ++FID, kind: 'seq', body: stmt.body, i: 0, word: null, ctx: `@${stmt.field}`, lvl: lvl + 1 });
          subj = v;
        } else if (isList(v)) {
          if (v.length) {
            k.push({ fid: ++FID, kind: 'map', field: stmt.field, saved: subj, items: v, results: [], body: stmt.body, lvl });
            k.push({ fid: ++FID, kind: 'seq', body: stmt.body, i: 0, word: null, ctx: `@${stmt.field}[0]`, lvl: lvl + 1 });
            subj = wrapItem(v[0]);
          }
        } else throw new RunError(`@${stmt.field}( … ) needs a record or a list, but ${stmt.field} is ${typeName(v)} ${show(v)}`);
        break;
      }
      case 'par':
        k.push({ fid: ++FID, kind: 'par', base: subj, branches: stmt.branches, results: [], lvl });
        k.push({ fid: ++FID, kind: 'seq', body: stmt.branches[0], i: 0, word: null, ctx: `‖ 1/${stmt.branches.length}`, lvl: lvl + 1 });
        break;
      case 'say': log.push({ tick: s.tick, text: text(evalE(stmt.expr, env)) }); outLen = log.length; break;
      case 'show': log.push({ tick: s.tick, text: show(subj) }); outLen = log.length; break;
      case 'raise': throw new RunError(text(evalE(stmt.expr, env)));
      case 'break': brk = true; break;
      case 'tcall': {
        if (!store) throw new RunError((rt && rt.storeError) || 'tables need a database, and none is loaded');
        const T = P.tables.get(stmt.table);
        if (!T) throw new RunError(`there's no table called ${stmt.table}`);
        store.tick = s.tick;
        const free = (...keys) => keys.forEach(key => { if (subj.has(key)) throw new RunError(`${key} is already there — take it out first with -{${key}}`); });
        const idOf = () => { need('id'); const id = subj.get('id'); if (!Number.isInteger(id)) throw new RunError(`id must be a whole number, got ${typeName(id)} ${show(id)}`); return id; };
        let mutated = false;
        switch (stmt.op) {
          case 'all': free(T.name); subj = setMap(subj, T.name, store.all(T)); break;
          case 'where':
            if (!stmt.compiled) throw new RunError(`this ${T.name}.where couldn't be turned into SQL — see Notes`);
            free(T.name); subj = setMap(subj, T.name, store.where(T, stmt, env)); break;
          case 'get': { const id = idOf(); free(T.single, 'missing'); const r = store.get(T, id); subj = setMap(subj, r ? T.single : 'missing', r || true); break; }
          case 'insert': { need(T.single); const r = store.insert(T, subj.get(T.single)); subj = setMap(subj, T.single, r); mutated = true; break; }
          case 'update': {
            const id = idOf(); need('changes'); free(T.single, 'missing');
            const ch = subj.get('changes');
            const r = store.update(T, id, isDelta(ch) ? deltaChanges(ch) : ch);
            const n = new Map(subj); n.delete('changes');
            subj = setMap(n, r ? T.single : 'missing', r || true); mutated = !!r; break;
          }
          case 'delete': { const id = idOf(); free('deleted', 'missing'); const ok = store.delete(T, id); subj = setMap(subj, ok ? 'deleted' : 'missing', true); mutated = ok; break; }
          default: throw new RunError(`${stmt.table}.${stmt.op} isn't a table word`);
        }
        if (mutated) db = store.snapshot();
        last = { kind: 'tcall', table: T.name, op: stmt.op };
        break;
      }
      case 'hole':
        return { ...s, tick: s.tick + 1, status: 'hole', hole: { note: stmt.note, pos: stmt.pos }, brk: false };
    }
    const r = settle(k, subj);
    k = r.k; subj = r.subj;
  } catch (e) {
    if (!(e instanceof RunError)) throw e;
    return { ...s, tick: s.tick + 1, status: 'error', brk: false, error: { msg: e.message, details: e.details, pos: stmt.pos }, sqlLen: store ? store.log.length : s.sqlLen };
  }
  return { subj, k, tick: s.tick + 1, outLen, status: k.length ? 'ready' : 'done', brk, last, db, sqlLen: store ? store.log.length : 0 };
}

// ---------- sessions: stepping and time travel ----------
const HIST_MAX = 60000;
class Session {
  constructor(P, entry, engine) {
    this.P = P; this.entry = entry; this.log = []; this.store = null; this.storeError = null;
    let err = null;
    if (P.tables.size) {
      try {
        if (!engine) throw new RunError('tables need a database, and SQLite is still loading');
        this.store = engine.kind === 'sql' ? new SqlStore(P, engine.driver(), engine.label) : new MemoryStore(P);
        for (const sd of P.seeds) {
          const T = P.tables.get(sd.table);
          if (!T) continue;
          err = { line: sd.line };
          const rows = evalE(sd.expr, { subj: new Map(), now: 0, outer: [] });
          if (!isList(rows)) throw new RunError(`seed ${sd.table} needs a list of records, got ${typeName(rows)}`);
          rows.forEach(r => this.store.insert(T, r));
          err = null;
        }
      } catch (e) {
        if (!(e instanceof RunError)) throw e;
        this.storeError = e.message;
        err = { line: err ? err.line : [...P.tables.values()][0].line, msg: e.message };
        if (this.store && !this.store.log) this.store = null;
      }
    }
    const init = initial(P, entry, this.store);
    if (err) {
      init.status = 'error';
      init.error = { msg: err.msg, details: null, pos: { line: err.line, col: 0, endLine: err.line, endCol: (P.lines[err.line - 1] || '').length } };
    }
    this.hist = [init]; this.cur = 0; this.dropped = 0;
    this.pause = { reason: err ? 'error' : 'start', at: 0 };
  }
  dispose() { if (this.store) this.store.close(); }
  get state() { return this.hist[this.cur]; }
  get index() { return this.cur + this.dropped; }
  forward() {
    if (this.cur < this.hist.length - 1) { this.cur++; return this.state; }
    const s = this.state;
    if (s.status !== 'ready') return s;
    const n = stepState(this.P, s, this.log, this);
    this.hist.push(n); this.cur++;
    if (this.hist.length > HIST_MAX) { const drop = 10000; this.hist.splice(0, drop); this.cur -= drop; this.dropped += drop; }
    return n;
  }
  back() { if (this.cur > 0) this.cur--; this.pause = { reason: 'back', at: this.index }; }
  seek(i) { this.cur = Math.max(0, Math.min(this.hist.length - 1, i - this.dropped)); this.pause = { reason: 'seek', at: this.index }; }
  restart() { this.cur = 0; this.pause = { reason: 'start', at: this.index }; }
  run(mode, bps, budget = 200000) {
    const start = this.state;
    if (start.status !== 'ready') return;
    const L = topLvl(start);
    const stop = reason => { this.pause = { reason, at: this.index }; };
    for (let n = 1; ; n++) {
      const s = this.forward();
      if (s.status !== 'ready') return stop(s.status);
      if (s.brk) return stop('break');
      if (mode === 'into') return stop('step');
      if (mode === 'over' && topLvl(s) <= L) return stop('step');
      if (mode === 'out' && topLvl(s) < L) return stop('step');
      const nx = nextStmt(s);
      if (nx && nx.pos && bps.has(nx.pos.line)) return stop('breakpoint');
      if (n >= budget) return stop('budget');
    }
  }
}

function contextOf(k) {
  const out = [];
  for (const f of k) {
    if (f.kind === 'focus') out.push(`@${f.field}`);
    else if (f.kind === 'map') out.push(`@${f.field}[${f.results.length}]`);
    else if (f.kind === 'par') out.push(`‖ ${f.results.length + 1}/${f.branches.length}`);
  }
  return out;
}

// ---------- signature checker ----------
// Every word gets one signature, built from its body and the signatures of the words it calls.
// Nothing runs: callee signatures are applied to symbolic shapes {..r, known fields}.
// Signatures are cached by content, Salsa-style (see makeChecker).
class TErr extends Error {}
const T_NUM = { t: 'num' }, T_TEXT = { t: 'text' }, T_BOOL = { t: 'bool' }, T_NIL = { t: 'nil' }, T_ANY = { t: 'any' }, T_NEVER = { t: 'never' };
const tConst = names => ({ t: 'const', names: names ? [...new Set(names)].sort() : null });
const tList = of => ({ t: 'list', of });
const tRec = shape => ({ t: 'rec', shape });
const tVar = key => ({ t: 'var', key });
const COLT = { Int: T_NUM, Num: T_NUM, Text: T_TEXT, Bool: T_BOOL, Const: tConst(null), Json: T_ANY };
// A shape: known fields, fields known to be gone, and where unknown fields come from:
// origin '' = the word's input row, 'card' = the input's card record, null = nothing else, '*' = unknown.
const mkS = (fields = new Map(), absent = new Set(), origin = null) => ({ fields, absent, origin });
const childKey = (origin, f) => (origin === '' ? f : origin + '.' + f);
const keyName = k => k.split('.').filter(x => x !== '[]').join('.') + (k.endsWith('.[]') ? '[]' : '');
function setF(s, name, type, opt = false) { const f = new Map(s.fields); f.set(name, { type, opt }); const a = new Set(s.absent); a.delete(name); return mkS(f, a, s.origin); }
function delF(s, name) { const f = new Map(s.fields); f.delete(name); const a = new Set(s.absent); a.add(name); return mkS(f, a, s.origin); }

function litType(v) {
  if (v === null) return T_NIL;
  if (typeof v === 'number') return T_NUM;
  if (typeof v === 'string') return T_TEXT;
  if (typeof v === 'boolean') return T_BOOL;
  if (isSym(v)) return tConst([v.sym]);
  if (isList(v)) return tList(v.reduce((t, x) => joinT(t, litType(x)), T_NEVER));
  if (isRec(v)) return tRec(mkS(new Map([...v].map(([k, x]) => [k, { type: litType(x), opt: false }]))));
  return T_ANY;
}
function joinT(a, b) {
  if (a.t === 'never') return b;
  if (b.t === 'never') return a;
  if (a.t === 'var' && b.t === 'var' && a.key === b.key) return a;
  if (a.t !== b.t || a.t === 'any' || a.t === 'var') return T_ANY;
  if (a.t === 'const') { if (!a.names || !b.names) return tConst(null); const n = [...new Set([...a.names, ...b.names])]; return tConst(n.length > 8 ? null : n); }
  if (a.t === 'list') return tList(joinT(a.of, b.of));
  if (a.t === 'rec') return tRec(joinS([a.shape, b.shape]));
  return a;
}
function joinS(shapes) {
  if (shapes.length === 1) return shapes[0];
  const names = [];
  shapes.forEach(s => s.fields.forEach((_, k) => { if (!names.includes(k)) names.push(k); }));
  const fields = new Map();
  for (const k of names) {
    let type = T_NEVER, opt = false;
    for (const s of shapes) { const f = s.fields.get(k); if (!f) opt = true; else { type = joinT(type, f.type); if (f.opt) opt = true; } }
    fields.set(k, { type, opt });
  }
  const absent = new Set([...shapes[0].absent].filter(k => shapes.every(s => s.absent.has(k))));
  const o = shapes.every(s => s.origin === shapes[0].origin) ? shapes[0].origin : null;
  return mkS(fields, absent, o);
}
function meet(a, b) {
  if (b.t === 'any') return a;
  if (a.t === 'any') return b;
  if (a.t !== b.t) return null;
  if (a.t === 'const') { if (!a.names) return b; if (!b.names) return a; const n = a.names.filter(x => b.names.includes(x)); return n.length ? tConst(n) : null; }
  if (a.t === 'list') { const m = meet(a.of, b.of); return m ? tList(m) : null; }
  return a;
}

// ---- worlds: one possible situation inside a word, with what it assumes about the input ----
// facts: input key → { has, type, src: 'pre' (must hold) | 'cond' (clause pattern) | 'lift' (callee dispatch) , lit }
function mkWorld(shape) { return { shape, facts: new Map(), writes: new Set(), lifted: false, fail: null, outer: [], guardMode: false }; }
function cloneW(W, patch) { return Object.assign({ ...W, facts: new Map(W.facts), writes: new Set(W.writes) }, patch || {}); }
function rt(W, t) {
  for (let i = 0; i < 20 && t && t.t === 'var'; i++) {
    const f = W.facts.get(t.key);
    if (!f || !f.has || !f.type || f.type.t === 'any') return { t: 'any', key: t.key };
    t = f.type;
  }
  return t;
}
function lookup(W, s, name) {
  const fd = s.fields.get(name);
  if (fd) return { st: fd.opt ? 'opt' : 'present', type: fd.type };
  if (s.absent.has(name) || s.origin === null) return { st: 'absent' };
  if (s.origin === '*') return { st: 'present', type: T_ANY };
  const key = childKey(s.origin, name);
  const f = W.facts.get(key);
  if (f) return f.has ? { st: 'present', type: tVar(key), key } : { st: 'absent', key };
  return { st: 'unknown', key };
}
let HERE = null;
function need(W, s, name, where = 'the record here') {
  const r = lookup(W, s, name);
  if (r.st === 'present') return r.type;
  if (r.st === 'unknown') { W.facts.set(r.key, W.guardMode ? { has: true, type: T_ANY, src: 'cond', soft: true } : { has: true, type: T_ANY, src: 'pre', via: HERE ? [HERE] : [] }); return tVar(r.key); }
  if (r.st === 'opt') throw new TErr(`${name} might be missing — ${where} is ${fmtW(W, s)}; match on {${name}} first`);
  throw new TErr(`needs ${name} — ${where} is ${fmtW(W, s)}`);
}
function needAbsent(W, s, name) {
  const r = lookup(W, s, name);
  if (r.st === 'absent') return;
  if (r.st === 'unknown') { W.facts.set(r.key, { has: false, src: 'pre' }); return; }
  throw new TErr(`${name} ${r.st === 'opt' ? 'might already be' : 'is already'} there — the record is ${fmtW(W, s)}; use ${name}=… to change it`);
}
function constrain(W, t, want, what) {
  if (t.t === 'var') {
    const f = W.facts.get(t.key);
    const cur = f && f.type ? f.type : T_ANY;
    const m = meet(cur, want);
    if (!m) throw new TErr(`${what}: ${keyName(t.key)} is used as ${fmtT(W, want)} here, but it's ${fmtT(W, cur)}`);
    if (f) W.facts.set(t.key, { ...f, type: m });
    return m;
  }
  const r = rt(W, t);
  if (r.t === 'any') return want;
  const m = meet(r, want);
  if (!m) throw new TErr(`${what} needs ${fmtT(W, want)}, got ${fmtT(W, r)}`);
  return r;
}
function asRec(W, t, what) {
  if (t.t === 'var') {
    const f = W.facts.get(t.key);
    const cur = f && f.type ? f.type : T_ANY;
    if (cur.t === 'any') { const r = tRec(mkS(new Map(), new Set(), t.key)); if (f) W.facts.set(t.key, { ...f, type: r }); return r.shape; }
    if (cur.t === 'rec') return cur.shape;
    throw new TErr(`${what} needs a record, but ${keyName(t.key)} is ${fmtT(W, cur)}`);
  }
  if (t.t === 'rec') return t.shape;
  if (t.t === 'any') return mkS(new Map(), new Set(), '*');
  throw new TErr(`${what} needs a record, got ${fmtT(W, t)}`);
}
function asList(W, t, what) {
  if (t.t === 'var') {
    const f = W.facts.get(t.key);
    const cur = f && f.type ? f.type : T_ANY;
    if (cur.t === 'any') {
      const ek = t.key + '.[]';
      if (f) W.facts.set(t.key, { ...f, type: tList(tVar(ek)) });
      W.facts.set(ek, { has: true, type: T_ANY, src: 'pre' });
      return tVar(ek);
    }
    if (cur.t === 'list') return cur.of;
    throw new TErr(`${what} needs a list, but ${keyName(t.key)} is ${fmtT(W, cur)}`);
  }
  if (t.t === 'list') return t.of;
  if (t.t === 'any') return T_ANY;
  throw new TErr(`${what} needs a list, got ${fmtT(W, t)}`);
}
const isNumLike = (W, t) => { const r = rt(W, t); return r.t === 'num'; };
const isTextLike = (W, t) => { const r = rt(W, t); return r.t === 'text'; };

function tyOf(W, e, s) {
  switch (e.k) {
    case 'lit': return litType(e.v);
    case 'now': return T_NUM;
    case 'field': return need(W, s, e.name);
    case 'outer': {
      const o = W.outer[e.up - 1];
      if (!o) throw new TErr(`^${e.name} only works inside @field( … )`);
      return need(W, o, e.name, 'the enclosing record');
    }
    case 'get': return getKeyT(W, tyOf(W, e.obj, s), e.key);
    case 'proj': return projT(W, tyOf(W, e.obj, s), e.names);
    case 'list': return tList(e.items.reduce((t, x) => joinT(t, tyOf(W, x, s)), T_NEVER));
    case 'rec': return tRec(mkS(new Map(e.fields.map(f => [f.name, { type: tyOf(W, f.expr, s), opt: false }]))));
    case 'interp': e.parts.forEach(p => { if (typeof p !== 'string') tyOf(W, p, s); }); return T_TEXT;
    case 'neg': constrain(W, tyOf(W, e.a, s), T_NUM, 'minus'); return T_NUM;
    case 'not': tyOf(W, e.a, s); return T_BOOL;
    case 'and': case 'or': tyOf(W, e.a, s); tyOf(W, e.b, s); return T_BOOL;
    case 'fn': {
      const listArg = ['sum', 'head', 'last', 'tail', 'reverse', 'push'].includes(e.name) || ((e.name === 'min' || e.name === 'max') && e.args.length === 1);
      return fnT(W, e.name, e.args.map((a, i) => (i === 0 && listArg ? tyOfListy(W, a, s) : tyOf(W, a, s))));
    }
    case 'bin': {
      const a = tyOf(W, e.a, s), b = tyOf(W, e.b, s);
      switch (e.op) {
        case '==': case '≠': case '⊑': return T_BOOL;
        case '⫽': return tRec(mergeShapes(W, asRec(W, a, '⫽'), asRec(W, b, '⫽')));
        case '⊔': {
          const A = asRec(W, a, '⊔'), B = asRec(W, b, '⊔');
          try { requireApart(W, A, B, 'a ⊔ b'); }
          catch (e) { if (!(e instanceof TErr)) throw e; throw new TErr(`${e.message}, so ⊔ fails if they disagree — use the ⊔{…} step, which adds conflict for a clause to handle`); }
          return tRec(mergeShapes(W, A, B));
        }
        case '⊓': return meetShapeT(W, asRec(W, a, '⊓'), asRec(W, b, '⊓'));
        case '∆': return deltaT(W, asRec(W, a, '∆'), asRec(W, b, '∆'));
        case '⍎': return applyT(W, asRec(W, a, '⍎'), rt(W, b));
        case '++': { const ra = rt(W, a), rb = rt(W, b); return ra.t === 'list' && rb.t === 'list' ? tList(joinT(ra.of, rb.of)) : T_TEXT; }
        case '<': case '>': case '≤': case '≥': {
          const what = `${e.op}`;
          if (isNumLike(W, a) || isNumLike(W, b)) { constrain(W, a, T_NUM, what); constrain(W, b, T_NUM, what); }
          else if (isTextLike(W, a) || isTextLike(W, b)) { constrain(W, a, T_TEXT, what); constrain(W, b, T_TEXT, what); }
          else { const ra = rt(W, a), rb = rt(W, b); if ((ra.t !== 'any') || (rb.t !== 'any')) throw new TErr(`can't compare ${fmtT(W, ra)} with ${fmtT(W, rb)}`); }
          return T_BOOL;
        }
      }
      const ra = rt(W, a), rb = rt(W, b);
      if (e.op === '+' && (ra.t === 'text' || rb.t === 'text')) throw new TErr(`can't use + on ${fmtT(W, ra)} and ${fmtT(W, rb)} — use ++ to join text`);
      constrain(W, a, T_NUM, `${e.op}`); constrain(W, b, T_NUM, `${e.op}`);
      return T_NUM;
    }
  }
  return T_ANY;
}
function tyOfListy(W, e, s) {
  // in sum(xs.amount) an unknown xs is a list whose items have amount
  if (e.k === 'get') {
    const base = tyOf(W, e.obj, s);
    if (base.t === 'var' && rt(W, base).t === 'any') return tList(getKeyT(W, asList(W, base, ''), e.key));
    return getKeyT(W, base, e.key);
  }
  return tyOf(W, e, s);
}
function getKeyT(W, t, key) {
  const r = rt(W, t);
  if (r.t === 'never') return T_NEVER;
  if (r.t === 'list') return tList(getKeyT(W, r.of, key));
  return need(W, asRec(W, t.t === 'var' ? t : r, `.${key}`), key);
}
function fnT(W, name, args) {
  const a0 = args[0] || T_ANY;
  const num = t => constrain(W, t, T_NUM, `${name}()`);
  switch (name) {
    case 'len': { const r = rt(W, a0); if (!['list', 'text', 'rec', 'any'].includes(r.t)) throw new TErr(`len() needs a list, text or record, got ${fmtT(W, r)}`); return T_NUM; }
    case 'sum': num(asList(W, a0, 'sum()')); return T_NUM;
    case 'min': case 'max': if (args.length === 1) num(asList(W, a0, name + '()')); else args.forEach(num); return T_NUM;
    case 'abs': case 'floor': case 'ceil': case 'round': args.forEach(num); return T_NUM;
    case 'head': case 'last': return asList(W, a0, name + '()');
    case 'tail': case 'reverse': asList(W, a0, name + '()'); return a0;
    case 'push': return tList(joinT(asList(W, a0, 'push()'), args[1] || T_ANY));
    case 'range': args.forEach(num); return tList(T_NUM);
    case 'str': return T_TEXT;
    case 'keys': asRec(W, a0, 'keys()'); return tList(tConst(null));
  }
  return T_ANY;
}

// ---- obligations about fields nobody has named yet ----
// When a record comes from the caller, its unnamed fields are unknown here. Two promises about them
// travel up the signature like needs/lacks do, and are checked where the record's shape is known:
//   key#only  — the record may only have these fields (it is stored as a table row)
//   key#apart — the record shares no fields with another one (they are spliced together)
const oblKey = (origin, kind) => origin + '#' + kind;
function addOnly(W, origin, allowed, table, role) {
  // role: 'row' (stored by insert), 'changes' (given to update), 'old' (old side of a change set given to update)
  const key = oblKey(origin, 'only');
  const f = W.facts.get(key);
  W.facts.set(key, { has: true, src: 'pre', obl: 'only', only: f ? f.only.filter(x => allowed.includes(x)) : [...allowed], table: f ? f.table : table, role: f ? f.role : role });
}
function onlyMsg(name, o, what, extra, P) {
  const T = P && P.tables.get(o.table);
  const why = k => (T && T.colMap.has(k) ? `${o.table}.${k} is required` : `${o.table} has no column ${k}`);
  const list = extra.join(', ');
  if (o.role === 'old') return `${name} would clear ${list} in ${o.table}: the new side of the change set leaves ${extra.length > 1 ? 'them' : 'it'} out, and ${extra.map(why).join('; ')}`;
  return `${name} stores ${what} as ${o.role === 'row' ? `${/^[aeiou]/i.test(o.table) ? 'an' : 'a'} ${o.table} row` : `${o.table} changes`}, but it has ${list} — ${extra.map(k => `${o.table} has no column ${k}`).join('; ')}`;
}
function addApart(W, a, b, what, except) {
  if (a > b) [a, b] = [b, a];
  const key = oblKey(a, 'apart');
  const f = W.facts.get(key);
  const list = f ? f.apart.filter(x => x.other !== b) : [];
  list.push({ other: b, what, except: except || [] });
  W.facts.set(key, { has: true, src: 'pre', obl: 'apart', apart: list });
}
function requireApart(W, A, B, what, except = []) {
  // A and B must share no fields; unknown parts become promises for the caller
  if (A.origin === '*' || B.origin === '*') throw new TErr(`${what}: can't tell which fields these records have`);
  const va = shapeView(W, A), vb = shapeView(W, B);
  const ka = [...va.present.keys()].filter(k => !except.includes(k));
  const both = ka.filter(k => vb.present.has(k));
  if (both.length) throw new TErr(`${what}: both records have ${both.join(', ')}`);
  if (B.origin !== null) for (const k of ka) needAbsent(W, B, k);
  if (A.origin !== null) for (const k of vb.present.keys()) needAbsent(W, A, k);
  if (A.origin !== null && B.origin !== null) addApart(W, A.origin, B.origin, what, except);
}
function shapeAtKey(W, key) {
  if (key === '') return W.shape;
  const loc = locate(W, W.shape, key.split('.'), true);
  if (loc.st === 'present' || loc.st === 'opt') return asRec(W, loc.type, '');
  return null;
}
let CHECK_P = null;
function checkObligations(W, c, name) {
  for (const o of c.obl || []) {
    if (o.obl === 'only') {
      const sh = shapeAtKey(W, o.key);
      if (!sh) continue;
      const v = shapeView(W, sh);
      const extra = [...v.present.keys()].filter(k => !o.only.includes(k));
      if (extra.length) throw new TErr(onlyMsg(name, o, o.key ? keyName(o.key) : 'the record', extra, CHECK_P));
      if (sh.origin !== null && sh.origin !== '*') addOnly(W, sh.origin, o.only, o.table, o.role);
    } else if (o.obl === 'apart') {
      const A = shapeAtKey(W, o.key);
      for (const x of o.apart) {
        const B = shapeAtKey(W, x.other);
        if (A && B) requireApart(W, A, B, `${name} (${x.what})`, x.except);
      }
    }
  }
}

// ---- record algebra: every result shape comes from the input shapes ----
const absentIn = (S, v, k) => v.gone.has(k) || (S.origin === null && !v.present.has(k));
const recT = entries => tRec(mkS(new Map(entries.map(([k, type, opt]) => [k, { type, opt: !!opt }]))));
function mergeShapes(W, A, B) {
  const va = shapeView(W, A), vb = shapeView(W, B);
  const f = new Map(va.present);
  for (const [k, fd] of vb.present) {
    const old = f.get(k);
    f.set(k, fd.opt && old ? { type: joinT(rt(W, old.type), rt(W, fd.type)), opt: old.opt } : fd);
  }
  const absent = new Set([...va.gone, ...vb.gone].filter(k => !f.has(k) && absentIn(A, va, k) && absentIn(B, vb, k)));
  return mkS(f, absent, A.origin !== null ? A.origin : B.origin);
}
function meetShapeT(W, A, B) {
  const va = shapeView(W, A), vb = shapeView(W, B);
  const f = new Map();
  for (const [k, a] of va.present) {
    const b = vb.present.get(k);
    if (!b) continue;
    f.set(k, { type: recT([['agree', joinT(rt(W, a.type), rt(W, b.type)), true], ['differ', recT([['left', a.type], ['right', b.type]]), true]]), opt: a.opt || b.opt });
  }
  return tRec(mkS(f));
}
function deltaT(W, A, B) {
  // Δ(A → B): fields only in B are add, only in A are del, shared ones same or set
  const va = shapeView(W, A), vb = shapeView(W, B);
  const f = new Map();
  for (const k of new Set([...va.present.keys(), ...vb.present.keys()])) {
    const a = va.present.get(k), b = vb.present.get(k);
    const aSure = !!a && !a.opt, bSure = !!b && !b.opt;
    const ta = a ? a.type : T_ANY, tb = b ? b.type : T_ANY;
    let e;
    if (aSure && !b && absentIn(B, vb, k)) e = recT([['del', ta]]);
    else if (bSure && !a && absentIn(A, va, k)) e = recT([['add', tb]]);
    else if (aSure && bSure) e = recT([['same', ta, true], ['set', tb, true], ['was', ta, true]]);
    else e = recT([['same', T_ANY, true], ['set', tb, true], ['was', ta, true], ['add', tb, true], ['del', ta, true]]);
    f.set(k, { type: e, opt: !(aSure || bSure) });
  }
  const t = tRec(mkS(f, new Set(), A.origin === null && B.origin === null ? null : '*'));
  t.delta = { a: A, b: B };
  return t;
}
function applyT(W, X, d) {
  if (!(d.t === 'rec' && d.delta)) return tRec(mkS(new Map(), new Set(), '*'));
  const { a: A, b: B } = d.delta;
  const va = shapeView(W, A), vb = shapeView(W, B);
  let s = X;
  for (const [k, fa] of va.present) if (!fa.opt && !vb.present.has(k) && absentIn(B, vb, k)) s = delF(s, k);
  for (const [k, fb] of vb.present) s = setF(s, k, fb.type, fb.opt);
  return tRec(s);
}
function deltaChangesShape(W, d) {
  const { a: A, b: B } = d.delta;
  const va = shapeView(W, A), vb = shapeView(W, B);
  const f = new Map();
  // id is left out: an unchanged id is dropped at run time, and a changed one is refused there
  for (const [k, fb] of vb.present) { if (k === 'id') continue; f.set(k, { type: fb.type, opt: true }); }
  for (const [k] of va.present) if (!vb.present.has(k) && absentIn(B, vb, k)) f.set(k, { type: T_NIL, opt: true });
  return mkS(f);
}
function projT(W, t, names) {
  const one = sh => {
    const f = new Map();
    for (const n of names) {
      const l = lookup(W, sh, n);
      if (l.st === 'absent') continue;
      f.set(n, { type: l.st === 'unknown' ? T_ANY : l.type, opt: l.st !== 'present' });
    }
    return tRec(mkS(f));
  };
  const r = rt(W, t);
  if (r.t === 'list') { const el = rt(W, r.of); return tList(el.t === 'rec' ? one(el.shape) : el.t === 'never' ? T_NEVER : tRec(mkS(new Map(), new Set(), '*'))); }
  return one(asRec(W, t, `.{${names.join(' ')}}`));
}

// ---- display ----
function fmtT(W, t, d = 0) {
  t = W ? rt(W, t) : t;
  switch (t.t) {
    case 'num': return 'Num';
    case 'text': return 'Text';
    case 'bool': return 'Bool';
    case 'nil': return 'nil';
    case 'any': return 'Any';
    case 'never': return 'Never';
    case 'var': return 'Any';
    case 'const': return t.names ? t.names.join('|') : 'Const';
    case 'list': return '[' + fmtT(W, t.of, d + 1) + ']';
    case 'rec':
      if (d > 3) return '{…}';
      return t.delta ? `Δ(${fmtW(W, t.delta.a, d + 1)} → ${fmtW(W, t.delta.b, d + 1)})` : fmtW(W, t.shape, d + 1);
  }
  return '?';
}
function shapeView(W, s) {
  // the fields a shape is known to have or lack, including input facts
  const present = new Map(), gone = new Set(s.absent);
  for (const [k, f] of s.fields) present.set(k, f);
  if (W && s.origin !== null && s.origin !== '*') {
    const pre = s.origin === '' ? '' : s.origin + '.';
    for (const [k, f] of W.facts) {
      if (!k.startsWith(pre)) continue;
      const rest = k.slice(pre.length);
      if (!rest || rest.includes('.') || rest.includes('#') || f.src === 'assume') continue;
      if (present.has(rest) || gone.has(rest)) continue;
      if (f.has) present.set(rest, { type: tVar(k), opt: false }); else gone.add(rest);
    }
  }
  return { present, gone, open: s.origin !== null };
}
function fmtW(W, s, d = 0) {
  const v = shapeView(W, s);
  const parts = [];
  if (s.origin === '') parts.push('..r'); else if (s.origin === '*') parts.push('..'); else if (s.origin) parts.push('..' + keyName(s.origin).split('.').pop().slice(0, 1));
  for (const [k, f] of v.present) { const ty = fmtT(W, f.type, d); parts.push(`${k}${f.opt ? '?' : ''}${ty === 'Any' ? '' : ': ' + ty}`); }
  for (const k of [...v.gone].sort()) parts.push('¬' + k);
  return '{' + parts.join(', ') + '}';
}
function pointOf(W) {
  const v = shapeView(W, W.shape);
  return { text: fmtW(W, W.shape), present: [...v.present].filter(([, f]) => !f.opt).map(([k]) => k), gone: [...v.gone], open: v.open };
}

// ---- signatures ----
// A signature is a list of cases, in clause order. Each case: dispatch conditions on the input,
// preconditions (must hold or it's an error), outputs, the fields it writes, or a failure.
function factsOf(W) {
  const cond = [], pre = [], lacks = [], obl = [];
  for (const [key, f] of W.facts) {
    if (f.src === 'assume') continue;
    if (f.obl) { obl.push({ key: key.slice(0, key.lastIndexOf('#')), obl: f.obl, only: f.only, table: f.table, role: f.role, apart: f.apart }); continue; }
    if (f.src === 'cond' || f.src === 'lift') cond.push({ key, src: f.src, has: f.has, lit: f.lit, soft: f.soft, kind: f.kind, type: f.has ? f.type : null });
    else if (f.has) pre.push({ key, type: f.type });
    else lacks.push(key);
  }
  const byDepth = (a, b) => a.key.split('.').length - b.key.split('.').length || (a.key < b.key ? -1 : 1);
  cond.sort(byDepth); pre.sort(byDepth); lacks.sort(); obl.sort((a, b) => (a.key + a.obl < b.key + b.obl ? -1 : 1));
  return { cond, pre, lacks, obl };
}
function plainType(W, t, depth = 0) {
  // a precondition's type, without references to this word's input
  const r = rt(W, t);
  if (depth > 4) return T_ANY;
  if (r.t === 'rec') return { t: 'rec', shape: mkS(new Map(), new Set(), '*') };
  if (r.t === 'list') return tList(plainType(W, r.of, depth + 1));
  if (r.t === 'any' || r.t === 'var') return T_ANY;
  return r;
}
const condItem = (W, x) => ({ key: x.key, has: x.has, lit: x.lit, soft: x.soft, kind: x.kind, type: x.type ? plainType(W, x.type) : null });
function patternOf(W) { return factsOf(W).cond.filter(x => x.src === 'cond').map(x => condItem(W, x)); }
function caseOf(W, group) {
  const fx = factsOf(W);
  const c = {
    cond: fx.cond.filter(x => x.src === 'lift').map(x => condItem(W, x)),
    pre: fx.pre.map(x => ({ key: x.key, type: plainType(W, x.type), via: W.facts.get(x.key).via || [] })),
    lacks: fx.lacks,
    obl: fx.obl,
    outs: W.fail || W.diverge ? [] : [W.shape],
    outText: W.fail || W.diverge ? [] : [fmtOut(W, W.shape)],
    writes: [...W.writes].sort(),
    fail: W.fail ? W.fail.msg : null,
    failReported: !!(W.fail && W.fail.reported),
    failAt: W.fail ? W.fail.at : null,
  };
  c.preText = fx.pre.map(x => [x.key, fmtT(W, x.type)]);
  finishCase(c, group);
  return c;
}
const condSig = cs => cs.map(x => [x.key, x.has, x.lit === undefined ? null : show(x.lit), x.soft || false, x.kind || null, x.type ? typeSig(x.type) : null]);
function finishCase(c, group) {
  c.inText = fmtIn({ cond: [...group.pattern, ...c.cond], preText: c.preText, lacks: c.lacks, obl: c.obl });
  c.id = JSON.stringify([condSig(c.cond), c.pre.map(x => [x.key, typeSig(x.type)]), c.lacks, c.fail, c.obl || []]);
}
function fmtIn(c) {
  // input requirements as a nested record: {..r, card: {..c, balance: Num}, ok, ¬paidAt}
  const root = { kids: new Map() };
  const node = key => {
    let n = root;
    for (const seg of key.split('.')) { if (!n.kids.has(seg)) n.kids.set(seg, { kids: new Map() }); n = n.kids.get(seg); }
    return n;
  };
  for (const x of c.cond) {
    if (x.key.endsWith('#kind')) { node(x.key.slice(0, -5)).kind = x.kind; continue; }
    const n = node(x.key);
    if (!x.has) n.absent = true; else { n.present = true; if (x.lit !== undefined) n.lit = show(x.lit); }
  }
  for (const [k, ty] of c.preText || []) { const n = node(k); n.present = true; if (ty !== 'Any' && !/^\{/.test(ty)) n.type = ty; }
  for (const k of c.lacks) node(k).absent = true;
  for (const o of c.obl || []) if (o.obl === 'only') (o.key === '' ? root : node(o.key)).only = o.table;
  const render = (n, name) => {
    const parts = [name + (n.only ? ` ⊆ ${n.only}` : '')];
    for (const [k, ch] of n.kids) {
      if (ch.absent && !ch.present) { parts.push('¬' + k); continue; }
      if (k === '[]') continue;
      const elem = ch.kids.get('[]');
      if (elem && elem.kids.size) parts.push(`${k}: [${render(elem, '..')}]`);
      else if (ch.kids.size || ch.only) parts.push(`${k}: ${render(ch, '..' + k[0])}`);
      else parts.push(k + (ch.lit ? ': ' + ch.lit : ch.type ? ': ' + ch.type : ch.kind === 'list' ? ': […]' : ''));
    }
    return '{' + parts.join(', ') + '}';
  };
  const outer = [...root.kids.keys()].filter(k => /^\^\d+$/.test(k));
  const outerText = outer.map(k => { const n = root.kids.get(k); root.kids.delete(k); return `${'^'.repeat(+k.slice(1))}${render(n, '..')}`; });
  return render(root, '..r') + (outerText.length ? ' inside ' + outerText.join(' inside ') : '');
}
function fmtOut(W, s) {
  const v = shapeView({ facts: new Map([...W.facts].filter(([, f]) => f.src === 'pre' || f.src === 'cond')) }, s);
  const parts = [s.origin === '' ? '..r' : s.origin === null ? '' : '..'].filter(Boolean);
  for (const [k, f] of v.present) { const ty = fmtT(W, f.type, 1); parts.push(`${k}${f.opt ? '?' : ''}${ty === 'Any' ? '' : ': ' + ty}`); }
  return '{' + parts.join(', ') + '}';
}
function typeSig(t) {
  switch (t.t) {
    case 'const': return t.names ? 'C:' + t.names.join('|') : 'C';
    case 'list': return '[' + typeSig(t.of) + ']';
    case 'rec': return shapeSig(t.shape);
    case 'var': return '$' + t.key;
    default: return t.t;
  }
}
function shapeSig(s) {
  return '{' + [...s.fields.keys()].sort().map(k => k + (s.fields.get(k).opt ? '?' : '') + ':' + typeSig(s.fields.get(k).type)).join(',') + '|¬' + [...s.absent].sort().join(',') + '|' + s.origin + '}';
}
function simplifyCases(cases, group) {
  const outsSig = c => c.outs.map(shapeSig).sort().join('|') + '#' + [...c.writes].sort().join(',');
  for (let changed = true; changed;) {
    changed = false;
    for (let i = 0; i < cases.length && !changed; i++) for (let j = i + 1; j < cases.length && !changed; j++) {
      const a = cases[i], b = cases[j];
      if (a.fail !== b.fail || a.cond.length !== b.cond.length) continue;
      if (JSON.stringify(a.pre.map(x => [x.key, typeSig(x.type)])) !== JSON.stringify(b.pre.map(x => [x.key, typeSig(x.type)]))) continue;
      if (JSON.stringify(a.lacks) !== JSON.stringify(b.lacks) || outsSig(a) !== outsSig(b)) continue;
      if (JSON.stringify(a.obl || []) !== JSON.stringify(b.obl || [])) continue;
      const diff = a.cond.filter(x => !b.cond.some(y => y.key === x.key && y.has === x.has && y.kind === x.kind));
      if (diff.length !== 1) continue;
      const k = diff[0].key;
      const other = b.cond.find(y => y.key === k);
      if (!other || (other.has === diff[0].has && other.kind === diff[0].kind)) continue;
      if (a.cond.some(x => x.key !== k && (x.key.startsWith(k + '.') || x.key.startsWith(k + '#')))) continue;
      a.cond = a.cond.filter(x => x.key !== k);
      finishCase(a, group);
      cases.splice(j, 1);
      changed = true;
    }
  }
  return cases;
}
function groupCases(cases) {
  const out = [];
  for (const c of cases) {
    const g = out.find(x => x.id === c.id);
    if (!g) { out.push({ ...c, outs: [...c.outs], outText: [...c.outText], writes: [...c.writes] }); continue; }
    c.outs.forEach((o, i) => { if (!g.outs.some(x => shapeSig(x) === shapeSig(o))) { g.outs.push(o); g.outText.push(c.outText[i]); } });
    c.writes.forEach(w => { if (!g.writes.includes(w)) g.writes.push(w); });
  }
  return out;
}
const sigKey = sig => sig.bottom ? '⊥' : JSON.stringify(sig.groups.map(g => [g.guard, condSig(g.pattern), g.cases.map(c => [c.id, c.outs.map(shapeSig).sort(), [...c.writes].sort()])]));

// ---- applying a callee signature at a call site ----
function outerBase(W, s, path) {
  const m = /^\^(\d+)$/.exec(path[0]);
  if (!m) return { s, path };
  const o = W.outer[+m[1] - 1];
  return o ? { s: o, path: path.slice(1) } : null;
}
function locate(W, s0, path0, create) {
  // find a (possibly nested) input path inside the caller's current shape; ^n paths start at an enclosing record
  const b = outerBase(W, s0, path0);
  if (!b) return { st: 'absent' };
  const s = b.s, path = b.path;
  let t = tRec(s);
  for (let i = 0; i < path.length - 1; i++) {
    const seg = path[i];
    if (seg === '[]') { t = asList(W, t, `reading ${keyName(path.slice(0, i).join('.'))}`); continue; }
    const sh = asRec(W, t, `reading ${keyName(path.join('.'))}`);
    const r = lookup(W, sh, seg);
    if (r.st === 'unknown' && create) { W.facts.set(r.key, { has: true, type: T_ANY, src: 'pre' }); t = tVar(r.key); continue; }
    if (r.st !== 'present') return { st: r.st === 'unknown' ? 'unknown-parent' : 'absent' };
    t = r.type;
  }
  const name = path[path.length - 1];
  if (name === '[]') return { st: 'present', type: asList(W, t, `reading ${keyName(path.join('.'))}`), shape: null, name };
  const sh = asRec(W, t, `reading ${keyName(path.join('.'))}`);
  return { ...lookup(W, sh, name), shape: sh, name };
}
function rebuildAt(W, s, path, fn) {
  // apply fn to the shape that holds the last path segment, rebuilding local records along the way
  if (path.length === 1) return fn(s);
  const r = s.fields.get(path[0]);
  if (!r) return s;
  const inner = rt(W, r.type);
  if (inner.t !== 'rec') return s;
  return setF(s, path[0], tRec(rebuildAt(W, inner.shape, path.slice(1), fn)), r.opt);
}
function condSplit(R, conds) {
  let parts = [{ w: R, yes: true, exact: true }];
  for (const c of conds) {
    const kindOf = c.key.endsWith('#kind');
    const path = (kindOf ? c.key.slice(0, -5) : c.key).split('.');
    const next = [];
    for (const p of parts) {
      if (!p.yes) { next.push(p); continue; }
      let loc;
      try { loc = locate(p.w, p.w.shape, path); }
      catch (e) { if (!(e instanceof TErr)) throw e; next.push({ w: p.w, yes: false, exact: p.exact }); continue; }
      if (kindOf) {
        if (loc.st === 'unknown') {
          for (const kind of ['list', 'rec']) {
            const w = cloneW(p.w, { lifted: true });
            w.facts.set(loc.key, { has: true, type: T_ANY, src: 'lift' });
            if (kind === 'list') asList(w, tVar(loc.key), ''); else asRec(w, tVar(loc.key), '');
            w.facts.set(loc.key + '#kind', { has: true, kind, src: 'lift' });
            next.push({ w, yes: kind === c.kind, exact: p.exact });
          }
          const w = cloneW(p.w, { lifted: true });
          w.facts.set(loc.key, { has: false, src: 'lift' });
          next.push({ w, yes: c.kind === 'rec', exact: p.exact });
          continue;
        }
        if (loc.st !== 'present') { next.push({ w: p.w, yes: c.kind === 'rec', exact: p.exact }); continue; }
        const ft = rt(p.w, loc.type);
        if (ft.t === 'list' || ft.t === 'rec') { next.push({ w: p.w, yes: (ft.t === 'list') === (c.kind === 'list'), exact: p.exact }); continue; }
        if (loc.type.t === 'var') {
          for (const kind of ['list', 'rec']) {
            const w = cloneW(p.w, { lifted: true });
            if (kind === 'list') asList(w, loc.type, ''); else asRec(w, loc.type, '');
            w.facts.set(loc.type.key + '#kind', { has: true, kind, src: 'lift' });
            next.push({ w, yes: kind === c.kind, exact: p.exact });
          }
          continue;
        }
        next.push({ w: p.w, yes: c.kind === 'rec', exact: false });
        continue;
      }
      if (c.soft && loc.st === 'unknown') {
        const w = cloneW(p.w);
        w.facts.set(loc.key, { has: true, type: T_ANY, src: 'assume' });
        next.push({ w, yes: true, exact: false });
        continue;
      }
      const yesNo = (present, w) => next.push({ w, yes: present === c.has, exact: p.exact });
      if (loc.st === 'present') {
        if (!c.has) { next.push({ w: p.w, yes: false, exact: p.exact }); continue; }
        if (c.lit !== undefined) {
          const ft = rt(p.w, loc.type);
          const lt = litType(c.lit);
          if (ft.t !== 'any' && ft.t !== lt.t && c.lit !== null) { next.push({ w: p.w, yes: false, exact: p.exact }); continue; }
          if (ft.t === 'const' && ft.names && isSym(c.lit) && loc.shape && loc.shape.fields.has(loc.name)) {
            if (!ft.names.includes(c.lit.sym)) { next.push({ w: p.w, yes: false, exact: p.exact }); continue; }
            const w1 = cloneW(p.w); w1.shape = rebuildAt(w1, w1.shape, path, sh => setF(sh, loc.name, tConst([c.lit.sym])));
            next.push({ w: w1, yes: true, exact: p.exact });
            const rest = ft.names.filter(n => n !== c.lit.sym);
            if (rest.length) { const w2 = cloneW(p.w); w2.shape = rebuildAt(w2, w2.shape, path, sh => setF(sh, loc.name, tConst(rest))); next.push({ w: w2, yes: false, exact: true }); }
            continue;
          }
          next.push({ w: p.w, yes: true, exact: false });
          continue;
        }
        if (c.type && c.type.t === 'rec') {
          const ft = rt(p.w, loc.type);
          if (ft.t !== 'rec' && ft.t !== 'any') { next.push({ w: p.w, yes: false, exact: p.exact }); continue; }
        }
        next.push(p);
      } else if (loc.st === 'absent') {
        yesNo(false, p.w);
      } else if (loc.st === 'opt') {
        const w1 = cloneW(p.w); w1.shape = rebuildAt(w1, w1.shape, path, sh => setF(sh, loc.name, sh.fields.get(loc.name).type));
        const w2 = cloneW(p.w); w2.shape = rebuildAt(w2, w2.shape, path, sh => delF(sh, loc.name));
        yesNo(true, w1); yesNo(false, w2);
      } else if (loc.st === 'unknown') {
        for (const has of [true, false]) {
          const w = cloneW(p.w, { lifted: true });
          w.facts.set(loc.key, { has, type: T_ANY, src: 'lift' });
          if (has && c.lit !== undefined) next.push({ w, yes: true, exact: false });
          else yesNo(has, w);
        }
      } else next.push({ w: p.w, yes: false, exact: p.exact });
    }
    parts = next;
  }
  return parts;
}
function typeAtPath(W, base0, path0) {
  const b = outerBase(W, base0, path0);
  if (!b) return T_ANY;
  const base = b.s, path = b.path;
  if (!path.length) return tRec(base);
  let t = tRec(base);
  for (const seg of path) {
    if (seg === '[]') { t = asList(W, t, ''); continue; }
    const sh = asRec(W, t, '');
    const r = lookup(W, sh, seg);
    if (r.st === 'present' || r.st === 'opt') t = r.type;
    else if (r.st === 'unknown') { W.facts.set(r.key, { has: true, type: T_ANY, src: 'pre' }); t = tVar(r.key); }
    else return T_ANY;
  }
  return t;
}
function instType(W, base, t) {
  if (t.t === 'var') return typeAtPath(W, base, t.key.split('.'));
  if (t.t === 'rec') {
    const sh = t.shape;
    if (sh.origin !== null && sh.origin !== '*') {
      const bt = typeAtPath(W, base, sh.origin === '' ? [] : sh.origin.split('.'));
      const r = rt(W, bt);
      const bs = r.t === 'rec' ? r.shape : asRec(W, bt, '');
      return tRec(overlay(W, base, bs, sh));
    }
    const f = new Map();
    for (const [k, fd] of sh.fields) f.set(k, { type: instType(W, base, fd.type), opt: fd.opt });
    return tRec(mkS(f, new Set(sh.absent), sh.origin));
  }
  if (t.t === 'list') return tList(instType(W, base, t.of));
  return t;
}
function overlay(W, base, bs, out) {
  if (out.origin === null) {
    const f = new Map();
    for (const [k, fd] of out.fields) f.set(k, { type: instType(W, base, fd.type), opt: fd.opt });
    return mkS(f, new Set(), null);
  }
  const fields = new Map(bs.fields), absent = new Set(bs.absent);
  for (const k of out.absent) { fields.delete(k); absent.add(k); }
  for (const [k, fd] of out.fields) { fields.set(k, { type: instType(W, base, fd.type), opt: fd.opt }); absent.delete(k); }
  return mkS(fields, absent, bs.origin);
}
function checkPre(W, c, name) {
  for (const p of c.pre) {
    const path = p.key.split('.');
    const loc = locate(W, W.shape, path, true);
    const label = keyName(p.key);
    if (loc.st === 'present') {
      if (p.type.t === 'rec') asRec(W, loc.type, `${name} uses ${label} as a record, so it`);
      else if (p.type.t === 'list') asList(W, loc.type, `${name} uses ${label} as a list, so it`);
      else if (p.type.t !== 'any') {
        try { constrain(W, loc.type, p.type, ''); }
        catch (e) { if (!(e instanceof TErr)) throw e; throw new TErr(`${name} needs ${label}: ${fmtT(null, p.type)}, but it's ${fmtT(W, loc.type)}`); }
      }
    } else if (loc.st === 'unknown') {
      W.facts.set(loc.key, { has: true, type: p.type.t === 'rec' ? T_ANY : p.type, src: 'pre', via: [...(p.via || []), ...(HERE ? [HERE] : [])] });
      if (p.type.t === 'rec') asRec(W, tVar(loc.key), '');
    } else {
      const via = p.via || [];
      const reader = via.length && via[0].word !== name ? ` (${via[0].word} reads it)` : '';
      const e = new TErr(loc.st === 'opt'
        ? `${name} needs ${label}, but it might be missing — the record here is ${fmtW(W, W.shape)}${reader}`
        : `${name} needs ${label} — the record here is ${fmtW(W, W.shape)}${reader}`);
      if (via.length) {
        e.at = via[Math.min(1, via.length - 1)];
        e.topMsg = `${via[0].word} needs ${label}, but nothing on this path provides it (${[...via.map(v => v.word), name].filter((w, i, a) => a.indexOf(w) === i).join(' ← ')})`;
      }
      throw e;
    }
  }
  for (const k of c.lacks) {
    const loc = locate(W, W.shape, k.split('.'), true);
    if (loc.st === 'unknown') W.facts.set(loc.key, { has: false, src: 'pre' });
    else if (loc.st === 'present' || loc.st === 'opt') throw new TErr(`${name} adds ${keyName(k)}, but it ${loc.st === 'opt' ? 'might already be' : 'is already'} there — the record here is ${fmtW(W, W.shape)}`);
  }
  checkObligations(W, c, name);
}
function oops(C, W, pos, msg, origin) {
  // a failure that depends on assumptions about the input becomes part of the signature;
  // once it's certain, it's reported where it happens (origin), not where it was discovered
  const here = pos && C.word !== undefined ? { word: C.word, ci: C.clauseIdx, line: pos.line, col: pos.col || 0 } : null;
  const at = origin || here;
  if (W.lifted) return [cloneW(W, { fail: { msg, pos, at } })];
  if (origin && C.errAt) C.errAt(origin, C.word !== undefined ? `${msg} (when called from ${C.word})` : msg);
  else if (pos) C.err(pos, msg);
  return [cloneW(W, { fail: { msg, pos, reported: true, at } })];
}
function applySig(C, W, sig, name, pos) {
  if (!sig) return [];
  if (sig.bottom) return [cloneW(W, { diverge: true })];
  let remaining = [W];
  const out = [];
  for (const g of sig.groups) {
    const nextRem = [];
    for (const R of remaining) {
      for (const part of condSplit(R, g.pattern)) {
        if (!part.yes) { nextRem.push(part.w); continue; }
        if (g.guard || !part.exact) nextRem.push(part.w);
        for (const c of g.cases) {
          for (const q of condSplit(part.w, c.cond)) {
            if (!q.yes) continue;
            if (c.fail) {
              if (c.failReported) { if (c.failAt && C.errAt && C.reportAgain) C.errAt(c.failAt, c.fail); out.push(cloneW(q.w, { fail: { msg: c.fail, pos, reported: true, at: c.failAt } })); }
              else out.push(...oops(C, q.w, pos, c.fail, c.failAt));
              continue;
            }
            const w = cloneW(q.w);
            try {
              checkPre(w, c, name);
              if (!c.outs.length) out.push(cloneW(w, { diverge: true }));
              for (const o of c.outs) {
                const w2 = cloneW(w);
                w2.shape = overlay(w2, w.shape, w.shape, o);
                c.writes.forEach(x => w2.writes.add(x));
                out.push(w2);
              }
            } catch (e) {
              if (!(e instanceof TErr)) throw e;
              if (C.top && e.at) out.push(...oops(C, q.w, pos, e.topMsg, e.at));
              else out.push(...oops(C, q.w, pos, e.message));
            }
          }
        }
      }
    }
    remaining = nextRem;
  }
  for (const R of remaining) out.push(...oops(C, R, pos, `no clause of ${name} matches ${fmtW(R, R.shape)}`));
  return out;
}

// ---- analysing a word's clauses ----
function rowShapeT(T) {
  const m = new Map([['id', { type: T_NUM, opt: false }]]);
  T.cols.forEach(c => m.set(c.name, { type: COLT[c.type], opt: c.opt }));
  return mkS(m);
}
function runBody(C, worlds, stmts) {
  let cur = worlds;
  for (const st of stmts) {
    const live = cur.filter(w => !w.fail && !w.diverge);
    if (!live.length) break;
    C.point('before', st, live);
    HERE = st.pos ? { word: C.word, ci: C.clauseIdx, line: st.pos.line, col: st.pos.col } : null;
    const next = [];
    for (const W of cur) {
      if (W.fail || W.diverge) { next.push(W); continue; }
      HERE = st.pos ? { word: C.word, ci: C.clauseIdx, line: st.pos.line, col: st.pos.col } : null;
      next.push(...runStmt(C, W, st));
    }
    cur = widenWorlds(next);
    C.point('after', st, cur.filter(w => !w.fail && !w.diverge));
  }
  return cur;
}
function widenWorlds(ws) {
  const seen = new Map();
  for (const w of ws) {
    const k = shapeSig(w.shape) + '#' + JSON.stringify([...w.facts].map(([k2, f]) => [k2, f.has, f.src, f.type ? typeSig(f.type) : null])) + '#' + (w.fail ? w.fail.msg : '') + '#' + (w.diverge ? 'D' : '') + '#' + [...w.writes].sort().join(',');
    if (!seen.has(k)) seen.set(k, w);
  }
  const out = [...seen.values()];
  if (out.length > 64) throw new TErr('too many cases to follow here — split this word up');
  return out;
}
function runStmt(C, W0, st) {
  if (st.k === 'call') return applySig(C, W0, C.sigOf(st.name), st.name, st.pos);
  const W = cloneW(W0);
  try {
    switch (st.k) {
      case 'add': {
        const vals = st.fields.map(f => (f.expr ? tyOf(W, f.expr, W0.shape) : T_BOOL));
        let s = W.shape;
        st.fields.forEach((f, i) => { needAbsent(W, s, f.name); s = setF(s, f.name, vals[i]); W.writes.add(f.name); });
        W.shape = s; return [W];
      }
      case 'del': { let s = W.shape; for (const n of st.names) { need(W, s, n); s = delF(s, n); W.writes.add(n); } W.shape = s; return [W]; }
      case 'pick': {
        const f = new Map();
        for (const n of st.names) f.set(n, { type: need(W, W.shape, n), opt: false });
        W.shape = mkS(f, new Set(), null); W.writes.add('*'); return [W];
      }
      case 'set': { need(W, W.shape, st.name); const t = tyOf(W, st.expr, W0.shape); W.shape = setF(W.shape, st.name, t); W.writes.add(st.name); return [W]; }
      case 'inc': { const t = need(W, W.shape, st.name); constrain(W, t, T_NUM, `${st.name}${st.n < 0 ? '' : '+'}${st.n}`); W.writes.add(st.name); return [W]; }
      case 'rename': {
        const t = need(W, W.shape, st.from);
        needAbsent(W, W.shape, st.to);
        W.shape = setF(delF(W.shape, st.from), st.to, t); W.writes.add(st.from); W.writes.add(st.to); return [W];
      }
      case 'wrap': {
        needAbsent(W, W.shape, st.field);
        const f = new Map();
        let s = W.shape;
        for (const n of st.names) { f.set(n, { type: need(W, s, n), opt: false }); s = delF(s, n); W.writes.add(n); }
        W.shape = setF(s, st.field, tRec(mkS(f))); W.writes.add(st.field); return [W];
      }
      case 'unwrap': {
        const inner = asRec(W, need(W, W.shape, st.field), `⊃${st.field}`);
        const v = shapeView(W, inner);
        let s = delF(W.shape, st.field);
        W.writes.add(st.field);
        requireApart(W, s, inner, `⊃${st.field}`, [st.field]);
        for (const [k, fd] of v.present) { s = setF(s, k, fd.type, fd.opt); W.writes.add(k); }
        // a closed record that takes in an open one now has the same unknowns
        if (s.origin === null && inner.origin !== null) s = mkS(s.fields, s.absent, inner.origin);
        W.shape = s; return [W];
      }
      case 'over': {
        const vals = st.fields.map(f => (f.expr ? tyOf(W, f.expr, W0.shape) : T_BOOL));
        let s = W.shape;
        st.fields.forEach((f, i) => { s = setF(s, f.name, vals[i]); W.writes.add(f.name); });
        W.shape = s; return [W];
      }
      case 'unify': {
        // ⊔{…} either merges or, when a present field disagrees, adds conflict for the next clause
        const vals = st.fields.map(f => (f.expr ? tyOf(W, f.expr, W0.shape) : T_BOOL));
        needAbsent(W, W.shape, 'conflict');
        let s = W.shape;
        const cf = new Map();
        st.fields.forEach((f, i) => {
          const r = lookup(W, W.shape, f.name);
          if (r.st !== 'absent') cf.set(f.name, { type: recT([['left', r.type || T_ANY], ['right', vals[i]]]), opt: true });
          s = setF(s, f.name, r.st === 'present' ? r.type : vals[i]);
          W.writes.add(f.name);
        });
        W.writes.add('conflict');
        if (!cf.size) { W.shape = s; return [W]; }
        return [cloneW(W, { shape: s }), cloneW(W, { shape: setF(W.shape, 'conflict', tRec(mkS(cf))) })];
      }
      case 'say': tyOf(W, st.expr, W.shape); return [W];
      case 'raise': tyOf(W, st.expr, W.shape); W.diverge = true; return [W];
      case 'show': case 'break': return [W];
      case 'hole': W.diverge = true; return [W];
      case 'focus': return focusW(C, W, st);
      case 'par': return parW(C, W, st);
      case 'tcall': return tcallW(C, W, st);
    }
    return [W];
  } catch (e) { if (!(e instanceof TErr)) throw e; return oops(C, W0, st.pos, e.message); }
}
function focusW(C, W0, st) {
  const t0 = need(W0, W0.shape, st.field);
  const r0 = rt(W0, t0);
  if (t0.t === 'var' && r0.t === 'any') {
    const out = [];
    for (const kind of ['list', 'rec']) {
      const W = cloneW(W0, { lifted: true });
      if (kind === 'list') asList(W, t0, ''); else asRec(W, t0, '');
      W.facts.set(t0.key + '#kind', { has: true, kind, src: 'lift' });
      try { out.push(...focusKnown(C, W, st)); }
      catch (e) { if (!(e instanceof TErr)) throw e; out.push(...oops(C, W, st.pos, e.message)); }
    }
    return out;
  }
  return focusKnown(C, W0, st);
}
function focusKnown(C, W, st) {
  const t = need(W, W.shape, st.field);
  const r = rt(W, t);
  const outerSh = W.shape;
  const inner = sh => cloneW(W, { shape: sh, outer: [outerSh, ...W.outer], writes: new Set() });
  const back = (R, type) => cloneW(R, { shape: setF(outerSh, st.field, type), outer: W.outer, writes: new Set([...W.writes, st.field]) });
  if (r.t === 'list' || (t.t === 'var' && r.t === 'list')) {
    const el = asList(W, t, `@${st.field}`);
    const er = rt(W, el);
    if (er.t === 'never') return [W];
    if (er.t === 'rec' || er.t === 'any') {
      const res = runBody(C, [inner(er.t === 'rec' ? er.shape : asRec(W, el, `@${st.field}`))], st.body);
      const ok = res.filter(x => !x.fail);
      if (res.some(x => x.fail)) return res.filter(x => x.fail).map(x => cloneW(x, { outer: W.outer }));
      if (!ok.length) return [];
      const merged = cloneW(ok[0]);
      ok.slice(1).forEach(o => o.facts.forEach((f, k) => { if (!merged.facts.has(k)) merged.facts.set(k, f); }));
      return [back(merged, tList(tRec(joinS(ok.map(o => o.shape)))))];
    }
    const res = runBody(C, [inner(mkS(new Map([['it', { type: el, opt: false }]])))], st.body).filter(x => !x.fail);
    if (!res.length) return [];
    let et = T_NEVER;
    for (const o of res) { const f = o.shape.fields.get('it'); if (!f || f.opt) throw new TErr(`@${st.field}( … ) over plain values must leave a field named it`); et = joinT(et, rt(o, f.type)); }
    return [back(res[0], tList(et))];
  }
  if (r.t === 'rec' || r.t === 'any') {
    const sh = asRec(W, t, `@${st.field}`);
    return runBody(C, [inner(sh)], st.body).map(R => (R.fail ? cloneW(R, { outer: W.outer }) : back(R, tRec(R.shape))));
  }
  throw new TErr(`@${st.field}( … ) needs a record or a list, but ${st.field} is ${fmtT(W, r)}`);
}
function parW(C, W, st) {
  const res = st.branches.map(b => runBody(C, [cloneW(W, { writes: new Set() })], b));
  const ws = res.map(rs => { const s = new Set(); rs.forEach(w => w.writes.forEach(x => s.add(x))); return s; });
  for (let i = 0; i < ws.length; i++) for (let j = i + 1; j < ws.length; j++) {
    const both = [...ws[i]].filter(x => ws[j].has(x) || x === '*');
    if (both.length || ws[j].has('*')) throw new TErr(`both sides of ‖ write ${both.length && both[0] !== '*' ? both[0] : 'the whole record'} — side ${i + 1} and side ${j + 1}`);
  }
  let combos = [W];
  res.forEach((rs, bi) => {
    const next = [];
    for (const c of combos) for (const r of rs) {
      if (r.fail) { next.push(r); continue; }
      const w = cloneW(c);
      let bad = false;
      r.facts.forEach((f, k) => { const g = w.facts.get(k); if (g && g.has !== f.has) bad = true; else if (!g) w.facts.set(k, f); });
      if (bad) continue;
      let s = w.shape;
      for (const f of ws[bi]) {
        const fd = r.shape.fields.get(f);
        if (fd) s = setF(s, f, fd.type, fd.opt); else if (r.shape.absent.has(f)) s = delF(s, f);
      }
      w.shape = s;
      ws[bi].forEach(x => w.writes.add(x));
      w.lifted = w.lifted || r.lifted;
      next.push(w);
    }
    combos = widenWorlds(next);
  });
  return combos;
}
function tcallW(C, W, st) {
  const T = C.P.tables.get(st.table);
  if (!T || !TABLE_OPS.includes(st.op)) return [];
  const row = tRec(rowShapeT(T));
  const s = W.shape;
  const idT = () => constrain(W, need(W, s, 'id'), T_NUM, `${st.table}.${st.op}: id`);
  const w = (...ks) => ks.forEach(k => W.writes.add(k));
  const two = (a, b) => [cloneW(W, { shape: a }), cloneW(W, { shape: b })];
  switch (st.op) {
    case 'all': needAbsent(W, s, T.name); w(T.name); W.shape = setF(s, T.name, tList(row)); return [W];
    case 'where':
      needAbsent(W, s, T.name);
      if (st.compiled) for (const p of st.compiled.params) if (p.expr) tyOf(W, p.expr, s);
      w(T.name); W.shape = setF(s, T.name, tList(row)); return [W];
    case 'get':
      idT(); needAbsent(W, s, T.single); needAbsent(W, s, 'missing'); w(T.single, 'missing');
      return two(setF(s, T.single, row), setF(s, 'missing', T_BOOL));
    case 'insert': {
      checkRowShape(W, T, asRec(W, need(W, s, T.single), `${st.table}.insert`), false);
      w(T.single); W.shape = setF(s, T.single, row); return [W];
    }
    case 'update': {
      idT();
      const t = need(W, s, 'changes');
      const r = rt(W, t);
      if (r.t === 'rec' && r.delta) {
        checkRowShape(W, T, deltaChangesShape(W, r), true);
        const { a: A, b: B } = r.delta;
        if (A.origin !== null && A.origin !== '*' && B.origin === null)
          addOnly(W, A.origin, ['id', ...shapeView(W, B).present.keys(), ...T.cols.filter(c => c.opt).map(c => c.name)], T.name, 'old');
      }
      else checkRowShape(W, T, asRec(W, t, `${st.table}.update`), true);
      needAbsent(W, s, T.single); needAbsent(W, s, 'missing');
      w('changes', T.single, 'missing');
      const b = delF(s, 'changes');
      return two(setF(b, T.single, row), setF(b, 'missing', T_BOOL));
    }
    case 'delete':
      idT(); needAbsent(W, s, 'deleted'); needAbsent(W, s, 'missing'); w('deleted', 'missing');
      return two(setF(s, 'deleted', T_BOOL), setF(s, 'missing', T_BOOL));
  }
  return [W];
}
function checkRowShape(W, T, sh, partial) {
  if (sh.origin !== null && sh.origin !== '*')
    addOnly(W, sh.origin, [...(partial ? [] : ['id']), ...T.cols.map(c => c.name)], T.name, partial ? 'changes' : 'row');
  const v = shapeView(W, sh);
  for (const [k, f] of v.present) {
    if (k === 'id') { if (partial) throw new TErr('changes can’t include id'); continue; }
    const c = T.colMap.get(k);
    if (!c) throw new TErr(`${T.name} has no column ${k}`);
    const ft = rt(W, f.type);
    if (ft.t === 'nil') { if (!c.opt) throw new TErr(`${T.name}.${k} is required, so it can't be nil`); continue; }
    const want = COLT[c.type];
    if (want.t !== 'any') constrain(W, f.type, want, `${T.name}.${k} is ${c.type}, so ${k}`);
  }
  if (!partial && sh.origin === null) for (const c of T.cols) if (!c.opt) {
    const f = v.present.get(c.name);
    if (!f || f.opt) throw new TErr(`${T.single} ${f ? 'might be missing' : 'needs'} ${c.name} — ${T.name}.${c.name} is required`);
  }
}
function patternFacts(W, entries, origin) {
  for (const e of entries || []) {
    const key = childKey(origin, e.name);
    if (e.mode === 'absent') { W.facts.set(key, { has: false, src: 'cond' }); continue; }
    const f = { has: true, src: 'cond', type: T_ANY };
    if ('lit' in e) { f.lit = e.lit; f.type = litType(e.lit); }
    W.facts.set(key, f);
    if (e.sub) { W.facts.set(key, { ...f, type: tRec(mkS(new Map(), new Set(), key)) }); patternFacts(W, e.sub, key); }
  }
}
function analyzeWord(C, w) {
  const groups = [];
  w.clauses.forEach((c, ci) => {
    C.clauseIdx = ci;
    const W = mkWorld(mkS(new Map(), new Set(), ''));
    W.outer = [1, 2, 3].map(n => mkS(new Map(), new Set(), '^' + n));
    patternFacts(W, c.pattern, '');
    if (c.guard) {
      W.guardMode = true;
      try { tyOf(W, c.guard, W.shape); } catch (e) { if (!(e instanceof TErr)) throw e; C.err({ line: c.line, col: 0 }, `guard of ${w.name}: ${e.message}`); }
      W.guardMode = false;
    }
    let sh = W.shape;
    for (const e of c.pattern || []) if (e.mode === 'take') { sh = delF(sh, e.name); W.writes.add(e.name); }
    W.shape = sh;
    C.point('head', c, [W]);
    const group = { guard: !!c.guard, pattern: patternOf(W), cases: [], ci };
    let res;
    try { res = runBody(C, [W], c.body); }
    catch (e) { if (!(e instanceof TErr)) throw e; C.err({ line: c.line, col: 0 }, e.message); res = []; }
    if (C.flush) {
      const live = res.filter(r => !r.fail);
      const extra = new Map();
      if (live.length) for (const [k, f] of live[0].facts) if (f.src === 'pre' && live.every(r => { const g = r.facts.get(k); return g && g.src === 'pre' && g.has === f.has; })) extra.set(k, f);
      C.flush(extra);
    }
    group.cases = simplifyCases(groupCases(res.map(R => caseOf(R, group))), group);
    groups.push(group);
  });
  return { groups };
}

// ---- Salsa-style driver: content-addressed queries per group of mutually recursive words ----
function hashStr(s) { let h = 2166136261; for (let i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 16777619); } return (h >>> 0).toString(36) + ':' + s.length; }
function parseChunk(text) {
  const lines = text.split('\n');
  const toks = [];
  lines.forEach((l, i) => { const code = stripComment(l); if (code.trim()) toks.push(...lexLine(code, i + 1)); });
  const def = parseDef(new TS(toks), lines);
  walkBody(def.body, s => { if (s.k === 'tcall' && s.op === 'where') s.needsCompile = true; });
  return def;
}
function tarjan(names, edges) {
  let index = 0;
  const idx = new Map(), low = new Map(), stack = [], on = new Set(), out = [];
  const strong = v => {
    idx.set(v, index); low.set(v, index); index++; stack.push(v); on.add(v);
    for (const w of edges.get(v) || []) {
      if (!idx.has(w)) { strong(w); low.set(v, Math.min(low.get(v), low.get(w))); }
      else if (on.has(w)) low.set(v, Math.min(low.get(v), idx.get(w)));
    }
    if (low.get(v) === idx.get(v)) { const comp = []; let w; do { w = stack.pop(); on.delete(w); comp.push(w); } while (w !== v); out.push(comp); }
  };
  names.forEach(n => { if (!idx.has(n)) strong(n); });
  return out;
}
function makeChecker() {
  const astCache = new Map();
  const memo = new Map();
  const ast = (text, P) => {
    let def = astCache.get(text);
    if (!def) {
      def = parseChunk(text);
      astCache.set(text, def);
      if (astCache.size > 4000) astCache.delete(astCache.keys().next().value);
    }
    walkBody(def.body, s => {
      if (s.needsCompile) { const T = P.tables.get(s.table); if (T) { try { s.compiled = compileWhere(T, s.pattern, s.guard); } catch (e) { s.compiled = null; } } }
    });
    return def;
  };
  function check(P, entry) {
    CHECK_P = P;
    const words = new Map();
    for (const [name, w] of P.words) words.set(name, { name, clauses: w.clauses.map(c => ast(c.chunkText, P)), texts: w.clauses.map(c => c.chunkText), lines: w.clauses.map(c => c.chunkLine) });
    const edges = new Map();
    for (const [name, w] of words) {
      const cs = new Set();
      w.clauses.forEach(c => walkBody(c.body, s => { if (s.k === 'call' && words.has(s.name)) cs.add(s.name); }));
      edges.set(name, [...cs]);
    }
    const tablesKey = JSON.stringify([...P.tables.values()].map(T => [T.name, T.single, T.cols]));
    const sigs = new Map();
    const stats = { recomputed: [], reused: [] };
    const results = [];
    for (const comp of tarjan([...words.keys()], edges)) {
      comp.sort();
      const ext = new Set();
      comp.forEach(n => edges.get(n).forEach(c => { if (!comp.includes(c)) ext.add(c); }));
      const usesTables = comp.some(n => words.get(n).texts.some(t => /\.(all|get|where|insert|update|delete)\b/.test(t)));
      const keySrc = JSON.stringify([comp.map(n => [n, words.get(n).texts]), [...ext].sort().map(c => [c, sigKey(sigs.get(c))]), usesTables ? tablesKey : '']);
      const key = hashStr(keySrc) + '|' + keySrc.length;
      let r = memo.get(key);
      if (r && r.src === keySrc) stats.reused.push(...comp);
      else {
        r = solveGroup(P, comp, words, sigs);
        r.src = keySrc;
        memo.set(key, r);
        if (memo.size > 2000) memo.delete(memo.keys().next().value);
        stats.recomputed.push(...comp);
      }
      for (const n of comp) sigs.set(n, r.sigs.get(n));
      results.push(r);
    }
    // the run starts from an empty record
    const top = { P, top: true, sigOf: n => sigs.get(n), err: null, errAt: null, point() {} };
    const topErrs = [];
    top.err = (pos, msg) => topErrs.push({ pos, msg });
    top.errAt = (at, msg) => topErrs.push({ at, msg });
    if (entry && words.has(entry)) {
      const W = mkWorld(mkS(new Map(), new Set(), null));
      applySig(top, W, sigs.get(entry), entry, { line: 1, col: 0 });
    }
    // relative positions → lines in the current text
    const abs = (word, ci, rel) => words.get(word).lines[ci] + rel - 1;
    const errors = [], seen = new Set();
    const pushErr = (line, col, msg) => { const k = line + ':' + col + ':' + msg; if (!seen.has(k)) { seen.add(k); errors.push({ line, col, msg }); } };
    const before = new Map(), after = new Map(), heads = new Map();
    for (const r of results) {
      for (const e of r.errors) pushErr(abs(e.word, e.ci, e.line), e.col, e.msg);
      for (const [k, v] of r.points) {
        const [kind, word, ci, line, col] = k.split('|');
        const target = kind === 'before' ? before : kind === 'after' ? after : heads;
        const ak = kind === 'head' ? String(abs(word, +ci, +line)) : `${abs(word, +ci, +line)}:${col}`;
        target.set(ak, v);
      }
    }
    for (const e of topErrs) {
      if (e.at && words.has(e.at.word)) pushErr(abs(e.at.word, e.at.ci, e.at.line), e.at.col, / \(.* ← /.test(e.msg) ? `${e.msg}, when ${entry} runs` : `${e.msg} (when ${entry} runs)`);
      else pushErr(words.get(entry).lines[0], 0, `running ${entry} from an empty record: ${e.msg}`);
    }
    errors.sort((a, b) => a.line - b.line || a.col - b.col);
    const signatures = new Map();
    for (const [n, s] of sigs) signatures.set(n, s && !s.bottom ? s.groups.flatMap(g => g.cases.map(c => ({ in: c.inText, out: c.fail ? null : [...new Set(c.outText)], fail: c.failReported ? 'stops with an error (see Notes)' : c.fail, guard: g.guard, clause: g.ci }))) : []);
    return { errors, before, after, heads, signatures, stats, entry, groups: results.length, raw: sigs };
  }
  return { check };
}
function solveGroup(P, comp, words, sigs) {
  const local = new Map(comp.map(n => [n, { bottom: true }]));
  let C;
  for (let iter = 0; iter < 25; iter++) {
    C = {
      P, errors: [], points: new Map(),
      sigOf: n => (local.has(n) ? local.get(n) : sigs.get(n)),
      reportAgain: true,
      err(pos, msg) { this.errors.push({ word: this.word, ci: this.clauseIdx, line: pos.line, col: pos.col || 0, msg }); },
      errAt(at, msg) { this.errors.push({ word: at.word, ci: at.ci, line: at.line, col: at.col, msg }); },
      pending: [],
      point(kind, node, ws) { for (const w of ws) this.pending.push({ kind, node, w, word: this.word, ci: this.clauseIdx }); },
      flush(extra) {
        // callers check a clause's preconditions before entering it, so they hold at every step inside
        for (const { kind, node, w, word, ci } of this.pending) {
          const pos = kind === 'head' ? { line: node.line, col: 0 } : node.pos;
          if (!pos) continue;
          const k = `${kind}|${word}|${ci}|${pos.line}|${pos.col}`;
          const list = this.points.get(k) || [];
          const ww = extra.size ? { ...w, facts: new Map([...extra, ...w.facts]) } : w;
          const pt = pointOf(ww);
          if (!list.some(x => x.text === pt.text)) list.push(pt);
          this.points.set(k, list);
        }
        this.pending = [];
      },
    };
    let changed = false;
    const next = new Map();
    for (const n of comp) {
      C.word = n;
      const sig = analyzeWord(C, words.get(n));
      next.set(n, sig);
      if (sigKey(sig) !== sigKey(local.get(n))) changed = true;
    }
    next.forEach((v, k) => local.set(k, v));
    if (!changed) break;
  }
  return { sigs: local, errors: C.errors, points: C.points };
}

const api = { parseProgram, Session, stepState, initial, nextStmt, topLvl, explainCall, outerChain, contextOf,
  SqlStore, MemoryStore, sqljsDriver, previewSql, compileWhere, TABLE_OPS, COL_TYPES,
  makeChecker, walkBody,
  show, text, eqv, isRec, isList, isSym, stripComment, FUNCS, KEYWORDS };
if (typeof module !== 'undefined' && module.exports) module.exports = api;
else root.Forthy = api;
})(typeof globalThis !== 'undefined' ? globalThis : this);
