# Mock fixed-price exchange

This contract is the local test counterparty for the
[native DCA tutorial](../native-dca/README.md). It is not an AMM, oracle, live
market or production exchange. Use local test assets only.

Instantiate with immutable native `input_denom`, `output_denom`, `numerator`
and `denominator`. Denominations must differ and both ratio terms must be
positive. Output is `floor(input * numerator / denominator)`, computed with
checked wide arithmetic; zero or overflowing output is rejected. There is no
price administration, fee, liquidity withdrawal or order-term editing API.
Supply output liquidity with a bank send of local output coins to the contract.

The `swap` execute message accepts exactly its requested input coin. It rejects
wrong denominations, zero input/minimum, output below the requested minimum,
insufficient output balance and a repeated `(requesting DCA address, order_id,
slice_index)`. It records the receipt, sends output directly to the fixed
beneficiary, then sends `swap_result` to the **actual sender**. A caller cannot
choose a separate callback target. The DCA contract authenticates this callback
and its pending dispatch, then requires the receipt in its dispatch reply.
Any error in this enclosing transaction rolls back fixture/DCA writes and bank
effects. This correctness claim depends on the reviewed fixture and actual
transactional bank execution, not on a receipt alone proving an arbitrary
counterparty honest.

Smart queries (IDs/addresses are illustrative):

```json
{"config":{}}
{"receipt":{"dca":"<DCA address>","order_id":1,"slice_index":0}}
```

Build from the repository root with:

```sh
bash contracts/native-dca/build-wasmd.sh
```

This creates `artifacts/mock_exchange.wasm` alongside the DCA and acceptance-only
fault artifacts. Run `bash scripts/native-dca-demo.sh liquidity` to demonstrate
liquidity failure, replenishment and explicit owner resume; replenishing the
fixture never resumes an order itself. A bad immutable minimum requires
cancellation/reclaim because the price cannot be changed. See the tutorial for
prerequisites, every scenario command, funding separation and runtime limits.
