//go:build upstream

package async_test

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"math/big"
	"math/rand"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"sync"
	"syscall"
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
	coretypes "github.com/cometbft/cometbft/rpc/core/types"
	cmttypes "github.com/cometbft/cometbft/types"
	"github.com/cosmos/cosmos-sdk/crypto/keys/secp256k1"
	sims "github.com/cosmos/cosmos-sdk/testutil/sims"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	banktypes "github.com/cosmos/cosmos-sdk/x/bank/types"
	"github.com/stretchr/testify/require"
)

type ownedDemoChild struct {
	command *exec.Cmd
	done    chan struct{}
	err     error
	once    sync.Once
}

func startDemoChild(command *exec.Cmd) (*ownedDemoChild, error) {
	if err := command.Start(); err != nil {
		return nil, err
	}
	child := &ownedDemoChild{command: command, done: make(chan struct{})}
	go func() { child.err = command.Wait(); close(child.done) }()
	return child, nil
}

func (c *ownedDemoChild) stop() error {
	c.once.Do(func() {
		select {
		case <-c.done:
			return
		default:
		}
		_ = c.command.Process.Signal(syscall.SIGTERM)
		select {
		case <-c.done:
		case <-time.After(3 * time.Second):
			_ = c.command.Process.Kill()
			<-c.done
		}
	})
	return nil
}

func TestDCADemoCleanupHelper(t *testing.T) {
	if os.Getenv("NATIVE_DCA_CLEANUP_HELPER") != "1" {
		t.Skip("owned cleanup helper")
	}
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, syscall.SIGTERM)
	defer signal.Stop(signals)
	<-signals
}

func TestDCADemoCleanupOwnedChild(t *testing.T) {
	executable, err := os.Executable()
	require.NoError(t, err)
	start := func() *ownedDemoChild {
		cmd := exec.Command(executable, "-test.run=^TestDCADemoCleanupHelper$", "-test.timeout=30s")
		cmd.Env = append(os.Environ(), "NATIVE_DCA_CLEANUP_HELPER=1")
		child, err := startDemoChild(cmd)
		require.NoError(t, err)
		t.Cleanup(func() { _ = child.command.Process.Kill(); <-child.done })
		return child
	}
	owned, unrelated := start(), start()
	require.NoError(t, owned.stop())
	select {
	case <-owned.done:
	case <-time.After(time.Second):
		t.Fatal("owned child was not stopped and reaped")
	}
	unrelatedProcessStillAlive := unrelated.command.Process.Signal(syscall.Signal(0)) == nil
	require.True(t, unrelatedProcessStillAlive)
	require.NoError(t, owned.stop(), "cleanup must be idempotent, with one Wait")
}

func TestNativeDCADemo(t *testing.T) {
	if os.Getenv("NATIVE_DCA_CHILD") != "" {
		runNativeDCANode(t)
		return
	}
	if os.Getenv("NATIVE_DCA_DEMO") != "1" {
		t.Skip("opt-in actual local validator; use scripts/native-dca-demo.sh")
	}
	scenario := os.Getenv("NATIVE_DCA_SCENARIO")
	names := []string{scenario}
	if scenario == "all" {
		names = []string{"happy", "funding", "liquidity", "cancel", "restart"}
	}
	for _, name := range names {
		require.Contains(t, []string{"happy", "funding", "liquidity", "cancel", "restart"}, name)
		t.Run(name, func(t *testing.T) {
			ctx, cancel := context.WithTimeout(context.Background(), 90*time.Second)
			defer cancel()
			h := newNativeDCADemo(t, ctx, name)
			runDCAScenario(t, h, name)
			h.finish()
		})
	}
}

