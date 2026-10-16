module pricing_negative {
  import pricing.* from "../pricing"
  // Deliberately one atomic too high. Kept outside the passing test suites.
  run oneAtomicWrongTest = assert(nextPrice(1000000000000000000, 1001, 3000,
    1000000000000000, 8) == 916708333333333335)
}
