// Copyright 2017 Parity Technologies (UK) Ltd.
// This file is part of Substrate Demo.

// Substrate Demo is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate Demo is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate Demo.  If not, see <http://www.gnu.org/licenses/>.

//! Council system: Handles the voting in and maintenance of council members.

use rstd::prelude::*;
use codec::KeyedVec;
use runtime_support::storage;
use demo_primitives::{AccountId, Hash, BlockNumber};
use runtime::{staking, system, session};
use runtime::democracy::PrivPass;
use runtime::staking::{PublicPass, Balance};

// no polynomial attacks:
//
// all unbonded public operations should be constant time.
// all other public operations must be linear time in terms of prior public operations and:
// - those "valid" ones that cost nothing be limited to a constant number per single protected operation
// - the rest costing the same order as the computational complexity
// all protected operations must complete in at most O(public operations)
//
// we assume "beneficial" transactions will have the same access as attack transactions.
//
// any storage requirements should be bonded by the same order as the volume.

// public operations:
// - express approvals (you pay in a "voter" bond the first time you do this; O(1); one extra DB entry, one DB change)
// - remove active voter (you get your "voter" bond back; O(1); one fewer DB entry, one DB change)
// - remove inactive voter (either you or the target is removed; if the target, you get their "voter" bond back; O(1); one fewer DB entry, one DB change)
// - submit candidacy (you pay a "candidate" bond; O(1); one extra DB entry, two DB changes)
// - present winner/runner-up (you may pay a "presentation" bond of O(voters) if the presentation is invalid; O(voters) compute; )
// protected operations:
// - remove candidacy (remove all votes for a candidate) (one fewer DB entry, two DB changes)

// to avoid a potentially problematic case of not-enough approvals prior to voting causing a
// back-to-back votes that have no way of ending, then there's a forced grace period between votes.
// to keep the system as stateless as possible (making it a bit easier to reason about), we just
// restrict when votes can begin to blocks that lie on boundaries (`voting_period`).

// for an approval vote of C councilers:

// top K runners-up are maintained between votes. all others are discarded.
// - candidate removed & bond returned when elected.
// - candidate removed & bond burned when discarded.

// at the point that the vote ends (), all voters' balances are snapshotted.

// for B blocks following, there's a counting period whereby each of the candidates that believe
// they fall in the top K+C voted can present themselves. they get the total stake
// recorded (based on the snapshot); an ordered list is maintained (the leaderboard). Noone may
// present themselves that, if elected, would result in being included twice on the council
// (important since existing councilers will will have their approval votes as it may be that they
// don't get removed), nor if existing presenters would mean they're not in the top K+C.

// following B blocks, the top C candidates are elected and have their bond returned. the top C
// candidates and all other candidates beyond the top C+K are cleared.

// vote-clearing happens lazily; for an approval to count, the most recent vote at the time of the
// voter's most recent vote must be no later than the most recent vote at the time that the
// candidate in the approval position was registered there. as candidates are removed from the
// register and others join in their place, this prevent an approval meant for an earlier candidate
// being used to elect a new candidate.

// the candidate list increases as needed, but the contents (though not really the capacity) reduce
// after each vote as all but K entries are cleared. newly registering candidates must use cleared
// entries before they increase the capacity.

pub type VoteIndex = u32;

// parameters
pub const CANDIDACY_BOND: &[u8] = b"cou:cbo";
pub const VOTING_BOND: &[u8] = b"cou:vbo";
pub const PRESENT_SLASH_PER_VOTER: &[u8] = b"cou:pss";
pub const CARRY_COUNT: &[u8] = b"cou:cco";
pub const PRESENTATION_DURATION: &[u8] = b"cou:pdu";
pub const INACTIVE_GRACE_PERIOD: &[u8] = b"cou:vgp";
pub const VOTING_PERIOD: &[u8] = b"cou:per";
pub const TERM_DURATION: &[u8] = b"cou:trm";
pub const DESIRED_SEATS: &[u8] = b"cou:sts";

// permanent state (always relevant, changes only at the finalisation of voting)
pub const ACTIVE_COUNCIL: &[u8] = b"cou:act";		// Vec<(AccountId, expiry: BlockNumber)>
pub const VOTE_COUNT: &[u8] = b"cou:vco";			// VoteIndex

// persistent state (always relevant, changes constantly)
pub const APPROVALS_OF: &[u8] = b"cou:apr:";		// Vec<bool>
pub const REGISTER_INFO_OF: &[u8] = b"cou:reg:";	// Candidate -> (VoteIndex, u32)
pub const LAST_ACTIVE_OF: &[u8] = b"cou:lac:";		// Voter -> VoteIndex
pub const VOTERS: &[u8] = b"cou:vrs";				// Vec<AccountId>
pub const CANDIDATES: &[u8] = b"cou:can";			// Vec<AccountId>, has holes
pub const CANDIDATE_COUNT: &[u8] = b"cou:cnc";		// u32

// temporary state (only relevant during finalisation/presentation)
pub const NEXT_FINALISE: &[u8] = b"cou:nxt";
pub const SNAPSHOTED_STAKES: &[u8] = b"cou:sss";		// Vec<Balance>
pub const LEADERBOARD: &[u8] = b"cou:win";				// Vec<(Balance, AccountId)> ORDERED low -> high

/// How much should be locked up in order to submit one's candidacy.
pub fn candidacy_bond() -> Balance {
	storage::get(CANDIDACY_BOND)
		.expect("all core parameters of council module must be in place")
}

/// How much should be locked up in order to be able to submit votes.
pub fn voting_bond() -> Balance {
	storage::get(VOTING_BOND)
		.expect("all core parameters of council module must be in place")
}

/// How long to give each top candidate to present themselves after the vote ends.
pub fn presentation_duration() -> BlockNumber {
	storage::get(PRESENTATION_DURATION)
		.expect("all core parameters of council module must be in place")
}

/// How often (in blocks) to check for new votes.
pub fn voting_period() -> BlockNumber {
	storage::get(VOTING_PERIOD)
		.expect("all core parameters of council module must be in place")
}

/// How many votes need to go by after a voter's last vote before they can be reaped if their
/// approvals are moot.
pub fn inactivity_grace_period() -> VoteIndex {
	storage::get(INACTIVE_GRACE_PERIOD)
		.expect("all core parameters of council module must be in place")
}

/// How long each position is active for.
pub fn term_duration() -> BlockNumber {
	storage::get(TERM_DURATION)
		.expect("all core parameters of council module must be in place")
}

/// The punishment, per voter, if you provide an invalid presentation.
pub fn present_slash_per_voter() -> Balance {
	storage::get(PRESENT_SLASH_PER_VOTER)
		.expect("all core parameters of council module must be in place")
}

/// Number of accounts that should be sitting on the council.
pub fn desired_seats() -> u32 {
	storage::get(DESIRED_SEATS)
		.expect("all core parameters of council module must be in place")
}

/// How many runners-up should have their approvals persist until the next vote.
pub fn carry_count() -> u32 {
	storage::get(CARRY_COUNT)
		.expect("all core parameters of council module must be in place")
}

