------------------------------ MODULE Ledger ------------------------------
EXTENDS Naturals, Integers, FiniteSets, TLC

(***************************************************************************)
(* One proposal-vault model for 03 §2.3, §5, §6 and §11.  Amounts are      *)
(* base units and scalar multiplication uses integer division, so TLC sees  *)
(* the claimant-adverse flooring required by 03 §6.3/§6.4.  The model      *)
(* proves I-3's exactly-once/authority/Open-only structure.  It assumes the *)
(* post-decision half at this interface: the proposal model discharges it   *)
(* through I-9/I-15 and the T17/T21/T22-only resolve paths (05 §2.1).       *)
(***************************************************************************)

CONSTANTS Holders, ResolveAuthority, SettleAuthority,
          MaxEscrow, MaxAmount, Amounts, ScoreScale, ScoreGrid

Branches == {"Accept", "Reject"}
Gates == {"Survival", "Security"}
ScalarKinds == {"Long", "Short"}
GateKinds == {"SurvivalYes", "SurvivalNo", "SecurityYes", "SecurityNo"}
Kinds == {"USDC"} \union ScalarKinds \union GateKinds
Positions == [branch : Branches, kind : Kinds]
Accounts == Holders \union {ResolveAuthority, SettleAuthority}
NoValue == "None"
NoScore == -1
VaultStates == {"Open", "Resolved", "ScalarSettled", "Voided"}
TerminalStates == {"ScalarSettled", "Voided"}
GateValues == {"Unset", "Yes", "No"}

OpNames == {
  "DecisionReady", "ArchiveDelayElapsed",
  "Split", "Merge", "SplitScalar", "MergeScalar",
  "SplitGate", "MergeGate", "Transfer", "Resolve", "Void",
  "SettleScalar", "SettleGate", "Redeem", "RedeemScalar",
  "RedeemScalarPair", "RedeemGate", "RedeemVoid", "SweepDust"
}

PublicOps == OpNames \ {"DecisionReady", "ArchiveDelayElapsed"}
AllowedInVoid == {"Merge", "MergeScalar", "MergeGate", "Transfer", "RedeemVoid"}
AllowedInArchivedVoid == AllowedInVoid \union {"SweepDust"}
OrdinaryRedemptions == {"Redeem", "RedeemScalar", "RedeemScalarPair", "RedeemGate"}

ASSUME /\ IsFiniteSet(Holders)
       /\ Cardinality(Holders) >= 1
       /\ ResolveAuthority \notin Holders
       /\ SettleAuthority \notin Holders
       /\ MaxEscrow \in Nat \ {0}
       /\ MaxAmount \in 1..MaxEscrow
       /\ Amounts \subseteq 1..MaxAmount
       /\ MaxAmount \in Amounts
       /\ ScoreScale \in Nat \ {0}
       /\ ScoreGrid \subseteq 0..ScoreScale
       /\ 0 \in ScoreGrid
       /\ ScoreScale \in ScoreGrid
       /\ \E s \in ScoreGrid \ {0, ScoreScale} :
              \E a \in Amounts : (a * s) % ScoreScale # 0

Pos(b, k) == [branch |-> b, kind |-> k]
YesKind(g) == IF g = "Survival" THEN "SurvivalYes" ELSE "SecurityYes"
NoKind(g) == IF g = "Survival" THEN "SurvivalNo" ELSE "SecurityNo"
KindGate(k) == IF k \in {"SurvivalYes", "SurvivalNo"}
               THEN "Survival"
               ELSE "Security"
MinNat(x, y) == IF x <= y THEN x ELSE y

RECURSIVE TotalOn(_, _, _)
TotalOn(h, people, p) ==
  IF people = {} THEN 0
  ELSE LET who == CHOOSE x \in people : TRUE
       IN h[who][p] + TotalOn(h, people \ {who}, p)

TotalIn(h, p) == TotalOn(h, Holders, p)
ScalarPairsIn(h, b) == MinNat(TotalIn(h, Pos(b, "Long")),
                              TotalIn(h, Pos(b, "Short")))
GatePairsIn(h, b, g) == MinNat(TotalIn(h, Pos(b, YesKind(g))),
                               TotalIn(h, Pos(b, NoKind(g))))

ZeroHoldings == [who \in Holders |-> [p \in Positions |-> 0]]
ZeroBranches == [b \in Branches |-> 0]
ZeroGates == [b \in Branches |-> [g \in Gates |-> 0]]

VARIABLES
  decisionReady,
  archiveReady,
  vaultState, winner, score, gateOutcome,
  escrow, escrowIn, cumulativePaidOut,
  holdings, usdcSupply, scalarMass, gateMass,
  voidEntryEscrow, voidPaidOut,
  resolveCount, hasLeftOpen,
  lastPreState, lastPreDecisionReady, lastPreArchiveReady, lastPreEscrow,
  lastOp, lastActor, lastKind,
  lastAmount, lastPayout, lastSucceeded

Supply(p) == TotalIn(holdings, p)

CoreVars == <<decisionReady, archiveReady,
              vaultState, winner, score, gateOutcome,
              escrow, escrowIn, cumulativePaidOut,
              holdings, usdcSupply, scalarMass, gateMass>>

AuditVars == <<voidEntryEscrow, voidPaidOut,
               resolveCount, hasLeftOpen,
               lastPreState, lastPreDecisionReady, lastPreArchiveReady,
               lastPreEscrow,
               lastOp, lastActor, lastKind,
               lastAmount, lastPayout, lastSucceeded>>

vars == <<CoreVars, AuditVars>>

(***************************************************************************)
(* TLC fingerprint view (state identity for dedup/expansion).  The last*   *)
(* audit variables are pure EDGE LABELS: no action guard or update reads   *)
(* them, so two states equal under this view have identical successor      *)
(* behavior — excluding them from the fingerprint loses no reachable core  *)
(* state or edge, and stops the label alphabet multiplying the explored    *)
(* space (~50-70x observed).  CAUTION: invariants OVER the labels are NOT  *)
(* reliably evaluated under this view — a Reject edge changes only labels, *)
(* so its successor deduplicates against its own source.  Label invariants *)
(* are therefore checked exclusively in the no-VIEW Audit/AuditQuarter and  *)
(* witness configs (see README, config-architecture section).  Cumulative  *)
(* audit state (voidPaidOut, voidEntryEscrow, resolveCount, hasLeftOpen)   *)
(* stays IN the view: invariants over it must see every reachable          *)
(* combination with the core state.                                        *)
(***************************************************************************)
FingerprintView ==
  <<decisionReady, archiveReady,
    vaultState, winner, score, gateOutcome,
    escrow, escrowIn, cumulativePaidOut,
    holdings, usdcSupply, scalarMass, gateMass,
    voidEntryEscrow, voidPaidOut, resolveCount, hasLeftOpen>>

Init ==
  /\ decisionReady = FALSE
  /\ archiveReady = FALSE
  /\ vaultState = "Open"
  /\ winner = NoValue
  /\ score = NoScore
  /\ gateOutcome = [g \in Gates |-> "Unset"]
  /\ escrow = 0
  /\ escrowIn = 0
  /\ cumulativePaidOut = 0
  /\ holdings = ZeroHoldings
  /\ usdcSupply = ZeroBranches
  /\ scalarMass = ZeroBranches
  /\ gateMass = ZeroGates
  /\ voidEntryEscrow = 0
  /\ voidPaidOut = 0
  /\ resolveCount = 0
  /\ hasLeftOpen = FALSE
  /\ lastPreState = "Open"
  /\ lastPreDecisionReady = FALSE
  /\ lastPreArchiveReady = FALSE
  /\ lastPreEscrow = 0
  /\ lastOp = "Init"
  /\ lastActor = NoValue
  /\ lastKind = NoValue
  /\ lastAmount = 0
  /\ lastPayout = 0
  /\ lastSucceeded = FALSE

(***************************************************************************)
(* Every failed dispatch is an explicit error transition.  The economic    *)
(* state stutters, but the rejected operation is recorded.  This is the    *)
(* non-vacuity mechanism for I-27: TLC explores both legal and illegal      *)
(* attempts rather than proving the call surface only from guards.          *)
(***************************************************************************)
Reject(op, actor, b, k, g, a) ==
  /\ UNCHANGED CoreVars
  /\ UNCHANGED <<voidEntryEscrow, voidPaidOut, resolveCount, hasLeftOpen>>
  /\ lastPreState' = vaultState
  /\ lastPreDecisionReady' = decisionReady
  /\ lastPreArchiveReady' = archiveReady
  /\ lastPreEscrow' = escrow
  /\ lastOp' = op
  /\ lastActor' = actor
  /\ lastKind' = IF k \in Kinds THEN k ELSE NoValue
  /\ lastAmount' = a
  /\ lastPayout' = 0
  /\ lastSucceeded' = FALSE

SuccessAudit(op, actor, b, k, g, a, payout) ==
  /\ voidEntryEscrow' = IF op = "Void" THEN escrow ELSE voidEntryEscrow
  /\ voidPaidOut' = IF op = "Void" THEN 0
                     ELSE IF vaultState = "Voided"
                          THEN voidPaidOut + payout
                          ELSE voidPaidOut
  /\ resolveCount' = resolveCount + IF op = "Resolve" THEN 1 ELSE 0
  /\ hasLeftOpen' = (hasLeftOpen
                     \/ (vaultState = "Open" /\ op \in {"Resolve", "Void"}))
  /\ lastPreState' = vaultState
  /\ lastPreDecisionReady' = decisionReady
  /\ lastPreArchiveReady' = archiveReady
  /\ lastPreEscrow' = escrow
  /\ lastOp' = op
  /\ lastActor' = actor
  /\ lastKind' = IF k \in Kinds THEN k ELSE NoValue
  /\ lastAmount' = a
  /\ lastPayout' = payout
  /\ lastSucceeded' = TRUE

