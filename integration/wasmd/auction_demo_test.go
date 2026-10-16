//go:build upstream

package async_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	sdkmath "cosmossdk.io/math"
	async "cosmwasm-durable-async/integration/wasmd"
	coretypes "github.com/cometbft/cometbft/rpc/core/types"
	cmttypes "github.com/cometbft/cometbft/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/stretchr/testify/require"
)

type auctionBatchRPC struct {
	DemoRPC
	attempted chan<- struct{}
	turn      <-chan struct{}
	next      chan struct{}
}

func (r auctionBatchRPC) BroadcastTxSync(ctx context.Context, tx cmttypes.Tx) (*coretypes.ResultBroadcastTx, error) {
	select {
	case <-r.turn:
	case <-ctx.Done():
		return nil, ctx.Err()
	}
	defer close(r.next)
	defer func() { r.attempted <- struct{}{} }()
	return r.DemoRPC.BroadcastTxSync(ctx, tx)
}

// Workers return errors as data. Only the parent releases consensus, after all
// first broadcast attempts; every later recovery uses the journal's same hash.
func journalAuctionBatch(ctx context.Context, rpc DemoRPC, paths []string, release func()) ([]TxReceipt, error) {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	type result struct {
		index   int
		receipt TxReceipt
		err     error
	}
	attempted := make(chan struct{}, len(paths))
	results := make(chan result, len(paths))
	turns := make([]chan struct{}, len(paths)+1)
	for i := range turns {
		turns[i] = make(chan struct{})
	}
	close(turns[0])
	for i, path := range paths {
		go func(i int, path string) {
			receipt, err := broadcastOnce(ctx, auctionBatchRPC{rpc, attempted, turns[i], turns[i+1]}, path)
			results <- result{i, receipt, err}
		}(i, path)
	}
	receipts := make([]TxReceipt, len(paths))
	var errs []error
	received, attempts := 0, 0
	aborted := false
	consume := func(r result) {
		received++
		receipts[r.index] = r.receipt
		if r.err != nil {
			// A definitive failed DeliverTx is evidence the caller may expect.
			// Persist/lookup/deadline failures remain operational errors.
			known := receiptError(r.receipt)
			if r.receipt.Outcome != "committed" || known == nil || r.err.Error() != known.Error() {
				errs = append(errs, r.err)
			}
		}
	}
	for attempts < len(paths) && !aborted {
		select {
		case <-attempted:
			attempts++
		case r := <-results:
			previousErrors := len(errs)
			consume(r)
			if len(errs) > previousErrors {
				aborted = true
				cancel()
			}
		case <-ctx.Done():
			aborted = true
			errs = append(errs, ctx.Err())
			cancel()
		}
	}
	if !aborted {
		release()
	}
	for received < len(paths) {
		consume(<-results)
	}
	return receipts, errors.Join(errs...)
}

type auctionDemoSalt struct {
	Input  map[string]any `json:"input"`
	Output struct {
		Digest    string `json:"digest_base64"`
		Salt      string `json:"salt_base64"`
		DigestHex string `json:"digest_hex"`
		SaltHex   string `json:"salt_hex"`
	} `json:"output"`
}

func writeAuctionSaltSnapshot(path string, value any) error {
	raw, err := json.MarshalIndent(value, "", "  ")
	if err != nil {
		return err
	}
	f, err := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return err
	}
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
	dir, err := os.Open(filepath.Dir(path))
	if err != nil {
		return err
	}
	defer dir.Close()
	return dir.Sync()
}

type auctionDemoSnapshot struct {
	Label   string         `json:"label"`
	Capture auctionCapture `json:"capture"`
}
type auctionDemoChild struct {
	PID    int  `json:"pid"`
	Exited bool `json:"exited"`
}
type auctionDemoRestart struct {
	OldPID    int            `json:"old_pid"`
	OldExited bool           `json:"old_exited"`
	Startup   auctionStartup `json:"startup"`
}
type auctionDemoReceipt struct {
	Scenario            string                `json:"scenario"`
	Source              string                `json:"source_revision"`
	SourceDirty         bool                  `json:"source_dirty"`
	Artifact            string                `json:"artifact_sha256"`
	HostConfig          async.Config          `json:"host_config"`
	FundingConfig       auctionFundingConfig  `json:"funding_config"`
	Info                demoInfo              `json:"node_config"`
	Transactions        []TxReceipt           `json:"transactions"`
	Snapshots           []auctionDemoSnapshot `json:"snapshots"`
	Restarts            []auctionDemoRestart  `json:"restarts"`
	Children            []auctionDemoChild    `json:"children"`
	OwnedChildrenExited bool                  `json:"owned_children_exited"`
	Passed              bool                  `json:"passed"`
}
type nativeAuctionDemo struct {
	h        *auctionNetwork
	output   string
	receipt  auctionDemoReceipt
	finished bool
}
type auctionDemoIndexEntry struct {
	Scenario string `json:"scenario"`
	Receipt  string `json:"receipt"`
}

