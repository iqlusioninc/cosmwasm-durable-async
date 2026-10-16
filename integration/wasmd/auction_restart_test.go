//go:build upstream

package async_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math/rand"
	"os"
	"os/exec"
	"path/filepath"
	"testing"
	"time"

	sdkmath "cosmossdk.io/math"
	async "cosmwasm-durable-async/integration/wasmd"
	wasmapp "github.com/CosmWasm/wasmd/app"
	wasmtypes "github.com/CosmWasm/wasmd/x/wasm/types"
	cmted "github.com/cometbft/cometbft/crypto/ed25519"
	"github.com/cometbft/cometbft/p2p"
	"github.com/cometbft/cometbft/privval"
	rpchttp "github.com/cometbft/cometbft/rpc/client/http"
	cmttypes "github.com/cometbft/cometbft/types"
	"github.com/cosmos/cosmos-sdk/crypto/keys/secp256k1"
	sims "github.com/cosmos/cosmos-sdk/testutil/sims"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	banktypes "github.com/cosmos/cosmos-sdk/x/bank/types"
	"github.com/stretchr/testify/require"
)

func TestAuctionProcessRestart(t *testing.T) {
	if os.Getenv("NATIVE_AUCTION_NETWORK_CHILD") != "" {
		runAuctionNode(t)
		return
	}
	if os.Getenv("NATIVE_AUCTION_NETWORK_TEST") != "1" {
		t.Skip("set NATIVE_AUCTION_NETWORK_TEST=1")
	}
	// Losing partial writes/checkpoints or recomputing final entitlements on a
	// real restart must fail exact pre-handshake state and final bank comparisons.
	for _, phase := range []string{"allocate_base", "build_claims", "finalized"} {
		t.Run(phase, func(t *testing.T) {
			h := newAuctionNetwork(t, 2)
			h.setupAuction()
			h.reach(phase)
			before := h.capture(0, 0)
			oldPID := h.nodes[0].child.command.Process.Pid
			restored := h.restartValidator(before)
			require.NotEqual(t, oldPID, restored.PID)
			require.Equal(t, before, restored.State, "fresh process before Comet handshake/replay")
			h.settle()
			h.pruneAndClaim()
		})
	}
}

// The process-backed helpers are shared with the opt-in demo. They use only
// owned loopback nodes and preserve gates across real process boundaries.
type auctionNodeSpec struct {
	RPC, P2P, Peers, RPCServers string
	TrustHeight                 int64
	TrustHash                   string
}
type auctionCheckpoint struct {
	Height    int64
	Contract  string
	Addresses []string
}
type auctionCapture struct {
	Height   int64
	AppHash  string
	State    auctionRecoveredState
	Storage  map[string]string
	Balances map[string]sdk.Coins
}

// A pruned host task is absent, never a fabricated task with an empty status.
type auctionRecoveredState struct {
	View    auctionView
	Task    *async.TaskResponse
	Funding *auctionFunding
	Bids    []auctionBid
}
type auctionStartup struct {
	PID   int
	State auctionCapture
}
type auctionNetworkNode struct {
	home   string
	spec   auctionNodeSpec
	client *rpchttp.HTTP
	child  *ownedDemoChild
}
type auctionNetwork struct {
	t            *testing.T
	ctx          context.Context
	template     *appHarness
	keys         auctionKeys
	nodes        []*auctionNetworkNode
	contract     sdk.AccAddress
	addresses    []string
	terms        auctionTerms
	transactions []TxReceipt
	// Optional tutorial hooks; zero values retain the recovery-test fixture.
	setupOptions auctionSetupOptions
	submitSigned func(raws [][]byte, height int64) ([]TxReceipt, error)
}
type auctionSetupOptions struct {
	initialGas func(lease, attempt sdkmath.Int) sdkmath.Int
	ceiling    string
	commitment func(index int, bid auctionBookBid) (digest, salt []byte)
}
type auctionNetworkTx struct {
	key         *secp256k1.PrivKey
	msg         sdk.Msg
	wantFailure bool
}

