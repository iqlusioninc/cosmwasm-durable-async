# Durable async Quint models

This directory owns its tooling; it adds no runtime dependency. The local
`@informalsystems/quint` version is pinned to **0.33.0** in `package.json` and
`package-lock.json`. It was exercised with Node **v26.11.1** and npm **11.20.0**.
The global Quint 0.31.0 installation is not used or changed.

## Reproduce the pricing checks

From the repository root:

```sh
cd formal/quint
npm ci --cache /private/tmp/cosmwasm-quint-npm-cache
./node_modules/.bin/quint --version
./node_modules/.bin/quint --help
./node_modules/.bin/quint parse --help
./node_modules/.bin/quint typecheck --help
./node_modules/.bin/quint test --help
npm run parse
npm run typecheck
npm test
```

The test command is exactly:

```sh
./node_modules/.bin/quint test pricing_tests.qnt --backend=typescript --seed=1 --max-samples=1
```

These tests contain no nondeterministic choices. Seed 1 and one sample make the
invocation explicit and reproducible. Parse and typecheck cover both pricing
files. These commands do not invoke symbolic or finite-state model checking.
The TypeScript evaluator handles the integer atomics without JavaScript `number`
rounding, as checked by the one-atomic and maximum-width fixtures.

CLI syntax is taken from this installed version's help. Upstream references:
[Quint CLI manual](https://quint.sh/docs/quint) and
[Quint source](https://github.com/quint-co/quint). The online manual can describe
features or defaults that differ from the pinned version.

## Exact pricing and domains

`pricing.qnt` models integer atomics at **10^18 atomics per whole price unit**.
Prices and floors have the Decimal u128 domain, maximum
`340282366920938463463374607431768211455`; gas, utilization, target and adjustment
denominator have the u64 domain. Quint integers provide wide intermediate
arithmetic, matching the Rust Uint256 calculation for these input widths.

`nextPrice(base, used, target, floor, denominator)` computes the magnitude in
this order, truncating after each division:

```text
magnitude = ((base * abs(used - target)) / target) / denominator
```

Increases saturate at u128 maximum. Decreases (including equality at target)
clamp at the configured floor, matching
[`next_base_price`](../../crates/native-async/src/pricing.rs).
Every `prices[]` row in
[`base-price.json`](../../fixtures/native-async/v1/base-price.json) has a test
with its exact literal expected atomics. The JSON's `bid_components[]` rows
describe key encoding, which is outside this arithmetic module.

`attemptCharge(gas, price)` computes `ceil(gas * price / 10^18)` with wide
integer arithmetic, matching the mathematical operation in
[`attempt_charge_at`](../../crates/native-async/src/host.rs). An oversized result
is not saturated: Rust's checked conversion rejects it. The maximum-width test
retains that wide result to expose overflow rather than hiding it.

Callers must guard with `nextPricePrecondition(...)` or
`attemptChargePrecondition(...)` before applying the corresponding operator to
runtime state. The price predicate rejects zero divisors and out-of-domain
inputs; the fee predicate also rejects a result above u128. The operators return
integers, not tagged Rust errors; invalid inputs are outside their contract.
No fallback price is assigned to a configuration with an invalid divisor.
Direct evaluation with a zero divisor is an evaluator error, not a modeled
rejection or a model-checker counterexample. State-machine callers must check the
precondition before evaluation.

These exact arithmetic tests are separate from bounded scheduler exploration.
Later scheduler profiles use small task/actor/gas/height domains for tractable
search; those bounds are not production caps. Passing these fixtures establishes
the modeled arithmetic examples, not a proof of Rust/Go execution or a complete
scheduler safety result.

## Tool dependency caveat

The pinned development dependency currently produces `npm audit` findings for
`adm-zip` through Quint (two high-severity dependency entries). No automatic
update or suppression is applied. The prescribed TypeScript tests operate on
local model text and do not download a Rust evaluator. In this version,
`dist/src/rust/binaryManager.js` uses `adm-zip` when extracting a downloaded
Windows `.zip` evaluator; macOS/Linux evaluator assets take its `.tar.gz` branch.
Do not interpret these development-tool findings as runtime crate findings.