(***************************************************************************)
(* Abstract interface inputs, not public ledger dispatches.  DecisionSignal *)
(* assumes the proposal-machine I-9/I-15 contract described above.  The     *)
(* archive signal compresses RedemptionArchiveDelay to one monotone event.  *)
(***************************************************************************)
DecisionSignal ==
  /\ vaultState = "Open"
  /\ ~decisionReady
  /\ decisionReady' = TRUE
  /\ UNCHANGED <<archiveReady, vaultState, winner, score, gateOutcome,
                  escrow, escrowIn, cumulativePaidOut,
                  holdings, usdcSupply, scalarMass, gateMass>>
  /\ SuccessAudit("DecisionReady", NoValue, NoValue, NoValue, NoValue, 0, 0)

ArchiveDelayElapsed ==
  /\ vaultState \in TerminalStates
  /\ ~archiveReady
  /\ archiveReady' = TRUE
  /\ UNCHANGED <<decisionReady, vaultState, winner, score, gateOutcome,
                  escrow, escrowIn, cumulativePaidOut,
                  holdings, usdcSupply, scalarMass, gateMass>>
  /\ SuccessAudit("ArchiveDelayElapsed", NoValue, NoValue, NoValue,
                    NoValue, 0, 0)

(***************************************************************************)
(* Minting, pair recovery and transfer: 03 §5.1, §6.1.                  *)
(***************************************************************************)
Split(who, a) ==
  IF vaultState = "Open" /\ escrowIn + a <= MaxEscrow
  THEN /\ escrow' = escrow + a
       /\ escrowIn' = escrowIn + a
       /\ UNCHANGED cumulativePaidOut
       /\ holdings' = [holdings EXCEPT
                          ![who][Pos("Accept", "USDC")] = @ + a,
                          ![who][Pos("Reject", "USDC")] = @ + a]
       /\ usdcSupply' = [b \in Branches |-> usdcSupply[b] + a]
       /\ UNCHANGED <<decisionReady, archiveReady, vaultState, winner, score, gateOutcome,
                       scalarMass, gateMass>>
       /\ SuccessAudit("Split", who, NoValue, "USDC", NoValue, a, 0)
  ELSE Reject("Split", who, NoValue, "USDC", NoValue, a)