func newAuctionNetwork(t *testing.T, count int) *auctionNetwork {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 150*time.Second)
	t.Cleanup(cancel)
	h := &auctionNetwork{t: t, ctx: ctx, template: newAcceptanceApp(t), keys: auctionFixtureKeys()}
	validatorKey := cmted.GenPrivKeyFromSecret([]byte("auction recovery validator"))
	validator := cmttypes.NewValidator(validatorKey.PubKey(), 1)
	accounts := []authtypes.GenesisAccount{}
	balances := []banktypes.Balance{}
	for i, key := range h.keys.all() {
		a := auctionAddress(key)
		accounts = append(accounts, authtypes.NewBaseAccount(a, key.PubKey(), uint64(i), 0))
		balances = append(balances, banktypes.Balance{Address: a.String(), Coins: auctionInitialCoins()})
		h.addresses = append(h.addresses, a.String())
	}
	h.addresses = append(h.addresses, authtypes.NewModuleAddress(async.ModuleName).String(), authtypes.NewModuleAddress(authtypes.FeeCollectorName).String())
	genesis, err := wasmapp.GenesisStateWithValSet(h.template.app.AppCodec(), h.template.app.DefaultGenesis(), cmttypes.NewValidatorSet([]*cmttypes.Validator{validator}), accounts, balances...)
	require.NoError(t, err)
	raw, err := json.Marshal(genesis)
	require.NoError(t, err)
	params := cmttypes.DefaultConsensusParams()
	params.Block.MaxGas = 400_000_000
	doc := cmttypes.GenesisDoc{GenesisTime: time.Now().UTC(), ChainID: acceptanceChain, InitialHeight: 1, ConsensusParams: params, Validators: []cmttypes.GenesisValidator{{Address: validator.Address, PubKey: validator.PubKey, Power: 1}}, AppState: raw}
	peers, rpcs := []string{}, []string{}
	for i := 0; i < count; i++ {
		n := &auctionNetworkNode{home: t.TempDir(), spec: auctionNodeSpec{RPC: networkFreeAddress(t), P2P: networkFreeAddress(t)}}
		require.NoError(t, os.MkdirAll(filepath.Join(n.home, "config"), 0700))
		require.NoError(t, os.MkdirAll(filepath.Join(n.home, "data"), 0700))
		require.NoError(t, doc.SaveAs(filepath.Join(n.home, "config", "genesis.json")))
		key := &p2p.NodeKey{PrivKey: cmted.GenPrivKey()}
		require.NoError(t, key.SaveAs(filepath.Join(n.home, "config", "node_key.json")))
		pvkey := cmted.GenPrivKey()
		if i == 0 {
			pvkey = validatorKey
		}
		pv := privval.NewFilePV(pvkey, filepath.Join(n.home, "config", "priv_validator_key.json"), filepath.Join(n.home, "data", "priv_validator_state.json"))
		pv.Save()
		n.client, err = rpchttp.New("http://"+n.spec.RPC, "/websocket")
		require.NoError(t, err)
		require.NoError(t, validateDemoRPC("http://"+n.spec.RPC))
		require.NoError(t, validateDemoRPC("http://"+n.spec.P2P))
		if i < 2 {
			peers = append(peers, string(key.ID())+"@"+n.spec.P2P)
			rpcs = append(rpcs, "http://"+n.spec.RPC)
		}
		h.nodes = append(h.nodes, n)
	}
	for _, n := range h.nodes {
		n.spec.Peers = joinAuctionStrings(peers)
		n.spec.RPCServers = joinAuctionStrings(rpcs)
		require.NoError(t, writeDemoMarker(filepath.Join(n.home, "allow-height.json"), int64(0)))
	}
	for _, n := range h.nodes[1:] {
		require.NoError(t, writeDemoMarker(filepath.Join(n.home, "allow-height.json"), int64(1<<60)))
	}
	h.startNode(1)
	h.startNode(0)
	// Connect before producing genesis height: a validator held inside a later
	// FinalizeBlock cannot gossip old consensus data to a late first peer.
	h.wait("loopback peer connected before first block", func() bool {
		ctx, cancel := context.WithTimeout(h.ctx, time.Second)
		defer cancel()
		info, err := h.nodes[1].client.NetInfo(ctx)
		return err == nil && info.NPeers == 1
	})
	h.allow(1)
	h.wait("first validator block", func() bool { return h.heightOK(0, 1) })
	h.waitGates(2)
	return h
}

