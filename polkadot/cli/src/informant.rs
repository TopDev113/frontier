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
// along with Polkadot.  If not, see <http://www.gnu.org/licenses/>.

//! Console informant. Prints sync progress and block events. Runs on the calling thread.

use std::time::{Duration, Instant};
use futures::stream::Stream;
use service::Service;
use tokio_core::reactor;
use network::{SyncState, SyncProvider};
use runtime_support::Hashable;
use primitives::block::HeaderHash;
use state_machine;
use client::{self, BlockchainEvents};

const TIMER_INTERVAL_MS: u64 = 5000;

/// Spawn informant on the event loop
pub fn start<B, E>(service: &Service<B, E>, handle: reactor::Handle)
	where
		B: client::backend::Backend + Send + Sync + 'static,
		E: client::CallExecutor + Send + Sync + 'static,
		client::error::Error: From<<<B as client::backend::Backend>::State as state_machine::backend::Backend>::Error>
{
	let interval = reactor::Interval::new_at(Instant::now(), Duration::from_millis(TIMER_INTERVAL_MS), &handle)
		.expect("Error creating informant timer");

	let network = service.network();
	let client = service.client();

	let display_notifications = interval.map_err(|e| debug!("Timer error: {:?}", e)).for_each(move |_| {
		let sync_status = network.status();

		if let Ok(best_block) = client.best_block_header() {
			let hash: HeaderHash = best_block.blake2_256().into();
			let status = match (sync_status.sync.state, sync_status.sync.best_seen_block) {
				(SyncState::Idle, _) => "Idle".into(),
				(SyncState::Downloading, None) => "Syncing".into(),
				(SyncState::Downloading, Some(n)) => format!("Syncing, target=#{}", n),
			};
			info!(target: "polkadot", "{} ({} peers), best: #{} ({})", status, sync_status.num_peers, best_block.number, hash)
		} else {
			warn!("Error getting best block information");
		}
		Ok(())
	});

	let client = service.client();
	let display_block_import = client.import_notification_stream().for_each(|n| {
		info!(target: "polkadot", "Imported #{} ({})", n.header.number, n.hash);
		Ok(())
	});

	handle.spawn(display_notifications);
	handle.spawn(display_block_import);
}

