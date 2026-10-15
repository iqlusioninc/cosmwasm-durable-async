//go:build upstream

package async_test

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/url"
	"os"
	"os/signal"
	"path/filepath"
	"strconv"
	"sync"
	"syscall"
	"testing"
	"time"

	async "cosmwasm-durable-async/integration/wasmd"
	wasmapp "github.com/CosmWasm/wasmd/app"
	abci "github.com/cometbft/cometbft/abci/types"
	cfg "github.com/cometbft/cometbft/config"
	cmtlog "github.com/cometbft/cometbft/libs/log"
	"github.com/cometbft/cometbft/node"
	"github.com/cometbft/cometbft/p2p"
	"github.com/cometbft/cometbft/privval"
	cmtproto "github.com/cometbft/cometbft/proto/tendermint/types"
	"github.com/cometbft/cometbft/proxy"
	"github.com/cosmos/cosmos-sdk/server"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	"github.com/stretchr/testify/require"
)

type demoQuery struct {
	Contract string          `json:"contract"`
	Message  json.RawMessage `json:"message"`
	Address  string          `json:"address"`
}
type demoAccount struct {
	Number   uint64    `json:"number"`
	Sequence uint64    `json:"sequence"`
	Balances sdk.Coins `json:"balances"`
}
type demoInfo struct {
	Height      int64  `json:"height"`
	AppHash     string `json:"app_hash"`
	HostReserve uint64 `json:"host_reserve"`
	Rent        string `json:"rent_per_byte_block"`
	Cleanup     string `json:"cleanup"`
}
type demoFillGate struct {
	Contract string `json:"contract"`
	OrderID  uint64 `json:"order_id"`
	Count    uint32 `json:"filled_count"`
}
type demoStartupEvidence struct {
	PID      int          `json:"owned_child_pid"`
	Snapshot demoSnapshot `json:"snapshot"`
}
type demoCommittedHeader struct {
	Header  cmtproto.Header `json:"header"`
	AppHash string          `json:"committed_app_hash"`
}

func validateDemoRPC(endpoint string) error {
	u, err := url.Parse(endpoint)
	if err != nil || u == nil || u.Scheme != "http" || u.User != nil || u.RawQuery != "" || u.Fragment != "" || (u.Path != "" && u.Path != "/") {
		return errors.New("demo RPC must be plain HTTP at a literal loopback address")
	}
	ip := net.ParseIP(u.Hostname())
	port, portErr := strconv.Atoi(u.Port())
	if ip == nil || !ip.IsLoopback() || portErr != nil || port <= 0 || port > 65535 {
		return errors.New("refusing nonloopback or invalid demo RPC endpoint")
	}
	return nil
}

// Marker changes are durable before a child restart. No shared directories or
// production scheduler state are touched by these test-only control files.
func writeDemoMarker(path string, value any) error {
	raw, err := json.Marshal(value)
	if err != nil {
		return err
	}
	f, err := os.CreateTemp(filepath.Dir(path), ".gate-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if _, err = f.Write(raw); err != nil {
		_ = f.Close()
		return err
	}
	if err = f.Sync(); err != nil {
		_ = f.Close()
		return err
	}
	if err = f.Close(); err != nil {
		return err
	}
	if err = os.Rename(f.Name(), path); err != nil {
		return err
	}
	dir, err := os.Open(filepath.Dir(path))
	if err != nil {
		return err
	}
	defer dir.Close()
	return dir.Sync()
}
func readDemoMarker(path string, value any) error {
	raw, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	return json.Unmarshal(raw, value)
}

// Per-connection Comet clients allow RPC inspection while consensus waits at the
// test gate. This mutex serializes every app operation used by this node. Gate
// waits are outside the mutex, so shutdown and committed queries remain possible.
type demoABCI struct {
	abci.Application
	app           *wasmapp.WasmApp
	home          string
	shutdown      <-chan struct{}
	mu            sync.Mutex
	pendingHeader cmtproto.Header
}

