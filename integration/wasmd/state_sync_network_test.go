//go:build upstream

package async_test

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math/rand"
	"net"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"cosmossdk.io/log"
	"cosmossdk.io/store/snapshots"
	snapshottypes "cosmossdk.io/store/snapshots/types"
	async "cosmwasm-durable-async/integration/wasmd"
	wasmapp "github.com/CosmWasm/wasmd/app"
	wasmkeeper "github.com/CosmWasm/wasmd/x/wasm/keeper"
	wasmtypes "github.com/CosmWasm/wasmd/x/wasm/types"
	abci "github.com/cometbft/cometbft/abci/types"
	cfg "github.com/cometbft/cometbft/config"
	cmted "github.com/cometbft/cometbft/crypto/ed25519"
	cmtlog "github.com/cometbft/cometbft/libs/log"
	"github.com/cometbft/cometbft/node"
	"github.com/cometbft/cometbft/p2p"
	"github.com/cometbft/cometbft/privval"
	cmtproto "github.com/cometbft/cometbft/proto/tendermint/types"
	"github.com/cometbft/cometbft/proxy"
	rpcclient "github.com/cometbft/cometbft/rpc/client"
	rpchttp "github.com/cometbft/cometbft/rpc/client/http"
	cmttypes "github.com/cometbft/cometbft/types"
	dbm "github.com/cosmos/cosmos-db"
	"github.com/cosmos/cosmos-sdk/baseapp"
	"github.com/cosmos/cosmos-sdk/client/flags"
	"github.com/cosmos/cosmos-sdk/crypto/keys/secp256k1"
	"github.com/cosmos/cosmos-sdk/server"
	sims "github.com/cosmos/cosmos-sdk/testutil/sims"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	banktypes "github.com/cosmos/cosmos-sdk/x/bank/types"
	"github.com/stretchr/testify/require"
)

