package async

import (
	"bytes"
	"encoding/json"
	"fmt"
	"testing"

	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/stretchr/testify/require"
)

func queuedAuctionTask(ctx sdk.Context, k *Keeper, id uint64, bid string, gas, height uint64) Task {
	owner := sdk.AccAddress(bytes.Repeat([]byte{byte(id)}, 20)).String()
	task := Task{ID: id, Owner: owner, Status: "runnable", Escrow: "999999999999", Fees: "0", Reserved: "0", LeasePaid: "0", Register: Register{Control: owner, Refund: owner, Funds: sdk.NewInt64Coin("ugas", 1), GasPerPoll: gas, MaxGasPrice: bid, ExpiresAt: 100, AllocatedBytes: 4096}}
	k.enqueue(ctx, &task, height)
	k.save(ctx, task)
	return task
}

func stageAuctionTimer(t *testing.T, ctx sdk.Context, k *Keeper, task *Task, due uint64) {
	t.Helper()
	var response PollResponse
	require.NoError(t, json.Unmarshal([]byte(fmt.Sprintf(`{"outcome":{"pending":{"state":"AQ==","wait":{"height":{"height":%d}}}},"messages":[]}`, due)), &response))
	require.NoError(t, k.stage(ctx, task, response))
}

// Moving the timer wake to h+1 would leave this real Wasm task waiting at h=12.
func TestTimerEligibleAtRequestedHeight(t *testing.T) {
	h := newFaultHarness(t)
	h.start(t, 80_000_000, 100)
	h.block(11)
	task := h.task(t, 1)
	stageAuctionTimer(t, h.ctx, h.k, &task, 12)
	h.block(12)
	require.Equal(t, uint64(12), h.task(t, 1).LastPoll)
	require.Equal(t, "completed", h.task(t, 1).Status)
}

// A timer reserved first must beat a later ordinary delivery at the same height
// and bid, even though deliveries happen before scheduler timer processing.
func TestTimerFIFOAgainstSameHeightDelivery(t *testing.T) {
	ctx, k, _ := setup(t)
	older := queuedAuctionTask(ctx, k, 1, "2", 80_000_000, 10)
	selected, stop := k.selectReady(ctx, 100_000_000, nil)
	require.False(t, stop)
	stageAuctionTimer(t, ctx, k, selected, 12)
	ctx = ctx.WithBlockHeight(12)
	newer := queuedAuctionTask(ctx, k, 2, "2", 80_000_000, 12)
	k.store(ctx).Delete(newer.QueueKey)
	newer.QueueKey = nil
	newer.Status, newer.Started, newer.Sequence = "waiting", true, 1
	require.NoError(t, json.Unmarshal([]byte(fmt.Sprintf(`{"operation":{"resolver":%q,"deadline":90,"max_result_bytes":32}}`, newer.Owner)), &newer.Wait))
	k.save(ctx, newer)
	var resolution Resolution
	require.NoError(t, json.Unmarshal([]byte(`{"success":{"data":"b2s="}}`), &resolution))
	require.NoError(t, k.Deliver(ctx, newer.Owner, newer.ID, 1, newer.Owner, resolution))
	scheduler := ctx.WithValue(schedulerKey{}, true)
	require.True(t, k.maintenanceOne(scheduler, "timer"))
	require.True(t, k.promoteDue(scheduler))
	ready, err := k.load(ctx, older.Owner, older.ID)
	require.NoError(t, err)
	require.Equal(t, uint64(12), ready.ReadyAt)
	got, stop := k.selectReady(ctx, 100_000_000, nil)
	require.False(t, stop)
	require.Equal(t, older.ID, got.ID)
}