Merge(who, a) ==
  IF vaultState \in {"Open", "Resolved", "Voided"}
     /\ holdings[who][Pos("Accept", "USDC")] >= a
     /\ holdings[who][Pos("Reject", "USDC")] >= a
     /\ escrow >= a
  THEN /\ escrow' = escrow - a
       /\ UNCHANGED escrowIn
       /\ cumulativePaidOut' = cumulativePaidOut + a
       /\ holdings' = [holdings EXCEPT
                          ![who][Pos("Accept", "USDC")] = @ - a,
                          ![who][Pos("Reject", "USDC")] = @ - a]
       /\ usdcSupply' = [b \in Branches |-> usdcSupply[b] - a]
       /\ UNCHANGED <<decisionReady, archiveReady, vaultState, winner, score, gateOutcome,
                       scalarMass, gateMass>>
       /\ SuccessAudit("Merge", who, NoValue, "USDC", NoValue, a, a)
  ELSE Reject("Merge", who, NoValue, "USDC", NoValue, a)

SplitScalar(who, b, a) ==
  IF vaultState = "Open" /\ holdings[who][Pos(b, "USDC")] >= a
  THEN /\ holdings' = [holdings EXCEPT
                          ![who][Pos(b, "USDC")] = @ - a,
                          ![who][Pos(b, "Long")] = @ + a,
                          ![who][Pos(b, "Short")] = @ + a]
       /\ usdcSupply' = [usdcSupply EXCEPT ![b] = @ - a]
       /\ scalarMass' = [scalarMass EXCEPT ![b] = @ + a]
       /\ UNCHANGED <<decisionReady, archiveReady, vaultState, winner, score, gateOutcome,
                       escrow, escrowIn, cumulativePaidOut, gateMass>>
       /\ SuccessAudit("SplitScalar", who, b, NoValue, NoValue, a, 0)
  ELSE Reject("SplitScalar", who, b, NoValue, NoValue, a)

MergeScalar(who, b, a) ==
  IF vaultState \in {"Open", "Resolved", "Voided"}
     /\ holdings[who][Pos(b, "Long")] >= a
     /\ holdings[who][Pos(b, "Short")] >= a
  THEN /\ holdings' = [holdings EXCEPT
                          ![who][Pos(b, "Long")] = @ - a,
                          ![who][Pos(b, "Short")] = @ - a,
                          ![who][Pos(b, "USDC")] = @ + a]
       /\ usdcSupply' = [usdcSupply EXCEPT ![b] = @ + a]
       /\ scalarMass' = [scalarMass EXCEPT ![b] = @ - a]
       /\ UNCHANGED <<decisionReady, archiveReady, vaultState, winner, score, gateOutcome,
                       escrow, escrowIn, cumulativePaidOut, gateMass>>
       /\ SuccessAudit("MergeScalar", who, b, NoValue, NoValue, a, 0)
  ELSE Reject("MergeScalar", who, b, NoValue, NoValue, a)

SplitGate(who, b, g, a) ==
  IF vaultState = "Open" /\ holdings[who][Pos(b, "USDC")] >= a
  THEN /\ holdings' = [holdings EXCEPT
                          ![who][Pos(b, "USDC")] = @ - a,
                          ![who][Pos(b, YesKind(g))] = @ + a,
                          ![who][Pos(b, NoKind(g))] = @ + a]
       /\ usdcSupply' = [usdcSupply EXCEPT ![b] = @ - a]
       /\ gateMass' = [gateMass EXCEPT ![b][g] = @ + a]
       /\ UNCHANGED <<decisionReady, archiveReady, vaultState, winner, score, gateOutcome,
                       escrow, escrowIn, cumulativePaidOut, scalarMass>>
       /\ SuccessAudit("SplitGate", who, b, NoValue, g, a, 0)
  ELSE Reject("SplitGate", who, b, NoValue, g, a)

