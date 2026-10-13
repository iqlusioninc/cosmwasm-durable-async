// The bounded profile intentionally explores one registered task and a
// strict subset of the environment actions. It is not the full native model.
module native_profiles {
  import native_async.* from "./native_async"

  action tinyInit = state' = register(initial(true, 1), 1, 1, 1, 1)
  action fixedTinyInit = state' = register(initial(false, 1), 1, 1, 1, 1)
  action tinyStep = any {
    state' = begin(state),
    state' = maintain(state, 1),
    state' = inspect(state),
    state' = finish(state, "yield", 1, 0),
    state' = finish(state, "complete", 1, 0),
    state' = finish(state, "rollback", 1, 0),
    state' = finalize(state),
    state' = restore(state, checkpoint(state), true),
  }

  // Reachable fixture: register -> begin -> reserve cap 2 at price 1.
  // Depth 1 checks all finish outcomes and consumed gas 0/1/2, without spending
  // symbolic search on scheduler prefixes. It does not prove reachability of
  // arbitrary in-flight states or the full multi-block transition relation.
  action accountingInit = state' = inspect(begin(register(initial(true, 1), 1, 1, 2, 2)))
  action accountingStep = {
    nondet outcome = Set("yield", "operation", "timer", "complete", "appfail", "rollback").oneOf()
    nondet used = Set(0, 1, 2).oneOf()
    state' = finish(state, outcome, used, 2)
  }

  // N1 discrepancy control, source: runtime.rs reservation-only guard. This
  // deliberately violates intended semantics; NEVER used by tinyStep or step.
  action weakFlightInit = state' = inspect(begin(register(initial(true, 0), 1, 1, 0, 1)))
  action weakFlightStep = state' = control(state, 1, 1, "cancel", 0, true)

  // Omitted reconstruction: a valid durable checkpoint is loaded, but discarded
  // ready/due indexes are not rebuilt. The next block loses runnable work.
  // This is a deliberate model mutation, not an implementation vulnerability.
  action weakRestoreInit = state' = register(initial(true, 1), 1, 1, 1, 1)
  action weakRestoreStep = state' = restore(state, checkpoint(state), false)

  // Recorded witnesses avoid claiming that all scenario branches were visited
  // merely because a seeded random simulation completed.
  val sawPoll = state.polls > 0
  val sawAccepted = ids.exists(id => state.tasks.get(id).accepted > 0)
  val sawRollback = ids.exists(id => state.tasks.get(id).reason == "poll_failure")
}
