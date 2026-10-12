//go:build upstream

package async_test

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sync"
	"testing"

	"cosmossdk.io/log"
	"cosmossdk.io/store/snapshots"
	snapshottypes "cosmossdk.io/store/snapshots/types"
	async "cosmwasm-durable-async/integration/wasmd"
	wasmapp "github.com/CosmWasm/wasmd/app"
	"github.com/CosmWasm/wasmd/x/wasm/ioutils"
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
)

// Each side owns a different empty database, snapshot store and Wasm directory.
// No file from the source home is copied or reused by the receiver.
func newSnapshotApp(t *testing.T, genesis bool) *appHarness {
	t.Helper()
	home := t.TempDir()
	entries, err := os.ReadDir(home)
	require.NoError(t, err)
	require.Empty(t, entries)
	h := openSnapshotHome(t, home)
	if !genesis {
		require.Zero(t, h.app.LastBlockHeight())
		return h
	}
	initializeSnapshotApp(t, h)
	return h
}

func openSnapshotHome(t *testing.T, home string) *appHarness {
	t.Helper()
	h := &appHarness{home: home, key: secp256k1.GenPrivKeyFromSecret([]byte("native deterministic signer"))}
	db, err := dbm.NewDB("application", dbm.GoLevelDBBackend, h.home)
	require.NoError(t, err)
	// Covers constructor panics too: the startup guard may intentionally reject.
	t.Cleanup(func() { _ = db.Close() })
	snapshotDB := dbm.NewMemDB()
	t.Cleanup(func() { require.NoError(t, snapshotDB.Close()) })
	store, err := snapshots.NewStore(snapshotDB, filepath.Join(h.home, "snapshots"))
	require.NoError(t, err)
	opts := []wasmkeeper.Option{wasmkeeper.WithWasmEngineDecorator(func(engine wasmtypes.WasmEngine) wasmtypes.WasmEngine {
		h.cleanupVM = engine.(interface{ Cleanup() }).Cleanup
		return engine
	})}
	defer func() {
		if p := recover(); p != nil {
			if h.cleanupVM != nil {
				h.cleanupVM()
			}
			_ = db.Close()
			panic(p)
		}
	}()
	h.app = wasmapp.NewWasmApp(log.NewNopLogger(), db, nil, true, sims.AppOptionsMap{flags.FlagHome: h.home}, opts,
		baseapp.SetChainID(acceptanceChain), baseapp.SetSnapshot(store, snapshottypes.NewSnapshotOptions(0, 2)))
	vmCleanup := h.cleanupVM
	var once sync.Once
	h.cleanupVM = func() { once.Do(func() { vmCleanup(); require.NoError(t, h.app.Close()) }) }
	t.Cleanup(h.cleanupVM)
	return h
}

func initializeSnapshotApp(t *testing.T, h *appHarness) {
	t.Helper()
	signer := sdk.AccAddress(h.key.PubKey().Address())
	account := authtypes.NewBaseAccount(signer, h.key.PubKey(), 0, 0)
	validator := cmttypes.NewValidator(cmted.GenPrivKeyFromSecret([]byte("native deterministic validator")).PubKey(), 1)
	state, err := wasmapp.GenesisStateWithValSet(h.app.AppCodec(), h.app.DefaultGenesis(), cmttypes.NewValidatorSet([]*cmttypes.Validator{validator}), []authtypes.GenesisAccount{account}, banktypes.Balance{Address: signer.String(), Coins: sdk.NewCoins(sdk.NewInt64Coin("stake", 100_000_000_000), sdk.NewInt64Coin("ugas", 100_000_000))})
	require.NoError(t, err)
	raw, err := json.Marshal(state)
	require.NoError(t, err)
	params := *sims.DefaultConsensusParams
	block := *params.Block
	block.MaxGas = 400_000_000
	params.Block = &block
	_, err = h.app.InitChain(&abci.RequestInitChain{ChainId: acceptanceChain, Time: acceptanceTime, InitialHeight: 1, ConsensusParams: &params, AppStateBytes: raw})
	require.NoError(t, err)
	code, err := os.ReadFile("../../artifacts/native_fault.wasm")
	require.NoError(t, err)
	h.block(t, h.tx(t, 100_000_000, &wasmtypes.MsgStoreCode{Sender: signer.String(), WASMByteCode: code}))
	result := h.block(t, h.tx(t, 100_000_000, &wasmtypes.MsgInstantiateContract{Sender: signer.String(), Admin: signer.String(), CodeID: 1, Label: "snapshot", Msg: []byte(`{}`)}))
	for _, event := range result.TxResults[0].Events {
		if event.Type == "instantiate" {
			for _, attr := range event.Attributes {
				if attr.Key == "_contract_address" {
					h.owner = sdk.MustAccAddressFromBech32(attr.Value)
				}
			}
		}
	}
	require.NotEmpty(t, h.owner)
}

