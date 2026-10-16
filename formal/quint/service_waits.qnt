// PR1 legacy service abstraction. No native scheduler, escrow, or automatic timeout.
module service_waits {
  pure val ids = Set(1, 2)
  pure val waitKeys = Set((1, 1), (1, 2), (2, 1), (2, 2))
  type Key = (int, int)
  type Workflow = { status: str, seq: int, deadline: int, resolver: int,
    order: int, reference: int }
  type Receipt = { id: int, supplied: int, current: int, sender: int,
    resolver: int, at: int, deadline: int }
  type Chain = { height: int, nextId: int, workflows: int -> Workflow,
    pending: Set[Key], seen: Set[Key], paymentSeen: int -> int,
    requestIds: Key -> int, nextRequest: int -> int,
    consumed: Set[Key], resolutions: int, receipts: Set[Receipt], external: int }
  type State = { chain: Chain, journal: str, selected: Key, ready: bool,
    command: str, running: bool, broadcasts: int, aborts: int, restarts: int }
  pure val absent: Workflow = { status: "absent", seq: 0, deadline: 0,
    resolver: 0, order: 0, reference: 0 }
  pure val initial: State = { chain: { height: 0, nextId: 1,
    workflows: ids.mapBy(_ => absent), pending: Set(), seen: Set(),
    requestIds: waitKeys.mapBy(_ => 0), nextRequest: Set(2, 3).mapBy(_ => 1),
    paymentSeen: ids.mapBy(_ => -1), consumed: Set(), resolutions: 0,
    receipts: Set(), external: 0 }, journal: "empty", selected: (0, 0),
    ready: false, command: "none", running: true, broadcasts: 0, aborts: 0, restarts: 0 }
  pure def terminal(w: Workflow): bool = Set("completed", "failed").contains(w.status)
  pure def save(s: State, id: int, w: Workflow): State =
    s.with("chain", s.chain.with("workflows", s.chain.workflows.set(id, w)))
  pure def beginRequest(s: State, key: Key, caller: int): State = {
    val service = if (key._2 == 1) 2 else 3
    val rid = s.chain.nextRequest.get(service)
    if (caller != 9 or not(waitKeys.contains(key)) or s.chain.seen.contains(key) or rid > 2) s else
      s.with("chain", { ...s.chain, pending: s.chain.pending.union(Set(key)),
        seen: s.chain.seen.union(Set(key)), requestIds: s.chain.requestIds.set(key, rid),
        nextRequest: s.chain.nextRequest.set(service, rid + 1) })
  }
  // Binding is a fixed deployment precondition: workflow 9, owner 1, payment
  // service 2, shipping service 3. Each Begin uses this immutable namespace.
  pure def startWorkflow(s: State, actor: int, order: int, funds: bool, abort: bool): State = {
    val id = s.chain.nextId
    if (actor != 1 or funds or not(ids.contains(order)) or id > 2 or s.chain.height > 4) s
    else if (abort) s.with("aborts", s.aborts + 1) else {
      val key = (id, 1)
      val w = { ...absent, status: "waiting", seq: 1, deadline: s.chain.height + 2,
        resolver: 2, order: order }
      val begun = beginRequest(s, key, 9)
      if (begun == s) s.with("aborts", s.aborts + 1)
      else save(begun.with("chain", begun.chain.with("nextId", id + 1)), id, w)
    }
  }
  // Consuming a wait is distinct from a service's dequeue. Domain failures are
  // successful transactions; malformed/oversized/trap/OOG/outbound failure abort.
  pure def resume(s: State, id: int, seq: int, sender: int, outcome: str, weak: bool): State = {
    val w = s.chain.workflows.get(id)
    if (w.status != "waiting" or (not(weak) and seq != w.seq) or sender != w.resolver or
      s.chain.height >= w.deadline) s
    else if (Set("malformed", "oversized", "outbound_abort", "gas_abort").contains(outcome))
      s.with("aborts", s.aborts + 1)
    else {
      val receipt: Receipt = { id: id, supplied: seq, current: w.seq, sender: sender,
        resolver: w.resolver, at: s.chain.height, deadline: w.deadline }
      val consumed = s.with("chain", { ...s.chain,
        consumed: s.chain.consumed.union(Set((id, w.seq))), resolutions: s.chain.resolutions + 1,
        receipts: s.chain.receipts.union(Set(receipt)) })
      if (outcome == "error") save(consumed, id, w.with("status", "failed"))
      else if (w.seq == 2) save(consumed, id, w.with("status", "completed")) else {
        val reference = if (outcome == "empty") 0 else 1
        val recorded = consumed.with("chain", consumed.chain.with("paymentSeen",
          consumed.chain.paymentSeen.set(w.order, reference)))
        if (reference == 0) save(recorded, id, w.with("status", "failed")) else {
          val nextKey = (id, 2)
          val changed = save(recorded, id, { ...w, seq: 2, resolver: 3,
            reference: reference, deadline: s.chain.height + 2 })
          val begun = beginRequest(changed, nextKey, 9)
          if (begun == changed) s.with("aborts", s.aborts + 1) else begun
        }
      }
    }
  }
  pure def deliver(s: State, key: Key, actor: int, outcome: str): State = {
    if (actor != 1 or not(s.chain.pending.contains(key))) s else {
      val result = resume(s, key._1, key._2, if (key._2 == 1) 2 else 3, outcome, false)
      // Dequeue and the continuation/next Begin commit together. Rejection or
      // abort preserves every modeled chain item, including the original row.
      if (result.chain.resolutions == s.chain.resolutions) result
      else result.with("chain", result.chain.with("pending", result.chain.pending.exclude(Set(key))))
    }
  }
  pure def advance(s: State): State = if (s.chain.height >= 6) s else
    s.with("chain", s.chain.with("height", s.chain.height + 1))
  pure def expire(s: State, id: int, seq: int): State = {
    val w = s.chain.workflows.get(id)
    if (w.status != "waiting" or w.seq != seq or s.chain.height < w.deadline) s else
      save(s.with("chain", { ...s.chain, resolutions: s.chain.resolutions + 1,
        consumed: s.chain.consumed.union(Set((id, seq))) }), id, w.with("status", "failed"))
  }
  pure def prune(s: State, key: Key): State = {
    val w = s.chain.workflows.get(key._1)
    if (s.chain.pending.contains(key) and (terminal(w) or (w.status == "waiting" and w.seq > key._2)))
      s.with("chain", s.chain.with("pending", s.chain.pending.exclude(Set(key)))) else s
  }
  // Abstract external effect, not a real payment claim or local contract write.
  pure def remoteEffect(s: State): State = if (s.chain.external >= 2) s else
    s.with("chain", s.chain.with("external", s.chain.external + 1))
  pure def prepare(s: State, service: int, scopeOk: bool): State = {
    val rows = s.chain.pending.filter(k => (if (k._2 == 1) 2 else 3) == service)
    val oldest = rows.fold((0, 0), (a, k) =>
      if (a._1 == 0) k else if (s.chain.requestIds.get(k) < s.chain.requestIds.get(a)) k else a)
    if (not(scopeOk) or not(s.running) or s.journal != "empty" or rows.size() == 0) s else
      { val w = s.chain.workflows.get(oldest._1)
        { ...s, journal: "intent", selected: oldest, ready: true,
          command: if (w.status != "waiting" or w.seq != oldest._2) "prune"
            else if (s.chain.height >= w.deadline) "expire" else "deliver" } }
  }
  // ready is process-local permission for the first attempt only. Persisted
  // Intent cannot grant another attempt after crash/transport ambiguity.
  pure def broadcast(s: State, outcome: str, scopeOk: bool): State =
    if (not(scopeOk) or not(s.running) or not(s.ready) or s.journal != "intent" or s.broadcasts >= 8) s else
      { ...s, ready: false, broadcasts: s.broadcasts + 1,
        journal: if (outcome == "hash") "hashed"
          else if (outcome == "checktx_rejected") "empty" else "intent" }
  pure def reconcile(s: State, outcome: str): State =
    if (s.journal == "hashed" and Set("confirmed_success", "confirmed_failure").contains(outcome))
      { ...s, journal: "empty", ready: false } else s
  pure def restart(s: State): State =
    { ...s, ready: false, running: true, restarts: s.restarts + 1 }
  pure def correlationOf(s: State): bool =
    s.chain.receipts.forall(r => r.supplied == r.current and r.at < r.deadline) and
    s.chain.resolutions == s.chain.consumed.size()
  pure def authorizationOf(s: State): bool =
    s.chain.receipts.forall(r => r.sender == r.resolver)
  pure def retainedStateOf(s: State): bool =
    s.chain.pending.forall(k => s.chain.seen.contains(k)) and
    s.chain.consumed.forall(k => s.chain.seen.contains(k)) and
    s.chain.seen.forall(k => s.chain.requestIds.get(k) > 0) and
    s.chain.seen.forall(a => s.chain.seen.forall(b =>
      a == b or a._2 != b._2 or s.chain.requestIds.get(a) != s.chain.requestIds.get(b))) and
    ids.forall(id => { val w = s.chain.workflows.get(id)
      Set("absent", "waiting", "completed", "failed").contains(w.status) and
      (w.status != "waiting" or (w.seq == 1 and w.resolver == 2) or (w.seq == 2 and w.resolver == 3)) }) and
    (not(s.ready) or s.journal == "intent")
  pure def safetyOf(s: State): bool = correlationOf(s) and authorizationOf(s) and retainedStateOf(s)
  var state: State
  action init = state' = initial
  action step = {
    nondet id = ids.oneOf()
    nondet seq = Set(1, 2).oneOf()
    nondet outcome = Set("success", "empty", "error", "malformed", "oversized", "outbound_abort", "gas_abort").oneOf()
    any {
      state' = startWorkflow(state, 1, id, false, false),
      state' = deliver(state, (id, seq), 1, outcome),
      state' = resume(state, id, seq, 4, outcome, false),
      state' = advance(state), state' = expire(state, id, seq),
      state' = prune(state, (id, seq)), state' = remoteEffect(state),
      state' = prepare(state, if (seq == 1) 2 else 3, true),
      state' = broadcast(state, "hash", true), state' = broadcast(state, "ambiguous", true),
      state' = broadcast(state, "checktx_rejected", true),
      state' = reconcile(state, "unknown"), state' = reconcile(state, "confirmed_failure"),
      state' = restart(state),
    }
  }
  action weakSequenceInit = state' = startWorkflow(initial, 1, 1, false, false)
  action weakSequenceStep = state' = resume(state, 1, 2, 2, "success", true)
  val sawAccepted = state.chain.receipts.size() > 0
  val sawRollback = state.aborts > 0
  val sawRestart = state.restarts > 0
  val sawAmbiguous = state.journal == "intent" and not(state.ready) and state.broadcasts > 0
  val correlation = correlationOf(state)
  val authorization = authorizationOf(state)
  val retainedState = retainedStateOf(state)
  val safety = correlation and authorization and retainedState
}