// A one-entry timer cap must follow wait staging order, even when task IDs and
// staging order disagree and maintenance processes an overdue timer.
func TestCappedTimersFollowOriginalReadinessSequence(t *testing.T) {
	ctx, k, _ := setup(t)
	queuedAuctionTask(ctx, k, 1, "1", 80_000_000, 10)
	second := queuedAuctionTask(ctx, k, 2, "2", 80_000_000, 10)
	for _, want := range []uint64{2, 1} {
		selected, stop := k.selectReady(ctx, 100_000_000, nil)
		require.False(t, stop)
		require.Equal(t, want, selected.ID)
		stageAuctionTimer(t, ctx, k, selected, 12)
	}
	require.NoError(t, k.UpdateBudget(ctx, second.Owner, UpdateBudgetMsg{TaskID: 2, Actor: second.Control, GasPerPoll: second.GasPerPoll, MaxGasPrice: "1", Refund: second.Refund}))
	ctx = ctx.WithBlockHeight(14).WithValue(schedulerKey{}, true)
	require.True(t, k.maintenanceOne(ctx, "timer"))
	require.True(t, k.promoteDue(ctx))
	got, stop := k.selectReady(ctx, 100_000_000, nil)
	require.False(t, stop)
	require.Equal(t, uint64(2), got.ID)
	require.Equal(t, uint64(12), got.ReadyAt)
	require.Equal(t, uint64(3), got.ReadySequence)
}

// Accepted delivery replaces the timeout readiness slot. Keeping its old timer
// would either invalidate the new sequence or reset the delivered task's age.
func TestDeliveryRemovesObsoleteTimeoutIndex(t *testing.T) {
	h := newFaultHarness(t)
	require.NoError(t, h.k.InitState(h.ctx))
	h.start(t, 80_000_000, 100)
	h.block(11)
	waiting := h.task(t, 1)
	h.ctx = h.ctx.WithBlockHeight(12)
	h.execute(t, map[string]any{"deliver": map[string]any{"task_id": 1, "wait_sequence": 1}}, 0)
	ready := h.task(t, 1)
	require.Nil(t, ready.TimerKey)
	require.Nil(t, h.k.store(h.ctx).Get(waiting.TimerKey))
	require.Equal(t, waiting.Wait, ready.Wait)
	require.NotNil(t, ready.Inbox)
	require.Equal(t, uint64(12), ready.ReadyAt)
	require.Greater(t, ready.ReadySequence, waiting.ReadySequence)
	require.NoError(t, h.k.ValidateState(h.ctx))
	corrupt, _ := h.ctx.CacheContext()
	withObsoleteTimer := ready
	withObsoleteTimer.TimerKey = waiting.TimerKey
	h.k.store(corrupt).Set(waiting.TimerKey, taskKey(ready.Owner, ready.ID))
	h.k.save(corrupt, withObsoleteTimer)
	require.Error(t, h.k.ValidateState(corrupt))
	h.ctx = h.ctx.WithBlockHeight(int64(waiting.Wait.Operation.Deadline))
	require.False(t, h.k.maintenanceOne(h.ctx.WithValue(schedulerKey{}, true), "timer"))
	require.Equal(t, ready, h.task(t, 1))
	require.NoError(t, h.k.ValidateState(h.ctx))
}

// Removing the below-base stop would select or pause the cheaper task.
func TestStopsAtFirstBidBelowBase(t *testing.T) {
	ctx, k, _ := setup(t)
	k.store(ctx).Set([]byte("base_price"), []byte("4"))
	queuedAuctionTask(ctx, k, 1, "5", 1, 10)
	low := queuedAuctionTask(ctx, k, 2, "3", 1, 10)
	got, stop := k.selectReady(ctx, 10, nil)
	require.False(t, stop)
	require.Equal(t, uint64(1), got.ID)
	got, stop = k.selectReady(ctx, 10, nil)
	require.True(t, stop)
	require.Nil(t, got)
	unchanged, err := k.load(ctx, low.Owner, low.ID)
	require.NoError(t, err)
	require.Equal(t, low, unchanged)
}

func TestBidEqualToBaseEligible(t *testing.T) {
	ctx, k, _ := setup(t)
	k.store(ctx).Set([]byte("base_price"), []byte("4"))
	queuedAuctionTask(ctx, k, 1, "4", 1, 10)
	got, stop := k.selectReady(ctx, 10, nil)
	require.False(t, stop)
	require.NotNil(t, got)
	require.Equal(t, uint64(1), got.ID)
}

