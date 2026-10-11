package async

import (
	"bytes"
	"context"
	sdkmath "cosmossdk.io/math"
	"cosmossdk.io/store/prefix"
	storetypes "cosmossdk.io/store/types"
	"encoding/binary"
	"encoding/json"
	"fmt"
	wasmkeeper "github.com/CosmWasm/wasmd/x/wasm/keeper"
	wasmtypes "github.com/CosmWasm/wasmd/x/wasm/types"
	vmtypes "github.com/CosmWasm/wasmvm/v2/types"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	"math"
)

const ModuleName = "native_async"

type Bank interface {
	SendCoins(context.Context, sdk.AccAddress, sdk.AccAddress, sdk.Coins) error
}
type Config struct {
	ModuleEndBlockGas                                                       uint64
	Denom                                                                   string
	GasPrice                                                                sdkmath.LegacyDec
	MaxPollGas, AsyncGas, MaintenanceGas, HostReserve                       uint64
	MaxTasks, MaxBytes, MaxLifetime, MaxMessages, MaxVisits, MaxMaintenance uint64
	StorageByteBlockPrice, CleanupPrice                                     sdkmath.Int
}
type Keeper struct {
	Key       storetypes.StoreKey
	Bank      Bank
	Wasm      *wasmkeeper.Keeper
	Config    Config
	messenger wasmkeeper.Messenger
}

func (k *Keeper) store(ctx sdk.Context) storetypes.KVStore {
	return prefix.NewStore(ctx.KVStore(k.Key), []byte("native-async/"))
}
func taskKey(owner string, id uint64) []byte { return []byte(fmt.Sprintf("task/%s/%020d", owner, id)) }
func indexKey(kind string, height, id uint64, owner string) []byte {
	return []byte(fmt.Sprintf("%s/%020d/%020d/%s", kind, height, id, owner))
}
func number(s storetypes.KVStore, key string) uint64 {
	b := s.Get([]byte(key))
	if b == nil {
		return 0
	}
	return binary.BigEndian.Uint64(b)
}
func setNumber(s storetypes.KVStore, key string, n uint64) {
	b := make([]byte, 8)
	binary.BigEndian.PutUint64(b, n)
	s.Set([]byte(key), b)
}
func (k *Keeper) load(ctx sdk.Context, owner string, id uint64) (Task, error) {
	var t Task
	b := k.store(ctx).Get(taskKey(owner, id))
	if b == nil {
		return t, fmt.Errorf("unknown task")
	}
	if e := json.Unmarshal(b, &t); e != nil {
		panic(e)
	}
	return t, nil
}
func (k *Keeper) save(ctx sdk.Context, t Task) { k.store(ctx).Set(taskKey(t.Owner, t.ID), marshal(t)) }

