module native_async {
  import pricing.* from "./pricing"

  // Finite exploration economics: integer whole-price units; charge and price
  // changes still delegate to the exact atomics helpers with guarded inputs.
  pure val ids = Set(1, 2, 3)
  pure val actors = Set(1, 2)
  pure val maxHeight = 6
  pure val blockGas = 3
  pure val maxVisits = 2
  pure val maxMaintenance = 6
  pure val maxDeposits = 24
  type Task = { owner: int, controller: int, refund: int, identity: int,
    status: str, reason: str, bid: int, cap: int, expiry: int,
    balance: int, reserve: int, lease: int, continuation: int,
    generation: int, wait: str, due: int, resolver: int, inbox: int,
    accepted: int, consumed: int, ready: int, order: int, last: int }
  type State = { tasks: int -> Task, height: int, phase: str, auction: bool,
    base: int, price: int, sequence: int, queue: Set[int], ring: List[int],
    readyIndex: Set[(int, int, int)], dueIndex: Set[(int, int, int)], scanned: Set[int],
    flight: int, staged: bool, before: Task, visits: int, maintenance: int, gas: int,
    deposits: int, fees: int, refunds: int, effects: int,
    history: Set[(int, int)], polls: int, authorityOk: bool,
    rollbackOk: bool, priceOk: bool, restoreOk: bool, guest: Guest }
  // Full transaction cache snapshot: nested task/accounting/index writes roll
  // back together. Execution meters/history and consumed poll fees live outside.
  type Guest = { tasks: int -> Task, sequence: int, queue: Set[int], ring: List[int],
    readyIndex: Set[(int, int, int)], dueIndex: Set[(int, int, int)],
    deposits: int, fees: int, refunds: int, effects: int }
  pure val emptyGuest: Guest = { tasks: ids.mapBy(_ => emptyTask), sequence: 0,
    queue: Set(), ring: List(), readyIndex: Set(), dueIndex: Set(),
    deposits: 0, fees: 0, refunds: 0, effects: 0 }
  pure def guestSnapshot(s: State): Guest = { tasks: s.tasks, sequence: s.sequence,
    queue: s.queue, ring: s.ring, readyIndex: s.readyIndex, dueIndex: s.dueIndex,
    deposits: s.deposits, fees: s.fees, refunds: s.refunds, effects: s.effects }
  pure def abortGuest(s: State): State = { ...s, tasks: s.guest.tasks,
    sequence: s.guest.sequence, queue: s.guest.queue, ring: s.guest.ring,
    readyIndex: s.guest.readyIndex, dueIndex: s.guest.dueIndex,
    deposits: s.guest.deposits, fees: s.guest.fees, refunds: s.guest.refunds,
    effects: s.guest.effects }
  // Durable boundary representation: no execution phase, reservation context,
  // block meters/cursor, cached price, or materialized live queue/index fields.
  // History/polls and accounting totals include verification ghost history;
  // they are carried across restart for checking, not a disk-schema claim.
  type Checkpoint = { tasks: int -> Task, height: int, auction: bool, base: int,
    sequence: int, ring: List[int], readyRecords: Set[(int, int, int)],
    dueRecords: Set[(int, int, int)], atBoundary: bool,
    deposits: int, fees: int, refunds: int, effects: int,
    history: Set[(int, int)], polls: int }
  pure val emptyTask: Task = { owner: 0, controller: 0, refund: 0, identity: 0,
    status: "absent", reason: "", bid: 0, cap: 1, expiry: 0,
    balance: 0, reserve: 0, lease: 0, continuation: 0,
    generation: 0, wait: "none", due: 0, resolver: 0, inbox: 0,
    accepted: 0, consumed: 0, ready: 0, order: 0, last: 0 }
  pure def initial(auction: bool, price: int): State = {
    tasks: ids.mapBy(_ => emptyTask), height: 0, phase: "idle", auction: auction,
    base: price, price: price, sequence: 0, queue: Set(), ring: List(),
    readyIndex: Set(), dueIndex: Set(), scanned: Set(),
    flight: 0, staged: false, before: emptyTask, visits: 0, maintenance: 0, gas: 0,
    deposits: 0, fees: 0, refunds: 0, effects: 0, history: Set(), polls: 0,
    authorityOk: true, rollbackOk: true, priceOk: true, restoreOk: true, guest: emptyGuest }

