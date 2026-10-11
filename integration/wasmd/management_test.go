package async

import (
	sdkmath "cosmossdk.io/math"
	storetypes "cosmossdk.io/store/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	"github.com/stretchr/testify/require"
	"os"
	"testing"
)

func TestRealWasmManagementRoutes(t *testing.T) {
	ctx, k, keepers := setup(t)
	creator := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 100_000_000))
	sponsor := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 1_000_000))
	refund := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 1))
	code, err := os.ReadFile("../../artifacts/native_fulfill_example.wasm")
	require.NoError(t, err)
	id, _, err := keepers.ContractKeeper.Create(ctx, creator, code, nil)
	require.NoError(t, err)
	owner, _, err := keepers.ContractKeeper.Instantiate(ctx, id, creator, creator, marshal(map[string]any{
		"payment_service": creator.String(), "shipment_service": creator.String(), "deadline_blocks": 10, "fee_denom": "ugas",
	}), "management", sdk.Coins{})
	require.NoError(t, err)
	execute := func(sender sdk.AccAddress, msg any, coins sdk.Coins) error {
		// Model the transaction boundary for bank transfers attached to contract calls.
		cached, commit := ctx.CacheContext()
		_, e := keepers.ContractKeeper.Execute(cached, owner, sender, marshal(msg), coins)
		if e == nil {
			commit()
		}
		return e
	}
	require.NoError(t, execute(creator, map[string]any{"Start": map[string]any{
		"order": map[string]any{"id": 9}, "business": sdk.NewInt64Coin("ugas", 0), "gas": sdk.NewInt64Coin("ugas", 2_000_000),
		"gas_per_poll": 100_000_000, "max_gas_price": "1", "expires_at": 100, "allocated_bytes": 524288,
	}}, sdk.NewCoins(sdk.NewInt64Coin("ugas", 2_000_000))))
	// First actual Wasm attempt fails outbound to the undeployed service, retaining its continuation.
	ctx = ctx.WithBlockHeight(11).WithGasMeter(storetypes.NewInfiniteGasMeter())
	ctx = k.ReserveOrdinaryGas(ctx, 400_000_000)
	ctx = k.EndBlock(ctx, 400_000_000)
	ctx = ctx.WithGasMeter(storetypes.NewInfiniteGasMeter())
	task, err := k.load(ctx, owner.String(), 1)
	require.NoError(t, err)
	require.Equal(t, "paused", task.Status)
	require.NoError(t, execute(sponsor, map[string]any{"Async": map[string]any{"fund_task": map[string]any{"task_id": 1}}}, sdk.NewCoins(sdk.NewInt64Coin("ugas", 1000))))
	lease := map[string]any{"ExtendLease": map[string]any{"task_id": 1, "allocated_bytes": 524388, "expires_at": 101}}
	budget := map[string]any{"UpdateBudget": map[string]any{"task_id": 1, "gas_per_poll": 100_000_000, "max_gas_price": "1.5", "refund": refund.String()}}
	for _, msg := range []any{lease, budget} {
		before := policySnapshot(ctx, k)
		require.ErrorContains(t, execute(sponsor, msg, sdk.Coins{}), "unauthorized")
		require.Error(t, execute(creator, msg, sdk.NewCoins(sdk.NewInt64Coin("ugas", 1))))
		require.Equal(t, before, policySnapshot(ctx, k))
	}
	task, err = k.load(ctx, owner.String(), 1)
	require.NoError(t, err)
	beforeEscrow, beforeLease := amount(task.Escrow), amount(task.LeasePaid)
	collector := authtypes.NewModuleAddress(authtypes.FeeCollectorName)
	beforeCollector := keepers.BankKeeper.GetBalance(ctx, collector, "ugas").Amount
	k.Config.StorageByteBlockPrice = sdkmath.OneInt()
	require.NoError(t, execute(creator, lease, sdk.Coins{})) // 100*(100-11)+524388*(101-100)=533288
	require.NoError(t, execute(creator, budget, sdk.Coins{}))
	task, err = k.load(ctx, owner.String(), 1)
	require.NoError(t, err)
	require.Equal(t, beforeEscrow.SubRaw(533288).String(), task.Escrow)
	require.Equal(t, beforeLease.AddRaw(533288).String(), task.LeasePaid)
	require.Equal(t, beforeCollector.AddRaw(533288), keepers.BankKeeper.GetBalance(ctx, collector, "ugas").Amount)
	require.Equal(t, task.Escrow, keepers.BankKeeper.GetBalance(ctx, authtypes.NewModuleAddress(ModuleName), "ugas").Amount.String())
	oldFees := task.Fees
	ctx = ctx.WithBlockHeight(12).WithGasMeter(storetypes.NewInfiniteGasMeter())
	ctx = k.ReserveOrdinaryGas(ctx, 400_000_000)
	ctx = k.EndBlock(ctx, 400_000_000)
	ctx = ctx.WithGasMeter(storetypes.NewInfiniteGasMeter())
	task, err = k.load(ctx, owner.String(), 1)
	require.NoError(t, err)
	require.Equal(t, "paused", task.Status)
	require.Equal(t, oldFees, task.Fees)
	require.NoError(t, execute(creator, map[string]any{"Async": map[string]any{"resume_task": map[string]any{"task_id": 1}}}, sdk.Coins{}))
	task, err = k.load(ctx, owner.String(), 1)
	require.NoError(t, err)
	require.Equal(t, "runnable", task.Status)
	ctx = ctx.WithBlockHeight(13).WithGasMeter(storetypes.NewInfiniteGasMeter())
	ctx = k.ReserveOrdinaryGas(ctx, 400_000_000)
	ctx = k.EndBlock(ctx, 400_000_000)
	ctx = ctx.WithGasMeter(storetypes.NewInfiniteGasMeter())
	task, err = k.load(ctx, owner.String(), 1)
	require.NoError(t, err)
	require.True(t, amount(task.Fees).GT(amount(oldFees)))
	remaining := amount(task.Escrow)
	require.NoError(t, execute(creator, map[string]any{"Async": map[string]any{"cancel_task": map[string]any{"task_id": 1}}}, sdk.Coins{}))
	require.Equal(t, remaining.AddRaw(1), keepers.BankKeeper.GetBalance(ctx, refund, "ugas").Amount)
	require.True(t, keepers.BankKeeper.GetBalance(ctx, authtypes.NewModuleAddress(ModuleName), "ugas").Amount.IsZero())
}