func (h *auctionNetwork) startNode(i int) {
	n := h.nodes[i]
	require.NoError(h.t, writeDemoMarker(filepath.Join(n.home, "auction-node.json"), n.spec))
	executable, err := os.Executable()
	require.NoError(h.t, err)
	logfile, err := os.OpenFile(filepath.Join(n.home, "node.log"), os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0600)
	require.NoError(h.t, err)
	cmd := exec.CommandContext(h.ctx, executable, "-test.run=^TestAuctionProcessRestart$", "-test.timeout=3m")
	cmd.Env = append(os.Environ(), "NATIVE_AUCTION_NETWORK_CHILD="+n.home)
	cmd.Stdout, cmd.Stderr = logfile, logfile
	n.child, err = startDemoChild(cmd)
	require.NoError(h.t, err)
	child := n.child
	h.t.Cleanup(func() {
		_ = child.stop()
		_ = logfile.Close()
		if h.t.Failed() {
			raw, _ := os.ReadFile(filepath.Join(n.home, "node.log"))
			h.t.Logf("auction node %d: %s", i, raw)
		}
	})
}
func (h *auctionNetwork) wait(what string, f func() bool) {
	h.t.Helper()
	for h.ctx.Err() == nil {
		for i, n := range h.nodes {
			if n.child != nil {
				select {
				case <-n.child.done:
					h.t.Fatalf("owned auction node %d exited during %s: %v", i, what, n.child.err)
				default:
				}
			}
		}
		if f() {
			return
		}
		select {
		case <-h.ctx.Done():
		case <-time.After(20 * time.Millisecond):
		}
	}
	h.t.Fatalf("bounded auction wait: %s: %v", what, h.ctx.Err())
}
func (h *auctionNetwork) query(i int, height int64, route string, request any, out any) error {
	ctx, cancel := context.WithTimeout(h.ctx, time.Second)
	defer cancel()
	raw, err := json.Marshal(request)
	if err != nil {
		return err
	}
	r, err := h.nodes[i].client.ABCIQueryWithOptions(ctx, "/native-dca-demo/"+route, raw, auctionQueryOptions(height))
	if err != nil {
		return err
	}
	if r.Response.Code != 0 {
		return fmt.Errorf("query %s: %s", route, r.Response.Log)
	}
	return json.Unmarshal(r.Response.Value, out)
}
func (h *auctionNetwork) heightOK(i int, height int64) bool {
	var info demoInfo
	return h.query(i, 0, "info", demoQuery{}, &info) == nil && info.Height == height
}
func (h *auctionNetwork) height() int64 {
	var info demoInfo
	require.NoError(h.t, h.query(0, 0, "info", demoQuery{}, &info))
	return info.Height
}
func (h *auctionNetwork) waitGates(height int64) {
	h.wait("persisted pre-FinalizeBlock gates", func() bool {
		for _, n := range h.nodes[:1] {
			var entered int64
			if readDemoMarker(filepath.Join(n.home, "gate-entered.json"), &entered) != nil || entered != height {
				return false
			}
		}
		return true
	})
}
func (h *auctionNetwork) allow(height int64) {
	require.NoError(h.t, writeDemoMarker(filepath.Join(h.nodes[0].home, "allow-height.json"), height))
}
func (h *auctionNetwork) runTo(height int64) {
	h.t.Helper()
	current := h.height()
	if current < height {
		h.allow(height)
		h.wait("validator committed target", func() bool { return h.heightOK(0, height) })
	} else {
		height = current
	}
	h.waitGates(height + 1)
}
func (h *auctionNetwork) step() { h.runTo(h.height() + 1) }
func (h *auctionNetwork) smart(i int, height int64) auctionSmartQuery {
	return func(msg any, out any) {
		raw, err := json.Marshal(msg)
		require.NoError(h.t, err)
		require.NoError(h.t, h.query(i, height, "smart", demoQuery{Contract: h.contract.String(), Message: raw}, out))
	}
}
func (h *auctionNetwork) state() auctionState { return auctionReadState(h.t, h.smart(0, 0)) }
func (h *auctionNetwork) capture(i int, height int64) auctionCapture {
	h.t.Helper()
	var out auctionCapture
	require.NoError(h.t, h.query(i, height, "auction-capture", auctionCheckpoint{Height: height, Contract: h.contract.String(), Addresses: h.addresses}, &out))
	return out
}
func (h *auctionNetwork) equalAt(height int64) {
	h.t.Helper()
	h.advanceUntil("independent apps have common historical height", func() bool {
		for i, n := range h.nodes {
			if i == 0 || n.child == nil {
				continue
			}
			var info demoInfo
			if h.query(i, 0, "info", demoQuery{}, &info) != nil || info.Height < height {
				return false
			}
		}
		return true
	})
	first := h.capture(0, height)
	for i, n := range h.nodes {
		if i > 0 && n.child != nil {
			require.Equal(h.t, first, h.capture(i, height), "independent app at height %d", height)
		}
	}
	h.logProgress("independent historical equality", height)
}
func (h *auctionNetwork) logProgress(label string, fixed int64) {
	heights := map[string]int64{}
	for i, n := range h.nodes {
		if n.child == nil {
			continue
		}
		var info demoInfo
		require.NoError(h.t, h.query(i, 0, "info", demoQuery{}, &info))
		heights[[]string{"validator", "peer", "joiner"}[i]] = info.Height
	}
	deadline, ok := h.ctx.Deadline()
	require.True(h.t, ok)
	h.t.Logf("%s fixed_H=%d elapsed=%s app_heights=%v", label, fixed, (150*time.Second - time.Until(deadline)).Round(time.Millisecond), heights)
}

