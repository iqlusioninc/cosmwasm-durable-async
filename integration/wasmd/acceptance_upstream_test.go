//go:build upstream

package async_test

import (
	"bytes"
	"cosmossdk.io/log"
	async "cosmwasm-durable-async/integration/wasmd"
	"encoding/json"
	wasmapp "github.com/CosmWasm/wasmd/app"
	wasmkeeper "github.com/CosmWasm/wasmd/x/wasm/keeper"
	wasmtypes "github.com/CosmWasm/wasmd/x/wasm/types"
	abci "github.com/cometbft/cometbft/abci/types"
	cmted "github.com/cometbft/cometbft/crypto/ed25519"
	cmtproto "github.com/cometbft/cometbft/proto/tendermint/types"
	cmttypes "github.com/cometbft/cometbft/types"
	dbm "github.com/cosmos/cosmos-db"
	"github.com/cosmos/cosmos-sdk/baseapp"
	"github.com/cosmos/cosmos-sdk/client/flags"
	"github.com/cosmos/cosmos-sdk/crypto/keys/secp256k1"
	sims "github.com/cosmos/cosmos-sdk/testutil/sims"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	banktypes "github.com/cosmos/cosmos-sdk/x/bank/types"
	"github.com/stretchr/testify/require"
	"math/rand"
	"os"
	"testing"
	"time"
)

const acceptanceChain = "native-acceptance"

var acceptanceTime = time.Unix(1_700_000_000, 0).UTC()

type appHarness struct {
	app       *wasmapp.WasmApp
	home      string
	key       *secp256k1.PrivKey
	owner     sdk.AccAddress
	records   []*abci.RequestFinalizeBlock
	roots     [][]byte
	cleanupVM func()
	results   []*abci.ResponseFinalizeBlock
	blockGas  []uint64
}

