//go:build upstream

package async_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"cosmossdk.io/log"
	"cosmossdk.io/store/prefix"
	"cosmossdk.io/store/snapshots"
	snapshottypes "cosmossdk.io/store/snapshots/types"
	async "cosmwasm-durable-async/integration/wasmd"
	wasmapp "github.com/CosmWasm/wasmd/app"
	wasmkeeper "github.com/CosmWasm/wasmd/x/wasm/keeper"
	wasmtypes "github.com/CosmWasm/wasmd/x/wasm/types"
	abci "github.com/cometbft/cometbft/abci/types"
	cfg "github.com/cometbft/cometbft/config"
	cmtlog "github.com/cometbft/cometbft/libs/log"
	"github.com/cometbft/cometbft/node"
	"github.com/cometbft/cometbft/p2p"
	"github.com/cometbft/cometbft/privval"
	cmtproto "github.com/cometbft/cometbft/proto/tendermint/types"
	"github.com/cometbft/cometbft/proxy"
	rpcclient "github.com/cometbft/cometbft/rpc/client"
	coretypes "github.com/cometbft/cometbft/rpc/core/types"
	cmttypes "github.com/cometbft/cometbft/types"
	dbm "github.com/cosmos/cosmos-db"
	"github.com/cosmos/cosmos-sdk/baseapp"
	"github.com/cosmos/cosmos-sdk/client/flags"
	"github.com/cosmos/cosmos-sdk/server"
	sims "github.com/cosmos/cosmos-sdk/testutil/sims"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/stretchr/testify/require"
)

func TestAuctionStateSync(t *testing.T) {
	if os.Getenv("NATIVE_AUCTION_NETWORK_TEST") != "1" {
		t.Skip("set NATIVE_AUCTION_NETWORK_TEST=1")
	}
	// Restoring the wrong snapshot, replaying genesis, or losing permanent claim
	// records must fail selected-height, exact-state and eventual-bank assertions.
	for _, phase := range []string{"allocate_base", "finalized"} {
		t.Run(phase, func(t *testing.T) {
			h := newAuctionNetwork(t, 3)
			h.setupAuction()
			h.reach(phase)
			before := h.capture(0, 0)
			h.joinFromSnapshot(before)
			h.settle()
			h.pruneAndClaim()
		})
	}
	t.Run("invalid_wait_snapshot_extension", testAuctionInvalidWaitSnapshot)
}

func joinAuctionStrings(values []string) string { return strings.Join(values, ",") }
func auctionQueryOptions(height int64) rpcclient.ABCIQueryOptions {
	return rpcclient.ABCIQueryOptions{Height: height}
}
func auctionContractAddress() sdk.AccAddress { return wasmkeeper.BuildContractAddressClassic(1, 1) }

// auctionABCI reuses the DCA mutex, pre-FinalizeBlock gates, exact durable
// committed header, historical smart queries, and owned-child shutdown behavior.
// No DCA order/fixture is used. Snapshot serving is pinned to one selected height.
type auctionABCI struct{ *demoABCI }