func snapshotOffer(t *testing.T, source, target *appHarness) *abci.Snapshot {
	t.Helper()
	_, err := source.app.SnapshotManager().Create(uint64(source.app.LastBlockHeight()))
	require.NoError(t, err)
	list, err := source.app.ListSnapshots(&abci.RequestListSnapshots{})
	require.NoError(t, err)
	require.Len(t, list.Snapshots, 1)
	snapshot := list.Snapshots[0]
	response, err := target.app.OfferSnapshot(&abci.RequestOfferSnapshot{Snapshot: snapshot, AppHash: source.app.LastCommitID().Hash})
	require.NoError(t, err)
	require.Equal(t, abci.ResponseOfferSnapshot_ACCEPT, response.Result)
	return snapshot
}

func snapshotChunks(t *testing.T, source, target *appHarness, snapshot *abci.Snapshot) {
	t.Helper()
	for i := uint32(0); i < snapshot.Chunks; i++ {
		chunk, err := source.app.LoadSnapshotChunk(&abci.RequestLoadSnapshotChunk{Height: snapshot.Height, Format: snapshot.Format, Chunk: i})
		require.NoError(t, err)
		require.NotEmpty(t, chunk.Chunk)
		response, err := target.app.ApplySnapshotChunk(&abci.RequestApplySnapshotChunk{Index: i, Chunk: chunk.Chunk, Sender: "source"})
		require.NoError(t, err)
		require.Equal(t, abci.ResponseApplySnapshotChunk_ACCEPT, response.Result)
	}
}

func snapshotEqual(t *testing.T, source, target *appHarness) {
	t.Helper()
	require.Equal(t, source.app.LastBlockHeight(), target.app.LastBlockHeight())
	require.Equal(t, source.app.LastCommitID().Hash, target.app.LastCommitID().Hash)
	ctx := func(h *appHarness) sdk.Context {
		return h.app.NewUncachedContext(false, cmtproto.Header{Height: h.app.LastBlockHeight()})
	}
	taskQuery := json.RawMessage(fmt.Sprintf(`{"task":{"contract":%q,"task_id":1}}`, source.owner.String()))
	taskA, taskErrA := source.app.AsyncKeeper.Query(ctx(source), taskQuery)
	taskB, taskErrB := target.app.AsyncKeeper.Query(ctx(target), taskQuery)
	require.Equal(t, fmt.Sprint(taskErrA), fmt.Sprint(taskErrB))
	require.Equal(t, taskA, taskB, "restore must not poll, consume inbox, or change attempt metadata")
	storage := func(h *appHarness) map[string][]byte {
		out := map[string][]byte{}
		h.app.WasmKeeper.IterateContractState(ctx(h), h.owner, func(k, v []byte) bool { out[string(k)] = bytes.Clone(v); return false })
		return out
	}
	require.Equal(t, storage(source), storage(target), "guest writes must not run during restore")
	for _, name := range []string{async.ModuleName, "fee_collector"} {
		addr := authtypes.NewModuleAddress(name)
		require.Equal(t, source.app.BankKeeper.GetAllBalances(ctx(source), addr), target.app.BankKeeper.GetAllBalances(ctx(target), addr))
	}
	require.Equal(t, source.app.BankKeeper.GetAllBalances(ctx(source), sdk.AccAddress(source.key.PubKey().Address())), target.app.BankKeeper.GetAllBalances(ctx(target), sdk.AccAddress(target.key.PubKey().Address())))
	query := json.RawMessage(fmt.Sprintf(`{"funding":{"task_funding":{"contract":%q,"task_id":1}}}`, source.owner.String()))
	a, ae := source.app.AsyncKeeper.Query(ctx(source), query)
	b, be := target.app.AsyncKeeper.Query(ctx(target), query)
	if taskErrA == nil {
		require.NoError(t, ae)
		require.NoError(t, be)
	}
	require.Equal(t, fmt.Sprint(ae), fmt.Sprint(be))
	require.Equal(t, a, b, "restore must not charge escrow")
}

