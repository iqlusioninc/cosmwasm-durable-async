package async

import (
	"bytes"
	"context"
	"fmt"
	"testing"

	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	"github.com/stretchr/testify/require"
)

// An unbacked or send-restricted refund must not halt EndBlock. The task still
// reaches its terminal state and the unrefunded escrow is reported.
func TestRejectedRefundDoesNotHaltScheduler(t *testing.T) {
	for _, path := range []string{"expiry", "ready"} {
		t.Run(path, func(t *testing.T) {
			ctx, k, _ := setup(t)
			owner := sdk.AccAddress(bytes.Repeat([]byte{1}, 20)).String()
			// Escrow is recorded but the module account holds nothing, so the
			// refund transfer fails.
			task := Task{ID: 1, Owner: owner, Status: "runnable", Escrow: "1000", Fees: "0", Reserved: "0", LeasePaid: "0", Register: Register{Control: owner, Refund: owner, GasPerPoll: 80_000_000, MaxGasPrice: "1", ExpiresAt: 10, AllocatedBytes: 4096}}
			s := k.store(ctx)
			task.ExpiryKey = indexKey("expiry", 10, 1, owner)
			s.Set(task.ExpiryKey, taskKey(owner, 1))
			if path == "ready" {
				k.enqueue(ctx, &task, 10)
			}
			k.save(ctx, task)
			setNumber(s, "active", 1)
			setNumber(s, "active/"+owner, 1)
			setNumber(s, "bytes", 4096)
			if path == "ready" {
				// The ready scan can meet an expiry before maintenance does.
				require.NotPanics(t, func() { k.selectReady(ctx, 90_000_000, nil) })
			} else {
				ctx = k.ReserveOrdinaryGas(ctx, 400_000_000)
				require.NotPanics(t, func() { ctx = k.EndBlock(ctx, 400_000_000) })
			}
			// The terminal record may already be pruned at its lease endpoint.
			if got, err := k.load(ctx, owner, 1); err == nil {
				require.Equal(t, "expired", got.Status)
				require.Equal(t, "0", got.Escrow)
			}
			require.Equal(t, uint64(0), number(k.store(ctx), "active"))
			found := false
			for _, e := range ctx.EventManager().Events() {
				if e.Type == "async_refund_failed" {
					found = true
				}
			}
			require.True(t, found, "stranded refund must be observable")
		})
	}
}

// User-initiated cancellation stays strict: the tx fails and nothing changes,
// so the controller can fix the refund address and retry.
func TestRejectedRefundFailsCancellation(t *testing.T) {
	ctx, k, _ := setup(t)
	owner := sdk.AccAddress(bytes.Repeat([]byte{1}, 20)).String()
	task := Task{ID: 1, Owner: owner, Status: "waiting", Escrow: "1000", Fees: "0", Reserved: "0", LeasePaid: "0", Register: Register{Control: owner, Refund: owner, ExpiresAt: 100}}
	task.ExpiryKey = indexKey("expiry", 100, 1, owner)
	k.store(ctx).Set(task.ExpiryKey, taskKey(owner, 1))
	k.save(ctx, task)
	setNumber(k.store(ctx), "active", 1)
	cache, _ := ctx.CacheContext()
	require.Error(t, k.Management(cache, owner, marshal(map[string]any{"cancel_task": map[string]any{"task_id": 1, "actor": owner}})))
	got, err := k.load(ctx, owner, 1)
	require.NoError(t, err)
	require.Equal(t, "waiting", got.Status)
}

type restrictedBank struct {
	Bank
	blocked map[string]bool
}

func (b restrictedBank) SendCoins(ctx context.Context, from, to sdk.AccAddress, amt sdk.Coins) error {
	if b.blocked[to.String()] {
		return fmt.Errorf("send restricted")
	}
	return b.Bank.SendCoins(ctx, from, to, amt)
}

// A restricted refund address falls back to the controller, then to the
// owning contract; only if all three reject does escrow remain stranded.
func TestRejectedRefundFallsBackToControlThenOwner(t *testing.T) {
	refund := sdk.AccAddress(bytes.Repeat([]byte{1}, 20))
	control := sdk.AccAddress(bytes.Repeat([]byte{2}, 20))
	owner := sdk.AccAddress(bytes.Repeat([]byte{3}, 20))
	for _, tc := range []struct {
		name      string
		blocked   []sdk.AccAddress
		recipient sdk.AccAddress
		event     string
	}{
		{"refund", nil, refund, ""},
		{"control", []sdk.AccAddress{refund}, control, "async_refund_redirected"},
		{"owner", []sdk.AccAddress{refund, control}, owner, "async_refund_redirected"},
		{"stranded", []sdk.AccAddress{refund, control, owner}, nil, "async_refund_failed"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			ctx, k, keepers := setup(t)
			keepers.Faucet.Fund(ctx, authtypes.NewModuleAddress(ModuleName), sdk.NewInt64Coin("ugas", 1000))
			blocked := map[string]bool{}
			for _, a := range tc.blocked {
				blocked[a.String()] = true
			}
			k.Bank = restrictedBank{k.Bank, blocked}
			task := Task{ID: 1, Owner: owner.String(), Status: "runnable", Escrow: "1000", Fees: "0", Reserved: "0", LeasePaid: "0", Register: Register{Control: control.String(), Refund: refund.String(), ExpiresAt: 10, AllocatedBytes: 4096}}
			task.ExpiryKey = indexKey("expiry", 10, 1, task.Owner)
			k.store(ctx).Set(task.ExpiryKey, taskKey(task.Owner, 1))
			k.save(ctx, task)
			setNumber(k.store(ctx), "active", 1)
			ctx = ctx.WithEventManager(sdk.NewEventManager())
			k.settle(ctx, &task, "expired")
			require.Equal(t, "0", task.Escrow)
			require.Equal(t, refund.String(), task.Refund, "stored refund address is unchanged")
			for _, a := range []sdk.AccAddress{refund, control, owner} {
				want := int64(0)
				if tc.recipient != nil && a.Equals(tc.recipient) {
					want = 1000
				}
				require.Equal(t, want, keepers.BankKeeper.GetBalance(ctx, a, "ugas").Amount.Int64(), a.String())
			}
			found := ""
			for _, e := range ctx.EventManager().Events() {
				if e.Type == "async_refund_redirected" || e.Type == "async_refund_failed" {
					found = e.Type
				}
			}
			require.Equal(t, tc.event, found)
		})
	}
}