MergeGate(who, b, g, a) ==
  IF vaultState \in {"Open", "Resolved", "Voided"}
     /\ holdings[who][Pos(b, YesKind(g))] >= a
     /\ holdings[who][Pos(b, NoKind(g))] >= a
  THEN /\ holdings' = [holdings EXCEPT
                          ![who][Pos(b, YesKind(g))] = @ - a,
                          ![who][Pos(b, NoKind(g))] = @ - a,
                          ![who][Pos(b, "USDC")] = @ + a]
       /\ usdcSupply' = [usdcSupply EXCEPT ![b] = @ + a]
       /\ gateMass' = [gateMass EXCEPT ![b][g] = @ - a]
       /\ UNCHANGED <<decisionReady, archiveReady, vaultState, winner, score, gateOutcome,
                       escrow, escrowIn, cumulativePaidOut, scalarMass>>
       /\ SuccessAudit("MergeGate", who, b, NoValue, g, a, 0)
  ELSE Reject("MergeGate", who, b, NoValue, g, a)

Transfer(who, to, p, a) ==
  IF vaultState \in {"Open", "Resolved", "Voided"}
     /\ who # to
     /\ holdings[who][p] >= a
  THEN /\ holdings' = [holdings EXCEPT
                          ![who][p] = @ - a,
                          ![to][p] = @ + a]
       /\ UNCHANGED <<decisionReady, archiveReady, vaultState, winner, score, gateOutcome,
                       escrow, escrowIn, cumulativePaidOut,
                       usdcSupply, scalarMass, gateMass>>
       /\ SuccessAudit("Transfer", who, p.branch, p.kind, NoValue, a, 0)
  ELSE Reject("Transfer", who, p.branch, p.kind, NoValue, a)

(***************************************************************************)
(* Authority transitions: 03 §2.3/§5.2.  Every modeled account attempts    *)
(* each authority call; only the configured authority can make it succeed.  *)
(***************************************************************************)
Resolve(actor, w) ==
  IF actor = ResolveAuthority /\ decisionReady /\ vaultState = "Open"
  THEN /\ vaultState' = "Resolved"
       /\ winner' = w
       /\ UNCHANGED <<decisionReady, archiveReady, score, gateOutcome,
                       escrow, escrowIn, cumulativePaidOut,
                       holdings, usdcSupply, scalarMass, gateMass>>
       /\ SuccessAudit("Resolve", actor, w, NoValue, NoValue, 0, 0)
  ELSE Reject("Resolve", actor, w, NoValue, NoValue, 0)

Void(actor) ==
  IF actor = ResolveAuthority /\ vaultState \in {"Open", "Resolved"}
  THEN /\ vaultState' = "Voided"
       /\ winner' = NoValue
       /\ score' = NoScore
       /\ UNCHANGED <<decisionReady, archiveReady, gateOutcome,
                       escrow, escrowIn, cumulativePaidOut,
                       holdings, usdcSupply, scalarMass, gateMass>>
       /\ SuccessAudit("Void", actor, NoValue, NoValue, NoValue, 0, 0)
  ELSE Reject("Void", actor, NoValue, NoValue, NoValue, 0)

SettleScalar(actor, s) ==
  IF actor = SettleAuthority /\ vaultState = "Resolved" /\ s \in ScoreGrid
  THEN /\ vaultState' = "ScalarSettled"
       /\ score' = s
       /\ UNCHANGED <<decisionReady, archiveReady, winner, gateOutcome,
                       escrow, escrowIn, cumulativePaidOut,
                       holdings, usdcSupply, scalarMass, gateMass>>
       /\ SuccessAudit("SettleScalar", actor, winner, NoValue, NoValue, 0, 0)
  ELSE Reject("SettleScalar", actor, NoValue, NoValue, NoValue, 0)

SettleGate(actor, g, outcome) ==
  IF actor = SettleAuthority
     /\ vaultState \in {"Resolved", "ScalarSettled"}
     /\ gateOutcome[g] = "Unset"
  THEN /\ gateOutcome' = [gateOutcome EXCEPT
                            ![g] = IF outcome THEN "Yes" ELSE "No"]
       /\ UNCHANGED <<decisionReady, archiveReady, vaultState, winner, score,
                       escrow, escrowIn, cumulativePaidOut,
                       holdings, usdcSupply, scalarMass, gateMass>>
       /\ SuccessAudit("SettleGate", actor, winner, NoValue, g, 0, 0)
  ELSE Reject("SettleGate", actor, NoValue, NoValue, g, 0)

