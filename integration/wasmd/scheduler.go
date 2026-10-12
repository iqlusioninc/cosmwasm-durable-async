package async

import (
	"bytes"
	sdkmath "cosmossdk.io/math"
	storetypes "cosmossdk.io/store/types"
	"encoding/json"
	"fmt"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
)

type schedulerKey struct{}
type pollingKey struct{}
type selectionScanKey struct{}

// One cursor and inspection budget span every selection call in EndBlock.
// Retained oversized/skipped bids must not be read repeatedly in this block.
type selectionScan struct {
	cursor  []byte
	visits  uint64
	stopped bool
}

// ReserveOrdinaryGas is a keeper-harness helper. Production applications MUST
// install ProposalHandlers; BaseApp resets its block meter after BeginBlock.
func (k *Keeper) ReserveOrdinaryGas(ctx sdk.Context, total uint64) sdk.Context {
	budget, err := k.Config.OrdinaryGas(total)
	if err != nil {
		panic(err)
	}
	return ctx.WithBlockGasMeter(storetypes.NewGasMeter(budget))
}

// EndBlock must be called exactly once after ordinary txs and before the app's
// remaining EndBlock modules. total is the consensus maximum block gas.
func (k *Keeper) EndBlock(ctx sdk.Context, total uint64) sdk.Context {
	originalMeter := ctx.GasMeter()
	budget, err := k.Config.OrdinaryGas(total)
	if err != nil {
		panic(err)
	}
	ordinary := ctx.BlockGasMeter().GasConsumedToLimit()
	if ordinary > budget {
		panic("ordinary transactions exceeded reserved partition")
	}
	block := ctx.BlockGasMeter()
	if block.Limit() != total {
		block = storetypes.NewGasMeter(total)
		block.ConsumeGas(ordinary, "ordinary transaction partition")
	}
	finalization := storetypes.NewGasMeter(PriceFinalizationGas)
	price := k.basePrice(ctx.WithGasMeter(finalization))
	used := uint64(0)
	meter := storetypes.NewGasMeter(k.Config.MaintenanceGas - PriceFinalizationGas)
	ctx = ctx.WithBlockGasMeter(block).WithGasMeter(meter).WithValue(schedulerKey{}, true).
		WithValue(blockPriceKey{}, price).WithValue(pollGasKey{}, &used).
		WithValue(selectionScanKey{}, &selectionScan{})
	// A host timer is eligible at its requested height, unlike a callback from
	// a poll (h+1). Resolve timers before promoting the due horizon.
	for _, kind := range []string{"expiry", "prune", "timer", "due"} {
		for n := uint64(0); n < k.Config.MaxMaintenance; n++ {
			more := false
			if !cachedMaintenance(ctx, func(c sdk.Context) {
				if kind == "due" {
					more = k.promoteDue(c)
				} else {
					more = k.maintenanceOne(c, kind)
				}
			}) || !more {
				break
			}
		}
	}
	k.pricePauses(ctx)
	remaining := k.Config.AsyncGas - k.Config.MaintenanceGas
	for {
		var selected *Task
		stop := false
		if !cachedMaintenance(ctx, func(c sdk.Context) { selected, stop = k.selectReady(c, remaining, nil) }) || stop {
			break
		}
		if selected == nil {
			continue
		}
		consumed := k.poll(ctx, *selected)
		remaining -= consumed
		block.ConsumeGas(consumed, "native async attempt")
	}
	block.ConsumeGas(meter.GasConsumedToLimit(), "native async maintenance")
	// Mandatory finalization has a bounded, separately reserved meter, even if a
	// discretionary visit exhausted its allowance. No state write is unmetered.
	final := ctx.WithGasMeter(finalization)
	target := (k.Config.AsyncGas - k.Config.MaintenanceGas) / 2
	next := nextBasePrice(price, used, target, k.Config.MinBasePrice, k.Config.PriceAdjustDenominator)
	k.store(final).Set([]byte("base_price"), []byte(next.String()))
	setNumber(k.store(final), "last_poll_gas", used)
	final.EventManager().EmitEvent(sdk.NewEvent("async_base_price", sdk.NewAttribute("old", price.String()), sdk.NewAttribute("new", next.String()), sdk.NewAttribute("used", fmt.Sprint(used)), sdk.NewAttribute("target", fmt.Sprint(target))))
	block.ConsumeGas(finalization.GasConsumedToLimit(), "native async price finalization")
	return ctx.WithValue(schedulerKey{}, false).WithValue(blockPriceKey{}, sdkmath.LegacyDec{}).WithValue(pollGasKey{}, (*uint64)(nil)).WithValue(selectionScanKey{}, (*selectionScan)(nil)).WithGasMeter(originalMeter)
}

