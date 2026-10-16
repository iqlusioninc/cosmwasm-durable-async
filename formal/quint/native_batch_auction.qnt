module native_batch_auction {
  pure val bidders = Set(1, 2, 3)
  pure val ticks = Set(0, 1, 2)
  pure val C = 2
  pure val R = 4
  pure val E = 20
  type Bid = { collateral: int, qty: int, tick: int, beneficiary: int,
    revealed: bool, claimed: bool, allocation: int, sale: int, refund: int }
  type Book = { bids: int -> Bid, order: List[int], phase: str, cursor: int,
    buckets: int -> int, total: int,
    scan: int, higher: int, marginal: int, price: int, residual: int,
    allocated: int, proceeds: int, saleLiability: int, refundLiability: int }
  type State = { book: Book, height: int, supply: int, batch: int,
    classes: Set[int], lotSize: int, reserve: int,
    sellerClaimed: bool, saleBank: int, quoteBank: int,
    salePaid: int -> int, quotePaid: int -> int, collateral: int,
    gas: int, gasDeposits: int, fees: int, paused: bool, lastPoll: int,
    polls: int, history: Set[int], visits: int, hostPresent: bool,
    rolledBack: bool, restarted: bool, claimSafe: bool, rollbackSafe: bool,
    terminalSafe: bool }
  pure val emptyBid: Bid = { collateral: 0, qty: 0, tick: 0, beneficiary: 0,
    revealed: false, claimed: false, allocation: 0, sale: 0, refund: 0 }
  // Immutable instance parameters. Random init below fixes the original small
  // domain; larger deterministic books reuse these same batch transitions.
  pure def initialFor(supply: int, batch: int, classes: Set[int], lotSize: int, reserve: int): State = {
    book: { bids: classes.mapBy(_ => emptyBid), order: List(), phase: "Open",
      buckets: ticks.mapBy(_ => 0), total: 0,
      cursor: 0, scan: 2, higher: 0, marginal: -1, price: reserve, residual: 0,
      allocated: 0, proceeds: 0, saleLiability: 0, refundLiability: 0 },
    height: 0, supply: supply, batch: batch, sellerClaimed: false,
    classes: classes, lotSize: lotSize, reserve: reserve,
    saleBank: supply * lotSize, quoteBank: 0,
    salePaid: classes.union(Set(0)).mapBy(_ => 0), quotePaid: classes.union(Set(0)).mapBy(_ => 0),
    collateral: 0, gas: 12, gasDeposits: 12, fees: 0, paused: false,
    lastPoll: -1, polls: 0, history: Set(), visits: 0, hostPresent: true,
    rolledBack: false, restarted: false, claimSafe: true, rollbackSafe: true,
    terminalSafe: true }
  pure def initial(supply: int, batch: int): State = initialFor(supply, batch, bidders, 1, 1)
  pure def terminal(s: State): bool = Set("Finalized", "Aborted").contains(s.book.phase)
  pure def commit(s: State, id: int, collateral: int): State = {
    if (s.height >= C or terminal(s) or collateral < 1 or collateral > 12 or
      s.book.bids.get(id).collateral > 0 or s.book.order.length() >= s.classes.size()) s else {
      val b = emptyBid.with("collateral", collateral)
      { ...s, collateral: s.collateral + collateral, quoteBank: s.quoteBank + collateral,
        book: { ...s.book, bids: s.book.bids.set(id, b), order: s.book.order.append(id) } }
    }
  }
  // Digest authentication is abstracted to caller-owned id and a valid opening.
  // IDs are canonical-address classes; no cryptographic or Bech32 claim.
  pure def reveal(s: State, id: int, qty: int, tick: int, beneficiary: int): State = {
    val bid = s.book.bids.get(id)
    if (s.height < C or s.height >= R or terminal(s) or bid.revealed or
      qty < 1 or qty > s.supply or not(ticks.contains(tick)) or
      not(s.classes.contains(beneficiary)) or qty * (tick + s.reserve) > bid.collateral) s else {
      val b = { ...bid, qty: qty, tick: tick, beneficiary: beneficiary, revealed: true }
      s.with("book", { ...s.book, bids: s.book.bids.set(id, b),
        total: s.book.total + qty, buckets: s.book.buckets.set(tick, s.book.buckets.get(tick) + qty) })
    }
  }
  pure def target(s: State): int = if (s.book.total < s.supply) s.book.total else s.supply
  pure def scanItem(s: State): State = {
    val b = s.book
    if (b.phase != "ScanPrice" or b.scan < 0) s else {
      val d = b.buckets.get(b.scan)
      val changed = if (b.higher + d >= s.supply)
        { ...b, phase: "AllocateBase", marginal: b.scan, price: b.scan + s.reserve, cursor: 0 }
      else { ...b, higher: b.higher + d, scan: b.scan - 1 }
      { ...s, book: changed, visits: s.visits + 1 }
    }
  }
  pure def allocateItem(s: State): State = {
    val b = s.book
    if (b.phase != "AllocateBase") s
    else if (b.cursor == b.order.length()) s.with("book", { ...b,
      phase: "BuildClaims", cursor: 0, residual: target(s) - b.allocated })
    else {
      val id = b.order.nth(b.cursor)
      val bid = b.bids.get(id)
      val a = if (not(bid.revealed)) 0 else if (b.total <= s.supply or bid.tick > b.marginal) bid.qty
        else if (bid.tick < b.marginal) 0 else bid.qty * (s.supply - b.higher) / b.buckets.get(b.marginal)
      val done = b.cursor + 1 == b.order.length()
      { ...s, visits: s.visits + 1, book: { ...b, bids: b.bids.set(id, bid.with("allocation", a)),
        allocated: b.allocated + a, cursor: if (done) 0 else b.cursor + 1,
        phase: if (done) "BuildClaims" else b.phase,
        residual: if (done) target(s) - b.allocated - a else b.residual } }
    }
  }
  pure def buildItem(s: State): State = {
    val b = s.book
    if (b.phase != "BuildClaims") s
    else if (b.cursor == b.order.length()) s.with("book", b.with("phase", "Finalize"))
    else {
      val id = b.order.nth(b.cursor)
      val bid = b.bids.get(id)
      val dust = if (b.residual > 0 and bid.revealed and bid.tick == b.marginal and bid.allocation < bid.qty) 1 else 0
      val a = bid.allocation + dust
      val refund = bid.collateral - a * b.price
      val done = b.cursor + 1 == b.order.length()
      { ...s, visits: s.visits + 1, book: { ...b,
        bids: b.bids.set(id, { ...bid, allocation: a, sale: a * s.lotSize, refund: refund }),
        allocated: b.allocated + dust, residual: b.residual - dust,
        proceeds: b.proceeds + a * b.price, saleLiability: b.saleLiability + a * s.lotSize,
        refundLiability: b.refundLiability + refund, cursor: b.cursor + 1,
        phase: if (done) "Finalize" else b.phase } }
    }
  }
  pure def batchStep(s: State, limit: int): State = {
    val b = s.book
    if (b.phase == "Open") {
      val changed = s.with("book", { ...b, phase: if (b.total <= s.supply) "AllocateBase" else "ScanPrice" })
      if (b.total <= s.supply) changed else 1.to(limit).fold(changed, (a, _) => scanItem(a))
    } else if (b.phase == "ScanPrice") 1.to(limit).fold(s, (a, _) => scanItem(a))
    else if (b.phase == "AllocateBase") 1.to(limit).fold(s, (a, _) => allocateItem(a))
    else if (b.phase == "BuildClaims") 1.to(limit).fold(s, (a, _) => buildItem(a))
    else if (b.phase == "Finalize" and b.cursor == b.order.length() and b.residual == 0 and
      b.allocated == target(s) and b.saleLiability == target(s) * s.lotSize and
      b.refundLiability + b.proceeds == s.collateral and b.proceeds == b.allocated * b.price)
      s.with("book", b.with("phase", "Finalized"))
    else s
  }
  pure def poll(s: State, abortTx: bool, weakRollback: bool, weakExpiry: bool): State = {
    if (terminal(s) or s.height < R or s.height > E or (s.height == E and not(weakExpiry)) or
      s.paused or s.lastPoll == s.height or not(s.hostPresent)) s
    else if (s.gas == 0) s.with("paused", true) else {
      // s is the complete pre-transaction snapshot, including all bid writes,
      // cursors, claim flags, balances and gas principal separation.
      // Fault injection stops after the first item, before the remaining B-1.
      val staged = batchStep(s.with("visits", 0), if (abortTx) 1 else s.batch)
      val cache = if (abortTx and not(weakRollback)) s else staged
      { ...cache, gas: s.gas - 1, fees: s.fees + 1, lastPoll: s.height,
        polls: s.polls + 1, history: s.history.union(Set(s.height)),
        paused: abortTx or (s.height + 1 >= E and not(terminal(cache))),
        rolledBack: s.rolledBack or abortTx,
        rollbackSafe: s.rollbackSafe and (not(abortTx) or cache == s),
        terminalSafe: s.terminalSafe and (cache.book.phase != "Finalized" or s.height < E) }
    }
  }
  pure def abort(s: State): State = if (terminal(s) or s.height < E) s
    else s.with("book", s.book.with("phase", "Aborted"))
  pure def claim(s: State, id: int, failSecond: bool, provisional: bool, repeated: bool): State = {
    val ready = abort(s)
    val b = ready.book
    val seller = id == 0
    val bid = if (seller) emptyBid else b.bids.get(id)
    val claimed = if (seller) ready.sellerClaimed else bid.claimed
    if ((not(terminal(ready)) and not(provisional)) or (claimed and not(repeated)) or
      (not(seller) and bid.collateral == 0)) s else {
      val sale = if (b.phase == "Aborted") (if (seller) ready.supply * ready.lotSize else 0)
        else if (seller) (ready.supply - b.allocated) * ready.lotSize else bid.sale
      val quote = if (b.phase == "Aborted") (if (seller) 0 else bid.collateral)
        else if (seller) b.proceeds else bid.refund
      val recipient = if (seller) 0 else if (bid.revealed) bid.beneficiary else id
      val marked = if (seller) ready.with("sellerClaimed", true)
        else ready.with("book", b.with("bids", b.bids.set(id, bid.with("claimed", true))))
      val saleSent = { ...marked, saleBank: ready.saleBank - sale,
        salePaid: ready.salePaid.set(recipient, ready.salePaid.get(recipient) + sale) }
      // Bank failure after first coin restores the complete outer transaction,
      // including lazy abort and the claim flag. Mutation controls bypass only
      // the named guard; ordinary bank insolvency remains a rejection.
      if (failSecond or sale > ready.saleBank or quote > ready.quoteBank) s else
        { ...saleSent, quoteBank: ready.quoteBank - quote,
          quotePaid: ready.quotePaid.set(id, ready.quotePaid.get(id) + quote),
          claimSafe: ready.claimSafe and terminal(ready) and not(claimed) }
    }
  }
  pure def restart(s: State, loseFlags: bool): State = {
    // Durable state round-trip; transient item-visit counter is discarded.
    val b = if (loseFlags) s.book.with("bids", s.classes.mapBy(id => s.book.bids.get(id).with("claimed", false))) else s.book
    { ...s, book: b, restarted: true, visits: 0,
      sellerClaimed: if (loseFlags) false else s.sellerClaimed }
  }
  pure def advance(s: State): State = if (s.height < 21) s.with("height", s.height + 1) else s
  pure def at(s: State, height: int): State = s.with("height", height)
  pure def demand(s: State): int = s.book.total
  pure def safe(s: State): bool = {
    val b = s.book
    val paidSale = s.classes.union(Set(0)).fold(0, (n, id) => n + s.salePaid.get(id))
    val paidQuote = s.classes.union(Set(0)).fold(0, (n, id) => n + s.quotePaid.get(id))
    val claimQuote = s.classes.fold(0, (n, id) => { val bid = b.bids.get(id)
      n + (if (bid.claimed) (if (b.phase == "Aborted") bid.collateral else bid.refund) else 0) }) +
      (if (s.sellerClaimed and b.phase == "Finalized") b.proceeds else 0)
    val claimSale = s.classes.fold(0, (n, id) => n + (if (b.bids.get(id).claimed and b.phase == "Finalized") b.bids.get(id).sale else 0)) +
      (if (s.sellerClaimed) (if (b.phase == "Aborted") s.supply * s.lotSize else (s.supply - b.allocated) * s.lotSize) else 0)
    s.claimSafe and s.rollbackSafe and s.terminalSafe and
      s.saleBank >= 0 and s.quoteBank >= 0 and s.saleBank + paidSale == s.supply * s.lotSize and
      s.quoteBank + paidQuote == s.collateral and paidSale == claimSale and paidQuote == claimQuote and
      s.gas >= 0 and s.gas + s.fees == s.gasDeposits and s.polls == s.history.size() and
      s.visits <= s.batch and b.cursor >= 0 and b.cursor <= b.order.length() and
      b.total == s.classes.fold(0, (n, id) => n + b.bids.get(id).qty) and
      s.classes.forall(id => { val bid = b.bids.get(id)
        bid.allocation >= 0 and bid.allocation <= bid.qty and bid.refund >= 0 and
          (not(bid.claimed) or terminal(s)) }) and
      (b.phase != "Finalized" or (b.allocated == target(s) and b.residual == 0 and
        b.proceeds + b.refundLiability == s.collateral and b.saleLiability == target(s) * s.lotSize))
  }
  var state: State
  action init = {
    nondet supply = 1.to(3).oneOf()
    nondet batch = 1.to(2).oneOf()
    state' = initial(supply, batch)
  }
  action step = {
    nondet id = bidders.oneOf()
    nondet qty = 1.to(3).oneOf()
    nondet tick = ticks.oneOf()
    nondet collateral = 1.to(12).oneOf()
    nondet beneficiary = bidders.oneOf()
    any {
      state' = commit(state, id, collateral),
      state' = reveal(state, id, qty, tick, beneficiary),
      state' = advance(state),
      state' = poll(state, false, false, false),
      state' = poll(state, false, false, false),
      state' = poll(state, true, false, false),
      state' = abort(state),
      state' = claim(state, id, false, false, false),
      state' = claim(state, 0, false, false, false),
      state' = claim(state, id, true, false, false),
      state' = restart(state, false),
      state' = state.with("hostPresent", if (terminal(state) or state.height >= E) false else state.hostPresent),
      state' = if (state.gasDeposits <= 24) { ...state, gas: state.gas + 2, gasDeposits: state.gasDeposits + 2 } else state,
      state' = state.with("paused", false),
    }
  }
  val safety = safe(state)
  val sawSettlement = state.book.phase == "Finalized"
  val sawAbort = state.book.phase == "Aborted"
  val sawRollback = state.rolledBack
  val sawRestart = state.restarted
}
