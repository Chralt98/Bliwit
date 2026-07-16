//! Governed DOT/USDC execution pricing with per-message refunds (09 §6.1).

use core::marker::PhantomData;
use frame_support::weights::constants::{WEIGHT_PROOF_SIZE_PER_MB, WEIGHT_REF_TIME_PER_SECOND};
use sp_runtime::traits::Zero;
use staging_xcm::latest::{Asset, AssetId, Error as XcmError, Fungibility, Weight, XcmContext};
use staging_xcm_builder::TakeRevenue;
use staging_xcm_executor::{traits::WeightTrader, AssetsInHolding};

use crate::identity::{dot_location, usdc_location};

/// Governed asset units charged for the two independent weight dimensions (09 §6.1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WeightRate {
    /// Asset units per second of reference time.
    pub units_per_second: u128,
    /// Asset units per mebibyte of proof size.
    pub units_per_megabyte: u128,
}

/// Runtime seam bound to the constitution's current trader-rate parameters by B1a (09 §6.1).
pub trait TraderRates {
    fn dot_rate() -> WeightRate;
    fn usdc_rate() -> WeightRate;
}

/// A two-asset trader whose instance is one message's purchase/refund ledger.
///
/// Purchases round up against the payer. Refunds use the frozen rate selected
/// by the first successful buy and round down against the payer. Repeat buys
/// accumulate in the same asset; the unrefunded balance becomes revenue only
/// when the message's trader is dropped (the `UsingComponents` pattern).
pub struct GovernedWeightTrader<Rates, Revenue: TakeRevenue> {
    asset_id: Option<AssetId>,
    rate: Option<WeightRate>,
    total_weight_bought: Weight,
    total_paid: u128,
    paid_assets: AssetsInHolding,
    marker: PhantomData<(Rates, Revenue)>,
}

impl<Rates: TraderRates, Revenue: TakeRevenue> GovernedWeightTrader<Rates, Revenue> {
    fn component_price_up(value: u128, denominator: u128, rate: u128) -> Option<u128> {
        let whole = value / denominator;
        let remainder = value % denominator;
        let whole_price = whole.checked_mul(rate)?;
        let remainder_price = if remainder == 0 || rate == 0 {
            0
        } else {
            remainder
                .checked_mul(rate)?
                .checked_add(denominator.checked_sub(1)?)?
                / denominator
        };
        whole_price.checked_add(remainder_price)
    }

    fn component_price_down(value: u128, denominator: u128, rate: u128) -> Option<u128> {
        value.checked_mul(rate)?.checked_div(denominator)
    }

    fn price_up(weight: Weight, rate: WeightRate) -> Result<u128, XcmError> {
        let reference = Self::component_price_up(
            u128::from(weight.ref_time()),
            u128::from(WEIGHT_REF_TIME_PER_SECOND),
            rate.units_per_second,
        )
        .ok_or(XcmError::Overflow)?;
        let proof = Self::component_price_up(
            u128::from(weight.proof_size()),
            u128::from(WEIGHT_PROOF_SIZE_PER_MB),
            rate.units_per_megabyte,
        )
        .ok_or(XcmError::Overflow)?;
        reference.checked_add(proof).ok_or(XcmError::Overflow)
    }

    fn price_down(weight: Weight, rate: WeightRate) -> Option<u128> {
        let reference = Self::component_price_down(
            u128::from(weight.ref_time()),
            u128::from(WEIGHT_REF_TIME_PER_SECOND),
            rate.units_per_second,
        )?;
        let proof = Self::component_price_down(
            u128::from(weight.proof_size()),
            u128::from(WEIGHT_PROOF_SIZE_PER_MB),
            rate.units_per_megabyte,
        )?;
        reference.checked_add(proof)
    }

    fn rate_for(asset_id: &AssetId) -> Option<WeightRate> {
        if asset_id.0 == dot_location() {
            Some(Rates::dot_rate())
        } else if asset_id.0 == usdc_location() {
            Some(Rates::usdc_rate())
        } else {
            None
        }
    }

    fn quote_at(weight: Weight, asset_id: AssetId, rate: WeightRate) -> Result<Asset, XcmError> {
        if weight.is_zero() {
            return Err(XcmError::NoDeal);
        }
        let amount = Self::price_up(weight, rate)?;
        if amount == 0 {
            return Err(XcmError::NoDeal);
        }
        Ok((asset_id, amount).into())
    }