func openAcceptanceApp(t *testing.T, home string, extra ...wasmkeeper.Option) (*wasmapp.WasmApp, func()) {
	t.Helper()
	db, e := dbm.NewDB("application", dbm.GoLevelDBBackend, home)
	require.NoError(t, e)
	var cleanup func()
	opts := []wasmkeeper.Option{wasmkeeper.WithWasmEngineDecorator(func(engine wasmtypes.WasmEngine) wasmtypes.WasmEngine {
		cleanup = engine.(interface{ Cleanup() }).Cleanup
		return engine
	})}
	opts = append(opts, extra...)
	app := wasmapp.NewWasmApp(log.NewNopLogger(), db, nil, true, sims.AppOptionsMap{flags.FlagHome: home}, opts, baseapp.SetChainID(acceptanceChain))
	return app, cleanup
}
func newAcceptanceApp(t *testing.T, extra ...wasmkeeper.Option) *appHarness {
	t.Helper()
	h := &appHarness{home: t.TempDir(), key: secp256k1.GenPrivKeyFromSecret([]byte("native deterministic signer"))}
	h.app, h.cleanupVM = openAcceptanceApp(t, h.home, extra...)
	t.Cleanup(func() { h.cleanupVM(); require.NoError(t, h.app.Close()) })
	signer := sdk.AccAddress(h.key.PubKey().Address())
	account := authtypes.NewBaseAccount(signer, h.key.PubKey(), 0, 0)
	validator := cmttypes.NewValidator(cmted.GenPrivKeyFromSecret([]byte("native deterministic validator")).PubKey(), 1)
	genesis, e := wasmapp.GenesisStateWithValSet(h.app.AppCodec(), h.app.DefaultGenesis(), cmttypes.NewValidatorSet([]*cmttypes.Validator{validator}), []authtypes.GenesisAccount{account}, banktypes.Balance{Address: signer.String(), Coins: sdk.NewCoins(sdk.NewInt64Coin("stake", 100_000_000_000), sdk.NewInt64Coin("ugas", 100_000_000))})
	require.NoError(t, e)
	raw, e := json.Marshal(genesis)
	require.NoError(t, e)
	params := *sims.DefaultConsensusParams
	block := *params.Block
	block.MaxGas = 400_000_000
	params.Block = &block
	_, e = h.app.InitChain(&abci.RequestInitChain{ChainId: acceptanceChain, Time: acceptanceTime, InitialHeight: 1, ConsensusParams: &params, AppStateBytes: raw})
	require.NoError(t, e)
	return h
}
func (h *appHarness) tx(t *testing.T, gas uint64, msg sdk.Msg) []byte {
	t.Helper()
	signer := sdk.AccAddress(h.key.PubKey().Address())
	ctx := h.app.NewUncachedContext(false, cmtproto.Header{})
	account := h.app.AccountKeeper.GetAccount(ctx, signer)
	// Before the first commit genesis lives in the initialized finalize state.
	if account == nil {
		account = h.app.AccountKeeper.GetAccount(h.app.NewContext(false), signer)
	}
	tx, e := sims.GenSignedMockTx(rand.New(rand.NewSource(1)), h.app.TxConfig(), []sdk.Msg{msg}, sdk.Coins{}, gas, acceptanceChain, []uint64{account.GetAccountNumber()}, []uint64{account.GetSequence()}, h.key)
	require.NoError(t, e)
	raw, e := h.app.TxConfig().TxEncoder()(tx)
	require.NoError(t, e)
	return raw
}
func (h *appHarness) block(t *testing.T, txs ...[]byte) *abci.ResponseFinalizeBlock {
	return h.finalize(t, true, txs...)
}
func (h *appHarness) finalize(t *testing.T, requireSuccess bool, txs ...[]byte) *abci.ResponseFinalizeBlock {
	t.Helper()
	height := h.app.LastBlockHeight() + 1
	req := &abci.RequestFinalizeBlock{Height: height, Time: acceptanceTime.Add(time.Duration(height) * time.Second), Txs: txs, Hash: bytes.Repeat([]byte{byte(height)}, 32)}
	result, e := h.app.FinalizeBlock(req)
	require.NoError(t, e)
	if requireSuccess {
		for _, tx := range result.TxResults {
			require.Zero(t, tx.Code, tx.Log)
		}
	}
	consumed := h.app.GetContextForFinalizeBlock(nil).BlockGasMeter().GasConsumed()
	require.LessOrEqual(t, consumed, uint64(400_000_000))
	h.blockGas = append(h.blockGas, consumed)
	_, e = h.app.Commit()
	require.NoError(t, e)
	h.records = append(h.records, req)
	h.roots = append(h.roots, bytes.Clone(h.app.LastCommitID().Hash))
	h.results = append(h.results, result)
	return result
}
func (h *appHarness) task(t *testing.T) async.TaskResponse {
	t.Helper()
	raw, e := h.app.AsyncKeeper.Query(h.app.NewUncachedContext(false, cmtproto.Header{Height: h.app.LastBlockHeight()}), json.RawMessage(`{"task":{"contract":"`+h.owner.String()+`","task_id":1}}`))
	require.NoError(t, e)
	var task async.TaskResponse
	require.NoError(t, json.Unmarshal(raw, &task))
	return task
}
func (h *appHarness) execute(t *testing.T, msg string, funds int64) []byte {
	coins := sdk.Coins{}
	if funds > 0 {
		coins = sdk.NewCoins(sdk.NewInt64Coin("ugas", funds))
	}
	return h.tx(t, 100_000_000, &wasmtypes.MsgExecuteContract{Sender: sdk.AccAddress(h.key.PubKey().Address()).String(), Contract: h.owner.String(), Msg: []byte(msg), Funds: coins})
}

