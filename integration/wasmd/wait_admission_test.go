package async

import (
	"bytes"
	"encoding/json"
	"os"
	"strings"
	"testing"

	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/stretchr/testify/require"
)

type waitAdmissionCase struct {
	Name           string `json:"name"`
	Height         uint64 `json:"height"`
	Expiry         uint64 `json:"expiry"`
	Kind           string `json:"kind"`
	Due            uint64 `json:"due"`
	Resolver       string `json:"resolver"`
	ExpectedAccept bool   `json:"expected_accept"`
}

// Catches a widened lease boundary or accepting alternate address spelling.
func TestWaitAdmissionSharedVectors(t *testing.T) {
	require.Equal(t, "cosmos", sdk.GetConfig().GetBech32AccountAddrPrefix())
	// The paired MockApi and this SDK policy accept 1..=255 decoded bytes.
	require.NoError(t, sdk.VerifyAddressFormat(bytes.Repeat([]byte{1}, 32)))
	require.Error(t, sdk.VerifyAddressFormat(bytes.Repeat([]byte{1}, 256)))
	data, err := os.ReadFile("../../fixtures/native-async/wait-admission.json")
	require.NoError(t, err)
	var cases []waitAdmissionCase
	require.NoError(t, json.Unmarshal(data, &cases))
	ctx, k, _ := setup(t)
	for _, tc := range cases {
		t.Run(tc.Name, func(t *testing.T) {
			child, _ := ctx.CacheContext()
			child = child.WithBlockHeight(int64(tc.Height))
			task := queuedAuctionTask(child, k, 1, "1", 80_000_000, tc.Height)
			task.ExpiresAt = tc.Expiry
			selected, stop := k.selectReady(child, 100_000_000, nil)
			require.False(t, stop)
			require.NotNil(t, selected)
			task.QueueKey = nil
			var wait any
			switch tc.Kind {
			case "timer":
				wait = map[string]any{"height": map[string]any{"height": tc.Due}}
			case "operation":
				wait = map[string]any{"operation": map[string]any{"resolver": tc.Resolver, "deadline": tc.Due, "max_result_bytes": 20}}
			default:
				t.Fatalf("unknown fixture kind %q", tc.Kind)
			}
			var response PollResponse
			require.NoError(t, json.Unmarshal(marshal(map[string]any{"outcome": map[string]any{"pending": map[string]any{"state": []byte("checkpoint"), "wait": wait}}, "messages": []any{}}), &response))
			err := k.stage(child, &task, response)
			if tc.ExpectedAccept {
				require.NoError(t, err)
				stored, err := k.load(child, task.Owner, task.ID)
				require.NoError(t, err)
				require.Equal(t, "waiting", stored.Status)
				require.Equal(t, &response.Outcome.Pending.Wait, stored.Wait)
				require.Equal(t, []byte("checkpoint"), stored.State)
				require.NotNil(t, k.store(child).Get(stored.TimerKey))
			} else {
				require.Error(t, err)
			}
		})
	}
}

// Invalid v1 waits must block restoration without rewriting or dropping them.
func TestSnapshotWaitAdmissionFailsClosed(t *testing.T) {
	h := newFaultHarness(t)
	require.NoError(t, h.k.InitState(h.ctx))
	h.start(t, 80_000_000, 100)
	h.block(11)
	original := h.task(t, 1)
	require.NotNil(t, original.Wait.Operation)
	for _, name := range []string{"zero_due", "due_equals_expiry", "due_after_expiry", "timer_zero_due", "timer_due_equals_expiry", "timer_due_after_expiry", "uppercase_resolver", "malformed_resolver", "transient_marker"} {
		t.Run(name, func(t *testing.T) {
			ctx, _ := h.ctx.CacheContext()
			task, err := h.k.load(ctx, original.Owner, original.ID)
			require.NoError(t, err)
			switch strings.TrimPrefix(name, "timer_") {
			case "zero_due":
				task.Wait.Operation.Deadline = 0
			case "due_equals_expiry":
				task.Wait.Operation.Deadline = task.ExpiresAt
			case "due_after_expiry":
				task.Wait.Operation.Deadline = task.ExpiresAt + 1
			case "uppercase_resolver":
				task.Wait.Operation.Resolver = strings.ToUpper(task.Wait.Operation.Resolver)
			case "malformed_resolver":
				task.Wait.Operation.Resolver = "cosmos1invalid"
			case "transient_marker":
				h.k.store(ctx).Set([]byte("na2/transient/flight/"+task.Owner+"/1"), []byte("1"))
			}
			h.k.store(ctx).Delete(task.TimerKey)
			task.ReadyAt = task.Wait.Operation.Deadline
			if strings.HasPrefix(name, "timer_") {
				var wait Wait
				require.NoError(t, json.Unmarshal(marshal(map[string]any{"height": map[string]any{"height": task.ReadyAt}}), &wait))
				task.Wait = &wait
			}
			task.TimerKey = indexKey("timer", task.ReadyAt, task.ReadySequence, task.Owner)
			h.k.store(ctx).Set(task.TimerKey, taskKey(task.Owner, task.ID))
			h.k.save(ctx, task)
			before := policySnapshot(ctx, h.k)
			require.Error(t, h.k.ValidateState(ctx))
			require.Equal(t, before, policySnapshot(ctx, h.k), "restore validation must not alter offending state")
		})
	}
}

// Catches treating ABI v1 as obsolete or rejecting a previously accepted
// outcome after a later failed poll advances LastPoll beyond its deadline.
func TestRestoreValidActiveV1AndRetainedOutcome(t *testing.T) {
	h := newFaultHarness(t)
	require.NoError(t, h.k.InitState(h.ctx))
	h.start(t, 80_000_000, 100)
	h.block(11)
	restore := func() {
		before := policySnapshot(h.ctx, h.k)
		h.k = &Keeper{Key: h.k.Key, Config: h.k.Config, Bank: h.k.Bank, Wasm: h.k.Wasm, messenger: h.k.messenger}
		require.NoError(t, h.k.ValidateState(h.ctx))
		require.Equal(t, before, policySnapshot(h.ctx, h.k))
		require.Equal(t, uint32(1), h.task(t, 1).Program.Version)
	}
	restore()
	deadline := h.task(t, 1).Wait.Operation.Deadline
	h.ctx = h.ctx.WithBlockHeight(int64(deadline) - 1)
	h.execute(t, map[string]any{"deliver": map[string]any{"task_id": 1, "wait_sequence": 1}}, 0)
	h.execute(t, map[string]any{"mode": map[string]any{"value": "trap"}}, 0)
	h.block(int64(deadline))
	task := h.task(t, 1)
	require.Equal(t, "paused", task.Status)
	require.Equal(t, "trap", task.Pause)
	require.NotNil(t, task.Inbox.Success)
	require.LessOrEqual(t, task.Wait.Operation.Deadline, task.LastPoll)
	restore()
}
