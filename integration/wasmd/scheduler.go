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

// ReserveOrdinaryGas is a keeper-harness helper. Production applications MUST
// install ProposalHandlers; BaseApp resets its block meter after BeginBlock.
func (k *Keeper) ReserveOrdinaryGas(ctx sdk.Context, total uint64) sdk.Context {
	if total <= k.Config.AsyncGas {
		panic("async allocation exceeds block gas")
	}
	return ctx.WithBlockGasMeter(storetypes.NewGasMeter(total - k.Config.AsyncGas))
}

// EndBlock must be called exactly once after ordinary txs and before the app's
// remaining EndBlock modules. total is the consensus maximum block gas.
func (k *Keeper) EndBlock(ctx sdk.Context, total uint64) sdk.Context {
	ordinary := ctx.BlockGasMeter().GasConsumedToLimit()
	if ordinary > total-k.Config.AsyncGas {
		panic("ordinary transactions exceeded reserved partition")
	}
	block := ctx.BlockGasMeter()
	if block.Limit() != total {
		block = storetypes.NewGasMeter(total)
		block.ConsumeGas(ordinary, "ordinary transaction partition")
	}
	meter := storetypes.NewGasMeter(k.Config.MaintenanceGas)
	ctx = ctx.WithBlockGasMeter(block).WithGasMeter(meter).WithValue(schedulerKey{}, true)
	for _, kind := range []string{"expiry", "prune", "timer"} {
		for n := uint64(0); n < k.Config.MaxMaintenance; n++ {
			more := false
			if !cachedMaintenance(ctx, func(c sdk.Context) { more = k.maintenanceOne(c, kind) }) || !more {
				break
			}
		}
	}
	remaining := k.Config.AsyncGas - k.Config.MaintenanceGas
	for visits := uint64(0); visits < k.Config.MaxVisits; visits++ {
		var selected *Task
		stop := false
		if !cachedMaintenance(ctx, func(c sdk.Context) { selected, stop = k.selectReady(c, remaining) }) || stop {
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
	return ctx.WithValue(schedulerKey{}, false)
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
func (k *Keeper) selectReady(ctx sdk.Context, remaining uint64) (*Task, bool) {
	p := []byte("contracts/")
	it := k.store(ctx).Iterator(p, storetypes.PrefixEndBytes(p))
	if !it.Valid() {
		it.Close()
		return nil, true
	}
	ck := bytes.Clone(it.Key())
	owner := string(it.Value())
	it.Close()
	key, value := k.first(ctx, "ready/"+owner, uint64(ctx.BlockHeight()))
	if key == nil {
		k.rotateContract(ctx, ck, owner)
		return nil, false
	}
	b := k.store(ctx).Get(value)
	if b == nil {
		k.store(ctx).Delete(key)
		k.rotateContract(ctx, ck, owner)
		return nil, false
	}
	var t Task
	if e := json.Unmarshal(b, &t); e != nil {
		panic(e)
	}
	if t.GasPerPoll+k.Config.HostReserve > remaining {
		return nil, true
	}
	k.store(ctx).Delete(key)
	k.rotateContract(ctx, ck, owner)
	t.QueueKey = nil
	if terminal(t) || t.Status != "runnable" || t.LastPoll == uint64(ctx.BlockHeight()) {
		return nil, false
	}
	if uint64(ctx.BlockHeight()) >= t.ExpiresAt {
		if e := k.finish(ctx, &t, "expired"); e != nil {
			panic(e)
		}
		return nil, false
	}
	price, _ := sdkmath.LegacyNewDecFromStr(t.MaxGasPrice)
	if price.LT(k.Config.GasPrice) {
		t.Status = "paused"
		t.Pause = "price_limit"
		k.save(ctx, t)
		return nil, false
	}
	if amount(t.Escrow).LT(k.fee(t.GasPerPoll + k.Config.HostReserve)) {
		t.Status = "paused"
		t.Pause = "insufficient_funds"
		k.save(ctx, t)
		return nil, false
	}
	return &t, false
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
			if e := k.finish(ctx, &t, "expired"); e != nil {
				panic(e)
			}
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
		}
		if t.Status != "paused" {
			t.Status = "runnable"
			k.enqueue(ctx, &t, uint64(ctx.BlockHeight()))
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
		t.TimerKey = indexKey("timer", due, t.ID, t.Owner)
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
	extra := uint64(1024)
	if t.Wait != nil && t.Wait.Operation != nil {
		extra += uint64(t.Wait.Operation.MaxResultBytes)
	}
	if uint64(len(marshal(*t)))+extra > t.AllocatedBytes {
		return fmt.Errorf("state exceeds prepaid allocation including metadata reserve")
	}
	k.save(ctx, *t)
	return nil
}
func (k *Keeper) attempt(ctx sdk.Context, t *Task) (err error) {
	defer func() {
		if p := recover(); p != nil {
			switch p.(type) {
			case storetypes.ErrorOutOfGas:
				err = fmt.Errorf("out_of_gas")
			default:
				panic(p)
			}
		}
	}()
	reserve := k.fee(t.GasPerPoll + k.Config.HostReserve)
	t.Reserved = reserve.String()
	t.Escrow = amount(t.Escrow).Sub(reserve).String()
	k.save(ctx, *t)
	if err = k.checkIdentity(ctx, *t); err != nil {
		return err
	}
	data, err := k.Wasm.Sudo(ctx, addr(t.Owner), t.Request())
	if err != nil {
		return err
	}
	if uint64(len(data)) > t.AllocatedBytes {
		return fmt.Errorf("poll response exceeds allocated bytes")
	}
	var response PollResponse
	if err = json.Unmarshal(data, &response); err != nil {
		return err
	}
	if err = k.stage(ctx, t, response); err != nil {
		return err
	}
	for _, m := range response.Messages {
		events, _, _, e := k.messenger.DispatchMsg(ctx, addr(t.Owner), "", m)
		if e != nil {
			return fmt.Errorf("message_failure: %w", e)
		}
		ctx.EventManager().EmitEvents(events)
	}
	// Immediate callback may have persisted an inbox and runnable queue.
	*t, err = k.load(ctx, t.Owner, t.ID)
	return err
}
func (k *Keeper) poll(parent sdk.Context, old Task) uint64 {
	child, commit := parent.CacheContext()
	execution := storetypes.NewGasMeter(old.GasPerPoll)
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
		t.Pause = err.Error()
		if len(t.Pause) > 64 {
			t.Pause = t.Pause[:64]
		}
		t.QueueKey = nil
	}
	// First persist the disposition; these real SDK storage costs are charged.
	k.save(accounting, t)
	charged := execution.GasConsumedToLimit() + host.GasConsumedToLimit()
	fee := k.fee(charged)
	t.Escrow = amount(old.Escrow).Sub(fee).String()
	t.Reserved = "0"
	t.Fees = amount(t.Fees).Add(fee).String()
	// The final transfer/update are maintenance work, paid by the prepaid cleanup
	// and lease fees, and still count against the async block allocation.
	if e := k.transfer(accounting, authtypes.NewModuleAddress(ModuleName), authtypes.NewModuleAddress(authtypes.FeeCollectorName), fee); e != nil {
		panic(e)
	}
	if err == nil && (t.Status == "completing" || t.Status == "failing") {
		status := "completed"
		if t.Status == "failing" {
			status = "failed"
		}
		if e := k.finish(accounting, &t, status); e != nil {
			panic(e)
		}
	} else {
		k.save(accounting, t)
	}
	return execution.GasConsumedToLimit() + host.GasConsumedToLimit()
}
