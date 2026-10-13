import { spawnSync } from 'node:child_process';
import { accessSync, constants, mkdirSync, mkdtempSync } from 'node:fs';
import { connect } from 'node:net';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const directory = dirname(fileURLToPath(import.meta.url));
const models = ['pricing.qnt', 'pricing_tests.qnt', 'native_async.qnt', 'native_async_tests.qnt',
  'native_profiles.qnt', 'service_waits.qnt', 'service_waits_tests.qnt', 'ibc_waits.qnt', 'ibc_waits_tests.qnt'];
const suites = ['pricing_tests.qnt', 'native_async_tests.qnt', 'service_waits_tests.qnt', 'ibc_waits_tests.qnt'];
const deterministic = ['--backend=typescript', '--seed=1', '--max-samples=1'];

export function requireExpectedFailure(result, kind) {
  const output = (result.stdout ?? '') + (result.stderr ?? '');
  const stderr = (result.stderr ?? '').trim();
  const stdoutErrors = (result.stdout ?? '').split(/\r?\n/).map(line => line.trim())
    .filter(line => /^(?:error:|[A-Za-z]*Error(?: \[QNT\d+\])?:|E[A-Z]+:|node:internal)/.test(line));
  const expectedStderr = kind === 'invariant' ? 'error: Invariant violated' : 'error: Tests failed';
  const expectedStdoutErrors = kind === 'invariant' ? [] : ['Error [QNT508]: Assertion failed'];
  if (result.error || result.signal || result.status !== 1 ||
      stderr !== expectedStderr || stdoutErrors.length !== expectedStdoutErrors.length ||
      expectedStdoutErrors.some(line => !stdoutErrors.includes(line))) {
    throw new Error(`expected ${kind} failure (exit 1 plus diagnostic), got status ${result.status}\n${output}`);
  }
}

export function runQuint(cli, args, { negative, quiet = false } = {}) {
  const result = spawnSync(cli, args, { cwd: directory, encoding: 'utf8', maxBuffer: 16 * 1024 * 1024 });
  if (negative) requireExpectedFailure(result, negative);
  else if (result.error || result.signal || result.status !== 0) {
    throw new Error(`positive check failed: quint ${args.join(' ')} (status ${result.status})\n${result.error ?? ''}\n${result.stdout}${result.stderr}`);
  }
  if (!quiet) {
    process.stdout.write(result.stdout);
    process.stderr.write(result.stderr);
  }
  return result;
}

async function requireBackend(endpoint) {
  const match = /^([^:]+):(\d+)$/.exec(endpoint ?? '');
  if (!match || Number(match[2]) < 1 || Number(match[2]) > 65535) {
    throw new Error('extended backend unavailable: set APALACHE_SERVER=host:port for an existing Apalache server');
  }
  await new Promise((accept, reject) => {
    const socket = connect({ host: match[1], port: Number(match[2]) });
    const fail = error => { socket.destroy(); reject(new Error(`extended backend unavailable at ${endpoint}: ${error.message}`)); };
    socket.setTimeout(2000, () => fail(new Error('connection timed out')));
    socket.once('error', fail);
    socket.once('connect', () => { socket.destroy(); accept(); });
  });
}

