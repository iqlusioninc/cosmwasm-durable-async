package async

import (
	"encoding/json"
	"fmt"
	"testing"

	wasmkeeper "github.com/CosmWasm/wasmd/x/wasm/keeper"
	vmtypes "github.com/CosmWasm/wasmvm/v2/types"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/stretchr/testify/require"
)

func TestPublicTaskQueryRealTerminalOutboundCallback(t *testing.T) {
	for _, mode := range []string{"outbound", "fail_outbound"} {
		for _, rollback := range []bool{false, true} {
			t.Run(fmt.Sprint(mode, "/rollback=", rollback), func(t *testing.T) {
				h := newFaultHarness(t)
				h.start(t, 80_000_000, 100)
				h.block(11)
				h.execute(t, map[string]any{"deliver": map[string]any{"task_id": 1, "wait_sequence": 1}}, 0)
				h.execute(t, map[string]any{"mode": map[string]any{"value": mode}}, 0)
				queried := false
				h.k.messenger = wasmkeeper.MessageHandlerFunc(func(c sdk.Context, a sdk.AccAddress, _ string, _ vmtypes.CosmosMsg) ([]sdk.Event, [][]byte, [][]*codectypes.Any, error) {
					raw, err := h.k.Query(c, marshal(map[string]any{"task": map[string]any{"contract": a.String(), "task_id": 1}}))
					require.NoError(t, err)
					var response TaskResponse
					require.NoError(t, json.Unmarshal(raw, &response))
					require.Equal(t, "runnable", response.Status.Kind)
					require.Nil(t, response.State)
					if mode == "outbound" {
						require.NotNil(t, response.Output)
						require.Nil(t, response.Error)
					} else {
						require.Nil(t, response.Output)
						require.NotNil(t, response.Error)
					}
					raw, err = h.k.Query(c, marshal(map[string]any{"funding": map[string]any{"task_funding": map[string]any{"contract": a.String(), "task_id": 1}}}))
					require.NoError(t, err)
					var funding struct {
						Status   TaskStatus `json:"status"`
						Reserved string     `json:"reserved"`
					}
					require.NoError(t, json.Unmarshal(raw, &funding))
					require.Equal(t, "runnable", funding.Status.Kind)
					require.NotEqual(t, "0", funding.Reserved, "callback observes reserved escrow before settlement")
					queried = true
					if rollback {
						return nil, nil, nil, fmt.Errorf("rollback staged outcome after query")
					}
					return nil, nil, nil, nil
				})
				h.block(12)
				require.True(t, queried)
				task := h.task(t, 1)
				if rollback {
					require.Equal(t, "paused", task.Status)
					require.Equal(t, "message_failure", task.Pause)
					require.Nil(t, publicTask(task).Output)
					require.Nil(t, publicTask(task).Error)
				} else {
					if mode == "outbound" {
						require.Equal(t, "completed", task.Status)
					} else {
						require.Equal(t, "failed", task.Status)
					}
				}
			})
		}
	}
}

func TestPublicTaskQueryDoesNotExposeStorageOrFunding(t *testing.T) {
	ctx, k, _ := setup(t)
	task := Task{ID: 7, Owner: "owner", Register: Register{Program: Program{Kind: "test", Version: 1}, ExpiresAt: 100, AllocatedBytes: 4096}, Status: "runnable"}
	k.save(ctx, task)
	raw, err := k.Query(ctx, json.RawMessage(`{"task":{"contract":"owner","task_id":7}}`))
	require.NoError(t, err)
	require.JSONEq(t, `{"contract":"owner","task_id":7,"program":{"kind":"test","version":1,"input":""},"code_checksum":"","status":"runnable","wait_sequence":0,"state":null,"wait":null,"inbox":null,"output":null,"error":null,"last_attempt":null,"expires_at":100,"retain_until":100,"allocated_bytes":4096}`, string(raw))
}