/// True if we're currently in a presentation period.
pub fn presentation_active() -> bool {
	storage::exists(NEXT_FINALISE)
}

/// The current council. When there's a vote going on, this should still be used for executive
/// matters.
pub fn active_council() -> Vec<(AccountId, BlockNumber)> {
	storage::get_or_default(ACTIVE_COUNCIL)
}

/// If `who` a candidate at the moment?
pub fn is_a_candidate(who: &AccountId) -> bool {
	storage::exists(&who.to_keyed_vec(REGISTER_INFO_OF))
}

/// Determine the block that a vote can happen on which is no less than `n`.
pub fn next_vote_from(n: BlockNumber) -> BlockNumber {
	let voting_period = voting_period();
	(n + voting_period - 1) / voting_period * voting_period
}

/// The block number on which the tally for the next election will happen. `None` only if the
/// desired seats of the council is zero.
pub fn next_tally() -> Option<BlockNumber> {
	let desired_seats = desired_seats();
	if desired_seats == 0 {
		None
	} else {
		let c = active_council();
		let (next_possible, count, coming) =
			if let Some((tally_end, comers, leavers)) = next_finalise() {
				// if there's a tally in progress, then next tally can begin immediately afterwards
				(tally_end, c.len() - leavers.len() + comers as usize, comers)
			} else {
				(system::block_number(), c.len(), 0)
			};
		if count < desired_seats as usize {
			Some(next_possible)
		} else {
			// next tally begins once enough council members expire to bring members below desired.
			if desired_seats <= coming {
				// the entire amount of desired seats is less than those new members - we'll have
				// to wait until they expire.
				Some(next_possible + term_duration())
			} else {
				Some(c[c.len() - (desired_seats - coming) as usize].1)
			}
		}.map(next_vote_from)
	}
}

/// The accounts holding the seats that will become free on the next tally.
pub fn next_finalise() -> Option<(BlockNumber, u32, Vec<AccountId>)> {
	storage::get(NEXT_FINALISE)
}

/// The total number of votes that have happened or are in progress.
pub fn vote_index() -> VoteIndex {
	storage::get_or_default(VOTE_COUNT)
}

/// The present candidate list.
pub fn candidates() -> Vec<AccountId> {
	storage::get_or_default(CANDIDATES)
}

/// The present voter list.
pub fn voters() -> Vec<AccountId> {
	storage::get_or_default(VOTERS)
}

/// The vote index and list slot that the candidate `who` was registered or `None` if they are not
/// currently registered.
pub fn candidate_reg_info(who: &AccountId) -> Option<(VoteIndex, u32)> {
	storage::get(&who.to_keyed_vec(REGISTER_INFO_OF))
}

/// The last cleared vote index that this voter was last active at.
pub fn voter_last_active(voter: &AccountId) -> Option<VoteIndex> {
	storage::get(&voter.to_keyed_vec(LAST_ACTIVE_OF))
}

/// The last cleared vote index that this voter was last active at.
pub fn approvals_of(voter: &AccountId) -> Vec<bool> {
	storage::get_or_default(&voter.to_keyed_vec(APPROVALS_OF))
}

/// Get the leaderboard if we;re in the presentation phase.
pub fn leaderboard() -> Option<Vec<(Balance, AccountId)>> {
	storage::get(LEADERBOARD)
}

impl_dispatch! {
	pub mod public;
	fn set_approvals(votes: Vec<bool>, index: VoteIndex) = 0;
	fn reap_inactive_voter(signed_index: u32, who: AccountId, who_index: u32, assumed_vote_index: VoteIndex) = 1;
	fn retract_voter(index: u32) = 2;
	fn submit_candidacy(slot: u32) = 3;
	fn present_winner(candidate: AccountId, total: Balance, index: VoteIndex) = 4;
}

impl<'a> public::Dispatch for PublicPass<'a> {
	/// Set candidate approvals. Approval slots stay valid as long as candidates in those slots
	/// are registered.
	fn set_approvals(self, votes: Vec<bool>, index: VoteIndex) {
		assert!(!presentation_active());
		assert_eq!(index, vote_index());
		if !storage::exists(&self.to_keyed_vec(LAST_ACTIVE_OF)) {
			// not yet a voter - deduct bond.
			staking::internal::reserve_balance(&self, voting_bond());
			storage::put(VOTERS, &{
				let mut v: Vec<AccountId> = storage::get_or_default(VOTERS);
				v.push(self.clone());
				v
			});
		}
		storage::put(&self.to_keyed_vec(APPROVALS_OF), &votes);
		storage::put(&self.to_keyed_vec(LAST_ACTIVE_OF), &index);
	}

	/// Remove a voter. For it not to be a bond-consuming no-op, all approved candidate indices
	/// must now be either unregistered or registered to a candidate that registered the slot after
	/// the voter gave their last approval set.
	///
	/// May be called by anyone. Returns the voter deposit to `signed`.
	fn reap_inactive_voter(self, signed_index: u32, who: AccountId, who_index: u32, assumed_vote_index: VoteIndex) {
		assert!(!presentation_active(), "cannot reap during presentation period");
		assert!(voter_last_active(&self).is_some(), "reaper must be a voter");
		let last_active = voter_last_active(&who).expect("target for inactivity cleanup must be active");
		assert!(assumed_vote_index == vote_index(), "vote index not current");
		assert!(last_active < assumed_vote_index - inactivity_grace_period(), "cannot reap during grace perid");
		let voters = voters();
		let signed_index = signed_index as usize;
		let who_index = who_index as usize;
		assert!(signed_index < voters.len() && voters[signed_index] == *self, "bad reporter index");
		assert!(who_index < voters.len() && voters[who_index] == who, "bad target index");

		// will definitely kill one of signed or who now.

		let valid = !approvals_of(&who).iter()
			.zip(candidates().iter())
			.any(|(&appr, addr)|
				appr &&
				*addr != AccountId::default() &&
				candidate_reg_info(addr)
					.expect("all items in candidates list are registered").0 <= last_active);

		remove_voter(
			if valid { &who } else { &self },
			if valid { who_index } else { signed_index },
			voters
		);
		if valid {
			staking::internal::transfer_reserved_balance(&who, &self, voting_bond());
		} else {
			staking::internal::slash_reserved(&self, voting_bond());
		}
	}

	/// Remove a voter. All votes are cancelled and the voter deposit is returned.
	fn retract_voter(self, index: u32) {
		assert!(!presentation_active(), "cannot retract when presenting");
		assert!(storage::exists(&self.to_keyed_vec(LAST_ACTIVE_OF)), "cannot retract non-voter");
		let voters = voters();
		let index = index as usize;
		assert!(index < voters.len(), "retraction index invalid");
		assert!(voters[index] == *self, "retraction index mismatch");
		remove_voter(&self, index, voters);
		staking::internal::unreserve_balance(&self, voting_bond());
	}