// Every mutation uses a signed transaction and actual ABCI FinalizeBlock/Commit.
// Reopening closes the database and builds fresh keepers/VM against the disk DB.
// Replay independently initializes genesis and reapplies the identical tx bytes.
func deployedAcceptanceApp(t *testing.T, extra ...wasmkeeper.Option) *appHarness {
	h := newAcceptanceApp(t, extra...)
	signer := sdk.AccAddress(h.key.PubKey().Address()).String()
	code, e := os.ReadFile("../../artifacts/native_fault.wasm")
	require.NoError(t, e)
	h.block(t, h.tx(t, 100_000_000, &wasmtypes.MsgStoreCode{Sender: signer, WASMByteCode: code}))
	result := h.block(t, h.tx(t, 100_000_000, &wasmtypes.MsgInstantiateContract{Sender: signer, Admin: signer, CodeID: 1, Label: "acceptance", Msg: []byte(`{}`)}))
	for _, ev := range result.TxResults[0].Events {
		if ev.Type == "instantiate" {
			for _, a := range ev.Attributes {
				if a.Key == "_contract_address" {
					h.owner = sdk.MustAccAddressFromBech32(a.Value)
				}
			}
		}
	}
	require.NotEmpty(t, h.owner)
	return h
}
func TestAcceptanceAppDiskReopenAndReplay(t *testing.T) {
	for _, pinned := range []bool{false, true} {
		name := "unpinned"
		if pinned {
			name = "pinned"
		}
		t.Run(name, func(t *testing.T) { acceptanceReopenReplay(t, pinned) })
	}
}
func acceptanceReopenReplay(t *testing.T, pinned bool) {
	h := deployedAcceptanceApp(t)
	pin := func(app *wasmapp.WasmApp) {
		ctx := app.NewUncachedContext(false, cmtproto.Header{Height: 2})
		require.NoError(t, wasmkeeper.NewGovPermissionKeeper(app.WasmKeeper).PinCode(ctx, 1))
		require.True(t, app.WasmKeeper.IsPinnedCode(ctx, 1))
	}
	// Pinning is identical host setup in both lanes, before the third commit.
	if pinned {
		pin(h.app)
	}
	h.block(t, h.execute(t, `{"start":{"gas_per_poll":80000000,"expires_at":100}}`, 1_000_000))
	require.Nil(t, h.task(t).LastAttempt, "registration must not poll at same height")
	h.block(t)
	before := h.task(t)
	require.Equal(t, "waiting", before.Status.Kind)
	require.NoError(t, h.app.Close())
	h.cleanupVM()
	h.app, h.cleanupVM = openAcceptanceApp(t, h.home)
	require.Equal(t, before, h.task(t), "fresh app and freshly opened DB restore durable wait")
	h.block(t, h.execute(t, `{"deliver":{"task_id":1,"wait_sequence":1}}`, 0))
	require.Equal(t, "completed", h.task(t).Status.Kind)
	replay := newAcceptanceApp(t)
	for i, req := range h.records {
		if pinned && i == 2 {
			pin(replay.app)
		}
		result, e := replay.app.FinalizeBlock(req)
		require.NoError(t, e)
		require.Equal(t, h.results[i].Events, result.Events, "same pin status: reopened VM vs continuous replay VM events and gas")
		require.Equal(t, h.results[i].TxResults, result.TxResults, "same transaction gas/results")
		_, e = replay.app.Commit()
		require.NoError(t, e)
		require.Equal(t, h.roots[i], replay.app.LastCommitID().Hash, "replay commit %d", i+1)
		t.Logf("height=%d replay_root=%X", req.Height, h.roots[i])
	}
}

// Catches bypassing the shared migration seam through a privileged permission
// keeper; compares code and task storage after genuine Migrate invocations.
func TestAcceptanceActualMigrationLifecycle(t *testing.T) {
	h := deployedAcceptanceApp(t)
	signer := sdk.AccAddress(h.key.PubKey().Address())
	code, e := os.ReadFile("../../artifacts/native_fault.wasm")
	require.NoError(t, e)
	h.block(t, h.tx(t, 100_000_000, &wasmtypes.MsgStoreCode{Sender: signer.String(), WASMByteCode: code}))
	h.block(t, h.execute(t, `{"start":{"gas_per_poll":80000000,"expires_at":100}}`, 1_000_000))
	reject := func(status string) {
		t.Helper()
		ctx := h.app.NewUncachedContext(false, cmtproto.Header{Height: h.app.LastBlockHeight()})
		task := h.task(t)
		require.Equal(t, status, task.Status.Kind)
		info := h.app.WasmKeeper.GetContractInfo(ctx, h.owner)
		snapshotStorage := func() map[string][]byte {
			snapshot := map[string][]byte{}
			h.app.WasmKeeper.IterateContractState(ctx, h.owner, func(key, value []byte) bool { snapshot[string(key)] = bytes.Clone(value); return false })
			return snapshot
		}
		storage := snapshotStorage()
		checkStorage := func() {
			require.Equal(t, storage, snapshotStorage())
			afterTask := h.task(t)
			require.Equal(t, task.State, afterTask.State)
			require.Equal(t, task.Inbox, afterTask.Inbox)
		}
		_, e = wasmkeeper.NewDefaultPermissionKeeper(h.app.WasmKeeper).Migrate(ctx, h.owner, signer, 2, []byte(`{}`))
		require.ErrorContains(t, e, "live async tasks")
		require.Equal(t, info, h.app.WasmKeeper.GetContractInfo(ctx, h.owner))
		require.Equal(t, task, h.task(t))
		checkStorage()
		_, e = wasmkeeper.NewGovPermissionKeeper(h.app.WasmKeeper).Migrate(ctx, h.owner, signer, 2, []byte(`{}`))
		require.ErrorContains(t, e, "live async tasks")
		require.Equal(t, info, h.app.WasmKeeper.GetContractInfo(ctx, h.owner))
		require.Equal(t, task, h.task(t))
		checkStorage()
	}
	reject("runnable")
	h.block(t)
	reject("waiting")
	h.block(t, h.execute(t, `{"mode":{"value":"trap"}}`, 0))
	h.block(t, h.execute(t, `{"deliver":{"task_id":1,"wait_sequence":1}}`, 0))
	reject("paused")
	h.block(t, h.execute(t, `{"cancel":{"task_id":1}}`, 0))
	require.Equal(t, "cancelled", h.task(t).Status.Kind)
	h.block(t, h.tx(t, 100_000_000, &wasmtypes.MsgMigrateContract{Sender: signer.String(), Contract: h.owner.String(), CodeID: 2, Msg: []byte(`{}`)}))
	require.Equal(t, uint64(2), h.app.WasmKeeper.GetContractInfo(h.app.NewUncachedContext(false, cmtproto.Header{}), h.owner).CodeID)
}

