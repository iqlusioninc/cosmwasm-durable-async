package async

import (
	"bytes"
	storetypes "cosmossdk.io/store/types"
	wasmkeeper "github.com/CosmWasm/wasmd/x/wasm/keeper"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	"github.com/stretchr/testify/require"
	"os"
	"strconv"
	"testing"
)

type faultHarness struct {
	ctx            sdk.Context
	k              *Keeper
	keepers        wasmkeeper.TestKeepers
	creator, owner sdk.AccAddress
	codeID         uint64
}

// Keep real harness assertions/fatals, but omit its routine IAVL debug logs.
type acceptanceQuietTB struct{ testing.TB }

func (acceptanceQuietTB) Log(...any)          {}
func (acceptanceQuietTB) Logf(string, ...any) {}

func newFaultHarness(t *testing.T) *faultHarness {
	t.Helper()
	code, err := os.ReadFile("../../artifacts/native_fault.wasm")
	require.NoError(t, err, "build the real fault fixture with build-fault-wasmd.sh")
	k := &Keeper{Config: testConfig()}
	ctx, keepers := wasmkeeper.CreateTestInput(acceptanceQuietTB{t}, false, wasmkeeper.BuiltInCapabilities(), k.Options()...)
	k.Key = keepers.WasmStoreKey
	k.Bank = keepers.BankKeeper
	k.Wasm = keepers.WasmKeeper
	module := keepers.AccountKeeper.NewAccount(ctx, authtypes.NewEmptyModuleAccount(ModuleName)).(sdk.ModuleAccountI)
	keepers.AccountKeeper.SetModuleAccount(ctx, module)
	ctx = ctx.WithBlockHeight(10)
	creator := sdk.AccAddress(bytes.Repeat([]byte{19}, 20))
	keepers.Faucet.Fund(ctx, creator, sdk.NewInt64Coin("ugas", 100_000_000))
	id, _, err := keepers.ContractKeeper.Create(ctx, creator, code, nil)
	require.NoError(t, err)
	owner, _, err := keepers.ContractKeeper.Instantiate(ctx, id, creator, creator, []byte(`{}`), "fault", sdk.Coins{})
	require.NoError(t, err)
	return &faultHarness{ctx, k, keepers, creator, owner, id}
}
func (h *faultHarness) execute(t *testing.T, msg any, funds int64) {
	t.Helper()
	coins := sdk.Coins{}
	if funds > 0 {
		coins = sdk.NewCoins(sdk.NewInt64Coin("ugas", funds))
	}
	_, err := h.keepers.ContractKeeper.Execute(h.ctx, h.owner, h.creator, marshal(msg), coins)
	require.NoError(t, err)
}
func (h *faultHarness) start(t *testing.T, gas uint64, expiry uint64) {
	h.execute(t, map[string]any{"start": map[string]any{"gas_per_poll": gas, "expires_at": expiry}}, 1_000_000)
}
func (h *faultHarness) block(height int64) uint64 {
	h.ctx = h.ctx.WithBlockHeight(height).WithGasMeter(storetypes.NewInfiniteGasMeter()).WithEventManager(sdk.NewEventManager())
	h.ctx = h.k.ReserveOrdinaryGas(h.ctx, 400_000_000)
	h.ctx = h.k.EndBlock(h.ctx, 400_000_000)
	gas := h.ctx.BlockGasMeter().GasConsumed()
	h.ctx = h.ctx.WithGasMeter(storetypes.NewInfiniteGasMeter())
	return gas
}
func (h *faultHarness) task(t *testing.T, id uint64) Task {
	t.Helper()
	v, e := h.k.load(h.ctx, h.owner.String(), id)
	require.NoError(t, e)
	return v
}