async function main(args) {
  if (args.length > 1 || (args.length === 1 && !['--extended', '--parse', '--typecheck', '--tests'].includes(args[0]))) {
    throw new Error('Usage: bash formal/quint/verify.sh [--extended|--parse|--typecheck|--tests]');
  }
  const mode = args[0] ?? '--portable';
  const cli = resolve(process.env.QUINT_BIN ?? join(directory, 'node_modules/.bin/quint'));
  try { accessSync(cli, constants.X_OK); } catch { throw new Error(`Quint CLI unavailable: ${cli}; run npm ci in formal/quint`); }
  const version = runQuint(cli, ['--version'], { quiet: true }).stdout.trim();
  if (version !== '0.33.0') throw new Error(`expected pinned Quint 0.33.0, got ${version}`);
  if (mode === '--extended') await requireBackend(process.env.APALACHE_SERVER);
  console.log(`Quint ${version}; Node ${process.version}; TypeScript evaluator`);

  if (mode === '--parse' || mode === '--typecheck') {
    for (const model of models) { runQuint(cli, [mode.slice(2), model], { quiet: true }); console.log(`${mode.slice(2)} ${model}: passed`); }
    return;
  }
  if (mode === '--tests') {
    for (const suite of suites) runQuint(cli, ['test', suite, ...deterministic]);
    return;
  }
  for (const model of models) {
    runQuint(cli, ['parse', model], { quiet: true });
    runQuint(cli, ['typecheck', model], { quiet: true });
    console.log(`parse/typecheck ${model}: passed`);
  }
  for (const suite of suites) runQuint(cli, ['test', suite, ...deterministic]);
  const profiles = [
    ['native_profiles.qnt', [], 20261012, 1000, ['sawPoll', 'sawAccepted', 'sawRollback']],
    ['native_profiles.qnt', ['--init=fixedInit'], 20261013, 500, ['sawPoll', 'sawAccepted', 'sawRollback']],
    ['service_waits.qnt', [], 20261014, 500, ['sawAccepted', 'sawRollback', 'sawRestart', 'sawAmbiguous']],
    ['ibc_waits.qnt', [], 20261015, 500, ['sawAccepted', 'sawRollback', 'sawRestart', 'sawEarlyTimeout']],
  ];
  for (const [model, extra, seed, samples, witnesses] of profiles) {
    console.log(`simulate ${model} ${extra.join(' ')} seed=${seed} samples=${samples} depth=80`);
    runQuint(cli, ['run', model, '--backend=typescript', ...extra, '--invariant=safety', '--witnesses', ...witnesses,
      `--seed=${seed}`, `--max-samples=${samples}`, '--max-steps=80', '--verbosity=1']);
  }
  const output = process.env.QUINT_RESULTS_DIR ? resolve(process.env.QUINT_RESULTS_DIR) : mkdtempSync(join(tmpdir(), 'cosmwasm-quint-results-'));
  mkdirSync(output, { recursive: true });
  runQuint(cli, ['test', 'runner-fixtures/pricing_negative.qnt', ...deterministic], { negative: 'assertion', quiet: true });
  console.log('negative pricing: expected QNT508 assertion failure (exit 1)');
  for (const [name, model, init, step] of [
    ['native-flight', 'native_profiles.qnt', 'weakFlightInit', 'weakFlightStep'],
    ['native-restore', 'native_profiles.qnt', 'weakRestoreInit', 'weakRestoreStep'],
    ['service-sequence', 'service_waits.qnt', 'weakSequenceInit', 'weakSequenceStep'],
    ['ibc-host', 'ibc_waits.qnt', 'weakHostInit', 'weakHostStep'],
    ['ibc-data', 'ibc_waits.qnt', 'weakDataInit', 'weakDataStep'],
  ]) {
    runQuint(cli, ['run', model, ...deterministic, `--init=${init}`, `--step=${step}`, '--invariant=safety',
      '--max-steps=2', `--out-itf=${join(output, `${name}.itf.json`)}`, '--verbosity=1'], { negative: 'invariant', quiet: true });
    console.log(`negative ${name}: expected invariant violation (exit 1)`);
  }
  console.log(`portable checks passed: 9 files, 4 deterministic suites, 4 simulations, 6 negative controls; ITF: ${output}`);
  if (mode === '--extended') {
    runQuint(cli, ['verify', 'native_profiles.qnt', '--backend=apalache', `--server-endpoint=${process.env.APALACHE_SERVER}`,
      '--init=accountingInit', '--step=accountingStep', '--invariant=safety', '--max-steps=1', '--verbosity=5']);
    console.log('extended check passed: accountingInit/accountingStep depth 1 only');
  } else console.log('extended backend: NOT RUN (portable mode; no symbolic or exhaustive proof)');
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch(error => { console.error(error.message); process.exitCode = 1; });
}
