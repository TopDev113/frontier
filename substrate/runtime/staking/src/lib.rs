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

//! Staking manager: Handles balances and periodically determines the best set of validators.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "std")]
extern crate serde;

#[cfg(test)]
extern crate wabt;

#[macro_use]
extern crate substrate_runtime_support as runtime_support;

#[cfg_attr(feature = "std", macro_use)]
extern crate substrate_runtime_std as rstd;

extern crate substrate_codec as codec;
extern crate substrate_primitives;
extern crate substrate_runtime_contract as contract;
extern crate substrate_runtime_io as runtime_io;
extern crate substrate_runtime_primitives as primitives;
extern crate substrate_runtime_consensus as consensus;
extern crate substrate_runtime_sandbox as sandbox;
extern crate substrate_runtime_session as session;
extern crate substrate_runtime_system as system;

#[cfg(test)] use std::fmt::Debug;
use rstd::prelude::*;
use rstd::{cmp, result};
use rstd::cell::RefCell;
use rstd::collections::btree_map::{BTreeMap, Entry};
use codec::Slicable;
use runtime_support::{StorageValue, StorageMap, Parameter};
use runtime_support::dispatch::Result;
use primitives::traits::{Zero, One, Bounded, RefInto, SimpleArithmetic, Executable, MakePayment, As};

#[cfg(test)]
#[derive(Debug, PartialEq, Clone)]
pub enum LockStatus<BlockNumber: Debug + PartialEq + Clone> {
	Liquid,
	LockedUntil(BlockNumber),
	Staked,
}

#[cfg(not(test))]
#[derive(PartialEq, Clone)]
pub enum LockStatus<BlockNumber: PartialEq + Clone> {
	Liquid,
	LockedUntil(BlockNumber),
	Staked,
}

pub trait ContractAddressFor<AccountId: Sized> {
	fn contract_address_for(code: &[u8], origin: &AccountId) -> AccountId;
}

impl<Hashing, AccountId> ContractAddressFor<AccountId> for Hashing where
	Hashing: runtime_io::Hashing,
	AccountId: Sized + Slicable + From<Hashing::Output>,
	Hashing::Output: Slicable
{
	fn contract_address_for(code: &[u8], origin: &AccountId) -> AccountId {
		let mut dest_pre = Hashing::hash(code).encode();
		origin.using_encoded(|s| dest_pre.extend(s));
		AccountId::from(Hashing::hash(&dest_pre))
	}
}

pub trait Trait: system::Trait + session::Trait {
	/// The balance of an account.
	type Balance: Parameter + SimpleArithmetic + Slicable + Default + Copy;
	type DetermineContractAddress: ContractAddressFor<Self::AccountId>;
}

decl_module! {
	pub struct Module<T: Trait>;
	pub enum Call where aux: T::PublicAux {
		fn transfer(aux, dest: T::AccountId, value: T::Balance) -> Result = 0;
		fn stake(aux) -> Result = 1;
		fn unstake(aux) -> Result = 2;
	}
	pub enum PrivCall {
		fn set_sessions_per_era(new: T::BlockNumber) -> Result = 0;
		fn set_bonding_duration(new: T::BlockNumber) -> Result = 1;
		fn set_validator_count(new: u32) -> Result = 2;
		fn force_new_era() -> Result = 3;
	}
}

decl_storage! {
	trait Store for Module<T: Trait>;

	// The length of the bonding duration in eras.
	pub BondingDuration get(bonding_duration): b"sta:loc" => required T::BlockNumber;
	// The length of a staking era in sessions.
	pub ValidatorCount get(validator_count): b"sta:vac" => required u32;
	// The length of a staking era in sessions.
	pub SessionsPerEra get(sessions_per_era): b"sta:spe" => required T::BlockNumber;
	// The total amount of stake on the system.
	pub TotalStake get(total_stake): b"sta:tot" => required T::Balance;
	// The fee to be paid for making a transaction; the base.
	pub TransactionBaseFee get(transaction_base_fee): b"sta:basefee" => required T::Balance;
	// The fee to be paid for making a transaction; the per-byte portion.
	pub TransactionByteFee get(transaction_byte_fee): b"sta:bytefee" => required T::Balance;

	// The current era index.
	pub CurrentEra get(current_era): b"sta:era" => required T::BlockNumber;
	// All the accounts with a desire to stake.
	pub Intentions: b"sta:wil:" => default Vec<T::AccountId>;
	// The next value of sessions per era.
	pub NextSessionsPerEra get(next_sessions_per_era): b"sta:nse" => T::BlockNumber;
	// The block number at which the era length last changed.
	pub LastEraLengthChange get(last_era_length_change): b"sta:lec" => default T::BlockNumber;

	// The balance of a given account.
	pub FreeBalance get(free_balance): b"sta:bal:" => default map [ T::AccountId => T::Balance ];

	// The amount of the balance of a given account that is exterally reserved; this can still get
	// slashed, but gets slashed last of all.
	pub ReservedBalance get(reserved_balance): b"sta:lbo:" => default map [ T::AccountId => T::Balance ];

	// The block at which the `who`'s funds become entirely liquid.
	pub Bondage get(bondage): b"sta:bon:" => default map [ T::AccountId => T::BlockNumber ];

	// The code associated with an account.
	pub CodeOf: b"sta:cod:" => default map [ T::AccountId => Vec<u8> ];	// TODO Vec<u8> values should be optimised to not do a length prefix.

	// The storage items associated with an account/key.
	pub StorageOf: b"sta:sto:" => map [ (T::AccountId, Vec<u8>) => Vec<u8> ];	// TODO: keys should also be able to take AsRef<KeyType> to ensure Vec<u8>s can be passed as &[u8]
}

