package async

import sdkmath "cosmossdk.io/math"

// ExperimentalConfig is for the reproducible local app harness only. These are
// deliberately explicit test parameters, not calibrated deployment economics.
func ExperimentalConfig() Config {
	return Config{Denom: "ugas", GasPrice: sdkmath.LegacyMustNewDecFromStr("0.001"), MaxPollGas: 10_000_000, AsyncGas: 100_000_000, MaintenanceGas: 40_000_000, HostReserve: 20_000_000, MaxTasks: 100, MaxBytes: 10_000_000, MaxLifetime: 10_000, MaxMessages: 10, MaxVisits: 20, MaxMaintenance: 20, StorageByteBlockPrice: sdkmath.NewInt(1), CleanupPrice: sdkmath.NewInt(100)}
}
