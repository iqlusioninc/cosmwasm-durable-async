package async

import (
	"bytes"
	"encoding/json"
	"fmt"
	"testing"

	storetypes "cosmossdk.io/store/types"
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

func auctionBlock(ctx sdk.Context, k *Keeper, height int64) sdk.Context {
	ctx = ctx.WithBlockHeight(height).WithGasMeter(storetypes.NewInfiniteGasMeter())
	return k.EndBlock(k.ReserveOrdinaryGas(ctx, 400_000_000), 400_000_000)
}

// Pausing before the third full block of readiness, resetting age at promotion,
// or charging a task without polling breaks this boundary test.
func TestPricedOutPausesOnlyAfterGrace(t *testing.T) {
	ctx, k, _ := setup(t)
	k.Config.PriceGraceBlocks = 3
	k.store(ctx).Set([]byte("base_price"), []byte("4"))
	task := queuedAuctionTask(ctx, k, 1, "1", 80_000_000, 11)
	for height := int64(11); height <= 14; height++ {
		ctx = auctionBlock(ctx, k, height)
		got, err := k.load(ctx, task.Owner, task.ID)
		require.NoError(t, err)
		require.Equal(t, task.Escrow, got.Escrow)
		require.Equal(t, task.Fees, got.Fees)
		require.Zero(t, got.LastPoll)
		require.Equal(t, uint64(11), got.ReadyAt)
		require.Equal(t, task.ReadySequence, got.ReadySequence)
		if height < 14 {
			require.Equal(t, "runnable", got.Status)
			require.True(t, bytes.HasPrefix(got.QueueKey, []byte("bid/")))
		} else {
			require.Equal(t, "paused", got.Status)
			require.Equal(t, "price_limit", got.Pause)
			require.Nil(t, got.QueueKey)
			require.Nil(t, k.store(ctx).Get(bidKey("1", 11, task.ReadySequence)))
		}
	}
}

// Exhausted poll capacity must retain an old eligible bid without price pause.
func TestFullBlockWaitNeverPricePauses(t *testing.T) {
	ctx, k, _ := setup(t)
	k.Config.PriceGraceBlocks = 0
	k.store(ctx).Set([]byte("base_price"), []byte("2"))
	task := queuedAuctionTask(ctx, k, 1, "2", 80_000_000, 10)
	ctx = ctx.WithBlockHeight(30)
	got, stop := k.selectReady(ctx, 0, nil)
	require.True(t, stop)
	require.Nil(t, got)
	// Consume selection's only visit with a stale higher bid, keeping this
	// already-capacity-blocked task queued while EndBlock runs maintenance.
	k.Config.MaxVisits = 1
	k.store(ctx).Set(bidKey("3", 10, 99), []byte("task/missing"))
	ctx = auctionBlock(ctx, k, 30)
	unchanged, err := k.load(ctx, task.Owner, task.ID)
	require.NoError(t, err)
	require.Equal(t, task, unchanged)
}

// This follows the reachable delivery -> runnable -> price pause path. Losing
// an accepted inbox, auto-resuming on decay, or failing explicit resume breaks it.
func TestPricePauseSurvivesPriceDropUntilResume(t *testing.T) {
	h := newFaultHarness(t)
	h.k.Config.PriceGraceBlocks = 3
	require.NoError(t, h.k.InitState(h.ctx))
	h.start(t, 80_000_000, 100)
	h.block(11)
	h.ctx = h.ctx.WithBlockHeight(12)
	h.execute(t, map[string]any{"deliver": map[string]any{"task_id": 1, "wait_sequence": 1}}, 0)
	ready := h.task(t, 1)
	require.NoError(t, h.k.UpdateBudget(h.ctx, ready.Owner, UpdateBudgetMsg{TaskID: 1, Actor: ready.Control, GasPerPoll: ready.GasPerPoll, MaxGasPrice: "0.001", Refund: ready.Refund}))
	h.k.store(h.ctx).Set([]byte("base_price"), []byte("0.004"))
	for height := int64(12); height <= 15; height++ {
		h.block(height)
	}
	paused := h.task(t, 1)
	require.Equal(t, "price_limit", paused.Pause)
	require.Equal(t, "paused", paused.Status)
	require.Equal(t, ready.Wait, paused.Wait)
	require.Equal(t, ready.Inbox, paused.Inbox)
	require.Equal(t, ready.State, paused.State)
	require.Equal(t, ready.Escrow, paused.Escrow)
	require.Equal(t, ready.Fees, paused.Fees)
	require.NoError(t, h.k.ValidateState(h.ctx))
	h.k.store(h.ctx).Set([]byte("base_price"), []byte("0.001"))
	h.block(16)
	require.Equal(t, paused, h.task(t, 1))
	require.NoError(t, h.k.Management(h.ctx, paused.Owner, marshal(map[string]any{"resume_task": map[string]any{"task_id": 1, "actor": paused.Control}})))
	require.Equal(t, uint64(17), h.task(t, 1).ReadyAt)
	h.block(17)
	require.Equal(t, "completed", h.task(t, 1).Status)
	require.Equal(t, uint64(17), h.task(t, 1).LastPoll)
	require.NoError(t, h.k.ValidateState(h.ctx))
}

// A real poll and same-height delivery establish LastPoll=ReadyAt=11. Lowering
// the bid at h=12 must not restart grace or allocate another FIFO sequence.
func TestLoweredBidGraceCountsFromLastPoll(t *testing.T) {
	h := newFaultHarness(t)
	h.k.Config.PriceGraceBlocks = 3
	h.start(t, 80_000_000, 100)
	h.block(11)
	require.Equal(t, uint64(11), h.task(t, 1).LastPoll)
	h.execute(t, map[string]any{"deliver": map[string]any{"task_id": 1, "wait_sequence": 1}}, 0)
	ready := h.task(t, 1)
	require.Equal(t, uint64(11), ready.ReadyAt)
	h.ctx = h.ctx.WithBlockHeight(12)
	h.k.store(h.ctx).Set([]byte("base_price"), []byte("0.004"))
	require.NoError(t, h.k.UpdateBudget(h.ctx, ready.Owner, UpdateBudgetMsg{TaskID: 1, Actor: ready.Control, GasPerPoll: ready.GasPerPoll, MaxGasPrice: "0.001", Refund: ready.Refund}))
	updated := h.task(t, 1)
	require.Equal(t, ready.ReadyAt, updated.ReadyAt)
	require.Equal(t, ready.ReadySequence, updated.ReadySequence)
	require.Equal(t, ready.LastPoll, updated.LastPoll)
	for height := int64(12); height <= 14; height++ {
		h.block(height)
		got := h.task(t, 1)
		if height < 14 {
			require.Equal(t, "runnable", got.Status)
		} else {
			require.Equal(t, "price_limit", got.Pause)
		}
		require.Equal(t, updated.Escrow, got.Escrow)
		require.Equal(t, updated.Fees, got.Fees)
	}
}

// Every inspected young entry consumes the cap. A fresh bounded walk on the
// next block can reach the old task once the cap covers those skips.
func TestPriceMaintenanceBoundsYoungSkips(t *testing.T) {
	ctx, k, _ := setup(t)
	k.Config.PriceGraceBlocks = 3
	k.Config.MaxMaintenance = 2
	k.store(ctx).Set([]byte("base_price"), []byte("4"))
	old := queuedAuctionTask(ctx, k, 1, "2", 80_000_000, 10)
	ctx = ctx.WithBlockHeight(20)
	for id := uint64(2); id <= 3; id++ {
		queuedAuctionTask(ctx, k, id, "1", 80_000_000, 20)
	}
	ctx = auctionBlock(ctx, k, 20)
	unchanged, err := k.load(ctx, old.Owner, old.ID)
	require.NoError(t, err)
	require.Equal(t, old, unchanged)
	k.Config.MaxMaintenance = 3
	ctx = auctionBlock(ctx, k, 21)
	paused, err := k.load(ctx, old.Owner, old.ID)
	require.NoError(t, err)
	require.Equal(t, "price_limit", paused.Pause)
	for id := uint64(2); id <= 3; id++ {
		got, err := k.load(ctx, sdk.AccAddress(bytes.Repeat([]byte{byte(id)}, 20)).String(), id)
		require.NoError(t, err)
		require.Equal(t, "runnable", got.Status)
	}
}

// A later costly entry must not roll back an earlier pause or prevent mandatory
// price finalization; the original entry remains intact after failed inspection.
func TestPricePauseSurvivesLaterVisitOutOfGas(t *testing.T) {
	ctx, k, _ := setup(t)
	k.Config.PriceGraceBlocks = 0
	k.Config.MaxMaintenance = 2
	k.Config.MaintenanceGas = PriceFinalizationGas + 100_000
	k.store(ctx).Set([]byte("base_price"), []byte("4"))
	first := queuedAuctionTask(ctx, k, 1, "1", 80_000_000, 10)
	later := queuedAuctionTask(ctx, k, 2, "2", 80_000_000, 10)
	later.State = make([]byte, 150_000)
	later.AllocatedBytes = 300_000
	require.NoError(t, k.checkQuota(later))
	k.save(ctx, later)
	require.NotPanics(t, func() { ctx = auctionBlock(ctx, k, 10) })
	paused, err := k.load(ctx, first.Owner, first.ID)
	require.NoError(t, err)
	require.Equal(t, "price_limit", paused.Pause)
	require.Nil(t, paused.QueueKey)
	require.Equal(t, first.Escrow, paused.Escrow)
	untouched, err := k.load(ctx, later.Owner, later.ID)
	require.NoError(t, err)
	require.Equal(t, later, untouched)
	require.Equal(t, "3.500000000000000000", k.basePrice(ctx).String())
	require.LessOrEqual(t, ctx.BlockGasMeter().GasConsumed(), k.Config.MaintenanceGas)
	k.Config.MaintenanceGas = 40_000_000
	ctx = auctionBlock(ctx, k, 11)
	pausedLater, err := k.load(ctx, later.Owner, later.ID)
	require.NoError(t, err)
	require.Equal(t, "price_limit", pausedLater.Pause)
	require.Equal(t, later.State, pausedLater.State)
	require.Equal(t, later.Escrow, pausedLater.Escrow)
}

// Timer resolution and due promotion precede price inspection; an overdue
// timer retains its original age and its newly accepted timer inbox on pause.
func TestOverdueTimerPricePausesAfterPromotion(t *testing.T) {
	ctx, k, _ := setup(t)
	k.Config.PriceGraceBlocks = 3
	queuedAuctionTask(ctx, k, 1, "1", 80_000_000, 10)
	task, stop := k.selectReady(ctx, 100_000_000, nil)
	require.False(t, stop)
	stageAuctionTimer(t, ctx, k, task, 11)
	readySequence := task.ReadySequence
	k.store(ctx).Set([]byte("base_price"), []byte("4"))
	ctx = auctionBlock(ctx, k, 14)
	paused, err := k.load(ctx, task.Owner, task.ID)
	require.NoError(t, err)
	require.Equal(t, "price_limit", paused.Pause)
	require.Equal(t, uint64(11), paused.ReadyAt)
	require.Equal(t, readySequence, paused.ReadySequence)
	require.NotNil(t, paused.Wait)
	require.NotNil(t, paused.Inbox.Timer)
	require.Equal(t, []byte{1}, paused.State)
	require.Nil(t, paused.TimerKey)
	require.Nil(t, paused.QueueKey)
}

// A resolved timer or timeout remains public while priced out and paused, then
// explicit resume must keep it runnable and pass it to the next real Wasm poll.
func TestMaturedWaitPricePauseResumeParity(t *testing.T) {
	for _, kind := range []string{"height", "operation"} {
		t.Run(kind, func(t *testing.T) {
			h := newFaultHarness(t)
			h.k.Config.PriceGraceBlocks = 3
			require.NoError(t, h.k.InitState(h.ctx))
			h.start(t, 80_000_000, 100)
			h.block(11)
			waiting := h.task(t, 1)
			wait := map[string]any{"height": map[string]any{"height": 12}}
			wantResolution := &Resolution{Timer: &struct{}{}}
			if kind == "operation" {
				wait = map[string]any{"operation": map[string]any{"resolver": waiting.Control, "deadline": 12, "max_result_bytes": 256}}
				wantResolution = &Resolution{Timeout: &struct{}{}}
			}
			var response PollResponse
			require.NoError(t, json.Unmarshal(marshal(map[string]any{"outcome": map[string]any{"pending": map[string]any{"state": waiting.State, "wait": wait}}, "messages": []any{}}), &response))
			require.NoError(t, h.k.stage(h.ctx, &waiting, response))
			originalSequence := waiting.ReadySequence
			require.NoError(t, h.k.UpdateBudget(h.ctx, waiting.Owner, UpdateBudgetMsg{TaskID: 1, Actor: waiting.Control, GasPerPoll: waiting.GasPerPoll, MaxGasPrice: "0.001", Refund: waiting.Refund}))
			query := func() TaskResponse {
				raw, err := h.k.Query(h.ctx, marshal(map[string]any{"task": map[string]any{"contract": waiting.Owner, "task_id": 1}}))
				require.NoError(t, err)
				var public TaskResponse
				require.NoError(t, json.Unmarshal(raw, &public))
				return public
			}
			require.Equal(t, TaskStatus{Kind: "waiting"}, query().Status)
			// Process the overdue wait one block late; grace still starts at 12.
			for height := int64(13); height <= 15; height++ {
				h.k.store(h.ctx).Set([]byte("base_price"), []byte("0.004"))
				h.block(height)
				got, public := h.task(t, 1), query()
				require.Equal(t, uint64(12), got.ReadyAt)
				require.Equal(t, originalSequence, got.ReadySequence)
				require.Equal(t, wantResolution, public.Inbox)
				require.Equal(t, waiting.Wait, public.Wait)
				require.Equal(t, waiting.State, public.State)
				require.Equal(t, uint64(11), *public.LastAttempt)
				wantStatus := TaskStatus{Kind: "runnable"}
				if height == 15 {
					wantStatus = TaskStatus{Kind: "paused", Reason: "price_limit", Resume: "runnable"}
				}
				require.Equal(t, wantStatus, public.Status)
			}
			h.k.store(h.ctx).Set([]byte("base_price"), []byte("0.001"))
			require.NoError(t, h.k.Management(h.ctx, waiting.Owner, marshal(map[string]any{"resume_task": map[string]any{"task_id": 1, "actor": waiting.Control}})))
			require.Equal(t, TaskStatus{Kind: "runnable"}, query().Status)
			require.Equal(t, wantResolution, query().Inbox)
			h.block(16)
			require.Equal(t, TaskStatus{Kind: "completed"}, query().Status)
			require.Equal(t, uint64(16), *query().LastAttempt)
			require.NoError(t, h.k.ValidateState(h.ctx))
		})
	}
}

func TestPricePausedTaskStillExpires(t *testing.T) {
	ctx, k, keepers := setup(t)
	k.Config.PriceGraceBlocks = 0
	owner := registrationOwner(t, ctx, k)
	keepers.Faucet.Fund(ctx, owner, sdk.NewInt64Coin("ugas", 1_000_000))
	r := registrationPolicy(owner)
	r.MaxGasPrice, r.ExpiresAt = "0.001", 14
	id, err := k.RegisterTask(ctx, owner, r)
	require.NoError(t, err)
	k.store(ctx).Set([]byte("base_price"), []byte("0.004"))
	ctx = auctionBlock(ctx, k, 11)
	paused, err := k.load(ctx, owner.String(), id)
	require.NoError(t, err)
	require.Equal(t, "price_limit", paused.Pause)
	ctx = auctionBlock(ctx, k, 14)
	_, err = k.load(ctx, owner.String(), id)
	require.Error(t, err, "expired record pruned at lease end")
	require.Zero(t, number(k.store(ctx), "active"))
	require.Equal(t, "999900", keepers.BankKeeper.GetBalance(ctx, owner, "ugas").Amount.String())
}

// Expiry's per-kind cap can leave another expired bid for a later block. Price
// maintenance must not relabel that task as price-paused in the meantime.
func TestExpiredPricedOutBacklogDoesNotPricePause(t *testing.T) {
	ctx, k, keepers := setup(t)
	k.Config.PriceGraceBlocks = 0
	k.Config.MaxMaintenance = 1
	owner := registrationOwner(t, ctx, k)
	keepers.Faucet.Fund(ctx, owner, sdk.NewInt64Coin("ugas", 3_000_000))
	r := registrationPolicy(owner)
	r.MaxGasPrice, r.ExpiresAt = "0.001", 14
	for i := 0; i < 2; i++ {
		_, err := k.RegisterTask(ctx, owner, r)
		require.NoError(t, err)
	}
	ctx = ctx.WithBlockHeight(11)
	require.True(t, k.promoteDue(ctx))
	require.True(t, k.promoteDue(ctx))
	k.store(ctx).Set([]byte("base_price"), []byte("0.004"))
	ctx = auctionBlock(ctx, k, 14)
	_, err := k.load(ctx, owner.String(), 1)
	require.Error(t, err, "the expired record is pruned at its prepaid endpoint")
	require.Equal(t, uint64(1), number(k.store(ctx), "active"))
	second, err := k.load(ctx, owner.String(), 2)
	require.NoError(t, err)
	require.Equal(t, "runnable", second.Status)
	require.Empty(t, second.Pause)
	ctx = auctionBlock(ctx, k, 15)
	_, err = k.load(ctx, owner.String(), 2)
	require.Error(t, err)
	require.Zero(t, number(k.store(ctx), "active"))
	require.Equal(t, "2999800", keepers.BankKeeper.GetBalance(ctx, owner, "ugas").Amount.String())
}