	/// Submit oneself for candidacy.
	///
	/// Account must have enough transferrable funds in it to pay the bond.
	fn submit_candidacy(self, slot: u32) {
		assert!(!is_a_candidate(&self), "duplicate candidate submission");
		assert!(staking::internal::deduct_unbonded(&self, candidacy_bond()), "candidate has not enough funds");

		let slot = slot as usize;
		let count = storage::get_or_default::<u32>(CANDIDATE_COUNT) as usize;
		let candidates: Vec<AccountId> = storage::get_or_default(CANDIDATES);
		assert!(
			(slot == count && count == candidates.len()) ||
			(slot < candidates.len() && candidates[slot] == AccountId::default()),
			"invalid candidate slot"
		);

		let mut candidates = candidates;
		if slot == candidates.len() {
			candidates.push(self.clone());
		} else {
			candidates[slot] = self.clone();
		}
		storage::put(CANDIDATES, &candidates);
		storage::put(CANDIDATE_COUNT, &(count as u32 + 1));
		storage::put(&self.to_keyed_vec(REGISTER_INFO_OF), &(vote_index(), slot));
	}

	/// Claim that `signed` is one of the top carry_count() + current_vote().1 candidates.
	/// Only works if the block number >= current_vote().0 and < current_vote().0 + presentation_duration()
	/// `signed` should have at least
	fn present_winner(self, candidate: AccountId, total: Balance, index: VoteIndex) {
		assert_eq!(index, vote_index(), "index not current");
		let (_, _, expiring): (BlockNumber, u32, Vec<AccountId>) = storage::get(NEXT_FINALISE)
			.expect("cannot present outside of presentation period");
		let stakes: Vec<Balance> = storage::get_or_default(SNAPSHOTED_STAKES);
		let voters: Vec<AccountId> = storage::get_or_default(VOTERS);
		let bad_presentation_punishment = present_slash_per_voter() * voters.len() as Balance;
		assert!(staking::can_slash(&self, bad_presentation_punishment), "presenter must have sufficient slashable funds");

		let mut leaderboard = leaderboard().expect("leaderboard must exist while present phase active");
		assert!(total > leaderboard[0].0, "candidate not worthy of leaderboard");

		if let Some(p) = active_council().iter().position(|&(ref c, _)| c == &candidate) {
			assert!(p < expiring.len(), "candidate must not form a duplicated member if elected");
		}

		let (registered_since, candidate_index): (VoteIndex, u32) =
			storage::get(&candidate.to_keyed_vec(REGISTER_INFO_OF)).expect("presented candidate must be current");
		let actual_total = voters.iter()
			.zip(stakes.iter())
			.filter_map(|(voter, stake)|
				match voter_last_active(voter) {
					Some(b) if b >= registered_since =>
						approvals_of(voter).get(candidate_index as usize)
							.and_then(|approved| if *approved { Some(stake) } else { None }),
					_ => None,
				})
			.sum();
		let dupe = leaderboard.iter().find(|&&(_, ref c)| c == &candidate).is_some();
		if total == actual_total && !dupe {
			// insert into leaderboard
			leaderboard[0] = (total, candidate.clone());
			leaderboard.sort_by_key(|&(t, _)| t);
			storage::put(LEADERBOARD, &leaderboard);
		} else {
			staking::internal::slash(&self, bad_presentation_punishment);
		}
	}
}

impl_dispatch! {
	pub mod privileged;
	fn set_desired_seats(count: u32) = 0;
	fn remove_member(who: AccountId) = 1;
	fn set_presentation_duration(count: BlockNumber) = 2;
	fn set_term_duration(count: BlockNumber) = 3;
}

impl privileged::Dispatch for PrivPass {
	/// Set the desired member count; if lower than the current count, then seats will not be up
	/// election when they expire. If more, then a new vote will be started if one is not already
	/// in progress.
	fn set_desired_seats(self, count: u32) {
		storage::put(DESIRED_SEATS, &count);
	}

	/// Remove a particular member. A tally will happen instantly (if not already in a presentation
	/// period) to fill the seat if removal means that the desired members are not met.
	/// This is effective immediately.
	fn remove_member(self, who: AccountId) {
		let new_council: Vec<(AccountId, BlockNumber)> = active_council()
			.into_iter()
			.filter(|i| i.0 != who)
			.collect();
		storage::put(ACTIVE_COUNCIL, &new_council);
	}

	/// Set the presentation duration. If there is current a vote being presented for, will
	/// invoke `finalise_vote`.
	fn set_presentation_duration(self, count: BlockNumber) {
		storage::put(PRESENTATION_DURATION, &count);
	}

	/// Set the presentation duration. If there is current a vote being presented for, will
	/// invoke `finalise_vote`.
	fn set_term_duration(self, count: BlockNumber) {
		storage::put(TERM_DURATION, &count);
	}
}

pub mod internal {
	use super::*;

	/// Check there's nothing to do this block
	pub fn end_block() {
		let block_number = system::block_number();
		if block_number % voting_period() == 0 {
			if let Some(number) = next_tally() {
				if block_number == number {
					start_tally();
				}
			}
		}
		if let Some((number, _, _)) = next_finalise() {
			if block_number == number {
				finalise_tally();
			}
		}
	}
}

/// Remove a voter from the system. Trusts that voters()[index] != voter.
fn remove_voter(voter: &AccountId, index: usize, mut voters: Vec<AccountId>) {
	storage::put(VOTERS, &{ voters.swap_remove(index); voters });
	storage::kill(&voter.to_keyed_vec(APPROVALS_OF));
	storage::kill(&voter.to_keyed_vec(LAST_ACTIVE_OF));
}

/// Close the voting, snapshot the staking and the number of seats that are actually up for grabs.
fn start_tally() {
	let active_council = active_council();
	let desired_seats = desired_seats() as usize;
	let number = system::block_number();
	let expiring = active_council.iter().take_while(|i| i.1 == number).cloned().collect::<Vec<_>>();
	if active_council.len() - expiring.len() < desired_seats {
		let empty_seats = desired_seats - (active_council.len() - expiring.len());
		storage::put(NEXT_FINALISE, &(number + presentation_duration(), empty_seats as u32, expiring));

		let voters: Vec<AccountId> = storage::get_or_default(VOTERS);
		let votes: Vec<Balance> = voters.iter().map(staking::balance).collect();
		storage::put(SNAPSHOTED_STAKES, &votes);

		// initialise leaderboard.
		let leaderboard_size = empty_seats + carry_count() as usize;
		storage::put(LEADERBOARD, &vec![(0 as Balance, AccountId::default()); leaderboard_size]);
	}
}