// A blocked FinalizeBlock also blocks Comet's consensus-state cache refresh.
// Keep producing until the peer has the fixed evidence height, then reclose a
// durable gate. Historical comparisons never substitute a later app state.
func (h *auctionNetwork) advanceUntil(what string, ready func() bool) {
	h.allow(h.height() + 1000)
	h.wait(what, ready)
	h.closeGate()
}
func (h *auctionNetwork) closeGate() {
	observed := h.height()
	h.allow(observed)
	var entered int64
	h.wait("acknowledged validator gate after provider progress", func() bool {
		if readDemoMarker(filepath.Join(h.nodes[0].home, "gate-entered.json"), &entered) != nil || entered <= observed {
			return false
		}
		return h.heightOK(0, entered-1)
	})
	require.Equal(h.t, entered-1, h.height(), "actual gate acknowledgement binds last committed height")
}
func (h *auctionNetwork) submitBatch(txs ...auctionNetworkTx) {
	h.t.Helper()
	height := h.height()
	h.waitGates(height + 1)
	raws := make([][]byte, 0, len(txs))
	for _, item := range txs {
		var account demoAccount
		require.NoError(h.t, h.query(0, 0, "account", demoQuery{Address: auctionAddress(item.key).String()}, &account))
		// Ordinary proposal gas is 180m after async/EndBlock reservations;
		// five transactions at 30m fit, independently of the 100m poll cap.
		tx, err := sims.GenSignedMockTx(rand.New(rand.NewSource(1)), h.template.app.TxConfig(), []sdk.Msg{item.msg}, sdk.NewCoins(sdk.NewInt64Coin("stake", 1)), 30_000_000, acceptanceChain, []uint64{account.Number}, []uint64{account.Sequence}, item.key)
		require.NoError(h.t, err)
		raw, err := h.template.app.TxConfig().TxEncoder()(tx)
		require.NoError(h.t, err)
		raws = append(raws, raw)
	}
	if h.submitSigned != nil {
		receipts, err := h.submitSigned(raws, height+2)
		h.transactions = append(h.transactions, receipts...)
		require.NoError(h.t, err, "uncertain or failed journal operations halt without resubmission")
		require.Len(h.t, receipts, len(txs))
		for i, receipt := range receipts {
			require.Equal(h.t, "committed", receipt.Outcome, "unresolved outcomes halt; never resubmit: %v", err)
			require.NotNil(h.t, receipt.Height)
			require.Equal(h.t, height+2, *receipt.Height)
			require.NotNil(h.t, receipt.DeliverTxCode)
			if txs[i].wantFailure {
				require.NotZero(h.t, *receipt.DeliverTxCode)
			} else {
				require.Zero(h.t, *receipt.DeliverTxCode, "transaction failed: %v", err)
			}
		}
		return
	}
	for _, raw := range raws {
		ctx, cancel := context.WithTimeout(h.ctx, time.Second)
		r, err := h.nodes[0].client.BroadcastTxSync(ctx, raw)
		cancel()
		require.NoError(h.t, err)
		require.Zero(h.t, r.Code, r.Log)
		h.transactions = append(h.transactions, TxReceipt{Hash: fmt.Sprintf("%X", cmttypes.Tx(raw).Hash())})
	}
	h.runTo(height + 2)
	for i, receipt := range h.transactions[len(h.transactions)-len(txs):] {
		hash, err := hex.DecodeString(receipt.Hash)
		require.NoError(h.t, err)
		h.wait("committed transaction index", func() bool {
			ctx, cancel := context.WithTimeout(h.ctx, time.Second)
			defer cancel()
			r, err := h.nodes[0].client.Tx(ctx, hash, false)
			if err != nil {
				return false
			}
			require.Equal(h.t, height+2, r.Height)
			if txs[i].wantFailure {
				require.NotZero(h.t, r.TxResult.Code, r.TxResult.Log)
			} else {
				require.Zero(h.t, r.TxResult.Code, r.TxResult.Log)
			}
			return true
		})
	}
}
func (h *auctionNetwork) execute(key *secp256k1.PrivKey, msg any, coins sdk.Coins) {
	h.submitBatch(h.execution(key, msg, coins))
}
func (h *auctionNetwork) execution(key *secp256k1.PrivKey, msg any, coins sdk.Coins) auctionNetworkTx {
	raw, err := json.Marshal(msg)
	require.NoError(h.t, err)
	return auctionNetworkTx{key: key, msg: &wasmtypes.MsgExecuteContract{Sender: auctionAddress(key).String(), Contract: h.contract.String(), Msg: raw, Funds: coins}}
}
func (h *auctionNetwork) setupAuction() {
	t := h.t
	code, err := os.ReadFile("../../artifacts/native_batch_auction.wasm")
	require.NoError(t, err)
	hash := sha256.Sum256(code)
	require.Equal(t, "10261b95c7a9d8185d01aab4efabc15803f828fdc2a8c69792a5065637874b97", hex.EncodeToString(hash[:]))
	t.Logf("auction source base=d2af86e artifact sha256=%x", hash)
	seller := auctionAddress(h.keys.Seller).String()
	h.submitBatch(auctionNetworkTx{key: h.keys.Seller, msg: &wasmtypes.MsgStoreCode{Sender: seller, WASMByteCode: code}})
	h.submitBatch(auctionNetworkTx{key: h.keys.Seller, msg: &wasmtypes.MsgInstantiateContract{Sender: seller, CodeID: 1, Label: "recovery auction", Msg: []byte(`{"sale_denom":"usale","quote_denom":"uquote","fee_denom":"ugas"}`)}})
	h.contract = auctionContractAddress()
	h.addresses = append(h.addresses, h.contract.String())
	registration := uint64(h.height() + 2)
	h.terms = auctionFixtureTerms(registration)
	var funding auctionFundingConfig
	h.smart(0, 0)(map[string]any{"async": map[string]any{"funding_config": map[string]any{}}}, &funding)
	price, err := sdkmath.LegacyNewDecFromStr(funding.GasPrice)
	require.NoError(t, err)
	deposit, lease, attempt := auctionDeposit(t, h.terms, registration, 30, async.ExperimentalConfig(), price)
	if h.setupOptions.initialGas != nil {
		deposit = h.setupOptions.initialGas(lease, attempt)
	}
	ceiling := h.setupOptions.ceiling
	if ceiling == "" {
		ceiling = "1"
	}
	h.execute(h.keys.Seller, map[string]any{"create_auction": map[string]any{"terms": h.terms, "gas": sdk.NewCoin("ugas", deposit), "gas_per_poll": auctionPollCap, "max_gas_price": ceiling, "allocated_bytes": auctionAllocation}}, sdk.NewCoins(sdk.NewCoin("ugas", deposit), sdk.NewInt64Coin("usale", 50)))
	require.Equal(t, registration, h.state().View.Auction.CreatedAt)
	commits, reveals := []auctionNetworkTx{}, []auctionNetworkTx{}
	for i, bid := range auctionFixtureBook() {
		salt := bytes.Repeat([]byte{byte(i + 1)}, 32)
		bidder, beneficiary := auctionAddress(h.keys.Bidders[i]).String(), auctionAddress(h.keys.Beneficiaries[i]).String()
		digest := auctionCommitment(acceptanceChain, h.contract.String(), bidder, beneficiary, bid.Collateral, bid.Quantity, bid.Tick, salt)
		if h.setupOptions.commitment != nil {
			digest, salt = h.setupOptions.commitment(i, bid)
		}
		commits = append(commits, h.execution(h.keys.Bidders[i], map[string]any{"commit_bid": map[string]any{"digest": digest, "collateral": bid.Collateral}}, sdk.NewCoins(sdk.NewCoin("uquote", auctionInt(t, bid.Collateral)))))
		if bid.Revealed {
			reveals = append(reveals, h.execution(h.keys.Bidders[i], map[string]any{"reveal_bid": map[string]any{"quantity_lots": bid.Quantity, "price_tick": bid.Tick, "beneficiary": beneficiary, "salt": salt}}, sdk.Coins{}))
		}
	}
	h.submitBatch(commits...)
	h.runTo(int64(h.terms.CommitEnd) - 2)
	h.submitBatch(reveals...)
	require.Equal(t, "29", h.state().View.Auction.TotalCollateral)
	require.Equal(t, "7", h.state().View.Auction.TotalDemand)
}
func (h *auctionNetwork) reach(phase string) {
	for i := 0; i < 60; i++ {
		s := h.state()
		a := s.View.Auction
		if (phase == "finalized" && a.State == "finalized") || (a.Settlement.Phase == phase && a.Settlement.Cursor == 2) {
			return
		}
		require.NotEqual(h.t, "paused", s.Task.Status.Kind)
		h.step()
	}
	h.t.Fatal("did not reach nonempty auction batch: " + phase)
}
func (h *auctionNetwork) settle() {
	t := h.t
	current := h.capture(0, 0)
	if current.State.View.Auction.State != "finalized" {
		h.reach("finalized")
		current = h.capture(0, 0)
	}
	terminal := current
	if terminal.State.Task == nil {
		// Providers can pass E while serving a slow joiner. Use a genuinely
		// committed pre-prune finalization, never synthesize a completed task.
		found := false
		for height := int64(h.terms.RevealEnd); height < int64(h.terms.ExpiresAt) && height <= current.Height; height++ {
			candidate := h.capture(0, height)
			if candidate.State.View.Auction.State == "finalized" {
				terminal = candidate
				found = true
				break
			}
		}
		require.True(t, found, "a real finalized task receipt must precede pruning")
	}
	require.NotNil(t, terminal.State.Task)
	require.NotNil(t, terminal.State.Funding)
	auctionAssertOracle(t, auctionState{View: terminal.State.View, Task: *terminal.State.Task, Funding: *terminal.State.Funding, Bids: terminal.State.Bids})
	require.Equal(t, terminal.State.View.Auction, current.State.View.Auction)
	require.Equal(t, terminal.State.Bids, current.State.Bids)
	t.Logf("genuine terminal receipt H=%d currentH=%d host_present=%v", terminal.Height, current.Height, current.State.Task != nil)
}
func (h *auctionNetwork) checkpoint() auctionCheckpoint {
	return auctionCheckpoint{h.height(), h.contract.String(), h.addresses}
}
func (h *auctionNetwork) restartValidator(before auctionCapture) auctionStartup {
	n := h.nodes[0]
	require.NoError(h.t, writeDemoMarker(filepath.Join(n.home, "auction-checkpoint.json"), h.checkpoint()))
	require.NoError(h.t, n.child.stop())
	require.NoError(h.t, n.child.err)
	_, err := os.Stat(filepath.Join(n.home, "auction-startup.json"))
	require.True(h.t, os.IsNotExist(err))
	h.startNode(0)
	var startup auctionStartup
	h.wait("PID-bound pre-handshake auction evidence", func() bool {
		return readDemoMarker(filepath.Join(n.home, "auction-startup.json"), &startup) == nil && startup.PID == n.child.command.Process.Pid
	})
	require.Equal(h.t, before, startup.State)
	h.t.Logf("restart old committed H=%d new_pid=%d app_hash=%s phase=%s cursor=%d", before.Height, startup.PID, before.AppHash, before.State.View.Auction.Settlement.Phase, before.State.View.Auction.Settlement.Cursor)
	h.allow(before.Height + 1)
	h.wait("gated restarted handshake and next committed block", func() bool { return h.heightOK(0, before.Height+1) })
	h.waitGates(before.Height + 2)
	return startup
}
func (h *auctionNetwork) pruneAndClaim() {
	t := h.t
	before := h.capture(0, 0)
	require.False(t, before.State.View.Auction.SellerClaimed)
	for _, bid := range before.State.Bids {
		require.False(t, bid.Claimed)
	}
	h.runTo(int64(h.terms.ExpiresAt) + 1)
	pruned := h.capture(0, 0)
	require.NotNil(t, pruned.State.View.Host.Unavailable)
	require.Equal(t, "finalized", pruned.State.View.Auction.State)
	require.Equal(t, before.State.View.Auction, pruned.State.View.Auction)
	require.Equal(t, before.State.Bids, pruned.State.Bids)
	// Query failure cannot turn finalized rights into full collateral refunds.
	for i := range auctionFixtureBook() {
		h.execute(h.keys.Bidders[i], map[string]any{"claim_bid": map[string]any{"bidder": auctionAddress(h.keys.Bidders[i]).String()}}, sdk.Coins{})
	}
	h.execute(h.keys.Seller, map[string]any{"claim_seller": map[string]any{}}, sdk.Coins{})
	after := h.capture(0, 0)
	require.Equal(t, auctionClaimTotals{"50", "14", "0", "15"}, after.State.View.Auction.Claimed)
	require.True(t, after.State.View.Auction.SellerClaimed)
	for i, bid := range after.State.Bids {
		require.True(t, bid.Claimed)
		beneficiary, bidder := auctionAddress(h.keys.Beneficiaries[i]).String(), auctionAddress(h.keys.Bidders[i]).String()
		require.Equal(t, before.Balances[beneficiary].AmountOf("usale").AddRaw([]int64{20, 20, 10, 0, 0}[i]), after.Balances[beneficiary].AmountOf("usale"))
		require.Equal(t, before.Balances[bidder].AmountOf("uquote").AddRaw([]int64{2, 0, 3, 2, 7}[i]), after.Balances[bidder].AmountOf("uquote"))
	}
	seller := auctionAddress(h.keys.Seller).String()
	require.Equal(t, before.Balances[seller].AmountOf("uquote").AddRaw(15), after.Balances[seller].AmountOf("uquote"))
	require.Zero(t, after.Balances[h.contract.String()].AmountOf("usale").Int64())
	require.Zero(t, after.Balances[h.contract.String()].AmountOf("uquote").Int64())
	h.equalAt(after.Height)
}