  pure def terminal(t: Task): bool =
    Set("completed", "failed", "cancelled", "expired", "pruned").contains(t.status)
  pure def active(t: Task): bool = t.status != "absent" and not(terminal(t))
  pure def feeValid(s: State, gas: int): bool =
    attemptChargePrecondition(gas, s.price * atomicsScale)
  pure def fee(s: State, gas: int): int =
    attemptCharge(gas, s.price * atomicsScale)
  pure def canonicalQueue(s: State): Set[int] =
    ids.filter(id => s.tasks.get(id).status == "ready")
  pure def queueOwners(s: State): Set[int] =
    s.queue.map(id => s.tasks.get(id).owner)
  pure def canonicalReadyIndex(s: State): Set[(int, int, int)] =
    canonicalQueue(s).map(id => (id, s.tasks.get(id).generation, s.tasks.get(id).order))
  pure def canonicalDueIndex(s: State): Set[(int, int, int)] =
    ids.filter(id => Set("waiting", "due").contains(s.tasks.get(id).status))
      .map(id => { val t = s.tasks.get(id)
        (id, t.generation, if (t.status == "waiting") t.due else t.ready) })
  pure def held(s: State): int = ids.fold(0, (sum, id) => {
    val t = s.tasks.get(id)
    sum + t.balance + t.reserve + t.lease
  })
  // Maintain redundant queue/ring metadata. Preserve existing contract order;
  // append newly ready owners. PR4 rotates its ring after an inspected task.
  pure def saveTask(s: State, id: int, t: Task): State = {
    val changed = s.with("tasks", s.tasks.set(id, t))
    val indexed = { ...changed, queue: canonicalQueue(changed),
      readyIndex: canonicalReadyIndex(changed), dueIndex: canonicalDueIndex(changed) }
    val owners = queueOwners(indexed)
    val ring = s.ring.foldl(List(), (acc, owner) =>
      if (owners.contains(owner)) acc.append(owner) else acc)
    indexed.with("ring", actors.fold(ring, (acc, owner) =>
      if (owners.contains(owner) and not(acc.indices().exists(i => acc.nth(i) == owner))) acc.append(owner)
      else acc))
  }
  pure def register(s: State, id: int, actor: int, bid: int, cap: int): State = {
    if (s.phase != "idle" or s.tasks.get(id).status != "absent" or
      s.deposits + 8 > maxDeposits or s.height >= 3) s else {
      val t = { ...emptyTask, owner: actor, controller: actor, refund: actor,
        identity: id, status: "ready", bid: bid, cap: cap, expiry: 4,
        balance: 6, lease: 1, ready: s.height + 1, order: s.sequence + 1 }
      saveTask({ ...s, deposits: s.deposits + 8, fees: s.fees + 1,
        sequence: s.sequence + 1 }, id, t)
    }
  }
  // Positive third-party funding does not change control, status, or refund.
  pure def fund(s: State, id: int, actor: int): State = {
    val t = s.tasks.get(id)
    if (not(active(t)) or s.deposits >= maxDeposits or not(actors.contains(actor)) or
      t.balance + t.reserve + 1 > 340282366920938463463374607431768211455) s
    else saveTask(s.with("deposits", s.deposits + 1), id,
      t.with("balance", t.balance + 1))
  }
  pure def settle(s: State, id: int, status: str): State = {
    val t = s.tasks.get(id)
    saveTask({ ...s, refunds: s.refunds + t.balance, fees: s.fees + t.lease }, id,
      { ...t, status: status, balance: 0, lease: 0, reserve: 0 })
  }
  pure def control(s: State, id: int, actor: int, command: str, value: int,
    weakGuard: bool): State = {
    val t = s.tasks.get(id)
    val inFlight = if (weakGuard) t.reserve > 0 else s.flight == id
    if (not(active(t)) or
      actor != t.controller or inFlight) s else {
      val checked = s.with("authorityOk", s.authorityOk and
        actor == t.owner and s.flight != id)
      if (command == "cancel") settle(checked, id, "cancelled")
      else if (command == "policy") saveTask(checked, id, t.with("bid", value))
      else if (command == "lease" and t.expiry < maxHeight and
        s.deposits < maxDeposits and s.height < t.expiry)
        saveTask({ ...checked, deposits: s.deposits + 1, fees: s.fees + 1 }, id,
          t.with("expiry", t.expiry + 1))
      else if (command == "resume" and t.status == "paused" and s.height < t.expiry)
        saveTask(checked.with("sequence", s.sequence + 1), id, { ...t,
          status: if (t.wait != "none" and t.inbox == 0) "waiting" else "due",
          reason: "", ready: s.height + 1, order: s.sequence + 1 })
      else s
    }
  }
  pure def begin(s: State): State =
    if (s.phase != "idle" or s.height >= maxHeight) s else
      { ...s, height: s.height + 1, phase: "maintenance", price: s.base,
        visits: 0, maintenance: 0, gas: 0, scanned: Set() }