func snapshotBlock(t *testing.T, source, target *appHarness, successful bool, txs ...[]byte) {
	t.Helper()
	a := source.finalize(t, successful, txs...)
	b := target.finalize(t, successful, txs...)
	require.Equal(t, a.Events, b.Events)
	require.Equal(t, a.TxResults, b.TxResults)
	require.Equal(t, source.blockGas[len(source.blockGas)-1], target.blockGas[len(target.blockGas)-1])
	snapshotEqual(t, source, target)
}

func TestSnapshotFreshNodeContinuation(t *testing.T) {
	for _, scenario := range []string{"runnable", "waiting_pinned", "waiting_deadline", "waiting_expiry", "paused_inbox", "completed"} {
		t.Run(scenario, func(t *testing.T) {
			source := newSnapshotApp(t, true)
			if scenario == "waiting_pinned" {
				require.NoError(t, wasmkeeper.NewGovPermissionKeeper(source.app.WasmKeeper).PinCode(source.app.NewUncachedContext(false, cmtproto.Header{Height: 2}), 1))
			}
			expires := 100
			if scenario == "waiting_expiry" {
				expires = 8
			}
			source.block(t, source.execute(t, fmt.Sprintf(`{"start":{"gas_per_poll":80000000,"expires_at":%d}}`, expires), 1_000_000))
			if scenario != "runnable" {
				source.block(t)
			}
			if scenario == "paused_inbox" {
				source.block(t, source.execute(t, `{"mode":{"value":"trap"}}`, 0))
				source.block(t, source.execute(t, `{"deliver":{"task_id":1,"wait_sequence":1}}`, 0))
				require.Equal(t, "paused", source.task(t).Status.Kind)
				require.NotNil(t, source.task(t).Inbox)
			}
			if scenario == "completed" {
				source.block(t, source.execute(t, `{"deliver":{"task_id":1,"wait_sequence":1}}`, 0))
			}
			target := newSnapshotApp(t, false)
			target.owner = source.owner
			before := source.task(t)
			snapshot := snapshotOffer(t, source, target)
			snapshotChunks(t, source, target, snapshot)
			require.Equal(t, before, target.task(t))
			snapshotEqual(t, source, target)
			// Restart a successfully restored receiver before it has executed any
			// new block. This must retain the same continuation and bank state.
			target.cleanupVM()
			target = openSnapshotHome(t, target.home)
			target.owner = source.owner
			snapshotEqual(t, source, target)
			codeA, err := source.app.WasmKeeper.GetByteCode(source.app.NewUncachedContext(false, cmtproto.Header{}), 1)
			require.NoError(t, err)
			codeB, err := target.app.WasmKeeper.GetByteCode(target.app.NewUncachedContext(false, cmtproto.Header{}), 1)
			require.NoError(t, err)
			require.Equal(t, codeA, codeB)
			switch scenario {
			case "waiting_pinned":
				require.True(t, target.app.WasmKeeper.IsPinnedCode(target.app.NewUncachedContext(false, cmtproto.Header{}), 1))
				snapshotBlock(t, source, target, true, source.execute(t, `{"deliver":{"task_id":1,"wait_sequence":1}}`, 0))
				require.Equal(t, "completed", source.task(t).Status.Kind)
			case "runnable":
				snapshotBlock(t, source, target, true)
				snapshotBlock(t, source, target, true, source.execute(t, `{"deliver":{"task_id":1,"wait_sequence":1}}`, 0))
				require.Equal(t, "completed", source.task(t).Status.Kind)
			case "paused_inbox":
				for source.app.LastBlockHeight() < 55 {
					snapshotBlock(t, source, target, true)
				}
				require.NotNil(t, source.task(t).Inbox, "accepted inbox survives operation deadline while paused")
				snapshotBlock(t, source, target, false, source.execute(t, `{"deliver":{"task_id":1,"wait_sequence":1}}`, 0))
				require.NotZero(t, source.results[len(source.results)-1].TxResults[0].Code)
				snapshotBlock(t, source, target, true, source.execute(t, `{"mode":{"value":"ok"}}`, 0))
				snapshotBlock(t, source, target, true, source.execute(t, `{"resume":{"task_id":1}}`, 0))
				snapshotBlock(t, source, target, true)
				require.Equal(t, "completed", source.task(t).Status.Kind)
			case "waiting_deadline":
				for source.app.LastBlockHeight() < 54 {
					snapshotBlock(t, source, target, true)
				}
				require.Equal(t, "completed", source.task(t).Status.Kind)
			case "waiting_expiry":
				for source.app.LastBlockHeight() < 8 {
					snapshotBlock(t, source, target, true)
				}
				_, err := source.app.AsyncKeeper.Query(source.app.NewUncachedContext(false, cmtproto.Header{Height: 8}), json.RawMessage(fmt.Sprintf(`{"task":{"contract":%q,"task_id":1}}`, source.owner.String())))
				require.ErrorContains(t, err, "unknown task", "zero retention prunes expired task")
			case "completed":
				snapshotBlock(t, source, target, false, source.execute(t, `{"deliver":{"task_id":1,"wait_sequence":1}}`, 0))
				require.NotZero(t, source.results[len(source.results)-1].TxResults[0].Code)
			}
			t.Logf("snapshot height=%d chunks=%d continued height=%d app_hash=%X", snapshot.Height, snapshot.Chunks, source.app.LastBlockHeight(), source.app.LastCommitID().Hash)
		})
	}
}