func (a *auctionABCI) Commit(ctx context.Context, r *abci.RequestCommit) (*abci.ResponseCommit, error) {
	v, err := a.demoABCI.Commit(ctx, r)
	if err != nil {
		return v, err
	}
	a.mu.Lock()
	defer a.mu.Unlock()
	err = writeDemoMarker(filepath.Join(a.home, fmt.Sprintf("auction-header-%d.json", a.app.LastBlockHeight())), demoCommittedHeader{Header: a.pendingHeader, AppHash: fmt.Sprintf("%X", a.app.LastCommitID().Hash)})
	return v, err
}
func (a *auctionABCI) capture(request auctionCheckpoint) (auctionCapture, error) {
	out := auctionCapture{Storage: map[string]string{}, Balances: map[string]sdk.Coins{}}
	state, err := a.app.CreateQueryContext(request.Height, false)
	if err != nil {
		return out, err
	}
	out.Height = state.BlockHeight()
	var header demoCommittedHeader
	err = readDemoMarker(filepath.Join(a.home, fmt.Sprintf("auction-header-%d.json", out.Height)), &header)
	if err != nil {
		return out, err
	}
	if header.Header.Height != out.Height || header.Header.Time.IsZero() || header.Header.ChainID != acceptanceChain {
		return out, fmt.Errorf("auction committed header mismatch")
	}
	state = state.WithBlockHeader(header.Header)
	out.AppHash = header.AppHash
	if request.Height == 0 || request.Height == a.app.LastBlockHeight() {
		if out.AppHash != fmt.Sprintf("%X", a.app.LastCommitID().Hash) {
			return out, fmt.Errorf("auction committed root mismatch")
		}
	}
	if err := a.app.AsyncKeeper.ValidateState(state); err != nil {
		return out, err
	}
	contract, err := sdk.AccAddressFromBech32(request.Contract)
	if err != nil {
		return out, err
	}
	smart := func(msg any, v any) error {
		raw, e := json.Marshal(msg)
		if e != nil {
			return e
		}
		answer, e := a.app.WasmKeeper.QuerySmart(state, contract, raw)
		if e != nil {
			return e
		}
		return json.Unmarshal(answer, v)
	}
	if err = smart(map[string]any{"auction": map[string]any{}}, &out.State.View); err != nil {
		return out, err
	}
	if err = smart(map[string]any{"bids": map[string]any{"limit": 32}}, &out.State.Bids); err != nil {
		return out, err
	}
	if out.State.View.Host.Present != nil {
		out.State.Task = out.State.View.Host.Present
		out.State.Funding = &auctionFunding{}
		err = smart(map[string]any{"async": map[string]any{"task_funding": map[string]any{"task_id": out.State.View.Auction.TaskID}}}, out.State.Funding)
		if err != nil {
			return out, err
		}
		if out.State.Funding.Reserved != "0" {
			return out, fmt.Errorf("committed auction reservation nonzero")
		}
	}
	a.app.WasmKeeper.IterateContractState(state, contract, func(k, v []byte) bool { out.Storage[hex.EncodeToString(k)] = hex.EncodeToString(v); return false })
	for _, address := range request.Addresses {
		key, e := sdk.AccAddressFromBech32(address)
		if e != nil {
			return out, e
		}
		out.Balances[address] = a.app.BankKeeper.GetAllBalances(state, key)
	}
	return out, nil
}
func (a *auctionABCI) Query(ctx context.Context, r *abci.RequestQuery) (*abci.ResponseQuery, error) {
	if r.Path == "/native-dca-demo/auction-snapshots" {
		a.mu.Lock()
		defer a.mu.Unlock()
		var target int64
		if readDemoMarker(filepath.Join(a.home, "auction-snapshot-target.json"), &target) == nil && a.app.LastBlockHeight() >= target {
			all, err := a.app.SnapshotManager().List()
			if err != nil {
				return nil, err
			}
			if len(all) == 0 {
				if _, err := a.app.SnapshotManager().Create(uint64(target)); err != nil {
					return nil, err
				}
			}
		}
		list, err := a.ListSnapshots(ctx, &abci.RequestListSnapshots{})
		if err != nil {
			return nil, err
		}
		raw, err := json.Marshal(list.Snapshots)
		return &abci.ResponseQuery{Value: raw}, err
	}
	if r.Path != "/native-dca-demo/auction-capture" {
		return a.demoABCI.Query(ctx, r)
	}
	a.mu.Lock()
	defer a.mu.Unlock()
	var request auctionCheckpoint
	err := json.Unmarshal(r.Data, &request)
	request.Height = r.Height
	var value auctionCapture
	if err == nil {
		value, err = a.capture(request)
	}
	if err != nil {
		return &abci.ResponseQuery{Code: 1, Log: err.Error()}, nil
	}
	raw, err := json.Marshal(value)
	return &abci.ResponseQuery{Height: value.Height, Value: raw}, err
}
func (a *auctionABCI) ListSnapshots(ctx context.Context, r *abci.RequestListSnapshots) (*abci.ResponseListSnapshots, error) {
	all, err := a.Application.ListSnapshots(ctx, r)
	if err != nil {
		return all, err
	}
	var target int64
	if err = readDemoMarker(filepath.Join(a.home, "auction-snapshot-target.json"), &target); err != nil {
		return &abci.ResponseListSnapshots{}, nil
	}
	selected := []*abci.Snapshot{}
	for _, s := range all.Snapshots {
		if int64(s.Height) == target {
			selected = append(selected, s)
		}
	}
	return &abci.ResponseListSnapshots{Snapshots: selected}, nil
}
func (a *auctionABCI) ApplySnapshotChunk(ctx context.Context, r *abci.RequestApplySnapshotChunk) (*abci.ResponseApplySnapshotChunk, error) {
	a.mu.Lock()
	defer a.mu.Unlock()
	v, err := a.Application.ApplySnapshotChunk(ctx, r)
	if err != nil || v.Result != abci.ResponseApplySnapshotChunk_ACCEPT || a.app.LastBlockHeight() == 0 {
		return v, err
	}
	// An ACCEPT can also mean intermediate chunk. Activation must have finished.
	if err = a.app.AsyncSnapshotter.CheckReady(); err != nil {
		return v, nil
	}
	var request auctionCheckpoint
	if err = readDemoMarker(filepath.Join(a.home, "auction-checkpoint.json"), &request); err != nil {
		return nil, err
	}
	if request.Height != a.app.LastBlockHeight() {
		return nil, fmt.Errorf("restored unintended snapshot height")
	}
	value, err := a.capture(request)
	if err != nil {
		return nil, err
	}
	err = writeDemoMarker(filepath.Join(a.home, "auction-snapshot-restored.json"), auctionStartup{PID: os.Getpid(), State: value})
	return v, err
}
func runAuctionNode(t *testing.T) {
	home := os.Getenv("NATIVE_AUCTION_NETWORK_CHILD")
	ctx, cancel := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT)
	defer cancel()
	var spec auctionNodeSpec
	require.NoError(t, readDemoMarker(filepath.Join(home, "auction-node.json"), &spec))
	for _, endpoint := range append([]string{"http://" + spec.RPC, "http://" + spec.P2P}, strings.Split(spec.RPCServers, ",")...) {
		require.NoError(t, validateDemoRPC(endpoint))
	}
	c := cfg.DefaultConfig().SetRoot(home)
	c.RPC.ListenAddress = "tcp://" + spec.RPC
	c.P2P.ListenAddress = "tcp://" + spec.P2P
	c.P2P.PersistentPeers = spec.Peers
	c.P2P.AllowDuplicateIP = true
	c.P2P.PexReactor, c.P2P.AddrBookStrict = false, false
	// Match the existing generic state-sync network's local pacing so an
	// unrestricted provider does not outrun consensus catchup in this fixture.
	c.Consensus.TimeoutCommit = 500 * time.Millisecond
	c.Consensus.TimeoutPropose = 300 * time.Millisecond
	if spec.TrustHeight > 0 {
		c.StateSync.Enable = true
		c.StateSync.TrustHeight = spec.TrustHeight
		c.StateSync.TrustHash = spec.TrustHash
		c.StateSync.RPCServers = strings.Split(spec.RPCServers, ",")
		c.StateSync.DiscoveryTime = 1 * time.Second
		c.StateSync.TrustPeriod = time.Hour
	}
	db, err := dbm.NewDB("application", dbm.GoLevelDBBackend, home)
	require.NoError(t, err)
	snapshotDir := filepath.Join(home, "data", "snapshots")
	require.NoError(t, os.MkdirAll(snapshotDir, 0700))
	metadata, err := dbm.NewDB("metadata", dbm.GoLevelDBBackend, snapshotDir)
	require.NoError(t, err)
	defer metadata.Close()
	store, err := snapshots.NewStore(metadata, snapshotDir)
	require.NoError(t, err)
	var cleanupVM func()
	opts := []wasmkeeper.Option{wasmkeeper.WithWasmEngineDecorator(func(engine wasmtypes.WasmEngine) wasmtypes.WasmEngine {
		cleanupVM = engine.(interface{ Cleanup() }).Cleanup
		return engine
	})}
	app := wasmapp.NewWasmApp(log.NewNopLogger(), db, nil, true, sims.AppOptionsMap{flags.FlagHome: home}, opts, baseapp.SetChainID(acceptanceChain), baseapp.SetSnapshot(store, snapshottypes.NewSnapshotOptions(0, 20)))
	defer func() { cleanupVM(); require.NoError(t, app.Close()) }()
	wrapper := &auctionABCI{&demoABCI{Application: server.NewCometABCIWrapper(app), app: app, home: home, shutdown: ctx.Done()}}
	var checkpoint auctionCheckpoint
	if readDemoMarker(filepath.Join(home, "auction-checkpoint.json"), &checkpoint) == nil && app.LastBlockHeight() > 0 {
		value, err := wrapper.capture(checkpoint)
		require.NoError(t, err)
		require.Equal(t, checkpoint.Height, value.Height)
		require.NoError(t, writeDemoMarker(filepath.Join(home, "auction-startup.json"), auctionStartup{PID: os.Getpid(), State: value}))
	}
	pv := privval.LoadOrGenFilePV(c.PrivValidatorKeyFile(), c.PrivValidatorStateFile())
	nk, err := p2p.LoadNodeKey(c.NodeKeyFile())
	require.NoError(t, err)
	n, err := node.NewNode(c, pv, nk, proxy.NewConnSyncLocalClientCreator(wrapper), node.DefaultGenesisDocProviderFunc(c), cfg.DefaultDBProvider, node.DefaultMetricsProvider(c.Instrumentation), cmtlog.NewNopLogger())
	require.NoError(t, err)
	require.NoError(t, n.Start())
	<-ctx.Done()
	require.NoError(t, n.Stop())
	n.Wait()
}
func (h *auctionNetwork) joinFromSnapshot(before auctionCapture) {
	t := h.t
	height := before.Height
	// Source and independent peer may only advertise the intended snapshot,
	// even after two header blocks are committed for the trusted light client.
	for i := 0; i < 2; i++ {
		require.NoError(t, writeDemoMarker(filepath.Join(h.nodes[i].home, "auction-snapshot-target.json"), height))
	}
	selected := make([][]*abci.Snapshot, 2)
	// A provider's ABCI gate holds Comet cs.mtx and can block gossip and P2P
	// receives. Keep providers advancing through restore and joiner progress.
	h.allow(h.height() + 1000)
	h.wait("selected snapshots on both independent sources", func() bool {
		for i := 0; i < 2; i++ {
			var header demoCommittedHeader
			if readDemoMarker(filepath.Join(h.nodes[i].home, fmt.Sprintf("auction-header-%d.json", height)), &header) != nil {
				return false
			}
			// The test-only RPC route exposes the same filtered ListSnapshots.
			if h.query(i, 0, "auction-snapshots", demoQuery{}, &selected[i]) != nil || len(selected[i]) != 1 || selected[i][0].Height != uint64(height) {
				return false
			}
		}
		return true
	})
	require.Equal(t, selected[0], selected[1], "independent sources exported identical selected snapshot")
	identity := selected[0][0]
	metadataHash := sha256.Sum256(identity.Metadata)
	t.Logf("snapshot identity height=%d format=%d chunks=%d snapshot_hash=%X metadata_sha256=%x", identity.Height, identity.Format, identity.Chunks, identity.Hash, metadataHash)
	// Comet stateprovider queries H/H+1/H+2. Readiness means both sources
	// actually serve each signed header and its matching validator set.
	var signed [2][3]*coretypes.ResultCommit
	h.wait("both providers serve authenticated H through H+2", func() bool {
		for i := 0; i < 2; i++ {
			for j := 0; j < 3; j++ {
				at := height + int64(j)
				ctx, cancel := context.WithTimeout(h.ctx, time.Second)
				commit, err := h.nodes[i].client.Commit(ctx, &at)
				cancel()
				if err != nil {
					return false
				}
				ctx, cancel = context.WithTimeout(h.ctx, time.Second)
				validators, err := h.nodes[i].client.Validators(ctx, &at, nil, nil)
				cancel()
				if err != nil {
					return false
				}
				require.Equal(t, 1, validators.Total)
				require.Len(t, validators.Validators, 1)
				set := cmttypes.NewValidatorSet(validators.Validators)
				require.Equal(t, set.Hash(), []byte(commit.Header.ValidatorsHash))
				require.NoError(t, commit.SignedHeader.ValidateBasic(acceptanceChain))
				require.NoError(t, set.VerifyCommitLight(acceptanceChain, commit.Commit.BlockID, at, commit.Commit))
				signed[i][j] = commit
			}
		}
		return true
	})
	for j := 0; j < 3; j++ {
		require.Equal(t, signed[0][j].SignedHeader, signed[1][j].SignedHeader, "independent trusted RPC header H+%d", j)
	}
	h.logProgress("providers authenticated through H+2", height)
	trusted := signed[0][0]
	rootBlock := signed[0][1]
	require.Equal(t, before.AppHash, fmt.Sprintf("%X", rootBlock.Header.AppHash))
	joiner := h.nodes[2]
	for _, path := range []string{"application.db", "wasm"} {
		_, err := os.Stat(filepath.Join(joiner.home, path))
		require.True(t, os.IsNotExist(err), "joiner empty: %s", path)
	}
	joiner.spec.TrustHeight = height
	joiner.spec.TrustHash = hex.EncodeToString(trusted.Header.Hash())
	checkpoint := auctionCheckpoint{height, h.contract.String(), h.addresses}
	require.NoError(t, writeDemoMarker(filepath.Join(joiner.home, "auction-checkpoint.json"), checkpoint))
	var header demoCommittedHeader
	require.NoError(t, readDemoMarker(filepath.Join(h.nodes[0].home, fmt.Sprintf("auction-header-%d.json", height)), &header))
	require.Equal(t, trusted.Header.Time, header.Header.Time)
	require.Equal(t, trusted.Header.ChainID, header.Header.ChainID)
	require.Equal(t, trusted.Header.Height, header.Header.Height)
	require.Equal(t, before.AppHash, header.AppHash)
	// Query metadata only, bound to the trusted RPC root; no source DB, Wasm
	// cache, or Comet data is copied into the empty joiner.
	header = demoCommittedHeader{Header: *trusted.Header.ToProto(), AppHash: before.AppHash}
	var metadataCopy []byte
	for _, name := range []string{fmt.Sprintf("auction-header-%d.json", height), "committed-header.json"} {
		path := filepath.Join(joiner.home, name)
		require.NoError(t, writeDemoMarker(path, header))
		raw, err := os.ReadFile(path)
		require.NoError(t, err)
		if metadataCopy == nil {
			metadataCopy = raw
		} else {
			require.Equal(t, metadataCopy, raw, "identical authenticated query metadata copies")
		}
	}
	require.NoError(t, writeDemoMarker(filepath.Join(joiner.home, "allow-height.json"), height))
	h.startNode(2)
	var restored auctionStartup
	h.wait("actual selected snapshot activation", func() bool {
		return readDemoMarker(filepath.Join(joiner.home, "auction-snapshot-restored.json"), &restored) == nil && restored.PID == joiner.child.command.Process.Pid
	})
	require.Equal(t, before, restored.State, "state before any restored next block")
	h.wait("joiner acknowledged gate before first restored next block", func() bool {
		var entered int64
		return readDemoMarker(filepath.Join(joiner.home, "gate-entered.json"), &entered) == nil && entered == height+1 && h.heightOK(2, height)
	})
	h.logProgress("exact snapshot restored and joiner gate acknowledged", height)
	_, err := os.Stat(filepath.Join(joiner.home, ".native-async-restore.pending"))
	require.True(t, os.IsNotExist(err), "normal activation cleared its own pending marker")
	require.NoError(t, writeDemoMarker(filepath.Join(joiner.home, "allow-height.json"), int64(1<<60)))
	h.wait("joiner serves fixed post-snapshot H+2", func() bool {
		var info demoInfo
		return h.query(2, 0, "info", demoQuery{}, &info) == nil && info.Height >= height+2
	})
	h.closeGate()
	ctx, cancel := context.WithTimeout(h.ctx, time.Second)
	status, err := joiner.client.Status(ctx)
	cancel()
	require.NoError(t, err)
	require.Greater(t, status.SyncInfo.EarliestBlockHeight, int64(1))
	for i := 0; i < 3; i++ {
		require.Equal(t, before, h.capture(i, height), "historical snapshot state")
	}
	t.Logf("snapshot phase=%s H=%d hash=%X trust_height=%d trust_hash=%X joiner_pid=%d earliest=%d", before.State.View.Auction.Settlement.Phase, height, rootBlock.Header.AppHash, height, trusted.Header.Hash(), restored.PID, status.SyncInfo.EarliestBlockHeight)
}

