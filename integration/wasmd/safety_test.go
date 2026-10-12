package async

import (
	"bytes"
	storetypes "cosmossdk.io/store/types"
	abci "github.com/cometbft/cometbft/abci/types"
	cmtproto "github.com/cometbft/cometbft/proto/tendermint/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/stretchr/testify/require"
	"testing"
)

func TestProposalGasPartition(t *testing.T) {
	ctx, k, keepers := setup(t)
	ctx = ctx.WithConsensusParams(cmtproto.ConsensusParams{Block: &cmtproto.BlockParams{MaxGas: 400_000_000}})
	noopPrepare := func(_ sdk.Context, r *abci.RequestPrepareProposal) (*abci.ResponsePrepareProposal, error) {
		return &abci.ResponsePrepareProposal{Txs: r.Txs}, nil
	}
	noopProcess := func(_ sdk.Context, _ *abci.RequestProcessProposal) (*abci.ResponseProcessProposal, error) {
		return &abci.ResponseProcessProposal{Status: abci.ResponseProcessProposal_ACCEPT}, nil
	}
	prepare, process := k.ProposalHandlers(keepers.EncodingConfig.TxConfig.TxDecoder(), noopPrepare, noopProcess)
	tx := func(gas uint64) []byte {
		b := keepers.EncodingConfig.TxConfig.NewTxBuilder()
		b.SetGasLimit(gas)
		raw, e := keepers.EncodingConfig.TxConfig.TxEncoder()(b.GetTx())
		require.NoError(t, e)
		return raw
	}
	small, big := tx(90_000_000), tx(180_000_001)
	response, e := process(ctx, &abci.RequestProcessProposal{Txs: [][]byte{big}})
	require.NoError(t, e)
	require.Equal(t, abci.ResponseProcessProposal_REJECT, response.Status)
	response, e = process(ctx, &abci.RequestProcessProposal{Txs: [][]byte{small, small}})
	require.NoError(t, e)
	require.Equal(t, abci.ResponseProcessProposal_ACCEPT, response.Status)
	response, e = process(ctx, &abci.RequestProcessProposal{Txs: [][]byte{tx(^uint64(0)), small}})
	require.NoError(t, e)
	require.Equal(t, abci.ResponseProcessProposal_REJECT, response.Status)
	proposal, e := prepare(ctx, &abci.RequestPrepareProposal{Txs: [][]byte{small, big, small}})
	require.NoError(t, e)
	require.Len(t, proposal.Txs, 2)
	require.Equal(t, int64(400_000_000), ctx.ConsensusParams().Block.MaxGas)
}
func TestMaintenanceOutOfGasAtomic(t *testing.T) {
	ctx, k, _ := setup(t)
	key := []byte("sentinel")
	k.store(ctx).Set(key, []byte("before"))
	ctx = ctx.WithGasMeter(storetypes.NewGasMeter(1_000))
	ok := cachedMaintenance(ctx, func(c sdk.Context) {
		k.store(c).Set(key, []byte("half-written"))
		c.GasMeter().ConsumeGas(10_000, "exhaust maintenance")
	})
	require.False(t, ok)
	ctx = ctx.WithGasMeter(storetypes.NewInfiniteGasMeter())
	require.Equal(t, []byte("before"), k.store(ctx).Get(key))
	k.Config.MaintenanceGas = 1
	ctx = k.ReserveOrdinaryGas(ctx, 400_000_000)
	require.NotPanics(t, func() { ctx = k.EndBlock(ctx, 400_000_000) })
}
func TestPersistentContractRoundRobin(t *testing.T) {
	ctx, k, _ := setup(t)
	a := Task{ID: 1, Owner: "a", Status: "runnable", Escrow: "99999999", Fees: "0", Register: Register{GasPerPoll: 1, MaxGasPrice: "1", ExpiresAt: 100}}
	b := a
	b.ID = 2
	c := a
	c.ID = 3
	c.Owner = "b"
	for _, p := range []*Task{&a, &b, &c} {
		k.enqueue(ctx, p, 10)
		k.save(ctx, *p)
	}
	for _, want := range []uint64{1, 3, 2} {
		selected, stop := k.selectReady(ctx, 90_000_000)
		require.False(t, stop)
		require.NotNil(t, selected)
		require.Equal(t, want, selected.ID)
	}
	require.Nil(t, k.store(ctx).Get([]byte("contract-queue/a")))
	require.Nil(t, k.store(ctx).Get([]byte("contract-queue/b")))
}
func TestInflightManagementRejected(t *testing.T) {
	ctx, k, _ := setup(t)
	owner := sdk.AccAddress(bytes.Repeat([]byte{1}, 20)).String()
	t0 := Task{ID: 1, Owner: owner, Status: "waiting", Escrow: "1000", Register: Register{Control: owner, ExpiresAt: 100}}
	k.save(ctx, t0)
	ctx = ctx.WithValue(pollingKey{}, string(taskKey(owner, 1)))
	require.ErrorContains(t, k.Management(ctx, owner, marshal(map[string]any{"cancel_task": map[string]any{"task_id": 1, "actor": owner}})), "in-flight")
}

// A runnable task polled earlier in this block is deferred, never dropped, and
// a stale entry for a non-runnable task leaves no dangling queue key.
func TestSelectReadyKeepsSkippedTasksConsistent(t *testing.T) {
	ctx, k, _ := setup(t)
	polled := Task{ID: 1, Owner: "a", Status: "runnable", LastPoll: 10, Escrow: "99999999", Fees: "0", Register: Register{GasPerPoll: 1, MaxGasPrice: "1", ExpiresAt: 100}}
	paused := Task{ID: 2, Owner: "b", Status: "paused", Pause: "trap", Escrow: "99999999", Fees: "0", Register: Register{GasPerPoll: 1, MaxGasPrice: "1", ExpiresAt: 100}}
	for _, p := range []*Task{&polled, &paused} {
		k.enqueue(ctx, p, 10)
		k.save(ctx, *p)
	}
	for i := 0; i < 2; i++ {
		selected, stop := k.selectReady(ctx, 90_000_000)
		require.False(t, stop)
		require.Nil(t, selected)
	}
	got, err := k.load(ctx, "a", 1)
	require.NoError(t, err)
	require.Equal(t, uint64(11), got.ReadyAt)
	require.Equal(t, indexKey("ready/a", 11, 3, ""), got.QueueKey)
	require.Equal(t, taskKey("a", 1), k.store(ctx).Get(got.QueueKey))
	got, err = k.load(ctx, "b", 2)
	require.NoError(t, err)
	require.Nil(t, got.QueueKey)
}
