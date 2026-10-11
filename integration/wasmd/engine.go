package async

import (
	"encoding/json"
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
	var envelope map[string]json.RawMessage
	if json.Unmarshal(msg, &envelope) == nil && envelope["poll_task"] != nil && err == nil && r != nil && r.Ok != nil && len(r.Ok.Messages) > 0 {
		return nil, used, fmt.Errorf("native async polls must encode outbound messages in data")
	}
	return r, used, err
}