func TestPublicTaskQueryTerminalAndPauseProjection(t *testing.T) {
	ctx, k, _ := setup(t)
	for _, tc := range []struct {
		status, pause                    string
		started                          bool
		state                            []byte
		height                           int64
		wantStatus                       string
		wantState, wantOutput, wantError string
	}{
		{"completed", "", true, []byte("done"), 10, `"completed"`, `null`, `"ZG9uZQ=="`, `null`},
		{"completed", "", true, nil, 10, `"completed"`, `null`, `""`, `null`},
		{"failed", "", true, []byte("program failed"), 10, `"failed"`, `null`, `null`, `"program failed"`},
		{"cancelled", "", true, nil, 10, `"cancelled"`, `null`, `null`, `null`},
		{"expired", "", true, nil, 100, `"expired"`, `null`, `null`, `null`},
		{"paused", "out_of_gas", false, nil, 10, `{"paused":{"reason":"out_of_gas","resume":"runnable"}}`, `null`, `null`, `null`},
		{"waiting", "", true, nil, 100, `"expired"`, `""`, `null`, `null`},
	} {
		t.Run(tc.status+tc.wantOutput, func(t *testing.T) {
			task := Task{ID: 7, Owner: "owner", Register: Register{Program: Program{Kind: "test", Version: 1}, ExpiresAt: 100}, Status: tc.status, Pause: tc.pause, Started: tc.started, State: tc.state, LastPoll: 9}
			k.save(ctx, task)
			raw, err := k.Query(ctx.WithBlockHeight(tc.height), json.RawMessage(`{"task":{"contract":"owner","task_id":7}}`))
			require.NoError(t, err)
			var response map[string]json.RawMessage
			require.NoError(t, json.Unmarshal(raw, &response))
			require.JSONEq(t, tc.wantStatus, string(response["status"]))
			require.JSONEq(t, tc.wantState, string(response["state"]))
			require.JSONEq(t, tc.wantOutput, string(response["output"]))
			require.JSONEq(t, tc.wantError, string(response["error"]))
			require.JSONEq(t, `9`, string(response["last_attempt"]))
		})
	}
}

// stage persists the outcome before outbound dispatch. A callback querying that
// same child cache must see a decodable public status before final settlement.
func TestPublicTaskQueryDuringStagedTerminalCallback(t *testing.T) {
	for _, outcome := range []string{`{"complete":{"output":"ZG9uZQ=="}}`, `{"fail":{"error":"program failed"}}`} {
		t.Run(outcome, func(t *testing.T) {
			ctx, k, _ := setup(t)
			child, _ := ctx.CacheContext()
			task := Task{ID: 7, Owner: "owner", Register: Register{Program: Program{Kind: "test", Version: 1}, ExpiresAt: 100, AllocatedBytes: 4096}, Status: "runnable", LastPoll: 10}
			var poll PollResponse
			require.NoError(t, json.Unmarshal([]byte(`{"outcome":`+outcome+`,"messages":[]}`), &poll))
			require.NoError(t, k.stage(child, &task, poll))
			raw, err := k.Query(child, json.RawMessage(`{"task":{"contract":"owner","task_id":7}}`))
			require.NoError(t, err)
			var response TaskResponse
			require.NoError(t, json.Unmarshal(raw, &response), "Rust-compatible typed status must decode during callbacks")
			require.Equal(t, "runnable", response.Status.Kind, "outbound dispatch has not settled the task")
			require.Nil(t, response.State, "terminal payload is not a continuation")
			if poll.Outcome.Complete != nil {
				require.Equal(t, []byte("done"), response.Output)
				require.Nil(t, response.Error)
			} else {
				require.Nil(t, response.Output)
				require.Equal(t, "program failed", *response.Error)
			}
			raw, err = k.Query(child, json.RawMessage(`{"funding":{"task_funding":{"contract":"owner","task_id":7}}}`))
			require.NoError(t, err)
			var funding struct {
				Status TaskStatus `json:"status"`
			}
			require.NoError(t, json.Unmarshal(raw, &funding))
			require.Equal(t, "runnable", funding.Status.Kind)
		})
	}
}