func (a *demoABCI) FinalizeBlock(ctx context.Context, r *abci.RequestFinalizeBlock) (*abci.ResponseFinalizeBlock, error) {
	for {
		var allowed, filled int64
		if err := readDemoMarker(filepath.Join(a.home, "allow-height.json"), &allowed); err != nil {
			return nil, err
		}
		err := readDemoMarker(filepath.Join(a.home, "fill-committed.json"), &filled)
		if err != nil && !os.IsNotExist(err) {
			return nil, err
		}
		_, released := os.Stat(filepath.Join(a.home, "release-fill.json"))
		if r.Height <= allowed && (filled == 0 || r.Height <= filled || released == nil) {
			break
		}
		if err := writeDemoMarker(filepath.Join(a.home, "gate-entered.json"), r.Height); err != nil {
			return nil, err
		}
		select {
		case <-a.shutdown:
			return nil, errors.New("owned demo child stopping at pre-FinalizeBlock gate")
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-time.After(20 * time.Millisecond):
		}
	}
	a.mu.Lock()
	defer a.mu.Unlock()
	a.pendingHeader = cmtproto.Header{Height: r.Height, Time: r.Time, ChainID: acceptanceChain}
	return a.Application.FinalizeBlock(ctx, r)
}
func (a *demoABCI) Commit(ctx context.Context, r *abci.RequestCommit) (*abci.ResponseCommit, error) {
	a.mu.Lock()
	defer a.mu.Unlock()
	answer, err := a.Application.Commit(ctx, r)
	if err != nil {
		return answer, err
	}
	if err := writeDemoMarker(filepath.Join(a.home, "committed-header.json"), demoCommittedHeader{Header: a.pendingHeader, AppHash: fmt.Sprintf("%X", a.app.LastCommitID().Hash)}); err != nil {
		return nil, err
	}
	var gate demoFillGate
	err = readDemoMarker(filepath.Join(a.home, "fill-target.json"), &gate)
	if os.IsNotExist(err) {
		return answer, nil
	}
	if err != nil {
		return nil, err
	}
	if _, err := os.Stat(filepath.Join(a.home, "fill-committed.json")); err == nil {
		return answer, nil
	}
	state, err := a.app.CreateQueryContext(0, false)
	if err != nil {
		return nil, err
	}
	address, err := sdk.AccAddressFromBech32(gate.Contract)
	if err != nil {
		return nil, err
	}
	msg, _ := json.Marshal(map[string]any{"order": map[string]any{"order_id": gate.OrderID}})
	raw, err := a.app.WasmKeeper.QuerySmart(state, address, msg)
	if err != nil {
		return nil, err
	}
	var view struct {
		Order dcaOrderRecord `json:"order"`
	}
	if err := json.Unmarshal(raw, &view); err != nil {
		return nil, err
	}
	if view.Order.FilledCount == gate.Count {
		err = writeDemoMarker(filepath.Join(a.home, "fill-committed.json"), a.app.LastBlockHeight())
	}
	return answer, err
}
func (a *demoABCI) Query(ctx context.Context, r *abci.RequestQuery) (*abci.ResponseQuery, error) {
	a.mu.Lock()
	defer a.mu.Unlock()
	if r.Path != "/native-dca-demo/info" && r.Path != "/native-dca-demo/account" && r.Path != "/native-dca-demo/smart" {
		return a.Application.Query(ctx, r)
	}
	state, err := a.app.CreateQueryContext(r.Height, false)
	if err != nil {
		return &abci.ResponseQuery{Code: 1, Log: err.Error()}, nil
	}
	// A freshly loaded BaseApp has not initialized its check-state time before
	// handshake. Use the exact header persisted after the matching real Commit.
	if state.BlockTime().IsZero() {
		var committed demoCommittedHeader
		if err := readDemoMarker(filepath.Join(a.home, "committed-header.json"), &committed); err != nil {
			return &abci.ResponseQuery{Code: 1, Log: err.Error()}, nil
		}
		header := committed.Header
		if header.Height != state.BlockHeight() || header.Height != a.app.LastBlockHeight() || header.Time.IsZero() || header.ChainID != acceptanceChain || committed.AppHash != fmt.Sprintf("%X", a.app.LastCommitID().Hash) {
			return &abci.ResponseQuery{Code: 1, Log: "committed query header mismatch"}, nil
		}
		state = state.WithBlockHeader(header)
	}
	var request demoQuery
	if len(r.Data) > 0 {
		if err := json.Unmarshal(r.Data, &request); err != nil {
			return &abci.ResponseQuery{Code: 1, Log: err.Error()}, nil
		}
	}
	var value []byte
	switch r.Path {
	case "/native-dca-demo/info":
		c := a.app.AsyncKeeper.Config
		value, err = json.Marshal(demoInfo{Height: state.BlockHeight(), AppHash: fmt.Sprintf("%X", a.app.LastCommitID().Hash), HostReserve: c.HostReserve, Rent: c.StorageByteBlockPrice.String(), Cleanup: c.CleanupPrice.String()})
	case "/native-dca-demo/account":
		var address sdk.AccAddress
		address, err = sdk.AccAddressFromBech32(request.Address)
		if err == nil {
			account := a.app.AccountKeeper.GetAccount(state, address)
			answer := demoAccount{Balances: a.app.BankKeeper.GetAllBalances(state, address)}
			if account != nil {
				answer.Number, answer.Sequence = account.GetAccountNumber(), account.GetSequence()
			}
			value, err = json.Marshal(answer)
		}
	case "/native-dca-demo/smart":
		var address sdk.AccAddress
		address, err = sdk.AccAddressFromBech32(request.Contract)
		if err == nil {
			value, err = a.app.WasmKeeper.QuerySmart(state, address, request.Message)
		}
	}
	if err != nil {
		return &abci.ResponseQuery{Code: 1, Log: err.Error()}, nil
	}
	return &abci.ResponseQuery{Height: state.BlockHeight(), Value: value}, nil
}

