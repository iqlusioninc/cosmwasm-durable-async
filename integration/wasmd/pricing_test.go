package async

import (
	"encoding/json"
	"os"
	"strconv"
	"testing"

	sdkmath "cosmossdk.io/math"
	storetypes "cosmossdk.io/store/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	"github.com/stretchr/testify/require"
)

// Literal shared expectations catch wrong rounding, adjustment direction and keys.
func TestBasePriceFixture(t *testing.T) {
	data, err := os.ReadFile("../../fixtures/native-async/v1/base-price.json")
	require.NoError(t, err)
	var fixture struct {
		Prices []struct {
			Name        string
			Base        string
			Used        uint64
			Target      uint64
			Floor       string
			Denominator uint64
			Next        string
		} `json:"prices"`
		BidComponents []struct {
			Bid string
			Key string
		} `json:"bid_components"`
	}
	require.NoError(t, json.Unmarshal(data, &fixture))
	require.NotEmpty(t, fixture.Prices)
	require.NotEmpty(t, fixture.BidComponents)
	for _, price := range fixture.Prices {
		t.Run(price.Name, func(t *testing.T) {
			next := nextBasePrice(sdkmath.LegacyMustNewDecFromStr(price.Base), price.Used, price.Target,
				sdkmath.LegacyMustNewDecFromStr(price.Floor), price.Denominator)
			require.True(t, next.Equal(sdkmath.LegacyMustNewDecFromStr(price.Next)), "got %s, want %s", next, price.Next)
		})
	}
	for _, bid := range fixture.BidComponents {
		t.Run("bid_"+bid.Bid, func(t *testing.T) {
			require.Equal(t, bid.Key, bidComponent(sdkmath.LegacyMustNewDecFromStr(bid.Bid)))
		})
	}
}

func priceEvent(t *testing.T, ctx sdk.Context, kind string) map[string]string {
	t.Helper()
	for _, event := range ctx.EventManager().Events() {
		if event.Type == kind {
			attrs := map[string]string{}
			for _, a := range event.Attributes {
				attrs[a.Key] = a.Value
			}
			return attrs
		}
	}
	t.Fatalf("missing %s event", kind)
	return nil
}

// Real execution saturation catches accidentally pricing host accounting, or
// skipping updates in idle blocks. The initial poll establishes the burn wait.
func TestBasePriceRisesAndDecays(t *testing.T) {
	h := newFaultHarness(t)
	h.k.Config.MaxPollGas = 160_000_000
	h.start(t, 160_000_000, 100)
	h.block(11)
	h.execute(t, map[string]any{"deliver": map[string]any{"task_id": 1, "wait_sequence": 1}}, 0)
	h.execute(t, map[string]any{"mode": map[string]any{"value": "out_of_gas"}}, 0)
	gas := h.block(12)
	event := priceEvent(t, h.ctx, "async_base_price")
	require.Equal(t, "0.001000000000000000", event["old"])
	require.Equal(t, "0.001015625000000000", event["new"])
	require.Equal(t, "90000000", event["used"])
	require.Equal(t, "80000000", event["target"])
	require.Equal(t, uint64(90_000_000), number(h.k.store(h.ctx), "last_poll_gas"))
	require.Equal(t, []byte(event["new"]), h.k.store(h.ctx).Get([]byte("base_price")))
	require.LessOrEqual(t, gas, h.k.Config.AsyncGas)
	h.block(13)
	event = priceEvent(t, h.ctx, "async_base_price")
	require.Equal(t, "0", event["used"])
	require.Equal(t, "0.001000000000000000", event["new"])
}

