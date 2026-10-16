module native_async_tests {
  import native_async.* from "./native_async"

  pure val one = register(initial(true, 1), 1, 1, 2, 1)
  pure val pollingTask = inspect(begin(one))
  run immediateSameTaskDeliveryCommitTest = {
    val staged = stagePending(pollingTask, "operation", 3, 2)
    val delivered = deliver(staged, 1, 2, 1)
    val finished = finish(delivered, "operation", 1, 3)
    assert(staged.flight == 1 and delivered.tasks.get(1).inbox == 1 and
      control(delivered, 1, 1, "cancel", 0, false) == delivered and
      finished.flight == 0 and finished.tasks.get(1).status == "due" and
      finished.tasks.get(1).continuation == 1 and finished.tasks.get(1).accepted == 1 and
      finished.tasks.get(1).consumed == 0 and finished.tasks.get(1).inbox == 1 and
      inspect(finished).flight == 0 and conservationOf(finished))
  }
  run immediateSameTaskDeliveryRollbackTest = {
    val staged = stagePending(pollingTask, "operation", 3, 2)
    val delivered = deliver(fund(staged, 1, 2), 1, 2, 1)
    val failed = finish(delivered, "rollback", 1, 0)
    assert(delivered.tasks.get(1).inbox == 1 and failed.tasks.get(1).inbox == 0 and
      failed.tasks.get(1).accepted == 0 and failed.tasks.get(1).generation == 0 and
      failed.tasks.get(1).continuation == 0 and failed.deposits == 8 and failed.fees == 2)
  }
  run restoreAcceptedOutcomeAfterFailedLaterPollTest = {
    val waiting = finish(pollingTask, "operation", 1, 2)
    val accepted = deliver(waiting, 1, 2, 1)
    val p = inspect(maintain(begin(finalize(accepted)), 1))
    val saved = checkpoint(finalize(finish(p, "rollback", 1, 0)))
    assert(saved.tasks.get(1).last == 2 and saved.tasks.get(1).due == 2 and checkpointValid(saved))
  }
  // Each catches a missing guest-cache transition or rollback boundary.
  run nestedFundingCommitsTest = {
    val funded = fund(pollingTask, 1, 2)
    val done = finish(funded, "yield", 1, 0)
    assert(funded.tasks.get(1).balance == 6 and done.tasks.get(1).balance == 6 and
      done.deposits == 9 and conservationOf(done))
  }
  run nestedOtherTaskControlCommitAndRollbackTest = {
    val p = inspect(begin(register(one, 2, 2, 2, 1)))
    val nested = control(p, 2, 2, "cancel", 0, false)
    val committed = finish(nested, "yield", 1, 0)
    val failed = finish(nested, "rollback", 1, 0)
    assert(nested.tasks.get(2).status == "cancelled" and
      committed.tasks.get(2).status == "cancelled" and failed.tasks.get(2) == p.tasks.get(2) and
      failed.refunds == 0 and failed.fees == 3 and conservationOf(failed))
  }
  run nestedFundingRollbackKeepsConsumedFeesTest = {
    val nested = fund(pollingTask, 1, 2)
    val failed = finish(nested, "rollback", 1, 0)
    assert(nested.deposits == 9 and failed.deposits == 8 and
      failed.tasks.get(1).balance == 5 and failed.fees == 2 and conservationOf(failed))
  }
  run nestedDeliveryCommitAndRollbackTest = {
    val first = finish(inspect(begin(register(one, 2, 2, 2, 1))), "operation", 1, 3)
    val p = inspect(first)
    val nested = deliver(p, 1, 2, 1)
    assert(p.flight == 2 and nested.tasks.get(1).inbox == 1 and
      finish(nested, "yield", 1, 0).tasks.get(1).inbox == 1 and
      finish(nested, "rollback", 1, 0).tasks.get(1).inbox == 0)
  }
  run totalLiabilityOverflowRejectedTest = {
    val t = pollingTask.tasks.get(1)
    val full = saveTask(pollingTask, 1, t.with("balance", 340282366920938463463374607431768211455 - t.reserve))
    assert(fund(full, 1, 2) == full)
  }
  run sameTaskGuardAllOutcomesAndPostAttemptTest = {
    assert(Set(0, 1).forall(price => Set("timer", "yield", "complete", "appfail", "rollback").forall(outcome => {
      val p = inspect(begin(register(initial(true, price), 1, 1, price, 1)))
      val done = finish(p, outcome, 1, 2)
      Set("resume", "cancel", "policy", "lease").forall(command => control(p, 1, 1, command, 2, false) == p) and
        done.flight == 0 and (terminal(done.tasks.get(1)) or
          control(done, 1, 1, "policy", 2, false).tasks.get(1).bid == 2)
    })))
  }
  run dueBoundariesRollbackNestedEffectsTest = {
    assert(Set("timer", "operation").forall(outcome => Set(1, 3, 4, 5).forall(due => {
      val nested = fund(pollingTask, 1, 2)
      val done = finish(nested, outcome, 1, due)
      if (due == 3) done.tasks.get(1).status == "waiting" and done.deposits == 9
      else done.tasks.get(1).reason == "invalid" and done.deposits == 8 and done.fees == 2
    })))
  }
  run restoreRejectsInvalidWaitAndFlightTest = {
    val live = finalize(finish(pollingTask, "operation", 1, 3))
    val saved = checkpoint(live)
    val t = saved.tasks.get(1)
    val bad = saved.with("tasks", saved.tasks.set(1, t.with("resolver", 9)))
    assert(not(checkpointValid(bad)) and not(checkpointValid(checkpoint(pollingTask))) and
      restore(initial(true, 1), saved, true).flight == 0)
  }
  // Catch omitted registration/fee/escrow accounting, not a constructor shape.
  run registrationTest = assert(one.deposits == 8 and one.fees == 1 and
    one.tasks.get(1).balance == 6 and one.tasks.get(1).lease == 1)
  run contributorCannotResumeTest = {
    val paused = finish(pollingTask, "rollback", 1, 0)
    val funded = fund(paused, 1, 2)
    assert(funded.tasks.get(1).balance == 6 and
      funded.tasks.get(1).status == "paused" and
      control(funded, 1, 2, "resume", 0, false) == funded and
      control(funded, 1, 2, "cancel", 0, false) == funded and
      funded.tasks.get(1).refund == 1)
  }
  run zeroPriceFlightGuardTest = {
    val p = inspect(begin(register(initial(true, 0), 1, 1, 0, 1)))
    assert(p.flight == 1 and p.tasks.get(1).reserve == 0 and
      control(p, 1, 1, "cancel", 0, false) == p and
      control(p, 1, 1, "policy", 2, false) == p and
      control(p, 1, 1, "resume", 0, false) == p)
  }
  run acceptedInboxSurvivesFailedPollTest = {
    val waiting = finish(pollingTask, "operation", 1, 3)
    val accepted = deliver(waiting, 1, 2, 1)
    val nextPoll = inspect(maintain(begin(finalize(accepted)), 1))
    val failed = finish(nextPoll, "rollback", 1, 0)
    assert(waiting.tasks.get(1).wait == "operation" and
      accepted.tasks.get(1).inbox == 1 and nextPoll.flight == 1 and
      failed.tasks.get(1).inbox == 1 and
      failed.tasks.get(1).continuation == 1 and failed.effects == 1 and
      failed.tasks.get(1).consumed == 0 and failed.fees == 3)
  }
  run acceptedResultBeatsDeadlineTest = {
    val waiting = finish(pollingTask, "operation", 1, 2)
    val accepted = deliver(waiting, 1, 2, 1)
    val matured = maintain(begin(finalize(accepted)), 1)
    assert(matured.height == 2 and matured.tasks.get(1).inbox == 1 and
      deliver(matured, 1, 2, 1) == matured and
      deliver(waiting, 1, 1, 1) == waiting and
      deliver(waiting, 1, 2, 0) == waiting)
  }
  run duplicateHeightPollRejectedTest = {
    val yielded = finish(pollingTask, "yield", 1, 0)
    val promoted = maintain(yielded, 1)
    assert(promoted.tasks.get(1).ready == 2 and
      inspect(promoted).flight == 0 and promoted.polls == 1)
  }
  run maturedWaitPricePauseResumeTest = {
    val timer = finish(pollingTask, "timer", 1, 2)
    val cheap = control(timer, 1, 1, "policy", 0, false)
    val mature = maintain(begin(finalize(cheap)), 1)
    val paused = maintain(begin(finalize(mature)), 1)
    val raised = control(paused, 1, 1, "policy", 2, false)
    val resumed = control(raised, 1, 1, "resume", 0, false)
    assert(paused.tasks.get(1).status == "paused" and
      paused.tasks.get(1).inbox == 2 and resumed.tasks.get(1).inbox == 2 and
      resumed.tasks.get(1).continuation == 1 and resumed.tasks.get(1).ready == 4)
  }
  run expiryBoundaryWaitRejectedTest = {
    val rejected = finish(pollingTask, "operation", 1, 4)
    assert(rejected.tasks.get(1).status == "paused" and
      rejected.tasks.get(1).reason == "invalid" and rejected.effects == 0 and
      rejected.tasks.get(1).continuation == 0 and rejected.fees == 2)
  }
  run restoreMetadataRejectedTest = {
    val saved = checkpoint(one)
    val bad = saved.with("readyRecords", Set())
    assert(checkpointValid(saved) and not(checkpointValid(bad)) and
      restore(initial(true, 1), bad, true) == initial(true, 1))
  }
  run restoreNextStepTest = {
    val waiting = finish(pollingTask, "operation", 1, 3)
    val live = finalize(deliver(waiting, 1, 2, 1))
    val restored = restore(initial(true, 1), checkpoint(live), true)
    val nextLive = inspect(maintain(begin(live), 1))
    val nextRestored = inspect(maintain(begin(restored), 1))
    assert(restored != live and restored.before == emptyTask and
      nextLive.flight == 1 and nextRestored.flight == 1 and
      nextRestored.tasks.get(1).inbox == 1 and
      finish(nextRestored, "complete", 1, 0) ==
        finish(nextLive, "complete", 1, 0))
  }
  run visitCapContentionTest = {
    val three = register(register(one, 2, 1, 2, 1), 3, 2, 1, 1)
    val b1 = begin(three)
    val y1 = finish(inspect(b1), "yield", 1, 0)
    val y2 = finish(inspect(y1), "yield", 1, 0)
    val b2 = maintain(maintain(begin(finalize(y2)), 1), 2)
    val z1 = finish(inspect(b2), "yield", 1, 0)
    val z2 = finish(inspect(z1), "yield", 1, 0)
    assert(y2.visits == 2 and y2.gas == 2 and y2.gas < blockGas and
      y2.tasks.get(3).last == 0 and inspect(y2).flight == 0 and
      z2.visits == 2 and z2.gas == 2 and z2.tasks.get(3).last == 0)
  }
  run fixedContractRoundRobinTest = {
    val twoOwners = register(register(initial(false, 1), 1, 1, 1, 1), 2, 1, 2, 1)
    val three = register(twoOwners, 3, 2, 2, 1)
    val first = inspect(begin(three))
    val second = inspect(finish(first, "yield", 1, 0))
    assert(first.flight == 1 and second.flight == 3)
  }
  run graceAgeSurvivesPolicyEditTest = {
    val low = register(initial(true, 1), 1, 1, 0, 1)
    val young = maintain(begin(low), 1)
    val edited = control(begin(finalize(young)), 1, 1, "policy", 0, false)
    val aged = maintain(edited, 1)
    assert(young.tasks.get(1).status == "ready" and
      aged.tasks.get(1).status == "paused" and aged.tasks.get(1).reason == "price")
  }
  run leaseExtensionPaysStorageTest = {
    val extended = control(one, 1, 1, "lease", 0, false)
    assert(extended.tasks.get(1).expiry == 5 and extended.deposits == 9 and
      extended.fees == 2 and extended.tasks.get(1).lease == 1)
  }
  run applicationFailureCommitsAndRefundsTest = {
    val failed = finish(pollingTask, "appfail", 1, 0)
    assert(failed.effects == 1 and failed.tasks.get(1).status == "failed" and
      failed.refunds == 5 and failed.fees == 3 and held(failed) == 0)
  }
  run generationMetadataRejectedTest = {
    val bad = checkpoint(one).with("readyRecords", Set((1, 9, 1)))
    assert(not(checkpointValid(bad)))
  }
  run blockStartPriceAndUpdateTest = {
    val first = fund(fund(register(initial(true, 4), 1, 1, 4, 2), 1, 2), 1, 2)
    val p1 = inspect(begin(register(first, 2, 2, 4, 1)))
    val p2 = inspect(finish(p1, "yield", 2, 0))
    val ended = finalize(finish(p2, "complete", 1, 0))
    assert(p1.tasks.get(1).reserve == 8 and p2.tasks.get(2).reserve == 4 and
      ended.price == 4 and ended.base == 5 and ended.gas == 3 and
      conservationOf(ended))
  }
  run fixedPriceDoesNotUpdateTest = {
    val p = inspect(begin(register(initial(false, 2), 1, 1, 2, 1)))
    val ended = finalize(finish(p, "complete", 0, 0))
    assert(ended.base == 2 and ended.price == 2 and ended.fees == 2)
  }
  run expiredTaskCannotPollTest = {
    val b1 = begin(one)
    val b2 = begin(finalize(b1))
    val b3 = begin(finalize(b2))
    val b4 = begin(finalize(b3))
    val expired = maintain(b4, 1)
    assert(Set(true, false).forall(auction => {
      val selected = inspect(b4.with("auction", auction))
      selected.flight == 0 and selected.polls == 0 and selected.gas == 0 and
        selected.tasks.get(1).status == "expired" and selected.refunds == 6 and
        selected.fees == 2 and held(selected) == 0 and selected.queue == Set() and
        selected.ring == List() and selected.readyIndex == Set() and
        selected.dueIndex == Set() and conservationOf(selected)
    }) and expired.tasks.get(1).status == "expired" and
      expired.refunds == 6 and expired.fees == 2 and held(expired) == 0)
  }
  // With maintenance omitted until H4, only task 1 has an extended live lease.
  pure def expiryAfterGas(auction: bool): State = {
    val two = register(register(initial(auction, 1), 1, 1, 2, 2), 2, 2, 1, 2)
    val extended = control(two, 1, 1, "lease", 0, false)
    val b1 = begin(extended)
    val b2 = begin(finalize(b1))
    val b3 = begin(finalize(b2))
    finish(inspect(begin(finalize(b3))), "complete", 2, 0)
  }
  run expiredOversizedAuctionSettlesTest = {
    val spent = expiryAfterGas(true)
    val selected = inspect(spent)
    assert(spent.gas == 2 and spent.queue == Set(2) and spent.refunds == 4 and
      selected.tasks.get(2).status == "expired" and selected.refunds == 10 and
      selected.fees == 7 and held(selected) == 0 and conservationOf(selected) and
      selected.queue == Set() and selected.ring == List() and
      selected.readyIndex == Set() and selected.dueIndex == Set() and
      selected.flight == 0 and selected.polls == 1 and selected.gas == 2)
  }
  run expiredFixedInsufficientGasPreservesRingTest = {
    val spent = expiryAfterGas(false)
    val deferred = inspect(spent)
    val settled = inspect(begin(finalize(deferred)))
    assert(deferred.phase == "closed" and deferred.queue == Set(2) and
      deferred.ring == List(2) and deferred.tasks.get(2).status == "ready" and
      deferred.tasks.get(2).balance == 6 and deferred.tasks.get(2).lease == 1 and
      deferred.refunds == 4 and deferred.fees == 6 and deferred.polls == 1 and
      deferred.flight == 0 and deferred.gas == 2 and conservationOf(deferred) and
      settled.tasks.get(2).status == "expired" and settled.refunds == 10 and
      settled.fees == 7 and settled.queue == Set() and settled.ring == List() and
      settled.flight == 0 and settled.polls == 1 and conservationOf(settled))
  }
  run auctionPendingReservesDueSlotTest = {
    assert(Set("timer", "operation").forall(outcome => {
      val waiting = finish(pollingTask, outcome, 1, 3)
      waiting.tasks.get(1).status == "waiting" and waiting.tasks.get(1).ready == 3 and
        waiting.tasks.get(1).order == 2 and waiting.sequence == 2 and
        waiting.queue == Set() and waiting.dueIndex == Set((1, 1, 3))
    }))
  }
  run delayedAuctionPromotionPreservesGraceTest = {
    val cheap = control(finish(pollingTask, "timer", 1, 2), 1, 1, "policy", 0, false)
    val skipped = begin(finalize(cheap))
    val promoted = maintain(begin(finalize(skipped)), 1)
    val paused = maintain(promoted, 1)
    assert(promoted.height == 3 and promoted.tasks.get(1).ready == 2 and
      promoted.tasks.get(1).order == 2 and promoted.sequence == 2 and
      paused.tasks.get(1).status == "paused" and paused.tasks.get(1).reason == "price" and
      paused.tasks.get(1).inbox == 2 and paused.polls == 1)
  }
  pure def twoDelayedWaits(firstDue: int): State = {
    val two = register(one, 2, 2, 2, 1)
    val first = finish(inspect(begin(two)), "timer", 1, firstDue)
    val second = finish(inspect(first), "timer", 1, 2)
    finalize(begin(finalize(second)))
  }
  run reverseMaintenancePreservesAuctionFifoTest = {
    val live = twoDelayedWaits(2)
    val promoted = maintain(maintain(begin(live), 2), 1)
    assert(promoted.tasks.get(1).ready == 2 and promoted.tasks.get(2).ready == 2 and
      promoted.tasks.get(1).order == 3 and promoted.tasks.get(2).order == 4 and
      promoted.sequence == 4 and inspect(promoted).flight == 1)
  }
  run equalBidEarlierReadinessBeatsSequenceTest = {
    val promoted = maintain(maintain(begin(twoDelayedWaits(3)), 1), 2)
    assert(promoted.tasks.get(1).ready == 3 and promoted.tasks.get(1).order == 3 and
      promoted.tasks.get(2).ready == 2 and promoted.tasks.get(2).order == 4 and
      inspect(promoted).flight == 2)
  }
  run restoredDelayedWaitPreservesAuctionSuccessorTest = {
    val live = twoDelayedWaits(2)
    val restored = restore(initial(true, 9), checkpoint(live), true)
    val nextLive = inspect(maintain(maintain(begin(live), 2), 1))
    val nextRestored = inspect(maintain(maintain(begin(restored), 2), 1))
    assert(restored != live and restored.dueIndex == Set((1, 1, 2), (2, 1, 2)) and
      restored.tasks.get(1).ready == 2 and restored.tasks.get(1).order == 3 and
      restored.tasks.get(2).order == 4 and nextRestored.flight == 1 and
      nextLive.flight == 1 and finish(nextRestored, "complete", 1, 0) ==
        finish(nextLive, "complete", 1, 0))
  }
  run delayedFixedPromotionAllocatesCurrentSlotTest = {
    val p = inspect(begin(register(initial(false, 1), 1, 1, 1, 1)))
    val timer = finish(p, "timer", 1, 2)
    val skipped = begin(finalize(timer))
    val promoted = maintain(begin(finalize(skipped)), 1)
    assert(timer.sequence == 1 and promoted.height == 3 and
      promoted.tasks.get(1).ready == 3 and promoted.tasks.get(1).order == 2 and
      promoted.sequence == 2 and inspect(promoted).flight == 1)
  }
  run corruptAccountingRestoreRejectedTest = {
    val bad = checkpoint(one).with("deposits", 9)
    assert(not(checkpointValid(bad)) and restore(initial(true, 1), bad, true) == initial(true, 1))
  }
  run acceptedInboxConsumedOnceTest = {
    val waiting = finish(pollingTask, "operation", 1, 3)
    val accepted = deliver(waiting, 1, 2, 1)
    val p = inspect(maintain(begin(finalize(accepted)), 1))
    val consumed = finish(p, "yield", 1, 0)
    assert(consumed.tasks.get(1).accepted == 1 and consumed.tasks.get(1).consumed == 1 and
      consumed.tasks.get(1).inbox == 0 and deliver(consumed, 1, 2, 1) == consumed)
  }
  run unexpiredOperationTimeoutTest = {
    val waiting = finish(pollingTask, "operation", 1, 2)
    val timedOut = maintain(begin(finalize(waiting)), 1)
    assert(timedOut.height == 2 and timedOut.tasks.get(1).expiry == 4 and
      timedOut.tasks.get(1).inbox == 2 and timedOut.tasks.get(1).status == "ready" and
      deliver(timedOut, 1, 2, 1) == timedOut)
  }
  run pricedOutInspectionDoesNotPollTest = {
    val low = begin(register(initial(true, 1), 1, 1, 0, 1))
    val inspected = inspect(low)
    assert(inspected.visits == 1 and inspected.flight == 0 and inspected.gas == 0 and
      inspected.tasks.get(1).status == "ready" and inspected.phase == "closed")
  }
  run prunedIdentityCannotBeReusedTest = {
    val completed = finish(pollingTask, "complete", 1, 0)
    val pruned = maintain(begin(finalize(completed)), 1)
    val idle = finalize(pruned)
    assert(pruned.tasks.get(1).status == "pruned" and
      register(idle, 1, 2, 2, 1) == idle and pruned.tasks.get(1).identity == 1)
  }
  // A restart must reconstruct the discarded queue and preserve historical
  // contract rotation, not simply copy a complete saved live State.
  run restartRebuildsQueueAndRingSuccessorTest = {
    val registered = register(register(register(initial(false, 1), 1, 1, 1, 1),
      2, 1, 1, 1), 3, 2, 1, 1)
    val live = finalize(finish(inspect(begin(registered)), "yield", 1, 0))
    val discarded = { ...live, queue: Set(), readyIndex: Set(), dueIndex: Set() }
    val restored = restore(initial(false, 9), checkpoint(discarded), true)
    val nextLive = inspect(begin(live))
    val nextRestored = inspect(begin(restored))
    assert(restored.queue == Set(2, 3) and restored.ring == List(2, 1) and
      nextLive.flight == 3 and nextRestored.flight == 3 and
      finish(nextRestored, "complete", 1, 0) == finish(nextLive, "complete", 1, 0))
  }
  run restartDiscardsExecutionCacheTest = {
    val live = finalize(finish(pollingTask, "operation", 1, 3))
    val restored = restore(initial(true, 9), checkpoint(live), true)
    assert(live.before != emptyTask and live.scanned == Set(1) and
      restored.before == emptyTask and restored.scanned == Set() and
      restored.visits == 0 and restored.maintenance == 0 and restored.gas == 0 and
      restored.tasks.get(1).continuation == 1 and restored.tasks.get(1).due == 3)
  }
  run omittedReconstructionLosesRunnableSuccessorTest = {
    val saved = checkpoint(one)
    val restored = restore(initial(true, 9), saved, true)
    val omitted = restore(initial(true, 9), saved, false)
    assert(inspect(begin(restored)).flight == 1 and
      inspect(begin(omitted)).flight == 0 and not(metadataOf(omitted)))
  }
  run missingDurableRingRejectedTest = {
    val bad = checkpoint(one).with("ring", List())
    assert(not(checkpointValid(bad)) and
      restore(initial(true, 9), bad, true) == initial(true, 9))
  }
  run midBlockCheckpointRejectedTest = {
    val saved = checkpoint(begin(one))
    assert(not(checkpointValid(saved)) and
      restore(initial(true, 9), saved, true) == initial(true, 9))
  }
}
