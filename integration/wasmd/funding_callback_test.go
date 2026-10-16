package async

import (
	sdkmath "cosmossdk.io/math"
	"encoding/json"
	"fmt"
	wasmkeeper "github.com/CosmWasm/wasmd/x/wasm/keeper"
	wasmtypes "github.com/CosmWasm/wasmd/x/wasm/types"
	wasmvm "github.com/CosmWasm/wasmvm/v2"
	vmtypes "github.com/CosmWasm/wasmvm/v2/types"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	"github.com/stretchr/testify/require"
	"os"
	"strconv"
	"testing"
)

// Keep real VM execution, writes, gas, and outbound messages. Select the return
// disposition at the VM boundary so every host dispatch disposition is covered.
type fundingOutcomeEngine struct {
	wasmtypes.WasmEngine
	outcome string
}

func (g fundingOutcomeEngine) Sudo(c wasmvm.Checksum, e vmtypes.Env, msg []byte, s wasmvm.KVStore, a wasmvm.GoAPI, q wasmvm.Querier, m wasmvm.GasMeter, l uint64, d vmtypes.UFraction) (*vmtypes.ContractResult, uint64, error) {
	r, used, err := g.WasmEngine.Sudo(c, e, msg, s, a, q, m, l, d)
	if err == nil && r != nil && r.Ok != nil && g.outcome != "pending" {
		var response, selected PollResponse
		if err = json.Unmarshal(r.Ok.Data, &response); err != nil {
			return nil, used, err
		}
		payload := map[string]string{"yield": `{"yield":{"state":""}}`, "complete": `{"complete":{"output":""}}`, "fail": `{"fail":{"error":"business failure"}}`}[g.outcome]
		if err = json.Unmarshal([]byte(`{"outcome":`+payload+`,"messages":[]}`), &selected); err != nil {
			return nil, used, err
		}
		response.Outcome = selected.Outcome
		r.Ok.Data = marshal(response)
	}
	return r, used, err
}

func setupFundingOutcome(t *testing.T, outcome string) (sdk.Context, *Keeper, wasmkeeper.TestKeepers) {
	t.Helper()
	k := &Keeper{Config: testConfig()}
	ctx, keepers := wasmkeeper.CreateTestInput(acceptanceQuietTB{t}, false, wasmkeeper.BuiltInCapabilities(),
		append([]wasmkeeper.Option{wasmkeeper.WithWasmEngineDecorator(func(engine wasmtypes.WasmEngine) wasmtypes.WasmEngine { return fundingOutcomeEngine{engine, outcome} })}, k.Options()...)...)
	k.Key, k.Bank, k.Wasm = keepers.WasmStoreKey, keepers.BankKeeper, keepers.WasmKeeper
	module := keepers.AccountKeeper.NewAccount(ctx, authtypes.NewEmptyModuleAccount(ModuleName)).(sdk.ModuleAccountI)
	keepers.AccountKeeper.SetModuleAccount(ctx, module)
	return ctx.WithBlockHeight(10), k, keepers
}

