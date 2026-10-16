module native_batch_auction_profiles {
  import native_batch_auction.* from "./native_batch_auction"
  pure val prepared = {
    val c = commit(commit(commit(initial(3, 2), 1, 12), 2, 12), 3, 12)
    val a = reveal(at(c, C), 1, 2, 2, 3)
    val b = reveal(a, 2, 2, 1, 2)
    at(reveal(b, 3, 2, 1, 1), R)
  }
  pure def progress(count: int): State = 1.to(count).fold(prepared,
    (s, _) => advance(poll(s, false, false, false)))
  // All controls start from a reachable prefix; each changes one named guard.
  action weakProvisionalInit = state' = progress(4)
  action weakProvisionalStep = state' = claim(state, 1, false, true, false)
  action weakRepeatedInit = state' = claim(progress(6), 2, false, false, false)
  action weakRepeatedStep = state' = claim(state, 2, false, false, true)
  action weakExpiryInit = state' = at(progress(5), E)
  action weakExpiryStep = state' = poll(state, false, false, true)
  action weakRollbackInit = state' = progress(1)
  action weakRollbackStep = state' = poll(state, true, true, false)
  action weakFlagsInit = state' = claim(progress(6), 1, false, false, false)
  action weakFlagsStep = state' = restart(state, true)

  // Deterministic independent witnesses supplement the random profile.
  run settlementWitnessTest = assert(progress(6).book.phase == "Finalized" and safe(progress(6)))
  run abortWitnessTest = assert(abort(at(progress(2), E)).book.phase == "Aborted")
  run rollbackWitnessTest = assert(poll(progress(1), true, false, false).rolledBack)
  run restartWitnessTest = assert(restart(progress(2), false).restarted)
}
