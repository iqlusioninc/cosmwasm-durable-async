//go:build upstream

package async_test

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"sync"
	"syscall"
	"testing"
	"time"

	abci "github.com/cometbft/cometbft/abci/types"
	coretypes "github.com/cometbft/cometbft/rpc/core/types"
	cmttypes "github.com/cometbft/cometbft/types"
	"github.com/stretchr/testify/require"
)

func TestAuctionDemoBatchJournalOrder(t *testing.T) {
	paths := []string{}
	rawByHash := map[string][]byte{}
	for _, raw := range [][]byte{[]byte("first bid"), []byte("second bid"), []byte("third bid")} {
		path := filepath.Join(t.TempDir(), "intent.json")
		hash, err := writeIntent(path, raw)
		require.NoError(t, err)
		rawByHash[hash] = raw
		paths = append(paths, path)
	}
	var mu sync.Mutex
	var order []string
	released := make(chan struct{})
	rpc := journalRPC{
		broadcast: func(_ context.Context, tx cmttypes.Tx) (*coretypes.ResultBroadcastTx, error) {
			// Unordered parallel submission would place the dust-sensitive first
			// bid last. Delayed first response must block later first attempts.
			if string(tx) == "first bid" {
				time.Sleep(20 * time.Millisecond)
			}
			mu.Lock()
			order = append(order, string(tx))
			mu.Unlock()
			return &coretypes.ResultBroadcastTx{Hash: tx.Hash()}, nil
		},
		lookup: func(ctx context.Context, hash []byte, _ bool) (*coretypes.ResultTx, error) {
			select {
			case <-released:
			case <-ctx.Done():
				return nil, ctx.Err()
			}
			return &coretypes.ResultTx{Hash: hash, Height: 9, Tx: rawByHash[fmt.Sprintf("%X", hash)], TxResult: abci.ExecTxResult{}}, nil
		},
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	receipts, err := journalAuctionBatch(ctx, rpc, paths, func() {
		mu.Lock()
		defer mu.Unlock()
		require.Equal(t, []string{"first bid", "second bid", "third bid"}, order)
		close(released)
	})
	require.NoError(t, err)
	require.Len(t, receipts, 3)
	for _, receipt := range receipts {
		require.Equal(t, "committed", receipt.Outcome)
	}
}

// Auction transactions use the same durable signed-byte journal as the DCA
// tutorial. A lost RPC answer never authorizes a new submission or signature.
func TestAuctionDemoJournalNoRebroadcast(t *testing.T) {
	path, tx, originalHash := journalFixture(t)
	hash, err := writeIntent(path, tx)
	require.NoError(t, err)
	require.Equal(t, originalHash, hash)
	broadcasts, lookups := 0, 0
	committed := false
	rpc := journalRPC{
		broadcast: func(_ context.Context, got cmttypes.Tx) (*coretypes.ResultBroadcastTx, error) {
			broadcasts++
			require.Equal(t, tx, []byte(got))
			stored, err := readJournal(path)
			require.NoError(t, err)
			require.Equal(t, "attempted", stored.Stage, "persist uncertainty before calling RPC")
			return nil, errors.New("lost response after possible submission")
		},
		lookup: func(_ context.Context, hash []byte, prove bool) (*coretypes.ResultTx, error) {
			lookups++
			require.Equal(t, originalHash, fmt.Sprintf("%X", hash), "lookup must use the original signed hash")
			require.False(t, prove)
			if !committed {
				return nil, errors.New("original hash not yet found")
			}
			return &coretypes.ResultTx{Hash: hash, Height: 9, Tx: cmttypes.Tx(tx), TxResult: abci.ExecTxResult{Code: 0}}, nil
		},
	}
	for attempt := 0; attempt < 2; attempt++ {
		ctx, cancel := context.WithTimeout(context.Background(), 35*time.Millisecond)
		receipt, err := broadcastOnce(ctx, rpc, path)
		cancel()
		require.Error(t, err, "an unresolved transaction must halt the scenario")
		require.Contains(t, err.Error(), originalHash)
		require.Equal(t, "unknown", receipt.Outcome)
		require.Nil(t, receipt.Height)
	}
	require.Equal(t, 1, broadcasts)
	require.Positive(t, lookups)
	committed = true
	receipt, err := broadcastOnce(context.Background(), rpc, path)
	require.NoError(t, err)
	require.Equal(t, originalHash, receipt.Hash)
	require.Equal(t, "committed", receipt.Outcome)
	require.Equal(t, int64(9), *receipt.Height)
	require.Equal(t, 1, broadcasts, "resolving the same intent must only query")
	lookupCount := lookups
	again, err := broadcastOnce(context.Background(), rpc, path)
	require.NoError(t, err)
	require.Equal(t, receipt, again)
	require.Equal(t, lookupCount, lookups, "a resolved journal needs no further RPC")
}

func TestAuctionDemoRPCDeadline(t *testing.T) {
	for _, blockAt := range []string{"broadcast", "lookup"} {
		t.Run(blockAt, func(t *testing.T) {
			path, tx, _ := journalFixture(t)
			_, err := writeIntent(path, tx)
			require.NoError(t, err)
			ctx, cancel := context.WithTimeout(context.Background(), 50*time.Millisecond)
			defer cancel()
			outerDeadline, _ := ctx.Deadline()
			checkDeadline := func(rpcCtx context.Context) {
				deadline, bounded := rpcCtx.Deadline()
				require.True(t, bounded)
				require.False(t, deadline.After(outerDeadline), "RPC may not outlive the scenario deadline")
			}
			broadcasts, lookups := 0, 0
			rpc := journalRPC{
				broadcast: func(rpcCtx context.Context, _ cmttypes.Tx) (*coretypes.ResultBroadcastTx, error) {
					broadcasts++
					checkDeadline(rpcCtx)
					if blockAt == "broadcast" {
						<-rpcCtx.Done()
						return nil, rpcCtx.Err()
					}
					return &coretypes.ResultBroadcastTx{Hash: cmttypes.Tx(tx).Hash()}, nil
				},
				lookup: func(rpcCtx context.Context, _ []byte, _ bool) (*coretypes.ResultTx, error) {
					lookups++
					checkDeadline(rpcCtx)
					<-rpcCtx.Done()
					return nil, rpcCtx.Err()
				},
			}
			started := time.Now()
			receipt, err := broadcastOnce(ctx, rpc, path)
			require.Error(t, err)
			require.Equal(t, "unknown", receipt.Outcome)
			require.Equal(t, 1, broadcasts)
			if blockAt == "lookup" {
				require.Equal(t, 1, lookups)
			}
			require.Less(t, time.Since(started), time.Second)
		})
	}
}

func TestAuctionDemoCleanupHelper(t *testing.T) {
	if os.Getenv("NATIVE_AUCTION_CLEANUP_HELPER") != "1" {
		t.Skip("owned auction cleanup helper child only")
	}
	signals := make(chan os.Signal, 1)
	signal.Notify(signals, syscall.SIGTERM)
	defer signal.Stop(signals)
	require.NoError(t, writeDemoMarker(os.Getenv("NATIVE_AUCTION_CLEANUP_READY"), os.Getpid()))
	<-signals
}

func TestAuctionDemoCleanupOwnedChild(t *testing.T) {
	executable, err := os.Executable()
	require.NoError(t, err)
	start := func() *ownedDemoChild {
		ready := filepath.Join(t.TempDir(), "ready.json")
		command := exec.Command(executable, "-test.run=^TestAuctionDemoCleanupHelper$", "-test.timeout=30s")
		command.Env = append(os.Environ(), "NATIVE_AUCTION_CLEANUP_HELPER=1", "NATIVE_AUCTION_CLEANUP_READY="+ready)
		child, err := startDemoChild(command)
		require.NoError(t, err)
		t.Cleanup(func() { _ = child.stop() })
		require.Eventually(t, func() bool {
			var pid int
			return readDemoMarker(ready, &pid) == nil && pid == child.command.Process.Pid
		}, 10*time.Second, 10*time.Millisecond, "helper must install its signal handler first")
		return child
	}
	owned, unrelated := start(), start()
	require.NoError(t, owned.stop())
	select {
	case <-owned.done:
	case <-time.After(time.Second):
		t.Fatal("owned child was not stopped and reaped")
	}
	require.NoError(t, owned.err, "owned child should exit through its installed signal handler")
	select {
	case <-unrelated.done:
		t.Fatal("unrelated process exited during owned cleanup")
	default:
	}
	require.NoError(t, unrelated.command.Process.Signal(syscall.Signal(0)), "unrelated process must survive")
	require.NoError(t, owned.stop(), "stop must be idempotent")
}
