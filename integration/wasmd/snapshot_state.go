package async

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/binary"
	"encoding/json"
	"fmt"
	"math"
	"strconv"
	"strings"

	sdkmath "cosmossdk.io/math"
	wasmtypes "github.com/CosmWasm/wasmd/x/wasm/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
)

const stateManifestKey = "state_manifest"

// The manifest is part of the authenticated application state, not an assertion
// supplied by a snapshot peer. Any change requires an explicit chain upgrade.
func (k *Keeper) stateManifest() []byte {
	return marshal(struct {
		Schema uint32 `json:"schema"`
		Config Config `json:"config"`
	}{2, k.Config})
}

// InitState is genesis-only. In particular, missing old-chain metadata is not
// silently synthesized during restore or ordinary application startup.
func (k *Keeper) InitState(ctx sdk.Context) error {
	if err := k.Config.Validate(); err != nil {
		return err
	}
	if k.store(ctx).Has([]byte(stateManifestKey)) {
		return fmt.Errorf("native async state already initialized")
	}
	it := k.store(ctx).Iterator(nil, nil)
	empty := !it.Valid()
	it.Close()
	if !empty {
		return fmt.Errorf("cannot initialize native async manifest over existing state")
	}
	k.store(ctx).Set([]byte(stateManifestKey), k.stateManifest())
	k.store(ctx).Set([]byte("base_price"), []byte(k.Config.MinBasePrice.String()))
	setNumber(k.store(ctx), "last_poll_gas", 0)
	return nil
}

