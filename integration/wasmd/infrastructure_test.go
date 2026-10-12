package async

import (
	wasmkeeper "github.com/CosmWasm/wasmd/x/wasm/keeper"
	wasmtypes "github.com/CosmWasm/wasmd/x/wasm/types"
	wasmvm "github.com/CosmWasm/wasmvm/v2"
	vmtypes "github.com/CosmWasm/wasmvm/v2/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	"github.com/stretchr/testify/require"
	"testing"
)

// Inject at the real callback storage boundary, not by mocking a VM error.
type brokenStore struct{ wasmvm.KVStore }

func (brokenStore) Get([]byte) []byte { panic("injected disk/backend failure") }

type backendFaultEngine struct {
	wasmtypes.WasmEngine
	method *string
}

func (g backendFaultEngine) store(method string, s wasmvm.KVStore) wasmvm.KVStore {
	if *g.method == method {
		return brokenStore{s}
	}
	return s
}
func (g backendFaultEngine) Sudo(c wasmvm.Checksum, e vmtypes.Env, msg []byte, s wasmvm.KVStore, a wasmvm.GoAPI, q wasmvm.Querier, m wasmvm.GasMeter, l uint64, d vmtypes.UFraction) (*vmtypes.ContractResult, uint64, error) {
	if *g.method == "cache" {
		c = make([]byte, 32)
		for i := range c {
			c[i] = 255
		}
	}
	return g.WasmEngine.Sudo(c, e, msg, g.store("sudo", s), a, q, m, l, d)
}
func (g backendFaultEngine) Execute(c wasmvm.Checksum, e vmtypes.Env, i vmtypes.MessageInfo, msg []byte, s wasmvm.KVStore, a wasmvm.GoAPI, q wasmvm.Querier, m wasmvm.GasMeter, l uint64, d vmtypes.UFraction) (*vmtypes.ContractResult, uint64, error) {
	return g.WasmEngine.Execute(c, e, i, msg, g.store("execute", s), a, q, m, l, d)
}
func (g backendFaultEngine) Query(c wasmvm.Checksum, e vmtypes.Env, msg []byte, s wasmvm.KVStore, a wasmvm.GoAPI, q wasmvm.Querier, m wasmvm.GasMeter, l uint64, d vmtypes.UFraction) (*vmtypes.QueryResult, uint64, error) {
	return g.WasmEngine.Query(c, e, msg, g.store("query", s), a, q, m, l, d)
}
func (g backendFaultEngine) Reply(c wasmvm.Checksum, e vmtypes.Env, r vmtypes.Reply, s wasmvm.KVStore, a wasmvm.GoAPI, q wasmvm.Querier, m wasmvm.GasMeter, l uint64, d vmtypes.UFraction) (*vmtypes.ContractResult, uint64, error) {
	return g.WasmEngine.Reply(c, e, r, g.store("reply", s), a, q, m, l, d)
}
func TestRealVMInfrastructureAbortsWithoutTaskCharge(t *testing.T) {
	for _, method := range []string{"sudo", "execute", "query", "reply", "cache"} {
		t.Run(method, func(t *testing.T) {
			active := ""
			h := newFaultHarness(t, wasmkeeper.WithWasmEngineDecorator(func(e wasmtypes.WasmEngine) wasmtypes.WasmEngine { return backendFaultEngine{e, &active} }))
			h.start(t, 100_000_000, 100)
			h.block(11)
			h.execute(t, map[string]any{"deliver": map[string]any{"task_id": 1, "wait_sequence": 1}}, 0)
			h.execute(t, map[string]any{"mode": map[string]any{"value": "host_" + method}}, 0)
			before := h.task(t, 1)
			collector := authtypes.NewModuleAddress(authtypes.FeeCollectorName)
			balance := h.keepers.BankKeeper.GetBalance(h.ctx, collector, "ugas")
			parent := h.ctx
			child, _ := parent.CacheContext()
			h.ctx = child
			active = method
			var caught any
			func() { defer func() { caught = recover() }(); h.block(12) }()
			require.IsType(t, vmtypes.InfrastructureError{}, caught, "real callback panic must propagate through native FFI")
			active = ""
			h.ctx = parent
			require.Equal(t, before, h.task(t, 1))
			require.Equal(t, balance, h.keepers.BankKeeper.GetBalance(h.ctx, collector, "ugas"))
			raw, err := h.keepers.WasmKeeper.QuerySmart(h.ctx, h.owner, []byte(`{}`))
			require.NoError(t, err)
			require.JSONEq(t, `{"writes":1}`, string(raw))
		})
	}
}

func TestSmartQueryRPCPreservesInfrastructureFailure(t *testing.T) {
	active := ""
	h := newFaultHarness(t, wasmkeeper.WithWasmEngineDecorator(func(e wasmtypes.WasmEngine) wasmtypes.WasmEngine {
		return backendFaultEngine{e, &active}
	}))
	active = "query"
	q := wasmkeeper.NewGrpcQuerier(nil, nil, h.keepers.WasmKeeper, 100_000_000)
	var caught any
	func() {
		defer func() { caught = recover() }()
		_, _ = q.SmartContractState(h.ctx, &wasmtypes.QuerySmartContractStateRequest{Address: h.owner.String(), QueryData: []byte(`{}`)})
	}()
	require.IsType(t, vmtypes.InfrastructureError{}, caught)
}

func TestNativeVMRejectsStartSectionAsGuestValidation(t *testing.T) {
	var engine wasmtypes.WasmEngine
	newFaultHarness(t, wasmkeeper.WithWasmEngineDecorator(func(e wasmtypes.WasmEngine) wasmtypes.WasmEngine { engine = e; return e }))
	// (module (func unreachable) (start 0)), assembled exact Wasm bytes.
	start := []byte{0, 97, 115, 109, 1, 0, 0, 0, 1, 4, 1, 96, 0, 0, 3, 2, 1, 0, 8, 1, 0, 10, 5, 1, 3, 0, 0, 11}
	_, _, err := engine.StoreCode(start, 100_000_000_000)
	var guest vmtypes.GuestError
	require.ErrorAs(t, err, &guest)
	require.Contains(t, err.Error(), "start sections are not supported")
	// State-import/raw-cache compilation must enforce the same restriction.
	_, err = engine.StoreCodeUnchecked(start)
	require.ErrorAs(t, err, &guest)
	require.Contains(t, err.Error(), "start sections are not supported")
}