// Every bounded maintenance visit is independently atomic. Exhaustion discards
// only this visit; earlier successful polls and their fees remain committed.
func cachedMaintenance(ctx sdk.Context, f func(sdk.Context)) (ok bool) {
	if ctx.GasMeter().IsOutOfGas() {
		return false
	}
	child, commit := ctx.CacheContext()
	defer func() {
		if p := recover(); p != nil {
			if _, gas := p.(storetypes.ErrorOutOfGas); gas {
				ok = false
			} else {
				panic(p)
			}
		}
	}()
	f(child)
	commit()
	return true
}

// Price is one additional maintenance kind: like expiry/timer/due it has its
// own MaxMaintenance inspection cap, and shares their discretionary gas meter.
// A one-key reverse cursor bounds reads as well as writes, so a later costly
// entry cannot roll back earlier pauses. Young entries also consume the cap.
func (k *Keeper) pricePauses(ctx sdk.Context) {
	prefix := []byte("bid/")
	end := storetypes.PrefixEndBytes(prefix)
	boundary := []byte("bid/" + bidComponent(k.basePrice(ctx)))
	height := uint64(ctx.BlockHeight())
	for n := uint64(0); n < k.Config.MaxMaintenance; n++ {
		stop := false
		if !cachedMaintenance(ctx, func(c sdk.Context) {
			s := k.store(c)
			key, value := func() ([]byte, []byte) {
				it := s.ReverseIterator(prefix, end)
				defer it.Close() // Close even when a metered read panics.
				if !it.Valid() {
					return nil, nil
				}
				return bytes.Clone(it.Key()), bytes.Clone(it.Value())
			}() // Iterator is closed before any mutation.
			if key == nil {
				stop = true
				return
			}
			end = key // ReverseIterator's upper bound is exclusive.
			// Inverted bids sort lowest last; eligibility ends the reverse walk.
			if len(key) < len(boundary) || bytes.Compare(key[:len(boundary)], boundary) <= 0 {
				stop = true
				return
			}
			b := s.Get(value)
			if b == nil {
				s.Delete(key)
				return
			}
			var t Task
			if err := json.Unmarshal(b, &t); err != nil {
				panic(err)
			}
			if !bytes.Equal(t.QueueKey, key) || !bytes.Equal(value, taskKey(t.Owner, t.ID)) {
				s.Delete(key)
				return
			}
			// Expiry may have a backlog; never replace its outcome with a pause.
			if terminal(t) || t.Status != "runnable" || height >= t.ExpiresAt {
				return
			}
			since := max(t.ReadyAt, t.LastPoll)
			if height < since || height-since < k.Config.PriceGraceBlocks {
				return
			}
			s.Delete(key)
			t.QueueKey = nil
			t.Status, t.Pause = "paused", "price_limit"
			k.save(c, t)
		}) || stop {
			break
		}
	}
}

func (k *Keeper) promoteDue(ctx sdk.Context) bool {
	key, value := k.first(ctx, "due", uint64(ctx.BlockHeight()))
	if key == nil {
		return false
	}
	s := k.store(ctx)
	s.Delete(key)
	b := s.Get(value)
	if b == nil {
		return true
	}
	var t Task
	if e := json.Unmarshal(b, &t); e != nil {
		panic(e)
	}
	// An old index entry cannot replace a newer readiness generation.
	if !bytes.Equal(t.QueueKey, key) || !bytes.Equal(value, taskKey(t.Owner, t.ID)) {
		return true
	}
	if t.ReadySequence == 0 || !bytes.Equal(key, dueKey(t.ReadyAt, t.ReadySequence)) {
		panic("corrupt native async due key")
	}
	t.QueueKey = nil
	if !terminal(t) && t.Status == "runnable" {
		// Keep original due height AND sequence, even after delayed promotion.
		t.QueueKey = bidKey(t.MaxGasPrice, t.ReadyAt, t.ReadySequence)
		s.Set(t.QueueKey, value)
	}
	k.save(ctx, t)
	return true
}