// Maximum mutable scalar widths, queue metadata, and bounded diagnostics fit
// the fixed 1024-byte slack. Inbox space is reserved independently of readiness.
func (k *Keeper) checkQuota(t Task) error {
	extra := uint64(1024)
	if t.Wait != nil && t.Wait.Operation != nil {
		extra += uint64(t.Wait.Operation.MaxResultBytes)*6 + 128
	}
	t.Inbox = nil
	if uint64(len(marshal(t)))+extra > t.AllocatedBytes {
		return fmt.Errorf("state exceeds prepaid allocation including metadata and inbox reserve")
	}
	return nil
}
func terminal(t Task) bool {
	return t.Status == "completed" || t.Status == "failed" || t.Status == "cancelled" || t.Status == "expired"
}
func (k *Keeper) active(ctx sdk.Context, owner string, id uint64) (Task, error) {
	t, e := k.load(ctx, owner, id)
	if e != nil {
		return t, e
	}
	if terminal(t) || uint64(ctx.BlockHeight()) >= t.ExpiresAt {
		return t, fmt.Errorf("task terminal or expired")
	}
	return t, nil
}
func addr(s string) sdk.AccAddress {
	a, e := sdk.AccAddressFromBech32(s)
	if e != nil {
		panic(e)
	}
	return a
}
func amount(s string) sdkmath.Int {
	n, ok := sdkmath.NewIntFromString(s)
	if !ok {
		panic("invalid persisted escrow")
	}
	return n
}
func (k *Keeper) coin(n sdkmath.Int) sdk.Coins {
	if n.IsZero() {
		return sdk.Coins{}
	}
	return sdk.NewCoins(sdk.NewCoin(k.Config.Denom, n))
}
func (k *Keeper) transfer(ctx sdk.Context, from, to sdk.AccAddress, n sdkmath.Int) error {
	if n.IsZero() {
		return nil
	}
	return k.Bank.SendCoins(ctx, from, to, k.coin(n))
}
func (k *Keeper) fee(gas uint64) sdkmath.Int {
	return k.Config.GasPrice.MulInt(sdkmath.NewIntFromUint64(gas)).Ceil().TruncateInt()
}
func (k *Keeper) enqueue(ctx sdk.Context, t *Task, height uint64) {
	s := k.store(ctx)
	if t.QueueKey != nil {
		s.Delete(t.QueueKey)
	}
	n := number(s, "ready_sequence")
	if n == math.MaxUint64 {
		panic("ready sequence exhausted")
	}
	n++
	setNumber(s, "ready_sequence", n)
	t.ReadyAt = height
	t.QueueKey = indexKey("ready/"+t.Owner, height, n, "")
	s.Set(t.QueueKey, taskKey(t.Owner, t.ID))
	k.ensureContract(ctx, t.Owner)
}
func (k *Keeper) ensureContract(ctx sdk.Context, owner string) {
	s := k.store(ctx)
	meta := []byte("contract-queue/" + owner)
	if s.Has(meta) {
		return
	}
	n := number(s, "contract_sequence")
	if n == math.MaxUint64 {
		panic("contract queue exhausted")
	}
	n++
	setNumber(s, "contract_sequence", n)
	key := []byte(fmt.Sprintf("contracts/%020d/%s", n, owner))
	s.Set(key, []byte(owner))
	s.Set(meta, key)
}
func (k *Keeper) rotateContract(ctx sdk.Context, key []byte, owner string) {
	s := k.store(ctx)
	s.Delete(key)
	s.Delete([]byte("contract-queue/" + owner))
	p := []byte("ready/" + owner + "/")
	it := s.Iterator(p, storetypes.PrefixEndBytes(p))
	exists := it.Valid()
	it.Close()
	if exists {
		k.ensureContract(ctx, owner)
	}
}
func (k *Keeper) cleanContract(ctx sdk.Context, owner string) {
	s := k.store(ctx)
	meta := []byte("contract-queue/" + owner)
	key := s.Get(meta)
	if key == nil {
		return
	}
	p := []byte("ready/" + owner + "/")
	it := s.Iterator(p, storetypes.PrefixEndBytes(p))
	exists := it.Valid()
	it.Close()
	if !exists {
		s.Delete(key)
		s.Delete(meta)
	}
}
func (k *Keeper) clearWait(ctx sdk.Context, t *Task) {
	s := k.store(ctx)
	if t.TimerKey != nil {
		s.Delete(t.TimerKey)
	}
	t.TimerKey = nil
	t.Wait = nil
	t.Inbox = nil
}
func (k *Keeper) finish(ctx sdk.Context, t *Task, status string) error {
	s := k.store(ctx)
	if t.QueueKey != nil {
		s.Delete(t.QueueKey)
	}
	k.cleanContract(ctx, t.Owner)
	k.clearWait(ctx, t)
	s.Delete(t.ExpiryKey)
	t.QueueKey = nil
	t.Status = status
	if status != "completed" && status != "failed" {
		t.State = nil
	}
	t.Program.Input = nil
	if e := k.transfer(ctx, authtypes.NewModuleAddress(ModuleName), addr(t.Refund), amount(t.Escrow)); e != nil {
		return e
	}
	t.Escrow = "0"
	setNumber(s, "active", number(s, "active")-1)
	key := "active/" + t.Owner
	if remaining := number(s, key) - 1; remaining == 0 {
		s.Delete([]byte(key))
	} else {
		setNumber(s, key, remaining)
	}
	// A bounded terminal record is retained only to the prepaid lease endpoint.
	t.ExpiryKey = indexKey("prune", t.ExpiresAt, t.ID, t.Owner)
	s.Set(t.ExpiryKey, taskKey(t.Owner, t.ID))
	k.save(ctx, *t)
	return nil
}
func (k *Keeper) RegisterTask(ctx sdk.Context, owner sdk.AccAddress, r Register) (uint64, error) {
	c := k.Config
	s := k.store(ctx)
	height := uint64(ctx.BlockHeight())
	if err := validateMaxGasPrice(r.MaxGasPrice); err != nil {
		return 0, err
	}
	if r.GasPerPoll <= c.HostReserve || r.GasPerPoll > c.MaxPollGas || r.ExpiresAt <= height || r.ExpiresAt-height > c.MaxLifetime || r.AllocatedBytes < 1024 || r.AllocatedBytes > c.MaxBytes || uint64(len(r.Program.Input)) > r.AllocatedBytes/2 || len(r.Program.Kind) > 128 || r.Program.Kind == "" {
		return 0, fmt.Errorf("invalid limits")
	}
	// Fixed-size banking plus two bounded JSON task writes must fit the host
	// recording slice even when guest gas is exhausted. SDK v0.50 default KV
	// writes cost 30 gas/byte; 120 covers JSON expansion and both record writes.
	if c.HostReserve < 1_000_000 || r.AllocatedBytes > (c.HostReserve-1_000_000)/120 {
		return 0, fmt.Errorf("allocation exceeds bounded failure-recording slice")
	}
	if number(s, "active") >= c.MaxTasks || number(s, "bytes") > c.MaxBytes-r.AllocatedBytes {
		return 0, fmt.Errorf("admission full")
	}
	if _, err := sdk.AccAddressFromBech32(r.Control); err != nil {
		return 0, err
	}
	if _, err := validateRefund(r.Refund); err != nil {
		return 0, err
	}
	if !r.Funds.IsValid() || !r.Funds.IsPositive() || r.Funds.Denom != c.Denom || r.Funds.Amount.BigInt().BitLen() > 128 {
		return 0, fmt.Errorf("invalid funding")
	}
	ci := k.Wasm.GetContractInfo(ctx, owner)
	if ci == nil {
		return 0, fmt.Errorf("owner is not a contract")
	}
	code := k.Wasm.GetCodeInfo(ctx, ci.CodeID)
	lease := c.StorageByteBlockPrice.Mul(sdkmath.NewIntFromUint64(r.AllocatedBytes)).Mul(sdkmath.NewIntFromUint64(r.ExpiresAt - height)).Add(c.CleanupPrice)
	if r.Funds.Amount.LT(lease.Add(k.fee(r.GasPerPoll))) {
		return 0, fmt.Errorf("insufficient prepaid lease and execution funding")
	}
	id := number(s, "next_id")
	if id == math.MaxUint64 {
		return 0, fmt.Errorf("id exhausted")
	}
	id++
	t := Task{ID: id, Owner: owner.String(), Register: r, Checksum: code.CodeHash, Status: "runnable", Escrow: r.Funds.Amount.Sub(lease).String(), Fees: "0", Reserved: "0", LeasePaid: lease.String()}
	if e:=k.checkQuota(t);e!=nil{return 0,e}
	if e := k.transfer(ctx, owner, authtypes.NewModuleAddress(ModuleName), r.Funds.Amount); e != nil {
		return 0, e
	}
	if e := k.transfer(ctx, authtypes.NewModuleAddress(ModuleName), authtypes.NewModuleAddress(authtypes.FeeCollectorName), lease); e != nil {
		return 0, e
	}
	t.ExpiryKey = indexKey("expiry", r.ExpiresAt, id, t.Owner)
	s.Set(t.ExpiryKey, taskKey(t.Owner, id))
	k.enqueue(ctx, &t, height+1)
	k.save(ctx, t)
	setNumber(s, "next_id", id)
	setNumber(s, "active", number(s, "active")+1)
	setNumber(s, "bytes", number(s, "bytes")+r.AllocatedBytes)
	key := "active/" + t.Owner
	setNumber(s, key, number(s, key)+1)
	return id, nil
}
func (k *Keeper) Deliver(ctx sdk.Context, owner string, id, sequence uint64, resolver string, r Resolution) error {
	t, e := k.active(ctx, owner, id)
	if e != nil {
		return e
	}
	if e = validResolution(r); e != nil {
		return e
	}
	if r.Timer != nil || r.Timeout != nil {
		return fmt.Errorf("host-only resolution")
	}
	if t.Wait == nil || t.Wait.Operation == nil || t.Sequence != sequence || t.Inbox != nil {
		return fmt.Errorf("stale or duplicate resolution")
	}
	w := t.Wait.Operation
	resultBytes := uint64(0)
	if r.Success != nil {
		resultBytes = uint64(len(r.Success.Data))
	} else {
		resultBytes = uint64(len(r.Error.Code)) + uint64(len(r.Error.Message))
	}
	if resolver != w.Resolver || uint64(ctx.BlockHeight()) >= w.Deadline || resultBytes > uint64(w.MaxResultBytes) {
		return fmt.Errorf("unauthorized, late or oversized resolution")
	}
	t.Inbox = &r
	if e := k.checkQuota(t); e != nil {
		return e
	}
	if t.Status != "paused" {
		t.Status = "runnable"
		height := uint64(ctx.BlockHeight())
		if ctx.Value(schedulerKey{}) == true {
			height++
		}
		k.enqueue(ctx, &t, height)
	}
	k.save(ctx, t)
	return nil
}
func (k *Keeper) Management(ctx sdk.Context, owner string, raw json.RawMessage) error {
	var msg map[string]json.RawMessage
	if e := json.Unmarshal(raw, &msg); e != nil {
		return e
	}
	if len(msg) != 1 {
		return fmt.Errorf("invalid funding message")
	}
	for kind, p := range msg {
		var a struct {
			FundTaskMsg
			Actor string `json:"actor"`
		}
		if e := json.Unmarshal(p, &a); e != nil {
			return e
		}
		t, e := k.active(ctx, owner, a.TaskID)
		if e != nil {
			return e
		}
		if current, ok := ctx.Value(pollingKey{}).(string); kind != "fund_task" && ok && current == string(taskKey(owner, a.TaskID)) {
			return fmt.Errorf("cannot manage an in-flight task")
		}
		switch kind {
		case "fund_task":
			if !a.Funds.IsValid() || !a.Funds.IsPositive() || a.Funds.Denom != k.Config.Denom {
				return fmt.Errorf("invalid deposit")
			}
			if amount(t.Escrow).Add(amount(t.Reserved)).Add(a.Funds.Amount).BigInt().BitLen() > 128 {
				return fmt.Errorf("escrow overflow")
			}
			if e = k.transfer(ctx, addr(owner), authtypes.NewModuleAddress(ModuleName), a.Funds.Amount); e != nil {
				return e
			}
			t.Escrow = amount(t.Escrow).Add(a.Funds.Amount).String()
			ctx.EventManager().EmitEvent(sdk.NewEvent("async_fund", sdk.NewAttribute("contract", owner), sdk.NewAttribute("task_id", fmt.Sprint(t.ID)), sdk.NewAttribute("contributor", a.Contributor), sdk.NewAttribute("amount", a.Funds.Amount.String()), sdk.NewAttribute("available", t.Escrow)))
		case "cancel_task":
			if a.Actor != t.Control {
				return fmt.Errorf("unauthorized control")
			}
			return k.finish(ctx, &t, "cancelled")
		case "resume_task":
			if a.Actor != t.Control || t.Status != "paused" {
				return fmt.Errorf("unauthorized or not paused")
			}
			p, _ := sdkmath.LegacyNewDecFromStr(t.MaxGasPrice)
			if p.LT(k.Config.GasPrice) || amount(t.Escrow).LT(k.fee(t.GasPerPoll)) {
				return fmt.Errorf("insufficient funds or price limit")
			}
			t.Pause = ""
			if t.Wait != nil && t.Inbox == nil {
				t.Status = "waiting"
			} else {
				t.Status = "runnable"
				k.enqueue(ctx, &t, uint64(ctx.BlockHeight())+1)
			}
		default:
			return fmt.Errorf("unknown management message")
		}
		k.save(ctx, t)
	}
	return nil
}
func (k *Keeper) Options() []wasmkeeper.Option {
	return []wasmkeeper.Option{
		wasmkeeper.WithWasmEngineDecorator(func(old wasmtypes.WasmEngine) wasmtypes.WasmEngine { return EngineGuard{old} }),
		wasmkeeper.WithMessageHandlerDecorator(func(old wasmkeeper.Messenger) wasmkeeper.Messenger {
			k.messenger = old
			return wasmkeeper.MessageHandlerFunc(func(ctx sdk.Context, owner sdk.AccAddress, port string, msg vmtypes.CosmosMsg) ([]sdk.Event, [][]byte, [][]*codectypes.Any, error) {
				if msg.Custom == nil {
					return old.DispatchMsg(ctx, owner, port, msg)
				}
				var envelope map[string]json.RawMessage
				if e := json.Unmarshal(msg.Custom, &envelope); e != nil {
					return nil, nil, nil, e
				}
				if len(envelope) != 1 {
					return nil, nil, nil, fmt.Errorf("invalid runtime message")
				}
				var data []byte
				var e error
				switch {
				case envelope["register"] != nil:
					var r Register
					e = json.Unmarshal(envelope["register"], &r)
					if e == nil {
						var id uint64
						id, e = k.RegisterTask(ctx, owner, r)
						data = marshal(id)
					}
				case envelope["funding"] != nil:
					e = k.Management(ctx, owner.String(), envelope["funding"])
				case envelope["extend_lease"] != nil:
					var r ExtendLeaseMsg
					e = json.Unmarshal(envelope["extend_lease"], &r)
					if e == nil {
						e = k.ExtendLease(ctx, owner.String(), r)
					}
				case envelope["update_budget"] != nil:
					var r UpdateBudgetMsg
					e = json.Unmarshal(envelope["update_budget"], &r)
					if e == nil {
						e = k.UpdateBudget(ctx, owner.String(), r)
					}
				case envelope["deliver"] != nil:
					var d DeliverMsg
					e = json.Unmarshal(envelope["deliver"], &d)
					if e == nil {
						e = k.Deliver(ctx, owner.String(), d.TaskID, d.WaitSequence, d.Resolver, d.Resolution)
					}
				default:
					e = fmt.Errorf("unknown runtime message")
				}
				return nil, [][]byte{data}, nil, e
			})
		}),
		wasmkeeper.WithQueryPlugins(&wasmkeeper.QueryPlugins{Custom: k.Query}),
	}
}
func (k *Keeper) Query(ctx sdk.Context, raw json.RawMessage) ([]byte, error) {
	var q map[string]json.RawMessage
	if e := json.Unmarshal(raw, &q); e != nil {
		return nil, e
	}
	if q["funding"] != nil {
		if e := json.Unmarshal(q["funding"], &q); e != nil {
			return nil, e
		}
	}
	if q["funding_config"] != nil {
		return marshal(map[string]any{"denom": k.Config.Denom, "gas_price": k.Config.GasPrice.String(), "max_gas_per_poll": k.Config.MaxPollGas, "max_lifetime_blocks": k.Config.MaxLifetime}), nil
	}
	p := q["task"]
	if p == nil {
		p = q["task_funding"]
	}
	var a struct {
		Contract string `json:"contract"`
		TaskID   uint64 `json:"task_id"`
	}
	if e := json.Unmarshal(p, &a); e != nil {
		return nil, e
	}
	t, e := k.load(ctx, a.Contract, a.TaskID)
	if e != nil {
		return nil, e
	}
	response := publicTask(t)
	if !terminal(t) && uint64(ctx.BlockHeight()) >= t.ExpiresAt {
		t.Status = "expired"
		response.Status = TaskStatus{Kind: "expired"}
	}
	if q["task_funding"] != nil {
		status := response.Status
		return marshal(map[string]any{"task_id": t.ID, "contract": t.Owner, "control": t.Control, "refund": t.Refund, "available": t.Escrow, "reserved": t.Reserved, "cumulative_execution_fees": t.Fees, "gas_per_poll": t.GasPerPoll, "max_gas_price": t.MaxGasPrice, "max_attempt_charge": k.fee(t.GasPerPoll).String(), "expires_at": t.ExpiresAt, "status": status, "allocated_bytes": t.AllocatedBytes, "storage_lease_paid": t.LeasePaid}), nil
	}
	return marshal(response), nil
}

// CheckMigration must be called by the application's migration entry point for
// every migration path (transaction, governance, and contract-emitted message).
func (k *Keeper) CheckMigration(ctx sdk.Context, contract sdk.AccAddress) error {
	if number(k.store(ctx), "active/"+contract.String()) != 0 {
		return fmt.Errorf("contract has live async tasks")
	}
	return nil
}
func (k *Keeper) checkIdentity(ctx sdk.Context, t Task) error {
	ci := k.Wasm.GetContractInfo(ctx, addr(t.Owner))
	if ci == nil {
		return fmt.Errorf("contract missing")
	}
	code := k.Wasm.GetCodeInfo(ctx, ci.CodeID)
	if code == nil || !bytes.Equal(code.CodeHash, t.Checksum) {
		return fmt.Errorf("task code identity changed")
	}
	return nil
}