impl<T: Trait> Module<T> {

	// PUBLIC IMMUTABLES

	/// The length of a staking era in blocks.
	pub fn era_length() -> T::BlockNumber {
		Self::sessions_per_era() * <session::Module<T>>::length()
	}

	/// The combined balance of `who`.
	pub fn balance(who: &T::AccountId) -> T::Balance {
		Self::free_balance(who) + Self::reserved_balance(who)
	}

	/// Some result as `slash(who, value)` (but without the side-effects) assuming there are no
	/// balance changes in the meantime.
	pub fn can_slash(who: &T::AccountId, value: T::Balance) -> bool {
		Self::balance(who) >= value
	}

	/// Same result as `deduct_unbonded(who, value)` (but without the side-effects) assuming there
	/// are no balance changes in the meantime.
	pub fn can_deduct_unbonded(who: &T::AccountId, value: T::Balance) -> bool {
		if let LockStatus::Liquid = Self::unlock_block(who) {
			Self::free_balance(who) >= value
		} else {
			false
		}
	}

	/// The block at which the `who`'s funds become entirely liquid.
	pub fn unlock_block(who: &T::AccountId) -> LockStatus<T::BlockNumber> {
		match Self::bondage(who) {
			i if i == T::BlockNumber::max_value() => LockStatus::Staked,
			i if i <= <system::Module<T>>::block_number() => LockStatus::Liquid,
			i => LockStatus::LockedUntil(i),
		}
	}

	/// Create a smart-contract account.
	pub fn create(aux: &T::PublicAux, code: &[u8], value: T::Balance) -> Result {
		// commit anything that made it this far to storage
		if let Some(commit) = Self::effect_create(aux.ref_into(), code, value, &DirectAccountDb)? {
			<AccountDb<T>>::merge(&mut DirectAccountDb, commit);
		}
		Ok(())
	}

	// PUBLIC DISPATCH

	/// Transfer some unlocked staking balance to another staker.
	/// TODO: probably want to state gas-limit and gas-price.
	fn transfer(aux: &T::PublicAux, dest: T::AccountId, value: T::Balance) -> Result {
		// commit anything that made it this far to storage
		if let Some(commit) = Self::effect_transfer(aux.ref_into(), &dest, value, &DirectAccountDb)? {
			<AccountDb<T>>::merge(&mut DirectAccountDb, commit);
		}
		Ok(())
	}

	/// Declare the desire to stake for the transactor.
	///
	/// Effects will be felt at the beginning of the next era.
	fn stake(aux: &T::PublicAux) -> Result {
		let mut intentions = <Intentions<T>>::get();
		// can't be in the list twice.
		ensure!(intentions.iter().find(|&t| t == aux.ref_into()).is_none(), "Cannot stake if already staked.");
		intentions.push(aux.ref_into().clone());
		<Intentions<T>>::put(intentions);
		<Bondage<T>>::insert(aux.ref_into(), T::BlockNumber::max_value());
		Ok(())
	}

	/// Retract the desire to stake for the transactor.
	///
	/// Effects will be felt at the beginning of the next era.
	fn unstake(aux: &T::PublicAux) -> Result {
		let mut intentions = <Intentions<T>>::get();
		let position = intentions.iter().position(|t| t == aux.ref_into()).ok_or("Cannot unstake if not already staked.")?;
		intentions.swap_remove(position);
		<Intentions<T>>::put(intentions);
		<Bondage<T>>::insert(aux.ref_into(), Self::current_era() + Self::bonding_duration());
		Ok(())
	}

	// PRIV DISPATCH

	/// Set the number of sessions in an era.
	fn set_sessions_per_era(new: T::BlockNumber) -> Result {
		<NextSessionsPerEra<T>>::put(&new);
		Ok(())
	}

	/// The length of the bonding duration in eras.
	fn set_bonding_duration(new: T::BlockNumber) -> Result {
		<BondingDuration<T>>::put(&new);
		Ok(())
	}

	/// The length of a staking era in sessions.
	fn set_validator_count(new: u32) -> Result {
		<ValidatorCount<T>>::put(&new);
		Ok(())
	}

	/// Force there to be a new era. This also forces a new session immediately after.
	fn force_new_era() -> Result {
		Self::new_era();
		<session::Module<T>>::rotate_session();
		Ok(())
	}

	// PUBLIC MUTABLES (DANGEROUS)

	/// Deduct from an unbonded balance. true if it happened.
	pub fn deduct_unbonded(who: &T::AccountId, value: T::Balance) -> Result {
		if let LockStatus::Liquid = Self::unlock_block(who) {
			let b = Self::free_balance(who);
			if b >= value {
				<FreeBalance<T>>::insert(who, b - value);
				return Ok(())
			}
		}
		Err("not enough liquid funds")
	}

	/// Refund some balance.
	pub fn refund(who: &T::AccountId, value: T::Balance) {
		<FreeBalance<T>>::insert(who, Self::free_balance(who) + value)
	}

	/// Will slash any balance, but prefer free over reserved.
	pub fn slash(who: &T::AccountId, value: T::Balance) -> Result {
		let free_balance = Self::free_balance(who);
		let free_slash = cmp::min(free_balance, value);
		<FreeBalance<T>>::insert(who, &(free_balance - free_slash));
		if free_slash < value {
			Self::slash_reserved(who, value - free_slash)
				.map_err(|_| "not enough funds")
		} else {
			Ok(())
		}
	}

