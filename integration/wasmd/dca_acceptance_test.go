//go:build upstream

package async_test

import (
	"bytes"
	"encoding/json"
	"fmt"
	"math/big"
	"math/rand"
	"os"
	"strconv"
	"testing"
	"time"

	sdkmath "cosmossdk.io/math"
	async "cosmwasm-durable-async/integration/wasmd"
	wasmapp "github.com/CosmWasm/wasmd/app"
	wasmkeeper "github.com/CosmWasm/wasmd/x/wasm/keeper"
	wasmtypes "github.com/CosmWasm/wasmd/x/wasm/types"
	wasmvm "github.com/CosmWasm/wasmvm/v2"
	vmtypes "github.com/CosmWasm/wasmvm/v2/types"
	abci "github.com/cometbft/cometbft/abci/types"
	cmted "github.com/cometbft/cometbft/crypto/ed25519"
	cmtproto "github.com/cometbft/cometbft/proto/tendermint/types"
	cmttypes "github.com/cometbft/cometbft/types"
	"github.com/cosmos/cosmos-sdk/crypto/keys/secp256k1"
	sims "github.com/cosmos/cosmos-sdk/testutil/sims"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	banktypes "github.com/cosmos/cosmos-sdk/x/bank/types"
	"github.com/stretchr/testify/require"
)

const dcaAllocation uint64 = 29_824
const dcaPollCap uint64 = 100_000_000

type dcaAcceptanceHarness struct {
	*appHarness
	sponsor    *secp256k1.PrivKey
	exchange   sdk.AccAddress
	dca        sdk.AccAddress
	boundary   *dcaBoundary
	pollEvents []sdk.Event
}

// The baseline hook does not copy scheduler events into the ABCI EndBlock
// response (ModuleManager replaces its event manager). Capture the actual
// finalize context before Commit; this adds observation, not runtime behavior.
func (h *dcaAcceptanceHarness) block(t *testing.T, txs ...[]byte) *abci.ResponseFinalizeBlock {
	return h.finalize(t, true, txs...)
}
func (h *dcaAcceptanceHarness) finalize(t *testing.T, success bool, txs ...[]byte) *abci.ResponseFinalizeBlock {
	t.Helper()
	height := h.app.LastBlockHeight() + 1
	req := &abci.RequestFinalizeBlock{Height: height, Time: acceptanceTime.Add(time.Duration(height) * time.Second), Txs: txs, Hash: bytes.Repeat([]byte{byte(height)}, 32)}
	r, err := h.app.FinalizeBlock(req)
	require.NoError(t, err)
	if success {
		for _, tx := range r.TxResults {
			require.Zero(t, tx.Code, tx.Log)
		}
	}
	ctx := h.app.GetContextForFinalizeBlock(nil)
	h.pollEvents = append([]sdk.Event(nil), ctx.EventManager().Events()...)
	used := ctx.BlockGasMeter().GasConsumed()
	require.LessOrEqual(t, used, uint64(400_000_000))
	h.blockGas = append(h.blockGas, used)
	_, err = h.app.Commit()
	require.NoError(t, err)
	h.records = append(h.records, req)
	h.roots = append(h.roots, bytes.Clone(h.app.LastCommitID().Hash))
	h.results = append(h.results, r)
	return r
}

// These are test-injected boundary failures, not natural guest errors or
// exploit claims. Every override runs the genuine Wasm first, keeping its gas.
type dcaBoundary struct {
	target                   string
	exchange                 string
	mode                     string
	trace                    []string
	selfSender               string
	exchangeBankThenCallback bool
	deliverReached           bool
	replyReached             bool
	replyError               string
}
type dcaBoundaryEngine struct {
	wasmtypes.WasmEngine
	fault *dcaBoundary
}

func (g dcaBoundaryEngine) Execute(c wasmvm.Checksum, e vmtypes.Env, i vmtypes.MessageInfo, msg []byte, s wasmvm.KVStore, a wasmvm.GoAPI, q wasmvm.Querier, m wasmvm.GasMeter, l uint64, d vmtypes.UFraction) (*vmtypes.ContractResult, uint64, error) {
	r, used, err := g.WasmEngine.Execute(c, e, i, msg, s, a, q, m, l, d)
	if err == nil && r != nil && r.Ok != nil {
		if e.Contract.Address == g.fault.target && bytes.Contains(msg, []byte(`"dispatch_slice"`)) {
			g.fault.trace = append(g.fault.trace, "self_dispatch")
			g.fault.selfSender = i.Sender
		}
		if e.Contract.Address == g.fault.exchange && bytes.Contains(msg, []byte(`"swap"`)) {
			g.fault.trace = append(g.fault.trace, "exchange")
			g.fault.exchangeBankThenCallback = len(r.Ok.Messages) >= 2 && r.Ok.Messages[0].Msg.Bank != nil && r.Ok.Messages[1].Msg.Wasm != nil
		}
	}
	if e.Contract.Address == g.fault.target && bytes.Contains(msg, []byte(`"swap_result"`)) && err == nil && r != nil && r.Ok != nil {
		g.fault.trace = append(g.fault.trace, "callback")
		for index, sub := range r.Ok.Messages {
			if len(sub.Msg.Custom) > 0 {
				g.fault.deliverReached = true
				if g.fault.mode == "host_delivery" || g.fault.mode == "result_2048" || g.fault.mode == "result_2049" {
					var envelope struct {
						Deliver async.DeliverMsg `json:"deliver"`
					}
					if json.Unmarshal(sub.Msg.Custom, &envelope) != nil {
						panic("test fixture cannot decode genuine delivery")
					}
					if g.fault.mode == "host_delivery" {
						envelope.Deliver.WaitSequence++
					} else {
						size := 2048
						if g.fault.mode == "result_2049" {
							size = 2049
						}
						if envelope.Deliver.Resolution.Success == nil || len(envelope.Deliver.Resolution.Success.Data) >= size {
							panic("unexpected canonical receipt size in result-boundary fixture")
						}
						// Whitespace preserves the genuine receipt's typed meaning.
						envelope.Deliver.Resolution.Success.Data = append(envelope.Deliver.Resolution.Success.Data, bytes.Repeat([]byte(" "), size-len(envelope.Deliver.Resolution.Success.Data))...)
					}
					raw, marshalErr := json.Marshal(envelope)
					if marshalErr != nil {
						panic(marshalErr)
					}
					r.Ok.Messages[index].Msg.Custom = raw
				}
			}
		}
	}
	return r, used, err
}
func (g dcaBoundaryEngine) Reply(c wasmvm.Checksum, e vmtypes.Env, reply vmtypes.Reply, s wasmvm.KVStore, a wasmvm.GoAPI, q wasmvm.Querier, m wasmvm.GasMeter, l uint64, d vmtypes.UFraction) (*vmtypes.ContractResult, uint64, error) {
	r, used, err := g.WasmEngine.Reply(c, e, reply, s, a, q, m, l, d)
	if e.Contract.Address == g.fault.target && reply.ID == 2 {
		g.fault.replyReached = true
		if r != nil {
			g.fault.replyError = r.Err
		}
		g.fault.trace = append(g.fault.trace, "dispatch_reply")
		if g.fault.mode == "late_reply" && err == nil && r != nil && r.Ok != nil {
			return &vmtypes.ContractResult{Err: "injected late reply rejection after genuine Wasm reply"}, used, nil
		}
	}
	return r, used, err
}