func TestFundingConfigReportsLivePrice(t *testing.T) {
	ctx, k, _ := setup(t)
	k.store(ctx).Set([]byte("base_price"), []byte("0.002"))
	setNumber(k.store(ctx), "last_poll_gas", 90_000_000)
	raw, err := k.Query(ctx, []byte(`{"funding_config":{}}`))
	require.NoError(t, err)
	require.JSONEq(t, `{"denom":"ugas","gas_price":"0.002000000000000000","min_base_price":"0.001000000000000000","max_gas_per_poll":100000000,"max_lifetime_blocks":10000,"target_gas":80000000,"price_grace_blocks":10,"last_block_poll_gas_used":90000000}`, string(raw))
}

// Fee collector and escrow deltas pin real charging at the stored block price.
func TestPollChargedAtBlockBasePrice(t *testing.T) {
	h := newFaultHarness(t)
	h.start(t, 80_000_000, 100)
	h.k.store(h.ctx).Set([]byte("base_price"), []byte("0.002"))
	before := h.task(t, 1)
	collector := authtypes.NewModuleAddress(authtypes.FeeCollectorName)
	collected := h.keepers.BankKeeper.GetBalance(h.ctx, collector, "ugas").Amount
	h.block(11)
	event := priceEvent(t, h.ctx, "async_poll")
	require.Equal(t, "0.002000000000000000", event["price"])
	execution, err := strconv.ParseUint(event["execution_gas"], 10, 64)
	require.NoError(t, err)
	host, err := strconv.ParseUint(event["charged_host_gas"], 10, 64)
	require.NoError(t, err)
	want := sdkmath.NewIntFromUint64((execution + host + 499) / 500)
	after := h.task(t, 1)
	require.Equal(t, want.String(), amount(after.Fees).Sub(amount(before.Fees)).String())
	require.Equal(t, want.String(), amount(before.Escrow).Sub(amount(after.Escrow)).String())
	require.Equal(t, want.String(), h.keepers.BankKeeper.GetBalance(h.ctx, collector, "ugas").Amount.Sub(collected).String())
	require.Equal(t, execution, number(h.k.store(h.ctx), "last_poll_gas"))
}

func TestLivePriceRegistrationFundingAndResume(t *testing.T) {
	ctx, k, keepers := setup(t)
	owner := registrationOwner(t, ctx, k)
	keepers.Faucet.Fund(ctx, owner, sdk.NewInt64Coin("ugas", 3_000_000))
	k.store(ctx).Set([]byte("base_price"), []byte("0.02"))
	r := registrationPolicy(owner)
	r.MaxGasPrice = "0.001" // Below live price is admitted if the attempt is funded.
	before := policySnapshot(ctx, k)
	balance := keepers.BankKeeper.GetAllBalances(ctx, owner)
	_, err := k.RegisterTask(ctx, owner, r)
	require.ErrorContains(t, err, "funding")
	require.Equal(t, before, policySnapshot(ctx, k))
	require.Equal(t, balance, keepers.BankKeeper.GetAllBalances(ctx, owner))
	r.Funds = sdk.NewInt64Coin("ugas", 2_000_100)
	id, err := k.RegisterTask(ctx, owner, r)
	require.NoError(t, err)
	raw, err := k.Query(ctx, marshal(map[string]any{"task_funding": map[string]any{"contract": owner.String(), "task_id": id}}))
	require.NoError(t, err)
	var funding map[string]any
	require.NoError(t, json.Unmarshal(raw, &funding))
	require.Equal(t, "2000000", funding["max_attempt_charge"])
	task, err := k.load(ctx, owner.String(), id)
	require.NoError(t, err)
	k.store(ctx).Delete(task.QueueKey)
	k.cleanContract(ctx, owner.String())
	task.QueueKey = nil
	task.Status = "paused"
	task.Pause = "trap"
	k.save(ctx, task)
	resume := marshal(map[string]any{"resume_task": map[string]any{"task_id": id, "actor": owner.String()}})
	require.Error(t, k.Management(ctx, owner.String(), resume))
	require.NoError(t, k.UpdateBudget(ctx, owner.String(), UpdateBudgetMsg{TaskID: id, Actor: owner.String(), Refund: owner.String(), GasPerPoll: r.GasPerPoll, MaxGasPrice: "0.02"}))
	require.NoError(t, k.Management(ctx, owner.String(), resume))
}

