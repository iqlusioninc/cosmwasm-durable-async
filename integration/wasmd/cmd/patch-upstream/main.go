// Mechanically applies the small wasmd integration seam to an isolated v0.55.1
// checkout. Exact anchors fail closed on source drift. No SDK fork is needed.
package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

func main() {
	if len(os.Args) != 2 {
		panic("usage: patch-upstream WASMD_CHECKOUT")
	}
	root := os.Args[1]
	patch := func(path, old, new string) {
		p := filepath.Join(root, path)
		b, e := os.ReadFile(p)
		if e != nil {
			panic(e)
		}
		if strings.Count(string(b), old) != 1 {
			panic(fmt.Sprintf("expected one anchor in %s: %s", path, old))
		}
		if e = os.WriteFile(p, []byte(strings.Replace(string(b), old, new, 1)), 0644); e != nil {
			panic(e)
		}
	}
	patch("x/wasm/keeper/keeper.go", "\twasmLimits wasmvmtypes.WasmLimits", "\twasmLimits wasmvmtypes.WasmLimits\n\tmigrationGuard func(sdk.Context, sdk.AccAddress) error")
	patch("x/wasm/keeper/keeper.go", "\tnewCodeInfo := k.GetCodeInfo(ctx, newCodeID)", "\tif k.migrationGuard != nil { if err := k.migrationGuard(sdk.UnwrapSDKContext(ctx), contractAddress); err != nil { return nil, err } }\n\tnewCodeInfo := k.GetCodeInfo(ctx, newCodeID)")
	patch("x/wasm/keeper/options.go", "type optsFn func(*Keeper)", "func WithMigrationGuard(guard func(sdk.Context, sdk.AccAddress) error) Option { return optsFn(func(k *Keeper) { k.migrationGuard = guard }) }\n\ntype optsFn func(*Keeper)")
	patch("app/app.go", "import (", "import (\n\tasync \"cosmwasm-durable-async/integration/wasmd\"")
	patch("app/app.go", "var maccPerms = map[string][]string{", "var maccPerms = map[string][]string{\n\tasync.ModuleName: nil,")
	patch("app/app.go", "\tWasmKeeper          wasmkeeper.Keeper", "\tWasmKeeper          wasmkeeper.Keeper\n\tAsyncKeeper *async.Keeper")
	patch("app/app.go", "\tkeys := storetypes.NewKVStoreKeys(", "\tkeys := storetypes.NewKVStoreKeys(\n\t\tasync.ModuleName,")
	patch("app/app.go", "\tapp.WasmKeeper = wasmkeeper.NewKeeper(", "\tapp.AsyncKeeper = &async.Keeper{Key: keys[async.ModuleName], Bank: app.BankKeeper, Config: async.ExperimentalConfig()}\n\tif err := app.AsyncKeeper.Config.Validate(); err != nil { panic(err) }\n\twasmOpts = append(wasmOpts, app.AsyncKeeper.Options()...)\n\twasmOpts = append(wasmOpts, wasmkeeper.WithMigrationGuard(app.AsyncKeeper.CheckMigration))\n\tapp.WasmKeeper = wasmkeeper.NewKeeper(")
	patch("app/app.go", "\t// Create fee enabled wasm ibc Stack", "\tapp.AsyncKeeper.Wasm = &app.WasmKeeper\n\tproposal := baseapp.NewDefaultProposalHandler(app.Mempool(), app.BaseApp)\n\tprepare, process := app.AsyncKeeper.ProposalHandlers(txConfig.TxDecoder(), proposal.PrepareProposalHandler(), proposal.ProcessProposalHandler())\n\tapp.SetPrepareProposal(prepare)\n\tapp.SetProcessProposal(process)\n\n\t// Create fee enabled wasm ibc Stack")
	patch("app/app.go", "func (app *WasmApp) EndBlocker(ctx sdk.Context) (sdk.EndBlock, error) {\n\treturn app.ModuleManager.EndBlock(ctx)", "func (app *WasmApp) EndBlocker(ctx sdk.Context) (sdk.EndBlock, error) {\n\treturn app.AsyncKeeper.ApplicationEndBlock(ctx, app.ModuleManager.EndBlock)")
	patch("app/app.go", "func (app *WasmApp) InitChainer(ctx sdk.Context, req *abci.RequestInitChain) (*abci.ResponseInitChain, error) {", "func (app *WasmApp) InitChainer(ctx sdk.Context, req *abci.RequestInitChain) (*abci.ResponseInitChain, error) {\n\tif req.ConsensusParams == nil || req.ConsensusParams.Block == nil || req.ConsensusParams.Block.MaxGas <= 0 { return nil, fmt.Errorf(\"native async requires finite genesis block gas\") }; if _, err := app.AsyncKeeper.Config.OrdinaryGas(uint64(req.ConsensusParams.Block.MaxGas)); err != nil { return nil, err }")
}