type dcaOrderRecord struct {
	OrderID            uint64          `json:"order_id"`
	TaskID             uint64          `json:"task_id"`
	Owner              string          `json:"owner"`
	Beneficiary        string          `json:"beneficiary"`
	Exchange           string          `json:"exchange"`
	InputPerSlice      sdk.Coin        `json:"input_per_slice"`
	OutputDenom        string          `json:"output_denom"`
	SliceCount         uint32          `json:"slice_count"`
	FilledCount        uint32          `json:"filled_count"`
	MinimumOutput      string          `json:"minimum_output"`
	InitialPrincipal   string          `json:"initial_principal"`
	Remaining          string          `json:"remaining"`
	Reclaimed          string          `json:"reclaimed"`
	Spent              string          `json:"spent"`
	Received           string          `json:"received"`
	IntervalBlocks     uint64          `json:"interval_blocks"`
	RegistrationHeight uint64          `json:"registration_height"`
	ExpiresAt          uint64          `json:"expires_at"`
	Pending            json.RawMessage `json:"pending"`
	LastFillHeight     *uint64         `json:"last_fill_height"`
}
type dcaCorrelation struct {
	WorkflowID   uint64 `json:"workflow_id"`
	WaitSequence uint64 `json:"wait_sequence"`
}
type dcaReceipt struct {
	OrderID     uint64         `json:"order_id"`
	SliceIndex  uint32         `json:"slice_index"`
	Correlation dcaCorrelation `json:"correlation"`
	Input       sdk.Coin       `json:"input"`
	Output      sdk.Coin       `json:"output"`
	Beneficiary string         `json:"beneficiary"`
}
type dcaFill struct {
	Receipt dcaReceipt `json:"receipt"`
	Height  uint64     `json:"height"`
}
type dcaFunding struct {
	TaskID           uint64           `json:"task_id"`
	Contract         string           `json:"contract"`
	Control          string           `json:"control"`
	Refund           string           `json:"refund"`
	Available        string           `json:"available"`
	Reserved         string           `json:"reserved"`
	Fees             string           `json:"cumulative_execution_fees"`
	GasPerPoll       uint64           `json:"gas_per_poll"`
	MaxGasPrice      string           `json:"max_gas_price"`
	MaxAttemptCharge string           `json:"max_attempt_charge"`
	ExpiresAt        uint64           `json:"expires_at"`
	Status           async.TaskStatus `json:"status"`
	AllocatedBytes   uint64           `json:"allocated_bytes"`
	LeasePaid        string           `json:"storage_lease_paid"`
}
type dcaFundingConfig struct {
	Denom         string `json:"denom"`
	GasPrice      string `json:"gas_price"`
	MinBasePrice  string `json:"min_base_price"`
	MaxGasPerPoll uint64 `json:"max_gas_per_poll"`
}
type dcaState struct {
	Order   dcaOrderRecord
	Task    async.TaskResponse
	Funding dcaFunding
}

func dcaAddress(key *secp256k1.PrivKey) sdk.AccAddress { return sdk.AccAddress(key.PubKey().Address()) }

