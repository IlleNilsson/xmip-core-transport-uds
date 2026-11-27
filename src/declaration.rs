//! This technology's declaration: the transport guide's "The declaration",
//! `module/core/capability/transport/doc/adding-a-transport.md`. The data
//! identifier a Location takes unless told otherwise is declared here, and
//! the code takes it from here; the identifiers and the wait are ISO-TP's,
//! taken by that crate's own `declaration` feature.

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

/// This technology's schemes, as [`transport::Configured::SCHEMES`] defines
/// them.
pub const SCHEMES: &[&str] = &["uds"];