func (k *Keeper) selectReady(ctx sdk.Context, remaining uint64, skipped [][]byte) (*Task, bool) {
	scan, _ := ctx.Value(selectionScanKey{}).(*selectionScan)
	if scan == nil {
		// Direct keeper callers get one bounded walk; EndBlock always supplies
		// the shared state above, including across independently cached visits.
		scan = &selectionScan{}
	}
	s := k.store(ctx)
	prefix := []byte("bid/")
	boundary := []byte("bid/" + bidComponent(k.basePrice(ctx)))
	for !scan.stopped && scan.visits < k.Config.MaxVisits {
		start := scan.cursor
		if start == nil {
			start = prefix
		}
		it := s.Iterator(start, storetypes.PrefixEndBytes(prefix))
		if !it.Valid() {
			it.Close()
			break
		}
		key, value := bytes.Clone(it.Key()), bytes.Clone(it.Value())
		it.Close() // Never mutate with an open iterator.
		scan.visits++
		scan.cursor = append(bytes.Clone(key), 0) // Exclusive lower bound.
		// Inverted bids sort highest first; one below-base read ends the walk.
		if len(key) < len(boundary) || bytes.Compare(key[:len(boundary)], boundary) > 0 {
			break
		}
		isSkipped := false
		for _, skip := range skipped {
			if bytes.Equal(key, skip) {
				isSkipped = true
				break
			}
		}
		if isSkipped {
			continue
		}
		b := s.Get(value)
		if b == nil {
			s.Delete(key)
			return nil, false
		}
		var t Task
		if e := json.Unmarshal(b, &t); e != nil {
			panic(e)
		}
		if !bytes.Equal(t.QueueKey, key) || !bytes.Equal(value, taskKey(t.Owner, t.ID)) {
			s.Delete(key)
			return nil, false
		}
		if t.GasPerPoll > remaining && t.Status == "runnable" && t.LastPoll != uint64(ctx.BlockHeight()) && uint64(ctx.BlockHeight()) < t.ExpiresAt {
			continue
		}
		s.Delete(key)
		t.QueueKey = nil
		if terminal(t) || t.Status != "runnable" {
			k.save(ctx, t)
			return nil, false
		}
		if uint64(ctx.BlockHeight()) >= t.ExpiresAt {
			k.settle(ctx, &t, "expired")
			return nil, false
		}
		if t.LastPoll == uint64(ctx.BlockHeight()) {
			k.enqueue(ctx, &t, uint64(ctx.BlockHeight())+1)
			k.save(ctx, t)
			return nil, false
		}
		if amount(t.Escrow).LT(k.fee(ctx, t.GasPerPoll)) {
			t.Status = "paused"
			t.Pause = "insufficient_funds"
			k.save(ctx, t)
			return nil, false
		}
		return &t, false
	}
	scan.stopped = true
	return nil, true
}

