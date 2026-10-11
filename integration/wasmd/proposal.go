package async

import (
	"fmt"
	abci "github.com/cometbft/cometbft/abci/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
)

// ProposalHandlers wraps the SDK's normal handlers. It both supplies their
// transaction selector the reduced gas budget and validates declared gas itself
// (the SDK no-op mempool prepare path otherwise bypasses gas selection).
func (k *Keeper) ProposalHandlers(decode sdk.TxDecoder, prepare sdk.PrepareProposalHandler, process sdk.ProcessProposalHandler) (sdk.PrepareProposalHandler, sdk.ProcessProposalHandler) {
	budget := func(ctx sdk.Context) (sdk.Context, uint64, error) {
		p := ctx.ConsensusParams()
		if p.Block == nil || p.Block.MaxGas <= 0 || uint64(p.Block.MaxGas) <= k.Config.AsyncGas {
			return ctx, 0, fmt.Errorf("native async requires a finite block gas limit greater than its reserve")
		}
		b := *p.Block
		b.MaxGas -= int64(k.Config.AsyncGas)
		p.Block = &b
		return ctx.WithConsensusParams(p), uint64(b.MaxGas), nil
	}
	selectTxs := func(txs [][]byte, max uint64) ([][]byte, bool) {
		var out [][]byte
		used := uint64(0)
		all := true
		for _, raw := range txs {
			tx, e := decode(raw)
			if e != nil {
				all = false
				continue
			}
			fee, ok := tx.(sdk.FeeTx)
			if !ok || fee.GetGas() > max-used {
				all = false
				continue
			}
			used += fee.GetGas()
			out = append(out, raw)
		}
		return out, all
	}
	return func(ctx sdk.Context, req *abci.RequestPrepareProposal) (*abci.ResponsePrepareProposal, error) {
			ctx, max, e := budget(ctx)
			if e != nil {
				return nil, e
			}
			r, e := prepare(ctx, req)
			if e != nil {
				return nil, e
			}
			r.Txs, _ = selectTxs(r.Txs, max)
			return r, nil
		}, func(ctx sdk.Context, req *abci.RequestProcessProposal) (*abci.ResponseProcessProposal, error) {
			ctx, max, e := budget(ctx)
			if e != nil {
				return nil, e
			}
			if _, all := selectTxs(req.Txs, max); !all {
				return &abci.ResponseProcessProposal{Status: abci.ResponseProcessProposal_REJECT}, nil
			}
			return process(ctx, req)
		}
}