  pure def maintain(s: State, id: int): State = {
    val t = s.tasks.get(id)
    if (s.phase != "maintenance" or s.flight != 0 or s.maintenance >= maxMaintenance) s
    else {
      val charged = s.with("maintenance", s.maintenance + 1)
      if (active(t) and s.height >= t.expiry) settle(charged, id, "expired")
      else if (terminal(t) and t.status != "pruned")
        saveTask(charged, id, t.with("status", "pruned"))
      else if (t.status == "due" and t.ready <= s.height)
        saveTask(charged, id, t.with("status", "ready"))
      else if (t.status == "waiting" and t.due <= s.height)
        // PR5 reserved its due/FIFO slot at staging; PR4 Go enqueues now.
        saveTask(charged.with("sequence", if (s.auction) s.sequence else s.sequence + 1), id, { ...t,
          status: "ready", inbox: if (t.inbox == 0) 2 else t.inbox,
          ready: if (s.auction) t.ready else s.height,
          order: if (s.auction) t.order else s.sequence + 1 })
      else if (s.auction and t.status == "ready" and t.bid < s.price and
        s.height - (if (t.ready > t.last) t.ready else t.last) >= 1)
        saveTask(charged, id, { ...t, status: "paused", reason: "price" })
      else charged
    }
  }
  pure def precedes(s: State, a: int, b: int): bool = {
    val x = s.tasks.get(a)
    val y = s.tasks.get(b)
    if (s.auction and x.bid != y.bid) x.bid > y.bid
    else if (s.auction and x.ready != y.ready) x.ready < y.ready
    else x.order < y.order or (x.order == y.order and a < b)
  }
  pure def candidate(s: State): int = {
    val available = s.queue.filter(id => s.tasks.get(id).ready <= s.height and
      (not(s.auction) or not(s.scanned.contains(id))))
    val owner = if (s.ring.length() == 0) 0 else s.ring.head()
    val choices = if (s.auction) available
      else available.filter(id => s.tasks.get(id).owner == owner)
    choices.fold(0, (best, id) =>
      if (best == 0) id else if (precedes(s, id, best)) id else best)
  }
  pure def inspect(s: State): State = {
    val id = candidate(s)
    if (not(Set("maintenance", "selection").contains(s.phase)) or s.flight != 0 or id == 0 or
      s.visits >= maxVisits or s.maintenance >= maxMaintenance) s else {
      val t = s.tasks.get(id)
      val visited = { ...s, phase: "selection", visits: s.visits + 1,
        maintenance: s.maintenance + 1, scanned: s.scanned.union(Set(id)) }
      // PR4 checks gas before expiry/rotation; PR5 skips only live eligible work.
      if (s.auction and t.bid < s.price) visited.with("phase", "closed")
      else if (t.cap > blockGas - s.gas and
        (not(s.auction) or (s.height < t.expiry and t.last != s.height)))
        if (s.auction) visited else visited.with("phase", "closed")
      else {
        val rotated = if (s.auction or s.ring.length() == 0) visited else
          visited.with("ring", s.ring.tail().append(t.owner))
        if (s.height >= t.expiry) settle(rotated, id, "expired")
        else if (t.bid < s.price or t.balance < t.cap * s.price)
          saveTask(rotated, id, { ...t, status: "paused",
            reason: if (t.bid < s.price) "price" else "funds" })
        else if (t.last == s.height or (t.wait != "none" and t.inbox == 0)) rotated
        else if (not(feeValid(s, t.cap))) rotated
        else {
          val charge = fee(s, t.cap)
          saveTask({ ...rotated, flight: id, before: t, guest: guestSnapshot(rotated),
            history: s.history.union(Set((id, s.height))), polls: s.polls + 1,
            priceOk: s.priceOk and t.bid >= s.price and s.price == s.base }, id,
            { ...t, status: "flight", reserve: charge,
              balance: t.balance - charge, last: s.height })
        }
      }
    }
  }
  // Pending response admission precedes outbound dispatch. The marker remains
  // visible while an immediate authenticated callback fills this staged wait.
  pure def stagePending(s: State, outcome: str, due: int, resolver: int): State = {
    if (s.flight == 0 or s.staged or not(Set("operation", "timer").contains(outcome))) s else {
      val t = s.tasks.get(s.flight)
      if (due <= s.height or due >= t.expiry or not(actors.contains(resolver))) s else
        saveTask({ ...s, staged: true, sequence: if (s.auction) s.sequence + 1 else s.sequence },
          s.flight, { ...t, continuation: t.continuation + 1,
            consumed: t.consumed + (if (t.inbox == 1) 1 else 0), inbox: 0,
            wait: outcome, generation: t.generation + 1, resolver: resolver, due: due,
            ready: if (s.auction) due else t.ready,
            order: if (s.auction) s.sequence + 1 else t.order })
    }
  }
  pure def finish(s: State, outcome: str, used: int, due: int): State = {
    // Host trust contract: used is trusted VM-metered consumption <= cap, and
    // SDK transaction/cache atomicity supplies the guest commit/rollback step.
    // These assumptions are not established by this nondeterministic model.
    if (s.flight == 0) s else {
      val id = s.flight
      val t = s.tasks.get(id)
      if (used < 0 or used > t.cap or
        not(feeValid(s, used))) s else {
        val charge = fee(s, used)
        val invalid = (Set("operation", "timer").contains(outcome) and
          (due <= s.height or due >= t.expiry)) or
          (s.staged and outcome != "rollback" and (outcome != t.wait or due != t.due))
        val failed = outcome == "rollback" or invalid
        val cache = if (failed) abortGuest(s) else s
        val paid = { ...cache, flight: 0, staged: false, gas: s.gas + used, fees: cache.fees + charge }
        val refunded = { ...t, balance: t.balance + t.reserve - charge, reserve: 0 }
        if (failed) {
          val rolled = { ...s.guest.tasks.get(id), balance: s.before.balance - charge,
            reserve: 0, last: s.height, status: "paused",
            reason: if (invalid) "invalid" else "poll_failure" }
          saveTask(paid.with("rollbackOk", s.rollbackOk and
            rolled.continuation == s.before.continuation and
            rolled.inbox == s.before.inbox and rolled.generation == s.before.generation),
            id, rolled)
        } else {
          val resolved = if (s.staged) refunded else { ...refunded, continuation: t.continuation + 1,
            wait: "none", inbox: 0,
            consumed: t.consumed + (if (t.inbox == 1) 1 else 0) }
          val committed = paid.with("effects", s.effects + 1)
          if (outcome == "complete" or outcome == "appfail")
            settle(saveTask(committed, id, resolved), id,
              if (outcome == "complete") "completed" else "failed")
          else if (outcome == "yield") saveTask(committed.with("sequence", s.sequence + 1),
            id, { ...resolved, status: "due", ready: s.height + 1,
              order: s.sequence + 1 })
          else if (s.staged) saveTask(committed, id,
            resolved.with("status", if (resolved.inbox == 0) "waiting" else "due"))
          else saveTask(committed.with("sequence", if (s.auction) s.sequence + 1 else s.sequence),
            id, { ...resolved, status: "waiting", wait: outcome,
              generation: t.generation + 1, resolver: 2, due: due,
              ready: if (s.auction) due else t.ready,
              order: if (s.auction) s.sequence + 1 else t.order })
        }
      }
    }
  }
  pure def finalize(s: State): State = {
    if (s.phase == "idle" or s.flight != 0) s else {
      val floor = if (s.base == 0) 0 else 1
      if (not(nextPricePrecondition(s.base, s.gas, 2, floor, 2))) s else
        { ...s, phase: "idle", base: if (s.auction)
          nextPrice(s.base, s.gas, 2, floor, 2) else s.base }
    }
  }
  pure def deliver(s: State, id: int, actor: int, generation: int): State = {
    val t = s.tasks.get(id)
    if (not(active(t)) or t.wait != "operation" or
      t.inbox != 0 or actor != t.resolver or generation != t.generation or
      s.height >= t.due or s.height >= t.expiry) s else
      saveTask(s.with("sequence", s.sequence + 1), id, { ...t, inbox: 1,
        accepted: t.accepted + 1,
        status: if (s.flight == id) "flight" else if (t.status == "paused") "paused" else "due",
        ready: s.height + 1, order: s.sequence + 1 })
  }

