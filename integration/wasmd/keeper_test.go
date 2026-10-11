package async

import (
	"bytes"
	sdkmath "cosmossdk.io/math"
	storetypes "cosmossdk.io/store/types"
	"encoding/json"
	wasmkeeper "github.com/CosmWasm/wasmd/x/wasm/keeper"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	"github.com/stretchr/testify/require"
	"os"
	"path/filepath"
	"testing"
)

func testConfig() Config {
	return Config{Denom: "ugas", GasPrice: sdkmath.LegacyMustNewDecFromStr("0.001"), MaxPollGas: 10_000_000, AsyncGas: 100_000_000, MaintenanceGas: 40_000_000, HostReserve: 20_000_000, MaxTasks: 100, MaxBytes: 10_000_000, MaxLifetime: 10_000, MaxMessages: 10, MaxVisits: 20, MaxMaintenance: 20, StorageByteBlockPrice: sdkmath.ZeroInt(), CleanupPrice: sdkmath.NewInt(100)}
}
func setup(t *testing.T) (sdk.Context, *Keeper, wasmkeeper.TestKeepers) {
	t.Helper()
	k := &Keeper{Config: testConfig()}
	ctx, keepers := wasmkeeper.CreateTestInput(t, false, wasmkeeper.BuiltInCapabilities(), k.Options()...)
	k.Key = keepers.WasmStoreKey
	k.Bank = keepers.BankKeeper
	k.Wasm = keepers.WasmKeeper
	module := keepers.AccountKeeper.NewAccount(ctx, authtypes.NewEmptyModuleAccount(ModuleName)).(sdk.ModuleAccountI)
	keepers.AccountKeeper.SetModuleAccount(ctx, module)
	return ctx.WithBlockHeight(10), k, keepers
}
func TestGoldenABI(t *testing.T) {
	files, e := filepath.Glob("../../fixtures/native-async/v1/*.json")
	require.NoError(t, e)
	require.NotEmpty(t, files)
	for _, f := range files {
		t.Run(filepath.Base(f), func(t *testing.T) {
			b, e := os.ReadFile(f)
			require.NoError(t, e)
			var round []byte
			switch filepath.Base(f) {
			case "complete.json", "failed.json", "pending-height.json", "pending-operation.json":
				var p PollResponse
				require.NoError(t, json.Unmarshal(b, &p))
				round = marshal(p)
			case "register.json":
				var p struct {
					Register Register `json:"register"`
				}
				require.NoError(t, json.Unmarshal(b, &p))
				round = marshal(p)
			case "poll-start.json", "poll-resume.json":
				var p PollSudo
				require.NoError(t, json.Unmarshal(b, &p))
				round = marshal(p)
			case "deliver-error.json":
				var p struct {
					Deliver DeliverMsg `json:"deliver"`
				}
				require.NoError(t, json.Unmarshal(b, &p))
				round = marshal(p)
			case "fund.json":
				var p struct {
					Funding struct {
						FundTask FundTaskMsg `json:"fund_task"`
					} `json:"funding"`
				}
				require.NoError(t, json.Unmarshal(b, &p))
				round = marshal(p)
			default:
				t.Fatalf("unhandled ABI fixture %s", f)
			}
			require.JSONEq(t, string(b), string(round))
		})
	}
}

func TestRealWasmOutboundRollback(t *testing.T) {
	artifact := os.Getenv("NATIVE_ASYNC_WASM")
	if artifact == "" {
		artifact = "../../artifacts/native_fulfill_example.wasm"
	}
	code, e := os.ReadFile(artifact)
	require.NoError(t, e, "build the real Rust native-fulfill contract before running integration tests")
	ctx, k, keepers := setup(t)
	creator := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 100_000_000))
	payment := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 1))
	shipment := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 1))
	id, _, e := keepers.ContractKeeper.Create(ctx, creator, code, nil)
	require.NoError(t, e)
	init := marshal(map[string]any{"payment_service": payment.String(), "shipment_service": shipment.String(), "deadline_blocks": 10, "fee_denom": "ugas"})
	owner, _, e := keepers.ContractKeeper.Instantiate(ctx, id, creator, creator, init, "native-async", sdk.Coins{})
	require.NoError(t, e)
	start := marshal(map[string]any{"Start": map[string]any{"order": map[string]any{"id": 9}, "business": sdk.NewInt64Coin("ugas", 0), "gas": sdk.NewInt64Coin("ugas", 1_000_000), "gas_per_poll": 10_000_000, "max_gas_price": "1", "expires_at": 100, "allocated_bytes": 131072}})
	// No service deployed yet: this intentionally exercises real Wasm writes plus
	// outbound failure rollback and paid retry, then keeper-delivered results.
	_, e = keepers.ContractKeeper.Execute(ctx, owner, creator, start, sdk.NewCoins(sdk.NewInt64Coin("ugas", 1_000_000)))
	require.NoError(t, e)
	task, e := k.load(ctx, owner.String(), 1)
	require.NoError(t, e)
	require.Equal(t, "runnable", task.Status)
	before := amount(task.Escrow)
	ctx = k.ReserveOrdinaryGas(ctx, 200_000_000)
	ctx = k.EndBlock(ctx, 200_000_000)
	task, e = k.load(ctx, owner.String(), 1)
	require.NoError(t, e)
	require.False(t, task.Started, "registration cannot poll at H")
	ctx = ctx.WithBlockHeight(11).WithGasMeter(storetypes.NewInfiniteGasMeter())
	ctx = k.ReserveOrdinaryGas(ctx, 200_000_000)
	ctx = k.EndBlock(ctx, 200_000_000)
	task, e = k.load(ctx, owner.String(), 1)
	require.NoError(t, e)
	require.Equal(t, "paused", task.Status)
	require.False(t, task.Started, "outbound failure must roll back the staged continuation")
	require.True(t, amount(task.Escrow).LT(before))
	require.True(t, amount(task.Fees).IsPositive())
	require.Error(t, k.CheckMigration(ctx, owner))
	t.Logf("real Wasm outbound-failure attempt fees=%s gas-price=%s pause=%s", task.Fees, k.Config.GasPrice, task.Pause)
	snapshot := marshal(task)
	replacement := &Keeper{Key: k.Key, Bank: k.Bank, Wasm: k.Wasm, Config: k.Config, messenger: k.messenger}
	restored, e := replacement.load(ctx, owner.String(), 1)
	require.NoError(t, e)
	require.True(t, bytes.Equal(snapshot, marshal(restored)), "keeper reconstruction must read committed durable state")
}

