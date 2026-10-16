//go:build upstream

package async_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
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

// Auction's conservative admission minimum: metadata 1,024 + inbox reserve
// (64*6+128) + generated timer/checkpoint allowance 16,384. Bid storage is separate.
const auctionAllocation uint64 = 17_920
const auctionPollCap uint64 = 100_000_000

// Shared test-only fixtures for the app and process-backed recovery lanes.
type auctionKeys struct {
	Seller, Sponsor        *secp256k1.PrivKey
	Bidders, Beneficiaries []*secp256k1.PrivKey
}

func auctionFixtureKeys() auctionKeys {
	key := func(label string) *secp256k1.PrivKey {
		return secp256k1.GenPrivKeyFromSecret([]byte("auction " + label))
	}
	k := auctionKeys{Seller: key("seller"), Sponsor: key("sponsor")}
	for i := 0; i < 5; i++ {
		k.Bidders = append(k.Bidders, key(fmt.Sprintf("bidder %d", i)))
		k.Beneficiaries = append(k.Beneficiaries, key(fmt.Sprintf("beneficiary %d", i)))
	}
	return k
}
func auctionAddress(k *secp256k1.PrivKey) sdk.AccAddress { return sdk.AccAddress(k.PubKey().Address()) }
func auctionInitialCoins() sdk.Coins {
	return sdk.NewCoins(sdk.NewInt64Coin("stake", 1_000_000_000), sdk.NewInt64Coin("ugas", 1_000_000_000), sdk.NewInt64Coin("usale", 1_000_000), sdk.NewInt64Coin("uquote", 1_000_000))
}
func (k auctionKeys) all() []*secp256k1.PrivKey {
	out := []*secp256k1.PrivKey{k.Seller, k.Sponsor}
	out = append(out, k.Bidders...)
	return append(out, k.Beneficiaries...)
}

type auctionTerms struct {
	SupplyLots   string `json:"supply_lots"`
	LotSize      string `json:"lot_size"`
	ReservePrice string `json:"reserve_price"`
	TickSize     string `json:"tick_size"`
	TickCount    uint16 `json:"tick_count"`
	MaxBids      uint16 `json:"max_bids"`
	BatchSize    uint16 `json:"batch_size"`
	CommitEnd    uint64 `json:"commit_end"`
	RevealEnd    uint64 `json:"reveal_end"`
	ExpiresAt    uint64 `json:"expires_at"`
}

func auctionFixtureTerms(registration uint64) auctionTerms {
	return auctionTerms{"5", "10", "2", "1", 3, 5, 2, registration + 8, registration + 15, registration + 70}
}

type auctionBookBid struct {
	Quantity, Collateral string
	Tick                 uint16
	Revealed             bool
}

// Independently hand-calculated oracle: high=2; marginal requests 2+2 share 3
// lots, base 1+1 and first-commit dust 1. Uniform price=3; sale=20,20,10,0,0;
// payments=6,6,3,0,0 and refunds=2,0,3,2,7. Total collateral=29.
func auctionFixtureBook() []auctionBookBid {
	return []auctionBookBid{{"2", "8", 2, true}, {"2", "6", 1, true}, {"2", "6", 1, true}, {"1", "2", 0, true}, {"1", "7", 2, false}}
}
func auctionCommitment(chain, contract, bidder, beneficiary string, collateral, quantity string, tick uint16, salt []byte) []byte {
	b := bytes.NewBufferString("cosmwasm/native-batch-auction/bid/v1")
	for _, s := range []string{chain, contract, bidder, beneficiary} {
		if uint64(len(s)) > uint64(^uint32(0)) {
			panic("commitment string too long")
		}
		requireWrite := binary.Write(b, binary.BigEndian, uint32(len(s)))
		if requireWrite != nil {
			panic(requireWrite)
		}
		b.WriteString(s)
	}
	binary.Write(b, binary.BigEndian, uint64(1))
	for _, s := range []string{collateral, quantity} {
		n, ok := new(big.Int).SetString(s, 10)
		if !ok || n.Sign() < 0 || n.BitLen() > 128 {
			panic("invalid commitment integer")
		}
		var field [16]byte
		n.FillBytes(field[:])
		b.Write(field[:])
	}
	binary.Write(b, binary.BigEndian, tick)
	if len(salt) != 32 {
		panic("invalid commitment salt")
	}
	b.Write(salt)
	digest := sha256.Sum256(b.Bytes())
	return digest[:]
}

