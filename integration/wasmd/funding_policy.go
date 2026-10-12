package async

import (
	"fmt"
	"regexp"

	sdkmath "cosmossdk.io/math"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
)

var policyDecimal = regexp.MustCompile(`^[0-9]+(\.[0-9]{1,18})?$`)

// Every admitted funding policy must fit the CosmWasm Decimal wire domain:
// an unsigned decimal with at most 18 fractional digits and Uint128 atomics.
func validateMaxGasPrice(value string) error {
	if !policyDecimal.MatchString(value) {
		return fmt.Errorf("invalid maximum gas price")
	}
	price, err := sdkmath.LegacyNewDecFromStr(value)
	if err != nil || price.BigInt().BitLen() > 128 {
		return fmt.Errorf("maximum gas price exceeds Decimal bounds")
	}
	return nil
}

func validateRefund(value string) (sdk.AccAddress, error) {
	refund, err := sdk.AccAddressFromBech32(value)
	if err != nil {
		return nil, err
	}
	if refund.Equals(authtypes.NewModuleAddress(ModuleName)) {
		return nil, fmt.Errorf("escrow cannot receive refunds")
	}
	return refund, nil
}