	/// Moves `value` from balance to reserved balance.
	pub fn reserve_balance(who: &T::AccountId, value: T::Balance) -> Result {
		let b = Self::free_balance(who);
		if b < value {
			return Err("not enough free funds")
		}
		<FreeBalance<T>>::insert(who, b - value);
		<ReservedBalance<T>>::insert(who, Self::reserved_balance(who) + value);
		Ok(())
	}

	/// Moves `value` from reserved balance to balance.
	pub fn unreserve_balance(who: &T::AccountId, value: T::Balance) {
		let b = Self::reserved_balance(who);
		let value = cmp::min(b, value);
		<ReservedBalance<T>>::insert(who, b - value);
		<FreeBalance<T>>::insert(who, Self::free_balance(who) + value);
	}

	/// Moves `value` from reserved balance to balance.
	pub fn slash_reserved(who: &T::AccountId, value: T::Balance) -> Result {
		let b = Self::reserved_balance(who);
		let slash = cmp::min(b, value);
		<ReservedBalance<T>>::insert(who, b - slash);
		if value == slash {
			Ok(())
		} else {
			Err("not enough (reserved) funds")
		}
	}

	/// Moves `value` from reserved balance to balance.
	pub fn transfer_reserved_balance(slashed: &T::AccountId, beneficiary: &T::AccountId, value: T::Balance) -> Result {
		let b = Self::reserved_balance(slashed);
		let slash = cmp::min(b, value);
		<ReservedBalance<T>>::insert(slashed, b - slash);
		<FreeBalance<T>>::insert(beneficiary, Self::free_balance(beneficiary) + slash);
		if value == slash {
			Ok(())
		} else {
			Err("not enough (reserved) funds")
		}
	}

	/// Hook to be called after to transaction processing.
	pub fn check_new_era() {
		// check block number and call new_era if necessary.
		if (<system::Module<T>>::block_number() - Self::last_era_length_change()) % Self::era_length() == Zero::zero() {
			Self::new_era();
		}
	}

	/// The era has changed - enact new staking set.
	///
	/// NOTE: This always happens immediately before a session change to ensure that new validators
	/// get a chance to set their session keys.
	fn new_era() {
		// Increment current era.
		<CurrentEra<T>>::put(&(<CurrentEra<T>>::get() + One::one()));

		// Enact era length change.
		if let Some(next_spe) = Self::next_sessions_per_era() {
			if next_spe != Self::sessions_per_era() {
				<SessionsPerEra<T>>::put(&next_spe);
				<LastEraLengthChange<T>>::put(&<system::Module<T>>::block_number());
			}
		}

		// evaluate desired staking amounts and nominations and optimise to find the best
		// combination of validators, then use session::internal::set_validators().
		// for now, this just orders would-be stakers by their balances and chooses the top-most
		// <ValidatorCount<T>>::get() of them.
		let mut intentions = <Intentions<T>>::get()
			.into_iter()
			.map(|v| (Self::balance(&v), v))
			.collect::<Vec<_>>();
		intentions.sort_unstable_by(|&(ref b1, _), &(ref b2, _)| b2.cmp(&b1));
		<session::Module<T>>::set_validators(
			&intentions.into_iter()
				.map(|(_, v)| v)
				.take(<ValidatorCount<T>>::get() as usize)
				.collect::<Vec<_>>()
		);
	}
}

impl<T: Trait> Executable for Module<T> {
	fn execute() {
		Self::check_new_era();
	}
}

// Each identity's stake may be in one of three bondage states, given by an integer:
// - n | n <= <CurrentEra<T>>::get(): inactive: free to be transferred.
// - ~0: active: currently representing a validator.
// - n | n > <CurrentEra<T>>::get(): deactivating: recently representing a validator and not yet
//   ready for transfer.

struct ChangeEntry<T: Trait> {
	balance: Option<T::Balance>,
	code: Option<Vec<u8>>,
	storage: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
}

// Cannot derive(Default) since it erroneously bounds T by Default.
impl<T: Trait> Default for ChangeEntry<T> {
	fn default() -> Self {
		ChangeEntry {
			balance: Default::default(),
			code: Default::default(),
			storage: Default::default(),
		}
	}
}

impl<T: Trait> ChangeEntry<T> {
	pub fn balance_changed(b: T::Balance) -> Self {
		ChangeEntry { balance: Some(b), code: None, storage: Default::default() }
	}
}

type State<T> = BTreeMap<<T as system::Trait>::AccountId, ChangeEntry<T>>;

trait AccountDb<T: Trait> {
	fn get_storage(&self, account: &T::AccountId, location: &[u8]) -> Option<Vec<u8>>;
	fn get_code(&self, account: &T::AccountId) -> Vec<u8>;
	fn get_balance(&self, account: &T::AccountId) -> T::Balance;

	fn set_storage(&mut self, account: &T::AccountId, location: Vec<u8>, value: Option<Vec<u8>>);
	fn set_code(&mut self, account: &T::AccountId, code: Vec<u8>);
	fn set_balance(&mut self, account: &T::AccountId, balance: T::Balance);

	fn merge(&mut self, state: State<T>);
}