func newDCAAcceptanceApp(t *testing.T) *dcaAcceptanceHarness {
	t.Helper()
	h := &dcaAcceptanceHarness{appHarness: &appHarness{home: t.TempDir(), key: secp256k1.GenPrivKeyFromSecret([]byte("DCA owner"))}, sponsor: secp256k1.GenPrivKeyFromSecret([]byte("DCA sponsor"))}
	h.boundary = &dcaBoundary{}
	h.app, h.cleanupVM = openAcceptanceApp(t, h.home, wasmkeeper.WithWasmEngineDecorator(func(engine wasmtypes.WasmEngine) wasmtypes.WasmEngine { return dcaBoundaryEngine{engine, h.boundary} }))
	t.Cleanup(func() { h.cleanupVM(); require.NoError(t, h.app.Close()) })
	accounts := []authtypes.GenesisAccount{}
	balances := []banktypes.Balance{}
	for _, key := range []*secp256k1.PrivKey{h.key, h.sponsor} {
		a := dcaAddress(key)
		accounts = append(accounts, authtypes.NewBaseAccount(a, key.PubKey(), uint64(len(accounts)), 0))
		balances = append(balances, banktypes.Balance{Address: a.String(), Coins: sdk.NewCoins(sdk.NewInt64Coin("stake", 1_000_000_000), sdk.NewInt64Coin("ugas", 1_000_000_000), sdk.NewInt64Coin("uinput", 1_000_000), sdk.NewInt64Coin("uout", 1_000_000))})
	}
	validator := cmttypes.NewValidator(cmted.GenPrivKeyFromSecret([]byte("DCA validator")).PubKey(), 1)
	genesis, err := wasmapp.GenesisStateWithValSet(h.app.AppCodec(), h.app.DefaultGenesis(), cmttypes.NewValidatorSet([]*cmttypes.Validator{validator}), accounts, balances...)
	require.NoError(t, err)
	raw, err := json.Marshal(genesis)
	require.NoError(t, err)
	params := *sims.DefaultConsensusParams
	block := *params.Block
	block.MaxGas = 400_000_000
	params.Block = &block
	_, err = h.app.InitChain(&abci.RequestInitChain{ChainId: acceptanceChain, Time: acceptanceTime, InitialHeight: 1, ConsensusParams: &params, AppStateBytes: raw})
	require.NoError(t, err)
	return h
}

func dcaSignedTx(t *testing.T, h *dcaAcceptanceHarness, key *secp256k1.PrivKey, msg sdk.Msg) []byte {
	t.Helper()
	account := h.app.AccountKeeper.GetAccount(h.app.NewUncachedContext(false, cmtproto.Header{}), dcaAddress(key))
	if account == nil {
		account = h.app.AccountKeeper.GetAccount(h.app.NewContext(false), dcaAddress(key))
	}
	tx, err := sims.GenSignedMockTx(rand.New(rand.NewSource(1)), h.app.TxConfig(), []sdk.Msg{msg}, sdk.NewCoins(sdk.NewInt64Coin("stake", 1)), 100_000_000, acceptanceChain, []uint64{account.GetAccountNumber()}, []uint64{account.GetSequence()}, key)
	require.NoError(t, err)
	raw, err := h.app.TxConfig().TxEncoder()(tx)
	require.NoError(t, err)
	return raw
}
func executeDCA(t *testing.T, h *dcaAcceptanceHarness, key *secp256k1.PrivKey, contract sdk.AccAddress, msg any, coins sdk.Coins) []byte {
	t.Helper()
	raw, err := json.Marshal(msg)
	require.NoError(t, err)
	return dcaSignedTx(t, h, key, &wasmtypes.MsgExecuteContract{Sender: dcaAddress(key).String(), Contract: contract.String(), Msg: raw, Funds: coins})
}
func dcaQuery(t *testing.T, h *dcaAcceptanceHarness, contract sdk.AccAddress, msg any, out any) {
	t.Helper()
	raw, err := json.Marshal(msg)
	require.NoError(t, err)
	height := h.app.LastBlockHeight()
	answer, err := h.app.WasmKeeper.QuerySmart(h.app.NewUncachedContext(false, cmtproto.Header{Height: height, Time: acceptanceTime.Add(time.Duration(height) * time.Second), ChainID: acceptanceChain}), contract, raw)
	require.NoError(t, err)
	require.NoError(t, json.Unmarshal(answer, out))
}
func readDCAState(t *testing.T, h *dcaAcceptanceHarness, orderID uint64) dcaState {
	t.Helper()
	var view struct {
		Order dcaOrderRecord `json:"order"`
	}
	dcaQuery(t, h, h.dca, map[string]any{"order": map[string]any{"order_id": orderID}}, &view)
	s := dcaState{Order: view.Order}
	dcaQuery(t, h, h.dca, map[string]any{"task": map[string]any{"order_id": orderID}}, &s.Task)
	dcaQuery(t, h, h.dca, map[string]any{"async": map[string]any{"task_funding": map[string]any{"task_id": s.Order.TaskID}}}, &s.Funding)
	require.Equal(t, s.Order.InitialPrincipal, dcaInt(t, s.Order.Spent).Add(dcaInt(t, s.Order.Remaining)).Add(dcaInt(t, s.Order.Reclaimed)).String())
	return s
}
func dcaInt(t *testing.T, s string) sdkmath.Int {
	t.Helper()
	n, ok := sdkmath.NewIntFromString(s)
	require.True(t, ok, "integer %q", s)
	return n
}
func dcaBalance(h *dcaAcceptanceHarness, address sdk.AccAddress, denom string) sdkmath.Int {
	return h.app.BankKeeper.GetBalance(h.app.NewUncachedContext(false, cmtproto.Header{}), address, denom).Amount
}
func dcaBusinessBalances(h *dcaAcceptanceHarness) []string {
	values := []string{}
	for _, a := range []sdk.AccAddress{dcaAddress(h.key), dcaAddress(h.sponsor), h.dca, h.exchange} {
		for _, d := range []string{"uinput", "uout"} {
			values = append(values, dcaBalance(h, a, d).String())
		}
	}
	return values
}
func dcaAllBalances(h *dcaAcceptanceHarness) map[string]string {
	values := map[string]string{}
	for _, a := range []sdk.AccAddress{dcaAddress(h.key), dcaAddress(h.sponsor), h.dca, h.exchange, authtypes.NewModuleAddress(async.ModuleName), authtypes.NewModuleAddress(authtypes.FeeCollectorName)} {
		values[a.String()] = h.app.BankKeeper.GetAllBalances(h.app.NewUncachedContext(false, cmtproto.Header{}), a).String()
	}
	return values
}
func dcaContractStorage(h *dcaAcceptanceHarness, contract sdk.AccAddress) map[string]string {
	values := map[string]string{}
	h.app.WasmKeeper.IterateContractState(h.app.NewUncachedContext(false, cmtproto.Header{}), contract, func(k, v []byte) bool { values[string(k)] = string(v); return false })
	return values
}
func dcaInstantiate(t *testing.T, h *dcaAcceptanceHarness, code uint64, msg any, label string) sdk.AccAddress {
	t.Helper()
	raw, err := json.Marshal(msg)
	require.NoError(t, err)
	r := h.block(t, dcaSignedTx(t, h, h.key, &wasmtypes.MsgInstantiateContract{Sender: dcaAddress(h.key).String(), CodeID: code, Label: label, Msg: raw}))
	for _, e := range r.TxResults[0].Events {
		if e.Type == "instantiate" {
			for _, a := range e.Attributes {
				if a.Key == "_contract_address" {
					return sdk.MustAccAddressFromBech32(a.Value)
				}
			}
		}
	}
	t.Fatal("missing instantiate address")
	return nil
}
func deployDCA(t *testing.T, h *dcaAcceptanceHarness) { deployDCAExchange(t, h, "mock_exchange", "") }
func deployDCAExchange(t *testing.T, h *dcaAcceptanceHarness, artifact, mode string) {
	t.Helper()
	for _, name := range []string{artifact, "native_dca"} {
		code, err := os.ReadFile("../../artifacts/" + name + ".wasm")
		require.NoError(t, err, "required DCA artifact %s; run contracts/native-dca/build-wasmd.sh", name)
		h.block(t, dcaSignedTx(t, h, h.key, &wasmtypes.MsgStoreCode{Sender: dcaAddress(h.key).String(), WASMByteCode: code}))
	}
	terms := map[string]any{"input_denom": "uinput", "output_denom": "uout", "numerator": "3", "denominator": "2"}
	if mode != "" {
		terms["mode"] = mode
	}
	h.exchange = dcaInstantiate(t, h, 1, terms, "DCA exchange fixture")
	h.dca = dcaInstantiate(t, h, 2, map[string]any{"exchange": h.exchange.String(), "fee_denom": "ugas"}, "native DCA")
	h.owner = h.dca
	h.boundary.target = h.dca.String()
	h.boundary.exchange = h.exchange.String()
}
func dcaLiquidity(t *testing.T, h *dcaAcceptanceHarness, amount int64) {
	h.block(t, dcaSignedTx(t, h, h.key, &banktypes.MsgSend{FromAddress: dcaAddress(h.key).String(), ToAddress: h.exchange.String(), Amount: sdk.NewCoins(sdk.NewInt64Coin("uout", amount))}))
}

