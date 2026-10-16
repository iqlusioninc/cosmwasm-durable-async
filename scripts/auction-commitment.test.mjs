import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import test from 'node:test';
import { encodeCommitment, commitmentDigest, newSalt } from './auction-commitment.mjs';
const vectors = JSON.parse(readFileSync(new URL('../fixtures/auction-commitments.json', import.meta.url)));

test('fixed vectors pin UTF-8 lengths, integer widths, bytes and digest', () => {
  for (const input of vectors) {
    assert.equal(encodeCommitment(input).toString('hex'), input.bytes_hex);
    assert.equal(commitmentDigest(input), input.digest_hex);
  }
});
test('every domain and bid term changes the digest', () => {
  const v = vectors[0];
  for (const [field, value] of Object.entries({ chain_id: 'another-chain', contract: v.bidder, bidder: v.contract, beneficiary: v.bidder, auction_id: '2', collateral: '30', quantity_lots: '3', price_tick: 2, salt_hex: '11'.repeat(32) })) {
    assert.notEqual(commitmentDigest({ ...v, [field]: value }), v.digest_hex, field);
  }
});
test('rejects lossy or malformed integers and out of range encodings', () => {
  const v = vectors[0];
  for (const field of ['auction_id', 'collateral', 'quantity_lots']) {
    for (const value of [1, 1n, '-1', '1.0', ' 1', '01', '', '1e2']) assert.throws(() => encodeCommitment({ ...v, [field]: value }));
  }
  for (const [field, value] of [['auction_id', '18446744073709551616'], ['collateral', '340282366920938463463374607431768211456'], ['quantity_lots', '340282366920938463463374607431768211456'], ['price_tick', -1], ['price_tick', 65536], ['price_tick', 1.5], ['price_tick', '1']]) assert.throws(() => encodeCommitment({ ...v, [field]: value }));
  assert.doesNotThrow(() => encodeCommitment({ ...v, auction_id: '0', collateral: '0', quantity_lots: '0', price_tick: 0 }));
});
test('requires canonical address spelling and a 32 byte salt', () => {
  for (const field of ['contract', 'bidder', 'beneficiary']) {
    for (const value of ['', ' '+vectors[0][field], vectors[0][field].toUpperCase(), vectors[0][field]+'\n']) assert.throws(() => encodeCommitment({ ...vectors[0], [field]: value }));
  }
  for (const salt_hex of ['', '00'.repeat(31), '00'.repeat(33), 'gg'.repeat(32)]) assert.throws(() => encodeCommitment({ ...vectors[0], salt_hex }));
});
test('newSalt supplies 32 fresh OS random bytes', () => {
  const a = newSalt(), b = newSalt(); assert.ok(Buffer.isBuffer(a)); assert.equal(a.length,32); assert.equal(b.length,32); assert.notDeepEqual(a,b);
});
test('stdin client emits CosmWasm Binary encodings and generates missing salts', () => {
  for (const provided of [true, false]) {
    const input = { ...vectors[0] }; if (!provided) delete input.salt_hex;
    const result=spawnSync(process.execPath,['scripts/auction-commitment.mjs'],{input:JSON.stringify(input),encoding:'utf8'});
    assert.equal(result.status,0,result.stderr);const out=JSON.parse(result.stdout);
    assert.match(out.salt_hex,/^[a-f0-9]{64}$/);assert.equal(Buffer.from(out.salt_base64,'base64').toString('hex'),out.salt_hex);
    assert.equal(Buffer.from(out.digest_base64,'base64').toString('hex'),out.digest_hex);
    assert.equal(out.digest_hex,commitmentDigest({...input,salt_hex:out.salt_hex}));
    if(provided)assert.equal(out.digest_hex,vectors[0].digest_hex);
  }
});
