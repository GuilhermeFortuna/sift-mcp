//! Reopen a store directory and run verify. Used by kill-during-write.sh.

use std::env;
use storage::{ChunkStore, Integrity};

fn main() {
    let dir = env::args().nth(1).expect("store directory");
    match ChunkStore::open(dir.as_ref()) {
        Ok(store) => match store.verify().expect("verify") {
            Integrity::Ok { live } => {
                println!("reopen_ok live={live}");
                std::process::exit(0);
            }
            broken => {
                eprintln!("verify_broken {broken:?}");
                std::process::exit(2);
            }
        },
        Err(e) => {
            // A crash mid-batch may leave orphan matrix rows; open fails with Corrupt.
            // Spec: store survives process kill, openable and verifiable, losing at most
            // the interrupted batch. Orphan matrix rows without metadata are recoverable
            // corruption that verify names — treat Corrupt-with-only-orphans as needing
            // compaction, but still "openable" via a recovery path.
            eprintln!("open_err {e:?}");
            std::process::exit(1);
        }
    }
}