func policySnapshot(ctx sdk.Context, k *Keeper) map[string]string {
	result := map[string]string{}
	it := k.store(ctx).Iterator(nil, nil)
	defer it.Close()
	for ; it.Valid(); it.Next() {
		result[string(it.Key())] = string(it.Value())
	}
	return result
}

func TestManagementRejectsUnauthorizedExpiredInflightAndUnpaidChanges(t *testing.T) {
	ctx, k, keepers := setup(t)
	owner := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 100000))
	sponsor := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 1))
	policyTask(ctx, k, owner.String())
	lease := ExtendLeaseMsg{TaskID: 1, Actor: owner.String(), AllocatedBytes: 8292, ExpiresAt: 21}
	budget := UpdateBudgetMsg{TaskID: 1, Actor: owner.String(), GasPerPoll: 100_000_000, MaxGasPrice: "1", Refund: owner.String()}
	for _, c := range []sdk.Context{ctx.WithBlockHeight(20), ctx.WithValue(pollingKey{}, string(taskKey(owner.String(), 1)))} {
		before := policySnapshot(ctx, k)
		require.Error(t, k.ExtendLease(c, owner.String(), lease))
		require.Error(t, k.UpdateBudget(c, owner.String(), budget))
		require.Equal(t, before, policySnapshot(ctx, k))
	}
	for _, actor := range []string{sponsor.String(), "invalid"} {
		a, b := lease, budget
		a.Actor = actor
		b.Actor = actor
		before := policySnapshot(ctx, k)
		require.Error(t, k.ExtendLease(ctx, owner.String(), a))
		require.Error(t, k.UpdateBudget(ctx, owner.String(), b))
		require.Equal(t, before, policySnapshot(ctx, k))
	}
	for _, a := range []ExtendLeaseMsg{
		{TaskID: 1, Actor: owner.String(), AllocatedBytes: 8192, ExpiresAt: 20},
		{TaskID: 1, Actor: owner.String(), AllocatedBytes: 8191, ExpiresAt: 21},
		{TaskID: 1, Actor: owner.String(), AllocatedBytes: 8292, ExpiresAt: 19},
		{TaskID: 1, Actor: owner.String(), AllocatedBytes: 8292, ExpiresAt: ^uint64(0)},
		{TaskID: 1, Actor: owner.String(), AllocatedBytes: 600000, ExpiresAt: 21},
	} {
		before := policySnapshot(ctx, k)
		require.Error(t, k.ExtendLease(ctx, owner.String(), a))
		require.Equal(t, before, policySnapshot(ctx, k))
	}
	before := policySnapshot(ctx, k)
	k.Config.MaxBytes = 8192
	require.Error(t, k.ExtendLease(ctx, owner.String(), lease))
	require.Equal(t, before, policySnapshot(ctx, k))
	k.Config.MaxBytes = 100_000_000
	k.Config.StorageByteBlockPrice = sdkmath.NewInt(100)
	require.ErrorContains(t, k.ExtendLease(ctx, owner.String(), lease), "insufficient")
	require.Equal(t, before, policySnapshot(ctx, k))
	k.Config.StorageByteBlockPrice = amount("340282366920938463463374607431768211455")
	require.ErrorContains(t, k.ExtendLease(ctx, owner.String(), lease), "overflow")
	require.Equal(t, before, policySnapshot(ctx, k))
	for _, price := range []string{"-1", "+1", "1.0000000000000000001", "340282366920938463463.374607431768211456"} {
		b := budget
		b.MaxGasPrice = price
		require.Error(t, k.UpdateBudget(ctx, owner.String(), b))
		require.Equal(t, before, policySnapshot(ctx, k))
	}
	for _, gas := range []uint64{0, k.Config.HostReserve, k.Config.MaxPollGas + 1} {
		b := budget
		b.GasPerPoll = gas
		require.Error(t, k.UpdateBudget(ctx, owner.String(), b))
		require.Equal(t, before, policySnapshot(ctx, k))
	}
	for _, refund := range []string{"invalid", authtypes.NewModuleAddress(ModuleName).String()} {
		b := budget
		b.Refund = refund
		require.Error(t, k.UpdateBudget(ctx, owner.String(), b))
		require.Equal(t, before, policySnapshot(ctx, k))
	}
}