struct DirectAccountDb;
impl<T: Trait> AccountDb<T> for DirectAccountDb {
	fn get_storage(&self, account: &T::AccountId, location: &[u8]) -> Option<Vec<u8>> {
		<StorageOf<T>>::get(&(account.clone(), location.to_vec()))
	}
	fn get_code(&self, account: &T::AccountId) -> Vec<u8> {
		<CodeOf<T>>::get(account)
	}
	fn get_balance(&self, account: &T::AccountId) -> T::Balance {
		<FreeBalance<T>>::get(account)
	}
	fn set_storage(&mut self, account: &T::AccountId, location: Vec<u8>, value: Option<Vec<u8>>) {
		if let Some(value) = value {
			<StorageOf<T>>::insert(&(account.clone(), location), &value);
		} else {
			<StorageOf<T>>::remove(&(account.clone(), location));
		}
	}
	fn set_code(&mut self, account: &T::AccountId, code: Vec<u8>) {
		<CodeOf<T>>::insert(account, &code);
	}
	fn set_balance(&mut self, account: &T::AccountId, balance: T::Balance) {
		<FreeBalance<T>>::insert(account, balance);
	}
	fn merge(&mut self, s: State<T>) {
		for (address, changed) in s.into_iter() {
			if let Some(balance) = changed.balance {
				<FreeBalance<T>>::insert(&address, balance);
			}
			if let Some(code) = changed.code {
				<CodeOf<T>>::insert(&address, &code);
			}
			for (k, v) in changed.storage.into_iter() {
				if let Some(value) = v {
					<StorageOf<T>>::insert((address.clone(), k), &value);
				} else {
					<StorageOf<T>>::remove((address.clone(), k));
				}
			}
		}
	}
}

struct OverlayAccountDb<'a, T: Trait + 'a> {
	local: RefCell<State<T>>,
	underlying: &'a AccountDb<T>,
}
impl<'a, T: Trait> OverlayAccountDb<'a, T> {
	fn new(underlying: &'a AccountDb<T>) -> OverlayAccountDb<'a, T> {
		OverlayAccountDb {
			local: RefCell::new(State::new()),
			underlying,
		}
	}

	fn into_state(self) -> State<T> {
		self.local.into_inner()
	}
}
impl<'a, T: Trait> AccountDb<T> for OverlayAccountDb<'a, T> {
	fn get_storage(&self, account: &T::AccountId, location: &[u8]) -> Option<Vec<u8>> {
		self.local
			.borrow()
			.get(account)
			.and_then(|a| a.storage.get(location))
			.cloned()
			.unwrap_or_else(|| self.underlying.get_storage(account, location))
	}
	fn get_code(&self, account: &T::AccountId) -> Vec<u8> {
		self.local
			.borrow()
			.get(account)
			.and_then(|a| a.code.clone())
			.unwrap_or_else(|| self.underlying.get_code(account))
	}
	fn get_balance(&self, account: &T::AccountId) -> T::Balance {
		self.local
			.borrow()
			.get(account)
			.and_then(|a| a.balance)
			.unwrap_or_else(|| self.underlying.get_balance(account))
	}
	fn set_storage(&mut self, account: &T::AccountId, location: Vec<u8>, value: Option<Vec<u8>>) {
		self.local
			.borrow_mut()
			.entry(account.clone())
			.or_insert(Default::default())
			.storage
			.insert(location, value);
	}
	fn set_code(&mut self, account: &T::AccountId, code: Vec<u8>) {
		self.local
			.borrow_mut()
			.entry(account.clone())
			.or_insert(Default::default())
			.code = Some(code);
	}
	fn set_balance(&mut self, account: &T::AccountId, balance: T::Balance) {
		self.local
			.borrow_mut()
			.entry(account.clone())
			.or_insert(Default::default())
			.balance = Some(balance);
	}
	fn merge(&mut self, s: State<T>) {
		let mut local = self.local.borrow_mut();

		for (address, changed) in s.into_iter() {
			match local.entry(address) {
				Entry::Occupied(e) => {
					let mut value = e.into_mut();
					if changed.balance.is_some() {
						value.balance = changed.balance;
					}
					if changed.code.is_some() {
						value.code = changed.code;
					}
					value.storage.extend(changed.storage.into_iter());
				}
				Entry::Vacant(e) => {
					e.insert(changed);
				}
			}
		}
	}
}

impl<T: Trait> Module<T> {
	fn effect_create<DB: AccountDb<T>>(
		transactor: &T::AccountId,
		code: &[u8],
		value: T::Balance,
		account_db: &DB,
	) -> result::Result<Option<State<T>>, &'static str> {
		let from_balance = account_db.get_balance(transactor);
		// TODO: a fee.
		if from_balance < value {
			return Err("balance too low to send value");
		}

		let dest = T::DetermineContractAddress::contract_address_for(code, transactor);

		// early-out if degenerate.
		if &dest == transactor {
			return Ok(None);
		}

		let mut local = BTreeMap::new();

		// two inserts are safe
		// note that we now know that `&dest != transactor` due to early-out before.
		local.insert(dest, ChangeEntry { balance: Some(value), code: Some(code.to_vec()), storage: Default::default() });
		local.insert(transactor.clone(), ChangeEntry::balance_changed(from_balance - value));

		Ok(Some(local))
	}

	fn effect_transfer<DB: AccountDb<T>>(
		transactor: &T::AccountId,
		dest: &T::AccountId,
		value: T::Balance,
		account_db: &DB,
	) -> result::Result<Option<State<T>>, &'static str> {
		let from_balance = account_db.get_balance(transactor);
		if from_balance < value {
			return Err("balance too low to send value");
		}

