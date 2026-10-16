module violation {
  var count: int
  action init = count' = 0
  action step = count' = count + 1
  val safety = count == 0
}