    fn quote(weight: Weight, asset_id: AssetId) -> Result<Asset, XcmError> {
        let rate = Self::rate_for(&asset_id).ok_or(XcmError::NotHoldingFees)?;
        Self::quote_at(weight, asset_id, rate)
    }
}

impl<Rates: TraderRates, Revenue: TakeRevenue> WeightTrader
    for GovernedWeightTrader<Rates, Revenue>
{
    fn new() -> Self {
        Self {
            asset_id: None,
            rate: None,
            total_weight_bought: Weight::zero(),
            total_paid: 0,
            paid_assets: AssetsInHolding::new(),
            marker: PhantomData,
        }
    }

    fn buy_weight(
        &mut self,
        weight: Weight,
        mut payment: AssetsInHolding,
        _context: &XcmContext,
    ) -> Result<AssetsInHolding, (AssetsInHolding, XcmError)> {
        let candidates = if let (Some(asset_id), Some(rate)) = (&self.asset_id, self.rate) {
            [(asset_id.clone(), Some(rate)), (asset_id.clone(), None)]
        } else {
            [
                (AssetId(dot_location()), Some(Rates::dot_rate())),
                (AssetId(usdc_location()), Some(Rates::usdc_rate())),
            ]
        };

        for (asset_id, maybe_rate) in candidates {
            let Some(rate) = maybe_rate else { continue };
            let required = match Self::quote_at(weight, asset_id.clone(), rate) {
                Ok(required) => required,
                Err(XcmError::NoDeal) => continue,
                Err(error) => return Err((payment, error)),
            };
            let amount = match &required.fun {
                Fungibility::Fungible(amount) => *amount,
                Fungibility::NonFungible(_) => return Err((payment, XcmError::NotHoldingFees)),
            };
            let Some(next_weight) = self.total_weight_bought.checked_add(&weight) else {
                return Err((payment, XcmError::Overflow));
            };
            let Some(next_paid) = self.total_paid.checked_add(amount) else {
                return Err((payment, XcmError::Overflow));
            };
            if let Ok(paid) = payment.try_take(required.into()) {
                self.asset_id = Some(asset_id);
                self.rate = Some(rate);
                self.total_weight_bought = next_weight;
                self.total_paid = next_paid;
                self.paid_assets.subsume_assets(paid);
                return Ok(payment);
            }
        }
        Err((payment, XcmError::TooExpensive))
    }

    fn refund_weight(&mut self, weight: Weight, _context: &XcmContext) -> Option<AssetsInHolding> {
        let asset_id = self.asset_id.clone()?;
        let rate = self.rate?;
        let refundable_weight = weight.min(self.total_weight_bought);
        if refundable_weight.is_zero() {
            return None;
        }

        // Refund pricing deliberately floors each dimension. Even where the
        // corresponding buy rounded up, a payer never receives more than the
        // exact governed price of unused weight (R-7 claimant-adverse rounding).
        let amount = Self::price_down(refundable_weight, rate)?.min(self.total_paid);
        self.total_weight_bought = self.total_weight_bought.saturating_sub(refundable_weight);
        if amount == 0 {
            return None;
        }

        let refund_asset: Asset = (asset_id, amount).into();
        let refunded = self.paid_assets.saturating_take(refund_asset.into());
        self.total_paid = self.total_paid.saturating_sub(amount);
        (!refunded.is_empty()).then_some(refunded)
    }

    fn quote_weight(
        &mut self,
        weight: Weight,
        given: AssetId,
        _context: &XcmContext,
    ) -> Result<Asset, XcmError> {
        if let (Some(selected), Some(rate)) = (&self.asset_id, self.rate) {
            if selected != &given {
                return Err(XcmError::NotHoldingFees);
            }
            Self::quote_at(weight, given, rate)
        } else {
            Self::quote(weight, given)
        }
    }
}

impl<Rates, Revenue: TakeRevenue> Drop for GovernedWeightTrader<Rates, Revenue> {
    fn drop(&mut self) {
        if !self.paid_assets.is_empty() {
            let mut revenue = AssetsInHolding::new();
            core::mem::swap(&mut revenue, &mut self.paid_assets);
            self.total_paid = 0;
            Revenue::take_revenue(revenue);
        }
    }
}
