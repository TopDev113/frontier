// Copyright 2017 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

//! BFT Agreement based on a rotating proposer in different rounds.

pub mod error;
pub mod generic;

extern crate substrate_codec as codec;
extern crate substrate_primitives as primitives;
extern crate ed25519;
extern crate tokio_timer;
extern crate parking_lot;

#[macro_use]
extern crate futures;

#[macro_use]
extern crate error_chain;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use codec::Slicable;
use ed25519::LocalizedSignature;
use primitives::bft::{Message as PrimitiveMessage, Action as PrimitiveAction, Justification as PrimitiveJustification};
use primitives::block::{Block, Id as BlockId, Header, HeaderHash};
use primitives::AuthorityId;

use futures::{stream, task, Async, Sink, Future, IntoFuture};
use futures::future::Executor;
use futures::sync::oneshot;
use tokio_timer::Timer;
use parking_lot::Mutex;

pub use generic::InputStreamConcluded;
pub use error::{Error, ErrorKind};

/// Messages over the proposal.
/// Each message carries an associated round number.
pub type Message = generic::Message<Block, HeaderHash>;

/// Localized message type.
pub type LocalizedMessage = generic::LocalizedMessage<
	Block,
	HeaderHash,
	AuthorityId,
	LocalizedSignature
>;

/// Justification of some hash.
pub type Justification = generic::Justification<HeaderHash, LocalizedSignature>;

/// Justification of a prepare message.
pub type PrepareJustification = generic::PrepareJustification<HeaderHash, LocalizedSignature>;

/// Unchecked justification.
pub type UncheckedJustification = generic::UncheckedJustification<HeaderHash, LocalizedSignature>;

impl From<PrimitiveJustification> for UncheckedJustification {
	fn from(just: PrimitiveJustification) -> Self {
		UncheckedJustification {
			round_number: just.round_number as usize,
			digest: just.hash,
			signatures: just.signatures.into_iter().map(|(from, sig)| LocalizedSignature {
				signer: ed25519::Public(from),
				signature: sig,
			}).collect(),
		}
	}
}

impl From<UncheckedJustification> for PrimitiveJustification {
	fn from(just: UncheckedJustification) -> Self {
		PrimitiveJustification {
			round_number: just.round_number as u32,
			hash: just.digest,
			signatures: just.signatures.into_iter().map(|s| (s.signer.into(), s.signature)).collect(),
		}
	}
}

/// Result of a committed round of BFT
pub type Committed = generic::Committed<Block, HeaderHash, LocalizedSignature>;

/// Communication between BFT participants.
pub type Communication = generic::Communication<Block, HeaderHash, AuthorityId, LocalizedSignature>;

/// Misbehavior observed from BFT participants.
pub type Misbehavior = generic::Misbehavior<HeaderHash, LocalizedSignature>;

/// Proposer factory. Can be used to create a proposer instance.
pub trait ProposerFactory {
	/// The proposer type this creates.
	type Proposer: Proposer;
	/// Error which can occur upon creation.
	type Error: From<Error>;

	/// Initialize the proposal logic on top of a specific header.
	// TODO: provide state context explicitly?
	fn init(&self, parent_header: &Header, authorities: &[AuthorityId], sign_with: Arc<ed25519::Pair>) -> Result<Self::Proposer, Self::Error>;
}

/// Logic for a proposer.
///
/// This will encapsulate creation and evaluation of proposals at a specific
/// block.
pub trait Proposer {
	/// Error type which can occur when proposing or evaluating.
	type Error: From<Error> + From<InputStreamConcluded> + 'static;
	/// Future that resolves to a created proposal.
	type Create: IntoFuture<Item=Block,Error=Self::Error>;
	/// Future that resolves when a proposal is evaluated.
	type Evaluate: IntoFuture<Item=bool,Error=Self::Error>;

	/// Create a proposal.
	fn propose(&self) -> Self::Create;
	/// Evaluate proposal. True means valid.
	// TODO: change this to a future.
	fn evaluate(&self, proposal: &Block) -> Self::Evaluate;
	/// Import witnessed misbehavior.
	fn import_misbehavior(&self, misbehavior: Vec<(AuthorityId, Misbehavior)>);
}

/// Block import trait.
pub trait BlockImport {
	/// Import a block alongside its corresponding justification.
	fn import_block(&self, block: Block, justification: Justification);
}

