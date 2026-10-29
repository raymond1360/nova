'use strict'
// Deterministic contract + TS syntax only. No module execution, DB, framework or production build.
const fs = require('node:fs')
const path = require('node:path')
const crypto = require('node:crypto')
const { createRequire } = require('node:module')
const hash = b => crypto.createHash('sha256').update(b).digest('hex')
const contract = JSON.parse(fs.readFileSync(path.join(__dirname, 'logo-contract.json'), 'utf8'))
function oracle(pre) {
  if (hash(pre) !== contract.pre_sha256) throw Error('preimage mismatch')
  const old = Buffer.from(contract.old_literal)
  const at = pre.indexOf(old)
  if (at < 0 || pre.indexOf(old, at + 1) >= 0) throw Error('ambiguous literal')
  return Buffer.concat([pre.subarray(0, at), Buffer.from(contract.new_literal), pre.subarray(at + old.length)])
}
function parseSyntax(ts, actual) {
  const ast = ts.createSourceFile('Logo.tsx', actual.toString('utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
  if (ast.parseDiagnostics.length) throw Error('TSX syntax failure')
  return ast
}
function check(ts, pre, actual) {
  const expected = oracle(pre)
  if (!actual.equals(expected) || actual.length !== 795 || hash(actual) !== contract.post_sha256) throw Error('byte oracle mismatch')
  const ast = parseSyntax(ts, actual)
  let matching = 0
  function visit(n) {
    if (ts.isJsxAttribute(n) && n.name.getText(ast) === 'alt' && n.initializer && ts.isStringLiteral(n.initializer)
      && n.initializer.text === 'Northstar Studio logo') matching++
    ts.forEachChild(n, visit)
  }
  visit(ast)
  if (matching !== 1) throw Error('alt AST contract mismatch')
  return { scope: 'deterministic contract and TSX parse only; not typecheck, lint, DOM or production build', bytes: actual.length, sha256: hash(actual) }
}
function main(args) {
  if (process.version !== 'v24.14.0' || process.arch !== 'x64') throw Error('fixed Node version/arch required')
  const policy = JSON.parse(fs.readFileSync(path.join(__dirname, 'opencode-policy.json'), 'utf8'))
  function policyTest() {
    const controls = ['429', 'rate_limit', 'provider_returned_error', 'socket hang up', 'read timed out', 'retry your request', 'temporarily at capacity']
    if (policy.retry_patterns.length !== 7 || policy.provider_id.startsWith('openai')) throw Error('retry contract identity')
    policy.retry_patterns.forEach((p, i) => { const r = new RegExp(p.source, p.flags); if (r.test(policy.denial_body) || !r.test(controls[i])) throw Error('fixed denial retry-rule regression') })
    if (/[0-9]/.test(policy.denial_body) || policy.non_api_terms.some(t => policy.denial_body.toLowerCase().includes(t))) throw Error('denial contains retry hint')
    return {retry_patterns: 7, non_api_terms: 3, source_sha256: policy.retry_source.sha256, scope: 'copied locked rules / SYNTHETIC; not SDK runtime behavior'}
  }
  if (args.length === 1 && args[0] === '--policy-self-test') { console.log(JSON.stringify(policyTest())); return }
  if (args.length !== 2 && args.length !== 3) throw Error('usage: check-logo.cjs <verifier-package.json> <--self-test|actual-file> [preimage-file]')
  const pkg = path.resolve(args[0]); if (!path.isAbsolute(args[0]) || path.basename(pkg) !== 'package.json') throw Error('absolute verifier package required')
  const lock = fs.readFileSync(path.join(path.dirname(pkg), 'package-lock.json'))
  if (hash(lock) !== contract.desktop_lock_blob_sha256) throw Error('verifier lock blob mismatch')
  const req = createRequire(pkg)
  const resolved = fs.realpathSync(req.resolve('typescript'))
  const approved = fs.realpathSync(path.join(path.dirname(pkg), 'node_modules', 'typescript')) + path.sep
  if (!resolved.startsWith(approved)) throw Error('TypeScript outside isolated verifier deps')
  const ts = req('typescript'); if (ts.version !== '5.9.3') throw Error('TypeScript version mismatch')
  const pre = fs.readFileSync(args[2] || path.join(__dirname, 'fixtures', 'Logo.tsx'))
  if (args[1] === '--self-test') {
    const expected = oracle(pre); check(ts, pre, expected)
    for (const invalid of [pre, expected.subarray(0, -1), Buffer.concat([expected, Buffer.from('\n')]), Buffer.from(expected.toString().replaceAll('\n', '\r\n')), Buffer.from(expected.toString().replace('Northstar Studio logo', 'Northstar'))]) {
      let rejected = false; try { check(ts, pre, invalid) } catch { rejected = true }
      if (!rejected) throw Error('negative oracle accepted')
    }
    let syntaxRejected = false
    try { parseSyntax(ts, Buffer.from('export const broken = <img alt="unterminated')) } catch { syntaxRejected = true }
    if (!syntaxRejected) throw Error('invalid TSX syntax accepted')
    console.log(JSON.stringify({ ...check(ts, pre, expected), typescript_version: ts.version, typescript_resolved: resolved, synthetic: true, cases: 6, syntax_negative_cases: 1, policy: policyTest() }))
  } else console.log(JSON.stringify(check(ts, pre, fs.readFileSync(args[1]))))
}
try { main(process.argv.slice(2)) } catch (e) { console.error(e.message); process.exitCode = 1 }