// Rent/reserve are absent from the public funding DTO. Read live keeper config
// for those fields and assert the pinned, uncalibrated harness policy explicitly.
func dcaDepositQuote(t *testing.T, h *dcaAcceptanceHarness, registration, expiry, attempts uint64) (sdkmath.Int, sdkmath.Int, sdkmath.Int) {
	t.Helper()
	var config dcaFundingConfig
	dcaQuery(t, h, h.dca, map[string]any{"async": map[string]any{"funding_config": map[string]any{}}}, &config)
	require.Equal(t, "ugas", config.Denom)
	require.Equal(t, "0.001", config.MinBasePrice)
	require.Equal(t, dcaPollCap, config.MaxGasPerPoll)
	c := h.app.AsyncKeeper.Config
	require.Equal(t, uint64(70_000_000), c.HostReserve)
	require.Equal(t, "1", c.StorageByteBlockPrice.String())
	require.Equal(t, "100", c.CleanupPrice.String())
	require.Greater(t, expiry, registration)
	price, err := sdkmath.LegacyNewDecFromStr(config.GasPrice)
	require.NoError(t, err)
	require.True(t, price.IsPositive())
	leaseBig := new(big.Int).Mul(new(big.Int).SetUint64(dcaAllocation), new(big.Int).SetUint64(expiry-registration))
	leaseBig.Mul(leaseBig, c.StorageByteBlockPrice.BigInt())
	leaseBig.Add(leaseBig, c.CleanupPrice.BigInt())
	require.LessOrEqual(t, leaseBig.BitLen(), 128)
	lease := sdkmath.NewIntFromBigInt(leaseBig)
	attempt := price.MulInt(sdkmath.NewIntFromUint64(dcaPollCap)).Ceil().TruncateInt()
	total := lease.Add(attempt.Mul(sdkmath.NewIntFromUint64(attempts)))
	require.LessOrEqual(t, total.BigInt().BitLen(), 128)
	return total, lease, attempt
}
func dcaCreate(t *testing.T, h *dcaAcceptanceHarness, slices uint32, interval, expiry, attempts uint64, minimum string) dcaState {
	t.Helper()
	registration := uint64(h.app.LastBlockHeight() + 1)
	deposit, lease, attempt := dcaDepositQuote(t, h, registration, expiry, attempts)
	msg := map[string]any{"create_order": map[string]any{"input_per_slice": sdk.NewInt64Coin("uinput", 10), "slice_count": slices, "minimum_output": minimum, "interval_blocks": interval, "beneficiary": dcaAddress(h.sponsor).String(), "gas": sdk.NewCoin("ugas", deposit), "gas_per_poll": dcaPollCap, "max_gas_price": "1", "expires_at": expiry, "allocated_bytes": dcaAllocation}}
	h.block(t, executeDCA(t, h, h.key, h.dca, msg, sdk.NewCoins(sdk.NewInt64Coin("uinput", 10*int64(slices)), sdk.NewCoin("ugas", deposit))))
	s := readDCAState(t, h, 1)
	require.Equal(t, registration, s.Order.RegistrationHeight)
	require.Nil(t, s.Task.LastAttempt)
	require.Equal(t, lease.String(), s.Funding.LeasePaid)
	require.Equal(t, attempt.String(), s.Funding.MaxAttemptCharge)
	require.Equal(t, deposit.Sub(lease).String(), s.Funding.Available)
	require.Equal(t, "0", s.Funding.Fees)
	return s
}
func dcaFillAt(t *testing.T, h *dcaAcceptanceHarness, index uint32) *dcaFill {
	t.Helper()
	var f *dcaFill
	dcaQuery(t, h, h.dca, map[string]any{"fill": map[string]any{"order_id": 1, "slice_index": index}}, &f)
	return f
}
func dcaExchangeReceipt(t *testing.T, h *dcaAcceptanceHarness, index uint32) *dcaReceipt {
	t.Helper()
	var f *dcaReceipt
	dcaQuery(t, h, h.exchange, map[string]any{"receipt": map[string]any{"dca": h.dca.String(), "order_id": 1, "slice_index": index}}, &f)
	return f
}
func dcaPollFees(t *testing.T, h *dcaAcceptanceHarness, r *abci.ResponseFinalizeBlock, old dcaState) sdkmath.Int {
	t.Helper()
	sum := sdkmath.ZeroInt()
	polls := 0
	for _, e := range h.pollEvents {
		if e.Type != "async_poll" {
			continue
		}
		a := map[string]string{}
		for _, v := range e.Attributes {
			a[v.Key] = v.Value
		}
		if a["contract"] != h.dca.String() {
			continue
		}
		polls++
		require.Equal(t, fmt.Sprint(old.Order.TaskID), a["task_id"])
		execution, err := strconv.ParseUint(a["execution_gas"], 10, 64)
		require.NoError(t, err)
		chargedHost, err := strconv.ParseUint(a["charged_host_gas"], 10, 64)
		require.NoError(t, err)
		price, err := sdkmath.LegacyNewDecFromStr(a["price"])
		require.NoError(t, err)
		require.True(t, price.IsPositive())
		require.Positive(t, execution)
		fee := price.MulInt(sdkmath.NewIntFromUint64(execution + chargedHost)).Ceil().TruncateInt()
		require.Equal(t, fee.String(), a["fee"])
		sum = sum.Add(fee)
		t.Logf("height=%d execution_gas=%d charged_host_gas=%d price=%s fee=%s pause=%s", h.app.LastBlockHeight(), execution, chargedHost, a["price"], a["fee"], a["pause_reason"])
	}
	require.LessOrEqual(t, polls, 1, "no same-block second poll")
	after := readDCAState(t, h, old.Order.OrderID)
	require.Equal(t, dcaInt(t, old.Funding.Fees).Add(sum).String(), after.Funding.Fees)
	return sum
}
func dcaRunTo(t *testing.T, h *dcaAcceptanceHarness, height uint64) {
	t.Helper()
	for uint64(h.app.LastBlockHeight()) < height {
		old := readDCAState(t, h, 1)
		r := h.block(t)
		dcaPollFees(t, h, r, old)
	}
}

