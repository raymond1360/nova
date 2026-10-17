"""Read only the explicitly isolated run-07 database; write audit evidence locally."""
from pathlib import Path
import json, sqlite3, hashlib, difflib, subprocess

REPO = Path(r'D:\nova')
ROOT = Path(__file__).resolve().parent
RUN = ROOT / 'run-07'
def load(p): return json.loads(p.read_text(encoding='utf-8-sig'))
def lines(p): return [json.loads(s) for s in p.read_text(encoding='utf-8-sig').splitlines() if s]
def save(p, value): p.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding='utf-8')
def sha(p): return hashlib.sha256(p.read_bytes()).hexdigest()
def resolved(p): return Path(str(p).removeprefix(chr(92)*2+'?'+chr(92))).resolve()
checks = []
def check(name, value):
    checks.append({'assertion':name, 'passed':bool(value)})
    assert value, name

driver = lines(RUN / 'driver-result.jsonl')[-1]
check('nine real WebView DOM/IPC scene groups passed', driver['completed'] and len(driver['results']) == 9 and all(r['passed'] for r in driver['results']))
proof = lines(RUN / 'startup-proof.jsonl')[0]
dbpath = RUN / 'app-data/nova.sqlite3'
check('database is exactly inside this new audit run', resolved(proof['resolved_database']) == dbpath.resolve())
check('WebView user data is exactly isolated', resolved(proof['resolved_webview']) == (RUN / 'app-data/webview').resolve())
check('fake binary pinned hash matches after run', sha(RUN / 'tools/nova_wp2a_fake_cli.exe') == proof['fake_sha256'])
boundary = lines(RUN / 'process-boundary.jsonl')
allowed = {str(resolved(proof['fake_cli'])).lower(), str(resolved(proof['git'])).lower()}
check('all spawned runtime commands use only pinned fake or git', bool(boundary) and all(r['allowed'] and str(resolved(r['program'])).lower() in allowed for r in boundary))
check('all spawned runtime working directories stay isolated', all(resolved(r['cwd']).is_relative_to(RUN.resolve()) for r in boundary))
fake = lines(RUN / 'fake-cli.jsonl')
development = [r for r in fake if 'stdin' in r]
check('exactly four fake development calls; no automatic Case C AI call', len(development) == 4)
check('each fake development reads actual snapshot and output contract', all('===NOVA_REQUIREMENT_ITEMS_START===' in r['stdin'] and '===NOVA_SELF_CHECK_START===' in r['stdin'] for r in development))

conn = sqlite3.connect(dbpath.as_uri() + '?mode=ro', uri=True)
conn.row_factory = sqlite3.Row
tables = ['tasks','confirmed_requirement_items','self_checks','task_actions','test_evidence','build_evidence','no_tests_confirmations','no_build_confirmations','user_acceptance','task_state_events']
after = {t:[dict(r) for r in conn.execute(f'SELECT * FROM {t} ORDER BY rowid')] for t in tables}
check('isolated database integrity_check', conn.execute('PRAGMA integrity_check').fetchone()[0] == 'ok')
check('isolated database foreign_key_check', not conn.execute('PRAGMA foreign_key_check').fetchall())
backup = sqlite3.connect(RUN / 'database-after-consistent.sqlite3')
conn.backup(backup)
backup.close()
conn.close()
save(RUN / 'database-after.json', after)
before = load(RUN / 'database-before.json')
task = lambda prefix: next(t for t in after['tasks'] if t['title'].startswith(prefix))
rows = lambda table, tid, db=after: [r for r in db[table] if r['task_id'] == tid]
normal = task('01 ')
check('normal fake result completed with exactly one acceptance', normal['state'] == 'completed' and len(rows('user_acceptance', normal['id'])) == 1)
check('normal actual persisted self-check is valid and complete', all(r['parse_status']=='valid' and r['self_reported_status']=='self_reported_complete' for r in rows('self_checks', normal['id'])))
for prefix in ['02 ','03 ','04 ']:
    t = task(prefix)
    check(prefix+' rejected completion and wrote no acceptance', t['state']=='awaiting_user_acceptance' and not rows('user_acceptance', t['id']))
casec = task('06 ')
check('Case C remains Queued after all subsequent scenarios', casec['state']=='queued')
check('Case C confirmation and all action records unchanged', rows('no_build_confirmations',casec['id']) == rows('no_build_confirmations',casec['id'],before) and rows('task_actions',casec['id']) == rows('task_actions',casec['id'],before))
frozen = task('08 ')
check('frozen cancellation preserves classification and writes no acceptance', frozen['state']=='cancelled' and frozen['legacy_classification']=='unknown_insufficient' and not rows('user_acceptance',frozen['id']))
check('new generation remains unaccepted', task('09 ')['result_generation']==2 and not rows('user_acceptance',task('09 ')['id']))
check('backfill creates exactly six items', len(rows('confirmed_requirement_items',task('05 ')['id']))==6)
check('DOM-created confirmed task contains six items', len(rows('confirmed_requirement_items',task('UI CREATED')['id']))==6)

