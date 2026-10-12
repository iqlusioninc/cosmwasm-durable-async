package async

import (
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	"github.com/stretchr/testify/require"
	"testing"
)

// Catches accepting unauthenticated/mismatched local scheduler configuration.
func TestSnapshotRequiresConsensusManifest(t *testing.T) {
	h := newFaultHarness(t)
	require.Error(t, h.k.ValidateState(h.ctx))
	require.NoError(t, h.k.InitState(h.ctx))
	require.NoError(t, h.k.ValidateState(h.ctx))
	h.k.Config.MaxVisits++
	require.Error(t, h.k.ValidateState(h.ctx))
	require.Error(t, h.k.InitState(h.ctx), "initialization must never overwrite an existing manifest")
}

// Catches broken task/index/counter/escrow state without making restore execute tasks.
func TestSnapshotSchedulerInvariants(t *testing.T) {
	h := newFaultHarness(t)
	require.NoError(t, h.k.InitState(h.ctx))
	h.start(t, 80_000_000, 100)
	before := policySnapshot(h.ctx, h.k)
	require.NoError(t, h.k.ValidateState(h.ctx))
	require.Equal(t, before, policySnapshot(h.ctx, h.k))
	for name, corrupt := range map[string]func(sdk.Context){
		"counter":           func(ctx sdk.Context) { setNumber(h.k.store(ctx), "active", 0) },
		"queue":             func(ctx sdk.Context) { h.k.store(ctx).Delete(h.task(t, 1).QueueKey) },
		"expiry":            func(ctx sdk.Context) { h.k.store(ctx).Delete(h.task(t, 1).ExpiryKey) },
		"reservation":       func(ctx sdk.Context) { v := h.task(t, 1); v.Reserved = "1"; h.k.save(ctx, v) },
		"status":            func(ctx sdk.Context) { v := h.task(t, 1); v.Status = "completing"; h.k.save(ctx, v) },
		"code_binding":      func(ctx sdk.Context) { v := h.task(t, 1); v.Checksum = []byte("bad"); h.k.save(ctx, v) },
		"escrow":            func(ctx sdk.Context) { v := h.task(t, 1); v.Escrow = "999999999999999"; h.k.save(ctx, v) },
		"unknown_key":       func(ctx sdk.Context) { h.k.store(ctx).Set([]byte("unknown"), []byte{1}) },
		"malformed_counter": func(ctx sdk.Context) { h.k.store(ctx).Set([]byte("next_id"), []byte{1}) },
	} {
		t.Run(name, func(t *testing.T) {
			ctx, _ := h.ctx.CacheContext()
			corrupt(ctx)
			require.Error(t, h.k.ValidateState(ctx))
		})
	}
	// Direct module donations are legitimate; backing must be >= claims, not equal.
	require.NoError(t, h.keepers.BankKeeper.SendCoins(h.ctx, h.creator, authtypes.NewModuleAddress(ModuleName), sdk.NewCoins(sdk.NewInt64Coin("ugas", 1))))
	require.NoError(t, h.k.ValidateState(h.ctx))
	h.block(11)
	require.NoError(t, h.k.ValidateState(h.ctx))
	h.execute(t, map[string]any{"deliver": map[string]any{"task_id": 1, "wait_sequence": 1}}, 0)
	require.NoError(t, h.k.ValidateState(h.ctx))
	h.execute(t, map[string]any{"mode": map[string]any{"value": "trap"}}, 0)
	h.block(12)
	require.NoError(t, h.k.ValidateState(h.ctx))
	h.block(100)
	require.NoError(t, h.k.ValidateState(h.ctx))
}

// Catches two durable tasks pointing at one readiness slot, which would strand
// one task even though each stored index value individually looks well formed.
func TestSnapshotRejectsSharedReadinessSlot(t *testing.T) {
	h := newFaultHarness(t)
	require.NoError(t, h.k.InitState(h.ctx))
	h.start(t, 80_000_000, 100)
	h.start(t, 80_000_000, 100)
	first, second := h.task(t, 1), h.task(t, 2)
	h.k.store(h.ctx).Delete(second.QueueKey)
	second.QueueKey = first.QueueKey
	h.k.store(h.ctx).Set(second.QueueKey, taskKey(second.Owner, second.ID))
	h.k.save(h.ctx, second)
	require.Error(t, h.k.ValidateState(h.ctx))
}

// Catches an inbox that bypasses Deliver's result bound despite its wait having
// prepaid only a smaller maximum payload.
func TestSnapshotRejectsOversizedInbox(t *testing.T) {
	h := newFaultHarness(t)
	require.NoError(t, h.k.InitState(h.ctx))
	h.start(t, 80_000_000, 100)
	h.block(11)
	h.execute(t, map[string]any{"deliver": map[string]any{"task_id": 1, "wait_sequence": 1}}, 0)
	task := h.task(t, 1)
	require.NotNil(t, task.Inbox.Success)
	task.Inbox.Success.Data = make([]byte, int(task.Wait.Operation.MaxResultBytes)+1)
	h.k.save(h.ctx, task)
	require.Error(t, h.k.ValidateState(h.ctx))
}
