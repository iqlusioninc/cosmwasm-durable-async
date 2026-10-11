package async

import (
	"encoding/json"
	"testing"

	"github.com/stretchr/testify/require"
)

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
