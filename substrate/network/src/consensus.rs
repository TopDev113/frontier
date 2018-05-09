// Copyright 2017 Parity Technologies (UK) Ltd.
// This file is part of Polkadot.

// Polkadot is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Polkadot is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.?

//! Consensus related bits of the network service.

use std::collections::{HashMap, HashSet};
use futures::sync::{oneshot, mpsc};
use std::time::{Instant, Duration};
use std::collections::hash_map::Entry;
use io::SyncIo;
use protocol::Protocol;
use network::PeerId;
use primitives::Hash;
use message::{self, Message};
use runtime_support::Hashable;

// TODO: Add additional spam/DoS attack protection.
const MESSAGE_LIFETIME_SECONDS: u64 = 600;

struct CandidateRequest {
	id: message::RequestId,
	completion: oneshot::Sender<Vec<u8>>,
}

struct PeerConsensus {
	candidate_fetch: Option<CandidateRequest>,
	candidate_available: Option<Hash>,
	known_messages: HashSet<Hash>,
}

/// Consensus network protocol handler. Manages statements and candidate requests.
pub struct Consensus {
	peers: HashMap<PeerId, PeerConsensus>,
	our_candidate: Option<(Hash, Vec<u8>)>,
	statement_sink: Option<mpsc::UnboundedSender<message::Statement>>,
	bft_message_sink: Option<(mpsc::UnboundedSender<message::LocalizedBftMessage>, Hash)>,
	messages: HashMap<Hash, (Instant, message::Message)>,
}

impl Consensus {
	/// Create a new instance.
	pub fn new() -> Consensus {
		Consensus {
			peers: HashMap::new(),
			our_candidate: None,
			statement_sink: None,
			bft_message_sink: None,
			messages: Default::default(),
		}
	}

	/// Closes all notification streams.
	pub fn restart(&mut self) {
		self.statement_sink = None;
		self.bft_message_sink = None;
	}

	/// Handle new connected peer.
	pub fn new_peer(&mut self, io: &mut SyncIo, protocol: &Protocol, peer_id: PeerId, roles: &[message::Role]) {
		if roles.iter().any(|r| *r == message::Role::Validator) {
			trace!(target:"sync", "Registering validator {}", peer_id);
			// Send out all known messages.
			// TODO: limit by size
			let mut known_messages = HashSet::new();
			for (hash, &(_, ref m)) in self.messages.iter() {
				known_messages.insert(hash.clone());
				protocol.send_message(io, peer_id, m.clone());
			}
			self.peers.insert(peer_id, PeerConsensus {
				candidate_fetch: None,
				candidate_available: None,
				known_messages,
			});
		}
	}

	fn propagate(&mut self, io: &mut SyncIo, protocol: &Protocol, message: message::Message, hash: Hash) {
		for (id, ref mut peer) in self.peers.iter_mut() {
			if peer.known_messages.insert(hash.clone()) {
				protocol.send_message(io, *id, message.clone());
			}
		}
	}

	fn register_message(&mut self, hash: Hash, message: message::Message) {
		if let Entry::Vacant(entry) = self.messages.entry(hash) {
			entry.insert((Instant::now(), message));
		}
	}

	pub fn on_statement(&mut self, io: &mut SyncIo, protocol: &Protocol, peer_id: PeerId, statement: message::Statement, hash: Hash) {
		if self.messages.contains_key(&hash) {
			trace!(target:"sync", "Ignored already known statement from {}", peer_id);
		}
		if let Some(ref mut peer) = self.peers.get_mut(&peer_id) {
			// TODO: validate signature?
			match &statement.statement {
				&message::UnsignedStatement::Candidate(ref receipt) => peer.candidate_available = Some(Hash::from(receipt.blake2_256())),
				&message::UnsignedStatement::Available(ref hash) => peer.candidate_available = Some(*hash),
				&message::UnsignedStatement::Valid(_) | &message::UnsignedStatement::Invalid(_) => (),
			}
			peer.known_messages.insert(hash);
			if let Some(sink) = self.statement_sink.take() {
				if let Err(e) = sink.unbounded_send(statement.clone()) {
					trace!(target:"sync", "Error broadcasting statement notification: {:?}", e);
				} else {
					self.statement_sink = Some(sink);
				}
			}
		} else {
			trace!(target:"sync", "Ignored statement from unregistered peer {}", peer_id);
			return;
		}
		let message = Message::Statement(statement);
		self.register_message(hash.clone(), message.clone());
		// Propagate to other peers.
		self.propagate(io, protocol, message, hash);
	}

	pub fn statements(&mut self) -> mpsc::UnboundedReceiver<message::Statement>{
		let (sink, stream) = mpsc::unbounded();
		self.statement_sink = Some(sink);
		stream
	}

	pub fn on_bft_message(&mut self, io: &mut SyncIo, protocol: &Protocol, peer_id: PeerId, message: message::LocalizedBftMessage, hash: Hash) {
		if self.messages.contains_key(&hash) {
			trace!(target:"sync", "Ignored already known BFT message from {}", peer_id);
			return;
		}

		if let Some(ref mut peer) = self.peers.get_mut(&peer_id) {
			peer.known_messages.insert(hash);
			// TODO: validate signature?
			if let Some((sink, parent_hash)) = self.bft_message_sink.take() {
				if message.parent_hash == parent_hash {
					if let Err(e) = sink.unbounded_send(message.clone()) {
						trace!(target:"sync", "Error broadcasting BFT message notification: {:?}", e);
					} else {
						self.bft_message_sink = Some((sink, parent_hash));
					}
				}
			}
		} else {
			trace!(target:"sync", "Ignored BFT statement from unregistered peer {}", peer_id);
			return;
		}

		let message = Message::BftMessage(message);
		self.register_message(hash.clone(), message.clone());
		// Propagate to other peers.
		self.propagate(io, protocol, message, hash);
	}

