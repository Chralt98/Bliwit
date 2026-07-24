//! FRAME v2 benchmarks for every public market call and internal admin operation.

use crate::*;
use alloc::vec::Vec;
use frame_benchmarking::v2::*;
use frame_support::{
    traits::{fungibles::Mutate, ConstU32, EnsureOrigin, Get},
    BoundedVec,
};
use frame_system::RawOrigin;
use futarchy_primitives::{bounds, kernel, Balance, Branch, FixedU64, MarketId, ScalarSide};
use market_core::{BookKind, MarketPhase, TwapCumulative, TwapWindow};
use pallet_conditional_ledger::core_ledger::proposal_positions;
use sp_runtime::traits::Saturating;

const UNIT: Balance = 1_000_000;
const B: Balance = 1_000 * UNIT;
// Mid-range settlement score for terminal-latch fixtures (any admissible value).
const SETTLE_SCORE: FixedU64 = FixedU64(500_000_000);
// Keep the synthetic saturation range disjoint from compact mock-runtime ids.
// Production AccountId32 derivation remains canonical for the same ids.
const TRY_STATE_MARKET_ID_BASE: MarketId = 1 << 32;
// Generated benchmark accounts are not necessarily runtime protocol accounts;
// keep the fee leg above the ledger's live position-creation floor.
const TRADE: Balance = 10 * UNIT;

fn fund<T: Config>(who: &T::AccountId, amount: Balance) {
    <T::Collateral as Mutate<T::AccountId>>::mint_into(T::UsdcAssetId::get(), who, amount)
        .expect("benchmark collateral mint succeeds");
}

fn admin_origin<T: Config>() -> T::RuntimeOrigin {
    T::MarketAdmin::try_successful_origin().expect("benchmark MarketAdmin origin exists")
}

fn seeded_decision<T: Config>(market: MarketId) -> (T::AccountId, T::AccountId, T::AccountId) {
    let book = T::MarketAccounts::book(market);
    let fees = T::MarketAccounts::fees(market);
    let treasury: T::AccountId = account("treasury", market as u32, 0);
    fund::<T>(&book, 10_000 * UNIT);
    fund::<T>(&fees, 10_000 * UNIT);
    fund::<T>(&treasury, 10_000 * UNIT);
    Pallet::<T>::create_market(
        admin_origin::<T>(),
        market,
        BookKind::Decision {
            proposal: market,
            branch: Branch::Accept,
        },
        0,
        book.clone(),
        fees.clone(),
        B,
    )
    .expect("benchmark market creation succeeds");
    Pallet::<T>::seed(admin_origin::<T>(), market, treasury.clone())
        .expect("benchmark seeding succeeds");
    (book, fees, treasury)
}

#[benchmarks]
mod benchmarks {
    use super::*;