func (a *demoABCI) Info(ctx context.Context, r *abci.RequestInfo) (*abci.ResponseInfo, error) {
	a.mu.Lock()
	defer a.mu.Unlock()
	return a.Application.Info(ctx, r)
}
func (a *demoABCI) InitChain(ctx context.Context, r *abci.RequestInitChain) (*abci.ResponseInitChain, error) {
	a.mu.Lock()
	defer a.mu.Unlock()
	return a.Application.InitChain(ctx, r)
}
func (a *demoABCI) CheckTx(ctx context.Context, r *abci.RequestCheckTx) (*abci.ResponseCheckTx, error) {
	a.mu.Lock()
	defer a.mu.Unlock()
	return a.Application.CheckTx(ctx, r)
}
func (a *demoABCI) PrepareProposal(ctx context.Context, r *abci.RequestPrepareProposal) (*abci.ResponsePrepareProposal, error) {
	a.mu.Lock()
	defer a.mu.Unlock()
	return a.Application.PrepareProposal(ctx, r)
}
func (a *demoABCI) ProcessProposal(ctx context.Context, r *abci.RequestProcessProposal) (*abci.ResponseProcessProposal, error) {
	a.mu.Lock()
	defer a.mu.Unlock()
	return a.Application.ProcessProposal(ctx, r)
}
func (a *demoABCI) ExtendVote(ctx context.Context, r *abci.RequestExtendVote) (*abci.ResponseExtendVote, error) {
	a.mu.Lock()
	defer a.mu.Unlock()
	return a.Application.ExtendVote(ctx, r)
}
func (a *demoABCI) VerifyVoteExtension(ctx context.Context, r *abci.RequestVerifyVoteExtension) (*abci.ResponseVerifyVoteExtension, error) {
	a.mu.Lock()
	defer a.mu.Unlock()
	return a.Application.VerifyVoteExtension(ctx, r)
}