/// Trait for getting the authorities at a given block.
pub trait Authorities {
	/// Get the authorities at the given block.
	fn authorities(&self, at: &BlockId) -> Result<Vec<AuthorityId>, Error>;
}

/// Instance of BFT agreement.
struct BftInstance<P> {
	key: Arc<ed25519::Pair>,
	authorities: Vec<AuthorityId>,
	parent_hash: HeaderHash,
	timer: Timer,
	round_timeout_multiplier: u64,
	proposer: P,
}

impl<P: Proposer> generic::Context for BftInstance<P> {
	type Error = P::Error;
	type AuthorityId = AuthorityId;
	type Digest = HeaderHash;
	type Signature = LocalizedSignature;
	type Candidate = Block;
	type RoundTimeout = Box<Future<Item=(),Error=Self::Error> + Send>;
	type CreateProposal = <P::Create as IntoFuture>::Future;
	type EvaluateProposal = <P::Evaluate as IntoFuture>::Future;

	fn local_id(&self) -> AuthorityId {
		self.key.public().0
	}

	fn proposal(&self) -> Self::CreateProposal {
		self.proposer.propose().into_future()
	}

	fn candidate_digest(&self, proposal: &Block) -> HeaderHash {
		proposal.header.hash()
	}

	fn sign_local(&self, message: Message) -> LocalizedMessage {
		sign_message(message, &*self.key, self.parent_hash.clone())
	}

	fn round_proposer(&self, round: usize) -> AuthorityId {
		use primitives::hashing::blake2_256;

		// repeat blake2_256 on parent hash round + 1 times.
		// use as index into authorities vec.
		// TODO: parent hash is really insecure as a randomness beacon as
		// the prior can easily influence the block hash.
		let hashed = (0..round + 1).fold(self.parent_hash.0, |a, _| {
			blake2_256(&a[..])
		});

		let index = u32::decode(&mut &hashed[..])
			.expect("there are more than 4 bytes in a 32 byte hash; qed");

		self.authorities[(index as usize) % self.authorities.len()]
	}

	fn proposal_valid(&self, proposal: &Block) -> Self::EvaluateProposal {
		self.proposer.evaluate(proposal).into_future()
	}

	fn begin_round_timeout(&self, round: usize) -> Self::RoundTimeout {
		use std::time::Duration;

		let round = ::std::cmp::min(63, round) as u32;
		let timeout = 1u64.checked_shl(round)
			.unwrap_or_else(u64::max_value)
			.saturating_mul(self.round_timeout_multiplier);

		Box::new(self.timer.sleep(Duration::from_secs(timeout))
			.map_err(|_| Error::from(ErrorKind::FaultyTimer))
			.map_err(Into::into))
	}
}

type Input<E> = stream::Empty<Communication, E>;

// "black hole" output sink.
struct Output<E>(::std::marker::PhantomData<E>);

impl<E> Sink for Output<E> {
	type SinkItem = Communication;
	type SinkError = E;

	fn start_send(&mut self, _item: Communication) -> ::futures::StartSend<Communication, E> {
		Ok(::futures::AsyncSink::Ready)
	}

	fn poll_complete(&mut self) -> ::futures::Poll<(), E> {
		Ok(Async::Ready(()))
	}
}

/// A future that resolves either when canceled (witnessing a block from the network at same height)
/// or when agreement completes.
pub struct BftFuture<P: Proposer, I> {
	inner: generic::Agreement<BftInstance<P>, Input<P::Error>, Output<P::Error>>,
	cancel: Arc<AtomicBool>,
	send_task: Option<oneshot::Sender<task::Task>>,
	import: Arc<I>,
}

impl<P: Proposer, I: BlockImport> Future for BftFuture<P, I> {
	type Item = ();
	type Error = ();

	fn poll(&mut self) -> ::futures::Poll<(), ()> {
		// send the task to the bft service so this can be cancelled.
		if let Some(sender) = self.send_task.take() {
			let _ = sender.send(task::current());
		}

		// service has canceled the future. bail
		if self.cancel.load(Ordering::Acquire) {
			return Ok(Async::Ready(()))
		}

		// TODO: handle this error, at least by logging.
		let committed = try_ready!(self.inner.poll().map_err(|_| ()));

		// If we didn't see the proposal (very unlikely),
		// we will get the block from the network later.
		if let Some(justified_block) = committed.candidate {
			self.import.import_block(justified_block, committed.justification)
		}

		Ok(Async::Ready(()))
	}
}

