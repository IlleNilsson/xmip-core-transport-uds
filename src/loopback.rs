//! Both ends of one diagnostic session on this machine (ADR-0051).
//!
//! A tester and an ECU, two nodes on a fresh simulated bus per round,
//! each end an ISO-TP session of its own: the tester's requests
//! reach the ECU, and its answers come back. The near end
//! writes a Stream to the data identifier; the far end serves the
//! identifier as the ECU and hands the Stream on when the write completes.
//! The two ends need two threads, so the capability's `round` drives it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use can_bus::Bus;
use iso_tp::{ECU_ID, IsoTpTransport, TESTER_ID};
use sdk::broadcast::Medium;
use transport::Arrived;
use transport::error::{Result, protocol_error};
use transport::loopback::{FarEnd, LOOPBACK_TIMEOUT, Loopback};

use crate::UdsTransport;

/// One loopback session: the tester and the ECU, each a node on one simulated
/// bus, hearing what the other transmits. Until 2026-09-24 a session was two
/// directed queues, because the in-process bus returned a node's own frames.
#[derive(Clone)]
pub(crate) struct Session {
    tester: Arc<dyn Bus>,
    ecu: Arc<dyn Bus>,
}

impl Session {
    /// A fresh bus with a tester and an ECU on it.
    fn fresh() -> Self {
        let medium = Medium::new("loopback");
        Self {
            tester: Arc::new(medium.node()),
            ecu: Arc::new(medium.node()),
        }
    }
}

/// The sessions a loopback has stood up and not yet taken, by address. A
/// fresh bus per round, so rounds driven at once from several
/// threads never read each other's frames.
pub(crate) type Standing = Arc<Mutex<HashMap<String, Session>>>;

/// Numbers the sessions, so each address names one.
static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

impl UdsTransport {
    /// Both ends on this machine: a tester whose far end is an ECU, the
    /// two nodes on a fresh simulated bus per round, the
    /// loopback timeout on both. The link this instance itself holds
    /// carries nothing; every round stands up its own.
    #[must_use]
    pub fn loopback() -> Self {
        let idle: Arc<dyn Bus> = Arc::new(Medium::new("loopback").node());
        let link = IsoTpTransport::new(Arc::clone(&idle), idle, TESTER_ID)
            .timing_out_after(LOOPBACK_TIMEOUT);
        Self::new(link)
    }

    /// The tester's end of the session at `address`.
    fn tester(&self, address: &str) -> Result<Self> {
        let session = self
            .standing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(address)
            .cloned()
            .ok_or_else(|| protocol_error(format!("{address} is not a session stood up here")))?;
        let link = IsoTpTransport::new(Arc::clone(&session.tester), session.tester, TESTER_ID)
            .timing_out_after(LOOPBACK_TIMEOUT);
        Ok(Self {
            link,
            did: self.did,
            ecu: Arc::clone(&self.ecu),
            standing: Arc::clone(&self.standing),
        })
    }
}

/// An ECU serving until one write completes. It owns the session: the
/// address is forgotten once the Stream is taken.
struct Serving {
    end: UdsTransport,
    address: String,
}

impl FarEnd for Serving {
    fn address(&self) -> &str {
        &self.address
    }

    fn take_one(self: Box<Self>) -> Result<Arrived> {
        let taken = self.end.serve();
        self.end
            .standing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.address);
        taken
    }
}

impl Loopback for UdsTransport {
    fn far_end(&self) -> Result<Box<dyn FarEnd>> {
        let session = Session::fresh();
        let link = IsoTpTransport::new(Arc::clone(&session.ecu), Arc::clone(&session.ecu), ECU_ID)
            .timing_out_after(LOOPBACK_TIMEOUT);
        let address = format!(
            "uds://loopback/{}",
            NEXT_SESSION.fetch_add(1, Ordering::Relaxed)
        );
        self.standing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(address.clone(), session);
        Ok(Box::new(Serving {
            end: Self {
                link,
                did: self.did,
                ecu: Arc::clone(&self.ecu),
                standing: Arc::clone(&self.standing),
            },
            address,
        }))
    }

    fn send_to(&self, address: &str, payload: &[u8]) -> Result<()> {
        let tester = self.tester(address)?;
        tester.write(tester.did, payload)
    }

    /// No socket to poke. An empty message is what no request is, so an
    /// ECU whose tester was refused reads it and is judged now rather than
    /// at its deadline.
    fn unblock(&self, address: &str) {
        if let Ok(tester) = self.tester(address) {
            drop(tester.link.deliver(&[]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DEFAULT_DID, Ecu};
    use transport::payload::{edge_payloads, patterned};

    #[test]
    fn the_loopback_returns_the_edge_payloads_whole() {
        let loopback = UdsTransport::loopback();
        let edges = edge_payloads();
        for (name, bytes) in edges {
            let arrived = loopback
                .round(&bytes)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(arrived.bytes, bytes, "{name}");
            assert_eq!(arrived.origin_uri, "uds://loopback/0xf190", "{name}");
        }
        assert!(
            Loopback::ceiling(&loopback).is_none(),
            "a download has no ceiling"
        );
        assert!(loopback.refuses(b"x").is_none());
        assert!(
            loopback.standing.lock().expect("lock").is_empty(),
            "a taken session is forgotten"
        );
        assert_eq!(
            loopback.ecu().held(DEFAULT_DID).expect("held"),
            b"\r\n".repeat(400)
        );
    }

    #[test]
    fn a_stream_too_long_for_one_message_downloads_in_blocks() {
        let long = patterned(10_000);
        let arrived = UdsTransport::loopback().round(&long).expect("three blocks");
        assert_eq!(arrived.bytes, long);
        let small = UdsTransport::loopback()
            .at(0x0100)
            .serving(Ecu::new().in_blocks_of(64));
        let arrived = small.round(&patterned(1000)).expect("seventeen blocks");
        assert_eq!(arrived.bytes, patterned(1000));
        assert_eq!(arrived.origin_uri, "uds://loopback/0x0100");
    }

    #[test]
    fn a_session_that_was_not_stood_up_is_refused_at_once() {
        let loopback = UdsTransport::loopback();
        let error = loopback
            .send_to("uds://loopback/0", b"x")
            .expect_err("no session");
        assert!(error.message.contains("not a session"), "{error}");
        loopback.unblock("uds://loopback/0");
    }
}
