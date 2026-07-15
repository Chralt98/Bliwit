//! `pallet-attestor` unit tests (15 §4.1): per-extrinsic × origin-misuse ×
//! error-path coverage, challenge/quorum boundaries, pallet storage bounds, and
//! try-state assertions.

use crate::mock::*;
use crate::{pallet::*, Error, Event};
use attestor_core::{ChallengeStatus, ATTESTOR_BOND, CHALLENGE_BOND, CHALLENGE_WINDOW_BLOCKS};
use frame_support::{assert_noop, assert_ok};
use sp_runtime::DispatchError;

fn hash(n: u8) -> futarchy_primitives::H256 {
    [n; 32]
}

fn attestor_events() -> Vec<Event<Test>> {
    frame_system::Pallet::<Test>::events()
        .into_iter()
        .filter_map(|record| match record.event {
            RuntimeEvent::Attestor(event) => Some(event),
            _ => None,
        })
        .collect()
}

fn attest_two(pid: u64, artifact_hash: futarchy_primitives::H256) {
    assert_ok!(Attestor::attest(
        RuntimeOrigin::signed(acct(1)),
        pid,
        artifact_hash,
        hash(11),
    ));
    assert_ok!(Attestor::attest(
        RuntimeOrigin::signed(acct(2)),
        pid,
        artifact_hash,
        hash(12),
    ));
}

#[test]
fn genesis_seeds_members_and_try_state_holds() {
    new_test_ext().execute_with(|| {
        let seeded = Members::<Test>::get();
        assert_eq!(seeded.len(), 3);
        assert!(seeded.iter().all(|member| {
            member.bond == ATTESTOR_BOND && member.false_count == 0 && member.active
        }));
        assert!(Attestations::<Test>::get().is_empty());
        assert_eq!(NextAttestationId::<Test>::get(), 0);
        assert_ok!(Attestor::do_try_state());
    });
}

#[test]
fn set_members_is_values_origin_only_and_validates_membership() {
    new_test_ext_empty().execute_with(|| {
        assert_noop!(
            Attestor::set_members(RuntimeOrigin::none(), members()),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::set_members(RuntimeOrigin::signed(acct(1)), members()),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::set_members(RuntimeOrigin::root(), members()),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::set_members(values_origin(), vec![acct(1), acct(2)]),
            Error::<Test>::TooFewMembers
        );

        let mut duplicate = members();
        duplicate[1] = duplicate[0].clone();
        assert_noop!(
            Attestor::set_members(values_origin(), duplicate),
            Error::<Test>::DuplicateMember
        );

        let oversized = (1..=17u8).map(acct).collect();
        assert_noop!(
            Attestor::set_members(values_origin(), oversized),
            Error::<Test>::TooManyAttestors
        );

        assert_ok!(Attestor::set_members(values_origin(), members()));
        assert_eq!(Members::<Test>::get().len(), 3);
        assert!(matches!(
            attestor_events().last(),
            Some(Event::MembersSet { members }) if members.len() == 3
        ));
        assert_ok!(Attestor::do_try_state());
    });
}

#[test]
fn attest_requires_initialization_signed_active_member_and_unique_assertion() {
    new_test_ext_empty().execute_with(|| {
        assert_noop!(
            Attestor::attest(RuntimeOrigin::signed(acct(1)), 7, hash(1), hash(2)),
            Error::<Test>::NotInitialized
        );
    });

    new_test_ext().execute_with(|| {
        assert_noop!(
            Attestor::attest(RuntimeOrigin::root(), 7, hash(1), hash(2)),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::attest(RuntimeOrigin::none(), 7, hash(1), hash(2)),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::attest(RuntimeOrigin::signed(acct(9)), 7, hash(1), hash(2)),
            Error::<Test>::NotMember
        );

        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            7,
            hash(1),
            hash(2),
        ));
        assert_eq!(Attestations::<Test>::get().len(), 1);
        assert_eq!(NextAttestationId::<Test>::get(), 1);
        assert!(matches!(
            attestor_events().last(),
            Some(Event::AttestationSubmitted {
                attestation_id: 0,
                pid: 7,
                artifact_hash,
                attestor,
            }) if *artifact_hash == hash(1) && *attestor == acct(1)
        ));
        assert_noop!(
            Attestor::attest(RuntimeOrigin::signed(acct(1)), 7, hash(1), hash(3)),
            Error::<Test>::DuplicateAttestation
        );
        assert_eq!(Attestations::<Test>::get().len(), 1);
        assert_eq!(NextAttestationId::<Test>::get(), 1);
    });
}