// Explicitly opt in: this starts three independent processes, never public listeners.
// NATIVE_ASYNC_NETWORK_TEST=1 go test -tags upstream -run '^TestStateSyncNetwork$' -count=1 -timeout 5m
func TestStateSyncNetwork(t *testing.T) {
	if os.Getenv("NATIVE_ASYNC_NETWORK_CHILD") != "" {
		runStateSyncNode(t)
		return
	}
	if os.Getenv("NATIVE_ASYNC_NETWORK_TEST") != "1" {
		t.Skip("opt-in separate-process state sync; set NATIVE_ASYNC_NETWORK_TEST=1")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 240*time.Second)
	defer cancel()
	key := secp256k1.GenPrivKeyFromSecret([]byte("native deterministic signer"))
	signer := sdk.AccAddress(key.PubKey().Address())
	template := newAcceptanceApp(t)
	valKey := cmted.GenPrivKeyFromSecret([]byte("network state sync validator"))
	val := cmttypes.NewValidator(valKey.PubKey(), 1)
	genesis, e := wasmapp.GenesisStateWithValSet(template.app.AppCodec(), template.app.DefaultGenesis(), cmttypes.NewValidatorSet([]*cmttypes.Validator{val}), []authtypes.GenesisAccount{authtypes.NewBaseAccount(signer, key.PubKey(), 0, 0)}, banktypes.Balance{Address: signer.String(), Coins: sdk.NewCoins(sdk.NewInt64Coin("stake", 100_000_000_000), sdk.NewInt64Coin("ugas", 100_000_000))})
	require.NoError(t, e)
	raw, e := json.Marshal(genesis)
	require.NoError(t, e)
	params := cmttypes.DefaultConsensusParams()
	params.Block.MaxGas = 400_000_000
	doc := &cmttypes.GenesisDoc{GenesisTime: time.Now().UTC(), ChainID: acceptanceChain, InitialHeight: 1, ConsensusParams: params, Validators: []cmttypes.GenesisValidator{{Address: val.Address, PubKey: val.PubKey, Power: 1}}, AppState: raw}
	homes := []string{t.TempDir(), t.TempDir(), t.TempDir()}
	rpcs := make([]string, 3)
	peers := make([]string, 3)
	for i, home := range homes {
		require.NoError(t, os.MkdirAll(filepath.Join(home, "config"), 0700))
		require.NoError(t, os.MkdirAll(filepath.Join(home, "data"), 0700))
		require.NoError(t, doc.SaveAs(filepath.Join(home, "config", "genesis.json")))
		nk := &p2p.NodeKey{PrivKey: cmted.GenPrivKey()}
		require.NoError(t, nk.SaveAs(filepath.Join(home, "config", "node_key.json")))
		rpc, p2paddr := networkFreeAddress(t), networkFreeAddress(t)
		rpcs[i] = "http://" + rpc
		peers[i] = string(nk.ID()) + "@" + p2paddr
		require.NoError(t, os.WriteFile(filepath.Join(home, "ports"), []byte(rpc+"\n"+p2paddr), 0600))
		pvkey := cmted.GenPrivKey()
		if i == 0 {
			pvkey = valKey
		}
		pv := privval.NewFilePV(pvkey, filepath.Join(home, "config", "priv_validator_key.json"), filepath.Join(home, "data", "priv_validator_state.json"))
		pv.Save()
	}
	clients := make([]*rpchttp.HTTP, 3)
	for i := range clients {
		clients[i], e = rpchttp.New(rpcs[i], "/websocket")
		require.NoError(t, e)
	}
	start := func(i int, trustHeight int64, trustHash string) {
		executable, e := os.Executable()
		require.NoError(t, e)
		logfile, e := os.Create(filepath.Join(homes[i], "node.log"))
		require.NoError(t, e)
		command := exec.CommandContext(ctx, executable, "-test.run=^TestStateSyncNetwork$", "-test.timeout=4m")
		command.Env = append(os.Environ(), "NATIVE_ASYNC_NETWORK_CHILD="+homes[i], "NATIVE_ASYNC_NETWORK_PEERS="+peers[0]+","+peers[1], "NATIVE_ASYNC_NETWORK_RPC="+rpcs[0]+","+rpcs[1], "NATIVE_ASYNC_NETWORK_TRUST_HEIGHT="+strconv.FormatInt(trustHeight, 10), "NATIVE_ASYNC_NETWORK_TRUST_HASH="+trustHash)
		command.Stdout = logfile
		command.Stderr = logfile
		require.NoError(t, command.Start())
		done := make(chan error, 1)
		go func() { done <- command.Wait() }()
		t.Cleanup(func() {
			_ = command.Process.Signal(syscall.SIGTERM)
			select {
			case <-done:
			case <-time.After(5 * time.Second):
				_ = command.Process.Kill()
				<-done
			}
			_ = logfile.Close()
			if t.Failed() {
				b, _ := os.ReadFile(filepath.Join(homes[i], "node.log"))
				t.Logf("node %d log:\n%s", i, b)
			}
		})
	}
	wait := func(what string, f func() bool) {
		t.Helper()
		for ctx.Err() == nil {
			if f() {
				return
			}
			select {
			case <-ctx.Done():
			case <-time.After(200 * time.Millisecond):
			}
		}
		t.Fatalf("timed out: %s", what)
	}
	start(0, 0, "")
	start(1, 0, "")
	wait("source height", func() bool { s, e := clients[0].Status(ctx); return e == nil && s.SyncInfo.LatestBlockHeight >= 3 })
	sequence := uint64(0)
	broadcast := func(msg sdk.Msg) {
		tx, e := sims.GenSignedMockTx(rand.New(rand.NewSource(1)), template.app.TxConfig(), []sdk.Msg{msg}, sdk.Coins{}, 100_000_000, acceptanceChain, []uint64{0}, []uint64{sequence}, key)
		require.NoError(t, e)
		encoded, e := template.app.TxConfig().TxEncoder()(tx)
		require.NoError(t, e)
		result, e := clients[0].BroadcastTxCommit(ctx, encoded)
		require.NoError(t, e)
		require.Zero(t, result.CheckTx.Code, result.CheckTx.Log)
		require.Zero(t, result.TxResult.Code, result.TxResult.Log)
		sequence++
	}
	code, e := os.ReadFile("../../artifacts/native_fault.wasm")
	require.NoError(t, e)
	broadcast(&wasmtypes.MsgStoreCode{Sender: signer.String(), WASMByteCode: code})
	broadcast(&wasmtypes.MsgInstantiateContract{Sender: signer.String(), Admin: signer.String(), CodeID: 1, Label: "state-sync", Msg: []byte(`{}`)})
	owner := wasmkeeper.BuildContractAddressClassic(1, 1)
	execute := func(msg string, funds sdk.Coins) {
		broadcast(&wasmtypes.MsgExecuteContract{Sender: signer.String(), Contract: owner.String(), Msg: []byte(msg), Funds: funds})
	}
	execute(`{"start":{"gas_per_poll":80000000,"expires_at":1000}}`, sdk.NewCoins(sdk.NewInt64Coin("ugas", 10_000_000)))
	task := func(i int) (async.TaskResponse, error) {
		r, e := clients[i].ABCIQuery(ctx, "/native-async-test/task", []byte(`{"task":{"contract":"`+owner.String()+`","task_id":1}}`))
		if e != nil {
			return async.TaskResponse{}, e
		}
		var value async.TaskResponse
		e = json.Unmarshal(r.Response.Value, &value)
		return value, e
	}
	wait("waiting workflow on both snapshot sources", func() bool {
		a, e := task(0)
		b, f := task(1)
		return e == nil && f == nil && a.Status.Kind == "waiting" && b.Status.Kind == "waiting"
	})
	status, e := clients[0].Status(ctx)
	require.NoError(t, e)
	trustHeight := status.SyncInfo.LatestBlockHeight - 1
	trusted, e := clients[0].Block(ctx, &trustHeight)
	require.NoError(t, e)
	// Wait for both source apps to publish a post-registration snapshot.
	wait("snapshots on both peers", func() bool {
		for i := 0; i < 2; i++ {
			r, e := clients[i].ABCIQuery(ctx, "/native-async-test/snapshots", nil)
			if e != nil {
				return false
			}
			var heights []uint64
			if json.Unmarshal(r.Response.Value, &heights) != nil {
				return false
			}
			found := false
			for _, h := range heights {
				found = found || h >= uint64(trustHeight)
			}
			if !found {
				return false
			}
		}
		return true
	})
	_, e = os.Stat(filepath.Join(homes[2], "application.db"))
	require.True(t, os.IsNotExist(e))
	_, e = os.Stat(filepath.Join(homes[2], "wasm"))
	require.True(t, os.IsNotExist(e))
	start(2, trustHeight, hex.EncodeToString(trusted.BlockID.Hash))
	wait("state sync snapshot restoration", func() bool { _, e := os.Stat(filepath.Join(homes[2], "snapshot-restored")); return e == nil })
	wait("joiner waiting continuation", func() bool {
		s, e := clients[2].Status(ctx)
		if e != nil || s.SyncInfo.CatchingUp || s.SyncInfo.EarliestBlockHeight <= 1 {
			return false
		}
		v, e := task(2)
		return e == nil && v.Status.Kind == "waiting"
	})
	// Serving a non-genesis earliest block proves the joiner did not replay from genesis.
	joined, e := clients[2].Status(ctx)
	require.NoError(t, e)
	require.Greater(t, joined.SyncInfo.EarliestBlockHeight, int64(1))
	restored, e := os.ReadFile(filepath.Join(homes[2], "snapshot-restored"))
	require.NoError(t, e)
	restoredHeight, e := strconv.ParseInt(string(restored), 10, 64)
	require.NoError(t, e)
	require.Greater(t, restoredHeight, int64(1))
	funding := func(i int) map[string]any {
		r, e := clients[i].ABCIQuery(ctx, "/native-async-test/task", []byte(`{"task_funding":{"contract":"`+owner.String()+`","task_id":1}}`))
		require.NoError(t, e)
		var value map[string]any
		require.NoError(t, json.Unmarshal(r.Response.Value, &value))
		return value
	}
	// Restoration is not a poll: the waiting continuation and its exact financial
	// accounting match the source before any result is delivered.
	sourceWait, e := task(0)
	require.NoError(t, e)
	joinedWait, e := task(2)
	require.NoError(t, e)
	require.Equal(t, sourceWait, joinedWait)
	require.Equal(t, funding(0), funding(2))
	execute(`{"deliver":{"task_id":1,"wait_sequence":1}}`, sdk.Coins{})
	wait("funded continuation completed on all processes", func() bool {
		for i := 0; i < 3; i++ {
			v, e := task(i)
			if e != nil || v.Status.Kind != "completed" {
				return false
			}
		}
		return true
	})
	finished, e := task(0)
	require.NoError(t, e)
	require.NotEmpty(t, finished.Output)
	settled := funding(0)
	require.Equal(t, "0", settled["available"])
	require.Equal(t, "0", settled["reserved"])
	for i := 1; i < 3; i++ {
		other, e := task(i)
		require.NoError(t, e)
		require.Equal(t, finished, other)
		require.Equal(t, settled, funding(i))
	}
	// Bank balances include the refunded controller account, fee collector and
	// scheduler module; equality supplements the common-height root checks.
	// BeginBlock distributes collected fees. Compare at an explicit common
	// committed height, bound below to the next block header's AppHash.
	current, e := clients[0].Status(ctx)
	require.NoError(t, e)
	target := current.SyncInfo.LatestBlockHeight + 3
	wait("subsequent blocks", func() bool {
		for i := 0; i < 3; i++ {
			s, e := clients[i].Status(ctx)
			if e != nil || s.SyncInfo.LatestBlockHeight < target {
				return false
			}
		}
		return true
	})
	queryOptions := rpcclient.ABCIQueryOptions{Height: target - 1}
	balance, e := clients[0].ABCIQueryWithOptions(ctx, "/native-async-test/balances", []byte(signer), queryOptions)
	require.NoError(t, e)
	for i := 1; i < 3; i++ {
		other, e := clients[i].ABCIQueryWithOptions(ctx, "/native-async-test/balances", []byte(signer), queryOptions)
		require.NoError(t, e)
		require.Equal(t, balance.Response.Value, other.Response.Value)
	}
	for height := target - 2; height <= target; height++ {
		first, e := clients[0].Block(ctx, &height)
		require.NoError(t, e)
		for i := 1; i < 3; i++ {
			other, e := clients[i].Block(ctx, &height)
			require.NoError(t, e)
			require.Equal(t, first.Block.AppHash, other.Block.AppHash)
			require.Equal(t, first.BlockID.Hash, other.BlockID.Hash)
		}
		t.Logf("network committed_state_height=%d header_height=%d app_hash=%X block_hash=%X", height-1, height, first.Block.AppHash, first.BlockID.Hash)
	}
	t.Logf("verified real network snapshot restore: trust_height=%d restored_height=%d joiner_earliest=%d", trustHeight, restoredHeight, joined.SyncInfo.EarliestBlockHeight)
}

