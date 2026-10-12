package async

import (
	"encoding/json"
	"errors"
	"fmt"
	wasmtypes "github.com/CosmWasm/wasmd/x/wasm/types"
	wasmvm "github.com/CosmWasm/wasmvm/v2"
	vmtypes "github.com/CosmWasm/wasmvm/v2/types"
)

// EngineGuard rejects outer sudo messages before wasmd can dispatch them. It
// delegates real Wasm execution and preserves the real VM gas result.
type EngineGuard struct{ wasmtypes.WasmEngine }

func (g EngineGuard) Sudo(c wasmvm.Checksum, e vmtypes.Env, msg []byte, s wasmvm.KVStore, a wasmvm.GoAPI, q wasmvm.Querier, m wasmvm.GasMeter, limit uint64, d vmtypes.UFraction) (*vmtypes.ContractResult, uint64, error) {
	r, used, err := g.WasmEngine.Sudo(c, e, msg, s, a, q, m, limit, d)
	if err != nil {
		var guest vmtypes.GuestError
		var gas vmtypes.OutOfGasError
		if !errors.As(err, &guest) && !errors.As(err, &gas) {
			panic(vmtypes.InfrastructureError{Message: err.Error()})
		}
	} else if r == nil {
		panic(vmtypes.InfrastructureError{Message: "VM returned nil Sudo result without error"})
	}
	var envelope map[string]json.RawMessage
	if json.Unmarshal(msg, &envelope) == nil && envelope["poll_task"] != nil && err == nil && r != nil && r.Ok != nil && len(r.Ok.Messages) > 0 {
		return nil, used, fmt.Errorf("native async polls must encode outbound messages in data")
	}
	return r, used, err
}

// InfrastructureRecovery is installed before BaseApp's catch-all runTx handler.
// Native infrastructure failure must abort block execution, not become a paid tx.
func InfrastructureRecovery(value interface{}) error {
	if _, ok := value.(vmtypes.InfrastructureError); ok {
		panic(value)
	}
	return nil
}