func TestDCAAcceptanceAtomicReceipt(t *testing.T) {
	h := newDCAAcceptanceApp(t)
	deployDCA(t, h)
	dcaLiquidity(t, h, 1_000)
	before := dcaBusinessBalances(h)
	gasBefore := dcaBalance(h, dcaAddress(h.key), "ugas")
	stakeBefore := dcaBalance(h, dcaAddress(h.key), "stake")
	s := dcaCreate(t, h, 3, 3, 60, 12, "14")
	require.Equal(t, "30", dcaBalance(h, h.dca, "uinput").String())
	require.Equal(t, "0", dcaBalance(h, h.dca, "ugas").String())
	for i := uint32(0); i < 3; i++ {
		target := s.Order.RegistrationHeight + 3
		if s.Order.LastFillHeight != nil {
			target = *s.Order.LastFillHeight + 3
		}
		dcaRunTo(t, h, target)
		s = readDCAState(t, h, 1)
		require.Equal(t, []string{"self_dispatch", "exchange", "callback", "dispatch_reply"}, h.boundary.trace)
		require.Equal(t, h.dca.String(), h.boundary.selfSender)
		require.True(t, h.boundary.exchangeBankThenCallback)
		require.True(t, h.boundary.deliverReached)
		require.True(t, h.boundary.replyReached)
		h.boundary.trace = nil
		require.Equal(t, i+1, s.Order.FilledCount)
		require.Equal(t, "runnable", s.Task.Status.Kind)
		require.NotNil(t, s.Task.Inbox)
		require.NotNil(t, s.Task.Inbox.Success)
		require.Equal(t, target, *s.Task.LastAttempt)
		require.Equal(t, []byte("null"), []byte(s.Order.Pending))
		fill := dcaFillAt(t, h, i)
		require.NotNil(t, fill)
		require.Equal(t, fill.Receipt, *dcaExchangeReceipt(t, h, i))
		require.Equal(t, target, fill.Height)
		require.Equal(t, s.Order.TaskID, fill.Receipt.Correlation.WorkflowID)
		require.Equal(t, s.Task.WaitSequence, fill.Receipt.Correlation.WaitSequence)
		require.Equal(t, "15", fill.Receipt.Output.Amount.String())
		old := s
		r := h.block(t)
		dcaPollFees(t, h, r, old)
		s = readDCAState(t, h, 1)
		require.Equal(t, i+1, s.Order.FilledCount)
	}
	require.Equal(t, "completed", s.Task.Status.Kind)
	require.Equal(t, "30", s.Order.Spent)
	require.Equal(t, "45", s.Order.Received)
	require.Equal(t, "0", s.Order.Remaining)
	require.Equal(t, "0", s.Funding.Available)
	require.Equal(t, dcaInt(t, before[0]).SubRaw(30).String(), dcaBalance(h, dcaAddress(h.key), "uinput").String())
	require.Equal(t, dcaInt(t, before[3]).AddRaw(45).String(), dcaBalance(h, dcaAddress(h.sponsor), "uout").String())
	require.Equal(t, "30", dcaBalance(h, h.exchange, "uinput").String())
	require.Equal(t, "955", dcaBalance(h, h.exchange, "uout").String())
	require.Equal(t, gasBefore.Sub(dcaInt(t, s.Funding.LeasePaid)).Sub(dcaInt(t, s.Funding.Fees)).String(), dcaBalance(h, dcaAddress(h.key), "ugas").String())
	require.Equal(t, stakeBefore.SubRaw(1).String(), dcaBalance(h, dcaAddress(h.key), "stake").String(), "ordinary fee is separate")
}

