//go:build upstream

package async_test

import (
	wasmkeeper "github.com/CosmWasm/wasmd/x/wasm/keeper"
	wasmtypes "github.com/CosmWasm/wasmd/x/wasm/types"
	wasmvm "github.com/CosmWasm/wasmvm/v2"
	vmtypes "github.com/CosmWasm/wasmvm/v2/types"
	abci "github.com/cometbft/cometbft/abci/types"
	"github.com/stretchr/testify/require"
	"testing"
)

type appBrokenStore struct{ wasmvm.KVStore }

func (appBrokenStore) Get([]byte) []byte { panic("injected ordinary transaction backend failure") }

type appFaultEngine struct {
	wasmtypes.WasmEngine
	active *bool
}

func (g appFaultEngine) Execute(c wasmvm.Checksum, e vmtypes.Env, i vmtypes.MessageInfo, msg []byte, s wasmvm.KVStore, a wasmvm.GoAPI, q wasmvm.Querier, m wasmvm.GasMeter, l uint64, d vmtypes.UFraction) (*vmtypes.ContractResult, uint64, error) {
	if *g.active {
		s = appBrokenStore{s}
	}
	return g.WasmEngine.Execute(c, e, i, msg, s, a, q, m, l, d)
}
func TestAppInfrastructurePanicEscapesRunTx(t *testing.T) {
	active := false
	h := deployedAcceptanceApp(t, wasmkeeper.WithWasmEngineDecorator(func(e wasmtypes.WasmEngine) wasmtypes.WasmEngine { return appFaultEngine{e, &active} }))
	commit := h.app.LastCommitID()
	tx := h.execute(t, `{"mode":{"value":"complete"}}`, 0)
	active = true
	var caught any
	func() {
		defer func() { caught = recover() }()
		_, _ = h.app.FinalizeBlock(&abci.RequestFinalizeBlock{Height: h.app.LastBlockHeight() + 1, Time: acceptanceTime, Txs: [][]byte{tx}})
	}()
	require.IsType(t, vmtypes.InfrastructureError{}, caught, "BaseApp must not turn infrastructure failure into a tx code")
	require.Equal(t, commit, h.app.LastCommitID(), "faulted block was not committed")
}
