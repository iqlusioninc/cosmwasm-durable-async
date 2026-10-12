package async

import (
	"fmt"
	"math/big"

	sdkmath "cosmossdk.io/math"
	sdk "github.com/cosmos/cosmos-sdk/types"
)

// PriceFinalizationGas is reserved inside MaintenanceGas for two bounded state
// reads and writes. SDK default KV costs for <=40-byte prices and an 8-byte gas
// counter are well below this allowance, even at the maximum Decimal value.
const PriceFinalizationGas uint64 = 100_000

type blockPriceKey struct{}
type pollGasKey struct{}

func storedPrice(value []byte, floor sdkmath.LegacyDec) (sdkmath.LegacyDec, error) {
	if len(value) == 0 || len(value) > 40 || validateMaxGasPrice(string(value)) != nil {
		return sdkmath.LegacyDec{}, fmt.Errorf("invalid base price")
	}
	price, err := sdkmath.LegacyNewDecFromStr(string(value))
	if err != nil || price.LT(floor) {
		return sdkmath.LegacyDec{}, fmt.Errorf("base price below floor")
	}
	return price, nil
}

// Scheduler descendants (including immediate callbacks) retain the immutable
// block-start price. Ordinary registration and queries read the live state.
func (k *Keeper) basePrice(ctx sdk.Context) sdkmath.LegacyDec {
	if price, ok := ctx.Value(blockPriceKey{}).(sdkmath.LegacyDec); ok && !price.IsNil() {
		return price
	}
	b := k.store(ctx).Get([]byte("base_price"))
	if b == nil {
		return k.Config.MinBasePrice
	}
	price, err := storedPrice(b, k.Config.MinBasePrice)
	if err != nil {
		panic(err)
	}
	return price
}

var maxPriceAtomics = new(big.Int).Sub(new(big.Int).Lsh(big.NewInt(1), 128), big.NewInt(1))

// priceAtomics copies the input and enforces the unsigned CosmWasm Decimal domain.
// Callers must validate prices before persisting or passing them to pricing.
func priceAtomics(price sdkmath.LegacyDec) *big.Int {
	atomics := price.BigInt()
	if atomics == nil || atomics.Sign() < 0 || atomics.BitLen() > 128 {
		panic("price exceeds unsigned 128-bit atomics domain")
	}
	return atomics
}

// nextBasePrice adjusts atomics with truncating division in protocol order.
// Callers must provide nonzero divisors and prices in the Decimal domain.
func nextBasePrice(base sdkmath.LegacyDec, used, target uint64, floor sdkmath.LegacyDec, denominator uint64) sdkmath.LegacyDec {
	if target == 0 || denominator == 0 {
		panic("price adjustment divisors must be nonzero")
	}
	baseAtomics, floorAtomics := priceAtomics(base), priceAtomics(floor)
	var delta uint64
	if used > target {
		delta = used - target
	} else {
		delta = target - used
	}
	magnitude := new(big.Int).Mul(baseAtomics, new(big.Int).SetUint64(delta))
	magnitude.Quo(magnitude, new(big.Int).SetUint64(target))
	magnitude.Quo(magnitude, new(big.Int).SetUint64(denominator))
	if used > target {
		baseAtomics.Add(baseAtomics, magnitude)
		if baseAtomics.Cmp(maxPriceAtomics) > 0 {
			baseAtomics.Set(maxPriceAtomics)
		}
	} else {
		// The delta is at most target, so magnitude cannot exceed base.
		baseAtomics.Sub(baseAtomics, magnitude)
		if baseAtomics.Cmp(floorAtomics) < 0 {
			baseAtomics.Set(floorAtomics)
		}
	}
	return sdkmath.LegacyNewDecFromBigIntWithPrec(baseAtomics, sdkmath.LegacyPrecision)
}

// bidComponent sorts higher bids first in ascending key order.
func bidComponent(bid sdkmath.LegacyDec) string {
	inverted := new(big.Int).Sub(maxPriceAtomics, priceAtomics(bid))
	return fmt.Sprintf("%032x", inverted)
}