func TestSnapshotCorruptChunkRefetch(t *testing.T) {
	source := newSnapshotApp(t, true)
	source.block(t, source.execute(t, `{"start":{"gas_per_poll":80000000,"expires_at":100}}`, 1_000_000))
	target := newSnapshotApp(t, false)
	target.owner = source.owner
	snapshot := snapshotOffer(t, source, target)
	bad, err := target.app.ApplySnapshotChunk(&abci.RequestApplySnapshotChunk{Index: 0, Chunk: []byte("corrupt"), Sender: "bad-peer"})
	require.NoError(t, err)
	require.Equal(t, abci.ResponseApplySnapshotChunk_RETRY, bad.Result)
	require.Equal(t, []uint32{0}, bad.RefetchChunks)
	require.Equal(t, []string{"bad-peer"}, bad.RejectSenders)
	require.Zero(t, target.app.LastBlockHeight(), "missing valid chunk cannot activate snapshot")
	snapshotChunks(t, source, target, snapshot)
	snapshotEqual(t, source, target)
	snapshotBlock(t, source, target, true)
}

// Re-encode a malicious snapshot with valid outer chunk hashes and unchanged
// authenticated IAVL state. This exercises code completeness, not merely the
// SDK transport checksum gate.
func TestSnapshotMissingWasmRejected(t *testing.T) {
	for _, mode := range []string{"missing_payload", "missing_extension", "unexpected_code", "corrupt_gzip", "oversize_code", "duplicate_code"} {
		t.Run(mode, func(t *testing.T) {
			source := newSnapshotApp(t, true)
			source.block(t, source.execute(t, `{"start":{"gas_per_poll":80000000,"expires_at":100}}`, 1_000_000))
			snap, err := source.app.SnapshotManager().Create(uint64(source.app.LastBlockHeight()))
			require.NoError(t, err)
			input := make(chan io.ReadCloser, snap.Chunks)
			for i := uint32(0); i < snap.Chunks; i++ {
				chunk, err := source.app.LoadSnapshotChunk(&abci.RequestLoadSnapshotChunk{Height: snap.Height, Format: snap.Format, Chunk: i})
				require.NoError(t, err)
				input <- io.NopCloser(bytes.NewReader(chunk.Chunk))
			}
			close(input)
			reader, err := snapshots.NewStreamReader(input)
			require.NoError(t, err)
			var items []*snapshottypes.SnapshotItem
			payloads := 0
			for {
				item := new(snapshottypes.SnapshotItem)
				err := reader.ReadMsg(item)
				if err == io.EOF {
					break
				}
				require.NoError(t, err)
				if item.GetExtensionPayload() != nil {
					payloads++
					switch mode {
					case "missing_payload", "missing_extension":
						continue
					case "unexpected_code":
						code, err := os.ReadFile("../../artifacts/native_fulfill_example.wasm")
						require.NoError(t, err)
						item.GetExtensionPayload().Payload, err = ioutils.GzipIt(code)
						require.NoError(t, err)
					case "corrupt_gzip":
						item.GetExtensionPayload().Payload = []byte("invalid compressed code")
					case "oversize_code":
						limit := wasmtypes.MaxProposalWasmSize
						if wasmtypes.MaxWasmSize > limit {
							limit = wasmtypes.MaxWasmSize
						}
						item.GetExtensionPayload().Payload, err = ioutils.GzipIt(bytes.Repeat([]byte{0}, int(limit)+1))
						require.NoError(t, err)
					case "duplicate_code":
						items = append(items, item)
					}
				}
				if mode == "missing_extension" && item.GetExtension() != nil {
					continue
				}
				items = append(items, item)
			}
			require.NoError(t, reader.Close())
			require.Positive(t, payloads)
			output := make(chan io.ReadCloser)
			writeErr := make(chan error, 1)
			go func() {
				writer := snapshots.NewStreamWriter(output)
				for _, item := range items {
					if err := writer.WriteMsg(item); err != nil {
						writer.CloseWithError(err)
						writeErr <- err
						return
					}
				}
				writeErr <- writer.Close()
			}()
			db := dbm.NewMemDB()
			defer db.Close()
			store, err := snapshots.NewStore(db, t.TempDir())
			require.NoError(t, err)
			malicious, err := store.Save(snap.Height, snap.Format, output)
			require.NoError(t, err)
			require.NoError(t, <-writeErr)
			abciSnapshot, err := malicious.ToABCI()
			require.NoError(t, err)
			target := newSnapshotApp(t, false)
			offer, err := target.app.OfferSnapshot(&abci.RequestOfferSnapshot{Snapshot: &abciSnapshot, AppHash: source.app.LastCommitID().Hash})
			require.NoError(t, err)
			require.Equal(t, abci.ResponseOfferSnapshot_ACCEPT, offer.Result)
			_, chunks, err := store.Load(malicious.Height, malicious.Format)
			require.NoError(t, err)
			var final *abci.ResponseApplySnapshotChunk
			var i uint32
			for chunk := range chunks {
				raw, err := io.ReadAll(chunk)
				require.NoError(t, err)
				require.NoError(t, chunk.Close())
				final, err = target.app.ApplySnapshotChunk(&abci.RequestApplySnapshotChunk{Index: i, Chunk: raw, Sender: "malicious-peer"})
				require.NoError(t, err)
				i++
			}
			require.NotNil(t, final)
			require.Equal(t, abci.ResponseApplySnapshotChunk_ABORT, final.Result, "missing Wasm must not activate even with valid chunk hashes and authentic task state")
			snapshotExecutionBlocked(t, target, int64(snap.Height)+1)
		})
	}
}

