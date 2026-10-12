package async

import (
	"encoding/json"
	"os"
	"strings"
	"testing"

	sdkmath "cosmossdk.io/math"
	wasmkeeper "github.com/CosmWasm/wasmd/x/wasm/keeper"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	"github.com/stretchr/testify/require"
)

// Exercise admission with a real Wasm contract and bank, without a contract's
// typed entry point pre-validating the custom-message funding policy.
func registrationOwner(t *testing.T, ctx sdk.Context, k *Keeper) sdk.AccAddress {
	t.Helper()
	code, err := os.ReadFile("../../artifacts/native_service.wasm")
	require.NoError(t, err)
	creator := sdk.AccAddress([]byte("policy-creator-000001"))
	contracts := wasmkeeper.NewDefaultPermissionKeeper(k.Wasm)
	id, _, err := contracts.Create(ctx, creator, code, nil)
	require.NoError(t, err)
	owner, _, err := contracts.Instantiate(ctx, id, creator, creator, []byte(`{}`), "policy", sdk.Coins{})
	require.NoError(t, err)
	return owner
}

func registrationPolicy(owner sdk.AccAddress) Register {
	return Register{Control: owner.String(), Refund: owner.String(), GasPerPoll: 100_000_000, MaxGasPrice: "1", ExpiresAt: 100,
		AllocatedBytes: 8192, Program: Program{Kind: "test", Version: 1, Input: []byte{}}, Funds: sdk.NewInt64Coin("ugas", 1_000_000)}
}

func TestRegistrationRejectsInvalidFundingPolicyWithoutSideEffects(t *testing.T) {
	for _, tc := range []struct{ name, price, refund string }{
		{"escrow-refund", "1", authtypes.NewModuleAddress(ModuleName).String()},
		{"negative", "-1", ""}, {"signed", "+1", ""}, {"precision", "1.0000000000000000001", ""},
		{"empty-fraction", "1.", ""}, {"missing-integer", ".1", ""},
		{"decimal-over-limit", "340282366920938463463.374607431768211456", ""},
		{"legacy-decimal-only", "1000000000000000000000", ""},
	} {
		t.Run(tc.name, func(t *testing.T) {
			ctx, k, keepers := setup(t)
			owner := registrationOwner(t, ctx, k)
			keepers.Faucet.Fund(ctx, owner, sdk.NewInt64Coin("ugas", 2_000_000))
			r := registrationPolicy(owner)
			r.MaxGasPrice = tc.price
			if tc.refund != "" {
				r.Refund = tc.refund
			}
			before := policySnapshot(ctx, k)
			balances := func() []sdk.Coins {
				return []sdk.Coins{keepers.BankKeeper.GetAllBalances(ctx, owner), keepers.BankKeeper.GetAllBalances(ctx, authtypes.NewModuleAddress(ModuleName)), keepers.BankKeeper.GetAllBalances(ctx, authtypes.NewModuleAddress(authtypes.FeeCollectorName))}
			}
			bankBefore := balances()
			id, err := k.RegisterTask(ctx, owner, r)
			require.Error(t, err)
			require.Zero(t, id)
			require.Equal(t, before, policySnapshot(ctx, k), "rejection must not allocate IDs, tasks or indexes")
			require.Equal(t, bankBefore, balances(), "rejection must not transfer funds or lease fees")
			r = registrationPolicy(owner)
			id, err = k.RegisterTask(ctx, owner, r)
			require.NoError(t, err)
			require.Equal(t, uint64(1), id)
		})
	}
}