type demoSnapshot struct {
	Label    string               `json:"label"`
	Source   string               `json:"source"`
	Info     demoInfo             `json:"committed"`
	State    dcaState             `json:"state"`
	Fills    []*dcaFill           `json:"fills"`
	Balances map[string]sdk.Coins `json:"balances"`
}
type demoRunReceipt struct {
	Scenario     string         `json:"scenario"`
	RPC          string         `json:"rpc"`
	Owner        string         `json:"owner"`
	Beneficiary  string         `json:"beneficiary_and_sponsor"`
	DCA          string         `json:"dca"`
	Exchange     string         `json:"exchange"`
	Transactions []TxReceipt    `json:"transactions"`
	Snapshots    []demoSnapshot `json:"snapshots"`
	ChildExited  bool           `json:"owned_child_exited"`
	Restarts     int            `json:"restarts"`
	RestartPID   int            `json:"restart_startup_child_pid,omitempty"`
	Passed       bool           `json:"passed"`
}
type nativeDCADemo struct {
	t             *testing.T
	ctx           context.Context
	template      *appHarness
	key, sponsor  *secp256k1.PrivKey
	client        *rpchttp.HTTP
	home, output  string
	child         *ownedDemoChild
	dca, exchange sdk.AccAddress
	receipt       demoRunReceipt
	finished      bool
	logs          []*os.File
}

func newNativeDCADemo(t *testing.T, ctx context.Context, scenario string) *nativeDCADemo {
	t.Helper()
	h := &nativeDCADemo{t: t, ctx: ctx, template: newAcceptanceApp(t), home: t.TempDir(), key: secp256k1.GenPrivKeyFromSecret([]byte("DCA owner")), sponsor: secp256k1.GenPrivKeyFromSecret([]byte("DCA sponsor"))}
	base := os.Getenv("NATIVE_DCA_OUTPUT")
	if base == "" {
		base = t.TempDir()
	}
	var err error
	h.output, err = os.MkdirTemp(base, scenario+"-")
	require.NoError(t, err)
	h.receipt = demoRunReceipt{Scenario: scenario, Owner: dcaAddress(h.key).String(), Beneficiary: dcaAddress(h.sponsor).String()}
	t.Cleanup(func() {
		if h.child != nil {
			_ = h.child.stop()
			select {
			case <-h.child.done:
				h.receipt.ChildExited = true
			default:
			}
		}
		for _, log := range h.logs {
			_ = log.Close()
		}
		h.receipt.Passed = h.finished && !t.Failed()
		require.NoError(t, writeDemoMarker(filepath.Join(h.output, "receipt.json"), h.receipt))
		t.Logf("scenario=%s passed=%v owned_child_exited=%v receipt=%s", scenario, h.receipt.Passed, h.receipt.ChildExited, filepath.Join(h.output, "receipt.json"))
	})
	require.NoError(t, os.MkdirAll(filepath.Join(h.home, "config"), 0700))
	require.NoError(t, os.MkdirAll(filepath.Join(h.home, "data"), 0700))
	validatorKey := cmted.GenPrivKeyFromSecret([]byte("native DCA demo validator"))
	validator := cmttypes.NewValidator(validatorKey.PubKey(), 1)
	accounts := []authtypes.GenesisAccount{}
	balances := []banktypes.Balance{}
	for _, key := range []*secp256k1.PrivKey{h.key, h.sponsor} {
		address := dcaAddress(key)
		accounts = append(accounts, authtypes.NewBaseAccount(address, key.PubKey(), uint64(len(accounts)), 0))
		balances = append(balances, banktypes.Balance{Address: address.String(), Coins: sdk.NewCoins(sdk.NewInt64Coin("stake", 1_000_000_000), sdk.NewInt64Coin("ugas", 1_000_000_000), sdk.NewInt64Coin("uinput", 1_000_000), sdk.NewInt64Coin("uout", 1_000_000))})
	}
	genesis, err := wasmapp.GenesisStateWithValSet(h.template.app.AppCodec(), h.template.app.DefaultGenesis(), cmttypes.NewValidatorSet([]*cmttypes.Validator{validator}), accounts, balances...)
	require.NoError(t, err)
	raw, err := json.Marshal(genesis)
	require.NoError(t, err)
	params := cmttypes.DefaultConsensusParams()
	params.Block.MaxGas = 400_000_000
	doc := cmttypes.GenesisDoc{GenesisTime: time.Now().UTC(), ChainID: acceptanceChain, InitialHeight: 1, ConsensusParams: params, Validators: []cmttypes.GenesisValidator{{Address: validator.Address, PubKey: validator.PubKey, Power: 1}}, AppState: raw}
	require.NoError(t, doc.SaveAs(filepath.Join(h.home, "config", "genesis.json")))
	nodeKey := &p2p.NodeKey{PrivKey: cmted.GenPrivKey()}
	require.NoError(t, nodeKey.SaveAs(filepath.Join(h.home, "config", "node_key.json")))
	pv := privval.NewFilePV(validatorKey, filepath.Join(h.home, "config", "priv_validator_key.json"), filepath.Join(h.home, "data", "priv_validator_state.json"))
	pv.Save()
	rpc, p2paddr := networkFreeAddress(t), networkFreeAddress(t)
	require.NoError(t, writeDemoMarker(filepath.Join(h.home, "ports.json"), struct{ RPC, P2P string }{rpc, p2paddr}))
	require.NoError(t, writeDemoMarker(filepath.Join(h.home, "allow-height.json"), int64(1)))
	h.receipt.RPC = "http://" + rpc
	require.NoError(t, validateDemoRPC(h.receipt.RPC))
	h.client, err = rpchttp.New(h.receipt.RPC, "/websocket")
	require.NoError(t, err)
	h.start()
	h.wait("first committed block", func() bool { var info demoInfo; return h.query("info", demoQuery{}, &info) == nil && info.Height == 1 })
	h.waitGate(2)
	for _, artifact := range []string{"mock_exchange", "native_dca"} {
		code, err := os.ReadFile("../../artifacts/" + artifact + ".wasm")
		require.NoError(t, err)
		h.submit(h.key, &wasmtypes.MsgStoreCode{Sender: dcaAddress(h.key).String(), WASMByteCode: code})
	}
	h.exchange = h.instantiate(1, map[string]any{"input_denom": "uinput", "output_denom": "uout", "numerator": "3", "denominator": "2"}, "local fixed-price fixture")
	h.dca = h.instantiate(2, map[string]any{"exchange": h.exchange.String(), "fee_denom": "ugas"}, "native DCA")
	h.receipt.DCA, h.receipt.Exchange = h.dca.String(), h.exchange.String()
	return h
}

