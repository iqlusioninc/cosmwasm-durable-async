package async

import (
	"fmt"
	"math/big"

	sdkmath "cosmossdk.io/math"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
)

// controlledTask is called only with the authenticated owning contract from the
// custom-message dispatcher. The contract must forward its actual caller as actor.
func (k *Keeper) controlledTask(ctx sdk.Context, owner, actor string, id uint64) (Task, error) {
	task, err := k.active(ctx, owner, id)
	if err != nil {
		return task, err
	}
	if _, err = sdk.AccAddressFromBech32(actor); err != nil || actor != task.Control {
		return task, fmt.Errorf("unauthorized task controller")
	}
	if current, ok := ctx.Value(pollingKey{}).(string); ok && current == string(taskKey(owner, id)) {
		return task, fmt.Errorf("cannot manage an in-flight task")
	}
	if task.Reserved != "0" {
		return task, fmt.Errorf("cannot manage reserved task")
	}
	return task, nil
}

func policyUint128(n *big.Int) (sdkmath.Int, error) {
	if n.Sign() < 0 || n.BitLen() > 128 {
		return sdkmath.Int{}, fmt.Errorf("Uint128 arithmetic overflow")
	}
	return sdkmath.NewIntFromBigInt(n), nil
}

// ExtendLease atomically prepays monotonic lease growth. This adapter retains
// terminal records only to the lease endpoint, so its extra retention tail is zero.
func (k *Keeper) ExtendLease(ctx sdk.Context, owner string, r ExtendLeaseMsg) error {
	cached, commit := ctx.CacheContext()
	task, err := k.controlledTask(cached, owner, r.Actor, r.TaskID)
	if err != nil {
		return err
	}
	c := k.Config
	height := uint64(ctx.BlockHeight())
	if r.AllocatedBytes < task.AllocatedBytes || r.ExpiresAt < task.ExpiresAt ||
		(r.AllocatedBytes == task.AllocatedBytes && r.ExpiresAt == task.ExpiresAt) ||
		r.ExpiresAt-height > c.MaxLifetime || r.AllocatedBytes > c.MaxBytes {
		return fmt.Errorf("invalid lease extension")
	}
	if c.HostReserve < 1_000_000 || r.AllocatedBytes > (c.HostReserve-1_000_000)/120 {
		return fmt.Errorf("allocation exceeds bounded failure-recording slice")
	}
	delta := r.AllocatedBytes - task.AllocatedBytes
	store := k.store(cached)
	used := number(store, "bytes")
	if delta > c.MaxBytes || used > c.MaxBytes-delta {
		return fmt.Errorf("runtime capacity exhausted")
	}
	byteBlocks := new(big.Int).Mul(new(big.Int).SetUint64(delta), new(big.Int).SetUint64(task.ExpiresAt-height))
	byteBlocks.Add(byteBlocks, new(big.Int).Mul(new(big.Int).SetUint64(r.AllocatedBytes), new(big.Int).SetUint64(r.ExpiresAt-task.ExpiresAt)))
	rent, err := policyUint128(byteBlocks.Mul(byteBlocks, c.StorageByteBlockPrice.BigInt()))
	if err != nil {
		return err
	}
	if amount(task.Escrow).LT(rent) {
		return fmt.Errorf("insufficient execution escrow for lease")
	}
	paid, err := policyUint128(new(big.Int).Add(amount(task.LeasePaid).BigInt(), rent.BigInt()))
	if err != nil {
		return err
	}
	task.Escrow = amount(task.Escrow).Sub(rent).String()
	task.LeasePaid = paid.String()
	task.AllocatedBytes = r.AllocatedBytes
	task.ExpiresAt = r.ExpiresAt
	if err = k.checkQuota(task); err != nil {
		return err
	}
	if err = k.transfer(cached, authtypes.NewModuleAddress(ModuleName), authtypes.NewModuleAddress(authtypes.FeeCollectorName), rent); err != nil {
		return err
	}
	store.Delete(task.ExpiryKey)
	task.ExpiryKey = indexKey("expiry", task.ExpiresAt, task.ID, task.Owner)
	store.Set(task.ExpiryKey, taskKey(task.Owner, task.ID))
	setNumber(store, "bytes", used+delta)
	k.save(cached, task)
	commit()
	return nil
}

// UpdateBudget changes policy without changing readiness, fee accounting, or a
// saved continuation. Identical updates are accepted; Resume checks affordability.
func (k *Keeper) UpdateBudget(ctx sdk.Context, owner string, r UpdateBudgetMsg) error {
	cached, commit := ctx.CacheContext()
	task, err := k.controlledTask(cached, owner, r.Actor, r.TaskID)
	if err != nil {
		return err
	}
	refund, err := validateRefund(r.Refund)
	if err != nil {
		return err
	}
	if r.GasPerPoll <= k.Config.HostReserve || r.GasPerPoll > k.Config.MaxPollGas ||
		k.Config.MaintenanceGas >= k.Config.AsyncGas || r.GasPerPoll > k.Config.AsyncGas-k.Config.MaintenanceGas {
		return fmt.Errorf("invalid poll gas cap")
	}
	if err = validateMaxGasPrice(r.MaxGasPrice); err != nil {
		return err
	}
	// Use unbounded intermediates to reject overflow rather than panic inside SDK math.
	scaled := new(big.Int).Mul(k.Config.GasPrice.BigInt(), new(big.Int).SetUint64(r.GasPerPoll))
	scale := big.NewInt(1_000_000_000_000_000_000)
	scaled.Add(scaled, new(big.Int).Sub(new(big.Int).Set(scale), big.NewInt(1)))
	if _, err = policyUint128(scaled.Quo(scaled, scale)); err != nil {
		return err
	}
	task.GasPerPoll = r.GasPerPoll
	task.MaxGasPrice = r.MaxGasPrice
	task.Refund = refund.String()
	if err = k.checkQuota(task); err != nil {
		return err
	}
	k.save(cached, task)
	commit()
	return nil
}
