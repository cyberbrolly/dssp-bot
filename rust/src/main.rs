//! Task 7a gate: spawn the Python worker, complete the ready handshake, and
//! round-trip a ping. The full one-job flow lands with decision.rs (7b).

mod protocol;
mod worker;

use protocol::{op, Request};
use worker::WorkerClient;

fn main() {
    let mut client = match WorkerClient::spawn().and_then(|c| {
        let mut c = c;
        c.wait_ready()?;
        Ok(c)
    }) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("dssp-bot: {e}");
            std::process::exit(1);
        }
    };

    let job_id = uuid::Uuid::new_v4().to_string();
    let resp = match client.send(&Request::ping(job_id.clone())) {
        Ok(resp) => resp,
        Err(e) => {
            eprintln!("dssp-bot: ping {job_id} failed: {e}");
            client.shutdown();
            std::process::exit(1);
        }
    };

    if resp.status == protocol::Status::Ok && resp.pong == Some(true) {
        println!("ping ok (job_id={job_id})");
    } else {
        eprintln!("dssp-bot: unexpected ping response: {:?}", resp.summary());
        client.shutdown();
        std::process::exit(1);
    }

    client.shutdown();
}