func TestDCAAcceptanceFailureRollback(t *testing.T) {
	for _, tc := range []struct {
		name, artifact, mode, minimum string
		liquidity                     int64
	}{{"liquidity", "mock_exchange", "", "14", 0}, {"minimum", "mock_exchange", "", "16", 1000}, {"omitted", "dca_fault", "omitted", "14", 1000}, {"invalid", "dca_fault", "invalid", "14", 1000}, {"duplicate", "dca_fault", "duplicate", "14", 1000}, {"bank_failure", "dca_fault", "bank_failure", "14", 1000}, {"host_delivery", "mock_exchange", "", "14", 1000}, {"late_reply", "mock_exchange", "", "14", 1000}, {"result_2049", "mock_exchange", "", "14", 1000}} {
		t.Run(tc.name, func(t *testing.T) {
			h := newDCAAcceptanceApp(t)
			deployDCAExchange(t, h, tc.artifact, tc.mode)
			if tc.liquidity > 0 {
				dcaLiquidity(t, h, tc.liquidity)
			}
			s := dcaCreate(t, h, 3, 3, 60, 8, tc.minimum)
			dcaRunTo(t, h, s.Order.RegistrationHeight+2)
			if tc.name == "host_delivery" || tc.name == "late_reply" || tc.name == "result_2049" {
				h.boundary.mode = tc.name
			}
			before := readDCAState(t, h, 1)
			balances := dcaBusinessBalances(h)
			dcaStorage, exchangeStorage := dcaContractStorage(h, h.dca), dcaContractStorage(h, h.exchange)
			collector := dcaBalance(h, authtypes.NewModuleAddress(authtypes.FeeCollectorName), "ugas")
			r := h.block(t)
			fee := dcaPollFees(t, h, r, before)
			after := readDCAState(t, h, 1)
			require.True(t, fee.IsPositive())
			require.Equal(t, "paused", after.Task.Status.Kind)
			require.Equal(t, "message_failure", after.Task.Status.Reason)
			require.Equal(t, before.Order, after.Order)
			require.Equal(t, balances, dcaBusinessBalances(h))
			require.Equal(t, dcaStorage, dcaContractStorage(h, h.dca))
			require.Equal(t, exchangeStorage, dcaContractStorage(h, h.exchange))
			require.Nil(t, dcaFillAt(t, h, 0))
			require.Nil(t, dcaExchangeReceipt(t, h, 0))
			require.Equal(t, before.Task.State, after.Task.State)
			require.Equal(t, before.Task.WaitSequence, after.Task.WaitSequence)
			require.NotNil(t, after.Task.Inbox)
			require.NotNil(t, after.Task.Inbox.Timer)
			require.Equal(t, dcaInt(t, before.Funding.Available).Sub(fee).String(), after.Funding.Available)
			require.Equal(t, collector.Add(fee).String(), dcaBalance(h, authtypes.NewModuleAddress(authtypes.FeeCollectorName), "ugas").String())
			if tc.name == "omitted" {
				require.True(t, h.boundary.replyReached, "production dispatch reply guard was invoked")
				require.NotEmpty(t, h.boundary.replyError, "production reply rejects absent fill")
				require.False(t, h.boundary.deliverReached)
			}
			if tc.name == "host_delivery" {
				require.True(t, h.boundary.deliverReached)
			}
			if tc.name == "late_reply" {
				require.True(t, h.boundary.deliverReached)
				require.True(t, h.boundary.replyReached)
			}
			if tc.name == "liquidity" {
				dcaLiquidity(t, h, 1000)
				dcaResume(t, h, after.Order.TaskID)
				h.block(t)
				require.Equal(t, uint32(1), readDCAState(t, h, 1).Order.FilledCount)
			}
		})
	}
}
func TestDCAAcceptanceResultLimit(t *testing.T) {
	h := newDCAAcceptanceApp(t)
	deployDCA(t, h)
	dcaLiquidity(t, h, 1000)
	s := dcaCreate(t, h, 1, 3, 60, 5, "14")
	h.boundary.mode = "result_2048"
	dcaRunTo(t, h, s.Order.RegistrationHeight+3)
	filled := readDCAState(t, h, 1)
	require.Equal(t, uint32(1), filled.Order.FilledCount)
	require.NotNil(t, filled.Task.Inbox.Success)
	require.Len(t, filled.Task.Inbox.Success.Data, 2048)
	r := h.block(t)
	dcaPollFees(t, h, r, filled)
	after := readDCAState(t, h, 1)
	require.Equal(t, "completed", after.Task.Status.Kind)
	require.Equal(t, filled.Order, after.Order)
	require.Equal(t, "10", dcaBalance(h, h.exchange, "uinput").String())
}
func dcaResume(t *testing.T, h *dcaAcceptanceHarness, taskID uint64) {
	h.block(t, executeDCA(t, h, h.key, h.dca, map[string]any{"async": map[string]any{"resume_task": map[string]any{"task_id": taskID}}}, sdk.Coins{}))
}
func dcaCancel(t *testing.T, h *dcaAcceptanceHarness, taskID uint64) {
	h.block(t, executeDCA(t, h, h.key, h.dca, map[string]any{"async": map[string]any{"cancel_task": map[string]any{"task_id": taskID}}}, sdk.Coins{}))
}
func dcaReclaim(t *testing.T, h *dcaAcceptanceHarness) {
	h.block(t, executeDCA(t, h, h.key, h.dca, map[string]any{"reclaim_principal": map[string]any{"order_id": 1}}, sdk.Coins{}))
}
func dcaFund(t *testing.T, h *dcaAcceptanceHarness, key *secp256k1.PrivKey, taskID uint64, amount sdkmath.Int) {
	t.Helper()
	h.block(t, executeDCA(t, h, key, h.dca, map[string]any{"async": map[string]any{"fund_task": map[string]any{"task_id": taskID}}}, sdk.NewCoins(sdk.NewCoin("ugas", amount))))
}

