import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createServer } from 'node:net';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const directory = fileURLToPath(new URL('.', import.meta.url));
const cli = `${directory}node_modules/.bin/quint`;
const fixture = name => `runner-fixtures/${name}.qnt`;
const simulation = ['--backend=typescript', '--seed=1', '--max-samples=1', '--max-steps=2', '--invariant=safety'];
const runner = args => spawnSync('bash', ['verify.sh', ...args], { cwd: directory, encoding: 'utf8', env: process.env });

test('missing CLI makes the unified runner fail before any success summary', () => {
  const result = spawnSync('bash', ['verify.sh'], {
    cwd: directory, encoding: 'utf8', env: { ...process.env, QUINT_BIN: `${directory}missing-quint` },
  });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Quint CLI unavailable/);
  assert.doesNotMatch(result.stdout, /portable checks passed/);
});

test('invalid model fails as a positive check', async () => {
  const { runQuint } = await import('./verify.mjs');
  assert.throws(() => runQuint(cli, ['parse', fixture('invalid')], { quiet: true }), /positive check failed/);
});

test('a genuine invariant failure fails a positive check', async () => {
  const { runQuint } = await import('./verify.mjs');
  assert.throws(() => runQuint(cli, ['run', fixture('violation'), ...simulation], { quiet: true }), /positive check failed/);
});

test('a parser error cannot satisfy an expected invariant counterexample', async () => {
  const { runQuint } = await import('./verify.mjs');
  assert.throws(() => runQuint(cli, ['run', fixture('invalid'), ...simulation], { negative: 'invariant', quiet: true }), /expected invariant failure/);
});

test('expected invariant and assertion controls require their real diagnostics', async () => {
  const { runQuint } = await import('./verify.mjs');
  const invariant = runQuint(cli, ['run', fixture('violation'), ...simulation], { negative: 'invariant', quiet: true });
  const assertion = runQuint(cli, ['test', fixture('pricing_negative'), '--backend=typescript', '--seed=1', '--max-samples=1'], { negative: 'assertion', quiet: true });
  assert.equal(invariant.status, 1);
  assert.match(invariant.stderr, /Invariant violated/);
  assert.equal(assertion.status, 1);
  assert.match(assertion.stdout + assertion.stderr, /QNT508.*Assertion failed/);
});

test('a successful process containing a diagnostic does not satisfy a negative control', async () => {
  const { requireExpectedFailure } = await import('./verify.mjs');
  assert.throws(() => requireExpectedFailure({ status: 0, stdout: 'error: Invariant violated', stderr: '' }, 'invariant'), /expected invariant failure/);
});

test('an unrelated CLI error cannot hide behind an expected counterexample diagnostic', async () => {
  const { requireExpectedFailure } = await import('./verify.mjs');
  assert.throws(() => requireExpectedFailure({
    status: 1,
    stdout: '[violation] Found an issue\nerror: Invariant violated\n',
    stderr: "Error [QNT404]: Name 'missingName' not found\nerror: parsing failed\n",
  }, 'invariant'), /expected invariant failure/);
});

test('a Node permission error cannot hide behind an expected negative diagnostic', async () => {
  const { requireExpectedFailure } = await import('./verify.mjs');
  assert.throws(() => requireExpectedFailure({
    status: 1,
    stdout: '[violation] Found an issue (3ms at 333 traces/second).\n',
    stderr: 'error: Invariant violated\nError: EACCES: permission denied, open counterexample.itf.json\n',
  }, 'invariant'), /expected invariant failure/);
  assert.throws(() => requireExpectedFailure({
    status: 1,
    stdout: '1 failed\nError [QNT508]: Assertion failed\nError: EACCES: permission denied\n',
    stderr: 'error: Tests failed\n',
  }, 'assertion'), /expected assertion failure/);
});

test('explicit extended mode fails when its server is unavailable', async () => {
  // Select a free local port, then close it. No backend is started or changed.
  const server = createServer();
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  const result = spawnSync('bash', ['verify.sh', '--extended'], {
    cwd: directory, encoding: 'utf8', env: { ...process.env, APALACHE_SERVER: `127.0.0.1:${port}` },
  });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /extended backend unavailable/);
  assert.doesNotMatch(result.stdout, /extended check passed|portable checks passed/);
});

test('unknown runner options fail instead of silently running another mode', () => {
  const result = runner(['--typo']);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Usage:/);
});
