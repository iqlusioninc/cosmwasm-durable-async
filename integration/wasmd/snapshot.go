package async

import (
	"bytes"
	"crypto/sha256"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"

	"cosmossdk.io/log"
	"cosmossdk.io/store/snapshots"
	snapshot "cosmossdk.io/store/snapshots/types"
	storetypes "cosmossdk.io/store/types"
	"github.com/CosmWasm/wasmd/x/wasm/ioutils"
	wasmtypes "github.com/CosmWasm/wasmd/x/wasm/types"
	abci "github.com/cometbft/cometbft/abci/types"
	tmproto "github.com/cometbft/cometbft/proto/tendermint/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
)

// Snapshotter preserves wasmd's existing bytecode-only extension wire format.
// Metadata/queues/funds are authenticated by the SDK multistore snapshot. Native
// machine-specific compiled caches are neither read nor serialized here.
type Snapshotter struct {
	snapshot.ExtensionSnapshotter
	cms           storetypes.CommitMultiStore
	keeper        *Keeper
	restoring     bool
	restored      bool
	trustedHash   []byte
	offeredHeight uint64
	marker        string
}

func NewSnapshotter(cms storetypes.CommitMultiStore, k *Keeper, wasm snapshot.ExtensionSnapshotter, homePath string) *Snapshotter {
	return &Snapshotter{ExtensionSnapshotter: wasm, cms: cms, keeper: k, marker: restoreMarker(homePath)}
}

func restoreMarker(homePath string) string {
	return filepath.Join(homePath, ".native-async-restore.pending")
}

// CheckSnapshotStartup is called before opening wasmvm or serving ABCI. A crash
// or failed restore cannot bypass the trusted-hash check by restarting from the
// partially imported database. Recovery requires a fresh application home; do
// not remove this marker and continue using the rejected state.
func CheckSnapshotStartup(homePath string) error {
	_, err := os.Lstat(restoreMarker(homePath))
	if os.IsNotExist(err) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("inspect native async restore marker: %w", err)
	}
	return fmt.Errorf("incomplete or rejected native async snapshot restoration: use a fresh application home")
}

func syncDirectory(path string) error {
	dir, err := os.Open(path)
	if err != nil {
		return err
	}
	defer dir.Close()
	return dir.Sync()
}

func (s *Snapshotter) markRestore() error {
	file, err := os.OpenFile(s.marker, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		return err
	}
	defer file.Close()
	if _, err = file.WriteString(fmt.Sprintf("height=%d app_hash=%x\n", s.offeredHeight, s.trustedHash)); err != nil {
		return err
	}
	if err = file.Sync(); err != nil {
		return err
	}
	if err = file.Close(); err != nil {
		return err
	}
	return syncDirectory(filepath.Dir(s.marker))
}

func (s *Snapshotter) finishRestore() error {
	if err := os.Remove(s.marker); err != nil {
		return err
	}
	return syncDirectory(filepath.Dir(s.marker))
}

func (s *Snapshotter) context(height uint64) sdk.Context {
	return sdk.NewContext(s.cms, tmproto.Header{Height: int64(height)}, false, log.NewNopLogger())
}

// CheckReady prevents block execution after an incomplete or rejected restore.
// The SDK cannot roll back a failed IAVL restore: use a fresh data directory.
func (s *Snapshotter) CheckReady() error {
	if s.restoring {
		return fmt.Errorf("native async snapshot restoration incomplete or rejected")
	}
	return nil
}

func (s *Snapshotter) OfferSnapshot(offer func(*abci.RequestOfferSnapshot) (*abci.ResponseOfferSnapshot, error), req *abci.RequestOfferSnapshot) (*abci.ResponseOfferSnapshot, error) {
	response, err := offer(req)
	if err == nil && response.Result == abci.ResponseOfferSnapshot_ACCEPT {
		s.restoring = true
		s.restored = false
		s.trustedHash = bytes.Clone(req.AppHash)
		s.offeredHeight = req.Snapshot.Height
		// No chunks can be accepted until the durable marker is present.
		if err := s.markRestore(); err != nil {
			return &abci.ResponseOfferSnapshot{Result: abci.ResponseOfferSnapshot_ABORT}, nil
		}
	}
	return response, err
}