func (h *nativeDCADemo) start() {
	executable, err := os.Executable()
	require.NoError(h.t, err)
	log, err := os.OpenFile(filepath.Join(h.output, "node.log"), os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0600)
	require.NoError(h.t, err)
	h.logs = append(h.logs, log)
	cmd := exec.CommandContext(h.ctx, executable, "-test.run=^TestNativeDCADemo$", "-test.timeout=2m")
	cmd.Env = append(os.Environ(), "NATIVE_DCA_CHILD="+h.home)
	cmd.Stdout, cmd.Stderr = log, log
	h.child, err = startDemoChild(cmd)
	require.NoError(h.t, err)
}
func (h *nativeDCADemo) wait(what string, f func() bool) {
	h.t.Helper()
	for h.ctx.Err() == nil {
		select {
		case <-h.child.done:
			h.t.Fatalf("owned child exited during %s: %v; log %s", what, h.child.err, filepath.Join(h.output, "node.log"))
		default:
		}
		if f() {
			return
		}
		select {
		case <-h.ctx.Done():
		case <-time.After(20 * time.Millisecond):
		}
	}
	h.t.Fatalf("bounded wait failed: %s: %v; node log %s", what, h.ctx.Err(), filepath.Join(h.output, "node.log"))
}
func (h *nativeDCADemo) query(route string, request demoQuery, result any) error {
	ctx, cancel := context.WithTimeout(h.ctx, demoRPCDeadline)
	defer cancel()
	raw, err := json.Marshal(request)
	if err != nil {
		return err
	}
	response, err := h.client.ABCIQuery(ctx, "/native-dca-demo/"+route, raw)
	if err != nil {
		return err
	}
	if response.Response.Code != 0 {
		return fmt.Errorf("query: %s", response.Response.Log)
	}
	return json.Unmarshal(response.Response.Value, result)
}
func (h *nativeDCADemo) info() demoInfo {
	h.t.Helper()
	var info demoInfo
	require.NoError(h.t, h.query("info", demoQuery{}, &info))
	return info
}
func (h *nativeDCADemo) account(address sdk.AccAddress) demoAccount {
	h.t.Helper()
	var value demoAccount
	require.NoError(h.t, h.query("account", demoQuery{Address: address.String()}, &value))
	return value
}
func (h *nativeDCADemo) smart(contract sdk.AccAddress, msg any, result any) {
	h.t.Helper()
	raw, err := json.Marshal(msg)
	require.NoError(h.t, err)
	require.NoError(h.t, h.query("smart", demoQuery{Contract: contract.String(), Message: raw}, result))
}
func (h *nativeDCADemo) waitGate(height int64) {
	h.wait("next consensus block waiting before FinalizeBlock", func() bool {
		var entered int64
		return readDemoMarker(filepath.Join(h.home, "gate-entered.json"), &entered) == nil && entered == height
	})
}
func (h *nativeDCADemo) allow(height int64) {
	require.NoError(h.t, writeDemoMarker(filepath.Join(h.home, "allow-height.json"), height))
}
func (h *nativeDCADemo) step() {
	height := h.info().Height + 1
	h.allow(height)
	h.wait("committed next consensus block", func() bool { return h.info().Height == height })
	h.waitGate(height + 1)
}

