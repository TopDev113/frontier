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

//! Polkadot CLI

#![warn(missing_docs)]

extern crate polkadot_cli as cli;

#[macro_use]
extern crate error_chain;
extern crate ctrlc;

use std::sync::mpsc;

quick_main!(run);

fn run() -> cli::error::Result<()> {
	let (exit_send, exit_receive) = mpsc::channel();
	ctrlc::CtrlC::set_handler(move || {
		exit_send.send(()).expect("Error sending exit notification");
	});
	cli::run(::std::env::args(), exit_receive)
}