func TestIdlePriceFinalizesAfterMaintenanceExhaustion(t *testing.T) {
	ctx, k, _ := setup(t)
	k.Config.MaintenanceGas = 100_001 // 100k reserved; discretionary read exhausts 1 gas.
	k.store(ctx).Set([]byte("base_price"), []byte("0.008"))
	ctx = ctx.WithGasMeter(storetypes.NewInfiniteGasMeter()).WithEventManager(sdk.NewEventManager())
	ctx = k.ReserveOrdinaryGas(ctx, 400_000_000)
	ctx = k.EndBlock(ctx, 400_000_000)
	require.Equal(t, []byte("0.007000000000000000"), k.store(ctx.WithGasMeter(storetypes.NewInfiniteGasMeter())).Get([]byte("base_price")))
	require.Equal(t, "0", priceEvent(t, ctx, "async_base_price")["used"])
	require.Greater(t, ctx.BlockGasMeter().GasConsumed(), uint64(1), "mandatory storage costs must be charged")
	require.LessOrEqual(t, ctx.BlockGasMeter().GasConsumed(), k.Config.MaintenanceGas)
}

func TestConfigRejectsInvalidPricing(t *testing.T) {
	for name, mutate := range map[string]func(*Config){
		"zero_denominator": func(c *Config) { c.PriceAdjustDenominator = 0 },
		"nil_floor":        func(c *Config) { c.MinBasePrice = sdkmath.LegacyDec{} },
		"negative_floor":   func(c *Config) { c.MinBasePrice = sdkmath.LegacyMustNewDecFromStr("-1") },
		"wide_floor": func(c *Config) {
			c.MinBasePrice = sdkmath.LegacyMustNewDecFromStr("340282366920938463463.374607431768211456")
		},
		"small_maintenance": func(c *Config) { c.MaintenanceGas = 99_999 },
		"zero_target":       func(c *Config) { c.AsyncGas = c.MaintenanceGas + 1 },
	} {
		t.Run(name, func(t *testing.T) { c := testConfig(); mutate(&c); require.Error(t, c.Validate()) })
	}
	c := testConfig()
	c.MinBasePrice = sdkmath.LegacyZeroDec()
	require.NoError(t, c.Validate())
}

func TestSnapshotPriceStateDomain(t *testing.T) {
	h := newFaultHarness(t)
	require.NoError(t, h.k.InitState(h.ctx))
	for name, corrupt := range map[string]func(sdk.Context){
		"negative": func(c sdk.Context) { h.k.store(c).Set([]byte("base_price"), []byte("-1")) },
		"wide": func(c sdk.Context) {
			h.k.store(c).Set([]byte("base_price"), []byte("340282366920938463463.374607431768211456"))
		},
		"floor":     func(c sdk.Context) { h.k.store(c).Set([]byte("base_price"), []byte("0")) },
		"malformed": func(c sdk.Context) { h.k.store(c).Set([]byte("base_price"), []byte("NaN")) },
		"long": func(c sdk.Context) {
			h.k.store(c).Set([]byte("base_price"), []byte("0000000000000000000000000000000000000000000000000000001"))
		},
		"short_gas": func(c sdk.Context) { h.k.store(c).Set([]byte("last_poll_gas"), []byte{1}) },
		"wide_gas":  func(c sdk.Context) { setNumber(h.k.store(c), "last_poll_gas", h.k.Config.AsyncGas) },
		"old_schema": func(c sdk.Context) {
			h.k.store(c).Set([]byte(stateManifestKey), marshal(struct {
				Schema uint32 `json:"schema"`
				Config Config `json:"config"`
			}{1, h.k.Config}))
		},
	} {
		t.Run(name, func(t *testing.T) {
			c, _ := h.ctx.CacheContext()
			corrupt(c)
			require.NotPanics(t, func() { require.Error(t, h.k.ValidateState(c)) })
		})
	}
	h.k.store(h.ctx).Set([]byte("base_price"), []byte("0.002"))
	setNumber(h.k.store(h.ctx), "last_poll_gas", 90_000_000)
	require.NoError(t, h.k.ValidateState(h.ctx))
}