func TestAllPricedOutPollsNothing(t *testing.T) {
	ctx, k, _ := setup(t)
	k.store(ctx).Set([]byte("base_price"), []byte("4"))
	var tasks []Task
	for i := uint64(1); i <= 5; i++ {
		tasks = append(tasks, queuedAuctionTask(ctx, k, i, "3", 1, 10))
	}
	ctx = k.ReserveOrdinaryGas(ctx, 400_000_000)
	ctx = k.EndBlock(ctx, 400_000_000)
	for _, task := range tasks {
		got, err := k.load(ctx, task.Owner, task.ID)
		require.NoError(t, err)
		require.Equal(t, task, got)
	}
	require.Equal(t, "3.500000000000000000", k.basePrice(ctx).String())
}

// A too-large head must neither block the smaller task nor lose next-block priority.
func TestOversizedSkippedThenFirstNextBlock(t *testing.T) {
	ctx, k, _ := setup(t)
	large := queuedAuctionTask(ctx, k, 1, "5", 9, 10)
	queuedAuctionTask(ctx, k, 2, "3", 4, 10)
	got, stop := k.selectReady(ctx, 5, nil)
	require.False(t, stop)
	require.NotNil(t, got)
	require.Equal(t, uint64(2), got.ID)
	still, err := k.load(ctx, large.Owner, large.ID)
	require.NoError(t, err)
	require.Equal(t, large, still)
	ctx = ctx.WithBlockHeight(11)
	got, stop = k.selectReady(ctx, 10, nil)
	require.False(t, stop)
	require.Equal(t, uint64(1), got.ID)
}

func TestUpdateBudgetRekeysBid(t *testing.T) {
	ctx, k, _ := setup(t)
	first := queuedAuctionTask(ctx, k, 1, "1", 80_000_000, 10)
	queuedAuctionTask(ctx, k, 2, "3", 80_000_000, 10)
	require.NoError(t, k.UpdateBudget(ctx, first.Owner, UpdateBudgetMsg{TaskID: first.ID, Actor: first.Control, GasPerPoll: first.GasPerPoll, MaxGasPrice: "5", Refund: first.Refund}))
	updated, err := k.load(ctx, first.Owner, first.ID)
	require.NoError(t, err)
	require.NotEqual(t, first.QueueKey, updated.QueueKey)
	require.Nil(t, k.store(ctx).Get(first.QueueKey))
	require.Equal(t, first.ReadyAt, updated.ReadyAt)
	require.Equal(t, "00000000000000000001", string(updated.QueueKey[len(updated.QueueKey)-20:]))
	require.Equal(t, uint64(2), number(k.store(ctx), "ready_sequence"))
	got, stop := k.selectReady(ctx, 100_000_000, nil)
	require.False(t, stop)
	require.Equal(t, uint64(1), got.ID)
}

// Resetting the budget per selectReady call or rescanning a retained head would
// admit the fifth entry despite the four-entry global limit.
func TestSelectionSharesVisitBudgetAndCursor(t *testing.T) {
	ctx, k, _ := setup(t)
	k.Config.MaxVisits = 4
	scan := &selectionScan{}
	ctx = ctx.WithValue(selectionScanKey{}, scan)
	stale := queuedAuctionTask(ctx, k, 1, "9", 1, 10)
	k.store(ctx).Delete(taskKey(stale.Owner, stale.ID))
	large := queuedAuctionTask(ctx, k, 2, "8", 9, 10)
	skipped := queuedAuctionTask(ctx, k, 3, "7", 1, 10)
	queuedAuctionTask(ctx, k, 4, "6", 1, 10)
	last := queuedAuctionTask(ctx, k, 5, "5", 1, 10)
	got, stop := k.selectReady(ctx, 5, [][]byte{skipped.QueueKey})
	require.False(t, stop)
	require.Nil(t, got) // Stale cleanup commits as its own maintenance visit.
	got, stop = k.selectReady(ctx, 5, [][]byte{skipped.QueueKey})
	require.False(t, stop)
	require.NotNil(t, got)
	require.Equal(t, uint64(4), got.ID)
	got, stop = k.selectReady(ctx, 5, nil)
	require.True(t, stop)
	require.Nil(t, got)
	require.Equal(t, uint64(4), scan.visits)
	require.Nil(t, k.store(ctx).Get(stale.QueueKey))
	for _, task := range []Task{large, skipped, last} {
		require.Equal(t, taskKey(task.Owner, task.ID), k.store(ctx).Get(task.QueueKey))
	}
}

