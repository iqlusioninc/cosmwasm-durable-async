//go:build upstream

package async_test

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"sync/atomic"
	"syscall"
	"testing"
	"time"

	abci "github.com/cometbft/cometbft/abci/types"
	coretypes "github.com/cometbft/cometbft/rpc/core/types"
	cmttypes "github.com/cometbft/cometbft/types"
	"github.com/stretchr/testify/require"
)

// These utilities belong only to the opt-in local-node tutorial. The journal is
// scoped to cooperating processes on local Darwin/Linux filesystems, not a
// distributed transaction log. No wallet key material is written here.
type DemoRPC interface {
	BroadcastTxSync(context.Context, cmttypes.Tx) (*coretypes.ResultBroadcastTx, error)
	Tx(context.Context, []byte, bool) (*coretypes.ResultTx, error)
}

type TxJournal struct {
	TxHex   string     `json:"tx_hex"`
	Hash    string     `json:"hash"`
	Stage   string     `json:"stage"`
	Receipt *TxReceipt `json:"receipt"`
}

type TxReceipt struct {
	Hash          string  `json:"hash"`
	Outcome       string  `json:"outcome"`
	CheckTxCode   *uint32 `json:"check_tx_code"`
	DeliverTxCode *uint32 `json:"deliver_tx_code"`
	Height        *int64  `json:"height"`
}

func writeIntent(path string, tx []byte) (string, error) {
	if len(tx) == 0 {
		return "", errors.New("empty signed transaction")
	}
	lock, err := lockJournal(path)
	if err != nil {
		return "", err
	}
	defer lock.Close()
	entry := TxJournal{TxHex: hex.EncodeToString(tx), Hash: fmt.Sprintf("%X", cmttypes.Tx(tx).Hash()), Stage: "prepared"}
	if err := storeJournal(path, entry, true); err != nil {
		return "", err
	}
	return entry.Hash, nil
}

// Readers need no lock: every journal installation publishes a complete file.
// In particular an RPC test/observer may read while the broadcaster owns its lock.
func readJournal(path string) (TxJournal, error) {
	file, err := os.Open(path)
	if err != nil {
		return TxJournal{}, err
	}
	defer file.Close()
	var entry TxJournal
	decoder := json.NewDecoder(io.LimitReader(file, 32<<20))
	decoder.DisallowUnknownFields()
	if err := decoder.Decode(&entry); err != nil {
		return TxJournal{}, err
	}
	if err := decoder.Decode(new(any)); err != io.EOF {
		return TxJournal{}, errors.New("trailing or oversized journal data")
	}
	tx, err := hex.DecodeString(entry.TxHex)
	if err != nil || len(tx) == 0 || entry.Hash != fmt.Sprintf("%X", cmttypes.Tx(tx).Hash()) {
		return TxJournal{}, errors.New("journal signed bytes/hash mismatch")
	}
	switch entry.Stage {
	case "prepared":
		if entry.Receipt != nil {
			return TxJournal{}, errors.New("prepared journal contains result evidence")
		}
	case "attempted", "resolved":
		if entry.Receipt != nil {
			if err := validateReceipt(entry.Hash, *entry.Receipt); err != nil {
				return TxJournal{}, err
			}
		}
		if entry.Stage == "resolved" && (entry.Receipt == nil || entry.Receipt.Outcome == "unknown") {
			return TxJournal{}, errors.New("resolved journal lacks definitive result")
		}
		if entry.Stage == "attempted" && entry.Receipt != nil && entry.Receipt.Outcome != "unknown" {
			return TxJournal{}, errors.New("attempted journal contains unmarked definitive result")
		}
	default:
		return TxJournal{}, errors.New("invalid journal stage")
	}
	return entry, nil
}