		let to_balance = account_db.get_balance(dest);
		if <Bondage<T>>::get(transactor) > <Bondage<T>>::get(dest) {
			return Err("bondage too high to send value");
		}
		if to_balance + value <= to_balance {
			return Err("destination balance too high to receive value");
		}

		// TODO: a fee, based upon gaslimit/gasprice.
		let gas_limit = 100_000;

		// TODO: consider storing upper-bound for contract's gas limit in fixed-length runtime
		// code in contract itself and use that.

		// Our local overlay: Should be used for any transfers and creates that happen internally.
		let mut overlay = OverlayAccountDb::new(account_db);

		if transactor != dest {
			overlay.set_balance(transactor, from_balance - value);
			overlay.set_balance(dest, to_balance + value);
		}

		let dest_code = overlay.get_code(dest);
		let should_commit = if dest_code.is_empty() {
			true
		} else {
			// TODO: logging (logs are just appended into a notable storage-based vector and cleared every
			// block).
			let mut staking_ext = StakingExt {
				account_db: &mut overlay,
				account: dest.clone(),
			};
			contract::execute(&dest_code, &mut staking_ext, gas_limit).is_ok()
		};

		Ok(if should_commit {
			Some(overlay.into_state())
		} else {
			None
		})
	}
}

struct StakingExt<'a, 'b: 'a, T: Trait + 'b> {
	account_db: &'a mut OverlayAccountDb<'b, T>,
	account: T::AccountId,
}
impl<'a, 'b: 'a, T: Trait> contract::Ext for StakingExt<'a, 'b, T> {
	type AccountId = T::AccountId;
	type Balance = T::Balance;

	fn get_storage(&self, key: &[u8]) -> Option<Vec<u8>> {
		self.account_db.get_storage(&self.account, key)
	}
	fn set_storage(&mut self, key: &[u8], value: Option<Vec<u8>>) {
		self.account_db.set_storage(&self.account, key.to_vec(), value);
	}
	fn create(&mut self, code: &[u8], value: Self::Balance) {
		if let Ok(Some(commit_state)) =
			Module::<T>::effect_create(&self.account, code, value, self.account_db)
		{
			self.account_db.merge(commit_state);
		}
	}
	fn transfer(&mut self, to: &Self::AccountId, value: Self::Balance) {
		if let Ok(Some(commit_state)) =
			Module::<T>::effect_transfer(&self.account, to, value, self.account_db)
		{
			self.account_db.merge(commit_state);
		}
	}
}

impl<T: Trait> MakePayment<T::AccountId> for Module<T> {
	fn make_payment(transactor: &T::AccountId, encoded_len: usize) -> bool {
		let b = Self::free_balance(transactor);
		let transaction_fee = Self::transaction_base_fee() + Self::transaction_byte_fee() * <T::Balance as As<usize>>::sa(encoded_len);
		if b < transaction_fee {
			return false;
		}
		<FreeBalance<T>>::insert(transactor, b - transaction_fee);
		true
	}
}

#[cfg(any(feature = "std", test))]
pub struct DummyContractAddressFor;
#[cfg(any(feature = "std", test))]
impl ContractAddressFor<u64> for DummyContractAddressFor {
	fn contract_address_for(_code: &[u8], origin: &u64) -> u64 {
		origin + 1
	}
}

#[cfg(any(feature = "std", test))]
pub struct GenesisConfig<T: Trait> {
	pub sessions_per_era: T::BlockNumber,
	pub current_era: T::BlockNumber,
	pub balances: Vec<(T::AccountId, T::Balance)>,
	pub intentions: Vec<T::AccountId>,
	pub validator_count: u64,
	pub bonding_duration: T::BlockNumber,
	pub transaction_base_fee: T::Balance,
	pub transaction_byte_fee: T::Balance,
}

#[cfg(any(feature = "std", test))]
impl<T: Trait> GenesisConfig<T> where T::AccountId: From<u64> {
	pub fn simple() -> Self {
		use primitives::traits::As;
		GenesisConfig {
			sessions_per_era: T::BlockNumber::sa(2),
			current_era: T::BlockNumber::sa(0),
			balances: vec![(T::AccountId::from(1), T::Balance::sa(111))],
			intentions: vec![T::AccountId::from(1), T::AccountId::from(2), T::AccountId::from(3)],
			validator_count: 3,
			bonding_duration: T::BlockNumber::sa(0),
			transaction_base_fee: T::Balance::sa(0),
			transaction_byte_fee: T::Balance::sa(0),
		}
	}

	pub fn extended() -> Self {
		use primitives::traits::As;
		GenesisConfig {
			sessions_per_era: T::BlockNumber::sa(3),
			current_era: T::BlockNumber::sa(1),
			balances: vec![
				(T::AccountId::from(1), T::Balance::sa(10)),
				(T::AccountId::from(2), T::Balance::sa(20)),
				(T::AccountId::from(3), T::Balance::sa(30)),
				(T::AccountId::from(4), T::Balance::sa(40)),
				(T::AccountId::from(5), T::Balance::sa(50)),
				(T::AccountId::from(6), T::Balance::sa(60)),
				(T::AccountId::from(7), T::Balance::sa(1))
			],
			intentions: vec![T::AccountId::from(1), T::AccountId::from(2), T::AccountId::from(3)],
			validator_count: 3,
			bonding_duration: T::BlockNumber::sa(0),
			transaction_base_fee: T::Balance::sa(1),
			transaction_byte_fee: T::Balance::sa(0),
		}
	}
}