func TestDCAAcceptanceFundingAndCancellation(t *testing.T) {
	t.Run("sponsor_topup_requires_owner_resume", func(t *testing.T) {
		h := newDCAAcceptanceApp(t)
		deployDCA(t, h)
		dcaLiquidity(t, h, 1000)
		s := dcaCreate(t, h, 3, 3, 60, 1, "14")
		dcaRunTo(t, h, s.Order.RegistrationHeight+3)
		paused := readDCAState(t, h, 1)
		require.Equal(t, "paused", paused.Task.Status.Kind)
		require.Equal(t, "insufficient_funds", paused.Task.Status.Reason)
		require.Zero(t, paused.Order.FilledCount)
		require.Nil(t, dcaFillAt(t, h, 0))
		quote := dcaInt(t, paused.Funding.MaxAttemptCharge)
		sponsorGas := dcaBalance(h, dcaAddress(h.sponsor), "ugas")
		dcaFund(t, h, h.sponsor, s.Order.TaskID, quote)
		topped := readDCAState(t, h, 1)
		require.Equal(t, paused.Order, topped.Order)
		require.Equal(t, paused.Task, topped.Task)
		require.Equal(t, dcaInt(t, paused.Funding.Available).Add(quote).String(), topped.Funding.Available)
		require.Equal(t, sponsorGas.Sub(quote).String(), dcaBalance(h, dcaAddress(h.sponsor), "ugas").String())
		require.Equal(t, dcaAddress(h.key).String(), topped.Funding.Control)
		require.Equal(t, dcaAddress(h.key).String(), topped.Funding.Refund)
		r := h.finalize(t, false, executeDCA(t, h, h.sponsor, h.dca, map[string]any{"async": map[string]any{"resume_task": map[string]any{"task_id": s.Order.TaskID}}}, sdk.Coins{}))
		require.NotZero(t, r.TxResults[0].Code)
		require.Equal(t, topped, readDCAState(t, h, 1))
		dcaResume(t, h, s.Order.TaskID)
		require.Zero(t, readDCAState(t, h, 1).Order.FilledCount, "resume queues next height")
		old := readDCAState(t, h, 1)
		r = h.block(t)
		dcaPollFees(t, h, r, old)
		filled := readDCAState(t, h, 1)
		require.Equal(t, uint32(1), filled.Order.FilledCount)
		require.NotNil(t, filled.Task.Inbox.Success)
		require.Greater(t, *filled.Order.LastFillHeight, s.Order.RegistrationHeight+3, "late dispatch does one slice")
		gas := dcaBalance(h, dcaAddress(h.key), "ugas")
		principal := dcaBalance(h, dcaAddress(h.key), "uinput")
		dcaCancel(t, h, s.Order.TaskID)
		cancelled := readDCAState(t, h, 1)
		require.Equal(t, "cancelled", cancelled.Task.Status.Kind)
		require.Equal(t, filled.Order, cancelled.Order)
		require.Equal(t, gas.Add(dcaInt(t, filled.Funding.Available)).String(), dcaBalance(h, dcaAddress(h.key), "ugas").String())
		dcaReclaim(t, h)
		reclaimed := readDCAState(t, h, 1)
		require.Equal(t, "20", reclaimed.Order.Reclaimed)
		require.Equal(t, "10", reclaimed.Order.Spent)
		require.Equal(t, principal.AddRaw(20).String(), dcaBalance(h, dcaAddress(h.key), "uinput").String())
		require.Equal(t, "15", reclaimed.Order.Received)
		require.Nil(t, dcaFillAt(t, h, 1))
		require.Equal(t, "10", dcaBalance(h, h.exchange, "uinput").String())
		r = h.finalize(t, false, executeDCA(t, h, h.key, h.dca, map[string]any{"reclaim_principal": map[string]any{"order_id": 1}}, sdk.Coins{}))
		require.NotZero(t, r.TxResults[0].Code)
		require.Equal(t, reclaimed, readDCAState(t, h, 1))
	})
	for _, window := range []string{"before_first", "between_fills"} {
		t.Run(window, func(t *testing.T) {
			h := newDCAAcceptanceApp(t)
			deployDCA(t, h)
			dcaLiquidity(t, h, 1000)
			s := dcaCreate(t, h, 3, 4, 60, 8, "14")
			spent, reclaim := "0", "30"
			if window == "between_fills" {
				dcaRunTo(t, h, s.Order.RegistrationHeight+5)
				spent, reclaim = "10", "20"
			}
			before := readDCAState(t, h, 1)
			owner := dcaBalance(h, dcaAddress(h.key), "uinput")
			dcaCancel(t, h, s.Order.TaskID)
			dcaReclaim(t, h)
			after := readDCAState(t, h, 1)
			require.Equal(t, "cancelled", after.Task.Status.Kind)
			require.Equal(t, spent, after.Order.Spent)
			require.Equal(t, reclaim, after.Order.Reclaimed)
			require.Equal(t, before.Order.FilledCount, after.Order.FilledCount)
			require.Equal(t, owner.Add(dcaInt(t, reclaim)).String(), dcaBalance(h, dcaAddress(h.key), "uinput").String())
			require.Equal(t, "0", after.Order.Remaining)
		})
	}
}

