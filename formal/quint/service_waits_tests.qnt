module service_waits_tests {
  import service_waits.* from "./service_waits"
  pure val waiting = startWorkflow(initial, 1, 1, false, false)
  // Missing Start/Begin atomic allocation or accepting outsider/funds breaks these.
  run startAndAdmissionTest = assert(waiting.chain.nextId == 2 and
    waiting.chain.pending == Set((1, 1)) and waiting.chain.seen == Set((1, 1)) and
    startWorkflow(initial, 4, 1, false, false) == initial and
    startWorkflow(initial, 1, 1, true, false) == initial and
    startWorkflow(initial, 1, 1, false, true).chain == initial.chain)
  run wrongSenderAndStaleSequenceTest = assert(
    waiting.chain.workflows.get(1).status == "waiting" and
    resume(waiting, 1, 1, 1, "success", false) == waiting and
    resume(waiting, 1, 2, 2, "success", false) == waiting and
    deliver(waiting, (1, 1), 4, "success") == waiting)
  run duplicateResponseAndStaleExpireTest = {
    val second = deliver(waiting, (1, 1), 1, "success")
    assert(second.chain.workflows.get(1).seq == 2 and
      second.chain.pending == Set((1, 2)) and
      deliver(second, (1, 1), 1, "success") == second and
      resume(second, 1, 1, 2, "success", false) == second and
      expire(second, 1, 1) == second)
  }
  run deadlineIsNotCancellationTest = {
    val elapsed = advance(advance(remoteEffect(waiting)))
    val expired = expire(elapsed, 1, 1)
    val pruned = prune(expired, (1, 1))
    assert(elapsed.chain.height == 2 and elapsed.chain.workflows.get(1).status == "waiting" and
      prune(elapsed, (1, 1)) == elapsed and
      deliver(elapsed, (1, 1), 1, "success").chain == elapsed.chain and
      expired.chain.workflows.get(1).status == "failed" and
      expired.chain.pending == Set((1, 1)) and expired.chain.external == 1 and
      pruned.chain.pending == Set() and pruned.chain.seen == Set((1, 1)))
  }
  run remoteErrorCommitsTest = {
    val failed = deliver(waiting, (1, 1), 1, "error")
    assert(failed.chain.workflows.get(1).status == "failed" and
      failed.chain.pending == Set() and failed.chain.resolutions == 1)
  }
  run applicationFailureCommitsWriteTest = {
    val failed = deliver(waiting, (1, 1), 1, "empty")
    assert(failed.chain.paymentSeen.get(1) == 0 and
      failed.chain.workflows.get(1).status == "failed" and failed.chain.pending == Set())
  }
  run callbackAbortAndRetryTest = {
    val malformed = deliver(waiting, (1, 1), 1, "malformed")
    val oversized = deliver(waiting, (1, 1), 1, "oversized")
    val outbound = deliver(waiting, (1, 1), 1, "outbound_abort")
    val lowGas = deliver(waiting, (1, 1), 1, "gas_abort")
    val retried = deliver(outbound, (1, 1), 1, "success")
    assert(malformed.chain == waiting.chain and oversized.chain == waiting.chain and
      lowGas.chain == waiting.chain and outbound.chain == waiting.chain and
      outbound.aborts == 1 and retried.chain.paymentSeen.get(1) == 1 and
      retried.chain.workflows.get(1).seq == 2)
  }
  run emptyTrackingAcceptedTest = {
    val done = deliver(deliver(waiting, (1, 1), 1, "success"), (1, 2), 1, "empty")
    assert(done.chain.workflows.get(1).status == "completed" and
      done.chain.resolutions == 2 and done.chain.pending == Set() and safetyOf(done))
  }
  run reusedOrderDoesNotReuseWorkflowIdentityTest = {
    val two = startWorkflow(waiting, 1, 1, false, false)
    assert(two.chain.nextId == 3 and two.chain.pending == Set((1, 1), (2, 1)) and
      two.chain.workflows.get(1).order == two.chain.workflows.get(2).order)
  }
  run ambiguousDriverRestartNeverRebroadcastsTest = {
    val intent = prepare(waiting, 2, true)
    val uncertain = broadcast(intent, "ambiguous", true)
    val restored = restart(uncertain)
    assert(intent.journal == "intent" and uncertain.broadcasts == 1 and
      restored.chain == waiting.chain and restored.journal == "intent" and
      not(restored.ready) and broadcast(restored, "hash", true) == restored and
      prepare(restored, 2, true) == restored)
  }
  run crashBeforeBroadcastBlocksSafelyTest = {
    val restored = restart(prepare(waiting, 2, true))
    assert(restored.journal == "intent" and restored.broadcasts == 0 and
      broadcast(restored, "hash", true) == restored)
  }
  run hashedJournalRequiresConfirmedReceiptTest = {
    val hashed = broadcast(prepare(waiting, 2, true), "hash", true)
    val failed = reconcile(hashed, "confirmed_failure")
    assert(hashed.journal == "hashed" and
      reconcile(hashed, "unknown") == hashed and
      reconcile(hashed, "unindexed") == hashed and
      reconcile(hashed, "zero_height") == hashed and
      failed.journal == "empty" and failed.chain == waiting.chain and
      prepare(failed, 2, true).journal == "intent")
  }
  run driverScopeAndCheckTxTest = {
    val prepared = prepare(waiting, 2, true)
    assert(prepared.journal == "intent" and prepare(waiting, 2, false) == waiting and
      broadcast(prepared, "hash", false) == prepared and
      broadcast(prepared, "checktx_rejected", true).journal == "empty")
  }
  run boundBeginAndPermanentTombstoneTest = {
    val enqueued = beginRequest(initial, (1, 1), 9)
    val delivered = deliver(waiting, (1, 1), 1, "success")
    assert(enqueued.chain.pending == Set((1, 1)) and
      beginRequest(initial, (1, 1), 4) == initial and
      beginRequest(waiting, (1, 1), 9) == waiting and
      beginRequest(delivered, (1, 1), 9) == delivered and
      delivered.chain.seen == Set((1, 1), (1, 2)))
  }
  run driverRediscoveryChoosesExpireThenPruneTest = {
    val elapsed = advance(advance(waiting))
    val expired = expire(elapsed, 1, 1)
    assert(prepare(waiting, 2, true).command == "deliver" and
      prepare(elapsed, 2, true).command == "expire" and
      prepare(expired, 2, true).command == "prune")
  }
  run requestIdsOrderEachServiceQueueTest = {
    val two = startWorkflow(waiting, 1, 1, false, false)
    val shippedSecondFirst = deliver(two, (2, 1), 1, "success")
    val bothShipping = deliver(shippedSecondFirst, (1, 1), 1, "success")
    assert(bothShipping.chain.requestIds.get((1, 1)) == 1 and
      bothShipping.chain.requestIds.get((2, 1)) == 2 and
      bothShipping.chain.requestIds.get((2, 2)) == 1 and
      bothShipping.chain.requestIds.get((1, 2)) == 2 and
      prepare(bothShipping, 3, true).selected == (2, 2))
  }
}