// The mandatory allowance must cover the widest stored Decimal even with no
// discretionary gas; a zero floor must still support registration and polls.
func TestPriceFinalizationBoundsAndZeroFloor(t *testing.T) {
	ctx, k, _ := setup(t)
	k.Config.MaintenanceGas = PriceFinalizationGas
	k.Config.MinBasePrice = sdkmath.LegacyMustNewDecFromStr("340282366920938463463.374607431768211455")
	ctx = k.ReserveOrdinaryGas(ctx, 400_000_000)
	ctx = k.EndBlock(ctx, 400_000_000)
	require.LessOrEqual(t, ctx.BlockGasMeter().GasConsumed(), PriceFinalizationGas)
	require.Equal(t, []byte(k.Config.MinBasePrice.String()), k.store(ctx).Get([]byte("base_price")))
	require.Equal(t, k.Config.MinBasePrice.String(), k.basePrice(ctx).String())

	h := newFaultHarness(t)
	h.k.Config.MinBasePrice = sdkmath.LegacyZeroDec()
	require.NoError(t, h.k.InitState(h.ctx))
	h.start(t, 80_000_000, 100)
	require.NoError(t, h.k.UpdateBudget(h.ctx, h.owner.String(), UpdateBudgetMsg{TaskID: 1, Actor: h.creator.String(), Refund: h.creator.String(), GasPerPoll: 80_000_000, MaxGasPrice: "0"}))
	h.block(11)
	require.Equal(t, "waiting", h.task(t, 1).Status)
	require.Equal(t, "0", h.task(t, 1).Fees)
	require.Equal(t, "0.000000000000000000", priceEvent(t, h.ctx, "async_poll")["price"])
	require.NoError(t, h.k.ValidateState(h.ctx))
}

func TestBasePriceLargeIntegerInputs(t *testing.T) {
	maxUint64 := ^uint64(0)
	maxPrice := sdkmath.LegacyMustNewDecFromStr("340282366920938463463.374607431768211455")
	require.True(t, nextBasePrice(maxPrice, maxUint64, 1, sdkmath.LegacyZeroDec(), 1).Equal(maxPrice))
	require.True(t, nextBasePrice(sdkmath.LegacyOneDec(), maxUint64, 1, sdkmath.LegacyZeroDec(), maxUint64).
		Equal(sdkmath.LegacyMustNewDecFromStr("1.999999999999999999")))
	require.True(t, nextBasePrice(sdkmath.LegacyOneDec(), 0, maxUint64, sdkmath.LegacyZeroDec(), 1).IsZero())
}

// Go's decimal type is wider and signed; pricing must enforce Rust's domain.
func TestPricingRejectsInvalidDomain(t *testing.T) {
	for _, value := range []sdkmath.LegacyDec{
		{},
		sdkmath.LegacyMustNewDecFromStr("-0.000000000000000001"),
		sdkmath.LegacyMustNewDecFromStr("340282366920938463463.374607431768211456"),
	} {
		require.Panics(t, func() { bidComponent(value) })
		require.Panics(t, func() { nextBasePrice(value, 1, 1, sdkmath.LegacyZeroDec(), 8) })
		require.Panics(t, func() { nextBasePrice(sdkmath.LegacyOneDec(), 1, 1, value, 8) })
	}
	require.Panics(t, func() { nextBasePrice(sdkmath.LegacyOneDec(), 0, 0, sdkmath.LegacyZeroDec(), 8) })
	require.Panics(t, func() { nextBasePrice(sdkmath.LegacyOneDec(), 0, 1, sdkmath.LegacyZeroDec(), 0) })
}