  // Snapshot validation operates on redundant persisted data rather than
  // merely assigning the saved record. Cryptographic authentication is a host
  // precondition; these checks do not model a forged authenticated snapshot.
  pure def conservationOf(s: State): bool =
    s.deposits == held(s) + s.fees + s.refunds and ids.forall(id => {
      val t = s.tasks.get(id)
      t.balance >= 0 and t.reserve >= 0 and t.lease >= 0
    })
  pure def identityOf(s: State): bool = ids.forall(id => {
    val t = s.tasks.get(id)
    (t.status == "absent" or (t.identity == id and t.owner == t.controller and
      t.owner == t.refund and actors.contains(t.owner))) and
      t.order <= s.sequence and t.generation >= 0 and
      t.consumed <= t.accepted and t.accepted <= t.generation and
      t.accepted - t.consumed == (if (t.inbox == 1) 1 else 0)
  })
  pure def metadataOf(s: State): bool = s.queue == canonicalQueue(s) and
    s.readyIndex == canonicalReadyIndex(s) and s.dueIndex == canonicalDueIndex(s) and
    s.ring.indices().map(i => s.ring.nth(i)) == queueOwners(s) and s.ring.length() == s.ring.indices().map(i => s.ring.nth(i)).size()
  pure def checkpoint(s: State): Checkpoint = {
    tasks: s.tasks, height: s.height, auction: s.auction, base: s.base,
    sequence: s.sequence, ring: s.ring,
    readyRecords: canonicalReadyIndex(s), dueRecords: canonicalDueIndex(s),
    atBoundary: s.phase == "idle" and s.flight == 0,
    deposits: s.deposits, fees: s.fees, refunds: s.refunds, effects: s.effects,
    history: s.history, polls: s.polls }
  pure def decodeDurable(saved: Checkpoint): State = {
    ...initial(saved.auction, saved.base), tasks: saved.tasks, height: saved.height,
    sequence: saved.sequence, ring: saved.ring, deposits: saved.deposits,
    fees: saved.fees, refunds: saved.refunds, effects: saved.effects,
    history: saved.history, polls: saved.polls }
  pure def checkpointValid(saved: Checkpoint): bool = {
    val decoded = decodeDurable(saved)
    val indexed = decoded.with("queue", canonicalQueue(decoded))
    saved.atBoundary and saved.polls == saved.history.size() and
      conservationOf(decoded) and identityOf(decoded) and
      saved.readyRecords == canonicalReadyIndex(decoded) and
      saved.dueRecords == canonicalDueIndex(decoded) and
      saved.ring.indices().map(i => saved.ring.nth(i)) == queueOwners(indexed) and
      saved.ring.length() == queueOwners(indexed).size() and
      ids.forall(id => {
        val t = decoded.tasks.get(id)
        t.reserve == 0 and t.status != "flight" and
          // Retained accepted outcomes may outlive their due height, including
          // a later failed attempt updating last. Admission and restore differ.
          (t.wait == "none" or (t.due > 0 and t.due < t.expiry and
            (t.wait != "operation" or actors.contains(t.resolver))))
      })
  }
  pure def restore(s: State, saved: Checkpoint, rebuildIndexes: bool): State = {
    if (not(checkpointValid(saved))) s else {
      val decoded = decodeDurable(saved)
      // false is an explicit negative-control mutation only: durable task and
      // ring data load, but the discarded live indexes never get rebuilt.
      val reconstructed = if (rebuildIndexes) { ...decoded,
        queue: canonicalQueue(decoded), readyIndex: canonicalReadyIndex(decoded),
        dueIndex: canonicalDueIndex(decoded) } else decoded
      reconstructed.with("restoreOk", metadataOf(reconstructed))
    }
  }

