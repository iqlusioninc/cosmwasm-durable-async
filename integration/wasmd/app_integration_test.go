//go:build upstream

package async_test

import (
	"bytes"
	storetypes "cosmossdk.io/store/types"
	async "cosmwasm-durable-async/integration/wasmd"
	"encoding/json"
	wasmapp "github.com/CosmWasm/wasmd/app"
	wasmkeeper "github.com/CosmWasm/wasmd/x/wasm/keeper"
	cmtproto "github.com/cometbft/cometbft/proto/tendermint/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/stretchr/testify/require"
	"os"
	"testing"
	"time"
)

func TestPatchedAppBlockHookAndMigrationGuard(t *testing.T) {
	app := wasmapp.Setup(t)
	require.NotNil(t, app.AsyncKeeper)
	ctx := app.NewContextLegacy(false, cmtproto.Header{Height: 10, Time: time.Unix(1_700_000_000, 0)}).WithConsensusParams(cmtproto.ConsensusParams{Block: &cmtproto.BlockParams{MaxGas: 400_000_000}})
	creator := sdk.AccAddress(bytes.Repeat([]byte{7}, 20))
	app.AccountKeeper.SetAccount(ctx, app.AccountKeeper.NewAccountWithAddress(ctx, creator))
	coins := sdk.NewCoins(sdk.NewInt64Coin("ugas", 100_000_000))
	require.NoError(t, app.BankKeeper.MintCoins(ctx, "mint", coins))
	require.NoError(t, app.BankKeeper.SendCoinsFromModuleToAccount(ctx, "mint", creator, coins))
	wasm := wasmkeeper.NewDefaultPermissionKeeper(app.WasmKeeper)
	code, e := os.ReadFile("../../artifacts/native_fulfill_example.wasm")
	require.NoError(t, e)
	id, _, e := wasm.Create(ctx, creator, code, nil)
	require.NoError(t, e)
	init, _ := json.Marshal(map[string]any{"payment_service": creator.String(), "shipment_service": creator.String(), "deadline_blocks": 10, "fee_denom": "ugas"})
	owner, _, e := wasm.Instantiate(ctx, id, creator, creator, init, "app-harness", sdk.Coins{})
	require.NoError(t, e)
	require.NoError(t, app.BankKeeper.SendCoins(ctx, creator, owner, coins))
	_, e = app.AsyncKeeper.RegisterTask(ctx, owner, async.Register{Control: creator.String(), Refund: creator.String(), GasPerPoll: 100_000_000, MaxGasPrice: "1", ExpiresAt: 100, Funds: sdk.NewInt64Coin("ugas", 100_000_000), Program: async.Program{Kind: "fulfill", Version: 1, Input: []byte(`{"order":{"id":9}}`)}, AllocatedBytes: 524288})
	require.NoError(t, e)
	_, e = wasm.Migrate(ctx, owner, creator, id, []byte(`{}`))
	require.ErrorContains(t, e, "live async tasks")
	_, e = app.EndBlocker(ctx.WithBlockHeight(11).WithBlockGasMeter(storetypes.NewGasMeter(400_000_000)))
	require.NoError(t, e)
	raw, e := app.AsyncKeeper.Query(ctx, json.RawMessage(`{"task":{"contract":"`+owner.String()+`","task_id":1}}`))
	require.NoError(t, e)
	var task async.TaskResponse
	require.NoError(t, json.Unmarshal(raw, &task))
	require.Equal(t, "paused", task.Status.Kind)
	raw, e = app.AsyncKeeper.Query(ctx, json.RawMessage(`{"funding":{"task_funding":{"contract":"`+owner.String()+`","task_id":1}}}`))
	require.NoError(t, e)
	var funding struct {
		Fees string `json:"cumulative_execution_fees"`
	}
	require.NoError(t, json.Unmarshal(raw, &funding))
	require.NotEqual(t, "0", funding.Fees)
	stats := map[string]map[string]string{}
	for _, event := range ctx.EventManager().Events() {
		if event.Type == "async_poll" || event.Type == "async_base_price" {
			stats[event.Type] = map[string]string{}
			for _, attr := range event.Attributes {
				stats[event.Type][attr.Key] = attr.Value
			}
		}
	}
	require.Equal(t, "0.001000000000000000", stats["async_poll"]["price"])
	require.Equal(t, map[string]string{"old": "0.001000000000000000", "new": "0.001000000000000000", "used": stats["async_poll"]["execution_gas"], "target": "80000000"}, stats["async_base_price"])
}

func TestPatchedAppModuleMeterIsolation(t *testing.T) {
	app := wasmapp.Setup(t)
	app.AsyncKeeper.Config.MaintenanceGas = async.PriceFinalizationGas + 1
	ctx := app.NewContextLegacy(false, cmtproto.Header{Height: 2, Time: time.Unix(1_700_000_000, 0)}).WithConsensusParams(cmtproto.ConsensusParams{Block: &cmtproto.BlockParams{MaxGas: 400_000_000}}).WithBlockGasMeter(storetypes.NewGasMeter(400_000_000))
	ordinary, e := app.AsyncKeeper.Config.OrdinaryGas(400_000_000)
	require.NoError(t, e)
	ctx.BlockGasMeter().ConsumeGas(ordinary, "maximum ordinary transaction allocation")
	_, e = app.EndBlocker(ctx)
	require.NoError(t, e)
	require.Greater(t, ctx.BlockGasMeter().GasConsumed(), ordinary+1, "normal modules must run on their own meter despite exhausted async maintenance")
	require.LessOrEqual(t, ctx.BlockGasMeter().GasConsumed(), uint64(400_000_000))
}
