package async

import (
	"encoding/json"
	"fmt"
	wasmkeeper "github.com/CosmWasm/wasmd/x/wasm/keeper"
	vmtypes "github.com/CosmWasm/wasmvm/v2/types"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	"github.com/stretchr/testify/require"
	"os"
	"testing"
)

// A real Wasm poll emits its outbound request. The injected message dispatcher
// isolates the escrow invariant: it performs an authenticated owner topup, then
// either succeeds or fails. The VM and bank keep their real SDK gas meters.
func TestInflightFundingConservation(t *testing.T) {
	for _, fail := range []bool{false, true} {
		t.Run(fmt.Sprint("dispatch_failure=", fail), func(t *testing.T) {
			ctx, k, keepers := setup(t)
			creator := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 10_000_000))
			code, e := os.ReadFile("../../artifacts/native_fulfill_example.wasm")
			require.NoError(t, e)
			id, _, e := keepers.ContractKeeper.Create(ctx, creator, code, nil)
			require.NoError(t, e)
			owner, _, e := keepers.ContractKeeper.Instantiate(ctx, id, creator, creator, marshal(map[string]any{"payment_service": creator.String(), "shipment_service": creator.String(), "deadline_blocks": 10, "fee_denom": "ugas"}), "funding-fixture", sdk.Coins{})
			require.NoError(t, e)
			start := marshal(map[string]any{"Start": map[string]any{"order": map[string]any{"id": 9}, "business": sdk.NewInt64Coin("ugas", 0), "gas": sdk.NewInt64Coin("ugas", 1_000_000), "gas_per_poll": 100_000_000, "max_gas_price": "1", "expires_at": 100, "allocated_bytes": 524288}})
			_, e = keepers.ContractKeeper.Execute(ctx, owner, creator, start, sdk.NewCoins(sdk.NewInt64Coin("ugas", 1_000_000)))
			require.NoError(t, e)
			require.NoError(t, keepers.BankKeeper.SendCoins(ctx, creator, owner, sdk.NewCoins(sdk.NewInt64Coin("ugas", 50))))
			ctx = ctx.WithBlockHeight(11)
			k.store(ctx).Set([]byte("base_price"), []byte("0.002"))
			old, stop := k.selectReady(ctx, 160_000_000)
			require.False(t, stop)
			require.NotNil(t, old)
			initial := amount(old.Escrow)
			k.messenger = wasmkeeper.MessageHandlerFunc(func(c sdk.Context, a sdk.AccAddress, _ string, _ vmtypes.CosmosMsg) ([]sdk.Event, [][]byte, [][]*codectypes.Any, error) {
				task, e := k.load(c, a.String(), old.ID)
				require.NoError(t, e)
				require.Equal(t, "200000", task.Reserved)
				// Deliberately mutate stored state to catch a callback rereading the
				// live price instead of inheriting this attempt's immutable price.
				k.store(c).Set([]byte("base_price"), []byte("0.004"))
				require.Equal(t, "200000", k.fee(c, task.GasPerPoll).String())
				query, e := k.Query(c, []byte(`{"funding_config":{}}`))
				require.NoError(t, e)
				var config map[string]any
				require.NoError(t, json.Unmarshal(query, &config))
				require.Equal(t, "0.002000000000000000", config["gas_price"])
				e = k.Management(c, a.String(), marshal(map[string]any{"fund_task": FundTaskMsg{TaskID: old.ID, Contributor: creator.String(), Funds: sdk.NewInt64Coin("ugas", 17)}}))
				require.NoError(t, e)
				require.ErrorContains(t, k.Management(c, a.String(), marshal(map[string]any{"cancel_task": map[string]any{"task_id": old.ID, "actor": creator.String()}})), "in-flight")
				if fail {
					return nil, nil, nil, fmt.Errorf("intentional outbound failure after topup")
				}
				return nil, nil, nil, nil
			})
			used := k.poll(ctx, *old)
			task, e := k.load(ctx, owner.String(), old.ID)
			require.NoError(t, e)
			require.LessOrEqual(t, used, old.GasPerPoll)
			expected := initial.Sub(amount(task.Fees))
			ownerBalance := int64(50)
			if fail {
				require.Equal(t, "message_failure", task.Pause)
			} else {
				expected = expected.AddRaw(17)
				ownerBalance -= 17
				require.Equal(t, "waiting", task.Status)
			}
			require.Equal(t, expected.String(), task.Escrow)
			require.Equal(t, "0", task.Reserved)
			require.Equal(t, fmt.Sprint(ownerBalance), keepers.BankKeeper.GetBalance(ctx, owner, "ugas").Amount.String())
			require.Equal(t, task.Escrow, keepers.BankKeeper.GetBalance(ctx, authtypes.NewModuleAddress(ModuleName), "ugas").Amount.String())
			attrs := priceEvent(t, ctx, "async_poll")
			require.Equal(t, "0.002000000000000000", attrs["price"])
			require.Equal(t, map[bool]string{false: "0.004000000000000000", true: "0.002000000000000000"}[fail], k.basePrice(ctx).String())
		})
	}
}