// Catches committing guest writes or clearing the durable inbox on failed polls,
// charging another task, and retrying paused work without controller action.
func TestAcceptanceRealVMFaultRollback(t *testing.T) {
	for _, mode := range []string{"trap", "out_of_gas", "invalid", "outbound"} {
		t.Run(mode, func(t *testing.T) {
			h := newFaultHarness(t)
			h.start(t, 80_000_000, 100)
			h.start(t, 80_000_000, 100)
			h.block(11)
			before := h.task(t, 1)
			other := h.task(t, 2)
			require.Equal(t, "waiting", before.Status)
			h.execute(t, map[string]any{"deliver": map[string]any{"task_id": 1, "wait_sequence": 1}}, 0)
			h.execute(t, map[string]any{"mode": map[string]any{"value": mode}}, 0)
			before = h.task(t, 1)
			collector := authtypes.NewModuleAddress(authtypes.FeeCollectorName)
			bankBefore := h.keepers.BankKeeper.GetBalance(h.ctx, collector, "ugas").Amount
			gas := h.block(12)
			polls := 0
			var execution, chargedHost, maintenanceHost, total uint64
			for _, event := range h.ctx.EventManager().Events() {
				if event.Type != "async_poll" {
					continue
				}
				polls++
				attrs := map[string]string{}
				for _, a := range event.Attributes {
					attrs[a.Key] = a.Value
				}
				parse := func(key string) uint64 {
					n, e := strconv.ParseUint(attrs[key], 10, 64)
					require.NoError(t, e)
					return n
				}
				execution = parse("execution_gas")
				chargedHost = parse("charged_host_gas")
				maintenanceHost = parse("maintenance_host_gas")
				total = parse("total_gas")
				require.Equal(t, execution+chargedHost+maintenanceHost, total)
				require.LessOrEqual(t, total, before.GasPerPoll)
			}
			require.Equal(t, 1, polls)
			after := h.task(t, 1)
			require.Equal(t, "paused", after.Status)
			require.Equal(t, before.State, after.State)
			require.Equal(t, before.Inbox, after.Inbox)
			require.Equal(t, before.Wait, after.Wait)
			require.Equal(t, before.TimerKey, after.TimerKey)
			require.Equal(t, other, h.task(t, 2))
			raw, e := h.keepers.WasmKeeper.QuerySmart(h.ctx, h.owner, []byte(`{}`))
			require.NoError(t, e)
			require.JSONEq(t, `{"writes":2}`, string(raw))
			paid := amount(after.Fees).Sub(amount(before.Fees))
			require.True(t, paid.IsPositive())
			// Gas price is 0.001: integer ceiling derived independently of Keeper.fee.
			require.Equal(t, strconv.FormatUint((execution+chargedHost+999)/1000, 10), paid.String())
			if mode == "out_of_gas" {
				require.Equal(t, before.GasPerPoll-h.k.Config.HostReserve, execution)
			}
			wantPause := map[string]string{"trap": "trap", "out_of_gas": "out_of_gas", "invalid": "invalid_response", "outbound": "message_failure"}[mode]
			require.Equal(t, wantPause, after.Pause)
			require.Equal(t, paid.String(), amount(before.Escrow).Sub(amount(after.Escrow)).String())
			require.Equal(t, paid.String(), h.keepers.BankKeeper.GetBalance(h.ctx, collector, "ugas").Amount.Sub(bankBefore).String())
			t.Logf("fault=%s block_sdk_gas=%d execution=%d charged_host=%d maintenance_host=%d attempt_total=%d exact_bank_fee=%s pause=%s", mode, gas, execution, chargedHost, maintenanceHost, total, paid, after.Pause)
			h.block(13)
			require.Equal(t, after, h.task(t, 1))
			h.execute(t, map[string]any{"mode": map[string]any{"value": "complete"}}, 0)
			h.execute(t, map[string]any{"resume": map[string]any{"task_id": 1}}, 0)
			h.block(14)
			require.Equal(t, "completed", h.task(t, 1).Status)
		})
	}
}