#[test]
fn two_distinct_attestors_form_quorum_only_after_window() {
    new_test_ext().execute_with(|| {
        set_block(10);
        attest_two(9, hash(7));

        assert!(!Attestor::has_quorum(9, hash(7)));
        set_block(u64::from(10 + CHALLENGE_WINDOW_BLOCKS));
        assert!(!Attestor::has_quorum(9, hash(7)));
        set_block(u64::from(11 + CHALLENGE_WINDOW_BLOCKS));
        assert!(Attestor::has_quorum(9, hash(7)));
        assert_eq!(Attestor::attestations_for(9).len(), 2);
        assert!(Attestor::open_challenges().is_empty());
    });
}

#[test]
fn one_or_rejected_attestation_never_forms_quorum() {
    new_test_ext().execute_with(|| {
        set_block(1);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            9,
            hash(7),
            hash(11),
        ));
        set_block(u64::from(CHALLENGE_WINDOW_BLOCKS + 2));
        assert!(!Attestor::has_quorum(9, hash(7)));

        set_block(1);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(2)),
            9,
            hash(7),
            hash(12),
        ));
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            0,
            hash(5),
            CHALLENGE_BOND,
        ));
        assert_ok!(Attestor::resolve_challenge(ratify_origin(), 0, false));
        set_block(u64::from(CHALLENGE_WINDOW_BLOCKS + 2));
        assert!(!Attestor::has_quorum(9, hash(7)));
    });
}

#[test]
fn open_challenge_suppresses_quorum_until_attestation_is_upheld() {
    new_test_ext().execute_with(|| {
        set_block(1);
        attest_two(9, hash(7));
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            0,
            hash(5),
            CHALLENGE_BOND,
        ));
        assert_eq!(Attestor::open_challenges().len(), 1);
        assert!(matches!(
            attestor_events().last(),
            Some(Event::AttestationChallenged {
                attestation_id: 0,
                challenger,
                evidence_hash,
            }) if *challenger == acct(9) && *evidence_hash == hash(5)
        ));

        set_block(u64::from(CHALLENGE_WINDOW_BLOCKS + 2));
        assert!(!Attestor::has_quorum(9, hash(7)));
        assert_ok!(Attestor::resolve_challenge(ratify_origin(), 0, true));
        assert!(Attestor::has_quorum(9, hash(7)));
        assert!(Attestor::open_challenges().is_empty());
        assert!(matches!(
            attestor_events().last(),
            Some(Event::ChallengeResolved {
                attestation_id: 0,
                upheld: true,
                loser,
                slashed,
            }) if *loser == acct(9) && *slashed == CHALLENGE_BOND / 2
        ));
        assert!(matches!(
            Attestations::<Test>::get()[0].challenge,
            Some(ChallengeStatus::Upheld)
        ));
    });
}

