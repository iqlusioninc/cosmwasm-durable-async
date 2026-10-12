package async

import (
	"bytes"
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

// Restored scheduler state must use canonical due/bid keys bound to the exact
// task identity, bid, original readiness height and sequence.
func TestSnapshotAuctionIndexValidationAndRestart(t *testing.T) {
	h := newFaultHarness(t)
	h.k.Config.MaxVisits = 1
	require.NoError(t, h.k.InitState(h.ctx))
	h.start(t, 80_000_000, 100)
	h.start(t, 80_000_000, 100)
	first := h.task(t, 1)
	require.Equal(t, []byte("due/00000000000000000011/00000000000000000001"), first.QueueKey)
	require.NoError(t, h.k.ValidateState(h.ctx))
	for name, corrupt := range map[string]func(sdk.Context){
		"missing_due":  func(ctx sdk.Context) { h.k.store(ctx).Delete(first.QueueKey) },
		"due_height":   func(ctx sdk.Context) { v := first; v.ReadyAt++; h.k.save(ctx, v) },
		"due_sequence": func(ctx sdk.Context) { v := first; v.ReadySequence++; h.k.save(ctx, v) },
		"wrong_task":   func(ctx sdk.Context) { h.k.store(ctx).Set(first.QueueKey, taskKey(first.Owner, 2)) },
		"noncanonical_due": func(ctx sdk.Context) {
			v := first
			h.k.store(ctx).Delete(v.QueueKey)
			v.QueueKey = []byte("due/11/1")
			h.k.store(ctx).Set(v.QueueKey, taskKey(v.Owner, v.ID))
			h.k.save(ctx, v)
		},
	} {
		t.Run(name, func(t *testing.T) {
			ctx, _ := h.ctx.CacheContext()
			corrupt(ctx)
			require.Error(t, h.k.ValidateState(ctx))
		})
	}
	h.block(11)
	second := h.task(t, 2)
	require.True(t, bytes.HasPrefix(second.QueueKey, []byte("bid/")))
	require.Equal(t, uint64(11), second.ReadyAt)
	require.Equal(t, uint64(2), second.ReadySequence)
	require.NoError(t, h.k.ValidateState(h.ctx))
	for name, corrupt := range map[string]func(sdk.Context){
		"missing_bid":  func(ctx sdk.Context) { h.k.store(ctx).Delete(second.QueueKey) },
		"bid_value":    func(ctx sdk.Context) { h.k.store(ctx).Set(second.QueueKey, taskKey(second.Owner, 1)) },
		"bid_price":    func(ctx sdk.Context) { v := second; v.MaxGasPrice = "2"; h.k.save(ctx, v) },
		"bid_height":   func(ctx sdk.Context) { v := second; v.ReadyAt++; h.k.save(ctx, v) },
		"bid_sequence": func(ctx sdk.Context) { v := second; v.ReadySequence++; h.k.save(ctx, v) },
		"noncanonical_bid": func(ctx sdk.Context) {
			v := second
			h.k.store(ctx).Delete(v.QueueKey)
			v.QueueKey = append([]byte("bid/"), bytes.ToUpper(v.QueueKey[4:])...)
			h.k.store(ctx).Set(v.QueueKey, taskKey(v.Owner, v.ID))
			h.k.save(ctx, v)
		},
		"stale_bid": func(ctx sdk.Context) {
			h.k.store(ctx).Set([]byte("bid/00000000000000000000000000000000/00000000000000000011/00000000000000000099"), taskKey(second.Owner, 2))
		},
	} {
		t.Run(name, func(t *testing.T) {
			ctx, _ := h.ctx.CacheContext()
			corrupt(ctx)
			require.Error(t, h.k.ValidateState(ctx))
		})
	}
	before := policySnapshot(h.ctx, h.k)
	h.k = &Keeper{Key: h.k.Key, Config: h.k.Config, Bank: h.k.Bank, Wasm: h.k.Wasm, messenger: h.k.messenger}
	require.NoError(t, h.k.ValidateState(h.ctx))
	require.Equal(t, before, policySnapshot(h.ctx, h.k))
	h.block(12)
	require.Equal(t, uint64(12), h.task(t, 2).LastPoll)
	require.NoError(t, h.k.ValidateState(h.ctx))
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
