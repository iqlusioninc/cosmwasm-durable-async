package async

import (
	sdkmath "cosmossdk.io/math"
	"fmt"
	"math"
)

func (c Config) Validate() error {
	if c.AsyncGas == 0 || c.ModuleEndBlockGas == 0 || c.MaintenanceGas == 0 || c.MaintenanceGas >= c.AsyncGas {
		return fmt.Errorf("invalid async or module gas allocation")
	}
	if c.AsyncGas > math.MaxUint64-c.ModuleEndBlockGas || c.HostReserve < 1_000_000 || c.MaxPollGas <= c.HostReserve || c.MaxPollGas > c.AsyncGas-c.MaintenanceGas {
		return fmt.Errorf("gas slice cannot fit reserved allocation")
	}
	if c.MaxTasks == 0 || c.MaxBytes == 0 || c.MaxLifetime == 0 || c.MaxMessages == 0 || c.MaxVisits == 0 || c.MaxMaintenance == 0 || c.GasPrice.IsNil() || c.GasPrice.IsNegative() || c.StorageByteBlockPrice.IsNil() || c.StorageByteBlockPrice.IsNegative() || c.CleanupPrice.IsNil() || c.CleanupPrice.IsNegative() {
		return fmt.Errorf("invalid resource or economic configuration")
	}
	return nil
}
func (c Config) OrdinaryGas(total uint64) (uint64, error) {
	if e := c.Validate(); e != nil {
		return 0, e
	}
	reserved := c.AsyncGas + c.ModuleEndBlockGas
	if total <= reserved {
		return 0, fmt.Errorf("finite block gas must exceed async and EndBlock module reserves")
	}
	return total - reserved, nil
}

// ExperimentalConfig is for the reproducible local app harness only. These are
// deliberately explicit test parameters, not calibrated deployment economics.
func ExperimentalConfig() Config {
	return Config{Denom: "ugas", GasPrice: sdkmath.LegacyMustNewDecFromStr("0.001"), MaxPollGas: 100_000_000, AsyncGas: 200_000_000, MaintenanceGas: 40_000_000, HostReserve: 70_000_000, ModuleEndBlockGas: 20_000_000, MaxTasks: 100, MaxBytes: 10_000_000, MaxLifetime: 10_000, MaxMessages: 10, MaxVisits: 20, MaxMaintenance: 20, StorageByteBlockPrice: sdkmath.NewInt(1), CleanupPrice: sdkmath.NewInt(100)}
}