    #[benchmark]
    fn buy() {
        let caller: T::AccountId = whitelisted_caller();
        fund::<T>(&caller, 10_000 * UNIT);
        seeded_decision::<T>(1);
        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller.clone()),
            1,
            ScalarSide::Long,
            TRADE,
            Balance::MAX,
        );
        assert_eq!(Markets::<T>::get(1).expect("book exists").q_long, TRADE);
    }

    #[benchmark]
    fn sell() {
        let caller: T::AccountId = whitelisted_caller();
        fund::<T>(&caller, 10_000 * UNIT);
        seeded_decision::<T>(1);
        Pallet::<T>::buy(
            RawOrigin::Signed(caller.clone()).into(),
            1,
            ScalarSide::Long,
            TRADE,
            Balance::MAX,
        )
        .expect("benchmark buy succeeds");
        #[extrinsic_call]
        _(
            RawOrigin::Signed(caller.clone()),
            1,
            ScalarSide::Long,
            TRADE,
            0,
        );
        assert_eq!(Markets::<T>::get(1).expect("book exists").q_long, 0);
    }

    #[benchmark]
    fn crank_observe() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_decision::<T>(1);
        let now = frame_system::Pallet::<T>::block_number();
        frame_system::Pallet::<T>::set_block_number(now.saturating_add(100u32.into()));
        <T as Config>::BenchmarkHelper::prime_keeper_rebate();
        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 1);
        <T as Config>::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::General,
        );
        assert!(
            Markets::<T>::get(1)
                .expect("book exists")
                .last_observed_block
                > 0
        );
    }

    #[benchmark]
    fn reap() {
        let caller: T::AccountId = whitelisted_caller();
        seeded_decision::<T>(1);
        Pallet::<T>::close(admin_origin::<T>(), 1).expect("benchmark close succeeds");
        // The archive delay anchors at the ledger terminal marker, not ClosedAt
        // (04 §2): drive the shared vault to its scalar-settled terminal through
        // the production authorities and latch the market-side settlement
        // observation so the POL obligation is released before the book ages.
        let resolve_origin =
            <T as pallet_conditional_ledger::Config>::ResolveAuthority::try_successful_origin()
                .expect("benchmark resolve authority origin exists");
        pallet_conditional_ledger::Pallet::<T>::resolve(resolve_origin, 1, Branch::Accept)
            .expect("benchmark vault resolution succeeds");
        let settle_origin =
            <T as pallet_conditional_ledger::Config>::SettleAuthority::try_successful_origin()
                .expect("benchmark settle authority origin exists");
        pallet_conditional_ledger::Pallet::<T>::settle_scalar(settle_origin, 1, SETTLE_SCORE)
            .expect("benchmark vault settlement succeeds");
        Pallet::<T>::observe_proposal_terminal(1).expect("benchmark terminal observation succeeds");
        let market = Markets::<T>::get(1).expect("benchmark book exists");
        // Saturate the bounded protocol-inventory cleanup: two owners across all
        // 14 proposal instruments. These writes are setup, while the measured
        // reap must read and remove every cell plus its aggregate total.
        for id in proposal_positions(1) {
            pallet_conditional_ledger::Positions::<T>::insert(id, &market.account, 1);
            pallet_conditional_ledger::Positions::<T>::insert(id, &market.fees_account, 1);
            pallet_conditional_ledger::PositionTotals::<T>::insert(id, 2);
        }
        let now = frame_system::Pallet::<T>::block_number();
        frame_system::Pallet::<T>::set_block_number(
            now.saturating_add(<T as Config>::ArchiveDelay::get())
                .saturating_add(1u32.into()),
        );
        <T as Config>::BenchmarkHelper::prime_keeper_rebate();
        #[extrinsic_call]
        _(RawOrigin::Signed(caller.clone()), 1);
        <T as Config>::BenchmarkHelper::assert_keeper_rebate_paid(
            futarchy_primitives::keeper::CrankClass::General,
        );
        assert!(!Markets::<T>::contains_key(1));
    }

    #[benchmark]
    fn freeze_creation() -> Result<(), BenchmarkError> {
        let origin = <T as Config>::EmergencyPlaybookOrigin::try_successful_origin()
            .map_err(|_| BenchmarkError::Stop("EmergencyPlaybook origin unavailable"))?;
        let expiry = frame_system::Pallet::<T>::block_number()
            .saturating_add(kernel::MIN_EPOCH_LENGTH_BLOCKS.into());
        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, expiry);
        assert_eq!(CreationFrozenUntil::<T>::get(), Some(expiry));
        Ok(())
    }

    #[benchmark]
    fn set_frozen() -> Result<(), BenchmarkError> {
        let origin = <T as Config>::EmergencyPlaybookOrigin::try_successful_origin()
            .map_err(|_| BenchmarkError::Stop("EmergencyPlaybook origin unavailable"))?;
        #[extrinsic_call]
        _(origin as T::RuntimeOrigin, true);
        assert!(FrozenUntil::<T>::get().is_some());
        Ok(())
    }

    #[benchmark]
    fn create_market() {
        let book = T::MarketAccounts::book(1);
        let fees = T::MarketAccounts::fees(1);
        #[block]
        {
            Pallet::<T>::create_market(
                admin_origin::<T>(),
                1,
                BookKind::Decision {
                    proposal: 1,
                    branch: Branch::Accept,
                },
                0,
                book,
                fees,
                B,
            )
            .expect("benchmark market creation succeeds");
        }
        assert!(Markets::<T>::contains_key(1));
    }

    #[benchmark]
    fn seed() {
        let book = T::MarketAccounts::book(1);
        let fees = T::MarketAccounts::fees(1);
        let treasury: T::AccountId = account("treasury", 0, 0);
        fund::<T>(&book, 10_000 * UNIT);
        fund::<T>(&fees, 10_000 * UNIT);
        fund::<T>(&treasury, 10_000 * UNIT);
        Pallet::<T>::create_market(
            admin_origin::<T>(),
            1,
            BookKind::Decision {
                proposal: 1,
                branch: Branch::Accept,
            },
            0,
            book,
            fees,
            B,
        )
        .expect("benchmark market creation succeeds");
        #[block]
        {
            Pallet::<T>::seed(admin_origin::<T>(), 1, treasury)
                .expect("benchmark seeding succeeds");
        }
    }

    #[benchmark]
    fn close() {
        seeded_decision::<T>(1);
        #[block]
        {
            Pallet::<T>::close(admin_origin::<T>(), 1).expect("benchmark close succeeds");
        }
        assert!(matches!(
            Markets::<T>::get(1).expect("book exists").phase,
            MarketPhase::Closed
        ));
    }

    #[benchmark]
    fn try_state() {
        seeded_decision::<T>(1);
        frame_system::Pallet::<T>::set_block_number(10_000_u32.into());
        let now = frame_system::Pallet::<T>::block_number();
        let vault_template = pallet_conditional_ledger::Vaults::<T>::get(1)
            .expect("benchmark proposal vault exists");
        let mut template = Markets::<T>::get(1).expect("benchmark book exists");
        template.phase = MarketPhase::Closed;
        Markets::<T>::insert(1, template.clone());
        ClosedAt::<T>::insert(1, now);
        SettlementObservedAt::<T>::insert(1, now);
        pallet_conditional_ledger::VaultTerminalAt::<T>::insert(1, now);
        ActiveMarketCount::<T>::put(0);
        LivePolCommitments::<T>::kill();
        pallet_conditional_ledger::Vaults::<T>::remove(1);
        RerunSeededMarkets::<T>::insert(1, ());

        // `try_state` has no dispatch parameter, so its benchmark fixture must
        // itself saturate every bounded map it scans. It retains 2,240 books and
        // 4,480 distinct ownership-index accounts, while 196 active books carry
        // full checkpoint/window/owner vectors (including the bounded quadratic
        // duplicate-owner check), seed/rerun markers and the full POL vector.
        // The remaining books are seeded/rerun terminal archives, maximizing
        // both unbounded-map scans under their Markets-derived bound.
        let mut commitments =
            BoundedVec::<(MarketId, Balance), ConstU32<{ bounds::MAX_LIVE_MARKETS }>>::default();
        for offset in 0..u64::from(bounds::MAX_STORED_MARKETS).saturating_sub(1) {
            let id = TRY_STATE_MARKET_ID_BASE.saturating_add(offset);
            let book_account = T::MarketAccounts::book(id);
            let fees_account = T::MarketAccounts::fees(id);
            let mut book = template.clone();
            book.id = id;
            book.kind = BookKind::Decision {
                proposal: id,
                branch: Branch::Accept,
            };
            book.account = book_account.clone();
            book.fees_account = fees_account.clone();
            let active = offset < u64::from(bounds::MAX_LIVE_MARKETS);
            if active {
                book.phase = MarketPhase::Trading;
            }
            Markets::<T>::insert(id, book);
            MarketProtocolAccounts::<T>::insert(book_account, 1);
            MarketProtocolAccounts::<T>::insert(fees_account, 1);
            ProposalMarketIds::<T>::try_mutate(id, |ids| {
                ids.try_push(id).map_err(|_| "proposal market id fits")
            })
            .expect("one market id fits the proposal bound");
            SeededMarkets::<T>::insert(id, ());
            RerunSeededMarkets::<T>::insert(id, ());
            if active {
                pallet_conditional_ledger::Vaults::<T>::insert(id, vault_template);
                let original_b = template.b.checked_div(2).expect("benchmark b is even");
                let commitment = market_core::seed_headroom(original_b)
                    .expect("benchmark b is in the LMSR domain")
                    .checked_mul(2)
                    .expect("rerun commitment fits Balance");
                commitments
                    .try_push((id, commitment))
                    .expect("active commitment fits the live bound");
                let windows: Vec<_> = (0..bounds::MAX_TWAP_WINDOWS_PER_MARKET)
                    .map(|window| {
                        let start = window.saturating_mul(3).saturating_add(1);
                        TwapWindow {
                            start,
                            trailing_start: start.saturating_add(1),
                            end: start.saturating_add(2),
                            observations: 0,
                            stale_events: 0,
                            contest_capital_blocks: 0,
                            contest_accrued_until: start.saturating_add(2),
                            contest_valid: true,
                            close_spot: Some(FixedU64(500_000_000)),
                            sealed: true,
                        }
                    })
                    .collect();
                let checkpoints: Vec<_> = windows
                    .iter()
                    .map(|window| (window.end, TwapCumulative::ZERO))
                    .collect();
                let owners: Vec<_> = (0..bounds::MAX_LIVE_PROPOSALS)
                    .flat_map(|owner| {
                        windows.iter().map(move |window| {
                            (
                                u64::from(owner),
                                window.start,
                                window.trailing_start,
                                window.end,
                            )
                        })
                    })
                    .collect();
                TwapCheckpoints::<T>::insert(id, BoundedVec::truncate_from(checkpoints));
                DecisionWindows::<T>::insert(id, BoundedVec::truncate_from(windows));
                DecisionWindowOwners::<T>::insert(id, BoundedVec::truncate_from(owners));
            } else {
                ClosedAt::<T>::insert(id, now);
                SettlementObservedAt::<T>::insert(id, now);
                pallet_conditional_ledger::VaultTerminalAt::<T>::insert(id, now);
            }
        }
        LivePolCommitments::<T>::put(commitments);
        ActiveMarketCount::<T>::put(bounds::MAX_LIVE_MARKETS);
        T::PolCommitmentSync::sync_pol_commitments()
            .expect("benchmark POL mirror accepts the saturated commitment set");
        assert_eq!(Markets::<T>::count(), bounds::MAX_STORED_MARKETS);
        assert_eq!(
            MarketProtocolAccounts::<T>::count(),
            bounds::MAX_STORED_MARKETS.saturating_mul(2),
        );
        assert_eq!(
            SeededMarkets::<T>::iter_keys().count(),
            bounds::MAX_STORED_MARKETS as usize
        );
        assert_eq!(
            RerunSeededMarkets::<T>::iter_keys().count(),
            bounds::MAX_STORED_MARKETS as usize,
        );
        assert_eq!(
            LivePolCommitments::<T>::get().len(),
            bounds::MAX_LIVE_MARKETS as usize,
        );
        #[block]
        {
            Pallet::<T>::do_try_state().expect("benchmark try-state succeeds");
        }
    }

    impl_benchmark_test_suite!(Pallet, crate::mock::new_test_ext(), crate::mock::Test);
}