	pub fn bft_messages(&mut self, parent_hash: Hash) -> mpsc::UnboundedReceiver<message::LocalizedBftMessage>{
		let (sink, stream) = mpsc::unbounded();

		for (_, message) in self.messages.iter() {
			let bft_message = match *message {
				(_, Message::BftMessage(ref msg)) => msg,
				_ => continue,
			};

			if bft_message.parent_hash == parent_hash {
				sink.unbounded_send(bft_message.clone()).expect("receiving end known to be open; qed");
			}
		}

		self.bft_message_sink = Some((sink, parent_hash));
		stream
	}

	pub fn fetch_candidate(&mut self, io: &mut SyncIo, protocol: &Protocol, hash: &Hash) -> oneshot::Receiver<Vec<u8>> {
		// Request from the first peer that has it available.
		// TODO: random peer selection.
		trace!(target:"sync", "Trying to fetch candidate {:?}", hash);
		let (sender, receiver) = oneshot::channel();
		if let Some((peer_id, ref mut peer)) = self.peers.iter_mut()
			.find(|&(_, ref peer)| peer.candidate_fetch.is_none() && peer.candidate_available.as_ref().map_or(false, |h| h == hash)) {

			trace!(target:"sync", "Fetching candidate from {}", peer_id);
			let id = 0; //TODO: generate unique id
			peer.candidate_fetch = Some(CandidateRequest {
				id: id,
				completion: sender,
			});
			let request = message::CandidateRequest {
				id: id,
				hash: *hash,
			};
			protocol.send_message(io, *peer_id, Message::CandidateRequest(request));
		}
		// If no peer found `sender` is dropped and `receiver` is canceled immediatelly.
		return receiver;
	}

	pub fn send_statement(&mut self, io: &mut SyncIo, protocol: &Protocol, statement: message::Statement) {
		// Broadcast statement to all validators.
		trace!(target:"sync", "Broadcasting statement {:?}", statement);
		let message = Message::Statement(statement);
		let hash = Protocol::hash_message(&message);
		self.register_message(hash.clone(), message.clone());
		self.propagate(io, protocol, message, hash);
	}

	pub fn send_bft_message(&mut self, io: &mut SyncIo, protocol: &Protocol, message: message::LocalizedBftMessage) {
		// Broadcast message to all validators.
		trace!(target:"sync", "Broadcasting BFT message {:?}", message);
		let message = Message::BftMessage(message);
		let hash = Protocol::hash_message(&message);
		self.register_message(hash.clone(), message.clone());
		self.propagate(io, protocol, message, hash);
	}

	pub fn set_local_candidate(&mut self, candidate: Option<(Hash, Vec<u8>)>) {
		trace!(target:"sync", "Set local candidate to {:?}", candidate.as_ref().map(|&(h, _)| h));
		self.our_candidate = candidate;
	}

	pub fn on_candidate_request(&mut self, io: &mut SyncIo, protocol: &Protocol, peer_id: PeerId, request: message::CandidateRequest) {
		let response = match self.our_candidate {
			Some((ref hash, ref data)) if *hash == request.hash => Some(data.clone()),
			_ => None,
		};
		let msg = message::CandidateResponse {
			id: request.id,
			data: response,
		};
		protocol.send_message(io, peer_id, Message::CandidateResponse(msg));
	}

	pub fn on_candidate_response(&mut self, io: &mut SyncIo, _protocol: &Protocol, peer_id: PeerId, response: message::CandidateResponse) {
		if let Some(ref mut peer) = self.peers.get_mut(&peer_id) {
			if let Some(request) = peer.candidate_fetch.take() {
				if response.id == request.id {
					if let Some(data) = response.data {
						if let Err(e) = request.completion.send(data) {
							trace!(target:"sync", "Error sending candidate data notification: {:?}", e);
						}
					}
				} else {
					trace!(target:"sync", "Unexpected candidate response from {}", peer_id);
					io.disable_peer(peer_id);
				}
			} else {
				trace!(target:"sync", "Unexpected candidate response from {}", peer_id);
				io.disable_peer(peer_id);
			}
		}
	}

	pub fn peer_disconnected(&mut self, _io: &mut SyncIo, _protocol: &Protocol, peer_id: PeerId) {
		self.peers.remove(&peer_id);
	}

	pub fn collect_garbage(&mut self) {
		let expiration = Duration::from_secs(MESSAGE_LIFETIME_SECONDS);
		let now = Instant::now();
		let before = self.messages.len();
		self.messages.retain(|_, &mut (timestamp, _)| timestamp < now + expiration);
		if self.messages.len() != before {
			trace!(target:"sync", "Cleaned up {} stale messages", before - self.messages.len());
		}
		let messages = &self.messages;
		for (_, ref mut peer) in self.peers.iter_mut() {
			peer.known_messages.retain(|h| messages.contains_key(h));
		}
	}
}