func TestAllPricedOutInspectsOnlyOneEntry(t *testing.T) {
	ctx, k, _ := setup(t)
	scan := &selectionScan{}
	ctx = ctx.WithValue(selectionScanKey{}, scan)
	k.store(ctx).Set([]byte("base_price"), []byte("4"))
	for i := uint64(1); i <= 10; i++ {
		queuedAuctionTask(ctx, k, i, "3", 1, 10)
	}
	for i := 0; i < 2; i++ {
		got, stop := k.selectReady(ctx, 10, nil)
		require.True(t, stop)
		require.Nil(t, got)
	}
	require.Equal(t, uint64(1), scan.visits)
}

// This exercises the EndBlock caller as well as selectReady: a stale index uses
// one of two global visits, leaving only one real VM task eligible for a poll.
func TestEndBlockGlobalVisitBudget(t *testing.T) {
	h := newFaultHarness(t)
	h.k.Config.MaxVisits = 2
	for i := 0; i < 3; i++ {
		h.start(t, 80_000_000, 100)
	}
	h.k.store(h.ctx).Set([]byte("bid/00000000000000000000000000000000/00000000000000000010/00000000000000000099"), []byte("task/missing"))
	h.block(11)
	require.Equal(t, uint64(11), h.task(t, 1).LastPoll)
	require.Zero(t, h.task(t, 2).LastPoll)
	require.Zero(t, h.task(t, 3).LastPoll)
	h.block(12)
	require.Equal(t, uint64(12), h.task(t, 2).LastPoll)
	require.Equal(t, uint64(12), h.task(t, 3).LastPoll)
}

// Pausing the affordable-to-inspect head must commit before a later large
// continuation exhausts the discretionary maintenance meter on its read.
func TestSelectionPauseSurvivesLaterVisitOutOfGas(t *testing.T) {
	ctx, k, _ := setup(t)
	k.Config.MaxVisits = 2
	k.Config.MaintenanceGas = PriceFinalizationGas + 100_000
	first := queuedAuctionTask(ctx, k, 1, "5", 80_000_000, 10)
	first.Escrow = "0"
	k.save(ctx, first)
	later := queuedAuctionTask(ctx, k, 2, "4", 80_000_000, 10)
	later.State = make([]byte, 150_000)
	later.AllocatedBytes = 300_000
	require.NoError(t, k.checkQuota(later))
	k.save(ctx, later)
	ctx = k.ReserveOrdinaryGas(ctx, 400_000_000)
	require.NotPanics(t, func() { ctx = k.EndBlock(ctx, 400_000_000) })
	paused, err := k.load(ctx, first.Owner, first.ID)
	require.NoError(t, err)
	require.Equal(t, "paused", paused.Status)
	require.Equal(t, "insufficient_funds", paused.Pause)
	require.Nil(t, paused.QueueKey)
	untouched, err := k.load(ctx, later.Owner, later.ID)
	require.NoError(t, err)
	require.Equal(t, later, untouched)
	require.Zero(t, number(k.store(ctx), "last_poll_gas"))
}