func TestRegistrationAndBudgetShareDecimalFundingDomain(t *testing.T) {
	b, err := os.ReadFile("../../fixtures/native-async/decimal-domain.json")
	require.NoError(t, err)
	var cases []struct {
		Name    string
		Valid   bool
		Funding map[string]any
	}
	require.NoError(t, json.Unmarshal(b, &cases))
	for _, tc := range cases {
		t.Run(tc.Name, func(t *testing.T) {
			ctx, k, keepers := setup(t)
			owner := registrationOwner(t, ctx, k)
			keepers.Faucet.Fund(ctx, owner, sdk.NewInt64Coin("ugas", 2_000_000))
			r := registrationPolicy(owner)
			r.MaxGasPrice = tc.Funding["max_gas_price"].(string)
			before := policySnapshot(ctx, k)
			id, err := k.RegisterTask(ctx, owner, r)
			if tc.Valid {
				require.NoError(t, err)
				query, err := k.Query(ctx, marshal(map[string]any{"task_funding": map[string]any{"contract": owner.String(), "task_id": id}}))
				require.NoError(t, err)
				// Only addresses vary in the real bank/contract harness; every ABI
				// field and numeric string must equal the Rust typed fixture.
				for _, key := range []string{"contract", "control", "refund"} {
					tc.Funding[key] = owner.String()
				}
				require.JSONEq(t, string(marshal(tc.Funding)), string(query))
			} else {
				require.Error(t, err)
				require.Equal(t, before, policySnapshot(ctx, k))
				r.MaxGasPrice = "1"
				id, err = k.RegisterTask(ctx, owner, r)
				require.NoError(t, err)
			}
			before = policySnapshot(ctx, k)
			err = k.UpdateBudget(ctx, owner.String(), UpdateBudgetMsg{TaskID: id, Actor: owner.String(), Refund: owner.String(), GasPerPoll: r.GasPerPoll, MaxGasPrice: tc.Funding["max_gas_price"].(string)})
			if tc.Valid {
				require.NoError(t, err)
			} else {
				require.Error(t, err)
				require.Equal(t, before, policySnapshot(ctx, k))
			}
		})
	}
	for _, price := range []string{"0", "0.001", "1", "1.5", "0001.000"} {
		ctx, k, keepers := setup(t)
		k.Config.MinBasePrice = sdkmath.LegacyZeroDec() // Syntax/domain is independent of the configured admission floor.
		owner := registrationOwner(t, ctx, k)
		keepers.Faucet.Fund(ctx, owner, sdk.NewInt64Coin("ugas", 2_000_000))
		r := registrationPolicy(owner)
		r.MaxGasPrice = price
		id, err := k.RegisterTask(ctx, owner, r)
		require.NoError(t, err, price)
		require.NoError(t, k.UpdateBudget(ctx, owner.String(), UpdateBudgetMsg{TaskID: id, Actor: owner.String(), Refund: owner.String(), GasPerPoll: r.GasPerPoll, MaxGasPrice: price}), price)
	}
}

func TestRegistrationRejectsBidBelowFloor(t *testing.T) {
	ctx, k, keepers := setup(t)
	owner := registrationOwner(t, ctx, k)
	keepers.Faucet.Fund(ctx, owner, sdk.NewInt64Coin("ugas", 3_000_000))
	r := registrationPolicy(owner)
	r.MaxGasPrice = "0.000999999999999999"
	before := policySnapshot(ctx, k)
	balance := keepers.BankKeeper.GetAllBalances(ctx, owner)
	_, err := k.RegisterTask(ctx, owner, r)
	require.Error(t, err)
	require.Equal(t, before, policySnapshot(ctx, k))
	require.Equal(t, balance, keepers.BankKeeper.GetAllBalances(ctx, owner))
	// Exactly the floor remains admissible below current base, provided funding
	// covers one attempt at that live price rather than the task's smaller bid.
	k.store(ctx).Set([]byte("base_price"), []byte("0.02"))
	r.MaxGasPrice = "0.001"
	before = policySnapshot(ctx, k)
	_, err = k.RegisterTask(ctx, owner, r)
	require.ErrorContains(t, err, "funding")
	require.Equal(t, before, policySnapshot(ctx, k))
	r.Funds = sdk.NewInt64Coin("ugas", 2_000_100)
	id, err := k.RegisterTask(ctx, owner, r)
	require.NoError(t, err)
	require.Equal(t, uint64(1), id)
	task, err := k.load(ctx, owner.String(), id)
	require.NoError(t, err)
	require.Equal(t, "runnable", task.Status)
	require.Equal(t, "2000000", task.Escrow)
}