func TestManagementRefundAndExplicitResumeWithInsufficientFunds(t *testing.T) {
	ctx, k, keepers := setup(t)
	owner := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 100001))
	refund := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 1))
	require.NoError(t, keepers.BankKeeper.SendCoins(ctx, owner, authtypes.NewModuleAddress(ModuleName), sdk.NewCoins(sdk.NewInt64Coin("ugas", 100000))))
	task := policyTask(ctx, k, owner.String())
	task.Status = "paused"
	task.Pause = "out_of_gas"
	task.Escrow = "99999"
	k.save(ctx, task)
	require.NoError(t, k.UpdateBudget(ctx, owner.String(), UpdateBudgetMsg{TaskID: 1, Actor: owner.String(), GasPerPoll: 100_000_000, MaxGasPrice: "1", Refund: refund.String()}))
	resume := marshal(map[string]any{"resume_task": map[string]any{"task_id": 1, "actor": owner.String()}})
	require.Error(t, k.Management(ctx, owner.String(), resume))
	require.NoError(t, k.Management(ctx, owner.String(), marshal(map[string]any{"fund_task": FundTaskMsg{TaskID: 1, Contributor: refund.String(), Funds: sdk.NewInt64Coin("ugas", 1)}})))
	task, e := k.load(ctx, owner.String(), 1)
	require.NoError(t, e)
	require.Equal(t, "paused", task.Status)
	require.NoError(t, k.Management(ctx, owner.String(), resume))
	task, e = k.load(ctx, owner.String(), 1)
	require.NoError(t, e)
	require.Equal(t, "waiting", task.Status)
	require.NoError(t, k.Management(ctx, owner.String(), marshal(map[string]any{"cancel_task": map[string]any{"task_id": 1, "actor": owner.String()}})))
	require.Equal(t, "100001", keepers.BankKeeper.GetBalance(ctx, refund, "ugas").Amount.String())
}