type demoSubmitRPC struct {
	*rpchttp.HTTP
	after func()
}

func (r demoSubmitRPC) BroadcastTxSync(ctx context.Context, tx cmttypes.Tx) (*coretypes.ResultBroadcastTx, error) {
	answer, err := r.HTTP.BroadcastTxSync(ctx, tx)
	r.after()
	return answer, err
}
func (h *nativeDCADemo) submitResult(key *secp256k1.PrivKey, msg sdk.Msg) (TxReceipt, error) {
	h.t.Helper()
	height := h.info().Height
	h.waitGate(height + 1)
	account := h.account(dcaAddress(key))
	tx, err := sims.GenSignedMockTx(rand.New(rand.NewSource(1)), h.template.app.TxConfig(), []sdk.Msg{msg}, sdk.NewCoins(sdk.NewInt64Coin("stake", 1)), 100_000_000, acceptanceChain, []uint64{account.Number}, []uint64{account.Sequence}, key)
	require.NoError(h.t, err)
	raw, err := h.template.app.TxConfig().TxEncoder()(tx)
	require.NoError(h.t, err)
	path := filepath.Join(h.output, fmt.Sprintf("tx-%03d.json", len(h.receipt.Transactions)+1))
	_, err = writeIntent(path, raw)
	require.NoError(h.t, err)
	rpc := demoSubmitRPC{h.client, func() { h.allow(height + 2) }}
	receipt, runErr := broadcastOnce(h.ctx, rpc, path)
	h.receipt.Transactions = append(h.receipt.Transactions, receipt)
	if receipt.Height != nil {
		h.t.Logf("tx hash=%s outcome=%s height=%d", receipt.Hash, receipt.Outcome, *receipt.Height)
	} else {
		h.t.Logf("tx hash=%s outcome=%s height=unknown", receipt.Hash, receipt.Outcome)
	}
	if receipt.Height != nil {
		require.Equal(h.t, height+2, *receipt.Height, "normal Comet mempool includes tx after the already-built empty block")
	}
	return receipt, runErr
}
func (h *nativeDCADemo) submit(key *secp256k1.PrivKey, msg sdk.Msg) TxReceipt {
	h.t.Helper()
	receipt, err := h.submitResult(key, msg)
	require.NoError(h.t, err)
	return receipt
}
func (h *nativeDCADemo) execute(key *secp256k1.PrivKey, msg any, coins sdk.Coins) TxReceipt {
	h.t.Helper()
	raw, err := json.Marshal(msg)
	require.NoError(h.t, err)
	return h.submit(key, &wasmtypes.MsgExecuteContract{Sender: dcaAddress(key).String(), Contract: h.dca.String(), Msg: raw, Funds: coins})
}
func (h *nativeDCADemo) instantiate(code uint64, msg any, label string) sdk.AccAddress {
	raw, err := json.Marshal(msg)
	require.NoError(h.t, err)
	receipt := h.submit(h.key, &wasmtypes.MsgInstantiateContract{Sender: dcaAddress(h.key).String(), CodeID: code, Msg: raw, Label: label})
	hash, err := hex.DecodeString(receipt.Hash)
	require.NoError(h.t, err)
	ctx, cancel := context.WithTimeout(h.ctx, demoRPCDeadline)
	defer cancel()
	result, err := h.client.Tx(ctx, hash, false)
	require.NoError(h.t, err)
	for _, event := range result.TxResult.Events {
		if event.Type == "instantiate" {
			for _, attr := range event.Attributes {
				if attr.Key == "_contract_address" {
					return sdk.MustAccAddressFromBech32(attr.Value)
				}
			}
		}
	}
	h.t.Fatal("committed instantiate lacks address")
	return nil
}