func TestNativeAuctionDemo(t *testing.T) {
	if os.Getenv("NATIVE_AUCTION_DEMO") != "1" {
		t.Skip("use scripts/native-auction-demo.sh")
	}
	scenario := os.Getenv("NATIVE_AUCTION_SCENARIO")
	names := []string{scenario}
	if scenario == "all" {
		names = []string{"happy", "funding", "restart", "expiry"}
	}
	for _, name := range names {
		require.Contains(t, []string{"happy", "funding", "restart", "expiry"}, name)
	}
	run := os.Getenv("NATIVE_AUCTION_RUN")
	if run == "" {
		base := os.Getenv("NATIVE_AUCTION_OUTPUT")
		if base == "" {
			base = os.TempDir()
		}
		var err error
		run, err = os.MkdirTemp(base, "native-auction-run-")
		require.NoError(t, err)
	}
	t.Logf("auction demo run receipts: %s", run)
	index := []auctionDemoIndexEntry{}
	for _, name := range names {
		var entry auctionDemoIndexEntry
		ok := t.Run(name, func(t *testing.T) {
			d := newNativeAuctionDemo(t, run, name)
			d.run(name)
			d.finished = true
			entry = auctionDemoIndexEntry{name, filepath.Join(filepath.Base(d.output), "receipt.json")}
		})
		if !ok {
			return
		}
		index = append(index, entry)
	}
	require.NoError(t, writeAuctionSaltSnapshot(filepath.Join(run, "suite-result.json"), index))
}

