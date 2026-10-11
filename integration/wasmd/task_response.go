package async

import (
	"encoding/json"
	"fmt"
)

// TaskStatus is the public Rust-compatible status enum.
type TaskStatus struct {
	Kind   string
	Reason string
	Resume string
}

func (s TaskStatus) MarshalJSON() ([]byte, error) {
	if s.Kind == "paused" {
		return json.Marshal(map[string]any{"paused": map[string]string{"reason": s.Reason, "resume": s.Resume}})
	}
	return json.Marshal(s.Kind)
}

func (s *TaskStatus) UnmarshalJSON(b []byte) error {
	var kind string
	if json.Unmarshal(b, &kind) == nil {
		switch kind {
		case "runnable", "waiting", "cancelled", "expired", "completed", "failed":
			*s = TaskStatus{Kind: kind}
			return nil
		}
		return fmt.Errorf("unknown task status %q", kind)
	}
	var paused struct {
		Paused *struct {
			Reason string `json:"reason"`
			Resume string `json:"resume"`
		} `json:"paused"`
	}
	if err := json.Unmarshal(b, &paused); err != nil {
		return err
	}
	if paused.Paused == nil {
		return fmt.Errorf("invalid task status")
	}
	switch paused.Paused.Reason {
	case "insufficient_funds", "price_limit", "out_of_gas", "trap", "message_failure", "invalid_response":
	default:
		return fmt.Errorf("invalid pause reason")
	}
	if paused.Paused.Resume != "runnable" && paused.Paused.Resume != "waiting" {
		return fmt.Errorf("invalid resume state")
	}
	*s = TaskStatus{Kind: "paused", Reason: paused.Paused.Reason, Resume: paused.Paused.Resume}
	return nil
}

// TaskResponse excludes internal indexes and financial bookkeeping. Optional
// execution fields always serialize, with null distinguishing absent Binary.
// Reentrant queries during outbound dispatch expose a candidate in-flight view:
// output/error may be staged while status remains nonterminal. Settlement and
// refunds are final only when the poll commits; dispatch failure rolls back the
// staged payload. Funding queries retain the active escrow reservation meanwhile.
type TaskResponse struct {
	Contract       string      `json:"contract"`
	TaskID         uint64      `json:"task_id"`
	Program        Program     `json:"program"`
	CodeChecksum   []byte      `json:"code_checksum"`
	Status         TaskStatus  `json:"status"`
	WaitSequence   uint64      `json:"wait_sequence"`
	State          []byte      `json:"state"`
	Wait           *Wait       `json:"wait"`
	Inbox          *Resolution `json:"inbox"`
	Output         []byte      `json:"output"`
	Error          *string     `json:"error"`
	LastAttempt    *uint64     `json:"last_attempt"`
	ExpiresAt      uint64      `json:"expires_at"`
	RetainUntil    uint64      `json:"retain_until"`
	AllocatedBytes uint64      `json:"allocated_bytes"`
}

func presentBinary(b []byte) []byte {
	if b == nil {
		return []byte{}
	}
	return b
}

func publicTask(t Task) TaskResponse {
	program := t.Program
	program.Input = presentBinary(program.Input)
	r := TaskResponse{Contract: t.Owner, TaskID: t.ID, Program: program,
		CodeChecksum: presentBinary(t.Checksum), Status: TaskStatus{Kind: t.Status},
		WaitSequence: t.Sequence, Wait: t.Wait, Inbox: t.Inbox,
		ExpiresAt: t.ExpiresAt, RetainUntil: t.ExpiresAt, AllocatedBytes: t.AllocatedBytes}
	if t.LastPoll != 0 {
		h := t.LastPoll
		r.LastAttempt = &h
	}
	switch t.Status {
	case "completed", "completing":
		r.Output = presentBinary(t.State)
	case "failed", "failing":
		e := string(t.State)
		r.Error = &e
	case "cancelled", "expired":
	default:
		if t.Started {
			r.State = presentBinary(t.State)
		}
	}
	// Staged terminal outcomes remain runnable until outbound dispatch and fee
	// settlement succeed. Candidate payloads never masquerade as continuations.
	if t.Status == "completing" || t.Status == "failing" {
		r.Status = TaskStatus{Kind: "runnable"}
	}
	if t.Status == "paused" {
		r.Status.Reason = t.Pause
		r.Status.Resume = "runnable"
		if t.Wait != nil && t.Inbox == nil {
			r.Status.Resume = "waiting"
		}
	}
	return r
}