(***************************************************************************)
(* Terminal redemptions: 03 §5.3, §6.3 and §6.4.                     *)
(***************************************************************************)
Redeem(who, a) ==
  IF vaultState = "ScalarSettled"
  THEN IF holdings[who][Pos(winner, "USDC")] >= a /\ escrow >= a
       THEN /\ holdings' = [holdings EXCEPT
                               ![who][Pos(winner, "USDC")] = @ - a]
            /\ usdcSupply' = [usdcSupply EXCEPT ![winner] = @ - a]
            /\ escrow' = escrow - a
            /\ cumulativePaidOut' = cumulativePaidOut + a
            /\ UNCHANGED <<decisionReady, archiveReady, vaultState, winner,
                            score, gateOutcome, escrowIn, scalarMass, gateMass>>
            /\ SuccessAudit("Redeem", who, winner, "USDC", NoValue, a, a)
       ELSE Reject("Redeem", who, NoValue, "USDC", NoValue, a)
  ELSE Reject("Redeem", who, NoValue, "USDC", NoValue, a)

ScalarPayout(k, a) ==
  IF k = "Long"
  THEN (a * score) \div ScoreScale
  ELSE (a * (ScoreScale - score)) \div ScoreScale

RedeemScalar(who, k, a) ==
  IF vaultState = "ScalarSettled"
  THEN LET payout == ScalarPayout(k, a)
       IN IF holdings[who][Pos(winner, k)] >= a /\ escrow >= payout
          THEN /\ holdings' = [holdings EXCEPT
                                  ![who][Pos(winner, k)] = @ - a]
               /\ scalarMass' = [scalarMass EXCEPT
                                    ![winner] = ScalarPairsIn(holdings', winner)]
               /\ escrow' = escrow - payout
               /\ cumulativePaidOut' = cumulativePaidOut + payout
               /\ UNCHANGED <<decisionReady, archiveReady, vaultState, winner,
                               score, gateOutcome, escrowIn, usdcSupply, gateMass>>
               /\ SuccessAudit("RedeemScalar", who, winner, k,
                                 NoValue, a, payout)
          ELSE Reject("RedeemScalar", who, NoValue, k, NoValue, a)
  ELSE Reject("RedeemScalar", who, NoValue, k, NoValue, a)

RedeemScalarPair(who, a) ==
  IF vaultState = "ScalarSettled"
  THEN IF holdings[who][Pos(winner, "Long")] >= a
          /\ holdings[who][Pos(winner, "Short")] >= a
          /\ escrow >= a
       THEN /\ holdings' = [holdings EXCEPT
                               ![who][Pos(winner, "Long")] = @ - a,
                               ![who][Pos(winner, "Short")] = @ - a]
            /\ scalarMass' = [scalarMass EXCEPT ![winner] = @ - a]
            /\ escrow' = escrow - a
            /\ cumulativePaidOut' = cumulativePaidOut + a
            /\ UNCHANGED <<decisionReady, archiveReady, vaultState, winner,
                            score, gateOutcome, escrowIn, usdcSupply, gateMass>>
            /\ SuccessAudit("RedeemScalarPair", who, winner,
                              NoValue, NoValue, a, a)
       ELSE Reject("RedeemScalarPair", who, NoValue, NoValue, NoValue, a)
  ELSE Reject("RedeemScalarPair", who, NoValue, NoValue, NoValue, a)

WinningGateKind(g) == IF gateOutcome[g] = "Yes" THEN YesKind(g) ELSE NoKind(g)