func TestAcceptanceOrdinaryOutOfGasRetainsAsyncPartition(t *testing.T) {
	h := deployedAcceptanceApp(t)
	h.block(t, h.execute(t, `{"start":{"gas_per_poll":80000000,"expires_at":100}}`, 1_000_000))
	burn := h.tx(t, 180_000_000, &wasmtypes.MsgExecuteContract{Sender: sdk.AccAddress(h.key.PubKey().Address()).String(), Contract: h.owner.String(), Msg: []byte(`{"burn":{}}`), Funds: sdk.Coins{}})
	result := h.finalize(t, false, burn)
	require.Len(t, result.TxResults, 1)
	tx := result.TxResults[0]
	require.NotZero(t, tx.Code)
	require.GreaterOrEqual(t, tx.GasUsed, int64(180_000_000), "ordinary transaction must actually exhaust its partition")
	require.Equal(t, "waiting", h.task(t).Status.Kind, "funded task still polls after ordinary OOG")
	total := h.blockGas[len(h.blockGas)-1]
	require.Greater(t, total, uint64(180_000_000))
	require.LessOrEqual(t, total, uint64(400_000_000))
	t.Logf("ordinary OOG gas_used=%d declared=180000000 final_block_gas=%d", tx.GasUsed, total)
}

// A normal signed FeeTx exactly fills the declared ordinary partition; the
// installed ABCI hook must still execute the funded task and retain MaxGas.
func TestAcceptanceAppProposalAndReservedPoll(t *testing.T) {
	h := deployedAcceptanceApp(t)
	h.block(t, h.execute(t, `{"start":{"gas_per_poll":80000000,"expires_at":100}}`, 1_000_000))
	sender := sdk.AccAddress(h.key.PubKey().Address()).String()
	send := &banktypes.MsgSend{FromAddress: sender, ToAddress: sender, Amount: sdk.NewCoins(sdk.NewInt64Coin("ugas", 1))}
	exact := h.tx(t, 180_000_000, send)
	over := h.tx(t, 180_000_001, send)
	overflow := h.tx(t, ^uint64(0), send)
	for _, tc := range []struct {
		name string
		txs  [][]byte
		want abci.ResponseProcessProposal_ProposalStatus
	}{{"exact", [][]byte{exact}, abci.ResponseProcessProposal_ACCEPT}, {"one_over", [][]byte{over}, abci.ResponseProcessProposal_REJECT}, {"aggregate_overflow", [][]byte{exact, overflow}, abci.ResponseProcessProposal_REJECT}, {"malformed", [][]byte{[]byte("bad tx")}, abci.ResponseProcessProposal_REJECT}} {
		t.Run(tc.name, func(t *testing.T) {
			r, e := h.app.ProcessProposal(&abci.RequestProcessProposal{Height: h.app.LastBlockHeight() + 1, Time: acceptanceTime.Add(4 * time.Second), Txs: tc.txs})
			require.NoError(t, e)
			require.Equal(t, tc.want, r.Status)
		})
	}
	prep, e := h.app.PrepareProposal(&abci.RequestPrepareProposal{Height: h.app.LastBlockHeight() + 1, Time: acceptanceTime.Add(4 * time.Second), MaxTxBytes: 1_000_000, Txs: [][]byte{over, exact}})
	require.NoError(t, e)
	require.Equal(t, [][]byte{exact}, prep.Txs)
	h.block(t, exact)
	require.Equal(t, "waiting", h.task(t).Status.Kind)
	t.Logf("declared_ordinary_gas=180000000 full_max_gas=400000000 actual_final_block_gas=%d", h.blockGas[len(h.blockGas)-1])
	params, e := h.app.ConsensusParamsKeeper.ParamsStore.Get(h.app.NewUncachedContext(false, cmtproto.Header{}))
	require.NoError(t, e)
	require.Equal(t, int64(400_000_000), params.Block.MaxGas)
}
