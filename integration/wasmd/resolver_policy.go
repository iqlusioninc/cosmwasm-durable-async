package async

import (
	"fmt"

	sdk "github.com/cosmos/cosmos-sdk/types"
)

// validateResolver uses the configured chain prefix/address verifier and rejects
// alternate spellings instead of normalizing the identity stored in a wait.
func validateResolver(raw string) (sdk.AccAddress, error) {
	parsed, err := sdk.AccAddressFromBech32(raw)
	if err != nil {
		return nil, err
	}
	if parsed.String() != raw {
		return nil, fmt.Errorf("noncanonical resolver address")
	}
	return parsed, nil
}
