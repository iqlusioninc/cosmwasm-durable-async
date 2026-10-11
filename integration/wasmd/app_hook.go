package async

import (
	storetypes "cosmossdk.io/store/types"
	"fmt"
	sdk "github.com/cosmos/cosmos-sdk/types"
)

// ApplicationEndBlock partitions scheduler and standard module execution. All
// EndBlock writes share one cache so a required module error never commits a
// partial EndBlock. The caller must propagate the error to ABCI FinalizeBlock.
func (k *Keeper) ApplicationEndBlock(ctx sdk.Context, modules func(sdk.Context) (sdk.EndBlock, error)) (result sdk.EndBlock, err error) {
	p := ctx.ConsensusParams()
	if p.Block == nil || p.Block.MaxGas <= 0 {
		return result, fmt.Errorf("native async requires finite consensus block gas")
	}
	total := uint64(p.Block.MaxGas)
	if _, err = k.Config.OrdinaryGas(total); err != nil {
		return result, err
	}
	child, commit := ctx.CacheContext()
	child = k.EndBlock(child, total)
	moduleMeter := storetypes.NewGasMeter(k.Config.ModuleEndBlockGas)
	charged := false
	charge := func() {
		if !charged {
			charged = true
			child.BlockGasMeter().ConsumeGas(moduleMeter.GasConsumedToLimit(), "application EndBlock modules")
		}
	}
	defer func() {
		if p := recover(); p != nil {
			if _, ok := p.(storetypes.ErrorOutOfGas); ok {
				err = fmt.Errorf("application EndBlock modules exceeded reserved gas")
			} else {
				panic(p)
			}
		}
		charge()
	}()
	result, err = modules(child.WithGasMeter(moduleMeter))
	charge()
	if err != nil {
		return result, err
	}
	commit()
	return result, nil
}
