package async

import (
	"encoding/json"
	"os"
	"testing"

	sdkmath "cosmossdk.io/math"
	"github.com/stretchr/testify/require"
)

// Literal shared expectations catch wrong rounding, adjustment direction and keys.
func TestBasePriceFixture(t *testing.T) {
	data, err := os.ReadFile("../../fixtures/native-async/v1/base-price.json")
	require.NoError(t, err)
	var fixture struct {
		Prices []struct {
			Name        string
			Base        string
			Used        uint64
			Target      uint64
			Floor       string
			Denominator uint64
			Next        string
		} `json:"prices"`
		BidComponents []struct {
			Bid string
			Key string
		} `json:"bid_components"`
	}
	require.NoError(t, json.Unmarshal(data, &fixture))
	require.NotEmpty(t, fixture.Prices)
	require.NotEmpty(t, fixture.BidComponents)
	for _, price := range fixture.Prices {
		t.Run(price.Name, func(t *testing.T) {
			next := nextBasePrice(sdkmath.LegacyMustNewDecFromStr(price.Base), price.Used, price.Target,
				sdkmath.LegacyMustNewDecFromStr(price.Floor), price.Denominator)
			require.True(t, next.Equal(sdkmath.LegacyMustNewDecFromStr(price.Next)), "got %s, want %s", next, price.Next)
		})
	}
	for _, bid := range fixture.BidComponents {
		t.Run("bid_"+bid.Bid, func(t *testing.T) {
			require.Equal(t, bid.Key, bidComponent(sdkmath.LegacyMustNewDecFromStr(bid.Bid)))
		})
	}
}

func TestBasePriceLargeIntegerInputs(t *testing.T) {
	maxUint64 := ^uint64(0)
	maxPrice := sdkmath.LegacyMustNewDecFromStr("340282366920938463463.374607431768211455")
	require.True(t, nextBasePrice(maxPrice, maxUint64, 1, sdkmath.LegacyZeroDec(), 1).Equal(maxPrice))
	require.True(t, nextBasePrice(sdkmath.LegacyOneDec(), maxUint64, 1, sdkmath.LegacyZeroDec(), maxUint64).
		Equal(sdkmath.LegacyMustNewDecFromStr("1.999999999999999999")))
	require.True(t, nextBasePrice(sdkmath.LegacyOneDec(), 0, maxUint64, sdkmath.LegacyZeroDec(), 1).IsZero())
}

// Go's decimal type is wider and signed; pricing must enforce Rust's domain.
func TestPricingRejectsInvalidDomain(t *testing.T) {
	for _, value := range []sdkmath.LegacyDec{
		{},
		sdkmath.LegacyMustNewDecFromStr("-0.000000000000000001"),
		sdkmath.LegacyMustNewDecFromStr("340282366920938463463.374607431768211456"),
	} {
		require.Panics(t, func() { bidComponent(value) })
		require.Panics(t, func() { nextBasePrice(value, 1, 1, sdkmath.LegacyZeroDec(), 8) })
		require.Panics(t, func() { nextBasePrice(sdkmath.LegacyOneDec(), 1, 1, value, 8) })
	}
	require.Panics(t, func() { nextBasePrice(sdkmath.LegacyOneDec(), 0, 0, sdkmath.LegacyZeroDec(), 8) })
	require.Panics(t, func() { nextBasePrice(sdkmath.LegacyOneDec(), 0, 1, sdkmath.LegacyZeroDec(), 0) })
}
