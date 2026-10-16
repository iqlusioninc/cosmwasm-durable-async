import { createHash } from 'node:crypto';
import { readFileSync, statSync } from 'node:fs';
import { resolve, sep } from 'node:path';
import { commitmentDigest } from './auction-commitment.mjs';

// A fresh run index plus actual scenario receipts is required. Go exits zero
// when a selected test is missing/skipped; that alone is never demo evidence.
try {
  const [directory, scenario, artifact] = process.argv.slice(2);
  const root = resolve(directory);
  const expected = scenario === 'all' ? ['happy', 'funding', 'restart', 'expiry'] : [scenario];
  const read = path => JSON.parse(readFileSync(path, 'utf8'));
  const check = (value, message) => { if (!value) throw new Error(message); };
  const hash = bytes => createHash('sha256').update(bytes).digest('hex');
  const artifactHash = hash(readFileSync(artifact));
  const entries = read(resolve(root, 'suite-result.json'));
  check(Array.isArray(entries) && entries.length === expected.length, 'missing expected scenario results');
  for (const [index, name] of expected.entries()) {
    const entry = entries[index];
    check(entry.scenario === name && typeof entry.receipt === 'string', 'unexpected scenario selection');
    const path = resolve(root, entry.receipt);
    check(path.startsWith(root + sep), 'receipt outside current run');
    const receipt = read(path);
    check(receipt.scenario === name && receipt.passed === true && receipt.owned_children_exited === true, `scenario did not pass/clean up: ${name}`);
    check(/^[a-f0-9]{40}$/.test(receipt.source_revision) && receipt.artifact_sha256 === artifactHash, 'missing or mismatched source/artifact identity');
    check(receipt.host_config && receipt.funding_config?.denom === 'ugas' && receipt.node_config?.host_reserve > 0, 'missing live configuration evidence');
    check(receipt.children?.length >= 2 && receipt.children.every(c => Number.isInteger(c.pid) && c.pid > 0 && c.exited === true), 'owned child exit evidence missing');
    check(receipt.transactions?.length > 0 && receipt.transactions.every(tx => tx.outcome === 'committed' && Number.isInteger(tx.deliver_tx_code) && tx.height > 0), 'unresolved/missing transaction evidence');
    const folder = resolve(path, '..');
    for (const [i, tx] of receipt.transactions.entries()) {
      const journal = read(resolve(folder, `tx-${String(i + 1).padStart(3, '0')}.json`));
      check(journal.stage === 'resolved' && journal.hash === tx.hash && hash(Buffer.from(journal.tx_hex, 'hex')).toUpperCase() === tx.hash, 'original signed transaction journal mismatch');
      check(JSON.stringify(journal.receipt) === JSON.stringify(tx), 'journal receipt mismatch');
    }
    const saltsPath = resolve(folder, 'commitments.json');
    check((statSync(saltsPath).mode & 0o777) === 0o600, 'salt snapshot mode must be 0600');
    const salts = read(saltsPath);
    check(salts.length === 5 && new Set(salts.map(s => s.output.salt_hex)).size === 5, 'missing/fixed salt snapshot');
    for (const s of salts) check(commitmentDigest({ ...s.input, salt_hex: s.output.salt_hex }) === s.output.digest_hex, 'commitment snapshot mismatch');
    const final = receipt.snapshots?.at(-1)?.capture?.State;
    check(final?.View?.auction?.state === (name === 'expiry' ? 'aborted' : 'finalized') && final.View.auction.seller_claimed === true && final.Bids.every(b => b.claimed === true), 'missing final application claims');
    check(final.Task === null && final.View.host.unavailable, 'missing separate pruned-host evidence');
    console.log(`VERIFIED ${name}: source=${receipt.source_revision} dirty=${receipt.source_dirty} artifact=${artifactHash} owned_children_exited=true receipt=${path}`);
  }
} catch (error) {
  console.error(`Auction demo evidence incomplete: ${error.message}`);
  process.exitCode = 1;
}