func TestSnapshotTrustedHashMismatchRejected(t *testing.T) {
	source := newSnapshotApp(t, true)
	source.block(t, source.execute(t, `{"start":{"gas_per_poll":80000000,"expires_at":100}}`, 1_000_000))
	snap, err := source.app.SnapshotManager().Create(uint64(source.app.LastBlockHeight()))
	require.NoError(t, err)
	abciSnapshot, err := snap.ToABCI()
	require.NoError(t, err)
	target := newSnapshotApp(t, false)
	offer, err := target.app.OfferSnapshot(&abci.RequestOfferSnapshot{Snapshot: &abciSnapshot, AppHash: bytes.Repeat([]byte{0xff}, 32)})
	require.NoError(t, err)
	require.Equal(t, abci.ResponseOfferSnapshot_ACCEPT, offer.Result)
	var final *abci.ResponseApplySnapshotChunk
	for i := uint32(0); i < snap.Chunks; i++ {
		chunk, err := source.app.LoadSnapshotChunk(&abci.RequestLoadSnapshotChunk{Height: snap.Height, Format: snap.Format, Chunk: i})
		require.NoError(t, err)
		final, err = target.app.ApplySnapshotChunk(&abci.RequestApplySnapshotChunk{Index: i, Chunk: chunk.Chunk, Sender: "source"})
		require.NoError(t, err)
	}
	require.NotNil(t, final)
	require.Equal(t, abci.ResponseApplySnapshotChunk_ABORT, final.Result)
	snapshotExecutionBlocked(t, target, int64(snap.Height)+1)
}