// ValidateState is a read-only, full-state activation check. It intentionally
// tolerates overdue tasks/timers and surplus module donations: bounded cleanup
// can leave expired tasks at a committed height, and bank transfers can donate.
// It never polls a contract, advances an index, or charges scheduler escrow.
func (k *Keeper) ValidateState(ctx sdk.Context) error {
	if err := k.Config.Validate(); err != nil {
		return err
	}
	s := k.store(ctx)
	if !bytes.Equal(s.Get([]byte(stateManifestKey)), k.stateManifest()) {
		return fmt.Errorf("native async schema or configuration mismatch (or missing manifest)")
	}
	expected := map[string][]byte{stateManifestKey: k.stateManifest()}
	if b := s.Get([]byte("base_price")); b != nil {
		if _, err := storedPrice(b, k.Config.MinBasePrice); err != nil {
			return err
		}
		expected["base_price"] = b
	}
	if b := s.Get([]byte("last_poll_gas")); b != nil {
		if len(b) != 8 || binary.BigEndian.Uint64(b) > k.Config.AsyncGas-k.Config.MaintenanceGas {
			return fmt.Errorf("invalid last poll gas")
		}
		expected["last_poll_gas"] = b
	}
	counters := map[string]uint64{}
	for _, name := range []string{"next_id", "ready_sequence", "contract_sequence", "active", "bytes"} {
		b := s.Get([]byte(name))
		if b != nil {
			if len(b) != 8 {
				return fmt.Errorf("invalid %s counter", name)
			}
			counters[name] = binary.BigEndian.Uint64(b)
			expected[name] = b
		}
	}
	owners := map[string]uint64{}
	readyOwners := map[string]bool{}
	ids := map[uint64]bool{}
	var active, allocated uint64
	escrow := sdkmath.ZeroInt()
	it := s.Iterator([]byte("task/"), []byte("task0"))
	defer it.Close()
	for ; it.Valid(); it.Next() {
		var t Task
		if err := json.Unmarshal(it.Value(), &t); err != nil {
			return fmt.Errorf("invalid task: %w", err)
		}
		key := taskKey(t.Owner, t.ID)
		if !bytes.Equal(it.Key(), key) || t.ID == 0 || t.ID > counters["next_id"] || ids[t.ID] {
			return fmt.Errorf("invalid task identity")
		}
		ids[t.ID] = true
		for _, a := range []string{t.Owner, t.Control, t.Refund} {
			if _, err := sdk.AccAddressFromBech32(a); err != nil {
				return fmt.Errorf("invalid task address: %w", err)
			}
		}
		if _, err := validateRefund(t.Refund); err != nil {
			return err
		}
		if t.Status != "runnable" && t.Status != "waiting" && t.Status != "paused" && !terminal(t) {
			return fmt.Errorf("invalid committed task status %q", t.Status)
		}
		if t.Reserved != "0" {
			return fmt.Errorf("committed task has in-flight reservation")
		}
		for _, n := range []string{t.Escrow, t.Fees, t.LeasePaid} {
			v, ok := sdkmath.NewIntFromString(n)
			if !ok || v.IsNegative() {
				return fmt.Errorf("invalid task funds")
			}
		}
		if t.AllocatedBytes < 1024 || t.AllocatedBytes > k.Config.MaxBytes || allocated > math.MaxUint64-t.AllocatedBytes {
			return fmt.Errorf("invalid task allocation")
		}
		allocated += t.AllocatedBytes
		if err := k.checkQuota(t); err != nil {
			return err
		}
		if t.ExpiresAt == 0 || t.GasPerPoll <= k.Config.HostReserve || t.GasPerPoll > k.Config.MaxPollGas {
			return fmt.Errorf("invalid task limits")
		}
		if err := validateMaxGasPrice(t.MaxGasPrice); err != nil {
			return err
		}
		if len(t.Checksum) != sha256.Size {
			return fmt.Errorf("invalid code checksum")
		}
		expected[string(key)] = bytes.Clone(it.Value())
		kind := "expiry"
		if terminal(t) {
			kind = "prune"
			if t.Escrow != "0" || t.Wait != nil || t.Inbox != nil || t.QueueKey != nil || t.TimerKey != nil {
				return fmt.Errorf("terminal task retains active resources")
			}
		} else {
			active++
			owners[t.Owner]++
			escrow = escrow.Add(amount(t.Escrow))
			ci := k.Wasm.GetContractInfo(ctx, addr(t.Owner))
			if ci == nil {
				return fmt.Errorf("task contract missing")
			}
			code := k.Wasm.GetCodeInfo(ctx, ci.CodeID)
			if code == nil || !bytes.Equal(code.CodeHash, t.Checksum) {
				return fmt.Errorf("active task code binding mismatch")
			}
		}
		if !bytes.Equal(t.ExpiryKey, indexKey(kind, t.ExpiresAt, t.ID, t.Owner)) {
			return fmt.Errorf("invalid task lease index")
		}
		expected[string(t.ExpiryKey)] = key
		if (t.Status == "runnable") != (t.QueueKey != nil) {
			return fmt.Errorf("task readiness index mismatch")
		}
		if t.QueueKey != nil {
			p := strings.Split(string(t.QueueKey), "/")
			if len(p) != 5 || p[0] != "ready" || p[1] != t.Owner || p[4] != "" {
				return fmt.Errorf("invalid ready key")
			}
			seq, err := strconv.ParseUint(p[3], 10, 64)
			if err != nil || seq == 0 || seq > counters["ready_sequence"] || !bytes.Equal(t.QueueKey, indexKey("ready/"+t.Owner, t.ReadyAt, seq, "")) {
				return fmt.Errorf("invalid ready sequence")
			}
			if _, duplicate := expected[string(t.QueueKey)]; duplicate {
				return fmt.Errorf("tasks share a readiness slot")
			}
			expected[string(t.QueueKey)] = key
			readyOwners[t.Owner] = true
		}
		if t.Inbox != nil {
			if err := validResolution(*t.Inbox); err != nil {
				return err
			}
			if t.Wait != nil && t.Wait.Operation != nil {
				var resultBytes uint64
				if t.Inbox.Success != nil {
					resultBytes = uint64(len(t.Inbox.Success.Data))
				}
				if t.Inbox.Error != nil {
					resultBytes = uint64(len(t.Inbox.Error.Code)) + uint64(len(t.Inbox.Error.Message))
				}
				if resultBytes > uint64(t.Wait.Operation.MaxResultBytes) || t.Inbox.Timer != nil {
					return fmt.Errorf("invalid operation inbox")
				}
			} else if t.Inbox.Timer == nil || !t.Started {
				return fmt.Errorf("invalid timer or yield inbox")
			}
		}
		if t.Wait != nil {
			if (t.Wait.Height == nil) == (t.Wait.Operation == nil) || t.Sequence == 0 || !t.Started {
				return fmt.Errorf("invalid committed wait")
			}
			due := uint64(0)
			if t.Wait.Height != nil {
				due = t.Wait.Height.Height
			} else {
				due = t.Wait.Operation.Deadline
				if _, err := sdk.AccAddressFromBech32(t.Wait.Operation.Resolver); err != nil {
					return err
				}
			}
			if due == 0 || due >= t.ExpiresAt {
				return fmt.Errorf("wait outside lease")
			}
			if t.TimerKey != nil {
				if !bytes.Equal(t.TimerKey, indexKey("timer", due, t.ID, t.Owner)) {
					return fmt.Errorf("invalid timer key")
				}
				expected[string(t.TimerKey)] = key
			} else if t.Inbox == nil {
				return fmt.Errorf("wait without timer or inbox")
			}
		} else if t.TimerKey != nil {
			return fmt.Errorf("timer without wait")
		}
		if t.Status == "waiting" && (t.Wait == nil || t.Inbox != nil) {
			return fmt.Errorf("invalid waiting task")
		}
	}
	if active != counters["active"] || allocated != counters["bytes"] || active > k.Config.MaxTasks || allocated > k.Config.MaxBytes {
		return fmt.Errorf("native async aggregate counter mismatch")
	}
	for owner, n := range owners {
		b := make([]byte, 8)
		binary.BigEndian.PutUint64(b, n)
		expected["active/"+owner] = b
	}
	for owner := range readyOwners {
		meta := "contract-queue/" + owner
		key := s.Get([]byte(meta))
		p := strings.Split(string(key), "/")
		if len(p) != 3 || p[0] != "contracts" || p[2] != owner {
			return fmt.Errorf("missing contract queue")
		}
		seq, err := strconv.ParseUint(p[1], 10, 64)
		if err != nil || seq == 0 || seq > counters["contract_sequence"] || string(key) != fmt.Sprintf("contracts/%020d/%s", seq, owner) {
			return fmt.Errorf("invalid contract queue sequence")
		}
		expected[meta] = key
		expected[string(key)] = []byte(owner)
	}
	all := s.Iterator(nil, nil)
	defer all.Close()
	for ; all.Valid(); all.Next() {
		v, ok := expected[string(all.Key())]
		if !ok || !bytes.Equal(v, all.Value()) {
			return fmt.Errorf("unexpected or inconsistent native async key %q", all.Key())
		}
		delete(expected, string(all.Key()))
	}
	if len(expected) != 0 {
		return fmt.Errorf("missing native async index or counter")
	}
	bank, ok := k.Bank.(interface {
		GetBalance(context.Context, sdk.AccAddress, string) sdk.Coin
	})
	if !ok {
		return fmt.Errorf("bank balance reader required for activation")
	}
	if bank.GetBalance(ctx, authtypes.NewModuleAddress(ModuleName), k.Config.Denom).Amount.LT(escrow) {
		return fmt.Errorf("scheduler escrow is not fully backed")
	}
	return k.validateCodeInventory(ctx)
}

func (k *Keeper) validateCodeInventory(ctx sdk.Context) error {
	var result error
	k.Wasm.IterateCodeInfos(ctx, func(id uint64, info wasmtypes.CodeInfo) bool {
		code, err := k.Wasm.GetByteCode(ctx, id)
		if err != nil {
			result = fmt.Errorf("missing code %d: %w", id, err)
			return true
		}
		checksum := sha256.Sum256(code)
		if !bytes.Equal(checksum[:], info.CodeHash) {
			result = fmt.Errorf("code %d checksum mismatch", id)
			return true
		}
		return false
	})
	return result
}
