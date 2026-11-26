//! What a Location of this technology is configured with: its declaration,
//! which `Configured` reads, built with the crate's `declaration` feature
//! alone and the code behind the default (ADR-0064, amendments 2026-10-22
//! and 2026-11-26), for the library the surfaces load to carry (open
//! problem 48). The data identifier a Location takes unless told otherwise
//! is declared here, and the code takes it from here; the identifiers and
//! the wait are ISO-TP's, taken by that crate's own `declaration` feature.

use iso_tp::declaration::{DEFAULT_TIMEOUT, ECU_ID, TESTER_ID};
use xcore::settings::{Applies, Fixed, Kind, Presence, Setting, Settings};

/// The data identifier a Location writes or serves unless told otherwise:
/// the vehicle identification number of ISO 14229-1 annex C.
pub const DEFAULT_DID: u16 = 0xf190;

/// Every setting a Location takes beyond its address, the bus, as can-bus
/// opens it: the kernel interface, `can0`, where the build has one; a
/// simulated bus of that name otherwise.
pub const SETTINGS: &Settings = &Settings {
    technology: env!("CARGO_PKG_NAME"),
    settings: &[
        Setting {
            name: "did",
            kind: Kind::Integer {
                minimum: 0,
                maximum: 0xffff,
            },
            presence: Presence::Default(Fixed::Integer(DEFAULT_DID as i64)),
            meaning: "The data identifier a Send Location writes and a Receive Location \
                      serves.",
            applies: Applies::Both,
        },
        Setting {
            name: "tester_id",
            kind: Kind::Integer {
                minimum: 0,
                maximum: 0x1fff_ffff,
            },
            presence: Presence::Default(Fixed::Integer(TESTER_ID as i64)),
            meaning: "The CAN identifier a Send Location's tester transmits under.",
            applies: Applies::Send,
        },
        Setting {
            name: "ecu_id",
            kind: Kind::Integer {
                minimum: 0,
                maximum: 0x1fff_ffff,
            },
            presence: Presence::Default(Fixed::Integer(ECU_ID as i64)),
            meaning: "The CAN identifier a Receive Location's ECU answers from.",
            applies: Applies::Receive,
        },
        transport::configured::timeout(
            Applies::Both,
            Some(DEFAULT_TIMEOUT),
            "How long a peer that stops mid-transfer is waited on; ISO-TP's own \
             when left out.",
        ),
    ],
};

/// The schemes a Location's address is entered with to choose this technology.
pub const SCHEMES: &[&str] = &["uds"];