func newNativeAuctionDemo(t *testing.T, run, scenario string) *nativeAuctionDemo {
	output, err := os.MkdirTemp(run, scenario+"-")
	require.NoError(t, err)
	d := &nativeAuctionDemo{output: output, receipt: auctionDemoReceipt{Scenario: scenario, HostConfig: async.ExperimentalConfig()}}
	raw, err := exec.Command("git", "rev-parse", "HEAD").Output()
	require.NoError(t, err)
	d.receipt.Source = strings.TrimSpace(string(raw))
	raw, err = exec.Command("git", "status", "--porcelain", "--untracked-files=no").Output()
	require.NoError(t, err)
	d.receipt.SourceDirty = len(bytes.TrimSpace(raw)) != 0
	code, err := os.ReadFile("../../artifacts/native_batch_auction.wasm")
	require.NoError(t, err)
	hash := sha256.Sum256(code)
	d.receipt.Artifact = hex.EncodeToString(hash[:])
	d.h = newAuctionNetwork(t, 2)
	h := d.h
	// Runs before network/temp-home cleanups, preserving only small evidence.
	t.Cleanup(func() {
		d.receipt.OwnedChildrenExited = true
		for i, n := range h.nodes {
			if n.child == nil {
				continue
			}
			_ = n.child.stop()
			exited := false
			select {
			case <-n.child.done:
				exited = true
			default:
			}
			d.receipt.Children = append(d.receipt.Children, auctionDemoChild{n.child.command.Process.Pid, exited})
			d.receipt.OwnedChildrenExited = d.receipt.OwnedChildrenExited && exited
			log, err := os.ReadFile(filepath.Join(n.home, "node.log"))
			if err == nil {
				require.NoError(t, os.WriteFile(filepath.Join(output, fmt.Sprintf("node-%d.log", i)), log, 0600))
			}
		}
		d.receipt.Transactions = h.transactions
		d.receipt.Passed = d.finished && !t.Failed() && d.receipt.OwnedChildrenExited
		require.NoError(t, writeDemoMarker(filepath.Join(output, "receipt.json"), d.receipt))
		t.Logf("scenario=%s passed=%v owned_children_exited=%v receipt=%s", scenario, d.receipt.Passed, d.receipt.OwnedChildrenExited, filepath.Join(output, "receipt.json"))
	})
	h.submitSigned = func(raws [][]byte, height int64) ([]TxReceipt, error) {
		paths := make([]string, len(raws))
		for i, raw := range raws {
			paths[i] = filepath.Join(output, fmt.Sprintf("tx-%03d.json", len(h.transactions)+i+1))
			if _, err := writeIntent(paths[i], raw); err != nil {
				return nil, err
			}
		}
		return journalAuctionBatch(h.ctx, h.nodes[0].client, paths, func() { h.runTo(height) })
	}
	var salts []auctionDemoSalt
	h.setupOptions.commitment = func(index int, bid auctionBookBid) ([]byte, []byte) {
		if salts == nil {
			for i, bid := range auctionFixtureBook() {
				entry := auctionDemoSalt{Input: map[string]any{"chain_id": acceptanceChain, "contract": h.contract.String(), "bidder": auctionAddress(h.keys.Bidders[i]).String(), "beneficiary": auctionAddress(h.keys.Beneficiaries[i]).String(), "auction_id": "1", "collateral": bid.Collateral, "quantity_lots": bid.Quantity, "price_tick": bid.Tick}}
				input, err := json.Marshal(entry.Input)
				require.NoError(t, err)
				command := exec.CommandContext(h.ctx, "node", "../../scripts/auction-commitment.mjs")
				command.Stdin = bytes.NewReader(input)
				answer, err := command.Output()
				require.NoError(t, err)
				require.NoError(t, json.Unmarshal(answer, &entry.Output))
				salts = append(salts, entry)
			}
			require.NoError(t, writeAuctionSaltSnapshot(filepath.Join(output, "commitments.json"), salts))
		}
		entry := salts[index]
		digest, err := base64.StdEncoding.DecodeString(entry.Output.Digest)
		require.NoError(t, err)
		salt, err := base64.StdEncoding.DecodeString(entry.Output.Salt)
		require.NoError(t, err)
		require.Len(t, salt, 32)
		require.Equal(t, auctionCommitment(acceptanceChain, h.contract.String(), auctionAddress(h.keys.Bidders[index]).String(), auctionAddress(h.keys.Beneficiaries[index]).String(), bid.Collateral, bid.Quantity, bid.Tick, salt), digest)
		return digest, salt
	}
	if scenario == "funding" || scenario == "expiry" {
		// Local calibrated margin: timer+scan <500 ugas, first allocation exhausts
		// the next full reservation. This is not a production gas bound.
		h.setupOptions.initialGas = func(lease, attempt sdkmath.Int) sdkmath.Int { return lease.Add(attempt).AddRaw(500) }
		h.setupOptions.ceiling = "0.001"
	}
	h.setupAuction()
	h.smart(0, 0)(map[string]any{"async": map[string]any{"funding_config": map[string]any{}}}, &d.receipt.FundingConfig)
	require.NoError(t, h.query(0, 0, "info", demoQuery{}, &d.receipt.Info))
	require.Equal(t, uint64(70_000_000), d.receipt.Info.HostReserve)
	require.Equal(t, "1", d.receipt.Info.Rent)
	require.Equal(t, "100", d.receipt.Info.Cleanup)
	d.snapshot("committed and revealed; principal locked")
	return d
}
func (d *nativeAuctionDemo) snapshot(label string) auctionCapture {
	s := d.h.capture(0, 0)
	d.receipt.Snapshots = append(d.receipt.Snapshots, auctionDemoSnapshot{label, s})
	host := "unavailable/pruned"
	if s.State.Task != nil {
		host = s.State.Task.Status.Kind
	}
	d.h.t.Logf("%s H=%d business=%s settlement=%s/%d host=%s", label, s.Height, s.State.View.Auction.State, s.State.View.Auction.Settlement.Phase, s.State.View.Auction.Settlement.Cursor, host)
	return s
}
func (d *nativeAuctionDemo) rejectedClaims(label string) {
	h := d.h
	before := h.capture(0, 0)
	bid := h.execution(h.keys.Sponsor, map[string]any{"claim_bid": map[string]any{"bidder": auctionAddress(h.keys.Bidders[0]).String()}}, sdk.Coins{})
	seller := h.execution(h.keys.Seller, map[string]any{"claim_seller": map[string]any{}}, sdk.Coins{})
	bid.wantFailure, seller.wantFailure = true, true
	h.submitBatch(bid, seller)
	after := d.snapshot(label)
	require.Equal(h.t, before.State.View.Auction, after.State.View.Auction)
	require.Equal(h.t, before.State.Bids, after.State.Bids)
	for _, address := range h.addresses {
		for _, denom := range []string{"usale", "uquote"} {
			require.Equal(h.t, before.Balances[address].AmountOf(denom), after.Balances[address].AmountOf(denom))
		}
	}
}
func (d *nativeAuctionDemo) settle(allBatches bool) {
	h := d.h
	counts := map[string]int{}
	for i := 0; i < 60; i++ {
		before := h.capture(0, 0)
		if before.State.View.Auction.State == "finalized" {
			h.settle()
			if allBatches {
				require.GreaterOrEqual(h.t, counts["allocate_base"], 3)
				require.GreaterOrEqual(h.t, counts["build_claims"], 3)
			}
			d.snapshot("business finalized; host completion shown separately")
			return
		}
		require.NotNil(h.t, before.State.Task)
		require.NotEqual(h.t, "paused", before.State.Task.Status.Kind)
		require.Equal(h.t, int64(50), before.Balances[h.contract.String()].AmountOf("usale").Int64())
		require.Equal(h.t, int64(29), before.Balances[h.contract.String()].AmountOf("uquote").Int64())
		h.step()
		after := d.snapshot("bounded settlement step")
		phase := before.State.View.Auction.Settlement.Phase
		if (phase == "allocate_base" || phase == "build_claims") && (before.State.View.Auction.Settlement.Cursor != after.State.View.Auction.Settlement.Cursor || phase != after.State.View.Auction.Settlement.Phase) {
			counts[phase]++
		}
	}
	h.t.Fatal("auction demo did not finalize within bounded steps")
}
func (d *nativeAuctionDemo) run(name string) {
	h, t := d.h, d.h.t
	switch name {
	case "happy":
		d.rejectedClaims("early claims rejected by committed transactions")
		d.settle(true)
	case "restart":
		h.reach("allocate_base")
		for _, label := range []string{"partial nonempty batch", "finalized before claims"} {
			before := d.snapshot("restart window: " + label)
			oldChild := h.nodes[0].child
			oldPID := oldChild.command.Process.Pid
			startup := h.restartValidator(before)
			require.NotEqual(t, oldPID, startup.PID)
			oldExited := false
			select {
			case <-oldChild.done:
				oldExited = true
			default:
			}
			require.True(t, oldExited)
			d.receipt.Restarts = append(d.receipt.Restarts, auctionDemoRestart{oldPID, oldExited, startup})
			require.NoError(t, os.Rename(filepath.Join(h.nodes[0].home, "auction-startup.json"), filepath.Join(h.nodes[0].home, fmt.Sprintf("auction-startup-%d.json", oldPID))))
			if label == "partial nonempty batch" {
				d.settle(false)
			}
		}
	case "funding", "expiry":
		h.runTo(int64(h.terms.RevealEnd) + 2)
		paused := d.snapshot("insufficient_funds after partial allocation")
		require.Equal(t, "paused", paused.State.Task.Status.Kind)
		require.Equal(t, "insufficient_funds", paused.State.Task.Status.Reason)
		require.Equal(t, "allocate_base", paused.State.View.Auction.Settlement.Phase)
		require.Equal(t, uint16(2), paused.State.View.Auction.Settlement.Cursor)
		if name == "expiry" {
			d.expire()
			return
		}
		amount := auctionInt(t, paused.State.Funding.MaxAttemptCharge).MulRaw(10)
		h.execute(h.keys.Sponsor, map[string]any{"async": map[string]any{"fund_task": map[string]any{"task_id": paused.State.View.Auction.TaskID}}}, sdk.NewCoins(sdk.NewCoin("ugas", amount)))
		topped := d.snapshot("third-party top-up does not resume")
		require.Equal(t, "paused", topped.State.Task.Status.Kind)
		require.Equal(t, paused.State.View.Auction, topped.State.View.Auction)
		require.Equal(t, paused.Balances[auctionAddress(h.keys.Sponsor).String()].AmountOf("ugas").Sub(amount), topped.Balances[auctionAddress(h.keys.Sponsor).String()].AmountOf("ugas"))
		h.execute(h.keys.Seller, map[string]any{"update_budget": map[string]any{"gas_per_poll": auctionPollCap, "max_gas_price": "1"}}, sdk.Coins{})
		budget := d.snapshot("seller ceiling increase still does not resume")
		require.Equal(t, "paused", budget.State.Task.Status.Kind)
		require.Equal(t, topped.State.View.Auction.Settlement, budget.State.View.Auction.Settlement)
		require.Equal(t, topped.State.Bids, budget.State.Bids)
		h.execute(h.keys.Seller, map[string]any{"async": map[string]any{"resume_task": map[string]any{"task_id": paused.State.View.Auction.TaskID}}}, sdk.Coins{})
		d.snapshot("seller explicit resume")
		d.settle(false)
	}
	h.pruneAndClaim()
	d.snapshot("host pruned; finalized principal claims paid")
	d.rejectedClaims("duplicate claims rejected after pruning")
}
func (d *nativeAuctionDemo) expire() {
	h, t := d.h, d.h.t
	h.runTo(int64(h.terms.ExpiresAt) - 3)
	d.rejectedClaims("provisional claims rejected at E-1")
	h.runTo(int64(h.terms.ExpiresAt) + 1)
	before := d.snapshot("fixed expiry; host absent before principal claims")
	require.Nil(t, before.State.Task)
	require.NotNil(t, before.State.View.Host.Unavailable)
	require.Equal(t, "active", before.State.View.Auction.State)
	require.Positive(t, auctionInt(t, before.State.Bids[0].AllocatedLots).Int64())
	for i := range auctionFixtureBook() {
		h.execute(h.keys.Sponsor, map[string]any{"claim_bid": map[string]any{"bidder": auctionAddress(h.keys.Bidders[i]).String()}}, sdk.Coins{})
	}
	h.execute(h.keys.Sponsor, map[string]any{"claim_seller": map[string]any{}}, sdk.Coins{})
	after := d.snapshot("lazy abort and full principal refunds after host pruning")
	require.Equal(t, "aborted", after.State.View.Auction.State)
	require.Equal(t, auctionClaimTotals{"0", "29", "50", "0"}, after.State.View.Auction.Claimed)
	for i, bid := range auctionFixtureBook() {
		bidder, beneficiary := auctionAddress(h.keys.Bidders[i]).String(), auctionAddress(h.keys.Beneficiaries[i]).String()
		require.Equal(t, before.Balances[bidder].AmountOf("uquote").Add(auctionInt(t, bid.Collateral)), after.Balances[bidder].AmountOf("uquote"))
		require.Equal(t, before.Balances[beneficiary].AmountOf("usale"), after.Balances[beneficiary].AmountOf("usale"))
		require.True(t, after.State.Bids[i].Claimed)
	}
	seller := auctionAddress(h.keys.Seller).String()
	require.Equal(t, before.Balances[seller].AmountOf("usale").AddRaw(50), after.Balances[seller].AmountOf("usale"))
	require.Equal(t, before.Balances[seller].AmountOf("uquote"), after.Balances[seller].AmountOf("uquote"))
	require.Zero(t, after.Balances[h.contract.String()].AmountOf("usale").Int64())
	require.Zero(t, after.Balances[h.contract.String()].AmountOf("uquote").Int64())
	d.rejectedClaims("duplicate aborted claims rejected")
}

func TestAuctionDemoSaltSnapshotImmutable(t *testing.T) {
	path := filepath.Join(t.TempDir(), "commitments.json")
	require.NoError(t, writeAuctionSaltSnapshot(path, []string{"first random salt snapshot"}))
	info, err := os.Stat(path)
	require.NoError(t, err)
	require.Equal(t, os.FileMode(0600), info.Mode().Perm())
	require.Error(t, writeAuctionSaltSnapshot(path, []string{"replacement"}))
	raw, err := os.ReadFile(path)
	require.NoError(t, err)
	require.Contains(t, string(raw), "first random salt snapshot")
}