/// Finalise the vote, removing each of the `removals` and inserting `seats` of the most approved
/// candidates in their place. If the total council members is less than the desired membership
/// a new vote is started.
/// Clears all presented candidates, returning the bond of the elected ones.
fn finalise_tally() {
	storage::kill(SNAPSHOTED_STAKES);
	let (_, coming, expiring): (BlockNumber, u32, Vec<AccountId>) = storage::take(NEXT_FINALISE)
		.expect("finalise can only be called after a tally is started.");
	let leaderboard: Vec<(Balance, AccountId)> = storage::take(LEADERBOARD).unwrap_or_default();
	let new_expiry = system::block_number() + term_duration();

	// return bond to winners.
	let candidacy_bond = candidacy_bond();
	for &(_, ref w) in leaderboard.iter()
		.rev()
		.take_while(|&&(b, _)| b != 0)
		.take(coming as usize)
	{
		staking::internal::refund(w, candidacy_bond);
	}

	// set the new council.
	let mut new_council: Vec<_> = active_council()
		.into_iter()
		.skip(expiring.len())
		.chain(leaderboard.iter()
			.rev()
			.take_while(|&&(b, _)| b != 0)
			.take(coming as usize)
			.cloned()
			.map(|(_, a)| (a, new_expiry)))
		.collect();
	new_council.sort_by_key(|&(_, expiry)| expiry);
	storage::put(ACTIVE_COUNCIL, &new_council);

	// clear all except runners-up from candidate list.
	let candidates: Vec<AccountId> = storage::get_or_default(CANDIDATES);
	let mut new_candidates = vec![AccountId::default(); candidates.len()];	// shrink later.
	let runners_up = leaderboard.into_iter()
		.rev()
		.take_while(|&(b, _)| b != 0)
		.skip(coming as usize)
		.map(|(_, a)| (a, candidate_reg_info(&a).expect("runner up must be registered").1));
	let mut count = 0u32;
	for (address, slot) in runners_up {
		new_candidates[slot as usize] = address;
		count += 1;
	}
	for (old, new) in candidates.iter().zip(new_candidates.iter()) {
		if old != new {
			// removed - kill it
			storage::kill(&old.to_keyed_vec(REGISTER_INFO_OF));
		}
	}
	// discard any superfluous slots.
	if let Some(last_index) = new_candidates.iter().rposition(|c| *c != AccountId::default()) {
		new_candidates.truncate(last_index + 1);
	}
	storage::put(CANDIDATES, &(new_candidates));
	storage::put(CANDIDATE_COUNT, &count);
	storage::put(VOTE_COUNT, &(vote_index() + 1));
}

#[cfg(test)]
pub mod testing {
	use super::*;
	use runtime_io::{twox_128, TestExternalities};
	use codec::Joiner;
	use runtime::democracy;

	pub fn externalities() -> TestExternalities {
		let extras: TestExternalities = map![
			twox_128(CANDIDACY_BOND).to_vec() => vec![].and(&9u64),
			twox_128(VOTING_BOND).to_vec() => vec![].and(&3u64),
			twox_128(PRESENT_SLASH_PER_VOTER).to_vec() => vec![].and(&1u64),
			twox_128(CARRY_COUNT).to_vec() => vec![].and(&2u32),
			twox_128(PRESENTATION_DURATION).to_vec() => vec![].and(&2u64),
			twox_128(VOTING_PERIOD).to_vec() => vec![].and(&4u64),
			twox_128(TERM_DURATION).to_vec() => vec![].and(&5u64),
			twox_128(DESIRED_SEATS).to_vec() => vec![].and(&2u32),
			twox_128(INACTIVE_GRACE_PERIOD).to_vec() => vec![].and(&1u32)
		];
		democracy::testing::externalities()
			.into_iter().chain(extras.into_iter()).collect()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use runtime_io::{with_externalities, twox_128, TestExternalities};
	use codec::{KeyedVec, Joiner};
	use keyring::Keyring::*;
	use environment::with_env;
	use demo_primitives::AccountId;
	use runtime::{staking, session, democracy};
	use super::public::Dispatch;
	use super::privileged::Dispatch as PrivDispatch;

	fn new_test_ext() -> TestExternalities {
		testing::externalities()
	}

	#[test]
	fn basic_environment_works() {
		let mut t = new_test_ext();
		with_externalities(&mut t, || {
			with_env(|e| e.block_number = 1);
			assert_eq!(next_vote_from(1), 4);
			assert_eq!(next_vote_from(4), 4);
			assert_eq!(next_vote_from(5), 8);
			assert_eq!(vote_index(), 0);
			assert_eq!(candidacy_bond(), 9);
			assert_eq!(voting_bond(), 3);
			assert_eq!(present_slash_per_voter(), 1);
			assert_eq!(presentation_duration(), 2);
			assert_eq!(voting_period(), 4);
			assert_eq!(term_duration(), 5);
			assert_eq!(desired_seats(), 2);
			assert_eq!(carry_count(), 2);

			assert_eq!(active_council(), vec![]);
			assert_eq!(next_tally(), Some(4));
			assert_eq!(presentation_active(), false);
			assert_eq!(next_finalise(), None);

			assert_eq!(candidates(), Vec::<AccountId>::new());
			assert_eq!(is_a_candidate(&Alice), false);
			assert_eq!(candidate_reg_info(&Alice), None);

			assert_eq!(voters(), Vec::<AccountId>::new());
			assert_eq!(voter_last_active(&Alice), None);
			assert_eq!(approvals_of(&Alice), vec![]);
		});
	}

	#[test]
	fn simple_candidate_submission_should_work() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 1);
			assert_eq!(candidates(), Vec::<AccountId>::new());
			assert_eq!(candidate_reg_info(&Alice), None);
			assert_eq!(candidate_reg_info(&Bob), None);
			assert_eq!(is_a_candidate(&Alice), false);
			assert_eq!(is_a_candidate(&Bob), false);

			PublicPass::test(&Alice).submit_candidacy(0);
			assert_eq!(candidates(), vec![Alice.to_raw_public()]);
			assert_eq!(candidate_reg_info(&Alice), Some((0 as VoteIndex, 0u32)));
			assert_eq!(candidate_reg_info(&Bob), None);
			assert_eq!(is_a_candidate(&Alice), true);
			assert_eq!(is_a_candidate(&Bob), false);