impl<P: Proposer, I> Drop for BftFuture<P, I> {
	fn drop(&mut self) {
		// TODO: have a trait member to pass misbehavior reports into.
		let misbehavior = self.inner.drain_misbehavior().collect::<Vec<_>>();
		self.inner.context().proposer.import_misbehavior(misbehavior);
	}
}

struct AgreementHandle {
	cancel: Arc<AtomicBool>,
	task: Option<oneshot::Receiver<task::Task>>,
}

impl Drop for AgreementHandle {
	fn drop(&mut self) {
		let task = match self.task.take() {
			Some(t) => t,
			None => return,
		};

		// if this fails, the task is definitely not live anyway.
		if let Ok(task) = task.wait() {
			self.cancel.store(true, Ordering::Release);
			task.notify();
		}
	}
}

/// The BftService kicks off the agreement process on top of any blocks it
/// is notified of.
pub struct BftService<P, E, I> {
	client: Arc<I>,
	executor: E,
	live_agreements: Mutex<HashMap<HeaderHash, AgreementHandle>>,
	timer: Timer,
	round_timeout_multiplier: u64,
	key: Arc<ed25519::Pair>, // TODO: key changing over time.
	factory: P,
}

impl<P, E, I> BftService<P, E, I>
	where
		P: ProposerFactory,
		E: Executor<BftFuture<P::Proposer, I>>,
		I: BlockImport + Authorities,
{
	/// Signal that a valid block with the given header has been imported.
	///
	/// If the local signing key is an authority, this will begin the consensus process to build a
	/// block on top of it. If the executor fails to run the future, an error will be returned.
	pub fn build_upon(&self, header: &Header) -> Result<(), P::Error> {
		let hash = header.hash();
		let mut _preempted_consensus = None; // defers drop of live to the end.

		let authorities = self.client.authorities(&BlockId::Hash(hash))?;

		let n = authorities.len();
		let max_faulty = max_faulty_of(n);

		let local_id = self.key.public().0;

		if !authorities.contains(&local_id) {
			self.live_agreements.lock().remove(&header.parent_hash);
			return Ok(())
		}

		let proposer = self.factory.init(header, &authorities, self.key.clone())?;

		let bft_instance = BftInstance {
			proposer,
			parent_hash: hash,
			round_timeout_multiplier: self.round_timeout_multiplier,
			timer: self.timer.clone(),
			key: self.key.clone(),
			authorities: authorities,
		};

		let agreement = generic::agree(
			bft_instance,
			n,
			max_faulty,
			stream::empty(),
			Output(Default::default()),
		);

		let cancel = Arc::new(AtomicBool::new(false));
		let (tx, rx) = oneshot::channel();

		self.executor.execute(BftFuture {
			inner: agreement,
			cancel: cancel.clone(),
			send_task: Some(tx),
			import: self.client.clone(),
		}).map_err(|e| e.kind()).map_err(ErrorKind::Executor).map_err(Error::from)?;

		{
			let mut live = self.live_agreements.lock();
			live.insert(hash, AgreementHandle {
				task: Some(rx),
				cancel,
			});

			// cancel any agreements attempted to build upon this block's parent
			// as clearly agreement has already been reached.
			_preempted_consensus = live.remove(&header.parent_hash);
		}

		Ok(())
	}
}

/// Given a total number of authorities, yield the maximum faulty that would be allowed.
/// This will always be under 1/3.
pub fn max_faulty_of(n: usize) -> usize {
	n.saturating_sub(1) / 3
}

fn check_justification_signed_message(authorities: &[AuthorityId], message: &[u8], just: UncheckedJustification)
	-> Result<Justification, UncheckedJustification>
{
	just.check(authorities.len() - max_faulty_of(authorities.len()), |_, _, sig| {
		let auth_id = sig.signer.0;
		if !authorities.contains(&auth_id) { return None }

		if ed25519::verify_strong(&sig.signature, message, &sig.signer) {
			Some(sig.signer.0)
		} else {
			None
		}
	})
}