// A deliberately invalid separate source home authenticates a v1 wait with
// due==expiry. Only this test's raw exporter bypasses outbound validation; the
// actual receiving Snapshotter must reject its genuine multistore/Wasm stream.
func testAuctionInvalidWaitSnapshot(t *testing.T) {
	source := newSnapshotApp(t, true)
	source.block(t, source.execute(t, `{"start":{"gas_per_poll":80000000,"expires_at":100}}`, 1_000_000))
	source.block(t)
	cms := source.app.CommitMultiStore()
	s := prefix.NewStore(cms.GetKVStore(source.app.AsyncKeeper.Key), []byte("native-async/"))
	it := s.Iterator([]byte("task/"), []byte("task0"))
	require.True(t, it.Valid())
	key := append([]byte(nil), it.Key()...)
	var task async.Task
	require.NoError(t, json.Unmarshal(it.Value(), &task))
	it.Close()
	require.NotNil(t, task.Wait.Operation)
	s.Delete(task.TimerKey)
	task.Wait.Operation.Deadline = task.ExpiresAt
	task.ReadyAt = task.ExpiresAt
	task.TimerKey = []byte(fmt.Sprintf("timer/%020d/%020d/%s", task.ReadyAt, task.ReadySequence, task.Owner))
	invalid, err := json.Marshal(task)
	require.NoError(t, err)
	s.Set(key, invalid)
	s.Set(task.TimerKey, key)
	commit := cms.Commit()
	require.ErrorContains(t, source.app.AsyncKeeper.ValidateState(source.app.NewUncachedContext(false, cmtproto.Header{Height: commit.Version})), "wait outside lease")
	db := dbm.NewMemDB()
	defer db.Close()
	store, err := snapshots.NewStore(db, t.TempDir())
	require.NoError(t, err)
	extension := wasmkeeper.NewWasmSnapshotter(cms, &source.app.WasmKeeper)
	manager := snapshots.NewManager(store, snapshottypes.NewSnapshotOptions(0, 2), cms, nil, log.NewNopLogger())
	require.NoError(t, manager.RegisterExtensions(extension))
	snap, err := manager.Create(uint64(commit.Version))
	require.NoError(t, err)
	metadata, err := snap.ToABCI()
	require.NoError(t, err)
	var diagnostic bytes.Buffer
	target := newSnapshotApp(t, false, log.NewLogger(&diagnostic, log.ColorOption(false)))
	offer, err := target.app.OfferSnapshot(&abci.RequestOfferSnapshot{Snapshot: &metadata, AppHash: commit.Hash})
	require.NoError(t, err)
	require.Equal(t, abci.ResponseOfferSnapshot_ACCEPT, offer.Result)
	var final *abci.ResponseApplySnapshotChunk
	for i := uint32(0); i < snap.Chunks; i++ {
		raw, err := manager.LoadChunk(snap.Height, snap.Format, i)
		require.NoError(t, err)
		final, err = target.app.ApplySnapshotChunk(&abci.RequestApplySnapshotChunk{Index: i, Chunk: raw, Sender: "test-invalid-wait-source"})
		require.NoError(t, err)
	}
	require.Equal(t, abci.ResponseApplySnapshotChunk_ABORT, final.Result)
	require.Contains(t, diagnostic.String(), "wait outside lease", "the wait policy, not a transport/hash error, rejected activation")
	restored := prefix.NewStore(target.app.CommitMultiStore().GetKVStore(target.app.AsyncKeeper.Key), []byte("native-async/"))
	require.Equal(t, invalid, restored.Get(key), "offending wait remains intact")
	require.Equal(t, key, restored.Get(task.TimerKey), "offending timer was not cleaned up")
	marker := filepath.Join(target.home, ".native-async-restore.pending")
	retained, err := os.ReadFile(marker)
	require.NoError(t, err)
	require.NotEmpty(t, retained)
	snapshotExecutionBlocked(t, target, commit.Version+1)
	after, err := os.ReadFile(marker)
	require.NoError(t, err)
	require.Equal(t, retained, after, "rejected activation marker retained through restart")
	t.Logf("invalid wait authenticated snapshot H=%d hash=%X rejected; task/timer/activation marker retained", snap.Height, commit.Hash)
}
