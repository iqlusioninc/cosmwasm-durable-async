package async

import (
	"encoding/json"
	"fmt"
	wasmvmtypes "github.com/CosmWasm/wasmvm/v2/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
)

type Program struct {
	Kind    string `json:"kind"`
	Version uint32 `json:"version"`
	Input   []byte `json:"input"`
}
type Register struct {
	Control        string   `json:"control"`
	Refund         string   `json:"refund"`
	GasPerPoll     uint64   `json:"gas_per_poll"`
	MaxGasPrice    string   `json:"max_gas_price"`
	ExpiresAt      uint64   `json:"expires_at"`
	Funds          sdk.Coin `json:"funds"`
	Program        Program  `json:"program"`
	AllocatedBytes uint64   `json:"allocated_bytes"`
}
type Resolution struct {
	Success *struct {
		Data []byte `json:"data"`
	} `json:"success,omitempty"`
	Error *struct {
		Code    string `json:"code"`
		Message string `json:"message"`
	} `json:"error,omitempty"`
	Timeout *struct{} `json:"timeout,omitempty"`
	Timer   *struct{} `json:"timer,omitempty"`
}
type Wait struct {
	Height *struct {
		Height uint64 `json:"height"`
	} `json:"height,omitempty"`
	Operation *struct {
		Resolver       string `json:"resolver"`
		Deadline       uint64 `json:"deadline"`
		MaxResultBytes uint32 `json:"max_result_bytes"`
	} `json:"operation,omitempty"`
}
type PollResponse struct {
	Outcome struct {
		Pending *struct {
			State []byte `json:"state"`
			Wait  Wait   `json:"wait"`
		} `json:"pending,omitempty"`
		Yield *struct {
			State []byte `json:"state"`
		} `json:"yield,omitempty"`
		Complete *struct {
			Output []byte `json:"output"`
		} `json:"complete,omitempty"`
		Fail *struct {
			Error string `json:"error"`
		} `json:"fail,omitempty"`
	} `json:"outcome"`
	Messages []wasmvmtypes.CosmosMsg `json:"messages"`
}
type Task struct {
	ID    uint64 `json:"task_id"`
	Owner string `json:"contract"`
	Register
	Checksum      []byte      `json:"checksum"`
	State         []byte      `json:"state"`
	Started       bool        `json:"started"`
	Wait          *Wait       `json:"wait,omitempty"`
	Inbox         *Resolution `json:"inbox,omitempty"`
	Sequence      uint64      `json:"wait_sequence"`
	Status        string      `json:"status"`
	Pause         string      `json:"pause_reason,omitempty"`
	Diagnostic    string      `json:"diagnostic,omitempty"`
	Escrow        string      `json:"available"`
	Fees          string      `json:"cumulative_execution_fees"`
	Reserved      string      `json:"reserved"`
	LeasePaid     string      `json:"storage_lease_paid"`
	LastPoll      uint64      `json:"last_poll"`
	ReadyAt       uint64      `json:"ready_at"`
	ReadySequence uint64      `json:"ready_sequence"`
	QueueKey      []byte      `json:"queue_key,omitempty"`
	TimerKey      []byte      `json:"timer_key,omitempty"`
	ExpiryKey     []byte      `json:"expiry_key"`
}

type StartStep struct {
	Input []byte `json:"input"`
}
type ResumeStep struct {
	State      []byte      `json:"state"`
	Resolution *Resolution `json:"resolution"`
}
type PollStep struct {
	Start  *StartStep  `json:"start,omitempty"`
	Resume *ResumeStep `json:"resume,omitempty"`
}
type PollRequest struct {
	TaskID       uint64   `json:"task_id"`
	WaitSequence uint64   `json:"wait_sequence"`
	Kind         string   `json:"kind"`
	Version      uint32   `json:"version"`
	Step         PollStep `json:"step"`
}
type PollSudo struct {
	PollTask PollRequest `json:"poll_task"`
}
type DeliverMsg struct {
	TaskID       uint64     `json:"task_id"`
	WaitSequence uint64     `json:"wait_sequence"`
	Resolver     string     `json:"resolver"`
	Resolution   Resolution `json:"resolution"`
}
type FundTaskMsg struct {
	TaskID      uint64   `json:"task_id"`
	Contributor string   `json:"contributor"`
	Funds       sdk.Coin `json:"funds"`
}
type ExtendLeaseMsg struct {
	TaskID         uint64 `json:"task_id"`
	Actor          string `json:"actor"`
	AllocatedBytes uint64 `json:"allocated_bytes"`
	ExpiresAt      uint64 `json:"expires_at"`
}
type UpdateBudgetMsg struct {
	TaskID      uint64 `json:"task_id"`
	Actor       string `json:"actor"`
	GasPerPoll  uint64 `json:"gas_per_poll"`
	MaxGasPrice string `json:"max_gas_price"`
	Refund      string `json:"refund"`
}

func (t Task) Request() []byte {
	step := PollStep{Start: &StartStep{Input: t.Program.Input}}
	if t.Started {
		step = PollStep{Resume: &ResumeStep{State: t.State, Resolution: t.Inbox}}
	}
	return marshal(PollSudo{PollTask: PollRequest{TaskID: t.ID, WaitSequence: t.Sequence, Kind: t.Program.Kind, Version: t.Program.Version, Step: step}})
}
func marshal(v any) []byte {
	b, e := json.Marshal(v)
	if e != nil {
		panic(e)
	}
	return b
}
func validResolution(r Resolution) error {
	n := 0
	if r.Success != nil {
		n++
	}
	if r.Error != nil {
		n++
	}
	if r.Timeout != nil {
		n++
	}
	if r.Timer != nil {
		n++
	}
	if n != 1 {
		return fmt.Errorf("resolution must contain exactly one variant")
	}
	return nil
}