/// Check a full justification for a header hash.
/// Provide all valid authorities.
///
/// On failure, returns the justification back.
pub fn check_justification(authorities: &[AuthorityId], parent: HeaderHash, just: UncheckedJustification)
	-> Result<Justification, UncheckedJustification>
{
	let message = Slicable::encode(&PrimitiveMessage {
		parent,
		action: PrimitiveAction::Commit(just.round_number as u32, just.digest),
	});

	check_justification_signed_message(authorities, &message[..], just)
}

/// Check a prepare justification for a header hash.
/// Provide all valid authorities.
///
/// On failure, returns the justification back.
pub fn check_prepare_justification(authorities: &[AuthorityId], parent: HeaderHash, just: UncheckedJustification)
	-> Result<PrepareJustification, UncheckedJustification>
{
	let message = Slicable::encode(&PrimitiveMessage {
		parent,
		action: PrimitiveAction::Prepare(just.round_number as u32, just.digest),
	});

	check_justification_signed_message(authorities, &message[..], just)
}

/// Sign a BFT message with the given key.
pub fn sign_message(message: Message, key: &ed25519::Pair, parent_hash: HeaderHash) -> LocalizedMessage {
	let signer = key.public();

	let sign_action = |action| {
		let primitive = PrimitiveMessage {
			parent: parent_hash,
			action,
		};

		let to_sign = Slicable::encode(&primitive);
		LocalizedSignature {
			signer: signer.clone(),
			signature: key.sign(&to_sign),
		}
	};

	match message {
		::generic::Message::Propose(r, proposal) => {
			let header_hash = proposal.header.hash();
			let action_header = PrimitiveAction::ProposeHeader(r as u32, header_hash.clone());
			let action_propose = PrimitiveAction::Propose(r as u32, proposal.clone());

			::generic::LocalizedMessage::Propose(::generic::LocalizedProposal {
				round_number: r,
				proposal,
				digest: header_hash,
				sender: signer.0,
				digest_signature: sign_action(action_header),
				full_signature: sign_action(action_propose),
			})
		}
		::generic::Message::Vote(vote) => {
			let action = match vote {
				::generic::Vote::Prepare(r, h) => PrimitiveAction::Prepare(r as u32, h),
				::generic::Vote::Commit(r, h) => PrimitiveAction::Commit(r as u32, h),
				::generic::Vote::AdvanceRound(r) => PrimitiveAction::AdvanceRound(r as u32),
			};

			::generic::LocalizedMessage::Vote(::generic::LocalizedVote {
				vote: vote,
				sender: signer.0,
				signature: sign_action(action),
			})
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::collections::HashSet;
	use primitives::block;
	use self::tokio_core::reactor::{Core, Handle};
	use self::keyring::Keyring;

	extern crate substrate_keyring as keyring;
	extern crate tokio_core;

	struct FakeClient {
		authorities: Vec<AuthorityId>,
		imported_heights: Mutex<HashSet<block::Number>>
	}

	impl BlockImport for FakeClient {
		fn import_block(&self, block: Block, _justification: Justification) {
			assert!(self.imported_heights.lock().insert(block.header.number))
		}
	}

	impl Authorities for FakeClient {
		fn authorities(&self, _at: &BlockId) -> Result<Vec<AuthorityId>, Error> {
			Ok(self.authorities.clone())
		}
	}

	struct DummyFactory;
	struct DummyProposer(block::Number);

	impl ProposerFactory for DummyFactory {
		type Proposer = DummyProposer;
		type Error = Error;

		fn init(&self, parent_header: &Header, _authorities: &[AuthorityId], _sign_with: Arc<ed25519::Pair>) -> Result<DummyProposer, Error> {
			Ok(DummyProposer(parent_header.number + 1))
		}
	}

	impl Proposer for DummyProposer {
		type Error = Error;
		type Create = Result<Block, Error>;
		type Evaluate = Result<bool, Error>;

		fn propose(&self) -> Result<Block, Error> {
			Ok(Block {
				header: Header::from_block_number(self.0),
				transactions: Default::default()
			})
		}

		fn evaluate(&self, proposal: &Block) -> Result<bool, Error> {
			Ok(proposal.header.number == self.0)
		}

		fn import_misbehavior(&self, _misbehavior: Vec<(AuthorityId, Misbehavior)>) {}
	}

	fn make_service(client: FakeClient, handle: Handle)
		-> BftService<DummyFactory, Handle, FakeClient>
	{
		BftService {
			client: Arc::new(client),
			executor: handle,
			live_agreements: Mutex::new(HashMap::new()),
			timer: Timer::default(),
			round_timeout_multiplier: 4,
			key: Arc::new(Keyring::One.into()),
			factory: DummyFactory
		}
	}

	fn sign_vote(vote: ::generic::Vote<HeaderHash>, key: &ed25519::Pair, parent_hash: HeaderHash) -> LocalizedSignature {
		match sign_message(vote.into(), key, parent_hash) {
			::generic::LocalizedMessage::Vote(vote) => vote.signature,
			_ => panic!("signing vote leads to signed vote"),
		}
	}

	#[test]
	fn future_gets_preempted() {
		let client = FakeClient {
			authorities: vec![
				Keyring::One.to_raw_public(),
				Keyring::Two.to_raw_public(),
				Keyring::Alice.to_raw_public(),
				Keyring::Eve.to_raw_public(),
			],
			imported_heights: Mutex::new(HashSet::new()),
		};

		let mut core = Core::new().unwrap();

		let service = make_service(client, core.handle());

		let first = Header::from_block_number(2);
		let first_hash = first.hash();

		let mut second = Header::from_block_number(3);
		second.parent_hash = first_hash;
		let second_hash = second.hash();

		service.build_upon(&first).unwrap();
		assert!(service.live_agreements.lock().contains_key(&first_hash));

		// turn the core so the future gets polled and sends its task to the
		// service. otherwise it deadlocks.
		core.turn(Some(::std::time::Duration::from_millis(100)));
		service.build_upon(&second).unwrap();
		assert!(!service.live_agreements.lock().contains_key(&first_hash));
		assert!(service.live_agreements.lock().contains_key(&second_hash));

		core.turn(Some(::std::time::Duration::from_millis(100)));
	}

	#[test]
	fn max_faulty() {
		assert_eq!(max_faulty_of(3), 0);
		assert_eq!(max_faulty_of(4), 1);
		assert_eq!(max_faulty_of(100), 33);
		assert_eq!(max_faulty_of(0), 0);
		assert_eq!(max_faulty_of(11), 3);
		assert_eq!(max_faulty_of(99), 32);
	}

	#[test]
	fn justification_check_works() {
		let parent_hash = Default::default();
		let hash = [0xff; 32].into();

		let authorities = vec![
			Keyring::One.to_raw_public(),
			Keyring::Two.to_raw_public(),
			Keyring::Alice.to_raw_public(),
			Keyring::Eve.to_raw_public(),
		];

		let authorities_keys = vec![
			Keyring::One.into(),
			Keyring::Two.into(),
			Keyring::Alice.into(),
			Keyring::Eve.into(),
		];

		let unchecked = UncheckedJustification {
			digest: hash,
			round_number: 1,
			signatures: authorities_keys.iter().take(3).map(|key| {
				sign_vote(generic::Vote::Commit(1, hash).into(), key, parent_hash)
			}).collect(),
		};

		assert!(check_justification(&authorities, parent_hash, unchecked).is_ok());

		let unchecked = UncheckedJustification {
			digest: hash,
			round_number: 0, // wrong round number (vs. the signatures)
			signatures: authorities_keys.iter().take(3).map(|key| {
				sign_vote(generic::Vote::Commit(1, hash).into(), key, parent_hash)
			}).collect(),
		};

		assert!(check_justification(&authorities, parent_hash, unchecked).is_err());

		// not enough signatures.
		let unchecked = UncheckedJustification {
			digest: hash,
			round_number: 1,
			signatures: authorities_keys.iter().take(2).map(|key| {
				sign_vote(generic::Vote::Commit(1, hash).into(), key, parent_hash)
			}).collect(),
		};

		assert!(check_justification(&authorities, parent_hash, unchecked).is_err());

		// wrong hash.
		let unchecked = UncheckedJustification {
			digest: [0xfe; 32].into(),
			round_number: 1,
			signatures: authorities_keys.iter().take(3).map(|key| {
				sign_vote(generic::Vote::Commit(1, hash).into(), key, parent_hash)
			}).collect(),
		};

		assert!(check_justification(&authorities, parent_hash, unchecked).is_err());
	}
}
