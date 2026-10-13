module ibc_waits_tests {
  import ibc_waits.* from "./ibc_waits"
  pure val connected = connect(initial, true)
  pure val waiting = startWorkflow(connected, 1, 5, false, false)
  pure val p1 = packet(waiting, 1, 1)
  run channelBindingAndStartTest = assert(connected.chain.bound and
    connect(initial, false) == initial and startWorkflow(initial, 1, 5, false, false) == initial and
    startWorkflow(connected, 4, 5, false, false) == connected and
    startWorkflow(connected, 1, 5, true, false) == connected and
    startWorkflow(connected, 1, 5, false, true).chain == connected.chain and
    waiting.chain.pending == Set((1, 1)) and waiting.chain.commitments == Set(p1))
  run hostAuthenticationAndSelfSenderTest = assert(waiting.chain.workflows.get(1).status == "waiting" and
    callback(waiting, p1, "success", false, false) == waiting and
    publicDeliver(waiting, 1, 1, 4) == waiting and
    publicDeliver(waiting, 1, 2, 9) == waiting)
  run packetCorrelationRejectsWrongEndpointsDataTimeoutTest = assert(p1.data == 5 and
    adapterFixture(waiting, p1.with("src", 3), "success", false) == waiting and
    adapterFixture(waiting, p1.with("dest", 3), "success", false) == waiting and
    adapterFixture(waiting, p1.with("data", 6), "success", false) == waiting and
    adapterFixture(waiting, p1.with("timeout", 3), "success", false) == waiting)
  run packetSequenceIsHostNotAdapterCheckTest = {
    val altered = p1.with("packetSequence", 987654)
    val direct = adapterFixture(waiting, altered, "success", false)
    assert(callback(waiting, altered, "success", false, true) == waiting and
      direct.chain.workflows.get(1).seq == 2 and direct.chain.pending == Set((1, 2)))
  }
  run duplicateAckAndStaleSequenceTest = {
    val second = callback(waiting, p1, "success", false, true)
    val replayed = adapterFixture(second, p1, "success", false)
    val done = callback(second, packet(second, 1, 2), "success", false, true)
    assert(second.chain.workflows.get(1).seq == 2 and replayed == second and
      expire(second, 1, 1) == second and done.chain.workflows.get(1).status == "completed" and
      done.chain.workflows.get(1).output == 20 and done.chain.resolutions == 2 and safetyOf(done))
  }
  run callbackAbortRetainsHostAndContractStateTest = {
    val gas = callback(waiting, p1, "gas_abort", false, true)
    val outbound = callback(waiting, p1, "outbound_abort", false, true)
    val retry = callback(outbound, p1, "success", false, true)
    assert(gas.chain == waiting.chain and outbound.chain == waiting.chain and
      outbound.aborts == 1 and retry.chain.workflows.get(1).seq == 2 and
      retry.chain.commitments.size() == 1 and retry.chain.sends == 2)
  }
  run localDeadlineOverridesSuccessTest = {
    val elapsed = advance(advance(waiting, false), false)
    val failed = callback(elapsed, p1, "success", false, true)
    assert(elapsed.chain.workflows.get(1).status == "waiting" and
      failed.chain.workflows.get(1).status == "failed" and failed.chain.pending == Set())
  }
  run earlyRemoteTimeoutLeavesWaitingWithoutPendingTest = {
    val remoteExpired = advance(advance(waiting, true), true)
    val early = callback(remoteExpired, p1, "success", true, true)
    val local = advance(advance(early, false), false)
    val expired = expire(local, 1, 1)
    assert(early.chain.time == 0 and early.chain.remoteTime == 2 and
      early.chain.workflows.get(1).status == "waiting" and early.chain.pending == Set() and
      early.chain.commitments == Set() and expire(early, 1, 1) == early and
      adapterFixture(early, p1, "success", false) == early and
      expired.chain.workflows.get(1).status == "failed" and safetyOf(early))
  }
  run localExpiryDoesNotCancelRemotePacketTest = {
    val elapsed = advance(advance(waiting, false), false)
    val expired = expire(elapsed, 1, 1)
    val late = callback(expired, p1, "success", false, true)
    assert(expired.chain.pending == Set() and expired.chain.commitments == Set(p1) and
      late.chain.workflows.get(1).status == "failed" and late.chain.resolutions == 1 and
      late.chain.commitments == Set())
  }
  run remoteAndInvalidAckAreCommittedFailuresTest = {
    val remoteError = callback(waiting, p1, "error", false, true)
    val invalid = callback(waiting, p1, "invalid", false, true)
    assert(remoteError.chain.workflows.get(1).status == "failed" and
      invalid.chain.workflows.get(1).status == "failed" and invalid.chain.pending == Set())
  }
  run closureRetainsWaitAndRefusesNewSendsTest = {
    val closed = close(waiting, 1, true)
    val failed = callback(closed, p1, "success", false, true)
    assert(close(waiting, 2, true) == waiting and close(waiting, 1, false) == waiting and
      closed.chain.closed and closed.chain.workflows.get(1).status == "waiting" and
      connect(closed, true) == closed and startWorkflow(closed, 1, 5, false, false) == closed and
      failed.chain.workflows.get(1).status == "failed" and failed.chain.sends == 1)
  }
  run restartPreservesPendingAndNextSuccessorTest = {
    val restored = restart(waiting)
    val progressed = callback(restored, p1, "success", false, true)
    assert(restored.chain == waiting.chain and restored.restarts == 1 and
      progressed.chain.workflows.get(1).seq == 2 and progressed.chain.pending == Set((1, 2)))
  }
  run authenticatedPeerCanLieTest = {
    val second = callback(waiting, p1, "peer_lie", false, true)
    val done = callback(second, packet(second, 1, 2), "peer_lie", false, true)
    assert(done.chain.workflows.get(1).status == "completed" and
      done.chain.workflows.get(1).output == 7 and safetyOf(done))
  }
  run authenticatedTimeoutAtLocalDeadlineTest = {
    val local = advance(advance(waiting, false), false)
    val both = advance(advance(local, true), true)
    val failed = callback(both, p1, "success", true, true)
    assert(failed.chain.workflows.get(1).status == "failed" and
      failed.chain.pending == Set() and failed.chain.commitments == Set() and safetyOf(failed))
  }
}