func runNativeDCANode(t *testing.T) {
	home := os.Getenv("NATIVE_DCA_CHILD")
	ctx, cancel := signal.NotifyContext(context.Background(), syscall.SIGTERM, syscall.SIGINT)
	defer cancel()
	var ports struct{ RPC, P2P string }
	require.NoError(t, readDemoMarker(filepath.Join(home, "ports.json"), &ports))
	require.NoError(t, validateDemoRPC("http://"+ports.RPC))
	require.NoError(t, validateDemoRPC("http://"+ports.P2P))
	c := cfg.DefaultConfig().SetRoot(home)
	c.RPC.ListenAddress = "tcp://" + ports.RPC
	c.P2P.ListenAddress = "tcp://" + ports.P2P
	c.P2P.PexReactor, c.P2P.AddrBookStrict = false, false
	c.Consensus.TimeoutCommit = 100 * time.Millisecond
	c.Consensus.TimeoutPropose = 300 * time.Millisecond
	app, cleanupVM := openAcceptanceApp(t, home)
	pv := privval.LoadOrGenFilePV(c.PrivValidatorKeyFile(), c.PrivValidatorStateFile())
	nk, err := p2p.LoadNodeKey(c.NodeKeyFile())
	require.NoError(t, err)
	wrapped := &demoABCI{Application: server.NewCometABCIWrapper(app), app: app, home: home, shutdown: ctx.Done()}
	// A decided block can be replayed during NewNode's handshake before RPC
	// starts. Capture the freshly reopened committed app before that replay;
	// the persistent fill gate must remain closed until the parent compares it.
	var gate demoFillGate
	if err := readDemoMarker(filepath.Join(home, "fill-target.json"), &gate); err == nil {
		var filled int64
		if err := readDemoMarker(filepath.Join(home, "fill-committed.json"), &filled); err == nil {
			value, err := demoStartupSnapshot(wrapped, gate)
			require.NoError(t, err)
			require.Equal(t, filled, value.Info.Height)
			require.NoError(t, writeDemoMarker(filepath.Join(home, "restored-state.json"), demoStartupEvidence{PID: os.Getpid(), Snapshot: value}))
		}
	}
	n, err := node.NewNode(c, pv, nk, proxy.NewConnSyncLocalClientCreator(wrapped), node.DefaultGenesisDocProviderFunc(c), cfg.DefaultDBProvider, node.DefaultMetricsProvider(c.Instrumentation), cmtlog.NewNopLogger())
	require.NoError(t, err)
	require.NoError(t, n.Start())
	<-ctx.Done()
	require.NoError(t, n.Stop())
	n.Wait()
	cleanupVM()
	require.NoError(t, app.Close())
}

func demoStartupSnapshot(app *demoABCI, gate demoFillGate) (demoSnapshot, error) {
	s := demoSnapshot{Label: "same-home restart, before release", Source: "child_startup_committed_queries", Balances: map[string]sdk.Coins{}}
	query := func(route string, request demoQuery, out any) error {
		raw, err := json.Marshal(request)
		if err != nil {
			return err
		}
		response, err := app.Query(context.Background(), &abci.RequestQuery{Path: "/native-dca-demo/" + route, Data: raw})
		if err != nil {
			return err
		}
		if response.Code != 0 {
			return errors.New(response.Log)
		}
		return json.Unmarshal(response.Value, out)
	}
	smart := func(contract string, msg any, out any) error {
		raw, err := json.Marshal(msg)
		if err != nil {
			return err
		}
		return query("smart", demoQuery{Contract: contract, Message: raw}, out)
	}
	if err := query("info", demoQuery{}, &s.Info); err != nil {
		return s, err
	}
	var view struct {
		Order dcaOrderRecord `json:"order"`
	}
	if err := smart(gate.Contract, map[string]any{"order": map[string]any{"order_id": gate.OrderID}}, &view); err != nil {
		return s, err
	}
	s.State.Order = view.Order
	if err := smart(gate.Contract, map[string]any{"task": map[string]any{"order_id": gate.OrderID}}, &s.State.Task); err != nil {
		return s, err
	}
	if err := smart(gate.Contract, map[string]any{"async": map[string]any{"task_funding": map[string]any{"task_id": view.Order.TaskID}}}, &s.State.Funding); err != nil {
		return s, err
	}
	for i := uint32(0); i < view.Order.SliceCount; i++ {
		var fill *dcaFill
		if err := smart(gate.Contract, map[string]any{"fill": map[string]any{"order_id": gate.OrderID, "slice_index": i}}, &fill); err != nil {
			return s, err
		}
		s.Fills = append(s.Fills, fill)
	}
	for _, address := range []string{view.Order.Owner, view.Order.Beneficiary, gate.Contract, view.Order.Exchange, authtypes.NewModuleAddress(async.ModuleName).String(), authtypes.NewModuleAddress(authtypes.FeeCollectorName).String()} {
		var account demoAccount
		if err := query("account", demoQuery{Address: address}, &account); err != nil {
			return s, err
		}
		s.Balances[address] = account.Balances
	}
	return s, nil
}

func TestDCADemoRPCRejectsNonLoopback(t *testing.T) {
	for _, endpoint := range []string{"http://example.com:26657", "http://0.0.0.0:26657", "http://localhost:26657", "http://127.0.0.1:26657@evil.example", "http://127.0.0.1:0", "http://127.0.0.1:26657/path"} {
		require.Error(t, validateDemoRPC(endpoint), endpoint)
	}
	for _, endpoint := range []string{"http://127.0.0.1:26657", "http://[::1]:26657"} {
		require.NoError(t, validateDemoRPC(endpoint), endpoint)
	}
}