func TestRealWasmTwoWaitCompletion(t *testing.T) {
	ctx, k, keepers := setup(t)
	creator := keepers.Faucet.NewFundedRandomAccount(ctx, sdk.NewInt64Coin("ugas", 100_000_000))
	deploy := func(path string, init []byte) sdk.AccAddress {
		code, e := os.ReadFile(path)
		require.NoError(t, e)
		id, _, e := keepers.ContractKeeper.Create(ctx, creator, code, nil)
		require.NoError(t, e)
		a, _, e := keepers.ContractKeeper.Instantiate(ctx, id, creator, creator, init, "fixture", sdk.Coins{})
		require.NoError(t, e)
		return a
	}
	payment := deploy("../../artifacts/native_service.wasm", []byte(`{}`))
	shipment := deploy("../../artifacts/native_service.wasm", []byte(`{}`))
	owner := deploy("../../artifacts/native_fulfill_example.wasm", marshal(map[string]any{"payment_service": payment.String(), "shipment_service": shipment.String(), "deadline_blocks": 10, "fee_denom": "ugas"}))
	start := marshal(map[string]any{"Start": map[string]any{"order": map[string]any{"id": 9}, "business": sdk.NewInt64Coin("ugas", 0), "gas": sdk.NewInt64Coin("ugas", 1_000_000), "gas_per_poll": 10_000_000, "max_gas_price": "1", "expires_at": 100, "allocated_bytes": 131072}})
	_, e := keepers.ContractKeeper.Execute(ctx, owner, creator, start, sdk.NewCoins(sdk.NewInt64Coin("ugas", 1_000_000)))
	require.NoError(t, e)
	block := func(h int64) {
		ctx = ctx.WithBlockHeight(h).WithGasMeter(storetypes.NewInfiniteGasMeter())
		ctx = k.ReserveOrdinaryGas(ctx, 200_000_000)
		ctx = k.EndBlock(ctx, 200_000_000)
		ctx = ctx.WithGasMeter(storetypes.NewInfiniteGasMeter())
	}
	block(11)
	task, e := k.load(ctx, owner.String(), 1)
	require.NoError(t, e)
	require.Equal(t, "waiting", task.Status, task.Pause)
	require.Equal(t, uint64(1), task.Sequence)
	require.True(t, task.Started)
	release := func(service sdk.AccAddress, seq uint64, output any) {
		msg := marshal(map[string]any{"Release": map[string]any{"correlation": map[string]any{"workflow_id": 1, "wait_sequence": seq}, "outcome": map[string]any{"Success": marshal(output)}}})
		_, e := keepers.ContractKeeper.Execute(ctx, service, creator, msg, sdk.Coins{})
		require.NoError(t, e)
	}
	// The callback is emitted by the actual service contract, so resolver identity
	// reaches the owner as MessageInfo.sender from wasmd, never a claimed caller.
	ctx = ctx.WithBlockHeight(12)
	release(payment, 1, map[string]any{"reference": "paid"})
	block(12)
	task, e = k.load(ctx, owner.String(), 1)
	require.NoError(t, e)
	require.Equal(t, "waiting", task.Status, task.Pause)
	require.Equal(t, uint64(2), task.Sequence)
	// Reconstruct the keeper from persisted state between external waits.
	k = &Keeper{Key: k.Key, Bank: k.Bank, Wasm: k.Wasm, Config: k.Config, messenger: k.messenger}
	ctx = ctx.WithBlockHeight(13)
	release(shipment, 2, map[string]any{"tracking": "sent"})
	block(13)
	task, e = k.load(ctx, owner.String(), 1)
	require.NoError(t, e)
	require.Equal(t, "completed", task.Status, task.Pause)
	require.Equal(t, "0", task.Escrow)
	require.True(t, amount(task.Fees).IsPositive())
	require.NoError(t, k.CheckMigration(ctx, owner))
	require.True(t, keepers.BankKeeper.GetBalance(ctx, authtypes.NewModuleAddress(ModuleName), "ugas").IsZero(), "all unused execution funds refunded")
	t.Logf("real Wasm completed two waits; execution fees=%s ugas", task.Fees)
}