// The stable sibling inode is never removed. Advisory locks serialize cooperating
// local writers, including two broadcastOnce callers, and release after a crash.
// A concurrent caller fails promptly; the owner has bounded RPC/overall deadlines.
func lockJournal(path string) (*os.File, error) {
	file, err := os.OpenFile(path+".lock", os.O_CREATE|os.O_RDWR|syscall.O_NOFOLLOW, 0600)
	if err != nil {
		return nil, err
	}
	if err := syscall.Flock(int(file.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); err != nil {
		_ = file.Close()
		return nil, fmt.Errorf("journal already in use: %w", err)
	}
	return file, nil
}

func storeJournal(path string, entry TxJournal, initial bool) error {
	encoded, err := json.Marshal(entry)
	if err != nil {
		return err
	}
	dir := filepath.Dir(path)
	temp, err := os.CreateTemp(dir, "."+filepath.Base(path)+".tmp-*")
	if err != nil {
		return err
	}
	defer os.Remove(temp.Name())
	if _, err := temp.Write(encoded); err != nil {
		_ = temp.Close()
		return err
	}
	if err := temp.Sync(); err != nil {
		_ = temp.Close()
		return err
	}
	if err := temp.Close(); err != nil {
		return err
	}
	if initial {
		// Link is an atomic, no-replace installation on supported local filesystems.
		// A rename alone could overwrite an existing initial intent.
		if err := os.Link(temp.Name(), path); err != nil {
			return fmt.Errorf("install new intent without replacement: %w", err)
		}
		if err := os.Remove(temp.Name()); err != nil {
			return err
		}
	} else if err := os.Rename(temp.Name(), path); err != nil {
		return err
	}
	directory, err := os.Open(dir)
	if err != nil {
		return err
	}
	defer directory.Close()
	return directory.Sync()
}

func validateReceipt(hash string, receipt TxReceipt) error {
	if receipt.Hash != hash {
		return errors.New("receipt does not match original transaction hash")
	}
	switch receipt.Outcome {
	case "unknown":
		if receipt.DeliverTxCode != nil || receipt.Height != nil || (receipt.CheckTxCode != nil && *receipt.CheckTxCode != 0) {
			return errors.New("unknown receipt contains definitive result fields")
		}
	case "check_tx_rejected":
		if receipt.CheckTxCode == nil || *receipt.CheckTxCode == 0 || receipt.DeliverTxCode != nil || receipt.Height != nil {
			return errors.New("invalid CheckTx rejection evidence")
		}
	case "committed":
		if receipt.DeliverTxCode == nil || receipt.Height == nil || *receipt.Height <= 0 || (receipt.CheckTxCode != nil && *receipt.CheckTxCode != 0) {
			return errors.New("invalid committed transaction evidence")
		}
	default:
		return errors.New("invalid transaction outcome")
	}
	return nil
}

const demoRPCDeadline = 2 * time.Second

func broadcastOnce(ctx context.Context, rpc DemoRPC, path string) (TxReceipt, error) {
	ctx, cancel := context.WithTimeout(ctx, 15*time.Second)
	defer cancel()
	lock, err := lockJournal(path)
	if err != nil {
		return TxReceipt{}, err
	}
	defer lock.Close()
	entry, err := readJournal(path)
	if err != nil {
		return TxReceipt{}, err
	}
	if entry.Stage == "resolved" {
		return *entry.Receipt, receiptError(*entry.Receipt)
	}
	receipt := TxReceipt{Hash: entry.Hash, Outcome: "unknown"}
	if entry.Receipt != nil {
		receipt = *entry.Receipt
	}
	if entry.Stage == "prepared" {
		if err := ctx.Err(); err != nil {
			return receipt, err
		}
		// Even a crash immediately after this sync is uncertain. Only a hash
		// lookup is allowed thereafter, whether or not the RPC actually began.
		entry.Stage, entry.Receipt = "attempted", &receipt
		if err := storeJournal(path, entry, false); err != nil {
			return receipt, err
		}
		tx, _ := hex.DecodeString(entry.TxHex) // validated by readJournal
		rpcCtx, rpcCancel := context.WithTimeout(ctx, demoRPCDeadline)
		result, broadcastErr := rpc.BroadcastTxSync(rpcCtx, cmttypes.Tx(tx))
		rpcCancel()
		if broadcastErr == nil && result != nil && strings.EqualFold(fmt.Sprintf("%X", result.Hash), entry.Hash) {
			code := result.Code
			receipt.CheckTxCode = &code
			if code != 0 {
				receipt.Outcome = "check_tx_rejected"
			}
			if err := recordResultLocked(path, receipt); err != nil {
				return receipt, err
			}
			if code != 0 {
				return receipt, receiptError(receipt)
			}
		}
	}
	hash, _ := hex.DecodeString(entry.Hash)
	var lastErr error
	for ctx.Err() == nil {
		rpcCtx, rpcCancel := context.WithTimeout(ctx, demoRPCDeadline)
		result, lookupErr := rpc.Tx(rpcCtx, hash, false)
		rpcCancel()
		if lookupErr == nil && result != nil {
			if fmt.Sprintf("%X", result.Hash) != entry.Hash || result.Height <= 0 || (len(result.Tx) != 0 && fmt.Sprintf("%X", result.Tx.Hash()) != entry.Hash) {
				return receipt, errors.New("lookup returned evidence for a different or invalid transaction")
			}
			code, height := result.TxResult.Code, result.Height
			receipt.Outcome, receipt.DeliverTxCode, receipt.Height = "committed", &code, &height
			if err := recordResultLocked(path, receipt); err != nil {
				return receipt, err
			}
			return receipt, receiptError(receipt)
		}
		lastErr = lookupErr
		timer := time.NewTimer(25 * time.Millisecond)
		select {
		case <-ctx.Done():
			if !timer.Stop() {
				<-timer.C
			}
		case <-timer.C:
		}
	}
	return receipt, fmt.Errorf("transaction %s unresolved; query this hash only: %w (last lookup: %v)", entry.Hash, ctx.Err(), lastErr)
}

func receiptError(receipt TxReceipt) error {
	switch receipt.Outcome {
	case "check_tx_rejected":
		return fmt.Errorf("transaction %s rejected by CheckTx: code %d", receipt.Hash, *receipt.CheckTxCode)
	case "committed":
		if *receipt.DeliverTxCode != 0 {
			return fmt.Errorf("transaction %s committed with execution code %d", receipt.Hash, *receipt.DeliverTxCode)
		}
		return nil
	default:
		return fmt.Errorf("transaction %s unresolved", receipt.Hash)
	}
}

func recordResult(path string, receipt TxReceipt) error {
	lock, err := lockJournal(path)
	if err != nil {
		return err
	}
	defer lock.Close()
	return recordResultLocked(path, receipt)
}

func recordResultLocked(path string, receipt TxReceipt) error {
	entry, err := readJournal(path)
	if err != nil {
		return err
	}
	if err := validateReceipt(entry.Hash, receipt); err != nil {
		return err
	}
	if entry.Stage == "prepared" {
		return errors.New("cannot resolve an unattempted intent")
	}
	if entry.Receipt != nil && entry.Receipt.CheckTxCode != nil {
		if receipt.CheckTxCode != nil && *receipt.CheckTxCode != *entry.Receipt.CheckTxCode {
			return errors.New("cannot replace known CheckTx evidence")
		}
		receipt.CheckTxCode = entry.Receipt.CheckTxCode
	}
	if entry.Stage == "resolved" {
		if reflect.DeepEqual(*entry.Receipt, receipt) {
			return nil
		}
		return errors.New("cannot replace resolved transaction evidence")
	}
	entry.Receipt = &receipt
	if receipt.Outcome != "unknown" {
		entry.Stage = "resolved"
	}
	return storeJournal(path, entry, false)
}

type journalRPC struct {
	broadcast func(context.Context, cmttypes.Tx) (*coretypes.ResultBroadcastTx, error)
	lookup    func(context.Context, []byte, bool) (*coretypes.ResultTx, error)
}

func (r journalRPC) BroadcastTxSync(ctx context.Context, tx cmttypes.Tx) (*coretypes.ResultBroadcastTx, error) {
	return r.broadcast(ctx, tx)
}
func (r journalRPC) Tx(ctx context.Context, hash []byte, prove bool) (*coretypes.ResultTx, error) {
	return r.lookup(ctx, hash, prove)
}

func journalFixture(t *testing.T) (string, []byte, string) {
	t.Helper()
	tx := []byte("signed local test transaction")
	hash := sha256.Sum256(tx)
	return filepath.Join(t.TempDir(), "intent.json"), tx, fmt.Sprintf("%X", hash)
}

// A possible submission followed by a lost response must never submit twice.
func TestDCADemoJournalNoRebroadcastAfterLostResponse(t *testing.T) {
	path, tx, originalHash := journalFixture(t)
	hash, err := writeIntent(path, tx)
	require.NoError(t, err)
	require.Equal(t, originalHash, hash)
	var broadcastCalls int
	var lookupHash string
	rpc := journalRPC{
		broadcast: func(ctx context.Context, got cmttypes.Tx) (*coretypes.ResultBroadcastTx, error) {
			broadcastCalls++
			require.Equal(t, tx, []byte(got))
			stored, readErr := readJournal(path)
			require.NoError(t, readErr)
			require.Equal(t, "attempted", stored.Stage, "intent must be durable before RPC")
			return nil, errors.New("response lost after possible submission")
		},
		lookup: func(ctx context.Context, hash []byte, prove bool) (*coretypes.ResultTx, error) {
			lookupHash = fmt.Sprintf("%X", hash)
			return nil, errors.New("transaction not yet found")
		},
	}
	for i := 0; i < 2; i++ {
		ctx, cancel := context.WithTimeout(context.Background(), 35*time.Millisecond)
		receipt, runErr := broadcastOnce(ctx, rpc, path)
		cancel()
		require.Error(t, runErr, "unresolved outcome must stop the scenario")
		require.Equal(t, "unknown", receipt.Outcome)
		require.Nil(t, receipt.CheckTxCode)
		require.Nil(t, receipt.DeliverTxCode)
		require.Nil(t, receipt.Height)
	}
	require.Equal(t, 1, broadcastCalls)
	require.Equal(t, originalHash, lookupHash)
	stored, err := readJournal(path)
	require.NoError(t, err)
	require.Equal(t, "attempted", stored.Stage)
	require.Equal(t, hex.EncodeToString(tx), stored.TxHex)
	encoded, err := json.Marshal(stored.Receipt)
	require.NoError(t, err)
	require.Contains(t, string(encoded), `"height":null`)
}

func TestDCADemoJournalRejectsCorruptIntent(t *testing.T) {
	path, tx, hash := journalFixture(t)
	for _, entry := range []TxJournal{
		{TxHex: hex.EncodeToString(tx), Hash: "BAD", Stage: "prepared"},
		{TxHex: "not-hex", Hash: hash, Stage: "prepared"},
		{TxHex: "", Hash: hash, Stage: "prepared"},
		{TxHex: hex.EncodeToString(tx), Hash: hash, Stage: "unrecognized"},
		{TxHex: hex.EncodeToString(tx), Hash: hash, Stage: "resolved"},
	} {
		encoded, err := json.Marshal(entry)
		require.NoError(t, err)
		require.NoError(t, os.WriteFile(path, encoded, 0600))
		_, err = readJournal(path)
		require.Error(t, err, "invalid intent must not reach submission")
	}
}

func TestDCADemoJournalRefusesExistingIntent(t *testing.T) {
	path, tx, _ := journalFixture(t)
	_, err := writeIntent(path, tx)
	require.NoError(t, err)
	_, err = writeIntent(path, []byte("different transaction"))
	require.Error(t, err)
	stored, err := readJournal(path)
	require.NoError(t, err)
	require.Equal(t, hex.EncodeToString(tx), stored.TxHex)
	mode, err := os.Stat(path)
	require.NoError(t, err)
	require.Equal(t, os.FileMode(0600), mode.Mode().Perm())
}

func TestDCADemoJournalAttemptedCrashIsLookupOnly(t *testing.T) {
	path, tx, hash := journalFixture(t)
	entry := TxJournal{TxHex: hex.EncodeToString(tx), Hash: hash, Stage: "attempted"}
	encoded, err := json.Marshal(entry)
	require.NoError(t, err)
	require.NoError(t, os.WriteFile(path, encoded, 0600))
	var broadcasts int
	var lookupHash string
	rpc := journalRPC{
		broadcast: func(context.Context, cmttypes.Tx) (*coretypes.ResultBroadcastTx, error) {
			broadcasts++
			return nil, errors.New("must not submit an attempted journal")
		},
		lookup: func(_ context.Context, got []byte, _ bool) (*coretypes.ResultTx, error) {
			lookupHash = fmt.Sprintf("%X", got)
			return &coretypes.ResultTx{Hash: got, Height: 7, Tx: tx, TxResult: abci.ExecTxResult{Code: 0}}, nil
		},
	}
	receipt, err := broadcastOnce(context.Background(), rpc, path)
	require.NoError(t, err)
	require.Zero(t, broadcasts)
	require.Equal(t, hash, lookupHash)
	require.Equal(t, "committed", receipt.Outcome)
	require.Nil(t, receipt.CheckTxCode, "lookup does not establish historical CheckTx")
	require.Equal(t, uint32(0), *receipt.DeliverTxCode)
	require.Equal(t, int64(7), *receipt.Height)
}

func TestDCADemoJournalConcurrentCallersSubmitOnce(t *testing.T) {
	path, tx, _ := journalFixture(t)
	_, err := writeIntent(path, tx)
	require.NoError(t, err)
	entered := make(chan struct{})
	release := make(chan struct{})
	var broadcasts atomic.Int32
	rpc := journalRPC{
		broadcast: func(ctx context.Context, got cmttypes.Tx) (*coretypes.ResultBroadcastTx, error) {
			broadcasts.Add(1)
			close(entered)
			select {
			case <-release:
				return &coretypes.ResultBroadcastTx{Hash: got.Hash()}, nil
			case <-ctx.Done():
				return nil, ctx.Err()
			}
		},
		lookup: func(_ context.Context, got []byte, _ bool) (*coretypes.ResultTx, error) {
			return &coretypes.ResultTx{Hash: got, Height: 8, Tx: tx}, nil
		},
	}
	first := make(chan error, 1)
	go func() { _, err := broadcastOnce(context.Background(), rpc, path); first <- err }()
	select {
	case <-entered:
	case <-time.After(time.Second):
		t.Fatal("first broadcast never entered")
	}
	_, err = broadcastOnce(context.Background(), rpc, path)
	require.Error(t, err, "concurrent caller must fail before RPC")
	close(release)
	require.NoError(t, <-first)
	require.Equal(t, int32(1), broadcasts.Load())
}

func TestDCADemoJournalResultPreservesIntent(t *testing.T) {
	path, tx, hash := journalFixture(t)
	_, err := writeIntent(path, tx)
	require.NoError(t, err)
	wrong := TxReceipt{Hash: "another-hash", Outcome: "unknown"}
	require.Error(t, recordResult(path, wrong))
	zero, height := uint32(0), int64(11)
	committed := TxReceipt{Hash: hash, Outcome: "committed", DeliverTxCode: &zero, Height: &height}
	require.Error(t, recordResult(path, committed), "unattempted intent cannot be resolved")
	rpc := journalRPC{
		broadcast: func(_ context.Context, got cmttypes.Tx) (*coretypes.ResultBroadcastTx, error) {
			return &coretypes.ResultBroadcastTx{Hash: got.Hash()}, nil
		},
		lookup: func(_ context.Context, got []byte, _ bool) (*coretypes.ResultTx, error) {
			return &coretypes.ResultTx{Hash: got, Height: height, Tx: tx}, nil
		},
	}
	receipt, err := broadcastOnce(context.Background(), rpc, path)
	require.NoError(t, err)
	require.NoError(t, recordResult(path, receipt), "identical evidence may be recorded again")
	require.Error(t, recordResult(path, TxReceipt{Hash: hash, Outcome: "unknown"}))
	stored, err := readJournal(path)
	require.NoError(t, err)
	require.Equal(t, "resolved", stored.Stage)
	require.Equal(t, hash, stored.Hash)
	require.Equal(t, hex.EncodeToString(tx), stored.TxHex)
	require.Equal(t, &zero, stored.Receipt.CheckTxCode)
	before, err := os.ReadFile(path)
	require.NoError(t, err)
	_, err = broadcastOnce(context.Background(), rpc, path)
	require.NoError(t, err)
	after, err := os.ReadFile(path)
	require.NoError(t, err)
	require.Equal(t, before, after)
}

func TestDCADemoJournalCheckTxRejectionIsDefinitive(t *testing.T) {
	path, tx, hash := journalFixture(t)
	_, err := writeIntent(path, tx)
	require.NoError(t, err)
	var broadcasts, lookups int
	rpc := journalRPC{
		broadcast: func(_ context.Context, got cmttypes.Tx) (*coretypes.ResultBroadcastTx, error) {
			broadcasts++
			return &coretypes.ResultBroadcastTx{Hash: got.Hash(), Code: 5}, nil
		},
		lookup: func(context.Context, []byte, bool) (*coretypes.ResultTx, error) {
			lookups++
			return nil, errors.New("rejected transaction has no commit")
		},
	}
	for i := 0; i < 2; i++ {
		receipt, err := broadcastOnce(context.Background(), rpc, path)
		require.Error(t, err)
		require.Equal(t, hash, receipt.Hash)
		require.Equal(t, "check_tx_rejected", receipt.Outcome)
		require.Equal(t, uint32(5), *receipt.CheckTxCode)
		require.Nil(t, receipt.DeliverTxCode)
		require.Nil(t, receipt.Height)
	}
	require.Equal(t, 1, broadcasts)
	require.Zero(t, lookups)
}

func TestDCADemoRPCDeadline(t *testing.T) {
	path, tx, _ := journalFixture(t)
	_, err := writeIntent(path, tx)
	require.NoError(t, err)
	var calls int
	rpc := journalRPC{
		broadcast: func(ctx context.Context, _ cmttypes.Tx) (*coretypes.ResultBroadcastTx, error) {
			calls++
			_, bounded := ctx.Deadline()
			require.True(t, bounded)
			<-ctx.Done()
			return nil, ctx.Err()
		},
		lookup: func(ctx context.Context, _ []byte, _ bool) (*coretypes.ResultTx, error) {
			<-ctx.Done()
			return nil, ctx.Err()
		},
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Millisecond)
	defer cancel()
	started := time.Now()
	_, err = broadcastOnce(ctx, rpc, path)
	require.Error(t, err)
	require.Equal(t, 1, calls)
	require.Less(t, time.Since(started), time.Second)
}