func dcaReopen(t *testing.T, h *dcaAcceptanceHarness) {
	t.Helper()
	h.cleanupVM()
	require.NoError(t, h.app.Close())
	h.app, h.cleanupVM = openAcceptanceApp(t, h.home)
}
func TestDCAAcceptanceDiskReopen(t *testing.T) {
	for _, window := range []string{"before_timer", "after_receipt", "expiry_after_receipt"} {
		t.Run(window, func(t *testing.T) {
			h := newDCAAcceptanceApp(t)
			deployDCA(t, h)
			dcaLiquidity(t, h, 1000)
			expiry := uint64(60)
			if window == "expiry_after_receipt" {
				expiry = 12
			}
			attempts := uint64(8)
			if window == "expiry_after_receipt" {
				attempts = 1
			}
			s := dcaCreate(t, h, 3, 3, expiry, attempts, "14")
			target := s.Order.RegistrationHeight + 3
			if window == "expiry_after_receipt" {
				dcaRunTo(t, h, target-2)
				timer := readDCAState(t, h, 1)
				// Refill only the measured timer charge. The fill then leaves less
				// than one maximum attempt, naturally pausing before consumption.
				dcaFund(t, h, h.sponsor, s.Order.TaskID, dcaInt(t, timer.Funding.MaxAttemptCharge).Sub(dcaInt(t, timer.Funding.Available)))
			}
			if window == "before_timer" {
				dcaRunTo(t, h, target-1)
			} else {
				dcaRunTo(t, h, target)
				require.NotNil(t, readDCAState(t, h, 1).Task.Inbox.Success)
			}
			before := readDCAState(t, h, 1)
			fill := dcaFillAt(t, h, 0)
			balances := dcaBusinessBalances(h)
			allBalances := dcaAllBalances(h)
			root := bytes.Clone(h.app.LastCommitID().Hash)
			t.Logf("database_reopen window=%s height=%d app_hash=%X filled=%d task=%d sequence=%d", window, h.app.LastBlockHeight(), root, before.Order.FilledCount, before.Order.TaskID, before.Task.WaitSequence)
			dcaReopen(t, h)
			require.Equal(t, root, h.app.LastCommitID().Hash)
			require.Equal(t, before, readDCAState(t, h, 1))
			require.Equal(t, fill, dcaFillAt(t, h, 0))
			require.Equal(t, balances, dcaBusinessBalances(h))
			require.Equal(t, allBalances, dcaAllBalances(h))
			old := before
			r := h.block(t)
			dcaPollFees(t, h, r, old)
			after := readDCAState(t, h, 1)
			if window == "before_timer" {
				require.Equal(t, uint32(1), after.Order.FilledCount)
			} else {
				require.Equal(t, before.Order, after.Order)
				require.Equal(t, balances, dcaBusinessBalances(h))
			}
			if window == "expiry_after_receipt" {
				require.Equal(t, "paused", after.Task.Status.Kind)
				require.Equal(t, "insufficient_funds", after.Task.Status.Reason)
				require.Equal(t, before.Task.Inbox, after.Task.Inbox)
				for uint64(h.app.LastBlockHeight()) < expiry {
					h.block(t)
				}
				dcaReclaim(t, h)
				var view struct {
					Order dcaOrderRecord `json:"order"`
					Host  struct {
						Unavailable *struct {
							LeaseExpired bool `json:"lease_expired"`
						} `json:"unavailable"`
					} `json:"host"`
				}
				dcaQuery(t, h, h.dca, map[string]any{"order": map[string]any{"order_id": 1}}, &view)
				require.Equal(t, "10", view.Order.Spent)
				require.Equal(t, "20", view.Order.Reclaimed)
				require.Equal(t, "0", view.Order.Remaining)
				require.Equal(t, expiry, view.Order.ExpiresAt)
				require.NotNil(t, view.Host.Unavailable, "expired host record was pruned")
				require.True(t, view.Host.Unavailable.LeaseExpired)
				require.Equal(t, fill, dcaFillAt(t, h, 0))
			}
		})
	}
}