			PublicPass::test(&Bob).submit_candidacy(1);
			assert_eq!(candidates(), vec![Alice.to_raw_public(), Bob.into()]);
			assert_eq!(candidate_reg_info(&Alice), Some((0 as VoteIndex, 0u32)));
			assert_eq!(candidate_reg_info(&Bob), Some((0 as VoteIndex, 1u32)));
			assert_eq!(is_a_candidate(&Alice), true);
			assert_eq!(is_a_candidate(&Bob), true);
		});
	}

	fn new_test_ext_with_candidate_holes() -> TestExternalities {
		let mut t = new_test_ext();
		t.insert(twox_128(CANDIDATES).to_vec(), vec![].and(&vec![AccountId::default(), AccountId::default(), Alice.to_raw_public()]));
		t.insert(twox_128(CANDIDATE_COUNT).to_vec(), vec![].and(&1u32));
		t.insert(twox_128(&Alice.to_raw_public().to_keyed_vec(REGISTER_INFO_OF)).to_vec(), vec![].and(&(0 as VoteIndex, 2u32)));
		t
	}

	#[test]
	fn candidate_submission_using_free_slot_should_work() {
		let mut t = new_test_ext_with_candidate_holes();

		with_externalities(&mut t, || {
			with_env(|e| e.block_number = 1);
			assert_eq!(candidates(), vec![AccountId::default(), AccountId::default(), Alice.to_raw_public()]);

			PublicPass::test(&Bob).submit_candidacy(1);
			assert_eq!(candidates(), vec![AccountId::default(), Bob.into(), Alice.to_raw_public()]);

			PublicPass::test(&Charlie).submit_candidacy(0);
			assert_eq!(candidates(), vec![Charlie.into(), Bob.into(), Alice.to_raw_public()]);
		});
	}

	#[test]
	fn candidate_submission_using_alternative_free_slot_should_work() {
		let mut t = new_test_ext_with_candidate_holes();

		with_externalities(&mut t, || {
			with_env(|e| e.block_number = 1);
			assert_eq!(candidates(), vec![AccountId::default(), AccountId::default(), Alice.into()]);

			PublicPass::test(&Bob).submit_candidacy(0);
			assert_eq!(candidates(), vec![Bob.into(), AccountId::default(), Alice.into()]);

			PublicPass::test(&Charlie).submit_candidacy(1);
			assert_eq!(candidates(), vec![Bob.to_raw_public(), Charlie.into(), Alice.into()]);
		});
	}

	#[test]
	#[should_panic(expected = "invalid candidate slot")]
	fn candidate_submission_not_using_free_slot_should_panic() {
		let mut t = new_test_ext_with_candidate_holes();

		with_externalities(&mut t, || {
			with_env(|e| e.block_number = 1);
			PublicPass::test(&Dave).submit_candidacy(3);
		});
	}

	#[test]
	#[should_panic(expected = "invalid candidate slot")]
	fn bad_candidate_slot_submission_should_panic() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 1);
			assert_eq!(candidates(), Vec::<AccountId>::new());
			PublicPass::test(&Alice).submit_candidacy(1);
		});
	}

	#[test]
	#[should_panic(expected = "invalid candidate slot")]
	fn non_free_candidate_slot_submission_should_panic() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 1);
			assert_eq!(candidates(), Vec::<AccountId>::new());
			PublicPass::test(&Alice).submit_candidacy(0);
			PublicPass::test(&Bob).submit_candidacy(0);
		});
	}

	#[test]
	#[should_panic(expected = "duplicate candidate submission")]
	fn dupe_candidate_submission_should_panic() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 1);
			assert_eq!(candidates(), Vec::<AccountId>::new());
			PublicPass::test(&Alice).submit_candidacy(0);
			PublicPass::test(&Alice).submit_candidacy(1);
		});
	}

	#[test]
	#[should_panic(expected = "candidate has not enough funds")]
	fn poor_candidate_submission_should_panic() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 1);
			assert_eq!(candidates(), Vec::<AccountId>::new());
			PublicPass::test(&One).submit_candidacy(0);
		});
	}

	#[test]
	fn voting_should_work() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 1);

			PublicPass::test(&Eve).submit_candidacy(0);

			PublicPass::test(&Alice).set_approvals(vec![true], 0);
			PublicPass::test(&Dave).set_approvals(vec![true], 0);

			assert_eq!(approvals_of(&Alice), vec![true]);
			assert_eq!(approvals_of(&Dave), vec![true]);
			assert_eq!(voters(), vec![Alice.to_raw_public(), Dave.into()]);

			PublicPass::test(&Bob).submit_candidacy(1);
			PublicPass::test(&Charlie).submit_candidacy(2);

			PublicPass::test(&Bob).set_approvals(vec![false, true, true], 0);
			PublicPass::test(&Charlie).set_approvals(vec![false, true, true], 0);

			assert_eq!(approvals_of(&Alice), vec![true]);
			assert_eq!(approvals_of(&Dave), vec![true]);
			assert_eq!(approvals_of(&Bob), vec![false, true, true]);
			assert_eq!(approvals_of(&Charlie), vec![false, true, true]);

			assert_eq!(voters(), vec![Alice.to_raw_public(), Dave.into(), Bob.into(), Charlie.into()]);
		});
	}

	#[test]
	fn resubmitting_voting_should_work() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 1);

			PublicPass::test(&Eve).submit_candidacy(0);
			PublicPass::test(&Dave).set_approvals(vec![true], 0);

			assert_eq!(approvals_of(&Dave), vec![true]);

			PublicPass::test(&Bob).submit_candidacy(1);
			PublicPass::test(&Charlie).submit_candidacy(2);
			PublicPass::test(&Dave).set_approvals(vec![true, false, true], 0);

			assert_eq!(approvals_of(&Dave), vec![true, false, true]);
		});
	}

	#[test]
	fn retracting_voter_should_work() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 1);

			PublicPass::test(&Eve).submit_candidacy(0);
			PublicPass::test(&Bob).submit_candidacy(1);
			PublicPass::test(&Charlie).submit_candidacy(2);

			PublicPass::test(&Alice).set_approvals(vec![true], 0);
			PublicPass::test(&Bob).set_approvals(vec![false, true, true], 0);
			PublicPass::test(&Charlie).set_approvals(vec![false, true, true], 0);
			PublicPass::test(&Dave).set_approvals(vec![true, false, true], 0);

			assert_eq!(voters(), vec![Alice.to_raw_public(), Bob.into(), Charlie.into(), Dave.into()]);
			assert_eq!(approvals_of(&Alice), vec![true]);
			assert_eq!(approvals_of(&Bob), vec![false, true, true]);
			assert_eq!(approvals_of(&Charlie), vec![false, true, true]);
			assert_eq!(approvals_of(&Dave), vec![true, false, true]);

			PublicPass::test(&Alice).retract_voter(0);

			assert_eq!(voters(), vec![Dave.to_raw_public(), Bob.into(), Charlie.into()]);
			assert_eq!(approvals_of(&Alice), Vec::<bool>::new());
			assert_eq!(approvals_of(&Bob), vec![false, true, true]);
			assert_eq!(approvals_of(&Charlie), vec![false, true, true]);
			assert_eq!(approvals_of(&Dave), vec![true, false, true]);

			PublicPass::test(&Bob).retract_voter(1);

			assert_eq!(voters(), vec![Dave.to_raw_public(), Charlie.into()]);
			assert_eq!(approvals_of(&Alice), Vec::<bool>::new());
			assert_eq!(approvals_of(&Bob), Vec::<bool>::new());
			assert_eq!(approvals_of(&Charlie), vec![false, true, true]);
			assert_eq!(approvals_of(&Dave), vec![true, false, true]);

			PublicPass::test(&Charlie).retract_voter(1);

			assert_eq!(voters(), vec![Dave.to_raw_public()]);
			assert_eq!(approvals_of(&Alice), Vec::<bool>::new());
			assert_eq!(approvals_of(&Bob), Vec::<bool>::new());
			assert_eq!(approvals_of(&Charlie), Vec::<bool>::new());
			assert_eq!(approvals_of(&Dave), vec![true, false, true]);
		});
	}

	#[test]
	#[should_panic(expected = "retraction index mismatch")]
	fn invalid_retraction_index_should_panic() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 1);
			PublicPass::test(&Charlie).submit_candidacy(0);
			PublicPass::test(&Alice).set_approvals(vec![true], 0);
			PublicPass::test(&Bob).set_approvals(vec![true], 0);
			PublicPass::test(&Alice).retract_voter(1);
		});
	}

	#[test]
	#[should_panic(expected = "retraction index invalid")]
	fn overflow_retraction_index_should_panic() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 1);
			PublicPass::test(&Charlie).submit_candidacy(0);
			PublicPass::test(&Alice).set_approvals(vec![true], 0);
			PublicPass::test(&Alice).retract_voter(1);
		});
	}

	#[test]
	#[should_panic(expected = "cannot retract non-voter")]
	fn non_voter_retraction_should_panic() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 1);
			PublicPass::test(&Charlie).submit_candidacy(0);
			PublicPass::test(&Alice).set_approvals(vec![true], 0);
			PublicPass::test(&Bob).retract_voter(0);
		});
	}

	#[test]
	fn simple_tally_should_work() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			assert!(!presentation_active());

			PublicPass::test(&Bob).submit_candidacy(0);
			PublicPass::test(&Eve).submit_candidacy(1);
			PublicPass::test(&Bob).set_approvals(vec![true, false], 0);
			PublicPass::test(&Eve).set_approvals(vec![false, true], 0);
			internal::end_block();

			with_env(|e| e.block_number = 6);
			assert!(presentation_active());
			PublicPass::test(&Dave).present_winner(Bob.into(), 11, 0);
			PublicPass::test(&Dave).present_winner(Eve.into(), 41, 0);
			assert_eq!(leaderboard(), Some(vec![(0, AccountId::default()), (0, AccountId::default()), (11, Bob.into()), (41, Eve.into())]));

			internal::end_block();

			assert!(!presentation_active());
			assert_eq!(active_council(), vec![(Eve.to_raw_public(), 11), (Bob.into(), 11)]);

			assert!(!is_a_candidate(&Bob));
			assert!(!is_a_candidate(&Eve));
			assert_eq!(vote_index(), 1);
			assert_eq!(voter_last_active(&Bob), Some(0));
			assert_eq!(voter_last_active(&Eve), Some(0));
		});
	}

	#[test]
	fn double_presentations_should_be_punished() {
		with_externalities(&mut new_test_ext(), || {
			assert!(staking::can_slash(&Dave, 10));

			with_env(|e| e.block_number = 4);
			PublicPass::test(&Bob).submit_candidacy(0);
			PublicPass::test(&Eve).submit_candidacy(1);
			PublicPass::test(&Bob).set_approvals(vec![true, false], 0);
			PublicPass::test(&Eve).set_approvals(vec![false, true], 0);
			internal::end_block();

			with_env(|e| e.block_number = 6);
			PublicPass::test(&Dave).present_winner(Bob.into(), 11, 0);
			PublicPass::test(&Dave).present_winner(Eve.into(), 41, 0);
			PublicPass::test(&Dave).present_winner(Eve.into(), 41, 0);
			internal::end_block();

			assert_eq!(active_council(), vec![(Eve.to_raw_public(), 11), (Bob.into(), 11)]);
			assert_eq!(staking::balance(&Dave), 38);
		});
	}

	#[test]
	fn retracting_inactive_voter_should_work() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			PublicPass::test(&Bob).submit_candidacy(0);
			PublicPass::test(&Bob).set_approvals(vec![true], 0);
			internal::end_block();

			with_env(|e| e.block_number = 6);
			PublicPass::test(&Dave).present_winner(Bob.into(), 11, 0);
			internal::end_block();

			with_env(|e| e.block_number = 8);
			PublicPass::test(&Eve).submit_candidacy(0);
			PublicPass::test(&Eve).set_approvals(vec![true], 1);
			internal::end_block();

			with_env(|e| e.block_number = 10);
			PublicPass::test(&Dave).present_winner(Eve.into(), 41, 1);
			internal::end_block();

			PublicPass::test(&Eve).reap_inactive_voter(
				voters().iter().position(|&i| i == *Eve).unwrap() as u32,
				Bob.into(), voters().iter().position(|&i| i == *Bob).unwrap() as u32,
				2
			);

			assert_eq!(voters(), vec![Eve.to_raw_public()]);
			assert_eq!(approvals_of(&Bob).len(), 0);
			assert_eq!(staking::balance(&Bob), 17);
			assert_eq!(staking::balance(&Eve), 53);
		});
	}

	#[test]
	#[should_panic(expected = "candidate must not form a duplicated member if elected")]
	fn presenting_for_double_election_should_panic() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			PublicPass::test(&Bob).submit_candidacy(0);
			PublicPass::test(&Bob).set_approvals(vec![true], 0);
			internal::end_block();

			with_env(|e| e.block_number = 6);
			PublicPass::test(&Dave).present_winner(Bob.into(), 11, 0);
			internal::end_block();

			with_env(|e| e.block_number = 8);
			PublicPass::test(&Bob).submit_candidacy(0);
			PublicPass::test(&Bob).set_approvals(vec![true], 1);
			internal::end_block();

			with_env(|e| e.block_number = 10);
			PublicPass::test(&Dave).present_winner(Bob.into(), 11, 1);
		});
	}

	#[test]
	fn retracting_inactive_voter_with_other_candidates_in_slots_should_work() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			PublicPass::test(&Bob).submit_candidacy(0);
			PublicPass::test(&Bob).set_approvals(vec![true], 0);
			internal::end_block();

			with_env(|e| e.block_number = 6);
			PublicPass::test(&Dave).present_winner(Bob.into(), 11, 0);
			internal::end_block();

			with_env(|e| e.block_number = 8);
			PublicPass::test(&Eve).submit_candidacy(0);
			PublicPass::test(&Eve).set_approvals(vec![true], 1);
			internal::end_block();

			with_env(|e| e.block_number = 10);
			PublicPass::test(&Dave).present_winner(Eve.into(), 41, 1);
			internal::end_block();

			with_env(|e| e.block_number = 11);
			PublicPass::test(&Alice).submit_candidacy(0);

			PublicPass::test(&Eve).reap_inactive_voter(
				voters().iter().position(|&i| i == *Eve).unwrap() as u32,
				Bob.into(), voters().iter().position(|&i| i == *Bob).unwrap() as u32,
				2
			);

			assert_eq!(voters(), vec![Eve.to_raw_public()]);
			assert_eq!(approvals_of(&Bob).len(), 0);
			assert_eq!(staking::balance(&Bob), 17);
			assert_eq!(staking::balance(&Eve), 53);
		});
	}

	#[test]
	#[should_panic(expected = "bad reporter index")]
	fn retracting_inactive_voter_with_bad_reporter_index_should_panic() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			PublicPass::test(&Bob).submit_candidacy(0);
			PublicPass::test(&Bob).set_approvals(vec![true], 0);
			internal::end_block();

			with_env(|e| e.block_number = 6);
			PublicPass::test(&Dave).present_winner(Bob.into(), 8, 0);
			internal::end_block();

			with_env(|e| e.block_number = 8);
			PublicPass::test(&Eve).submit_candidacy(0);
			PublicPass::test(&Eve).set_approvals(vec![true], 1);
			internal::end_block();

			with_env(|e| e.block_number = 10);
			PublicPass::test(&Dave).present_winner(Eve.into(), 38, 1);
			internal::end_block();

			PublicPass::test(&Bob).reap_inactive_voter(
				42,
				Bob.into(), voters().iter().position(|&i| i == *Bob).unwrap() as u32,
				2
			);
		});
	}

	#[test]
	#[should_panic(expected = "bad target index")]
	fn retracting_inactive_voter_with_bad_target_index_should_panic() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			PublicPass::test(&Bob).submit_candidacy(0);
			PublicPass::test(&Bob).set_approvals(vec![true], 0);
			internal::end_block();

			with_env(|e| e.block_number = 6);
			PublicPass::test(&Dave).present_winner(Bob.into(), 8, 0);
			internal::end_block();

			with_env(|e| e.block_number = 8);
			PublicPass::test(&Eve).submit_candidacy(0);
			PublicPass::test(&Eve).set_approvals(vec![true], 1);
			internal::end_block();

			with_env(|e| e.block_number = 10);
			PublicPass::test(&Dave).present_winner(Eve.into(), 38, 1);
			internal::end_block();

			PublicPass::test(&Bob).reap_inactive_voter(
				voters().iter().position(|&i| i == *Bob).unwrap() as u32,
				Bob.into(), 42,
				2
			);
		});
	}

	#[test]
	fn attempting_to_retract_active_voter_should_slash_reporter() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			PublicPass::test(&Bob).submit_candidacy(0);
			PublicPass::test(&Charlie).submit_candidacy(1);
			PublicPass::test(&Dave).submit_candidacy(2);
			PublicPass::test(&Eve).submit_candidacy(3);
			PublicPass::test(&Bob).set_approvals(vec![true, false, false, false], 0);
			PublicPass::test(&Charlie).set_approvals(vec![false, true, false, false], 0);
			PublicPass::test(&Dave).set_approvals(vec![false, false, true, false], 0);
			PublicPass::test(&Eve).set_approvals(vec![false, false, false, true], 0);
			internal::end_block();

			with_env(|e| e.block_number = 6);
			PublicPass::test(&Dave).present_winner(Bob.into(), 11, 0);
			PublicPass::test(&Dave).present_winner(Charlie.into(), 21, 0);
			PublicPass::test(&Dave).present_winner(Dave.into(), 31, 0);
			PublicPass::test(&Dave).present_winner(Eve.into(), 41, 0);
			internal::end_block();

			with_env(|e| e.block_number = 8);
			PrivPass::test().set_desired_seats(3);
			internal::end_block();

			with_env(|e| e.block_number = 10);
			PublicPass::test(&Dave).present_winner(Bob.into(), 11, 1);
			PublicPass::test(&Dave).present_winner(Charlie.into(), 21, 1);
			internal::end_block();

			PublicPass::test(&Dave).reap_inactive_voter(
				voters().iter().position(|&i| i == *Dave).unwrap() as u32,
				Bob.into(), voters().iter().position(|&i| i == *Bob).unwrap() as u32,
				2
			);

			assert_eq!(voters(), vec![Bob.to_raw_public(), Charlie.into(), Eve.into()]);
			assert_eq!(approvals_of(&Dave).len(), 0);
			assert_eq!(staking::balance(&Dave), 37);
		});
	}

	#[test]
	#[should_panic(expected = "reaper must be a voter")]
	fn attempting_to_retract_inactive_voter_by_nonvoter_should_panic() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			PublicPass::test(&Bob).submit_candidacy(0);
			PublicPass::test(&Bob).set_approvals(vec![true], 0);
			internal::end_block();

			with_env(|e| e.block_number = 6);
			PublicPass::test(&Dave).present_winner(Bob.into(), 11, 0);
			internal::end_block();

			with_env(|e| e.block_number = 8);
			PublicPass::test(&Eve).submit_candidacy(0);
			PublicPass::test(&Eve).set_approvals(vec![true], 1);
			internal::end_block();

			with_env(|e| e.block_number = 10);
			PublicPass::test(&Dave).present_winner(Eve.into(), 41, 1);
			internal::end_block();

			PublicPass::test(&Dave).reap_inactive_voter(
				0,
				Bob.into(), voters().iter().position(|&i| i == *Bob).unwrap() as u32,
				2
			);
		});
	}

	#[test]
	#[should_panic(expected = "candidate not worthy of leaderboard")]
	fn presenting_loser_should_panic() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			PublicPass::test(&Alice).submit_candidacy(0);
			PublicPass::test(&Ferdie).set_approvals(vec![true], 0);
			PublicPass::test(&Bob).submit_candidacy(1);
			PublicPass::test(&Bob).set_approvals(vec![false, true], 0);
			PublicPass::test(&Charlie).submit_candidacy(2);
			PublicPass::test(&Charlie).set_approvals(vec![false, false, true], 0);
			PublicPass::test(&Dave).submit_candidacy(3);
			PublicPass::test(&Dave).set_approvals(vec![false, false, false, true], 0);
			PublicPass::test(&Eve).submit_candidacy(4);
			PublicPass::test(&Eve).set_approvals(vec![false, false, false, false, true], 0);
			internal::end_block();

			with_env(|e| e.block_number = 6);
			PublicPass::test(&Dave).present_winner(Alice.into(), 60, 0);
			PublicPass::test(&Dave).present_winner(Charlie.into(), 21, 0);
			PublicPass::test(&Dave).present_winner(Dave.into(), 31, 0);
			PublicPass::test(&Dave).present_winner(Eve.into(), 41, 0);
			PublicPass::test(&Dave).present_winner(Bob.into(), 11, 0);
		});
	}

	#[test]
	fn presenting_loser_first_should_not_matter() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			PublicPass::test(&Alice).submit_candidacy(0);
			PublicPass::test(&Ferdie).set_approvals(vec![true], 0);
			PublicPass::test(&Bob).submit_candidacy(1);
			PublicPass::test(&Bob).set_approvals(vec![false, true], 0);
			PublicPass::test(&Charlie).submit_candidacy(2);
			PublicPass::test(&Charlie).set_approvals(vec![false, false, true], 0);
			PublicPass::test(&Dave).submit_candidacy(3);
			PublicPass::test(&Dave).set_approvals(vec![false, false, false, true], 0);
			PublicPass::test(&Eve).submit_candidacy(4);
			PublicPass::test(&Eve).set_approvals(vec![false, false, false, false, true], 0);
			internal::end_block();

			with_env(|e| e.block_number = 6);
			PublicPass::test(&Dave).present_winner(Bob.into(), 11, 0);
			PublicPass::test(&Dave).present_winner(Alice.into(), 60, 0);
			PublicPass::test(&Dave).present_winner(Charlie.into(), 21, 0);
			PublicPass::test(&Dave).present_winner(Dave.into(), 31, 0);
			PublicPass::test(&Dave).present_winner(Eve.into(), 41, 0);

			assert_eq!(leaderboard(), Some(vec![
				(21, Charlie.into()),
				(31, Dave.into()),
				(41, Eve.into()),
				(60, Alice.to_raw_public())
			]));
		});
	}

	#[test]
	#[should_panic(expected = "cannot present outside of presentation period")]
	fn present_panics_outside_of_presentation_period() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			assert!(!presentation_active());
			PublicPass::test(&Eve).present_winner(Eve.into(), 1, 0);
		});
	}

	#[test]
	#[should_panic(expected = "index not current")]
	fn present_panics_with_invalid_vote_index() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			PublicPass::test(&Bob).submit_candidacy(0);
			PublicPass::test(&Eve).submit_candidacy(1);
			PublicPass::test(&Bob).set_approvals(vec![true, false], 0);
			PublicPass::test(&Eve).set_approvals(vec![false, true], 0);
			internal::end_block();

			with_env(|e| e.block_number = 6);
			PublicPass::test(&Dave).present_winner(Bob.into(), 11, 1);
		});
	}

	#[test]
	#[should_panic(expected = "presenter must have sufficient slashable funds")]
	fn present_panics_when_presenter_is_poor() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			assert!(!presentation_active());

			PublicPass::test(&Alice).submit_candidacy(0);
			PublicPass::test(&Eve).submit_candidacy(1);
			PublicPass::test(&Bob).set_approvals(vec![true, false], 0);
			PublicPass::test(&Eve).set_approvals(vec![false, true], 0);
			internal::end_block();

			with_env(|e| e.block_number = 6);
			assert_eq!(staking::balance(&Alice), 1);
			PublicPass::test(&Alice).present_winner(Alice.into(), 30, 0);
		});
	}

	#[test]
	fn invalid_present_tally_should_slash() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			assert!(!presentation_active());
			assert_eq!(staking::balance(&Dave), 40);

			PublicPass::test(&Bob).submit_candidacy(0);
			PublicPass::test(&Eve).submit_candidacy(1);
			PublicPass::test(&Bob).set_approvals(vec![true, false], 0);
			PublicPass::test(&Eve).set_approvals(vec![false, true], 0);
			internal::end_block();

			with_env(|e| e.block_number = 6);
			PublicPass::test(&Dave).present_winner(Bob.into(), 80, 0);

			assert_eq!(staking::balance(&Dave), 38);
		});
	}

	#[test]
	fn runners_up_should_be_kept() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			assert!(!presentation_active());

			PublicPass::test(&Alice).submit_candidacy(0);
			PublicPass::test(&Ferdie).set_approvals(vec![true], 0);
			PublicPass::test(&Bob).submit_candidacy(1);
			PublicPass::test(&Bob).set_approvals(vec![false, true], 0);
			PublicPass::test(&Charlie).submit_candidacy(2);
			PublicPass::test(&Charlie).set_approvals(vec![false, false, true], 0);
			PublicPass::test(&Dave).submit_candidacy(3);
			PublicPass::test(&Dave).set_approvals(vec![false, false, false, true], 0);
			PublicPass::test(&Eve).submit_candidacy(4);
			PublicPass::test(&Eve).set_approvals(vec![false, false, false, false, true], 0);

			internal::end_block();

			with_env(|e| e.block_number = 6);
			assert!(presentation_active());
			PublicPass::test(&Dave).present_winner(Alice.into(), 60, 0);
			assert_eq!(leaderboard(), Some(vec![
				(0, AccountId::default()),
				(0, AccountId::default()),
				(0, AccountId::default()),
				(60, Alice.to_raw_public())
			]));
			PublicPass::test(&Dave).present_winner(Charlie.into(), 21, 0);
			PublicPass::test(&Dave).present_winner(Dave.into(), 31, 0);
			PublicPass::test(&Dave).present_winner(Eve.into(), 41, 0);
			assert_eq!(leaderboard(), Some(vec![
				(21, Charlie.into()),
				(31, Dave.into()),
				(41, Eve.into()),
				(60, Alice.to_raw_public())
			]));

			internal::end_block();

			assert!(!presentation_active());
			assert_eq!(active_council(), vec![(Alice.to_raw_public(), 11), (Eve.into(), 11)]);

			assert!(!is_a_candidate(&Alice));
			assert!(!is_a_candidate(&Eve));
			assert!(!is_a_candidate(&Bob));
			assert!(is_a_candidate(&Charlie));
			assert!(is_a_candidate(&Dave));
			assert_eq!(vote_index(), 1);
			assert_eq!(voter_last_active(&Bob), Some(0));
			assert_eq!(voter_last_active(&Charlie), Some(0));
			assert_eq!(voter_last_active(&Dave), Some(0));
			assert_eq!(voter_last_active(&Eve), Some(0));
			assert_eq!(voter_last_active(&Ferdie), Some(0));
			assert_eq!(candidate_reg_info(&Charlie), Some((0, 2)));
			assert_eq!(candidate_reg_info(&Dave), Some((0, 3)));
		});
	}

	#[test]
	fn second_tally_should_use_runners_up() {
		with_externalities(&mut new_test_ext(), || {
			with_env(|e| e.block_number = 4);
			PublicPass::test(&Alice).submit_candidacy(0);
			PublicPass::test(&Ferdie).set_approvals(vec![true], 0);
			PublicPass::test(&Bob).submit_candidacy(1);
			PublicPass::test(&Bob).set_approvals(vec![false, true], 0);
			PublicPass::test(&Charlie).submit_candidacy(2);
			PublicPass::test(&Charlie).set_approvals(vec![false, false, true], 0);
			PublicPass::test(&Dave).submit_candidacy(3);
			PublicPass::test(&Dave).set_approvals(vec![false, false, false, true], 0);
			PublicPass::test(&Eve).submit_candidacy(4);
			PublicPass::test(&Eve).set_approvals(vec![false, false, false, false, true], 0);
			internal::end_block();

			with_env(|e| e.block_number = 6);
			PublicPass::test(&Dave).present_winner(Alice.into(), 60, 0);
			PublicPass::test(&Dave).present_winner(Charlie.into(), 21, 0);
			PublicPass::test(&Dave).present_winner(Dave.into(), 31, 0);
			PublicPass::test(&Dave).present_winner(Eve.into(), 41, 0);
			internal::end_block();

			with_env(|e| e.block_number = 8);
			PublicPass::test(&Ferdie).set_approvals(vec![false, false, true, false], 1);
			PrivPass::test().set_desired_seats(3);
			internal::end_block();

			with_env(|e| e.block_number = 10);
			PublicPass::test(&Dave).present_winner(Charlie.into(), 81, 1);
			PublicPass::test(&Dave).present_winner(Dave.into(), 31, 1);
			internal::end_block();

			assert!(!presentation_active());
			assert_eq!(active_council(), vec![(Alice.to_raw_public(), 11), (Eve.into(), 11), (Charlie.into(), 15)]);

			assert!(!is_a_candidate(&Alice));
			assert!(!is_a_candidate(&Bob));
			assert!(!is_a_candidate(&Charlie));
			assert!(!is_a_candidate(&Eve));
			assert!(is_a_candidate(&Dave));
			assert_eq!(vote_index(), 2);
			assert_eq!(voter_last_active(&Bob), Some(0));
			assert_eq!(voter_last_active(&Charlie), Some(0));
			assert_eq!(voter_last_active(&Dave), Some(0));
			assert_eq!(voter_last_active(&Eve), Some(0));
			assert_eq!(voter_last_active(&Ferdie), Some(1));

			assert_eq!(candidate_reg_info(&Dave), Some((0, 3)));
		});
	}
}