func TestManagementNewEncodingsRecheckQuotaAndBankFailureRollsBack(t *testing.T) {
	ctx, k, keepers := setup(t)
	owner := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 1))
	task := policyTask(ctx, k, owner.String())
	// An exact admitted boundary must not permit a larger budget encoding.
	for task.AllocatedBytes = 1024; k.checkQuota(task) != nil; task.AllocatedBytes++ {
	}
	k.save(ctx, task)
	before := policySnapshot(ctx, k)
	require.Error(t, k.UpdateBudget(ctx, owner.String(), UpdateBudgetMsg{TaskID: 1, Actor: owner.String(), GasPerPoll: 100_000_000, MaxGasPrice: "340282366920938463463.374607431768211455", Refund: owner.String()}))
	require.Equal(t, before, policySnapshot(ctx, k))
	k.Config.StorageByteBlockPrice = sdkmath.OneInt()
	require.Error(t, k.ExtendLease(ctx, owner.String(), ExtendLeaseMsg{TaskID: 1, Actor: owner.String(), AllocatedBytes: 8192, ExpiresAt: 21}))
	require.Equal(t, before, policySnapshot(ctx, k))
}

func policyTask(ctx sdk.Context, k *Keeper, owner string) Task {
	task := Task{ID: 1, Owner: owner, Status: "waiting", Escrow: "100000", Reserved: "0", Fees: "9", LeasePaid: "100",
		Register: Register{Control: owner, Refund: owner, GasPerPoll: 80_000_000, MaxGasPrice: "1", ExpiresAt: 20, AllocatedBytes: 8192, Program: Program{Kind: "test", Version: 1, Input: []byte{}}},
		Sequence: 1, Started: true, State: []byte("saved"), Wait: &Wait{},
	}
	task.Wait.Operation = &struct {
		Resolver       string `json:"resolver"`
		Deadline       uint64 `json:"deadline"`
		MaxResultBytes uint32 `json:"max_result_bytes"`
	}{owner, 15, 10}
	task.ExpiryKey = indexKey("expiry", 20, task.ID, owner)
	k.store(ctx).Set(task.ExpiryKey, taskKey(owner, task.ID))
	setNumber(k.store(ctx), "bytes", 8192)
	setNumber(k.store(ctx), "active", 1)
	setNumber(k.store(ctx), "active/"+owner, 1)
	k.save(ctx, task)
	return task
}