#[test]
fn challenge_is_signed_only_and_checks_id_bond_window_and_single_open_case() {
    new_test_ext_empty().execute_with(|| {
        assert_noop!(
            Attestor::challenge_attestation(
                RuntimeOrigin::signed(acct(9)),
                0,
                hash(3),
                CHALLENGE_BOND,
            ),
            Error::<Test>::NotInitialized
        );
    });

    new_test_ext().execute_with(|| {
        set_block(1);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            1,
            hash(1),
            hash(2),
        ));
        assert_noop!(
            Attestor::challenge_attestation(RuntimeOrigin::root(), 0, hash(3), CHALLENGE_BOND),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::challenge_attestation(RuntimeOrigin::none(), 0, hash(3), CHALLENGE_BOND),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::challenge_attestation(
                RuntimeOrigin::signed(acct(9)),
                99,
                hash(3),
                CHALLENGE_BOND,
            ),
            Error::<Test>::AttestationNotFound
        );
        assert_noop!(
            Attestor::challenge_attestation(
                RuntimeOrigin::signed(acct(9)),
                0,
                hash(3),
                CHALLENGE_BOND - 1,
            ),
            Error::<Test>::ChallengeBondTooSmall
        );

        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            0,
            hash(3),
            CHALLENGE_BOND,
        ));
        assert_noop!(
            Attestor::challenge_attestation(
                RuntimeOrigin::signed(acct(8)),
                0,
                hash(4),
                CHALLENGE_BOND,
            ),
            Error::<Test>::ChallengeAlreadyOpen
        );
    });

    new_test_ext().execute_with(|| {
        set_block(1);
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            1,
            hash(1),
            hash(2),
        ));
        set_block(u64::from(CHALLENGE_WINDOW_BLOCKS + 2));
        assert_noop!(
            Attestor::challenge_attestation(
                RuntimeOrigin::signed(acct(9)),
                0,
                hash(3),
                CHALLENGE_BOND,
            ),
            Error::<Test>::ChallengeWindowClosed
        );
    });
}

#[test]
fn resolve_challenge_is_ratify_origin_only_and_requires_an_open_challenge() {
    new_test_ext_empty().execute_with(|| {
        assert_noop!(
            Attestor::resolve_challenge(ratify_origin(), 0, true),
            Error::<Test>::NotInitialized
        );
    });

    new_test_ext().execute_with(|| {
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            1,
            hash(1),
            hash(2),
        ));
        assert_noop!(
            Attestor::resolve_challenge(RuntimeOrigin::signed(acct(1)), 0, true),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::resolve_challenge(RuntimeOrigin::root(), 0, true),
            DispatchError::BadOrigin
        );
        assert_noop!(
            Attestor::resolve_challenge(RuntimeOrigin::none(), 0, true),
            DispatchError::BadOrigin
        );
        // Attestation 0 exists but has no open challenge (precise error).
        assert_noop!(
            Attestor::resolve_challenge(ratify_origin(), 0, true),
            Error::<Test>::NoOpenChallenge
        );
        assert_ok!(Attestor::challenge_attestation(
            RuntimeOrigin::signed(acct(9)),
            0,
            hash(3),
            CHALLENGE_BOND,
        ));
        assert_ok!(Attestor::resolve_challenge(ratify_origin(), 0, true));
        // Re-resolving an already-resolved (Upheld) challenge: the attestation
        // exists but no longer carries an open challenge.
        assert_noop!(
            Attestor::resolve_challenge(ratify_origin(), 0, true),
            Error::<Test>::NoOpenChallenge
        );
    });
}

#[test]
fn two_false_attestations_slash_then_eject_the_attestor() {
    new_test_ext().execute_with(|| {
        for pid in 1..=2u64 {
            assert_ok!(Attestor::attest(
                RuntimeOrigin::signed(acct(1)),
                pid,
                hash(1),
                hash(2),
            ));
            let id = (pid - 1) as u32;
            assert_ok!(Attestor::challenge_attestation(
                RuntimeOrigin::signed(acct(9)),
                id,
                hash(3),
                CHALLENGE_BOND,
            ));
            assert_ok!(Attestor::resolve_challenge(ratify_origin(), id, false));
        }

        let core_account: futarchy_primitives::AccountId = acct(1).into();
        let member = Members::<Test>::get()
            .into_iter()
            .find(|member| member.account == core_account)
            .expect("seeded attestor remains represented after ejection");
        assert_eq!(member.bond, 0);
        assert_eq!(member.false_count, 2);
        assert!(!member.active);
        assert!(attestor_events().iter().any(|event| matches!(
            event,
            Event::AttestorEjected { who } if *who == acct(1)
        )));
        assert_noop!(
            Attestor::attest(RuntimeOrigin::signed(acct(1)), 3, hash(1), hash(2)),
            Error::<Test>::NotMember
        );
        assert_ok!(Attestor::do_try_state());
    });
}

