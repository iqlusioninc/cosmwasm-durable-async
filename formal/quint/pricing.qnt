module pricing {
  pure val atomicsScale = 1000000000000000000
  pure val maxAtomics = 340282366920938463463374607431768211455
  pure val maxGas = 18446744073709551615

  pure def isPrice(price: int): bool = price >= 0 and price <= maxAtomics
  pure def isGas(gas: int): bool = gas >= 0 and gas <= maxGas

  // Caller guard: nextPrice is a partial integer operator on this domain.
  // assert is an action in Quint, so it cannot enforce a pure operator's domain.
  pure def nextPricePrecondition(base: int, used: int, target: int,
    floor: int, denominator: int): bool = and {
    isPrice(base), isPrice(floor), isGas(used),
    target > 0, target <= maxGas, denominator > 0, denominator <= maxGas,
  }

  pure def nextPrice(base: int, used: int, target: int,
    floor: int, denominator: int): int = {
    val delta = if (used > target) used - target else target - used
    // Retain Rust's wide multiplication and ordered truncating divisions.
    val magnitude = ((base * delta) / target) / denominator
    if (used > target) {
      val increased = base + magnitude
      if (increased > maxAtomics) maxAtomics else increased
    } else {
      val decreased = base - magnitude
      if (decreased < floor) floor else decreased
    }
  }

  // Mathematical ceiling, including wide results that Rust would reject.
  pure def attemptCharge(gas: int, price: int): int =
    (gas * price + atomicsScale - 1) / atomicsScale

  // Model state transitions must reject input/result overflow before charging.
  pure def attemptChargePrecondition(gas: int, price: int): bool = and {
    isGas(gas), isPrice(price), attemptCharge(gas, price) <= maxAtomics,
  }
}