func TestLeaseManagementExactRentAndIndex(t *testing.T) {
	for _, tc := range []struct {
		bytes, expiry uint64
		rent          string
	}{{8292, 20, "1000"}, {8192, 21, "8192"}, {8292, 21, "9292"}} {
		ctx, k, keepers := setup(t)
		owner := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 100000))
		require.NoError(t, keepers.BankKeeper.SendCoins(ctx, owner, authtypes.NewModuleAddress(ModuleName), sdk.NewCoins(sdk.NewInt64Coin("ugas", 100000))))
		old := policyTask(ctx, k, owner.String())
		k.Config.StorageByteBlockPrice = sdkmath.OneInt()
		require.NoError(t, k.ExtendLease(ctx, owner.String(), ExtendLeaseMsg{TaskID: 1, Actor: owner.String(), AllocatedBytes: tc.bytes, ExpiresAt: tc.expiry}))
		task, e := k.load(ctx, owner.String(), 1)
		require.NoError(t, e)
		require.Equal(t, amount("100000").Sub(amount(tc.rent)).String(), task.Escrow)
		require.Equal(t, amount("100").Add(amount(tc.rent)).String(), task.LeasePaid)
		require.Equal(t, old.Wait, task.Wait)
		require.Equal(t, old.State, task.State)
		require.Equal(t, old.Sequence, task.Sequence)
		require.Equal(t, old.Status, task.Status)
		require.Equal(t, old.Fees, task.Fees)
		require.Equal(t, tc.bytes, number(k.store(ctx), "bytes"))
		if tc.expiry != old.ExpiresAt {
			require.False(t, k.store(ctx).Has(old.ExpiryKey))
		}
		require.True(t, k.store(ctx).Has(indexKey("expiry", tc.expiry, 1, owner.String())))
		require.Equal(t, tc.rent, keepers.BankKeeper.GetBalance(ctx, authtypes.NewModuleAddress(authtypes.FeeCollectorName), "ugas").Amount.String())
	}
}

func TestBudgetManagementPreservesPauseAndRefundAuthority(t *testing.T) {
	ctx, k, keepers := setup(t)
	owner := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 100000))
	refund := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 1))
	task := policyTask(ctx, k, owner.String())
	task.Status = "paused"
	task.Pause = "out_of_gas"
	k.save(ctx, task)
	msg := UpdateBudgetMsg{TaskID: 1, Actor: owner.String(), GasPerPoll: 100_000_000, MaxGasPrice: "1.5", Refund: refund.String()}
	require.NoError(t, k.UpdateBudget(ctx, owner.String(), msg))
	next, e := k.load(ctx, owner.String(), 1)
	require.NoError(t, e)
	require.Equal(t, "paused", next.Status)
	require.Equal(t, "out_of_gas", next.Pause)
	require.Equal(t, task.Escrow, next.Escrow)
	require.Equal(t, task.LeasePaid, next.LeasePaid)
	require.Equal(t, task.Fees, next.Fees)
	require.Equal(t, uint64(100_000_000), next.GasPerPoll)
	require.Equal(t, "1.5", next.MaxGasPrice)
	require.Equal(t, refund.String(), next.Refund)
	require.Equal(t, task.Wait, next.Wait)
	require.NoError(t, k.UpdateBudget(ctx, owner.String(), msg))
}

func TestLeaseManagementExpirySurvivesKeeperRecreation(t *testing.T) {
	ctx, k, keepers := setup(t)
	owner := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 100000))
	require.NoError(t, keepers.BankKeeper.SendCoins(ctx, owner, authtypes.NewModuleAddress(ModuleName), sdk.NewCoins(sdk.NewInt64Coin("ugas", 100000))))
	policyTask(ctx, k, owner.String())
	require.NoError(t, k.ExtendLease(ctx, owner.String(), ExtendLeaseMsg{TaskID: 1, Actor: owner.String(), AllocatedBytes: 8292, ExpiresAt: 25}))
	restored := &Keeper{Key: k.Key, Bank: k.Bank, Wasm: k.Wasm, Config: k.Config}
	require.False(t, restored.maintenanceOne(ctx.WithBlockHeight(20), "expiry"))
	require.True(t, restored.maintenanceOne(ctx.WithBlockHeight(25), "expiry"))
	task, e := restored.load(ctx, owner.String(), 1)
	require.NoError(t, e)
	require.Equal(t, "expired", task.Status)
	require.Equal(t, "100000", keepers.BankKeeper.GetBalance(ctx, owner, "ugas").Amount.String())
	require.True(t, restored.maintenanceOne(ctx.WithBlockHeight(25), "prune"))
	_, e = restored.load(ctx, owner.String(), 1)
	require.Error(t, e)
	require.Equal(t, uint64(0), number(restored.store(ctx), "bytes"))
}
