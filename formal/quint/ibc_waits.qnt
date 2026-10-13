// PR3 classic-IBC pure-query adapter; separate from all native scheduling models.
module ibc_waits {
  pure val ids = Set(1, 2)
  pure val waitKeys = Set((1, 1), (1, 2), (2, 1), (2, 2))
  type Key = (int, int)
  type Workflow = { status: str, seq: int, deadline: int, request: int, output: int }
  type Packet = { id: int, seq: int, data: int, src: int, dest: int,
    timeout: int, packetSequence: int }
  type Receipt = { id: int, seq: int, current: int, matched: bool,
    authenticated: bool, sender: int, at: int, deadline: int }
  type Chain = { time: int, remoteTime: int, bound: bool, closed: bool,
    nextId: int, nextPacket: int, workflows: int -> Workflow,
    pending: Set[Key], intents: Key -> Packet, commitments: Set[Packet],
    consumed: Set[Key], resolutions: int, receipts: Set[Receipt], sends: int, sendsAtClose: int }
  type State = { chain: Chain, aborts: int, restarts: int }
  pure val absent: Workflow = { status: "absent", seq: 0, deadline: 0, request: 0, output: 0 }
  pure val emptyPacket: Packet = { id: 0, seq: 0, data: 0, src: 0, dest: 0,
    timeout: 0, packetSequence: 0 }
  pure val initial: State = { chain: { time: 0, remoteTime: 0, bound: false,
    closed: false, nextId: 1, nextPacket: 1, workflows: ids.mapBy(_ => absent),
    pending: Set(), intents: waitKeys.mapBy(_ => emptyPacket), commitments: Set(),
    consumed: Set(), resolutions: 0, receipts: Set(), sends: 0, sendsAtClose: 0 }, aborts: 0, restarts: 0 }
  // valid abstracts the fixed connection, peer/local ports, unordered ordering,
  // and both protocol versions. Channel binding never resets, including close.
  pure def connect(s: State, valid: bool): State =
    if (valid and not(s.chain.bound)) s.with("chain", s.chain.with("bound", true)) else s
  pure def save(s: State, id: int, w: Workflow): State =
    s.with("chain", s.chain.with("workflows", s.chain.workflows.set(id, w)))
  pure def send(s: State, id: int, seq: int, value: int): State = {
    val p: Packet = { id: id, seq: seq, data: value, src: 1, dest: 2,
      timeout: s.chain.time + 2, packetSequence: s.chain.nextPacket }
    val changed = save(s, id, { status: "waiting", seq: seq,
      deadline: p.timeout, request: value, output: 0 })
    changed.with("chain", { ...changed.chain, pending: changed.chain.pending.union(Set((id, seq))),
      intents: changed.chain.intents.set((id, seq), p), commitments: changed.chain.commitments.union(Set(p)),
      nextPacket: changed.chain.nextPacket + 1, sends: changed.chain.sends + 1 })
  }
  pure def startWorkflow(s: State, actor: int, value: int, funds: bool, abort: bool): State =
    if (actor != 1 or funds or not(s.chain.bound) or s.chain.closed or
      s.chain.nextId > 2 or s.chain.time > 4 or not(Set(0, 5).contains(value))) s
    else if (abort) s.with("aborts", s.aborts + 1)
    else send(s.with("chain", s.chain.with("nextId", s.chain.nextId + 1)), s.chain.nextId, 1, value)
  pure def packet(s: State, id: int, seq: int): Packet = s.chain.intents.get((id, seq))
  pure def advance(s: State, remote: bool): State = {
    if (remote) {
      if (s.chain.remoteTime >= 6) s else s.with("chain", s.chain.with("remoteTime", s.chain.remoteTime + 1))
    } else if (s.chain.time >= 6) s else s.with("chain", s.chain.with("time", s.chain.time + 1))
  }
  pure def close(s: State, channel: int, authenticated: bool): State =
    if (channel != 1 or not(authenticated) or not(s.chain.bound) or s.chain.closed) s else
      s.with("chain", { ...s.chain, closed: true, sendsAtClose: s.chain.sends })
  // Packet data is a small representative for saved application bytes. Equality
  // abstracts bytewise matching; alternative encodings are not enumerated.
  // id/seq are their decoded correlation. packetSequence is intentionally absent
  // from this comparison: only IBC core authenticates that host-owned identity.
  pure def matches(s: State, p: Packet): bool = {
    val expected = packet(s, p.id, p.seq)
    p.data == expected.data and p.src == expected.src and p.dest == expected.dest and p.timeout == expected.timeout
  }
  pure def consume(s: State, id: int, seq: int): State =
    s.with("chain", { ...s.chain, consumed: s.chain.consumed.union(Set((id, seq))),
      resolutions: s.chain.resolutions + 1 })
  pure def continuation(s: State, id: int, seq: int, outcome: str, authenticated: bool, matched: bool, sender: int): State = {
    val w = s.chain.workflows.get(id)
    val receipt: Receipt = { id: id, seq: seq, current: w.seq, matched: matched,
      authenticated: authenticated, sender: sender, at: s.chain.time, deadline: w.deadline }
    val consumed = consume(s, id, seq)
    val recorded = consumed.with("chain", consumed.chain.with("receipts", consumed.chain.receipts.union(Set(receipt))))
    if (s.chain.closed or Set("error", "invalid").contains(outcome))
      save(recorded, id, w.with("status", "failed")) else {
      // Trusted peers can lie. Honest doubling is a scenario, not safety.
      val value = if (outcome == "peer_lie") 7 else w.request * 2
      if (seq == 1) send(recorded, id, 2, value)
      else save(recorded, id, { ...w, status: "completed", output: value })
    }
  }
  // An atomic transaction spans IBC-core consumption, pending deletion, self
  // Deliver/Expire and the next SendPacket. Abort restores all chain/host items.
  pure def settle(s: State, p: Packet, outcome: str, timeout: bool, authenticated: bool, weakData: bool): State = {
    val key = (p.id, p.seq)
    if (not(s.chain.bound) or p.src != 1 or p.dest != 2 or not(s.chain.pending.contains(key))) s
    else if (not(weakData) and not(matches(s, p))) s else {
      val w = s.chain.workflows.get(p.id)
      if (Set("gas_abort", "outbound_abort").contains(outcome)) s.with("aborts", s.aborts + 1) else {
        val deleted = s.with("chain", { ...s.chain, pending: s.chain.pending.exclude(Set(key)),
          commitments: s.chain.commitments.filter(old => old.id != p.id or old.seq != p.seq) })
        if (w.status != "waiting" or w.seq != p.seq) deleted
        else if (s.chain.time >= w.deadline)
          save(consume(deleted, p.id, p.seq), p.id, w.with("status", "failed"))
        else if (timeout) deleted
        else continuation(deleted, p.id, p.seq, outcome, authenticated, matches(s, p), 9)
      }
    }
  }
  // Abstract IBC-core proof/replay gate, not contract code. Full packet equality
  // binds packetSequence AND bytes/endpoints/timeout to the emitted commitment.
  // The authenticated bool stands for a valid ack/timeout proof; Quint cannot
  // establish the proof, channel trust, or mutual exclusivity on a real chain.
  pure def callback(s: State, p: Packet, outcome: str, timeout: bool, authenticated: bool): State =
    if (not(authenticated) or not(s.chain.commitments.contains(p)) or
      (timeout and s.chain.remoteTime < p.timeout)) s
    else if (s.chain.pending.contains((p.id, p.seq)) and not(matches(s, p))) s else {
      val result = settle(s, p, outcome, timeout, authenticated, false)
      // An authenticated late packet still settles at IBC core when the adapter
      // returns already_settled. A local public expiry did not cancel it earlier.
      if (result.aborts != s.aborts) result else
        result.with("chain", result.chain.with("commitments", result.chain.commitments.exclude(Set(p))))
    }
  // Direct-entrypoint fixtures intentionally omit IBC admission, matching Rust
  // mocks. They demonstrate adapter guards, not valid production proof events.
  pure def adapterFixture(s: State, p: Packet, outcome: str, timeout: bool): State =
    settle(s, p, outcome, timeout, true, false)
  // 9 denotes the actual host-authenticated self sender, not a relayer field.
  pure def publicDeliver(s: State, id: int, seq: int, sender: int): State = {
    val w = s.chain.workflows.get(id)
    if (sender != 9 or w.status != "waiting" or w.seq != seq or s.chain.time >= w.deadline) s
    else continuation(s, id, seq, "success", true, true, sender)
  }
  pure def expire(s: State, id: int, seq: int): State = {
    val w = s.chain.workflows.get(id)
    if (w.status != "waiting" or w.seq != seq or s.chain.time < w.deadline) s else {
      val failed = save(consume(s, id, seq), id, w.with("status", "failed"))
      failed.with("chain", failed.chain.with("pending", failed.chain.pending.exclude(Set((id, seq)))))
    }
  }
  pure def restart(s: State): State = s.with("restarts", s.restarts + 1)
  pure def correlationOf(s: State): bool =
    s.chain.receipts.forall(r => r.seq == r.current and r.matched and r.at < r.deadline) and
    s.chain.resolutions == s.chain.consumed.size()
  pure def authorizationOf(s: State): bool = s.chain.receipts.forall(r => r.authenticated and r.sender == 9)
  pure def retainedStateOf(s: State): bool =
    s.chain.pending.forall(k => { val p = packet(s, k._1, k._2)
      val w = s.chain.workflows.get(k._1)
      p.id == k._1 and p.seq == k._2 and p.src == 1 and p.dest == 2 and
      w.status == "waiting" and w.seq == k._2 and p.data == w.request and p.timeout == w.deadline }) and
    (not(s.chain.closed) or (s.chain.bound and s.chain.sends == s.chain.sendsAtClose)) and
    ids.forall(id => Set("absent", "waiting", "completed", "failed").contains(s.chain.workflows.get(id).status))
  pure def safetyOf(s: State): bool = correlationOf(s) and authorizationOf(s) and retainedStateOf(s)
  var state: State
  action init = state' = initial
  action step = {
    nondet id = ids.oneOf()
    nondet seq = Set(1, 2).oneOf()
    nondet value = Set(0, 5).oneOf()
    nondet outcome = Set("success", "error", "invalid", "peer_lie", "gas_abort", "outbound_abort").oneOf()
    nondet proof = Set(true, false).oneOf()
    any {
      state' = connect(state, proof), state' = startWorkflow(state, 1, value, false, false),
      state' = callback(state, packet(state, id, seq), outcome, false, proof),
      state' = callback(state, packet(state, id, seq), outcome, true, proof),
      state' = publicDeliver(state, id, seq, 4), state' = expire(state, id, seq),
      state' = advance(state, false), state' = advance(state, true),
      state' = close(state, 1, proof), state' = restart(state),
    }
  }
  action weakHostInit = state' = startWorkflow(connect(initial, true), 1, 5, false, false)
  action weakHostStep = state' = settle(state, packet(state, 1, 1), "success", false, false, false)
  action weakDataInit = state' = startWorkflow(connect(initial, true), 1, 5, false, false)
  action weakDataStep = state' = settle(state, packet(state, 1, 1).with("data", 6), "success", false, true, true)
  val sawAccepted = state.chain.receipts.size() > 0
  val sawRollback = state.aborts > 0
  val sawRestart = state.restarts > 0
  val sawEarlyTimeout = ids.exists(id => { val w = state.chain.workflows.get(id)
    w.status == "waiting" and state.chain.time < w.deadline and not(state.chain.pending.contains((id, w.seq))) })
  val correlation = correlationOf(state)
  val authorization = authorizationOf(state)
  val retainedState = retainedStateOf(state)
  val safety = correlation and authorization and retainedState
}