#[test]
fn attestation_id_overflow_is_a_rejected_noop() {
    new_test_ext().execute_with(|| {
        NextAttestationId::<Test>::put(u32::MAX);
        assert_noop!(
            Attestor::attest(RuntimeOrigin::signed(acct(1)), 1, hash(1), hash(2)),
            Error::<Test>::Overflow
        );
        assert!(Attestations::<Test>::get().is_empty());
        assert_eq!(NextAttestationId::<Test>::get(), u32::MAX);
    });
}

#[test]
fn challenge_deadline_overflow_is_a_rejected_noop() {
    new_test_ext().execute_with(|| {
        set_block(u64::from(u32::MAX));
        assert_noop!(
            Attestor::attest(RuntimeOrigin::signed(acct(1)), 1, hash(1), hash(2)),
            Error::<Test>::Overflow
        );
        assert!(Attestations::<Test>::get().is_empty());
        assert_eq!(NextAttestationId::<Test>::get(), 0);
    });
}

#[test]
fn invalid_genesis_members_are_rejected() {
    let invalid_sets = [
        vec![acct(1), acct(2)],
        vec![acct(1), acct(1), acct(2)],
        (1..=17u8).map(acct).collect(),
    ];
    for invalid in invalid_sets {
        let result = std::panic::catch_unwind(move || {
            new_test_ext_with(crate::GenesisConfig::<Test> {
                members: invalid,
                _config: Default::default(),
            });
        });
        assert!(result.is_err());
    }
}

#[test]
fn recall_after_attest_is_valid_and_drops_recalled_attestor_from_quorum() {
    // 06 §7: recall is a first-class guardian-track action and `has_quorum`
    // counts only active members, so a lawful `set_members` recall that leaves a
    // now-non-member's historical attestation in the ledger MUST NOT trip
    // try_state (A10 spec-reviewer major; the over-strict core clause was
    // relaxed). The recalled attestor's attestation simply stops counting.
    new_test_ext().execute_with(|| {
        set_block(1);
        attest_two(9, hash(7));
        // Recall acct(1) through the real values-track `set_members` path.
        assert_ok!(Attestor::set_members(
            values_origin(),
            vec![acct(2), acct(3), acct(4)]
        ));
        assert_ok!(Attestor::do_try_state());
        // Only acct(2) remains an active attestor for (9, hash(7)); quorum needs
        // two distinct active attestors, so it is no longer met.
        set_block(u64::from(CHALLENGE_WINDOW_BLOCKS + 2));
        assert!(!Attestor::has_quorum(9, hash(7)));
    });
}

#[test]
fn resolve_without_open_challenge_is_a_precise_error() {
    new_test_ext().execute_with(|| {
        assert_ok!(Attestor::attest(
            RuntimeOrigin::signed(acct(1)),
            1,
            hash(1),
            hash(2),
        ));
        // Attestation 0 exists but carries no open challenge.
        assert_noop!(
            Attestor::resolve_challenge(ratify_origin(), 0, true),
            Error::<Test>::NoOpenChallenge
        );
        // A non-existent id is still a distinct AttestationNotFound.
        assert_noop!(
            Attestor::resolve_challenge(ratify_origin(), 99, true),
            Error::<Test>::AttestationNotFound
        );
    });
}

#[test]
fn attestation_storage_cap_rejects_without_mutating_existing_rows() {
    new_test_ext().execute_with(|| {
        for pid in 0..256u64 {
            assert_ok!(Attestor::attest(
                RuntimeOrigin::signed(acct(1)),
                pid,
                hash(1),
                hash(2),
            ));
        }
        assert_eq!(Attestations::<Test>::get().len(), 256);
        assert_eq!(NextAttestationId::<Test>::get(), 256);
        assert_noop!(
            Attestor::attest(RuntimeOrigin::signed(acct(1)), 256, hash(1), hash(2)),
            Error::<Test>::TooManyAttestations
        );
        assert_eq!(Attestations::<Test>::get().len(), 256);
        assert_eq!(NextAttestationId::<Test>::get(), 256);
    });
}
