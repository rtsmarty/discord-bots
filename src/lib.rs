#![recursion_limit="1024"]
#![feature(hash_set_entry, maybe_uninit_slice_assume_init, try_blocks)]

pub mod chain;
pub mod discord;
pub mod error;
pub mod tls;
pub mod ws;