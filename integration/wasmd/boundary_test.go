package async

import (
	"bytes"
	storetypes "cosmossdk.io/store/types"
	"encoding/json"
	cmtproto "github.com/cometbft/cometbft/proto/tendermint/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/stretchr/testify/require"
	"os"
	"testing"
)

func TestSharedResultSizeBoundaries(t *testing.T) {
	b, e := os.ReadFile("testdata/result-boundaries.json")
	require.NoError(t, e)
	var cases []struct {
		Name       string     `json:"name"`
		Max        uint32     `json:"max_result_bytes"`
		Resolution Resolution `json:"resolution"`
		Accepted   bool       `json:"accepted"`
	}
	require.NoError(t, json.Unmarshal(b, &cases))
	ctx, k, _ := setup(t)
	owner := sdk.AccAddress(bytes.Repeat([]byte{2}, 20)).String()
	for i, c := range cases {
		t.Run(c.Name, func(t *testing.T) {
			var w Wait
			require.NoError(t, json.Unmarshal(marshal(map[string]any{"operation": map[string]any{"resolver": owner, "deadline": 20, "max_result_bytes": c.Max}}), &w))
			task := Task{ID: uint64(i + 1), Owner: owner, Register: Register{ExpiresAt: 100, AllocatedBytes: 8192}, Wait: &w, Sequence: 1, Status: "waiting"}
			k.save(ctx, task)
			err := k.Deliver(ctx, owner, task.ID, 1, owner, c.Resolution)
			if c.Accepted {
				require.NoError(t, err)
			} else {
				require.Error(t, err)
			}
		})
	}
}
func TestModuleEndBlockMeterIsolationAndRollback(t *testing.T) {
	ctx, k, _ := setup(t)
	k.Config.MaintenanceGas = 1
	ctx = ctx.WithConsensusParams(cmtproto.ConsensusParams{Block: &cmtproto.BlockParams{MaxGas: 400_000_000}}).WithBlockGasMeter(storetypes.NewGasMeter(400_000_000))
	budget, e := k.Config.OrdinaryGas(400_000_000)
	require.NoError(t, e)
	ctx.BlockGasMeter().ConsumeGas(budget, "ordinary tx maximum")
	_, e = k.ApplicationEndBlock(ctx, func(c sdk.Context) (sdk.EndBlock, error) {
		require.Equal(t, k.Config.ModuleEndBlockGas, c.GasMeter().Limit())
		require.Zero(t, c.GasMeter().GasConsumed())
		c.GasMeter().ConsumeGas(1234, "module work")
		return sdk.EndBlock{}, nil
	})
	require.NoError(t, e)
	require.Equal(t, budget+1+1234, ctx.BlockGasMeter().GasConsumed())
	require.LessOrEqual(t, ctx.BlockGasMeter().GasConsumed(), uint64(400_000_000))
	ctx = ctx.WithBlockGasMeter(storetypes.NewGasMeter(400_000_000))
	k.store(ctx).Set([]byte("module-sentinel"), []byte("before"))
	_, e = k.ApplicationEndBlock(ctx, func(c sdk.Context) (sdk.EndBlock, error) {
		k.store(c).Set([]byte("module-sentinel"), []byte("partial"))
		c.GasMeter().ConsumeGas(k.Config.ModuleEndBlockGas, "module overflow")
		return sdk.EndBlock{}, nil
	})
	require.ErrorContains(t, e, "exceeded reserved gas")
	require.Equal(t, []byte("before"), k.store(ctx).Get([]byte("module-sentinel")))
	require.LessOrEqual(t, ctx.BlockGasMeter().GasConsumed(), uint64(400_000_000))
}
func TestInvalidAllocationRejected(t *testing.T) {
	for _, mutate := range []func(*Config){func(c *Config) { c.AsyncGas = ^uint64(0) }, func(c *Config) { c.ModuleEndBlockGas = 0 }, func(c *Config) { c.MaintenanceGas = c.AsyncGas }, func(c *Config) { c.HostReserve = c.AsyncGas }} {
		c := ExperimentalConfig()
		mutate(&c)
		require.Error(t, c.Validate())
	}
	_, e := ExperimentalConfig().OrdinaryGas(220_000_000)
	require.Error(t, e)
}