func (h *nativeDCADemo) liquidity(amount int64) {
	h.submit(h.key, &banktypes.MsgSend{FromAddress: dcaAddress(h.key).String(), ToAddress: h.exchange.String(), Amount: sdk.NewCoins(sdk.NewInt64Coin("uout", amount))})
}
func (h *nativeDCADemo) create(slices uint32, interval, attempts uint64) {
	info := h.info()
	registration := uint64(info.Height + 2)
	expiry := registration + 100
	var config dcaFundingConfig
	h.smart(h.dca, map[string]any{"async": map[string]any{"funding_config": map[string]any{}}}, &config)
	require.Equal(h.t, "ugas", config.Denom)
	require.Equal(h.t, "0.001", config.MinBasePrice)
	require.Equal(h.t, dcaPollCap, config.MaxGasPerPoll)
	require.Equal(h.t, uint64(70_000_000), info.HostReserve)
	require.Equal(h.t, "1", info.Rent)
	require.Equal(h.t, "100", info.Cleanup)
	price, err := sdkmath.LegacyNewDecFromStr(config.GasPrice)
	require.NoError(h.t, err)
	require.True(h.t, price.IsPositive())
	leaseBig := new(big.Int).Mul(new(big.Int).SetUint64(dcaAllocation), new(big.Int).SetUint64(expiry-registration))
	leaseBig.Mul(leaseBig, dcaInt(h.t, info.Rent).BigInt())
	leaseBig.Add(leaseBig, dcaInt(h.t, info.Cleanup).BigInt())
	require.LessOrEqual(h.t, leaseBig.BitLen(), 128)
	lease := sdkmath.NewIntFromBigInt(leaseBig)
	attempt := price.MulInt(sdkmath.NewIntFromUint64(dcaPollCap)).Ceil().TruncateInt()
	deposit := lease.Add(attempt.Mul(sdkmath.NewIntFromUint64(attempts)))
	require.LessOrEqual(h.t, deposit.BigInt().BitLen(), 128)
	msg := map[string]any{"create_order": map[string]any{"input_per_slice": sdk.NewInt64Coin("uinput", 10), "slice_count": slices, "minimum_output": "14", "interval_blocks": interval, "beneficiary": dcaAddress(h.sponsor).String(), "gas": sdk.NewCoin("ugas", deposit), "gas_per_poll": dcaPollCap, "max_gas_price": "1", "expires_at": expiry, "allocated_bytes": dcaAllocation}}
	h.execute(h.key, msg, sdk.NewCoins(sdk.NewInt64Coin("uinput", 10*int64(slices)), sdk.NewCoin("ugas", deposit)))
	s := h.snapshot("registered")
	require.Equal(h.t, registration, s.State.Order.RegistrationHeight)
	require.Nil(h.t, s.State.Task.LastAttempt)
	require.Equal(h.t, lease.String(), s.State.Funding.LeasePaid)
	require.Equal(h.t, attempt.Mul(sdkmath.NewIntFromUint64(attempts)).String(), s.State.Funding.Available)
}
func (h *nativeDCADemo) snapshot(label string) demoSnapshot {
	h.t.Helper()
	s := demoSnapshot{Label: label, Source: "rpc", Info: h.info(), Balances: map[string]sdk.Coins{}}
	var view struct {
		Order dcaOrderRecord `json:"order"`
	}
	h.smart(h.dca, map[string]any{"order": map[string]any{"order_id": 1}}, &view)
	s.State.Order = view.Order
	h.smart(h.dca, map[string]any{"task": map[string]any{"order_id": 1}}, &s.State.Task)
	h.smart(h.dca, map[string]any{"async": map[string]any{"task_funding": map[string]any{"task_id": view.Order.TaskID}}}, &s.State.Funding)
	for i := uint32(0); i < view.Order.SliceCount; i++ {
		var fill *dcaFill
		h.smart(h.dca, map[string]any{"fill": map[string]any{"order_id": 1, "slice_index": i}}, &fill)
		s.Fills = append(s.Fills, fill)
	}
	for _, a := range []sdk.AccAddress{dcaAddress(h.key), dcaAddress(h.sponsor), h.dca, h.exchange, authtypes.NewModuleAddress(async.ModuleName), authtypes.NewModuleAddress(authtypes.FeeCollectorName)} {
		s.Balances[a.String()] = h.account(a).Balances
	}
	o := s.State.Order
	require.Equal(h.t, o.InitialPrincipal, dcaInt(h.t, o.Spent).Add(dcaInt(h.t, o.Remaining)).Add(dcaInt(h.t, o.Reclaimed)).String())
	require.JSONEq(h.t, "null", string(o.Pending))
	require.Equal(h.t, fmt.Sprint(10*o.FilledCount), o.Spent)
	require.Equal(h.t, fmt.Sprint(15*o.FilledCount), o.Received)
	require.Equal(h.t, dcaInt(h.t, o.Remaining).String(), s.Balances[h.dca.String()].AmountOf("uinput").String())
	require.Equal(h.t, sdkmath.NewInt(1_000_000).Add(dcaInt(h.t, o.Received)).String(), s.Balances[dcaAddress(h.sponsor).String()].AmountOf("uout").String())
	var previous uint64
	for i, fill := range s.Fills {
		if uint32(i) < o.FilledCount {
			require.NotNil(h.t, fill)
			require.Equal(h.t, uint32(i), fill.Receipt.SliceIndex)
			require.Equal(h.t, o.TaskID, fill.Receipt.Correlation.WorkflowID)
			require.Equal(h.t, "15", fill.Receipt.Output.Amount.String())
			if i == 0 {
				require.GreaterOrEqual(h.t, fill.Height, o.RegistrationHeight+o.IntervalBlocks)
			} else {
				require.GreaterOrEqual(h.t, fill.Height, previous+o.IntervalBlocks)
			}
			previous = fill.Height
		} else {
			require.Nil(h.t, fill)
		}
	}
	h.receipt.Snapshots = append(h.receipt.Snapshots, s)
	h.t.Logf("%s H=%d order=%d task=%d business_filled=%d/%d host=%s pause=%s principal_remaining=%s spent=%s reclaimed=%s gas_available=%s gas_fees=%s", label, s.Info.Height, o.OrderID, o.TaskID, o.FilledCount, o.SliceCount, s.State.Task.Status.Kind, s.State.Task.Status.Reason, o.Remaining, o.Spent, o.Reclaimed, s.State.Funding.Available, s.State.Funding.Fees)
	return s
}
func (h *nativeDCADemo) until(label string, predicate func(demoSnapshot) bool) demoSnapshot {
	for i := 0; i < 60; i++ {
		s := h.snapshot(label)
		if predicate(s) {
			return s
		}
		h.step()
	}
	h.t.Fatal("bounded block count exhausted: " + label)
	return demoSnapshot{}
}
func (h *nativeDCADemo) control(name string) {
	h.execute(h.key, map[string]any{"async": map[string]any{name: map[string]any{"task_id": 1}}}, sdk.Coins{})
}
func (h *nativeDCADemo) reclaim() {
	h.execute(h.key, map[string]any{"reclaim_principal": map[string]any{"order_id": 1}}, sdk.Coins{})
}
func (h *nativeDCADemo) finish() {
	require.NoError(h.t, h.child.stop())
	select {
	case <-h.child.done:
		h.receipt.ChildExited = true
	default:
		h.t.Fatal("owned child did not exit")
	}
	require.NoError(h.t, h.child.err, "owned validator exited cleanly")
	h.finished = true
}