RedeemGate(who, g, a) ==
  IF vaultState = "ScalarSettled" /\ gateOutcome[g] # "Unset"
  THEN LET k == WinningGateKind(g)
       IN IF holdings[who][Pos(winner, k)] >= a /\ escrow >= a
          THEN /\ holdings' = [holdings EXCEPT
                                  ![who][Pos(winner, k)] = @ - a]
               /\ gateMass' = [gateMass EXCEPT
                                 ![winner][g] = GatePairsIn(holdings', winner, g)]
               /\ escrow' = escrow - a
               /\ cumulativePaidOut' = cumulativePaidOut + a
               /\ UNCHANGED <<decisionReady, archiveReady, vaultState, winner,
                               score, gateOutcome, escrowIn,
                               usdcSupply, scalarMass>>
               /\ SuccessAudit("RedeemGate", who, winner, k, g, a, a)
          ELSE Reject("RedeemGate", who, NoValue, NoValue, g, a)
  ELSE Reject("RedeemGate", who, NoValue, NoValue, g, a)

VoidPayout(k, a) == IF k = "USDC" THEN a \div 2 ELSE a \div 4

RedeemVoid(who, b, k, a) ==
  LET payout == VoidPayout(k, a)
      g == IF k \in GateKinds THEN KindGate(k) ELSE NoValue
  IN IF vaultState = "Voided"
        /\ holdings[who][Pos(b, k)] >= a
        /\ escrow >= payout
     THEN /\ holdings' = [holdings EXCEPT ![who][Pos(b, k)] = @ - a]
          /\ usdcSupply' = IF k = "USDC"
                            THEN [usdcSupply EXCEPT ![b] = @ - a]
                            ELSE usdcSupply
          /\ scalarMass' = IF k \in ScalarKinds
                            THEN [scalarMass EXCEPT ![b] = ScalarPairsIn(holdings', b)]
                            ELSE scalarMass
          /\ gateMass' = IF k \in GateKinds
                          THEN [gateMass EXCEPT
                                  ![b][g] = GatePairsIn(holdings', b, g)]
                          ELSE gateMass
          /\ escrow' = escrow - payout
          /\ cumulativePaidOut' = cumulativePaidOut + payout
          /\ UNCHANGED <<decisionReady, archiveReady, vaultState, winner, score,
                          gateOutcome, escrowIn>>
          /\ SuccessAudit("RedeemVoid", who, b, k, g, a, payout)
     ELSE Reject("RedeemVoid", who, b, k, g, a)

HasAnyClaim == \E who \in Holders : \E p \in Positions : holdings[who][p] > 0

SweepDust(who) ==
  IF vaultState \in TerminalStates /\ archiveReady
     /\ (escrow > 0 \/ HasAnyClaim)
  THEN /\ holdings' = ZeroHoldings
       /\ usdcSupply' = ZeroBranches
       /\ scalarMass' = ZeroBranches
       /\ gateMass' = ZeroGates
       /\ cumulativePaidOut' = cumulativePaidOut + escrow
       /\ escrow' = 0
       /\ UNCHANGED <<decisionReady, archiveReady, vaultState, winner, score,
                       gateOutcome, escrowIn>>
       /\ SuccessAudit("SweepDust", who, NoValue, NoValue, NoValue, 0, escrow)
  ELSE Reject("SweepDust", who, NoValue, NoValue, NoValue, 0)

Done == UNCHANGED vars

(***************************************************************************)
(* Every public call is attempted without a Next-level state, authority,    *)
(* holding, or amount prefilter.  Each action owns its success/error split,  *)
(* making the negative halves of I-27 and outflow monotonicity non-vacuous.  *)
(***************************************************************************)
Next ==
  \/ DecisionSignal
  \/ ArchiveDelayElapsed
  \/ \E who \in Holders, a \in Amounts : Split(who, a)
  \/ \E who \in Holders, a \in Amounts : Merge(who, a)
  \/ \E who \in Holders, b \in Branches, a \in Amounts :
       SplitScalar(who, b, a)
  \/ \E who \in Holders, b \in Branches, a \in Amounts :
       MergeScalar(who, b, a)
  \/ \E who \in Holders, b \in Branches, g \in Gates, a \in Amounts :
       SplitGate(who, b, g, a)
  \/ \E who \in Holders, b \in Branches, g \in Gates, a \in Amounts :
       MergeGate(who, b, g, a)
  \/ \E who \in Holders, to \in Holders, p \in Positions, a \in Amounts :
       Transfer(who, to, p, a)
  \/ \E actor \in Accounts, w \in Branches : Resolve(actor, w)
  \/ \E actor \in Accounts : Void(actor)
  \/ \E actor \in Accounts, s \in ScoreGrid : SettleScalar(actor, s)
  \/ \E actor \in Accounts, g \in Gates, outcome \in BOOLEAN :
       SettleGate(actor, g, outcome)
  \/ \E who \in Holders, a \in Amounts : Redeem(who, a)
  \/ \E who \in Holders, k \in ScalarKinds, a \in Amounts :
       RedeemScalar(who, k, a)
  \/ \E who \in Holders, a \in Amounts : RedeemScalarPair(who, a)
  \/ \E who \in Holders, g \in Gates, a \in Amounts : RedeemGate(who, g, a)
  \/ \E who \in Holders, b \in Branches, k \in Kinds, a \in Amounts :
       RedeemVoid(who, b, k, a)
  \/ \E who \in Holders : SweepDust(who)
  \/ Done

Spec == Init /\ [][Next]_vars

(***************************************************************************)
(* Invariants and history-backed action properties.                        *)
(***************************************************************************)
TypeOK ==
  /\ decisionReady \in BOOLEAN
  /\ archiveReady \in BOOLEAN
  /\ vaultState \in VaultStates
  /\ winner \in Branches \union {NoValue}
  /\ score \in ScoreGrid \union {NoScore}
  /\ gateOutcome \in [Gates -> GateValues]
  /\ escrow \in 0..MaxEscrow
  /\ escrowIn \in 0..MaxEscrow
  /\ cumulativePaidOut \in 0..MaxEscrow
  /\ holdings \in [Holders -> [Positions -> 0..MaxEscrow]]
  /\ usdcSupply \in [Branches -> 0..MaxEscrow]
  /\ scalarMass \in [Branches -> 0..MaxEscrow]
  /\ gateMass \in [Branches -> [Gates -> 0..MaxEscrow]]
  /\ voidEntryEscrow \in 0..MaxEscrow
  /\ voidPaidOut \in 0..MaxEscrow
  /\ resolveCount \in 0..1
  /\ hasLeftOpen \in BOOLEAN
  /\ lastPreState \in VaultStates
  /\ lastPreDecisionReady \in BOOLEAN
  /\ lastPreArchiveReady \in BOOLEAN
  /\ lastPreEscrow \in 0..MaxEscrow
  /\ lastOp \in OpNames \union {"Init"}
  /\ lastActor \in Accounts \union {NoValue}
  /\ lastKind \in Kinds \union {NoValue}
  /\ lastAmount \in 0..MaxAmount
  /\ lastPayout \in 0..MaxEscrow
  /\ lastSucceeded \in BOOLEAN

StateCoherence ==
  /\ CASE vaultState = "Open" -> winner = NoValue /\ score = NoScore
       [] vaultState = "Resolved" -> winner \in Branches /\ score = NoScore
       [] vaultState = "ScalarSettled" -> winner \in Branches /\ score \in ScoreGrid
       [] vaultState = "Voided" -> winner = NoValue /\ score = NoScore

TrackerOK ==
  /\ \A b \in Branches : usdcSupply[b] = Supply(Pos(b, "USDC"))
  /\ \A b \in Branches : scalarMass[b] = ScalarPairsIn(holdings, b)
  /\ \A b \in Branches : \A g \in Gates :
       gateMass[b][g] = GatePairsIn(holdings, b, g)

PairedSupplyOK ==
  \A b \in Branches :
    /\ Supply(Pos(b, "Long")) = scalarMass[b]
    /\ Supply(Pos(b, "Short")) = scalarMass[b]
    /\ \A g \in Gates :
         /\ Supply(Pos(b, YesKind(g))) = gateMass[b][g]
         /\ Supply(Pos(b, NoKind(g))) = gateMass[b][g]

BranchIdentity(b) ==
  escrow = usdcSupply[b] + scalarMass[b]
           + gateMass[b]["Survival"] + gateMass[b]["Security"]

ScalarLegLiability(b) ==
  LET longs == Supply(Pos(b, "Long"))
      shorts == Supply(Pos(b, "Short"))
      pairs == MinNat(longs, shorts)
  IN pairs
     + ((longs - pairs) * score) \div ScoreScale
     + ((shorts - pairs) * (ScoreScale - score)) \div ScoreScale

GateLiability(b, g) ==
  CASE gateOutcome[g] = "Yes" -> Supply(Pos(b, YesKind(g)))
    [] gateOutcome[g] = "No" -> Supply(Pos(b, NoKind(g)))
    [] OTHER -> MinNat(MaxEscrow,
                       IF Supply(Pos(b, YesKind(g))) >= Supply(Pos(b, NoKind(g)))
                       THEN Supply(Pos(b, YesKind(g)))
                       ELSE Supply(Pos(b, NoKind(g))))

ScalarSettledLiability ==
  usdcSupply[winner] + ScalarLegLiability(winner)
  + GateLiability(winner, "Survival") + GateLiability(winner, "Security")

EffectiveBranch(b) ==
  usdcSupply[b] + scalarMass[b]
  + gateMass[b]["Survival"] + gateMass[b]["Security"]

UnpairedLegLiability(b) ==
  (Supply(Pos(b, "Long")) - scalarMass[b]) \div 4
  + (Supply(Pos(b, "Short")) - scalarMass[b]) \div 4
  + (Supply(Pos(b, "SurvivalYes")) - gateMass[b]["Survival"]) \div 4
  + (Supply(Pos(b, "SurvivalNo")) - gateMass[b]["Survival"]) \div 4
  + (Supply(Pos(b, "SecurityYes")) - gateMass[b]["Security"]) \div 4
  + (Supply(Pos(b, "SecurityNo")) - gateMass[b]["Security"]) \div 4

VoidLiability ==
  LET ae == EffectiveBranch("Accept")
      re == EffectiveBranch("Reject")
      cross == MinNat(ae, re)
  IN cross + (ae - cross) \div 2 + (re - cross) \div 2
     + UnpairedLegLiability("Accept") + UnpairedLegLiability("Reject")

MaxRemainingPayout ==
  CASE vaultState \in {"Open", "Resolved"} -> escrow
    [] vaultState = "ScalarSettled" -> ScalarSettledLiability
    [] vaultState = "Voided" -> VoidLiability

LedgerIdentities ==
  /\ StateCoherence
  /\ TrackerOK
  /\ IF vaultState \in {"Open", "Resolved"}
        THEN PairedSupplyOK /\ \A b \in Branches : BranchIdentity(b)
        ELSE MaxRemainingPayout <= escrow

ConservationBound ==
  /\ MaxRemainingPayout <= escrow
  /\ cumulativePaidOut <= escrowIn
  /\ escrowIn = escrow + cumulativePaidOut

ResolveSafety ==
  /\ resolveCount <= 1
  /\ (lastSucceeded /\ lastOp = "Resolve") =>
       /\ lastPreState = "Open"
       /\ lastPreDecisionReady
       /\ lastActor = ResolveAuthority

AuthoritySafety ==
  /\ (lastSucceeded /\ lastOp = "Void") =>
       /\ lastPreState \in {"Open", "Resolved"}
       /\ lastActor = ResolveAuthority
  /\ (lastSucceeded /\ lastOp \in {"SettleScalar", "SettleGate"}) =>
       lastActor = SettleAuthority

SweepTiming ==
  /\ (lastSucceeded /\ lastOp = "SweepDust") =>
       /\ lastPreState \in TerminalStates
       /\ lastPreArchiveReady
       /\ lastPayout = lastPreEscrow
  /\ (lastOp = "SweepDust"
      /\ (~lastPreArchiveReady \/ lastPreState \notin TerminalStates)) =>
       ~lastSucceeded

VoidConservation ==
  /\ vaultState = "Voided" =>
       /\ voidPaidOut <= voidEntryEscrow
       /\ voidPaidOut + escrow = voidEntryEscrow
       /\ VoidLiability <= escrow
  /\ (lastSucceeded /\ lastPreState = "Voided" /\ lastOp = "Merge") =>
       lastPayout = lastAmount
  /\ (lastSucceeded /\ lastPreState = "Voided"
      /\ lastOp \in {"MergeScalar", "MergeGate", "Transfer"}) =>
       lastPayout = 0
  /\ (lastSucceeded /\ lastPreState = "Voided" /\ lastOp = "RedeemVoid") =>
       lastPayout = IF lastKind = "USDC"
                    THEN lastAmount \div 2
                    ELSE lastAmount \div 4

(***************************************************************************)
(* Pure-state projections of the composite invariants above.  The large    *)
(* VIEW-pruned scopes (Small/Full/Quarter) check ONLY these plus the       *)
(* RejectedEconomicStutter ACTION property: under the FingerprintView, a   *)
(* Reject edge's successor deduplicates against its own source, so         *)
(* invariants over the last* EDGE LABELS are not reliably evaluated there. *)
(* The label-dependent composites (ResolveSafety, AuthoritySafety,         *)
(* SweepTiming, VoidConservation, VoidedCallSurface, NoReopening,          *)
(* OutflowMonotonicity, PayoutExactness) are checked exhaustively in the   *)
(* no-VIEW Audit/AuditQuarter scopes and the witness configs instead.      *)
(***************************************************************************)
ResolveCountSafety == resolveCount <= 1

VoidAccounting ==
  vaultState = "Voided" =>
    /\ voidPaidOut <= voidEntryEscrow
    /\ voidPaidOut + escrow = voidEntryEscrow
    /\ VoidLiability <= escrow

NoReopeningState == hasLeftOpen => vaultState # "Open"

AllowedVoidOps(archived) ==
  IF archived THEN AllowedInArchivedVoid ELSE AllowedInVoid

VoidedCallSurface ==
  /\ (lastPreState = "Voided" /\ lastSucceeded /\ lastOp \in PublicOps) =>
       lastOp \in AllowedVoidOps(lastPreArchiveReady)
  /\ (lastPreState = "Voided"
      /\ lastOp \in (PublicOps \ AllowedVoidOps(lastPreArchiveReady))) =>
       ~lastSucceeded
  /\ (lastPreState = "Voided" /\ lastOp = "SweepDust" /\ lastSucceeded) =>
       lastPreArchiveReady

NoReopening ==
  /\ hasLeftOpen => vaultState # "Open"
  /\ (lastPreState = "ScalarSettled") => vaultState = "ScalarSettled"
  /\ (lastPreState = "Voided") => vaultState = "Voided"

OutflowMonotonicity ==
  /\ (lastPreState = "Resolved" /\ escrow < lastPreEscrow) =>
       /\ lastSucceeded
       /\ lastOp = "Merge"
       /\ lastPreEscrow - escrow = lastAmount
  /\ (lastPreState = "Resolved" /\ lastOp \in OrdinaryRedemptions) =>
       ~lastSucceeded

PayoutExactness ==
  /\ (lastSucceeded /\ lastOp \in {"Redeem", "RedeemScalarPair", "RedeemGate"}) =>
       lastPayout = lastAmount
  /\ (lastSucceeded /\ lastOp = "RedeemScalar" /\ lastKind = "Long") =>
       lastPayout = (lastAmount * score) \div ScoreScale
  /\ (lastSucceeded /\ lastOp = "RedeemScalar" /\ lastKind = "Short") =>
       lastPayout = (lastAmount * (ScoreScale - score)) \div ScoreScale

(***************************************************************************)
(* Action-level check: every explicit public-call error leaves the complete *)
(* economic state unchanged, while its real operation name is audited.      *)
(***************************************************************************)
RejectedStep ==
  (lastOp' \in PublicOps /\ ~lastSucceeded') => UNCHANGED CoreVars

RejectedEconomicStutter == [][RejectedStep]_vars

(***************************************************************************)
(* Reachability witnesses.  A witness config checks one of these negations  *)
(* as an invariant and must make TLC violate it (runner manifest contract).  *)
(***************************************************************************)
NoVoidedLiveUnpairedRedemption ==
  ~(lastSucceeded
    /\ lastPreState = "Voided"
    /\ lastOp = "RedeemVoid"
    /\ lastKind \in GateKinds
    /\ lastPayout > 0
    /\ HasAnyClaim)

NoNonzeroQuarterPayout ==
  ~(lastSucceeded
    /\ lastPreState = "Voided"
    /\ lastOp = "RedeemVoid"
    /\ lastKind \in ScalarKinds
    /\ lastAmount \in {4, 5}
    /\ lastPayout = lastAmount \div 4
    /\ lastPayout > 0)

NoRejectedResolvedRedeem ==
  ~(lastPreState = "Resolved" /\ lastOp = "Redeem" /\ ~lastSucceeded)

NoRejectedVoidedSplit ==
  ~(lastPreState = "Voided" /\ lastOp = "Split" /\ ~lastSucceeded)

HolderSymmetry == Permutations(Holders)

=============================================================================