func (k *Keeper) first(ctx sdk.Context, kind string, height uint64) ([]byte, []byte) {
	p := []byte(kind + "/")
	it := k.store(ctx).Iterator(p, storetypes.PrefixEndBytes(p))
	defer it.Close()
	if !it.Valid() {
		return nil, nil
	}
	// Keys sort by zero-padded height, then monotonic ID/readiness sequence.
	bound := []byte(fmt.Sprintf("%s/%020d/", kind, height))
	if len(it.Key()) < len(bound) {
		panic("corrupt native async index key")
	}
	if bytes.Compare(it.Key()[:len(bound)], bound) > 0 {
		return nil, nil
	}
	return bytes.Clone(it.Key()), bytes.Clone(it.Value())
}
func (k *Keeper) maintenanceOne(ctx sdk.Context, kind string) bool {
	key, value := k.first(ctx, kind, uint64(ctx.BlockHeight()))
	if key == nil {
		return false
	}
	s := k.store(ctx)
	s.Delete(key)
	b := s.Get(value)
	if b == nil {
		return true
	}
	var t Task
	if e := json.Unmarshal(b, &t); e != nil {
		panic(e)
	}
	switch kind {
	case "prune":
		if terminal(t) {
			s.Delete(value)
			setNumber(s, "bytes", number(s, "bytes")-t.AllocatedBytes)
		}
	case "expiry":
		if !terminal(t) {
			k.settle(ctx, &t, "expired")
		}
	case "timer":
		t.TimerKey = nil
		if terminal(t) || t.Wait == nil {
			return true
		}
		if t.Inbox == nil {
			r := Resolution{}
			if t.Wait.Height != nil {
				r.Timer = &struct{}{}
			} else {
				r.Timeout = &struct{}{}
			}
			t.Inbox = &r
			if t.Status != "paused" {
				t.Status = "runnable"
				// The wait reserved this FIFO slot when it was staged. Preserve
				// both its due height and sequence even if timer work is delayed.
				k.enqueueWithSequence(ctx, &t, t.ReadyAt, t.ReadySequence)
			}
		}
		k.save(ctx, t)
	}
	return true
}
func (k *Keeper) stage(ctx sdk.Context, t *Task, r PollResponse) error {
	n := 0
	if r.Outcome.Pending != nil {
		n++
	}
	if r.Outcome.Yield != nil {
		n++
	}
	if r.Outcome.Complete != nil {
		n++
	}
	if r.Outcome.Fail != nil {
		n++
	}
	if n != 1 {
		return fmt.Errorf("invalid outcome variants")
	}
	if uint64(len(r.Messages)) > k.Config.MaxMessages {
		return fmt.Errorf("too many messages")
	}
	k.clearWait(ctx, t)
	t.Started = true
	t.Program.Input = nil
	switch {
	case r.Outcome.Pending != nil:
		p := r.Outcome.Pending
		t.State = p.State
		t.Wait = &p.Wait
		t.Status = "waiting"
		if (p.Wait.Height == nil) == (p.Wait.Operation == nil) {
			return fmt.Errorf("invalid wait")
		}
		if t.Sequence == ^uint64(0) {
			return fmt.Errorf("sequence exhausted")
		}
		t.Sequence++
		due := uint64(0)
		reserve := uint64(0)
		if p.Wait.Height != nil {
			due = p.Wait.Height.Height
		} else {
			w := p.Wait.Operation
			due = w.Deadline
			reserve = uint64(w.MaxResultBytes)
			if _, e := sdk.AccAddressFromBech32(w.Resolver); e != nil {
				return e
			}
		}
		if due <= uint64(ctx.BlockHeight()) || due >= t.ExpiresAt {
			return fmt.Errorf("wait outside lease")
		}
		if uint64(len(t.State))+reserve+1024 > t.AllocatedBytes {
			return fmt.Errorf("storage quota exceeded")
		}
		t.ReadyAt = due
		t.ReadySequence = k.nextReadySequence(ctx)
		t.TimerKey = indexKey("timer", due, t.ReadySequence, t.Owner)
		k.store(ctx).Set(t.TimerKey, taskKey(t.Owner, t.ID))
	case r.Outcome.Yield != nil:
		t.State = r.Outcome.Yield.State
		t.Status = "runnable"
		t.Inbox = &Resolution{Timer: &struct{}{}}
		if uint64(len(t.State))+1024 > t.AllocatedBytes {
			return fmt.Errorf("storage quota exceeded")
		}
		k.enqueue(ctx, t, uint64(ctx.BlockHeight())+1)
	case r.Outcome.Complete != nil:
		if uint64(len(r.Outcome.Complete.Output))+1024 > t.AllocatedBytes {
			return fmt.Errorf("output quota exceeded")
		}
		t.State = r.Outcome.Complete.Output
		t.Status = "completing"
	case r.Outcome.Fail != nil:
		if uint64(len(r.Outcome.Fail.Error))+1024 > t.AllocatedBytes {
			return fmt.Errorf("error quota exceeded")
		}
		t.State = []byte(r.Outcome.Fail.Error)
		t.Status = "failing"
	}
	if err := k.checkQuota(*t); err != nil {
		return err
	}
	k.save(ctx, *t)
	return nil
}

type pollFailure struct {
	Reason string
	Cause  error
}