// A real Wasm poll emits its outbound request. The injected message dispatcher
// isolates the escrow invariant: it performs an authenticated owner topup, then
// either succeeds or fails. The VM and bank keep their real SDK gas meters.
func TestInFlightFundingConservation(t *testing.T) {
	for _, price := range []string{"0", "0.002"} {
		for _, outcome := range []string{"pending", "yield", "complete", "fail"} {
			for _, fail := range []bool{false, true} {
				t.Run(fmt.Sprintf("price=%s/outcome=%s/dispatch_failure=%v", price, outcome, fail), func(t *testing.T) {
					ctx, k, keepers := setupFundingOutcome(t, outcome)
					k.Config.MinBasePrice = sdkmath.LegacyZeroDec()
					creator := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 10_000_000))
					code, e := os.ReadFile("../../artifacts/native_fulfill_example.wasm")
					require.NoError(t, e)
					id, _, e := keepers.ContractKeeper.Create(ctx, creator, code, nil)
					require.NoError(t, e)
					owner, _, e := keepers.ContractKeeper.Instantiate(ctx, id, creator, creator, marshal(map[string]any{"payment_service": creator.String(), "shipment_service": creator.String(), "deadline_blocks": 10, "fee_denom": "ugas"}), "funding-fixture", sdk.Coins{})
					require.NoError(t, e)
					start := marshal(map[string]any{"Start": map[string]any{"order": map[string]any{"id": 9}, "business": sdk.NewInt64Coin("ugas", 0), "gas": sdk.NewInt64Coin("ugas", 1_000_000), "gas_per_poll": 100_000_000, "max_gas_price": "1", "expires_at": 100, "allocated_bytes": 524288}})
					_, e = keepers.ContractKeeper.Execute(ctx, owner, creator, start, sdk.NewCoins(sdk.NewInt64Coin("ugas", 1_000_000)))
					require.NoError(t, e)
					require.NoError(t, keepers.BankKeeper.SendCoins(ctx, creator, owner, sdk.NewCoins(sdk.NewInt64Coin("ugas", 50))))
					keepers.Faucet.Fund(ctx, owner, sdk.NewInt64Coin("ugas", 1_000_000))
					r := registrationPolicy(owner)
					r.Control, r.Refund = creator.String(), creator.String()
					otherID, e := k.RegisterTask(ctx, owner, r)
					require.NoError(t, e)
					ctx = ctx.WithBlockHeight(11)
					k.store(ctx).Set([]byte("base_price"), []byte(price))
					require.True(t, k.promoteDue(ctx))
					old, stop := k.selectReady(ctx, 160_000_000, nil)
					require.False(t, stop)
					require.NotNil(t, old)
					require.Equal(t, uint64(1), old.ID)
					otherBefore, e := k.load(ctx, owner.String(), otherID)
					require.NoError(t, e)
					initial := amount(old.Escrow)
					refundBefore := keepers.BankKeeper.GetBalance(ctx, creator, "ugas").Amount
					dispatched := 0
					k.messenger = wasmkeeper.MessageHandlerFunc(func(c sdk.Context, a sdk.AccAddress, _ string, _ vmtypes.CosmosMsg) ([]sdk.Event, [][]byte, [][]*codectypes.Any, error) {
						dispatched++
						task, e := k.load(c, a.String(), old.ID)
						require.NoError(t, e)
						wantReserve := map[string]string{"0": "0", "0.002": "200000"}[price]
						require.Equal(t, wantReserve, task.Reserved)
						// Deliberately mutate stored state to catch a callback rereading the
						// live price instead of inheriting this attempt's immutable price.
						k.store(c).Set([]byte("base_price"), []byte("0.004"))
						require.Equal(t, wantReserve, k.fee(c, task.GasPerPoll).String())
						query, e := k.Query(c, []byte(`{"funding_config":{}}`))
						require.NoError(t, e)
						var config map[string]any
						require.NoError(t, json.Unmarshal(query, &config))
						require.Equal(t, map[string]string{"0": "0.000000000000000000", "0.002": "0.002000000000000000"}[price], config["gas_price"])
						before := policySnapshot(c, k)
						for _, kind := range []string{"cancel_task", "resume_task"} {
							require.ErrorContains(t, k.Management(c, a.String(), marshal(map[string]any{kind: map[string]any{"task_id": old.ID, "actor": creator.String()}})), "in-flight")
						}
						require.ErrorContains(t, k.ExtendLease(c, a.String(), ExtendLeaseMsg{TaskID: old.ID, Actor: creator.String(), AllocatedBytes: old.AllocatedBytes, ExpiresAt: 101}), "in-flight")
						require.ErrorContains(t, k.UpdateBudget(c, a.String(), UpdateBudgetMsg{TaskID: old.ID, Actor: creator.String(), GasPerPoll: old.GasPerPoll, MaxGasPrice: old.MaxGasPrice, Refund: old.Refund}), "in-flight")
						require.Equal(t, before, policySnapshot(c, k))
						require.NoError(t, k.UpdateBudget(c, a.String(), UpdateBudgetMsg{TaskID: otherID, Actor: creator.String(), GasPerPoll: r.GasPerPoll, MaxGasPrice: r.MaxGasPrice, Refund: r.Refund}))
						require.Equal(t, before, policySnapshot(c, k))
						// Only available+deposit fits at a positive reservation. The
						// total must reject before any bank or task/index mutation.
						original := task
						task.Escrow = amount("340282366920938463463374607431768211455").Sub(amount(task.Reserved)).String()
						k.save(c, task)
						overflowBefore := policySnapshot(c, k)
						bankBefore := keepers.BankKeeper.GetAllBalances(c, a)
						escrowBefore := keepers.BankKeeper.GetAllBalances(c, authtypes.NewModuleAddress(ModuleName))
						require.ErrorContains(t, k.Management(c, a.String(), marshal(map[string]any{"fund_task": FundTaskMsg{TaskID: old.ID, Contributor: creator.String(), Funds: sdk.NewInt64Coin("ugas", 1)}})), "overflow")
						require.Equal(t, overflowBefore, policySnapshot(c, k))
						require.Equal(t, bankBefore, keepers.BankKeeper.GetAllBalances(c, a))
						require.Equal(t, escrowBefore, keepers.BankKeeper.GetAllBalances(c, authtypes.NewModuleAddress(ModuleName)))
						k.save(c, original)
						e = k.Management(c, a.String(), marshal(map[string]any{"fund_task": FundTaskMsg{TaskID: old.ID, Contributor: creator.String(), Funds: sdk.NewInt64Coin("ugas", 17)}}))
						require.NoError(t, e)
						if fail {
							return nil, nil, nil, fmt.Errorf("intentional outbound failure after topup")
						}
						return nil, nil, nil, nil
					})
					used := k.poll(ctx, *old)
					require.Nil(t, ctx.Value(pollingKey{}))
					require.Equal(t, 1, dispatched, "new parity cases must execute the nested dispatcher")
					task, e := k.load(ctx, owner.String(), old.ID)
					require.NoError(t, e)
					require.LessOrEqual(t, used, old.GasPerPoll)
					expected := initial.Sub(amount(task.Fees))
					ownerBalance := int64(50)
					if fail {
						require.Equal(t, "message_failure", task.Pause)
					} else {
						expected = expected.AddRaw(17)
						ownerBalance -= 17
						require.Equal(t, map[string]string{"pending": "waiting", "yield": "runnable", "complete": "completed", "fail": "failed"}[outcome], task.Status)
					}
					terminal := !fail && (outcome == "complete" || outcome == "fail")
					if terminal {
						require.Equal(t, "0", task.Escrow)
						require.Equal(t, expected.String(), keepers.BankKeeper.GetBalance(ctx, creator, "ugas").Amount.Sub(refundBefore).String())
					} else {
						require.Equal(t, expected.String(), task.Escrow)
					}
					require.Equal(t, "0", task.Reserved)
					require.Equal(t, fmt.Sprint(ownerBalance), keepers.BankKeeper.GetBalance(ctx, owner, "ugas").Amount.String())
					otherAfter, e := k.load(ctx, owner.String(), otherID)
					require.NoError(t, e)
					require.Equal(t, otherBefore, otherAfter)
					require.Equal(t, amount(task.Escrow).Add(amount(otherAfter.Escrow)).String(), keepers.BankKeeper.GetBalance(ctx, authtypes.NewModuleAddress(ModuleName), "ugas").Amount.String())
					attrs := priceEvent(t, ctx, "async_poll")
					require.Equal(t, map[string]string{"0": "0.000000000000000000", "0.002": "0.002000000000000000"}[price], attrs["price"])
					wantBase := "0.004000000000000000"
					if fail {
						wantBase = map[string]string{"0": "0.000000000000000000", "0.002": "0.002000000000000000"}[price]
					}
					require.Equal(t, wantBase, k.basePrice(ctx).String())
					if price == "0" {
						require.Equal(t, "0", task.Fees)
					} else {
						execution, err := strconv.ParseUint(attrs["execution_gas"], 10, 64)
						require.NoError(t, err)
						host, err := strconv.ParseUint(attrs["charged_host_gas"], 10, 64)
						require.NoError(t, err)
						// ceil(gas * .002), independent of Keeper.feeAt.
						require.Equal(t, fmt.Sprint((execution+host+499)/500), task.Fees)
					}
					if !terminal {
						require.NoError(t, k.UpdateBudget(ctx, owner.String(), UpdateBudgetMsg{TaskID: old.ID, Actor: creator.String(), GasPerPoll: old.GasPerPoll, MaxGasPrice: old.MaxGasPrice, Refund: old.Refund}))
						if fail {
							require.NoError(t, k.Management(ctx, owner.String(), marshal(map[string]any{"resume_task": map[string]any{"task_id": old.ID, "actor": creator.String()}})))
						}
						require.NoError(t, k.ExtendLease(ctx, owner.String(), ExtendLeaseMsg{TaskID: old.ID, Actor: creator.String(), AllocatedBytes: old.AllocatedBytes, ExpiresAt: 101}))
						require.NoError(t, k.Management(ctx, owner.String(), marshal(map[string]any{"cancel_task": map[string]any{"task_id": old.ID, "actor": creator.String()}})))
					}
				})
			}
		}
	}
}