func TestSnapshotConfigurationMismatchRejected(t *testing.T) {
	source := newSnapshotApp(t, true)
	source.block(t, source.execute(t, `{"start":{"gas_per_poll":80000000,"expires_at":100}}`, 1_000_000))
	target := newSnapshotApp(t, false)
	// Model a receiving binary with a different otherwise-valid constructor
	// configuration; the consensus manifest must not be replaced on restore.
	target.app.AsyncKeeper.Config.MaxPollGas--
	require.NoError(t, target.app.AsyncKeeper.Config.Validate())
	snapshot := snapshotOffer(t, source, target)
	var final *abci.ResponseApplySnapshotChunk
	for i := uint32(0); i < snapshot.Chunks; i++ {
		chunk, err := source.app.LoadSnapshotChunk(&abci.RequestLoadSnapshotChunk{Height: snapshot.Height, Format: snapshot.Format, Chunk: i})
		require.NoError(t, err)
		final, err = target.app.ApplySnapshotChunk(&abci.RequestApplySnapshotChunk{Index: i, Chunk: chunk.Chunk, Sender: "source"})
		require.NoError(t, err)
	}
	require.NotNil(t, final)
	require.Equal(t, abci.ResponseApplySnapshotChunk_ABORT, final.Result)
	snapshotExecutionBlocked(t, target, int64(snapshot.Height)+1)
}

func snapshotExecutionBlocked(t *testing.T, target *appHarness, height int64) {
	t.Helper()
	_, err := target.app.FinalizeBlock(&abci.RequestFinalizeBlock{Height: height, Time: acceptanceTime})
	require.ErrorContains(t, err, "snapshot restoration incomplete or rejected")
	target.cleanupVM()
	snapshotRestartBlocked(t, target.home)
}

func TestSnapshotIncompleteRestoreRestartRejected(t *testing.T) {
	source := newSnapshotApp(t, true)
	source.block(t, source.execute(t, `{"start":{"gas_per_poll":80000000,"expires_at":100}}`, 1_000_000))
	target := newSnapshotApp(t, false)
	snapshotOffer(t, source, target)
	// Simulate interruption after accepting an offer but before any chunk.
	target.cleanupVM()
	snapshotRestartBlocked(t, target.home)
}

func snapshotRestartBlocked(t *testing.T, home string) {
	t.Helper()
	var failure any
	func() {
		defer func() { failure = recover() }()
		openSnapshotHome(t, home)
	}()
	require.NotNil(t, failure, "restarting must not bypass failed-restore activation guard")
	require.Contains(t, fmt.Sprint(failure), "incomplete or rejected native async snapshot restoration")
}