func runDCAScenario(t *testing.T, h *nativeDCADemo, name string) {
	if name != "liquidity" {
		h.liquidity(1000)
	}
	interval := uint64(3)
	if name == "cancel" {
		interval = 8
	}
	attempts := uint64(12)
	if name == "funding" {
		attempts = 1
	}
	h.create(3, interval, attempts)
	switch name {
	case "happy":
		filled := h.until("scheduled", func(s demoSnapshot) bool { return s.State.Order.FilledCount == 3 })
		require.NotEqual(t, "completed", filled.State.Task.Status.Kind)
		require.NotNil(t, filled.State.Task.Inbox)
		require.NotNil(t, filled.State.Task.Inbox.Success)
		completed := h.until("consume final receipt", func(s demoSnapshot) bool { return s.State.Task.Status.Kind == "completed" })
		require.Equal(t, "0", completed.State.Funding.Available)
		require.Equal(t, "0", completed.State.Funding.Reserved)
		require.Equal(t, uint32(3), completed.State.Order.FilledCount)
		refund := dcaInt(t, filled.State.Funding.Available).Sub(dcaInt(t, completed.State.Funding.Fees).Sub(dcaInt(t, filled.State.Funding.Fees)))
		require.Equal(t, filled.Balances[dcaAddress(h.key).String()].AmountOf("ugas").Add(refund).String(), completed.Balances[dcaAddress(h.key).String()].AmountOf("ugas").String())
	case "funding":
		paused := h.until("wait for insufficient gas", func(s demoSnapshot) bool { return s.State.Task.Status.Kind == "paused" })
		require.Equal(t, "insufficient_funds", paused.State.Task.Status.Reason)
		require.Zero(t, paused.State.Order.FilledCount)
		quote := dcaInt(t, paused.State.Funding.MaxAttemptCharge).MulRaw(3)
		h.execute(h.sponsor, map[string]any{"async": map[string]any{"fund_task": map[string]any{"task_id": 1}}}, sdk.NewCoins(sdk.NewCoin("ugas", quote)))
		topped := h.snapshot("sponsor funded, still paused")
		require.Equal(t, paused.State.Order, topped.State.Order)
		require.Equal(t, paused.State.Task, topped.State.Task)
		require.Equal(t, dcaAddress(h.key).String(), topped.State.Funding.Control)
		require.Equal(t, dcaAddress(h.key).String(), topped.State.Funding.Refund)
		require.Equal(t, dcaInt(t, paused.State.Funding.Available).Add(quote).String(), topped.State.Funding.Available)
		h.control("resume_task")
		h.snapshot("owner resumed")
		h.until("funded completion", func(s demoSnapshot) bool { return s.State.Task.Status.Kind == "completed" })
	case "liquidity":
		paused := h.until("empty output liquidity", func(s demoSnapshot) bool { return s.State.Task.Status.Kind == "paused" })
		require.Equal(t, "message_failure", paused.State.Task.Status.Reason)
		require.Zero(t, paused.State.Order.FilledCount)
		require.True(t, dcaInt(t, paused.State.Funding.Fees).IsPositive())
		h.liquidity(1000)
		replenished := h.snapshot("replenished, still paused")
		require.Equal(t, paused.State.Order, replenished.State.Order)
		require.Equal(t, paused.State.Task, replenished.State.Task)
		h.control("resume_task")
		h.snapshot("owner resumed after liquidity repair")
		h.until("liquidity recovery completion", func(s demoSnapshot) bool { return s.State.Task.Status.Kind == "completed" })
	case "cancel":
		filled := h.until("one fill before cancellation", func(s demoSnapshot) bool { return s.State.Order.FilledCount == 1 })
		h.control("cancel_task")
		cancelled := h.snapshot("owner cancelled between fills")
		require.Equal(t, uint32(1), cancelled.State.Order.FilledCount)
		require.Equal(t, filled.State.Order, cancelled.State.Order)
		require.Equal(t, "cancelled", cancelled.State.Task.Status.Kind)
		h.reclaim()
		reclaimed := h.snapshot("explicit principal reclaim")
		require.Equal(t, "20", reclaimed.State.Order.Reclaimed)
		require.Equal(t, "0", reclaimed.State.Order.Remaining)
	case "restart":
		require.NoError(t, writeDemoMarker(filepath.Join(h.home, "fill-target.json"), demoFillGate{Contract: h.dca.String(), OrderID: 1, Count: 1}))
		before := h.until("before restart", func(s demoSnapshot) bool { return s.State.Order.FilledCount == 1 })
		require.NotNil(t, before.State.Task.Inbox)
		require.NotNil(t, before.State.Task.Inbox.Success)
		var committed int64
		require.NoError(t, readDemoMarker(filepath.Join(h.home, "fill-committed.json"), &committed))
		require.Equal(t, before.Info.Height, committed)
		h.allow(committed + 1)
		h.waitGate(committed + 1)
		require.NoError(t, h.child.stop())
		require.NoError(t, h.child.err, "owned validator stops cleanly at gate")
		require.NoError(t, os.Remove(filepath.Join(h.home, "gate-entered.json")))
		_, statErr := os.Stat(filepath.Join(h.home, "restored-state.json"))
		require.True(t, os.IsNotExist(statErr), "fresh startup evidence must not pre-exist")
		h.start()
		h.receipt.Restarts++
		var startup demoStartupEvidence
		h.wait("fresh child committed startup queries before handshake replay", func() bool {
			return readDemoMarker(filepath.Join(h.home, "restored-state.json"), &startup) == nil && startup.PID == h.child.command.Process.Pid && startup.Snapshot.Info.Height == committed
		})
		h.receipt.RestartPID = startup.PID
		after := startup.Snapshot
		h.waitGate(committed + 1)
		h.receipt.Snapshots = append(h.receipt.Snapshots, after)
		h.t.Logf("same-home startup queries H=%d filled=%d host=%s app_hash=%s before gate release", after.Info.Height, after.State.Order.FilledCount, after.State.Task.Status.Kind, after.Info.AppHash)
		before.Label = after.Label
		before.Source = after.Source
		require.Equal(t, before, after, "identical persisted evidence before any next FinalizeBlock")
		require.NoError(t, writeDemoMarker(filepath.Join(h.home, "release-fill.json"), true))
		h.wait("live RPC after gated handshake consumes receipt", func() bool {
			var info demoInfo
			return h.query("info", demoQuery{}, &info) == nil && info.Height == committed+1
		})
		h.waitGate(committed + 2)
		consumed := h.snapshot("released, receipt consumed once")
		require.Equal(t, uint32(1), consumed.State.Order.FilledCount)
		require.Equal(t, before.State.Order, consumed.State.Order)
		require.Equal(t, before.Fills, consumed.Fills)
		h.until("restarted completion", func(s demoSnapshot) bool { return s.State.Task.Status.Kind == "completed" })
	}
}
