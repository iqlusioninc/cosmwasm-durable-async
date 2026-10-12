//go:build upstream

package async_test

import (
	"bytes"
	"encoding/json"
	"os"
	"strconv"
	"testing"
	"time"

	sdkmath "cosmossdk.io/math"
	storetypes "cosmossdk.io/store/types"
	async "cosmwasm-durable-async/integration/wasmd"
	wasmapp "github.com/CosmWasm/wasmd/app"
	wasmkeeper "github.com/CosmWasm/wasmd/x/wasm/keeper"
	cmtproto "github.com/cometbft/cometbft/proto/tendermint/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/stretchr/testify/require"
)

// This proves congestion from real metered Wasm faults, not sustained successful
// application work. Fulfill alone fails its outbound call too early to saturate
// the execution target. Retrying the existing fault fixture retains its inbox.
func TestCongestionRaisesPriceAndPausesLowBidder(t *testing.T) {
	app := wasmapp.Setup(t)
	k := app.AsyncKeeper
	k.Config.MaxPollGas = 160_000_000 // Valid full poll slice; preserve the 70m host reserve.
	k.Config.PriceGraceBlocks = 3
	require.NoError(t, k.Config.Validate())
	require.Equal(t, uint64(70_000_000), k.Config.HostReserve)
	ctx := app.NewContextLegacy(false, cmtproto.Header{Height: 10, Time: time.Unix(1_700_000_000, 0)}).
		WithConsensusParams(cmtproto.ConsensusParams{Block: &cmtproto.BlockParams{MaxGas: 400_000_000}})
	creator := sdk.AccAddress(bytes.Repeat([]byte{28}, 20))
	app.AccountKeeper.SetAccount(ctx, app.AccountKeeper.NewAccountWithAddress(ctx, creator))
	coins := sdk.NewCoins(sdk.NewInt64Coin("ugas", 200_000_000))
	require.NoError(t, app.BankKeeper.MintCoins(ctx, "mint", coins))
	require.NoError(t, app.BankKeeper.SendCoinsFromModuleToAccount(ctx, "mint", creator, coins))
	wasm := wasmkeeper.NewDefaultPermissionKeeper(app.WasmKeeper)
	instantiate := func(path, label string, init []byte, funds int64) sdk.AccAddress {
		code, err := os.ReadFile(path)
		require.NoError(t, err)
		id, _, err := wasm.Create(ctx, creator, code, nil)
		require.NoError(t, err)
		owner, _, err := wasm.Instantiate(ctx, id, creator, creator, init, label, sdk.Coins{})
		require.NoError(t, err)
		require.NoError(t, app.BankKeeper.SendCoins(ctx, creator, owner, sdk.NewCoins(sdk.NewInt64Coin("ugas", funds))))
		return owner
	}
	init, err := json.Marshal(map[string]any{"payment_service": creator.String(), "shipment_service": creator.String(), "deadline_blocks": 10, "fee_denom": "ugas"})
	require.NoError(t, err)
	low := instantiate("../../artifacts/native_fulfill_example.wasm", "auction-fulfill", init, 60_000_000)
	high := instantiate("../../artifacts/native_fault.wasm", "auction-fault", []byte(`{}`), 20_000_000)
	register := func(owner sdk.AccAddress, gas uint64, bid string, funds int64, program async.Program, allocation uint64) uint64 {
		id, err := k.RegisterTask(ctx, owner, async.Register{Control: creator.String(), Refund: creator.String(), GasPerPoll: gas,
			MaxGasPrice: bid, ExpiresAt: 100, Funds: sdk.NewInt64Coin("ugas", funds), Program: program, AllocatedBytes: allocation})
		require.NoError(t, err)
		return id
	}
	task := func(owner sdk.AccAddress, id uint64) async.TaskResponse {
		raw, err := k.Query(ctx, json.RawMessage(`{"task":{"contract":"`+owner.String()+`","task_id":`+strconv.FormatUint(id, 10)+`}}`))
		require.NoError(t, err)
		var result async.TaskResponse
		require.NoError(t, json.Unmarshal(raw, &result))
		return result
	}
	execute := func(owner sdk.AccAddress, msg any) {
		raw, err := json.Marshal(msg)
		require.NoError(t, err)
		_, err = wasm.Execute(ctx, owner, creator, raw, sdk.Coins{})
		require.NoError(t, err)
	}
	block := func(height int64) map[string]string {
		ctx = ctx.WithBlockHeight(height).WithGasMeter(storetypes.NewInfiniteGasMeter()).
			WithBlockGasMeter(storetypes.NewGasMeter(400_000_000)).WithEventManager(sdk.NewEventManager())
		_, err := app.EndBlocker(ctx)
		require.NoError(t, err)
		require.LessOrEqual(t, ctx.BlockGasMeter().GasConsumed(), uint64(400_000_000))
		var price map[string]string
		for _, event := range ctx.EventManager().Events() {
			if event.Type == "async_base_price" {
				require.Nil(t, price, "exactly one price finalization per block")
				price = map[string]string{}
				for _, attr := range event.Attributes {
					price[attr.Key] = attr.Value
				}
			}
		}
		require.NotNil(t, price)
		for _, event := range ctx.EventManager().Events() {
			if event.Type == "async_poll" {
				priceAttributes := 0
				for _, attr := range event.Attributes {
					if attr.Key == "price" {
						priceAttributes++
						require.Equal(t, price["old"], attr.Value, "polls use the frozen block price")
					}
				}
				require.Equal(t, 1, priceAttributes, "every poll emits exactly one price attribute")
			}
		}
		return price
	}
	highIDs := []uint64{
		register(high, 160_000_000, "1", 10_000_000, async.Program{Kind: "fault", Version: 1, Input: []byte{}}, 8192),
		register(high, 160_000_000, "1", 10_000_000, async.Program{Kind: "fault", Version: 1, Input: []byte{}}, 8192),
	}
	// Each 160m cap needs a fresh block even though the initial poll is cheap.
	block(11)
	block(12) // Establish both real continuations and authenticated waits.
	for _, id := range highIDs {
		require.Equal(t, "waiting", task(high, id).Status.Kind, "%+v", task(high, id))
		execute(high, map[string]any{"deliver": map[string]any{"task_id": id, "wait_sequence": 1}})
		require.NotNil(t, task(high, id).Inbox)
	}
	execute(high, map[string]any{"mode": map[string]any{"value": "out_of_gas"}})
	lowID := register(low, 100_000_000, k.Config.MinBasePrice.String(), 60_000_000,
		async.Program{Kind: "fulfill", Version: 1, Input: []byte(`{"order":{"id":9}}`)}, 524288)
	previous := k.Config.MinBasePrice
	for height := int64(13); height <= 16; height++ {
		price := block(height)
		require.Equal(t, previous.String(), price["old"])
		require.Equal(t, "90000000", price["used"], "only guest execution counts, excluding host accounting")
		require.Equal(t, "80000000", price["target"])
		next := sdkmath.LegacyMustNewDecFromStr(price["new"])
		require.True(t, next.GT(previous), "price must rise in every congested block")
		previous = next
		lowTask := task(low, lowID)
		require.Nil(t, lowTask.LastAttempt, "the low bidder must not enter Wasm")
		if height < 16 { // ready at 13, grace age is three at 16.
			require.Equal(t, "runnable", lowTask.Status.Kind)
		} else {
			require.Equal(t, async.TaskStatus{Kind: "paused", Reason: "price_limit", Resume: "runnable"}, lowTask.Status)
		}
		for _, id := range highIDs {
			current := task(high, id)
			require.NotNil(t, current.Inbox, "fault rollback retains the accepted result")
			require.Equal(t, []byte("committed continuation"), current.State)
			if current.Status.Kind == "paused" {
				require.Equal(t, "out_of_gas", current.Status.Reason)
				if height < 16 {
					execute(high, map[string]any{"resume": map[string]any{"task_id": id}})
				}
			}
		}
	}
	for _, id := range highIDs {
		execute(high, map[string]any{"cancel": map[string]any{"task_id": id}})
		require.Equal(t, "cancelled", task(high, id).Status.Kind)
	}
	for height := int64(17); ; height++ {
		require.Less(t, height, int64(30), "idle price must reach its floor")
		price := block(height)
		require.Equal(t, "0", price["used"])
		next := sdkmath.LegacyMustNewDecFromStr(price["new"])
		require.True(t, next.LT(previous))
		previous = next
		require.Equal(t, "price_limit", task(low, lowID).Status.Reason)
		if next.Equal(k.Config.MinBasePrice) {
			break
		}
	}
	require.Nil(t, task(low, lowID).LastAttempt, "falling prices never resume a paused task")
	execute(low, map[string]any{"Async": map[string]any{"resume_task": map[string]any{"task_id": lowID}}})
	require.Equal(t, "runnable", task(low, lowID).Status.Kind)
}