checkpoints = lines(RUN / 'checkpoints.jsonl')
out = RUN / 'checkpoints'
out.mkdir(exist_ok=True)
for c in checkpoints:
    name = c['checkpoint']['name']
    (out / (name+'.txt')).write_text(c['checkpoint']['dom'], encoding='utf-8')
    save(out / (name+'.json'), c)
readfail = next(c for c in checkpoints if c['checkpoint']['name']=='07-read-failure')
check('actual SQLite failure displays both read errors', '完工预检读取失败' in readfail['checkpoint']['dom'] and '自检记录读取失败' in readfail['checkpoint']['dom'])

baseline = load(ROOT / 'baseline-manifest.json')
delta = [r['path'] for r in baseline if sha(REPO / r['path']) != r['sha256']]
expected = ['apps/desktop/src-tauri/Cargo.toml','apps/desktop/src-tauri/src/lib.rs','apps/desktop/src-tauri/src/runtime/process.rs']
check('only three pre-existing files changed from this-turn checkpoint', sorted(delta)==sorted(expected))
new = ['apps/desktop/src-tauri/src/isolated_validation.rs','apps/desktop/src-tauri/tests/fixtures/wp2a_validation_main.rs','apps/desktop/src-tauri/tests/fixtures/wp2a_fake_cli.rs','apps/desktop/src-tauri/tests/fixtures/wp2a_validation_driver.js']
patch = []
for rel in expected + new:
    old = (ROOT / 'before' / rel).read_text(encoding='utf-8-sig').splitlines(keepends=True) if rel in expected else []
    current = (REPO / rel).read_text(encoding='utf-8-sig').splitlines(keepends=True)
    patch.extend(difflib.unified_diff(old,current,fromfile='a/'+rel if old else '/dev/null',tofile='b/'+rel))
(ROOT / 'this-turn-source.patch').write_bytes(''.join(patch).encode('utf-8'))
save(ROOT / 'source-delivery-manifest.json',[{'path':rel,'sha256':sha(REPO/rel),'boundary':'modified this turn' if rel in expected else 'new this turn'} for rel in expected+new])
save(ROOT / 'baseline-preservation-verification.json', {'changed':delta,'unchanged_count':len(baseline)-len(delta),'expected_changed':expected})

release = REPO / 'apps/desktop/src-tauri/target/release/nova.exe'
needles = [b'validation_control',b'ISOLATED-VALIDATION.json',b'NOVA_WP2A_FAKE_CLI_NO_NETWORK_NO_SPAWN_V1',b'WP2A ISOLATED FIXTURE']
binary = release.read_bytes()
check('normal release binary contains no validation command/driver identity', all(n not in binary and n.decode().encode('utf-16le') not in binary for n in needles))
check('release feature intentionally refuses compilation', (ROOT/'release-feature-exit-code.txt').read_text(encoding='utf-8-sig').strip()=='101' and 'debug-only and must never ship' in (ROOT/'release-feature-must-fail.log').read_text(encoding='utf-8-sig'))
prior = (ROOT/'before/apps/desktop/src-tauri/src/lib.rs').read_text(encoding='utf-8-sig')
current = (REPO/'apps/desktop/src-tauri/src/lib.rs').read_text(encoding='utf-8-sig')
import re
check('production command registration list identical to reviewed delivery', re.findall(r'commands::\w+',prior)==re.findall(r'commands::\w+',current))
save(ROOT/'build-artifact-hashes.json',[{'path':str(p.relative_to(REPO)), 'sha256':sha(p)} for p in [release,REPO/'apps/desktop/src-tauri/target/release/bundle/nsis/Nova_0.1.0_x64-setup.exe',REPO/'apps/desktop/src-tauri/target/debug/nova_wp2a_validation.exe',REPO/'apps/desktop/src-tauri/target/debug/nova_wp2a_fake_cli.exe']])
save(ROOT/'evidence-assertions.json', {'assertions':checks,'process_count':len(boundary),'fake_development_calls':len(development),'checkpoint_count':len(checkpoints),'driver':driver,'screenshots_captured':0})
print(json.dumps({'assertions_passed':len(checks),'process_count':len(boundary),'fake_development_calls':len(development),'checkpoints':len(checkpoints)},ensure_ascii=False))