// A scheduler-phase authenticated delivery must enter due(h+1) and cannot cause
// a second poll in this block, including after reconstruction. The context flag
// injects scheduler timing; the next block resumes through the real VM.
func TestSchedulerDeliveryDeferredAtMostOnce(t *testing.T) {
	h := newFaultHarness(t)
	h.start(t, 80_000_000, 100)
	h.block(11)
	ctx := h.ctx.WithValue(schedulerKey{}, true)
	var resolution Resolution
	require.NoError(t, json.Unmarshal([]byte(`{"success":{"data":"b2s="}}`), &resolution))
	require.NoError(t, h.k.Deliver(ctx, h.owner.String(), 1, 1, h.creator.String(), resolution))
	deferred := h.task(t, 1)
	require.Equal(t, uint64(12), deferred.ReadyAt)
	require.True(t, bytes.HasPrefix(deferred.QueueKey, []byte("due/00000000000000000012/")))
	require.False(t, h.k.promoteDue(ctx))
	got, stop := h.k.selectReady(ctx, 160_000_000, nil)
	require.True(t, stop)
	require.Nil(t, got)
	h.k = &Keeper{Key: h.k.Key, Config: h.k.Config, Bank: h.k.Bank, Wasm: h.k.Wasm, messenger: h.k.messenger}
	h.block(12)
	require.Equal(t, uint64(12), h.task(t, 1).LastPoll)
	require.Equal(t, "completed", h.task(t, 1).Status)
}

// An equal-height delivery can enter bids while an older due entry is waiting
// for its capped promotion; neither promotion nor budget changes may reorder it.
func TestDelayedPromotionPreservesHeightAndSequence(t *testing.T) {
	ctx, k, _ := setup(t)
	older := queuedAuctionTask(ctx, k, 1, "1", 80_000_000, 11)
	require.Equal(t, []byte("due/00000000000000000011/00000000000000000001"), older.QueueKey)
	require.NoError(t, k.UpdateBudget(ctx, older.Owner, UpdateBudgetMsg{TaskID: older.ID, Actor: older.Control, GasPerPoll: older.GasPerPoll, MaxGasPrice: "2", Refund: older.Refund}))
	unchanged, err := k.load(ctx, older.Owner, older.ID)
	require.NoError(t, err)
	require.Equal(t, older.QueueKey, unchanged.QueueKey)
	ctx = ctx.WithBlockHeight(11)
	newer := Task{ID: 2, Owner: older.Owner, Status: "waiting", Started: true, Sequence: 1, Escrow: older.Escrow, Fees: "0", Register: older.Register,
		Wait: &Wait{Operation: &struct {
			Resolver       string `json:"resolver"`
			Deadline       uint64 `json:"deadline"`
			MaxResultBytes uint32 `json:"max_result_bytes"`
		}{older.Owner, 90, 32}}}
	newer.MaxGasPrice = "2"
	k.save(ctx, newer)
	require.NoError(t, k.Deliver(ctx, newer.Owner, 2, 1, newer.Owner, Resolution{Success: &struct {
		Data []byte `json:"data"`
	}{[]byte("ok")}}))
	ctx = ctx.WithBlockHeight(14)
	require.True(t, k.promoteDue(ctx))
	require.False(t, k.promoteDue(ctx))
	promoted, err := k.load(ctx, older.Owner, 1)
	require.NoError(t, err)
	require.Equal(t, uint64(11), promoted.ReadyAt)
	require.Equal(t, "00000000000000000001", string(promoted.QueueKey[len(promoted.QueueKey)-20:]))
	require.Equal(t, uint64(2), number(k.store(ctx), "ready_sequence"))
	k = &Keeper{Key: k.Key, Config: k.Config, Bank: k.Bank, Wasm: k.Wasm}
	got, stop := k.selectReady(ctx, 100_000_000, nil)
	require.False(t, stop)
	require.Equal(t, uint64(1), got.ID)
}

func TestValidateStateRejectsContractQueueKeys(t *testing.T) {
	h := newFaultHarness(t)
	require.NoError(t, h.k.InitState(h.ctx))
	for _, key := range []string{"contracts/00000000000000000001/owner", "contract-queue/owner", "contract_sequence", "ready/owner/00000000000000000010/00000000000000000001/"} {
		t.Run(key, func(t *testing.T) {
			ctx, _ := h.ctx.CacheContext()
			if key == "contract_sequence" {
				setNumber(h.k.store(ctx), key, 1)
			} else {
				h.k.store(ctx).Set([]byte(key), []byte("old"))
			}
			require.Error(t, h.k.ValidateState(ctx))
		})
	}
}