// With no new arrivals, six tasks on three owners and two visits per block
// require at most six blocks per phase, even when a visit rotates a waiting
// owner's queue. This is a finite workload bound, not arbitrary-arrival liveness.
func TestAcceptanceFiniteRealVMWorkload(t *testing.T) {
	h := newFaultHarness(t)
	h.k.Config.MaxVisits = 2
	h.k.Config.MaxMaintenance = 2
	owners := []sdk.AccAddress{h.owner}
	for i := 0; i < 2; i++ {
		owner, _, e := h.keepers.ContractKeeper.Instantiate(h.ctx, h.codeID, h.creator, h.creator, []byte(`{}`), "finite", sdk.Coins{})
		require.NoError(t, e)
		owners = append(owners, owner)
	}
	type item struct {
		owner sdk.AccAddress
		id    uint64
	}
	tasks := []item{}
	for i, owner := range owners {
		h.owner = owner
		for j := 0; j < 2; j++ {
			h.start(t, 80_000_000+uint64(i%2)*10_000_000, 100)
			tasks = append(tasks, item{owner, uint64(len(tasks) + 1)})
		}
	}
	h.block(10)
	for _, item := range tasks {
		h.owner = item.owner
		require.False(t, h.task(t, item.id).Started)
	}
	checkedBlock := func(height int64) {
		gas := h.block(height)
		polls := map[string]bool{}
		attemptGas := uint64(0)
		for _, event := range h.ctx.EventManager().Events() {
			if event.Type != "async_poll" {
				continue
			}
			attrs := map[string]string{}
			for _, a := range event.Attributes {
				attrs[a.Key] = a.Value
			}
			key := attrs["task_id"]
			n, e := strconv.ParseUint(attrs["total_gas"], 10, 64)
			require.NoError(t, e)
			attemptGas += n
			require.False(t, polls[key], "at most one attempt per task per height")
			polls[key] = true
		}
		require.LessOrEqual(t, len(polls), 2)
		require.LessOrEqual(t, gas, h.k.Config.AsyncGas)
		require.GreaterOrEqual(t, gas, attemptGas)
		require.LessOrEqual(t, gas-attemptGas, h.k.Config.MaintenanceGas)
		t.Logf("finite height=%d polls=%d block_gas=%d maintenance_gas=%d", height, len(polls), gas, gas-attemptGas)
	}
	for height := int64(11); height <= 16; height++ {
		checkedBlock(height)
	}
	for _, item := range tasks {
		h.owner = item.owner
		task := h.task(t, item.id)
		require.Equal(t, "waiting", task.Status)
		h.execute(t, map[string]any{"deliver": map[string]any{"task_id": item.id, "wait_sequence": 1}}, 0)
		_, e := h.keepers.ContractKeeper.Execute(h.ctx, h.owner, h.creator, marshal(map[string]any{"deliver": map[string]any{"task_id": item.id, "wait_sequence": 1}}), sdk.Coins{})
		require.Error(t, e, "duplicate wake must not enqueue another poll")
	}
	for height := int64(17); height <= 22; height++ {
		checkedBlock(height)
	}
	for _, item := range tasks {
		h.owner = item.owner
		require.Equal(t, "completed", h.task(t, item.id).Status)
	}
	require.Zero(t, number(h.k.store(h.ctx), "active"))

	// A separate expiry backlog exceeds both maintenance and visit counts.
	expiry := newFaultHarness(t)
	expiry.k.Config.MaxMaintenance = 2
	expiry.k.Config.MaxVisits = 2
	for i := 0; i < 5; i++ {
		expiry.start(t, 80_000_000, 100)
	}
	// Paused tasks have no ready entries. This isolates indexed expiration
	// from the separate MaxVisits allowance for expiring ready tasks on access.
	for height := int64(11); height <= 13; height++ {
		expiry.block(height)
	}
	expiry.execute(t, map[string]any{"mode": map[string]any{"value": "trap"}}, 0)
	for id := uint64(1); id <= 5; id++ {
		expiry.execute(t, map[string]any{"deliver": map[string]any{"task_id": id, "wait_sequence": 1}}, 0)
	}
	for height := int64(14); height <= 16; height++ {
		expiry.block(height)
	}
	for id := uint64(1); id <= 5; id++ {
		require.Equal(t, "paused", expiry.task(t, id).Status)
	}
	for height := int64(100); height <= 102; height++ {
		before := number(expiry.k.store(expiry.ctx), "active")
		gas := expiry.block(height)
		after := number(expiry.k.store(expiry.ctx), "active")
		require.LessOrEqual(t, before-after, uint64(2))
		require.LessOrEqual(t, gas, expiry.k.Config.AsyncGas)
		t.Logf("expiry height=%d expired=%d remaining=%d gas=%d", height, before-after, after, gas)
	}
	require.Zero(t, number(expiry.k.store(expiry.ctx), "active"))
	require.Zero(t, number(expiry.k.store(expiry.ctx), "bytes"))
}

func TestAcceptanceDeferredTaskFitsFreshBlock(t *testing.T) {
	h := newFaultHarness(t)
	h.k.Config.AsyncGas = 120_000_000
	h.k.Config.MaxPollGas = 80_000_000
	h.start(t, 80_000_000, 100)
	h.start(t, 80_000_000, 100)
	h.block(11)
	require.Equal(t, "waiting", h.task(t, 1).Status)
	require.Equal(t, "runnable", h.task(t, 2).Status)
	require.False(t, h.task(t, 2).Started)
	h.block(12)
	require.Equal(t, "waiting", h.task(t, 2).Status)
	require.Equal(t, uint64(12), h.task(t, 2).LastPoll)
}