type auctionClearing struct {
	Price          string  `json:"price"`
	MarginalTick   *uint16 `json:"marginal_tick"`
	MarginalLots   string  `json:"marginal_lots"`
	MarginalDemand string  `json:"marginal_demand"`
}
type auctionSettlement struct {
	Phase             string           `json:"phase"`
	Cursor            uint16           `json:"cursor"`
	HigherDemand      string           `json:"higher_demand"`
	Clearing          *auctionClearing `json:"clearing"`
	BaseAllocated     string           `json:"base_allocated"`
	Residual          string           `json:"residual"`
	FinalAllocated    string           `json:"final_allocated"`
	BidderSaleTotal   string           `json:"bidder_sale_total"`
	BidderRefundTotal string           `json:"bidder_refund_total"`
	SellerQuote       string           `json:"seller_quote"`
}
type auctionClaimTotals struct {
	BidderSale  string `json:"bidder_sale"`
	BidderQuote string `json:"bidder_quote"`
	SellerSale  string `json:"seller_sale"`
	SellerQuote string `json:"seller_quote"`
}
type auctionRecord struct {
	AuctionID       uint64             `json:"auction_id"`
	TaskID          uint64             `json:"task_id"`
	CreatedAt       uint64             `json:"created_at"`
	Terms           auctionTerms       `json:"terms"`
	State           string             `json:"state"`
	CommitCount     uint16             `json:"commit_count"`
	TotalCollateral string             `json:"total_collateral"`
	TotalDemand     string             `json:"total_demand"`
	Settlement      auctionSettlement  `json:"settlement"`
	Claimed         auctionClaimTotals `json:"claimed"`
	SellerClaimed   bool               `json:"seller_claimed"`
	Budget          struct {
		GasPerPoll  uint64 `json:"gas_per_poll"`
		MaxGasPrice string `json:"max_gas_price"`
	} `json:"budget"`
}
type auctionBid struct {
	Index         uint16 `json:"index"`
	Bidder        string `json:"bidder"`
	Digest        []byte `json:"digest"`
	Collateral    string `json:"collateral"`
	AllocatedLots string `json:"allocated_lots"`
	Claimed       bool   `json:"claimed"`
	Reveal        *struct {
		QuantityLots string `json:"quantity_lots"`
		PriceTick    uint16 `json:"price_tick"`
		Beneficiary  string `json:"beneficiary"`
	} `json:"reveal"`
	Entitlement *struct {
		Sale        string `json:"sale"`
		QuoteRefund string `json:"quote_refund"`
	} `json:"entitlement"`
}
type auctionView struct {
	Config struct {
		Seller     string `json:"seller"`
		SaleDenom  string `json:"sale_denom"`
		QuoteDenom string `json:"quote_denom"`
		FeeDenom   string `json:"fee_denom"`
	} `json:"config"`
	Auction auctionRecord `json:"auction"`
	Phase   string        `json:"phase"`
	Host    struct {
		Present     *async.TaskResponse `json:"present"`
		Unavailable *struct {
			Error        string `json:"error"`
			LeaseExpired bool   `json:"lease_expired"`
		} `json:"unavailable"`
	} `json:"host"`
}
type auctionFunding struct {
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
type auctionFundingConfig struct {
	Denom         string `json:"denom"`
	GasPrice      string `json:"gas_price"`
	MinBasePrice  string `json:"min_base_price"`
	MaxGasPerPoll uint64 `json:"max_gas_per_poll"`
}
type auctionState struct {
	View    auctionView
	Task    async.TaskResponse
	Funding auctionFunding
	Bids    []auctionBid
}
type auctionSmartQuery func(msg any, out any)

func auctionReadState(t *testing.T, query auctionSmartQuery) auctionState {
	t.Helper()
	var s auctionState
	query(map[string]any{"auction": map[string]any{}}, &s.View)
	query(map[string]any{"task": map[string]any{}}, &s.Task)
	query(map[string]any{"async": map[string]any{"task_funding": map[string]any{"task_id": s.View.Auction.TaskID}}}, &s.Funding)
	query(map[string]any{"bids": map[string]any{"limit": 32}}, &s.Bids)
	return s
}

type auctionObservation struct {
	Reached       bool
	BidWrites     int
	VMGas         uint64
	VMError       error
	ContractError string
}
type auctionEngine struct {
	wasmtypes.WasmEngine
	observation *auctionObservation
}
type auctionStore struct {
	wasmvm.KVStore
	observation *auctionObservation
}

func (s auctionStore) Set(k, v []byte) {
	if bytes.HasPrefix(k, []byte("auction/v1/bid/")) {
		s.observation.BidWrites++
	}
	if string(k) == "test-only/real-batch-reached" {
		s.observation.Reached = true
	}
	s.KVStore.Set(k, v)
}
func (e auctionEngine) Sudo(c wasmvm.Checksum, env vmtypes.Env, msg []byte, store wasmvm.KVStore, api wasmvm.GoAPI, q wasmvm.Querier, meter wasmvm.GasMeter, limit uint64, fraction vmtypes.UFraction) (*vmtypes.ContractResult, uint64, error) {
	*e.observation = auctionObservation{}
	result, gas, err := e.WasmEngine.Sudo(c, env, msg, auctionStore{store, e.observation}, api, q, meter, limit, fraction)
	e.observation.VMGas = gas
	e.observation.VMError = err
	if result != nil {
		e.observation.ContractError = result.Err
	}
	return result, gas, err
}

type auctionAcceptanceHarness struct {
	*appHarness
	keys        auctionKeys
	auction     sdk.AccAddress
	pollEvents  []sdk.Event
	observation *auctionObservation
}

func newAuctionAcceptanceApp(t *testing.T) *auctionAcceptanceHarness {
	t.Helper()
	keys := auctionFixtureKeys()
	h := &auctionAcceptanceHarness{appHarness: &appHarness{home: t.TempDir(), key: keys.Seller}, keys: keys, observation: &auctionObservation{}}
	h.app, h.cleanupVM = openAcceptanceApp(t, h.home, wasmkeeper.WithWasmEngineDecorator(func(e wasmtypes.WasmEngine) wasmtypes.WasmEngine { return auctionEngine{e, h.observation} }))
	t.Cleanup(func() { h.cleanupVM(); require.NoError(t, h.app.Close()) })
	var accounts []authtypes.GenesisAccount
	var balances []banktypes.Balance
	for i, key := range keys.all() {
		a := auctionAddress(key)
		accounts = append(accounts, authtypes.NewBaseAccount(a, key.PubKey(), uint64(i), 0))
		balances = append(balances, banktypes.Balance{Address: a.String(), Coins: auctionInitialCoins()})
	}
	validator := cmttypes.NewValidator(cmted.GenPrivKeyFromSecret([]byte("auction validator")).PubKey(), 1)
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
func (h *auctionAcceptanceHarness) block(t *testing.T, txs ...[]byte) *abci.ResponseFinalizeBlock {
	return h.finalize(t, true, txs...)
}
func (h *auctionAcceptanceHarness) finalize(t *testing.T, success bool, txs ...[]byte) *abci.ResponseFinalizeBlock {
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
func (h *auctionAcceptanceHarness) signed(t *testing.T, key *secp256k1.PrivKey, msg sdk.Msg) []byte {
	t.Helper()
	account := h.app.AccountKeeper.GetAccount(h.app.NewUncachedContext(false, cmtproto.Header{}), auctionAddress(key))
	if account == nil {
		account = h.app.AccountKeeper.GetAccount(h.app.NewContext(false), auctionAddress(key))
	}
	tx, err := sims.GenSignedMockTx(rand.New(rand.NewSource(1)), h.app.TxConfig(), []sdk.Msg{msg}, sdk.NewCoins(sdk.NewInt64Coin("stake", 1)), 100_000_000, acceptanceChain, []uint64{account.GetAccountNumber()}, []uint64{account.GetSequence()}, key)
	require.NoError(t, err)
	raw, err := h.app.TxConfig().TxEncoder()(tx)
	require.NoError(t, err)
	return raw
}
func (h *auctionAcceptanceHarness) execute(t *testing.T, key *secp256k1.PrivKey, msg any, coins sdk.Coins) []byte {
	t.Helper()
	raw, err := json.Marshal(msg)
	require.NoError(t, err)
	return h.signed(t, key, &wasmtypes.MsgExecuteContract{Sender: auctionAddress(key).String(), Contract: h.auction.String(), Msg: raw, Funds: coins})
}
func (h *auctionAcceptanceHarness) query(t *testing.T) auctionSmartQuery {
	return func(msg any, out any) {
		t.Helper()
		raw, err := json.Marshal(msg)
		require.NoError(t, err)
		height := h.app.LastBlockHeight()
		answer, err := h.app.WasmKeeper.QuerySmart(h.app.NewUncachedContext(false, cmtproto.Header{Height: height, Time: acceptanceTime.Add(time.Duration(height) * time.Second), ChainID: acceptanceChain}), h.auction, raw)
		require.NoError(t, err)
		require.NoError(t, json.Unmarshal(answer, out))
	}
}
func (h *auctionAcceptanceHarness) state(t *testing.T) auctionState {
	return auctionReadState(t, h.query(t))
}
func auctionInt(t *testing.T, s string) sdkmath.Int {
	t.Helper()
	n, ok := sdkmath.NewIntFromString(s)
	require.True(t, ok, "integer %q", s)
	return n
}
func (h *auctionAcceptanceHarness) balance(a sdk.AccAddress, d string) sdkmath.Int {
	return h.app.BankKeeper.GetBalance(h.app.NewUncachedContext(false, cmtproto.Header{}), a, d).Amount
}
func (h *auctionAcceptanceHarness) balances() map[string]string {
	out := map[string]string{}
	addresses := []sdk.AccAddress{h.auction, authtypes.NewModuleAddress(async.ModuleName), authtypes.NewModuleAddress(authtypes.FeeCollectorName)}
	for _, k := range h.keys.all() {
		addresses = append(addresses, auctionAddress(k))
	}
	for _, a := range addresses {
		out[a.String()] = h.app.BankKeeper.GetAllBalances(h.app.NewUncachedContext(false, cmtproto.Header{}), a).String()
	}
	return out
}
func (h *auctionAcceptanceHarness) storage() map[string]string {
	out := map[string]string{}
	h.app.WasmKeeper.IterateContractState(h.app.NewUncachedContext(false, cmtproto.Header{}), h.auction, func(k, v []byte) bool { out[hex.EncodeToString(k)] = hex.EncodeToString(v); return false })
	return out
}
func (h *auctionAcceptanceHarness) deploy(t *testing.T, artifact string) {
	t.Helper()
	code, err := os.ReadFile("../../artifacts/" + artifact + ".wasm")
	require.NoError(t, err, "run contracts/native-batch-auction/build-wasmd.sh")
	hash := sha256.Sum256(code)
	t.Logf("artifact=%s sha256=%x", artifact, hash)
	h.block(t, h.signed(t, h.key, &wasmtypes.MsgStoreCode{Sender: auctionAddress(h.key).String(), WASMByteCode: code}))
	r := h.block(t, h.signed(t, h.key, &wasmtypes.MsgInstantiateContract{Sender: auctionAddress(h.key).String(), CodeID: 1, Label: "batch auction", Msg: []byte(`{"sale_denom":"usale","quote_denom":"uquote","fee_denom":"ugas"}`)}))
	for _, e := range r.TxResults[0].Events {
		if e.Type == "instantiate" {
			for _, a := range e.Attributes {
				if a.Key == "_contract_address" {
					h.auction = sdk.MustAccAddressFromBech32(a.Value)
				}
			}
		}
	}
	require.NotEmpty(t, h.auction)
	h.owner = h.auction
	info := h.app.WasmKeeper.GetContractInfo(h.app.NewUncachedContext(false, cmtproto.Header{}), h.auction)
	require.Empty(t, info.Admin)
}
func auctionDeposit(t *testing.T, terms auctionTerms, registration, attempts uint64, c async.Config, price sdkmath.LegacyDec) (sdkmath.Int, sdkmath.Int, sdkmath.Int) {
	t.Helper()
	require.Greater(t, terms.ExpiresAt, registration)
	lease := sdkmath.NewIntFromUint64(auctionAllocation).Mul(sdkmath.NewIntFromUint64(terms.ExpiresAt - registration)).Mul(c.StorageByteBlockPrice).Add(c.CleanupPrice)
	attempt := price.MulInt(sdkmath.NewIntFromUint64(auctionPollCap)).Ceil().TruncateInt()
	total := lease.Add(attempt.Mul(sdkmath.NewIntFromUint64(attempts)))
	require.LessOrEqual(t, total.BigInt().BitLen(), 128)
	return total, lease, attempt
}
func (h *auctionAcceptanceHarness) create(t *testing.T, attempts uint64, ceiling string) auctionState {
	t.Helper()
	registration := uint64(h.app.LastBlockHeight() + 1)
	terms := auctionFixtureTerms(registration)
	var config auctionFundingConfig
	h.query(t)(map[string]any{"async": map[string]any{"funding_config": map[string]any{}}}, &config)
	require.Equal(t, "ugas", config.Denom)
	require.Equal(t, auctionPollCap, config.MaxGasPerPoll)
	price, err := sdkmath.LegacyNewDecFromStr(config.GasPrice)
	require.NoError(t, err)
	deposit, lease, attempt := auctionDeposit(t, terms, registration, attempts, h.app.AsyncKeeper.Config, price)
	msg := map[string]any{"create_auction": map[string]any{"terms": terms, "gas": sdk.NewCoin("ugas", deposit), "gas_per_poll": auctionPollCap, "max_gas_price": ceiling, "allocated_bytes": auctionAllocation}}
	h.block(t, h.execute(t, h.key, msg, sdk.NewCoins(sdk.NewInt64Coin("usale", 50), sdk.NewCoin("ugas", deposit))))
	s := h.state(t)
	require.Equal(t, registration, s.View.Auction.CreatedAt)
	require.Nil(t, s.Task.LastAttempt)
	require.Equal(t, lease.String(), s.Funding.LeasePaid)
	require.Equal(t, attempt.String(), s.Funding.MaxAttemptCharge)
	require.Equal(t, deposit.Sub(lease).String(), s.Funding.Available)
	require.Equal(t, auctionAddress(h.key).String(), s.Funding.Control)
	require.Equal(t, s.Funding.Control, s.Funding.Refund)
	t.Logf("registration=%d C=%d R=%d E=%d allocation=%d deposit=%s lease=%s max_attempt=%s host_reserve=%d", registration, terms.CommitEnd, terms.RevealEnd, terms.ExpiresAt, auctionAllocation, deposit, lease, attempt, h.app.AsyncKeeper.Config.HostReserve)
	return s
}
func (h *auctionAcceptanceHarness) fees(t *testing.T, before auctionState) sdkmath.Int {
	t.Helper()
	sum := sdkmath.ZeroInt()
	polls := 0
	for _, e := range h.pollEvents {
		if e.Type != "async_poll" {
			continue
		}
		attrs := map[string]string{}
		for _, a := range e.Attributes {
			attrs[a.Key] = a.Value
		}
		if attrs["contract"] != h.auction.String() {
			continue
		}
		polls++
		execution, err := strconv.ParseUint(attrs["execution_gas"], 10, 64)
		require.NoError(t, err)
		host, err := strconv.ParseUint(attrs["charged_host_gas"], 10, 64)
		require.NoError(t, err)
		price, err := sdkmath.LegacyNewDecFromStr(attrs["price"])
		require.NoError(t, err)
		require.Positive(t, execution)
		require.LessOrEqual(t, execution, before.Funding.GasPerPoll-h.app.AsyncKeeper.Config.HostReserve, "SDK guest execution partition")
		require.LessOrEqual(t, host, h.app.AsyncKeeper.Config.HostReserve, "SDK reserved host partition")
		require.LessOrEqual(t, execution+host, before.Funding.GasPerPoll)
		fee := price.MulInt(sdkmath.NewIntFromUint64(execution + host)).Ceil().TruncateInt()
		require.Equal(t, fee.String(), attrs["fee"])
		sum = sum.Add(fee)
		t.Logf("height=%d phase=%s cursor=%d execution_gas=%d host_gas=%d total=%d cap=%d price=%s fee=%s pause=%s", h.app.LastBlockHeight(), before.View.Auction.Settlement.Phase, before.View.Auction.Settlement.Cursor, execution, host, execution+host, before.Funding.GasPerPoll, attrs["price"], fee, attrs["pause_reason"])
	}
	require.LessOrEqual(t, polls, 1, "one poll maximum per block")
	after := h.state(t)
	require.LessOrEqual(t, len(after.Task.State), 4096, "actual continuation fits conservative lease allowance")
	require.Equal(t, auctionInt(t, before.Funding.Fees).Add(sum).String(), after.Funding.Fees)
	return sum
}
func (h *auctionAcceptanceHarness) next(t *testing.T, txs ...[]byte) {
	t.Helper()
	old := h.state(t)
	h.block(t, txs...)
	h.fees(t, old)
}
func (h *auctionAcceptanceHarness) runTo(t *testing.T, height uint64) {
	t.Helper()
	for uint64(h.app.LastBlockHeight()) < height {
		h.next(t)
	}
}
func (h *auctionAcceptanceHarness) seed(t *testing.T) auctionState {
	t.Helper()
	s := h.state(t)
	book := auctionFixtureBook()
	for i, bid := range book {
		salt := bytes.Repeat([]byte{byte(i + 1)}, 32)
		digest := auctionCommitment(acceptanceChain, h.auction.String(), auctionAddress(h.keys.Bidders[i]).String(), auctionAddress(h.keys.Beneficiaries[i]).String(), bid.Collateral, bid.Quantity, bid.Tick, salt)
		h.next(t, h.execute(t, h.keys.Bidders[i], map[string]any{"commit_bid": map[string]any{"digest": digest, "collateral": bid.Collateral}}, sdk.NewCoins(sdk.NewCoin("uquote", auctionInt(t, bid.Collateral)))))
	}
	h.runTo(t, s.View.Auction.Terms.CommitEnd-1)
	for i, bid := range book {
		if !bid.Revealed {
			continue
		}
		h.next(t, h.execute(t, h.keys.Bidders[i], map[string]any{"reveal_bid": map[string]any{"quantity_lots": bid.Quantity, "price_tick": bid.Tick, "beneficiary": auctionAddress(h.keys.Beneficiaries[i]).String(), "salt": bytes.Repeat([]byte{byte(i + 1)}, 32)}}, sdk.Coins{}))
	}
	after := h.state(t)
	require.Equal(t, "29", after.View.Auction.TotalCollateral)
	require.Equal(t, "7", after.View.Auction.TotalDemand)
	return after
}
func (h *auctionAcceptanceHarness) settle(t *testing.T) {
	t.Helper()
	for i := 0; i < 60; i++ {
		s := h.state(t)
		if s.View.Auction.State == "finalized" {
			return
		}
		require.NotEqual(t, "paused", s.Task.Status.Kind, "%+v", s.Task.Status)
		h.next(t)
	}
	t.Fatal("auction did not finalize")
}
func auctionAssertOracle(t *testing.T, s auctionState) {
	t.Helper()
	require.Equal(t, "finalized", s.View.Auction.State)
	require.Equal(t, "completed", s.Task.Status.Kind)
	a := s.View.Auction
	require.Equal(t, "done", a.Settlement.Phase)
	require.Equal(t, "3", a.Settlement.Clearing.Price)
	require.Equal(t, "5", a.Settlement.FinalAllocated)
	require.Equal(t, "50", a.Settlement.BidderSaleTotal)
	require.Equal(t, "14", a.Settlement.BidderRefundTotal)
	require.Equal(t, "15", a.Settlement.SellerQuote)
	require.Equal(t, "0", a.Settlement.Residual)
	require.Len(t, s.Bids, 5)
	for i, want := range []string{"2", "2", "1", "0", "0"} {
		require.Equal(t, want, s.Bids[i].AllocatedLots)
		require.Equal(t, []string{"20", "20", "10", "0", "0"}[i], s.Bids[i].Entitlement.Sale)
		require.Equal(t, []string{"2", "0", "3", "2", "7"}[i], s.Bids[i].Entitlement.QuoteRefund)
	}
}
func (h *auctionAcceptanceHarness) claims(t *testing.T) {
	t.Helper()
	for i := range auctionFixtureBook() {
		recipient := auctionAddress(h.keys.Beneficiaries[i])
		bidder := auctionAddress(h.keys.Bidders[i])
		sale := h.balance(recipient, "usale")
		quote := h.balance(bidder, "uquote")
		h.next(t, h.execute(t, h.keys.Sponsor, map[string]any{"claim_bid": map[string]any{"bidder": bidder.String()}}, sdk.Coins{}))
		after := h.state(t)
		require.True(t, after.Bids[i].Claimed)
		require.Equal(t, sale.AddRaw([]int64{20, 20, 10, 0, 0}[i]).String(), h.balance(recipient, "usale").String())
		require.Equal(t, quote.AddRaw([]int64{2, 0, 3, 2, 7}[i]).String(), h.balance(bidder, "uquote").String())
		require.Equal(t, int64(1_000_000), h.balance(bidder, "usale").Int64())
	}
	seller := auctionAddress(h.key)
	quote := h.balance(seller, "uquote")
	h.next(t, h.execute(t, h.keys.Sponsor, map[string]any{"claim_seller": map[string]any{}}, sdk.Coins{}))
	require.Equal(t, quote.AddRaw(15).String(), h.balance(seller, "uquote").String())
	require.Equal(t, "0", h.balance(h.auction, "usale").String())
	require.Equal(t, "0", h.balance(h.auction, "uquote").String())
	s := h.state(t)
	require.True(t, s.View.Auction.SellerClaimed)
	require.Equal(t, auctionClaimTotals{"50", "14", "0", "15"}, s.View.Auction.Claimed)
}

// Catches wrong marginal rounding, premature claims and misplaced bank recipients.
func TestAuctionAcceptanceClearing(t *testing.T) {
	h := newAuctionAcceptanceApp(t)
	h.deploy(t, "native_batch_auction")
	// A normal artifact has no fault-control execute variant.
	storageBeforeCreate := h.storage()
	r := h.finalize(t, false, h.execute(t, h.key, map[string]any{"fault": map[string]any{"mode": "error", "phase": "allocate_base", "cursor": 0}}, sdk.Coins{}))
	require.NotZero(t, r.TxResults[0].Code)
	require.Contains(t, r.TxResults[0].Log, "unknown variant")
	require.Equal(t, storageBeforeCreate, h.storage())
	h.create(t, 30, "1")
	h.seed(t)
	beforeSeller := h.state(t)
	r = h.finalize(t, false, h.execute(t, h.keys.Sponsor, map[string]any{"claim_seller": map[string]any{}}, sdk.Coins{}))
	require.NotZero(t, r.TxResults[0].Code)
	require.Equal(t, beforeSeller, h.state(t))
	h.runTo(t, h.state(t).View.Auction.Terms.RevealEnd)
	nonempty := 0
	claimsBatches := 0
	for h.state(t).View.Auction.State != "finalized" {
		before := h.state(t)
		business := h.storage()
		bank := h.balances()
		r := h.finalize(t, false, h.execute(t, h.keys.Sponsor, map[string]any{"claim_bid": map[string]any{"bidder": auctionAddress(h.keys.Bidders[0]).String()}}, sdk.Coins{}))
		require.NotZero(t, r.TxResults[0].Code, "provisional claims locked")
		h.fees(t, before)
		require.Equal(t, bank[h.auction.String()], h.balances()[h.auction.String()])
		if before.View.Auction.Settlement.Phase == "allocate_base" {
			nonempty++
		}
		if before.View.Auction.Settlement.Phase == "build_claims" {
			claimsBatches++
		}
		require.NotEqual(t, business, h.storage(), "real batch changes storage")
	}
	require.GreaterOrEqual(t, nonempty, 3)
	require.GreaterOrEqual(t, claimsBatches, 3)
	auctionAssertOracle(t, h.state(t))
	h.claims(t)
	before := h.state(t)
	r = h.finalize(t, false, h.execute(t, h.keys.Sponsor, map[string]any{"claim_bid": map[string]any{"bidder": auctionAddress(h.keys.Bidders[0]).String()}}, sdk.Coins{}))
	require.NotZero(t, r.TxResults[0].Code)
	require.Equal(t, before, h.state(t))
	r = h.finalize(t, false, h.execute(t, h.keys.Sponsor, map[string]any{"claim_seller": map[string]any{}}, sdk.Coins{}))
	require.NotZero(t, r.TxResults[0].Code)
	require.Equal(t, before, h.state(t))
}

// Catches committing real auction writes/checkpoints when the VM later aborts.
func TestAuctionAcceptanceRollback(t *testing.T) {
	for _, mode := range []string{"error", "trap", "oog"} {
		t.Run(mode, func(t *testing.T) {
			h := newAuctionAcceptanceApp(t)
			h.deploy(t, "auction_fault")
			h.create(t, 30, "1")
			h.seed(t)
			h.next(t, h.execute(t, h.key, map[string]any{"fault": map[string]any{"mode": mode, "phase": "allocate_base", "cursor": 0}}, sdk.Coins{}))
			h.runTo(t, h.state(t).View.Auction.Terms.RevealEnd)
			before := h.state(t)
			require.Equal(t, "allocate_base", before.View.Auction.Settlement.Phase)
			storage := h.storage()
			balances := h.balances()
			// BeginBlock forwards prior fees to distribution before EndBlock.
			collector := h.balance(authtypes.NewModuleAddress(authtypes.FeeCollectorName), "ugas").Add(h.balance(authtypes.NewModuleAddress("distribution"), "ugas"))
			escrow := h.balance(authtypes.NewModuleAddress(async.ModuleName), "ugas")
			h.block(t)
			fee := h.fees(t, before)
			after := h.state(t)
			require.Equal(t, "paused", after.Task.Status.Kind)
			require.True(t, h.observation.Reached, "real sudo wrote batch before injected failure")
			require.GreaterOrEqual(t, h.observation.BidWrites, 2)
			require.Equal(t, storage, h.storage())
			require.Equal(t, before.View.Auction, after.View.Auction)
			require.Equal(t, before.View.Config, after.View.Config)
			require.Equal(t, before.Bids, after.Bids)
			require.Equal(t, before.Task.State, after.Task.State)
			require.Equal(t, before.Task.WaitSequence, after.Task.WaitSequence)
			require.True(t, fee.IsPositive())
			require.Equal(t, escrow.Sub(fee).String(), h.balance(authtypes.NewModuleAddress(async.ModuleName), "ugas").String())
			require.Equal(t, collector.Add(fee).String(), h.balance(authtypes.NewModuleAddress(authtypes.FeeCollectorName), "ugas").Add(h.balance(authtypes.NewModuleAddress("distribution"), "ugas")).String())
			for _, a := range h.keys.all() {
				require.Equal(t, balances[auctionAddress(a).String()], h.balances()[auctionAddress(a).String()])
			}
			require.Equal(t, balances[h.auction.String()], h.balances()[h.auction.String()])
			if mode == "oog" {
				require.Equal(t, "out_of_gas", after.Task.Status.Reason)
				var gas vmtypes.OutOfGasError
				require.True(t, errors.As(h.observation.VMError, &gas), "actual VM OOG: %v", h.observation.VMError)
			} else if mode == "error" {
				require.Contains(t, h.observation.ContractError, "intentional error after real batch")
				require.Nil(t, h.observation.VMError)
			} else {
				require.Equal(t, "trap", after.Task.Status.Reason)
				require.Error(t, h.observation.VMError)
			}
			t.Logf("failure=%s reached=%v bid_writes=%d vm_gas=%d vm_error=%v contract_error=%s checkpoint_bytes=%d fee=%s", mode, h.observation.Reached, h.observation.BidWrites, h.observation.VMGas, h.observation.VMError, h.observation.ContractError, len(after.Task.State), fee)
			h.next(t, h.execute(t, h.key, map[string]any{"fault": map[string]any{"mode": "off", "phase": "allocate_base", "cursor": 0}}, sdk.Coins{}))
			h.next(t, h.execute(t, h.key, map[string]any{"async": map[string]any{"resume_task": map[string]any{"task_id": after.Task.TaskID}}}, sdk.Coins{}))
			require.Equal(t, before.View.Auction, h.state(t).View.Auction)
			h.settle(t)
			auctionAssertOracle(t, h.state(t))
			h.claims(t)
		})
	}
}

// The injected SDK bank restriction fails the SECOND transfer. At that point the
// genuine first sale payout is already visible in the child cache.
func TestAuctionAcceptanceClaimsAtomic(t *testing.T) {
	h := newAuctionAcceptanceApp(t)
	h.deploy(t, "native_batch_auction")
	h.create(t, 30, "1")
	h.seed(t)
	h.settle(t)
	auctionAssertOracle(t, h.state(t))
	before := h.state(t)
	storage := h.storage()
	beneficiary := auctionAddress(h.keys.Beneficiaries[0])
	bidder := auctionAddress(h.keys.Bidders[0])
	sale := h.balance(beneficiary, "usale")
	quote := h.balance(bidder, "uquote")
	contractSale := h.balance(h.auction, "usale")
	contractQuote := h.balance(h.auction, "uquote")
	reached := false
	h.app.BankKeeper.AppendSendRestriction(func(ctx context.Context, from, to sdk.AccAddress, amount sdk.Coins) (sdk.AccAddress, error) {
		if from.Equals(h.auction) && to.Equals(bidder) && amount.AmountOf("uquote").IsPositive() {
			reached = true
			require.Equal(t, sale.AddRaw(20).String(), h.app.BankKeeper.GetBalance(ctx, beneficiary, "usale").Amount.String(), "first payout succeeded before second failed")
			return nil, fmt.Errorf("intentional second payout bank failure")
		}
		return to, nil
	})
	r := h.finalize(t, false, h.execute(t, h.keys.Sponsor, map[string]any{"claim_bid": map[string]any{"bidder": bidder.String()}}, sdk.Coins{}))
	require.NotZero(t, r.TxResults[0].Code)
	require.Contains(t, r.TxResults[0].Log, "intentional second payout bank failure")
	require.True(t, reached)
	t.Log("observed first sale payout +20 in child cache before injected second quote-refund failure; both rollback")
	require.Equal(t, before, h.state(t))
	require.Equal(t, storage, h.storage())
	require.Equal(t, sale, h.balance(beneficiary, "usale"))
	require.Equal(t, quote, h.balance(bidder, "uquote"))
	require.Equal(t, contractSale, h.balance(h.auction, "usale"))
	require.Equal(t, contractQuote, h.balance(h.auction, "uquote"))
	h.app.BankKeeper.ClearSendRestriction()
	h.claims(t)
}

func (h *auctionAcceptanceHarness) fund(t *testing.T, amount sdkmath.Int) {
	s := h.state(t)
	h.next(t, h.execute(t, h.keys.Sponsor, map[string]any{"async": map[string]any{"fund_task": map[string]any{"task_id": s.Task.TaskID}}}, sdk.NewCoins(sdk.NewCoin("ugas", amount))))
}
func (h *auctionAcceptanceHarness) resume(t *testing.T) {
	s := h.state(t)
	h.next(t, h.execute(t, h.key, map[string]any{"async": map[string]any{"resume_task": map[string]any{"task_id": s.Task.TaskID}}}, sdk.Coins{}))
}
func TestAuctionAcceptanceFunding(t *testing.T) {
	for _, reason := range []string{"insufficient_funds", "price_limit"} {
		t.Run(reason, func(t *testing.T) {
			h := newAuctionAcceptanceApp(t)
			h.deploy(t, "native_batch_auction")
			attempts, ceiling := uint64(30), "0.001"
			if reason == "insufficient_funds" {
				attempts, ceiling = 1, "1"
			}
			h.create(t, attempts, ceiling)
			h.seed(t)
			if reason == "insufficient_funds" {
				// Maintain exactly one maximum attempt through the scan and first
				// nonempty allocation, then let its consumed fee cause the pause.
				s := h.state(t)
				h.fund(t, auctionInt(t, s.Funding.MaxAttemptCharge).Sub(auctionInt(t, s.Funding.Available)))
				h.runTo(t, s.View.Auction.Terms.RevealEnd)
				s = h.state(t)
				h.fund(t, auctionInt(t, s.Funding.MaxAttemptCharge).Sub(auctionInt(t, s.Funding.Available)))
			} else {
				h.runTo(t, h.state(t).View.Auction.Terms.RevealEnd+1)
			}
			partial := h.state(t)
			require.Equal(t, uint16(2), partial.View.Auction.Settlement.Cursor)
			require.Equal(t, "allocate_base", partial.View.Auction.Settlement.Phase)
			if reason == "price_limit" {
				// Explicit local host calibration: inject a valid .002 current
				// price and zero grace, then exercise the real scheduler/escrow.
				// This proves ceiling recovery, not an organic market price rise.
				h.app.AsyncKeeper.Config.PriceAdjustDenominator = 1_000_000
				h.app.AsyncKeeper.Config.PriceGraceBlocks = 0
				ctx := h.app.NewUncachedContext(false, cmtproto.Header{Height: h.app.LastBlockHeight()})
				ctx.KVStore(h.app.AsyncKeeper.Key).Set([]byte("native-async/base_price"), []byte("0.002"))
			}
			h.next(t)
			paused := h.state(t)
			require.Equal(t, "paused", paused.Task.Status.Kind)
			require.Equal(t, reason, paused.Task.Status.Reason)
			require.Equal(t, partial.View.Auction, paused.View.Auction, "pause preserves partial allocation")
			t.Logf("pause=%s height=%d cursor=%d available=%s max_attempt=%s", reason, h.app.LastBlockHeight(), paused.View.Auction.Settlement.Cursor, paused.Funding.Available, paused.Funding.MaxAttemptCharge)
			sponsor := h.balance(auctionAddress(h.keys.Sponsor), "ugas")
			amount := auctionInt(t, paused.Funding.MaxAttemptCharge)
			h.fund(t, amount)
			topped := h.state(t)
			require.Equal(t, paused.View.Auction, topped.View.Auction)
			require.Equal(t, paused.Task, topped.Task)
			require.Equal(t, sponsor.Sub(amount), h.balance(auctionAddress(h.keys.Sponsor), "ugas"))
			require.Equal(t, auctionInt(t, paused.Funding.Available).Add(amount), auctionInt(t, topped.Funding.Available))
			if reason == "price_limit" {
				// Resume itself rejects an ineligible ceiling. More funds cannot
				// repair it; the seller must first increase the ceiling.
				r := h.finalize(t, false, h.execute(t, h.key, map[string]any{"async": map[string]any{"resume_task": map[string]any{"task_id": paused.Task.TaskID}}}, sdk.Coins{}))
				require.NotZero(t, r.TxResults[0].Code)
				require.Contains(t, r.TxResults[0].Log, "insufficient funds or price limit")
				require.Equal(t, topped, h.state(t))
				h.next(t, h.execute(t, h.key, map[string]any{"update_budget": map[string]any{"gas_per_poll": auctionPollCap, "max_gas_price": "1"}}, sdk.Coins{}))
				require.Equal(t, "paused", h.state(t).Task.Status.Kind)
			} else {
				// One maximum attempt enables one poll only; sponsor sufficient
				// remaining attempts while still requiring the seller to resume.
				h.fund(t, amount.MulRaw(10))
			}
			h.resume(t)
			require.Equal(t, paused.View.Auction.Settlement, h.state(t).View.Auction.Settlement)
			h.settle(t)
			auctionAssertOracle(t, h.state(t))
			h.claims(t)
		})
	}
}

func (h *auctionAcceptanceHarness) reopen(t *testing.T) {
	t.Helper()
	require.NoError(t, h.app.Close())
	h.cleanupVM()
	h.app, h.cleanupVM = openAcceptanceApp(t, h.home, wasmkeeper.WithWasmEngineDecorator(func(e wasmtypes.WasmEngine) wasmtypes.WasmEngine { return auctionEngine{e, h.observation} }))
}

// This closes and reopens disk-backed DB/keepers/VM. Process restart and network
// state sync are separate evidence in the process-backed recovery suite.
func TestAuctionAcceptanceDiskReopen(t *testing.T) {
	h := newAuctionAcceptanceApp(t)
	h.deploy(t, "native_batch_auction")
	h.create(t, 30, "1")
	h.seed(t)
	h.runTo(t, h.state(t).View.Auction.Terms.RevealEnd+1)
	before := h.state(t)
	require.Equal(t, uint16(2), before.View.Auction.Settlement.Cursor)
	storage := h.storage()
	balances := h.balances()
	root := bytes.Clone(h.app.LastCommitID().Hash)
	h.reopen(t)
	require.Equal(t, before, h.state(t))
	require.Equal(t, storage, h.storage())
	require.Equal(t, balances, h.balances())
	require.Equal(t, root, h.app.LastCommitID().Hash)
	h.settle(t)
	auctionAssertOracle(t, h.state(t))
	final := h.state(t)
	storage = h.storage()
	balances = h.balances()
	h.reopen(t)
	require.Equal(t, final, h.state(t))
	require.Equal(t, storage, h.storage())
	require.Equal(t, balances, h.balances())
	h.claims(t)
	replay := newAuctionAcceptanceApp(t)
	for i, req := range h.records {
		r, err := replay.app.FinalizeBlock(req)
		require.NoError(t, err)
		require.Equal(t, h.results[i].TxResults, r.TxResults)
		_, err = replay.app.Commit()
		require.NoError(t, err)
		require.Equal(t, h.roots[i], replay.app.LastCommitID().Hash, "continuous replay root height %d", req.Height)
	}
	replay.auction = h.auction
	replay.owner = h.auction
	require.Equal(t, h.state(t), replay.state(t))
	require.Equal(t, h.storage(), replay.storage())
	require.Equal(t, h.balances(), replay.balances())
	t.Logf("disk reopen at partial and finalized-unclaimed; %d committed roots match uninterrupted replay", len(h.records))
}
