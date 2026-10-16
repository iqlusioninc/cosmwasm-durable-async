module native_batch_auction_tests {
  import native_batch_auction.* from "./native_batch_auction"
  // The literal expected results are hand-derived, not computed by the allocator.
  pure def book(supply: int, batch: int): State = {
    val committed = commit(commit(commit(initial(supply, batch), 1, 12), 2, 12), 3, 12)
    val a = reveal(at(committed, C), 1, 2, 2, 3)
    val b = reveal(a, 2, 2, 1, 2)
    at(reveal(b, 3, 2, 1, 1), R)
  }
  pure def settle(s: State): State = 0.to(11).fold(s, (a, _) => advance(poll(a, false, false, false)))
  run underfullReserveTest = {
    val s = at(reveal(at(commit(initial(3, 1), 1, 12), C), 1, 2, 2, 3), R)
    val f = settle(s)
    assert(f.book.phase == "Finalized" and f.book.price == 1 and
      f.book.bids.get(1).sale == 2 and f.book.bids.get(1).refund == 10 and f.book.proceeds == 2)
  }
  run exactDemandReserveTest = {
    val s = at(reveal(at(commit(initial(2, 2), 1, 12), C), 1, 2, 2, 3), R)
    val f = settle(s)
    assert(f.book.phase == "Finalized" and f.book.price == 1 and f.book.proceeds == 2)
  }
  run marginalDustCommitOrderTest = {
    val f = settle(book(3, 1))
    assert(f.book.phase == "Finalized" and f.book.price == 2 and
      f.book.bids.get(1).allocation == 2 and f.book.bids.get(2).allocation == 1 and
      f.book.bids.get(3).allocation == 0 and f.book.residual == 0 and f.book.proceeds == 6)
  }
  run threeBidDustOutsideFiniteProfileTest = {
    val c = commit(commit(commit(initial(5, 2), 1, 12), 2, 12), 3, 12)
    val a = reveal(at(c, C), 1, 3, 1, 1)
    val b = reveal(a, 2, 3, 1, 2)
    val f = settle(at(reveal(b, 3, 3, 1, 3), R))
    assert(f.book.phase == "Finalized" and f.book.bids.get(1).allocation == 2 and
      f.book.bids.get(2).allocation == 2 and f.book.bids.get(3).allocation == 1)
  }
  run exactStandardBookWithUnrevealedBidTest = {
    // This is the literal Task 3 standard book, not the three-bid fixture above.
    // Extra address classes/lot size/reserve belong only to this instance.
    val configured = initialFor(5, 2, Set(1, 2, 3, 4, 5), 10, 2)
    if (not(configured.book.bids.keys().contains(5))) assert(false) else {
      val c = commit(commit(commit(commit(commit(configured, 1, 8), 2, 6), 3, 6), 4, 2), 5, 7)
      val a = reveal(at(c, C), 1, 2, 2, 1)
      val b = reveal(a, 2, 2, 1, 2)
      val d = reveal(b, 3, 2, 1, 3)
      val f = settle(at(reveal(d, 4, 1, 0, 4), R))
      val paid = Set(1, 2, 3, 4, 5).fold(claim(f, 0, false, false, false),
        (s, id) => claim(s, id, false, false, false))
      assert(f.book.phase == "Finalized" and f.book.price == 3 and f.collateral == 29 and
        List(1, 2, 3, 4, 5).foldl(List(), (xs, id) => xs.append(f.book.bids.get(id).allocation)) == List(2, 2, 1, 0, 0) and
        List(1, 2, 3, 4, 5).foldl(List(), (xs, id) => xs.append(f.book.bids.get(id).refund)) == List(2, 0, 3, 2, 7) and
        List(1, 2, 3, 4, 5).foldl(List(), (xs, id) => xs.append(f.book.bids.get(id).sale)) == List(20, 20, 10, 0, 0) and
        f.book.proceeds == 15 and not(f.book.bids.get(5).revealed) and
        paid.quotePaid.get(0) == 15 and paid.salePaid.get(0) == 0 and
        paid.saleBank == 0 and paid.quoteBank == 0 and safe(f) and safe(paid))
    }
  }
  run unrevealedRefundTest = {
    val f = settle(at(commit(initial(2, 1), 1, 12), R))
    val paid = claim(f, 1, false, false, false)
    assert(f.book.bids.get(1).refund == 12 and f.book.bids.get(1).sale == 0 and paid.quotePaid.get(1) == 12)
  }
  // These callers have no existing bid/reveal and capacity/collateral remain
  // valid, so removing the closing-height guard must change the rejected state.
  run commitCloseBoundaryFreeSlotTest = {
    val s = commit(initial(3, 1), 1, 12)
    val before = at(s, C - 1)
    val accepted = commit(before, 2, 6)
    val closed = at(s, C)
    assert(accepted.book.order == List(1, 2) and accepted.book.bids.get(2).collateral == 6 and
      accepted.collateral == 18 and accepted.quoteBank == 18 and
      closed.book.order.length() == 1 and closed.book.bids.get(2).collateral == 0 and
      commit(closed, 2, 6) == closed)
  }
  run revealCloseBoundaryUnrevealedTest = {
    val s = commit(initial(3, 1), 1, 12)
    val before = at(s, R - 1)
    val accepted = reveal(before, 1, 2, 2, 3)
    val closed = at(s, R)
    assert(accepted.book.bids.get(1).revealed and accepted.book.total == 2 and
      accepted.book.buckets.get(2) == 2 and not(closed.book.bids.get(1).revealed) and
      reveal(closed, 1, 2, 2, 3) == closed)
  }
  run boundedItemsAndOnePollPerHeightTest = {
    val p = poll(book(3, 1), false, false, false)
    assert(p.polls == 1 and p.visits <= 1 and poll(p, false, false, false) == p)
  }
  run lockedAndRepeatedClaimsTest = {
    val s = book(3, 1)
    val f = settle(s)
    val paid = claim(f, 1, false, false, false)
    assert(claim(s, 1, false, false, false) == s and paid.book.bids.get(1).claimed and
      paid.salePaid.get(3) == 2 and paid.quotePaid.get(1) == 8 and
      claim(paid, 1, false, false, false) == paid)
  }
  run twoCoinClaimFailureTest = {
    val f = settle(book(3, 1))
    assert(claim(f, 1, true, false, false) == f and not(f.book.bids.get(1).claimed) and f.book.bids.get(1).sale == 2)
  }
  run partialWriteRollbackTest = {
    val s = advance(poll(book(3, 2), false, false, false))
    val failed = poll(s, true, false, false)
    assert(failed.book == s.book and failed.gas == s.gas - 1 and failed.fees == s.fees + 1 and
      failed.paused and failed.rolledBack and failed.rollbackSafe)
  }
  run injectedFailureStopsAfterFirstItemTest = {
    val s = advance(poll(book(3, 2), false, false, false))
    val mutation = poll(s, true, true, false)
    assert(mutation.book.cursor == 1 and mutation.book.bids.get(1).allocation == 2 and
      mutation.book.bids.get(2).allocation == 0 and not(safe(mutation)))
  }
  run finalizeBoundaryTest = {
    val s = book(3, 2)
    val p = 0.to(4).fold(s, (a, _) => advance(poll(a, false, false, false)))
    assert(p.book.phase == "Finalize" and poll(at(p, E - 1), false, false, false).book.phase == "Finalized" and
      poll(at(p, E), false, false, false).book.phase == "Finalize")
  }
  run unfinishedLastHeightCommitsBatchLocksClaimsUntilExpiryTest = {
    val scan = advance(poll(book(3, 2), false, false, false))
    val last = at(scan, E - 1)
    val stopped = poll(last, false, false, false)
    val expired = advance(stopped)
    val refunded = claim(expired, 1, false, false, false)
    val inventory = claim(refunded, 0, false, false, false)
    // paused coalesces stopped execution in this abstraction. Source completes
    // its host task with LeaseWindowExhausted; business remains Active until E.
    assert(stopped.book.phase == "AllocateBase" and stopped.book.cursor == 2 and
      stopped.book.bids.get(1).allocation == 2 and stopped.book.allocated == 2 and
      stopped.book != last.book and stopped.paused and stopped.lastPoll == E - 1 and
      stopped.fees == last.fees + 1 and safe(stopped) and
      claim(stopped, 1, false, false, false) == stopped and
      claim(stopped, 0, false, false, false) == stopped and
      poll(stopped, false, false, false) == stopped and
      poll(expired, false, false, false) == expired and
      refunded.book.phase == "Aborted" and refunded.quotePaid.get(1) == 12 and
      refunded.salePaid.get(3) == 0 and inventory.salePaid.get(0) == 3 and safe(inventory))
  }
  pure def pollNext(s: State): State = advance(poll(s, false, false, false))
  run abortEveryReachablePhaseRefundsFullPrincipalTest = {
    val s = book(3, 1)
    val scan = pollNext(s)
    val allocationStart = pollNext(scan)
    val allocationPartial = pollNext(allocationStart)
    val buildStart = pollNext(pollNext(allocationPartial))
    val buildPartial = pollNext(buildStart)
    val finalizing = pollNext(pollNext(buildPartial))
    val prefixes = List(s, scan, allocationPartial, buildPartial, finalizing)
    assert(scan.book.phase == "ScanPrice" and scan.book.scan == 1 and scan.book.higher == 2 and
      allocationPartial.book.phase == "AllocateBase" and allocationPartial.book.cursor == 1 and
      allocationPartial.book.allocated == 2 and allocationPartial.book.bids.get(1).allocation == 2 and
      buildPartial.book.phase == "BuildClaims" and buildPartial.book.cursor == 1 and
      buildPartial.book.proceeds == 4 and buildPartial.book.saleLiability == 2 and buildPartial.book.refundLiability == 8 and
      finalizing.book.phase == "Finalize" and finalizing.book.cursor == 3 and
      finalizing.book.allocated == 3 and finalizing.book.proceeds == 6 and finalizing.book.refundLiability == 30 and
      prefixes.indices().forall(i => {
      val p = prefixes.nth(i)
      val a = abort(at(p, E))
      val one = claim(a, 1, false, false, false)
      val two = claim(one, 2, false, false, false)
      val three = claim(two, 3, false, false, false)
      val allPaid = claim(three, 0, false, false, false)
      a.book.phase == "Aborted" and safe(a) and safe(one) and safe(two) and safe(three) and safe(allPaid) and
        bidders.forall(id => allPaid.quotePaid.get(id) == 12 and allPaid.salePaid.get(id) == 0 and allPaid.book.bids.get(id).claimed) and
        allPaid.salePaid.get(0) == 3 and allPaid.quotePaid.get(0) == 0 and allPaid.sellerClaimed and
        allPaid.saleBank == 0 and allPaid.quoteBank == 0 and allPaid.collateral == 36 and
        poll(a, false, false, false) == a and poll(allPaid, false, false, false) == allPaid and
        poll(advance(allPaid), false, false, false) == advance(allPaid)
    }))
  }
  run finalizedNeverAbortsTest = {
    val f = at(settle(book(3, 1)), 21)
    assert(f.book.phase == "Finalized" and abort(f) == f and
      claim(f, 1, false, false, false).quotePaid.get(1) == 8 and
      claim(f, 1, false, false, false).salePaid.get(3) == 2)
  }
  run hostPrunedClaimsTest = {
    val f = settle(book(3, 1)).with("hostPresent", false)
    assert(claim(f, 1, false, false, false).salePaid.get(3) == 2 and
      claim(at(book(3, 1), E).with("hostPresent", false), 1, false, false, false).quotePaid.get(1) == 12)
  }
  run restartSuccessorsAndClaimFlagsTest = {
    val p = advance(poll(book(3, 1), false, false, false))
    val r = restart(p, false)
    val f = claim(settle(p), 1, false, false, false)
    val restored = restart(f, false)
    assert(r.restarted and poll(r, false, false, false).book == poll(p, false, false, false).book and
      restored.book == f.book and claim(restored, 1, false, false, false) == restored)
  }
}