#[cfg(any(feature = "std", test))]
impl<T: Trait> Default for GenesisConfig<T> {
	fn default() -> Self {
		use primitives::traits::As;
		GenesisConfig {
			sessions_per_era: T::BlockNumber::sa(1000),
			current_era: T::BlockNumber::sa(0),
			balances: vec![],
			intentions: vec![],
			validator_count: 0,
			bonding_duration: T::BlockNumber::sa(1000),
			transaction_base_fee: T::Balance::sa(0),
			transaction_byte_fee: T::Balance::sa(0),
		}
	}
}

#[cfg(any(feature = "std", test))]
impl<T: Trait> primitives::BuildExternalities for GenesisConfig<T> {
	fn build_externalities(self) -> runtime_io::TestExternalities {
		use runtime_io::twox_128;
		use codec::Slicable;

		let total_stake: T::Balance = self.balances.iter().fold(Zero::zero(), |acc, &(_, n)| acc + n);

		let mut r: runtime_io::TestExternalities = map![
			twox_128(<Intentions<T>>::key()).to_vec() => self.intentions.encode(),
			twox_128(<SessionsPerEra<T>>::key()).to_vec() => self.sessions_per_era.encode(),
			twox_128(<ValidatorCount<T>>::key()).to_vec() => self.validator_count.encode(),
			twox_128(<BondingDuration<T>>::key()).to_vec() => self.bonding_duration.encode(),
			twox_128(<TransactionBaseFee<T>>::key()).to_vec() => self.transaction_base_fee.encode(),
			twox_128(<TransactionByteFee<T>>::key()).to_vec() => self.transaction_byte_fee.encode(),
			twox_128(<CurrentEra<T>>::key()).to_vec() => self.current_era.encode(),
			twox_128(<TotalStake<T>>::key()).to_vec() => total_stake.encode()
		];

		for (who, value) in self.balances.into_iter() {
			r.insert(twox_128(&<FreeBalance<T>>::key_for(who)).to_vec(), value.encode());
		}
		r
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use runtime_io::with_externalities;
	use substrate_primitives::H256;
	use primitives::BuildExternalities;
	use primitives::traits::{HasPublicAux, Identity};
	use primitives::testing::{Digest, Header};

	pub struct Test;
	impl HasPublicAux for Test {
		type PublicAux = u64;
	}
	impl consensus::Trait for Test {
		type PublicAux = <Self as HasPublicAux>::PublicAux;
		type SessionKey = u64;
	}
	impl system::Trait for Test {
		type Index = u64;
		type BlockNumber = u64;
		type Hash = H256;
		type Hashing = runtime_io::BlakeTwo256;
		type Digest = Digest;
		type AccountId = u64;
		type Header = Header;
	}
	impl session::Trait for Test {
		type ConvertAccountIdToSessionKey = Identity;
	}
	impl Trait for Test {
		type Balance = u64;
		type DetermineContractAddress = DummyContractAddressFor;
	}

	fn new_test_ext(session_length: u64, sessions_per_era: u64, current_era: u64, monied: bool) -> runtime_io::TestExternalities {
		let mut t = system::GenesisConfig::<Test>::default().build_externalities();
		t.extend(consensus::GenesisConfig::<Test>{
			code: vec![],
			authorities: vec![],
		}.build_externalities());
		t.extend(session::GenesisConfig::<Test>{
			session_length,
			validators: vec![10, 20],
		}.build_externalities());
		t.extend(GenesisConfig::<Test>{
			sessions_per_era,
			current_era,
			balances: if monied { vec![(1, 10), (2, 20), (3, 30), (4, 40)] } else { vec![] },
			intentions: vec![],
			validator_count: 2,
			bonding_duration: 3,
			transaction_base_fee: 0,
			transaction_byte_fee: 0,
		}.build_externalities());
		t
	}

	type System = system::Module<Test>;
	type Session = session::Module<Test>;
	type Staking = Module<Test>;

	#[test]
	fn staking_should_work() {
		with_externalities(&mut new_test_ext(1, 2, 0, true), || {
			assert_eq!(Staking::era_length(), 2);
			assert_eq!(Staking::validator_count(), 2);
			assert_eq!(Staking::bonding_duration(), 3);
			assert_eq!(Session::validators(), vec![10, 20]);

			// Block 1: Add three validators. No obvious change.
			System::set_block_number(1);
			assert_ok!(Staking::stake(&1));
			assert_ok!(Staking::stake(&2));
			assert_ok!(Staking::stake(&4));
			Staking::check_new_era();
			assert_eq!(Session::validators(), vec![10, 20]);

			// Block 2: New validator set now.
			System::set_block_number(2);
			Staking::check_new_era();
			assert_eq!(Session::validators(), vec![4, 2]);

			// Block 3: Unstake highest, introduce another staker. No change yet.
			System::set_block_number(3);
			assert_ok!(Staking::stake(&3));
			assert_ok!(Staking::unstake(&4));
			Staking::check_new_era();

			// Block 4: New era - validators change.
			System::set_block_number(4);
			Staking::check_new_era();
			assert_eq!(Session::validators(), vec![3, 2]);

			// Block 5: Transfer stake from highest to lowest. No change yet.
			System::set_block_number(5);
			assert_ok!(Staking::transfer(&4, 1, 40));
			Staking::check_new_era();

			// Block 6: Lowest now validator.
			System::set_block_number(6);
			Staking::check_new_era();
			assert_eq!(Session::validators(), vec![1, 3]);

			// Block 7: Unstake three. No change yet.
			System::set_block_number(7);
			assert_ok!(Staking::unstake(&3));
			Staking::check_new_era();
			assert_eq!(Session::validators(), vec![1, 3]);

			// Block 8: Back to one and two.
			System::set_block_number(8);
			Staking::check_new_era();
			assert_eq!(Session::validators(), vec![1, 2]);
		});
	}

	#[test]
	fn staking_eras_work() {
		with_externalities(&mut new_test_ext(1, 2, 0, true), || {
			assert_eq!(Staking::era_length(), 2);
			assert_eq!(Staking::sessions_per_era(), 2);
			assert_eq!(Staking::last_era_length_change(), 0);
			assert_eq!(Staking::current_era(), 0);

			// Block 1: No change.
			System::set_block_number(1);
			Staking::check_new_era();
			assert_eq!(Staking::sessions_per_era(), 2);
			assert_eq!(Staking::last_era_length_change(), 0);
			assert_eq!(Staking::current_era(), 0);

			// Block 2: Simple era change.
			System::set_block_number(2);
			Staking::check_new_era();
			assert_eq!(Staking::sessions_per_era(), 2);
			assert_eq!(Staking::last_era_length_change(), 0);
			assert_eq!(Staking::current_era(), 1);

			// Block 3: Schedule an era length change; no visible changes.
			System::set_block_number(3);
			assert_ok!(Staking::set_sessions_per_era(3));
			Staking::check_new_era();
			assert_eq!(Staking::sessions_per_era(), 2);
			assert_eq!(Staking::last_era_length_change(), 0);
			assert_eq!(Staking::current_era(), 1);

			// Block 4: Era change kicks in.
			System::set_block_number(4);
			Staking::check_new_era();
			assert_eq!(Staking::sessions_per_era(), 3);
			assert_eq!(Staking::last_era_length_change(), 4);
			assert_eq!(Staking::current_era(), 2);

			// Block 5: No change.
			System::set_block_number(5);
			Staking::check_new_era();
			assert_eq!(Staking::sessions_per_era(), 3);
			assert_eq!(Staking::last_era_length_change(), 4);
			assert_eq!(Staking::current_era(), 2);

			// Block 6: No change.
			System::set_block_number(6);
			Staking::check_new_era();
			assert_eq!(Staking::sessions_per_era(), 3);
			assert_eq!(Staking::last_era_length_change(), 4);
			assert_eq!(Staking::current_era(), 2);

			// Block 7: Era increment.
			System::set_block_number(7);
			Staking::check_new_era();
			assert_eq!(Staking::sessions_per_era(), 3);
			assert_eq!(Staking::last_era_length_change(), 4);
			assert_eq!(Staking::current_era(), 3);
		});
	}

	#[test]
	fn staking_balance_works() {
		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(1, 42);
			assert_eq!(Staking::free_balance(&1), 42);
			assert_eq!(Staking::reserved_balance(&1), 0);
			assert_eq!(Staking::balance(&1), 42);
			assert_eq!(Staking::free_balance(&2), 0);
			assert_eq!(Staking::reserved_balance(&2), 0);
			assert_eq!(Staking::balance(&2), 0);
		});
	}

	#[test]
	fn staking_balance_transfer_works() {
		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(1, 111);
			assert_ok!(Staking::transfer(&1, 2, 69));
			assert_eq!(Staking::balance(&1), 42);
			assert_eq!(Staking::balance(&2), 69);
		});
	}

	#[test]
	fn staking_balance_transfer_when_bonded_should_not_work() {
		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(1, 111);
			assert_ok!(Staking::stake(&1));
			assert_noop!(Staking::transfer(&1, 2, 69), "bondage too high to send value");
		});
	}

	#[test]
	fn reserving_balance_should_work() {
		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(1, 111);

			assert_eq!(Staking::balance(&1), 111);
			assert_eq!(Staking::free_balance(&1), 111);
			assert_eq!(Staking::reserved_balance(&1), 0);

			assert_ok!(Staking::reserve_balance(&1, 69));

			assert_eq!(Staking::balance(&1), 111);
			assert_eq!(Staking::free_balance(&1), 42);
			assert_eq!(Staking::reserved_balance(&1), 69);
		});
	}

	#[test]
	fn staking_balance_transfer_when_reserved_should_not_work() {
		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(1, 111);
			assert_ok!(Staking::reserve_balance(&1, 69));
			assert_noop!(Staking::transfer(&1, 2, 69), "balance too low to send value");
		});
	}

	#[test]
	fn deducting_balance_should_work() {
		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(1, 111);
			assert_ok!(Staking::deduct_unbonded(&1, 69));
			assert_eq!(Staking::free_balance(&1), 42);
		});
	}

	#[test]
	fn deducting_balance_when_bonded_should_not_work() {
		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(1, 111);
			<Bondage<Test>>::insert(1, 2);
			System::set_block_number(1);
			assert_eq!(Staking::unlock_block(&1), LockStatus::LockedUntil(2));
			assert_noop!(Staking::deduct_unbonded(&1, 69), "not enough liquid funds");
		});
	}

	#[test]
	fn refunding_balance_should_work() {
		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(1, 42);
			Staking::refund(&1, 69);
			assert_eq!(Staking::free_balance(&1), 111);
		});
	}

	#[test]
	fn slashing_balance_should_work() {
		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(1, 111);
			assert_ok!(Staking::reserve_balance(&1, 69));
			assert_ok!(Staking::slash(&1, 69));
			assert_eq!(Staking::free_balance(&1), 0);
			assert_eq!(Staking::reserved_balance(&1), 42);
		});
	}

	#[test]
	fn slashing_incomplete_balance_should_work() {
		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(1, 42);
			assert_ok!(Staking::reserve_balance(&1, 21));
			assert_err!(Staking::slash(&1, 69), "not enough funds");
			assert_eq!(Staking::free_balance(&1), 0);
			assert_eq!(Staking::reserved_balance(&1), 0);
		});
	}

	#[test]
	fn unreserving_balance_should_work() {
		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(1, 111);
			assert_ok!(Staking::reserve_balance(&1, 111));
			Staking::unreserve_balance(&1, 42);
			assert_eq!(Staking::reserved_balance(&1), 69);
			assert_eq!(Staking::free_balance(&1), 42);
		});
	}

	#[test]
	fn slashing_reserved_balance_should_work() {
		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(1, 111);
			assert_ok!(Staking::reserve_balance(&1, 111));
			assert_ok!(Staking::slash_reserved(&1, 42));
			assert_eq!(Staking::reserved_balance(&1), 69);
			assert_eq!(Staking::free_balance(&1), 0);
		});
	}

	#[test]
	fn slashing_incomplete_reserved_balance_should_work() {
		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(1, 111);
			assert_ok!(Staking::reserve_balance(&1, 42));
			assert_err!(Staking::slash_reserved(&1, 69), "not enough (reserved) funds");
			assert_eq!(Staking::free_balance(&1), 69);
			assert_eq!(Staking::reserved_balance(&1), 0);
		});
	}

	#[test]
	fn transferring_reserved_balance_should_work() {
		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(1, 111);
			assert_ok!(Staking::reserve_balance(&1, 111));
			assert_ok!(Staking::transfer_reserved_balance(&1, &2, 42));
			assert_eq!(Staking::reserved_balance(&1), 69);
			assert_eq!(Staking::free_balance(&1), 0);
			assert_eq!(Staking::reserved_balance(&2), 0);
			assert_eq!(Staking::free_balance(&2), 42);
		});
	}

	#[test]
	fn transferring_incomplete_reserved_balance_should_work() {
		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(1, 111);
			assert_ok!(Staking::reserve_balance(&1, 42));
			assert_err!(Staking::transfer_reserved_balance(&1, &2, 69), "not enough (reserved) funds");
			assert_eq!(Staking::reserved_balance(&1), 0);
			assert_eq!(Staking::free_balance(&1), 69);
			assert_eq!(Staking::reserved_balance(&2), 0);
			assert_eq!(Staking::free_balance(&2), 42);
		});
	}

	const CODE_TRANSFER: &str = r#"
