import { createHash, randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const DOMAIN = Buffer.from('cosmwasm/native-batch-auction/bid/v1', 'ascii');

function utf8(value, address = false) {
  if (typeof value !== 'string') throw new TypeError('expected a UTF-8 string');
  // A spelling guard only. The contract's configured API validates addresses.
  if (address && (!value || /\s/u.test(value) || value !== value.toLowerCase())) {
    throw new Error('address must use canonical lowercase spelling without whitespace');
  }
  const bytes = Buffer.from(value, 'utf8');
  if (bytes.toString('utf8') !== value) throw new Error('invalid UTF-8 string');
  if (bytes.length > 0xffffffff) throw new RangeError('UTF-8 length exceeds u32');
  const length = Buffer.alloc(4);
  length.writeUInt32BE(bytes.length);
  return Buffer.concat([length, bytes]);
}

function unsigned(value, width) {
  if (typeof value !== 'string' || !/^(0|[1-9][0-9]*)$/.test(value)) {
    throw new TypeError('amounts and IDs must be canonical decimal strings');
  }
  let n = BigInt(value);
  if (n >= (1n << BigInt(width * 8))) throw new RangeError('integer exceeds encoding width');
  const bytes = Buffer.alloc(width);
  for (let i = width - 1; i >= 0; i--) {
    bytes[i] = Number(n & 255n);
    n >>= 8n;
  }
  return bytes;
}

function saltBytes(salt) {
  if (typeof salt !== 'string' || !/^[a-fA-F0-9]{64}$/.test(salt)) {
    throw new Error('salt_hex must encode exactly 32 bytes');
  }
  return Buffer.from(salt, 'hex');
}

export function encodeCommitment(input) {
  if (!Number.isInteger(input.price_tick) || input.price_tick < 0 || input.price_tick > 65535) {
    throw new RangeError('price_tick must be an unsigned 16 bit integer');
  }
  const tick = Buffer.alloc(2);
  tick.writeUInt16BE(input.price_tick);
  return Buffer.concat([
    DOMAIN, utf8(input.chain_id), utf8(input.contract, true), utf8(input.bidder, true),
    utf8(input.beneficiary, true), unsigned(input.auction_id, 8),
    unsigned(input.collateral, 16), unsigned(input.quantity_lots, 16), tick,
    saltBytes(input.salt_hex),
  ]);
}

export function commitmentDigest(input) {
  return createHash('sha256').update(encodeCommitment(input)).digest('hex');
}

export function newSalt() {
  return randomBytes(32);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const input = JSON.parse(readFileSync(0, 'utf8'));
    if (input.salt_hex === undefined) input.salt_hex = newSalt().toString('hex');
    const digest_hex = commitmentDigest(input);
    const salt = saltBytes(input.salt_hex);
    process.stdout.write(JSON.stringify({
      digest_hex, digest_base64: Buffer.from(digest_hex, 'hex').toString('base64'),
      salt_hex: salt.toString('hex'), salt_base64: salt.toString('base64'),
    }) + '\n');
  } catch (error) {
    process.stderr.write(`Invalid commitment input: ${error.message}\n`);
    process.exitCode = 1;
  }
}