func networkFreeAddress(t *testing.T) string {
	t.Helper()
	l, e := net.Listen("tcp", "127.0.0.1:0")
	require.NoError(t, e)
	a := l.Addr().String()
	require.NoError(t, l.Close())
	return a
}

type networkABCI struct {
	abci.Application
	app  *wasmapp.WasmApp
	home string
}

func (a networkABCI) Query(ctx context.Context, r *abci.RequestQuery) (*abci.ResponseQuery, error) {
	switch r.Path {
	case "/native-async-test/balances":
		state, e := a.app.CreateQueryContext(r.Height, false)
		if e != nil {
			return nil, e
		}
		balances := map[string]sdk.Coins{
			"controller": a.app.BankKeeper.GetAllBalances(state, sdk.AccAddress(r.Data)),
			"scheduler":  a.app.BankKeeper.GetAllBalances(state, authtypes.NewModuleAddress(async.ModuleName)),
			"fees":       a.app.BankKeeper.GetAllBalances(state, authtypes.NewModuleAddress(authtypes.FeeCollectorName)),
		}
		value, e := json.Marshal(balances)
		return &abci.ResponseQuery{Value: value}, e
	case "/native-async-test/task":
		value, e := a.app.AsyncKeeper.Query(a.app.NewUncachedContext(false, cmtproto.Header{Height: a.app.LastBlockHeight()}), r.Data)
		if e != nil {
			return nil, e
		}
		return &abci.ResponseQuery{Value: value}, nil
	case "/native-async-test/snapshots":
		s, e := a.app.SnapshotManager().List()
		if e != nil {
			return nil, e
		}
		heights := []uint64{}
		for _, v := range s {
			heights = append(heights, v.Height)
		}
		value, e := json.Marshal(heights)
		return &abci.ResponseQuery{Value: value}, e
	}
	return a.Application.Query(ctx, r)
}
func (a networkABCI) ApplySnapshotChunk(ctx context.Context, r *abci.RequestApplySnapshotChunk) (*abci.ResponseApplySnapshotChunk, error) {
	v, e := a.Application.ApplySnapshotChunk(ctx, r)
	if e == nil && v.Result == abci.ResponseApplySnapshotChunk_ACCEPT && a.app.LastBlockHeight() > 0 {
		e = os.WriteFile(filepath.Join(a.home, "snapshot-restored"), []byte(fmt.Sprint(a.app.LastBlockHeight())), 0600)
	}
	return v, e
}
func runStateSyncNode(t *testing.T) {
	home := os.Getenv("NATIVE_ASYNC_NETWORK_CHILD")
	c := cfg.DefaultConfig().SetRoot(home)
	ports, e := os.ReadFile(filepath.Join(home, "ports"))
	require.NoError(t, e)
	var rpc, p2paddr string
	_, e = fmt.Sscan(string(ports), &rpc, &p2paddr)
	require.NoError(t, e)
	c.RPC.ListenAddress = "tcp://" + rpc
	c.P2P.ListenAddress = "tcp://" + p2paddr
	c.P2P.PersistentPeers = os.Getenv("NATIVE_ASYNC_NETWORK_PEERS")
	c.P2P.AllowDuplicateIP = true
	c.P2P.AddrBookStrict = false
	c.P2P.PexReactor = false
	c.Consensus.TimeoutCommit = 500 * time.Millisecond
	c.Consensus.TimeoutPropose = time.Second
	height, e := strconv.ParseInt(os.Getenv("NATIVE_ASYNC_NETWORK_TRUST_HEIGHT"), 10, 64)
	require.NoError(t, e)
	if height > 0 {
		c.StateSync.Enable = true
		c.StateSync.TrustHeight = height
		c.StateSync.TrustHash = os.Getenv("NATIVE_ASYNC_NETWORK_TRUST_HASH")
		c.StateSync.RPCServers = strings.Split(os.Getenv("NATIVE_ASYNC_NETWORK_RPC"), ",")
		c.StateSync.DiscoveryTime = 5 * time.Second
		c.StateSync.TrustPeriod = time.Hour
	}
	db, e := dbm.NewDB("application", dbm.GoLevelDBBackend, home)
	require.NoError(t, e)
	snapshotDir := filepath.Join(home, "data", "snapshots")
	require.NoError(t, os.MkdirAll(snapshotDir, 0700))
	metadata, e := dbm.NewDB("metadata", dbm.GoLevelDBBackend, snapshotDir)
	require.NoError(t, e)
	store, e := snapshots.NewStore(metadata, snapshotDir)
	require.NoError(t, e)
	app := wasmapp.NewWasmApp(log.NewNopLogger(), db, nil, true, sims.AppOptionsMap{flags.FlagHome: home}, nil, baseapp.SetChainID(acceptanceChain), baseapp.SetSnapshot(store, snapshottypes.NewSnapshotOptions(2, 20)))
	pv := privval.LoadOrGenFilePV(c.PrivValidatorKeyFile(), c.PrivValidatorStateFile())
	nk, e := p2p.LoadNodeKey(c.NodeKeyFile())
	require.NoError(t, e)
	n, e := node.NewNode(c, pv, nk, proxy.NewLocalClientCreator(networkABCI{Application: server.NewCometABCIWrapper(app), app: app, home: home}), node.DefaultGenesisDocProviderFunc(c), cfg.DefaultDBProvider, node.DefaultMetricsProvider(c.Instrumentation), cmtlog.NewFilter(cmtlog.NewTMLogger(cmtlog.NewSyncWriter(os.Stdout)), cmtlog.AllowInfo()))
	require.NoError(t, e)
	require.NoError(t, n.Start())
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, syscall.SIGTERM, syscall.SIGINT)
	defer signal.Stop(signals)
	<-signals
	require.NoError(t, n.Stop())
	n.Wait()
	require.NoError(t, app.Close())
}