  var state: State
  action init = state' = initial(true, 1)
  action fixedInit = state' = initial(false, 1)
  action zeroInit = state' = initial(true, 0)
  action step = {
    nondet id = ids.oneOf()
    nondet actor = actors.oneOf()
    nondet bid = Set(0, 1, 2).oneOf()
    nondet cap = Set(1, 2).oneOf()
    nondet outcome = Set("yield", "operation", "timer", "complete", "appfail", "rollback").oneOf()
    nondet command = Set("resume", "cancel", "policy", "lease").oneOf()
    nondet used = Set(0, 1, 2).oneOf()
    nondet due = Set(2, 3, 6).oneOf()
    nondet generation = Set(0, 1, 2).oneOf()
    any {
      state' = register(state, id, actor, bid, cap),
      state' = fund(state, id, actor),
      state' = control(state, id, actor, command, bid, false),
      state' = begin(state), state' = maintain(state, id),
      state' = inspect(state), state' = finish(state, outcome, used, due),
      state' = stagePending(state, outcome, due, actor),
      state' = finalize(state), state' = deliver(state, id, actor, generation),
      state' = restore(state, checkpoint(state), true),
    }
  }
  val conservation = conservationOf(state)
  val authority = state.authorityOk and identityOf(state)
  val polling = state.polls == state.history.size() and state.gas <= blockGas and
    state.visits <= maxVisits and state.maintenance <= maxMaintenance and
    (state.flight == 0 or {
      val t = state.tasks.get(state.flight)
      t.status == "flight" and t.expiry > state.height and t.last == state.height and
        (state.staged or t.wait == "none" or t.inbox != 0) and state.gas + t.cap <= blockGas
    })
  val rollback = state.rollbackOk
  val price = state.priceOk and state.base >= 0 and state.price >= 0
  val restoreEquivalence = state.restoreOk and metadataOf(state)
  val safety = conservation and authority and polling and rollback and price and restoreEquivalence
}