func (e pollFailure) Error() string { return e.Cause.Error() }
func (k *Keeper) attempt(ctx sdk.Context, t *Task) (err error) {
	defer func() {
		if p := recover(); p != nil {
			switch p.(type) {
			case storetypes.ErrorOutOfGas:
				err = pollFailure{"out_of_gas", fmt.Errorf("poll exhausted its execution slice")}
			default:
				panic(p)
			}
		}
	}()
	reserve := k.fee(ctx, t.GasPerPoll)
	t.Reserved = reserve.String()
	t.Escrow = amount(t.Escrow).Sub(reserve).String()
	k.save(ctx, *t)
	if err = k.checkIdentity(ctx, *t); err != nil {
		return pollFailure{"invalid_response", err}
	}
	data, err := k.Wasm.Sudo(ctx, addr(t.Owner), t.Request())
	if err != nil {
		return pollFailure{"trap", err}
	}
	if uint64(len(data)) > t.AllocatedBytes {
		return pollFailure{"invalid_response", fmt.Errorf("poll response exceeds allocated bytes")}
	}
	var response PollResponse
	if err = json.Unmarshal(data, &response); err != nil {
		return pollFailure{"invalid_response", err}
	}
	if err = k.stage(ctx, t, response); err != nil {
		return pollFailure{"invalid_response", err}
	}
	for _, m := range response.Messages {
		events, _, _, e := k.messenger.DispatchMsg(ctx, addr(t.Owner), "", m)
		if e != nil {
			return pollFailure{"message_failure", e}
		}
		ctx.EventManager().EmitEvents(events)
	}
	// Immediate callback may have persisted an inbox and runnable queue.
	*t, err = k.load(ctx, t.Owner, t.ID)
	return err
}
func (k *Keeper) poll(parent sdk.Context, old Task) uint64 {
	price := k.basePrice(parent)
	parent = parent.WithValue(blockPriceKey{}, price)
	child, commit := parent.CacheContext()
	execution := storetypes.NewGasMeter(old.GasPerPoll - k.Config.HostReserve)
	child = child.WithGasMeter(execution).WithValue(pollingKey{}, string(taskKey(old.Owner, old.ID)))
	t := old
	t.LastPoll = uint64(parent.BlockHeight())
	err := k.attempt(child, &t)
	// Host accounting is deliberately outside the discarded execution cache and
	// separately metered. Its reserve is included in admission and escrow checks.
	host := storetypes.NewGasMeter(k.Config.HostReserve)
	accounting := parent.WithGasMeter(host)
	if err == nil {
		commit()
	} else {
		t = old
		t.LastPoll = uint64(parent.BlockHeight())
		t.Status = "paused"
		t.Pause = "trap"
		if failure, ok := err.(pollFailure); ok {
			t.Pause = failure.Reason
		}
		t.Diagnostic = err.Error()
		if len(t.Diagnostic) > 64 {
			t.Diagnostic = t.Diagnostic[:64]
		}
		t.QueueKey = nil
	}
	// First persist the disposition; these real SDK storage costs are charged.
	k.save(accounting, t)
	charged := execution.GasConsumedToLimit() + host.GasConsumedToLimit()
	fee := k.feeAt(charged, price)
	if err == nil {
		t.Escrow = amount(t.Escrow).Add(amount(t.Reserved)).Sub(fee).String()
	} else {
		t.Escrow = amount(old.Escrow).Sub(fee).String()
	}
	t.Reserved = "0"
	t.Fees = amount(t.Fees).Add(fee).String()
	// The final transfer/update are maintenance work, paid by the prepaid cleanup
	// and lease fees, and still count against the async block allocation.
	// The fee was reserved from this task's escrow and the fee collector is a
	// module account, so this fails only if the module escrow is underbacked:
	// the invariant ValidateState treats as fatal. Halting is intentional.
	if e := k.transfer(accounting, authtypes.NewModuleAddress(ModuleName), authtypes.NewModuleAddress(authtypes.FeeCollectorName), fee); e != nil {
		panic(e)
	}
	if err == nil && (t.Status == "completing" || t.Status == "failing") {
		status := "completed"
		if t.Status == "failing" {
			status = "failed"
		}
		k.settle(accounting, &t, status)
	} else {
		k.save(accounting, t)
	}
	if used, ok := parent.Value(pollGasKey{}).(*uint64); ok && used != nil {
		*used += execution.GasConsumedToLimit()
	}
	parent.EventManager().EmitEvent(sdk.NewEvent("async_poll", sdk.NewAttribute("contract", t.Owner), sdk.NewAttribute("task_id", fmt.Sprint(t.ID)), sdk.NewAttribute("execution_gas", fmt.Sprint(execution.GasConsumedToLimit())), sdk.NewAttribute("charged_host_gas", fmt.Sprint(charged-execution.GasConsumedToLimit())), sdk.NewAttribute("maintenance_host_gas", fmt.Sprint(host.GasConsumedToLimit()-(charged-execution.GasConsumedToLimit()))), sdk.NewAttribute("total_gas", fmt.Sprint(execution.GasConsumedToLimit()+host.GasConsumedToLimit())), sdk.NewAttribute("price", price.String()), sdk.NewAttribute("fee", fee.String()), sdk.NewAttribute("pause_reason", t.Pause)))
	return execution.GasConsumedToLimit() + host.GasConsumedToLimit()
}
