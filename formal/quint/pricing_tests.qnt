module pricing_tests {
  import pricing.* from "./pricing"

  // Exact atomics from fixtures/native-async/v1/base-price.json, prices[];
  // each decimal string is converted at 10^18 atomics per whole unit.
  run fullTest = assert(nextPrice(1000000000000000000, 2000, 1000,
    1000000000000000, 8) == 1125000000000000000)
  run emptyTest = assert(nextPrice(1000000000000000000, 0, 1000,
    1000000000000000, 8) == 875000000000000000)
  run at_targetTest = assert(nextPrice(1000000000000000000, 1000, 1000,
    1000000000000000, 8) == 1000000000000000000)
  run floor_clampTest = assert(nextPrice(1000000000000000, 0, 1000,
    1000000000000000, 8) == 1000000000000000)
  run sub_atomic_stepTest = assert(nextPrice(3, 2000, 1000, 1, 8) == 3)
  run truncationTest = assert(nextPrice(1000000000000000000, 1001, 3000,
    1000000000000000, 8) == 916708333333333334)
  run saturateTest = assert(nextPrice(340282366920938463463374607431768211455,
    2000, 1000, 1000000000000000, 8) == 340282366920938463463374607431768211455)

  run charge_zero_gasTest = assert(attemptCharge(0, 1000000000000000000) == 0)
  run charge_zero_priceTest = assert(attemptCharge(18446744073709551615, 0) == 0)
  run charge_one_atomicTest = assert(attemptCharge(1, 1) == 1)
  run charge_one_unitTest = assert(attemptCharge(1, 1000000000000000000) == 1)
  run charge_round_upTest = assert(attemptCharge(3, 500000000000000001) == 2)
  run charge_max_priceTest = assert(attemptCharge(1,
    340282366920938463463374607431768211455) == 340282366920938463464)
  run charge_max_widthTest = assert(attemptCharge(18446744073709551615,
    340282366920938463463374607431768211455) ==
    6277101735386680763495507056286727952621)

  // Preconditions are caller guards, never substitute a numeric price for an
  // invalid configuration. These catch removed divisor/domain guards.
  run target_zero_rejectedTest = assert(not(nextPricePrecondition(1, 0, 0, 0, 8)))
  run denominator_zero_rejectedTest = assert(not(nextPricePrecondition(1, 0, 1, 0, 0)))
  run valid_price_domainTest = assert(nextPricePrecondition(1, 0, 1, 0, 8))
  run negative_base_rejectedTest = assert(not(nextPricePrecondition(-1, 0, 1, 0, 8)))
  run oversized_price_rejectedTest = assert(not(nextPricePrecondition(
    340282366920938463463374607431768211456, 0, 1, 0, 8)))
  run charge_overflow_rejectedTest = assert(not(attemptChargePrecondition(
    18446744073709551615, 340282366920938463463374607431768211455)))
  run charge_valid_domainTest = assert(attemptChargePrecondition(1,
    340282366920938463463374607431768211455))
  run negative_gas_rejectedTest = assert(not(attemptChargePrecondition(-1, 1)))
  run oversized_gas_rejectedTest = assert(not(attemptChargePrecondition(
    18446744073709551616, 1)))
  run negative_price_rejectedTest = assert(not(attemptChargePrecondition(1, -1)))
  run oversized_fee_price_rejectedTest = assert(not(attemptChargePrecondition(1,
    340282366920938463463374607431768211456)))
  run negative_floor_rejectedTest = assert(not(nextPricePrecondition(1, 0, 1, -1, 8)))
  run negative_used_rejectedTest = assert(not(nextPricePrecondition(1, -1, 1, 0, 8)))
  run oversized_used_rejectedTest = assert(not(nextPricePrecondition(1,
    18446744073709551616, 1, 0, 8)))
}