(module
	;; ext_transfer(transfer_to: u32, transfer_to_len: u32, value_ptr: u32, value_len: u32)
	(import "env" "ext_transfer" (func $ext_transfer (param i32 i32 i32 i32)))
	(import "env" "memory" (memory 1 1))
	(func (export "call")
		(call $ext_transfer
			(i32.const 4)  ;; Pointer to "Transfer to" address.
			(i32.const 8)  ;; Length of "Transfer to" address.
			(i32.const 12)  ;; Pointer to the buffer with value to transfer
			(i32.const 8)   ;; Length of the buffer with value to transfer.
		)
	)
	;; Destination AccountId to transfer the funds.
	;; Represented by u64 (8 bytes long) in little endian.
	(data (i32.const 4) "\02\00\00\00\00\00\00\00")
	;; Amount of value to transfer.
	;; Represented by u64 (8 bytes long) in little endian.
	(data (i32.const 12) "\06\00\00\00\00\00\00\00")
)
"#;

	#[test]
	fn contract_transfer() {
		let code_transfer = wabt::wat2wasm(CODE_TRANSFER).unwrap();

		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			<FreeBalance<Test>>::insert(0, 111);
			<FreeBalance<Test>>::insert(1, 0);
			<FreeBalance<Test>>::insert(2, 30);

			<CodeOf<Test>>::insert(1, code_transfer.to_vec());

			assert_ok!(Staking::transfer(&0, 1, 11));

			assert_eq!(Staking::balance(&0), 100);
			assert_eq!(Staking::balance(&1), 5);
			assert_eq!(Staking::balance(&2), 36);
		});
	}

	const CODE_MEM: &str =
r#"
(module
	;; Internal memory is not allowed.
	(memory 1 1)
	(func (export "call")
		nop
	)
)
"#;

	#[test]
	fn contract_internal_mem() {
		let code_mem = wabt::wat2wasm(CODE_MEM).unwrap();

		with_externalities(&mut new_test_ext(1, 3, 1, false), || {
			// Set initial balances.
			<FreeBalance<Test>>::insert(0, 111);
			<FreeBalance<Test>>::insert(1, 0);

			<CodeOf<Test>>::insert(1, code_mem.to_vec());

			// Transfer some balance from 0 to 1.
			assert_ok!(Staking::transfer(&0, 1, 11));

			// The balance should remain unchanged since we are expecting
			// validation error caused by internal memory declaration.
			assert_eq!(Staking::balance(&0), 111);
		});
	}
}