// ApplySnapshotChunk must replace BaseApp's method, which discards RestoreChunk's
// completion flag and accepts a stream that omits all extension metadata.
func (s *Snapshotter) ApplySnapshotChunk(manager *snapshots.Manager, req *abci.RequestApplySnapshotChunk) (*abci.ResponseApplySnapshotChunk, error) {
	if manager == nil || !s.restoring {
		return &abci.ResponseApplySnapshotChunk{Result: abci.ResponseApplySnapshotChunk_ABORT}, nil
	}
	complete, err := manager.RestoreChunk(req.Chunk)
	if errors.Is(err, snapshot.ErrChunkHashMismatch) {
		return &abci.ResponseApplySnapshotChunk{Result: abci.ResponseApplySnapshotChunk_RETRY, RefetchChunks: []uint32{req.Index}, RejectSenders: []string{req.Sender}}, nil
	}
	if err == nil && complete {
		commit := s.cms.LastCommitID()
		if !s.restored || uint64(commit.Version) != s.offeredHeight || len(s.trustedHash) != sha256.Size || !bytes.Equal(commit.Hash, s.trustedHash) {
			err = fmt.Errorf("incomplete snapshot extension or trusted app hash mismatch")
		} else {
			err = s.keeper.ValidateState(s.context(s.offeredHeight))
		}
	}
	if err != nil {
		return &abci.ResponseApplySnapshotChunk{Result: abci.ResponseApplySnapshotChunk_ABORT}, nil
	}
	if complete {
		if err := s.finishRestore(); err != nil {
			return &abci.ResponseApplySnapshotChunk{Result: abci.ResponseApplySnapshotChunk_ABORT}, nil
		}
		s.restoring = false
		s.restored = false
	}
	return &abci.ResponseApplySnapshotChunk{Result: abci.ResponseApplySnapshotChunk_ACCEPT}, nil
}

func (s *Snapshotter) SnapshotExtension(height uint64, write snapshot.ExtensionPayloadWriter) error {
	cms, err := s.cms.CacheMultiStoreWithVersion(int64(height))
	if err != nil {
		return err
	}
	ctx := sdk.NewContext(cms, tmproto.Header{Height: int64(height)}, false, log.NewNopLogger())
	if err := s.keeper.ValidateState(ctx); err != nil {
		return err
	}
	return s.ExtensionSnapshotter.SnapshotExtension(height, write)
}

// Recover only at the restore boundary: a corrupt/missing native artifact must
// reject a snapshot, not kill the SDK's asynchronous restoration goroutine.
// Normal execution's infrastructure-panic policy is deliberately unchanged.
func (s *Snapshotter) RestoreExtension(height uint64, format uint32, read snapshot.ExtensionPayloadReader) (err error) {
	defer func() {
		if p := recover(); p != nil {
			err = fmt.Errorf("snapshot restoration fault: %v", p)
		}
	}()
	s.restored = false
	ctx := s.context(height)
	if s.restoring {
		commit := s.cms.LastCommitID()
		if uint64(commit.Version) != s.offeredHeight || height != s.offeredHeight || len(s.trustedHash) != sha256.Size || !bytes.Equal(commit.Hash, s.trustedHash) {
			return fmt.Errorf("trusted snapshot app hash mismatch")
		}
	}
	if !bytes.Equal(s.keeper.store(ctx).Get([]byte(stateManifestKey)), s.keeper.stateManifest()) {
		return fmt.Errorf("native async schema or configuration mismatch")
	}
	required := map[[sha256.Size]byte]bool{}
	s.keeper.Wasm.IterateCodeInfos(ctx, func(_ uint64, info wasmtypes.CodeInfo) bool {
		if len(info.CodeHash) != sha256.Size {
			err = fmt.Errorf("invalid code checksum in restored state")
			return true
		}
		var hash [sha256.Size]byte
		copy(hash[:], info.CodeHash)
		required[hash] = false
		return false
	})
	if err != nil {
		return err
	}
	maxCodeSize := wasmtypes.MaxProposalWasmSize
	if wasmtypes.MaxWasmSize > maxCodeSize {
		maxCodeSize = wasmtypes.MaxWasmSize
	}
	boundedRead := func() ([]byte, error) {
		payload, e := read()
		if e == io.EOF {
			for _, seen := range required {
				if !seen {
					return nil, fmt.Errorf("snapshot missing required Wasm bytecode")
				}
			}
			return nil, e
		}
		if e != nil {
			return nil, e
		}
		if !ioutils.IsGzip(payload) {
			return nil, fmt.Errorf("snapshot bytecode is not gzip")
		}
		code, e := ioutils.Uncompress(payload, int64(maxCodeSize))
		if e != nil {
			return nil, e
		}
		hash := sha256.Sum256(code)
		seen, ok := required[hash]
		if !ok {
			return nil, fmt.Errorf("unexpected Wasm checksum in snapshot")
		}
		if seen {
			return nil, fmt.Errorf("duplicate Wasm checksum in snapshot")
		}
		required[hash] = true
		return payload, nil
	}
	if err = s.ExtensionSnapshotter.RestoreExtension(height, format, boundedRead); err != nil {
		return err
	}
	if err = s.keeper.ValidateState(ctx); err != nil {
		return err
	}
	s.restored = true
	return nil
}