// Live-base and funding checks are independent requirements; equality suffices
// only when escrow also funds the entire attempt at that same base price.
func TestResumeRequiresBidAtBase(t *testing.T) {
	ctx, k, _ := setup(t)
	k.store(ctx).Set([]byte("base_price"), []byte("0.004"))
	task := queuedAuctionTask(ctx, k, 1, "0.003", 80_000_000, 10)
	k.store(ctx).Delete(task.QueueKey)
	task.QueueKey, task.Status, task.Pause = nil, "paused", "price_limit"
	k.save(ctx, task)
	resume := marshal(map[string]any{"resume_task": map[string]any{"task_id": task.ID, "actor": task.Control}})
	before := policySnapshot(ctx, k)
	require.Error(t, k.Management(ctx, task.Owner, resume))
	require.Equal(t, before, policySnapshot(ctx, k))
	require.NoError(t, k.UpdateBudget(ctx, task.Owner, UpdateBudgetMsg{TaskID: task.ID, Actor: task.Control, GasPerPoll: task.GasPerPoll, MaxGasPrice: "0.004", Refund: task.Refund}))
	task, err := k.load(ctx, task.Owner, task.ID)
	require.NoError(t, err)
	task.Escrow = "319999"
	k.save(ctx, task)
	before = policySnapshot(ctx, k)
	require.Error(t, k.Management(ctx, task.Owner, resume))
	require.Equal(t, before, policySnapshot(ctx, k))
	task.Escrow = "320000"
	k.save(ctx, task)
	require.NoError(t, k.Management(ctx, task.Owner, resume))
	task, err = k.load(ctx, task.Owner, task.ID)
	require.NoError(t, err)
	require.Equal(t, "runnable", task.Status)
	require.Empty(t, task.Pause)
	require.Equal(t, uint64(11), task.ReadyAt)
	require.True(t, strings.HasPrefix(string(task.QueueKey), "due/"))
}

func TestLastActiveTaskRemovesOwnerCountAndPruningKeepsIDs(t *testing.T) {
	ctx, k, keepers := setup(t)
	a, b := registrationOwner(t, ctx, k), registrationOwner(t, ctx, k)
	for _, owner := range []sdk.AccAddress{a, b} {
		keepers.Faucet.Fund(ctx, owner, sdk.NewInt64Coin("ugas", 3_000_000))
	}
	for _, owner := range []sdk.AccAddress{a, a, b} {
		_, err := k.RegisterTask(ctx, owner, registrationPolicy(owner))
		require.NoError(t, err)
	}
	cancel := func(owner sdk.AccAddress, id uint64) {
		require.NoError(t, k.Management(ctx, owner.String(), marshal(map[string]any{"cancel_task": map[string]any{"task_id": id, "actor": owner.String()}})))
	}
	cancel(a, 1)
	require.Equal(t, uint64(1), number(k.store(ctx), "active/"+a.String()))
	cancel(b, 3)
	require.False(t, k.store(ctx).Has([]byte("active/"+b.String())))
	require.True(t, k.store(ctx).Has([]byte("active/"+a.String())))
	cancel(a, 2)
	require.False(t, k.store(ctx).Has([]byte("active/"+a.String())))
	ctx = ctx.WithBlockHeight(100)
	for i := 0; i < 3; i++ {
		require.True(t, k.maintenanceOne(ctx, "prune"))
	}
	require.False(t, k.maintenanceOne(ctx, "prune"))
	for key := range policySnapshot(ctx, k) {
		for _, prefix := range []string{"active/", "task/", "prune/", "expiry/", "due/", "bid/", "ready/", "contracts/", "contract-queue/"} {
			require.False(t, strings.HasPrefix(key, prefix), "leftover index %s", key)
		}
	}
	require.Equal(t, uint64(3), number(k.store(ctx), "next_id"))
	require.Zero(t, number(k.store(ctx), "bytes"))
	r := registrationPolicy(a)
	r.ExpiresAt = 110
	id, err := k.RegisterTask(ctx, a, r)
	require.NoError(t, err)
	require.Equal(t, uint64(4), id)
}

func TestPerContractAdmissionCapLeavesRoomForOthers(t *testing.T) {
	ctx, k, keepers := setup(t)
	k.Config.MaxContractTasks = 2
	a, b := registrationOwner(t, ctx, k), registrationOwner(t, ctx, k)
	for _, owner := range []sdk.AccAddress{a, b} {
		keepers.Faucet.Fund(ctx, owner, sdk.NewInt64Coin("ugas", 5_000_000))
	}
	for i := 0; i < 2; i++ {
		_, err := k.RegisterTask(ctx, a, registrationPolicy(a))
		require.NoError(t, err)
	}
	before := policySnapshot(ctx, k)
	_, err := k.RegisterTask(ctx, a, registrationPolicy(a))
	require.ErrorContains(t, err, "contract admission full")
	require.Equal(t, before, policySnapshot(ctx, k))
	_, err = k.RegisterTask(ctx, b, registrationPolicy(b))
	require.NoError(t, err)
}
