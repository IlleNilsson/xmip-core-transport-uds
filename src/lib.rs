#![forbid(unsafe_code)]

//! Streams written to an ECU's data identifier, the way ISO 14229 writes
//! them.
//!
//! UDS is the diagnostic session of every modern vehicle: a tester asks a
//! service of an ECU and the ECU answers, positively with the service
//! identifier plus `0x40`, negatively with `0x7f` and a code. A Send
//! Location writes a Stream to a data identifier — `WriteDataByIdentifier`
//! when the Stream fits one message, a download in `TransferData` blocks
//! between `RequestDownload` and `RequestTransferExit` when it does not. A
//! Receive Location serves a data identifier as an ECU would, and the
//! bytes written to it arrive as a Stream. There is no ceiling: a download
//! is as long as its blocks.
//!
//! The carrier is [`iso_tp`](iso_tp): each request and each response is
//! one ISO-TP message, segmented and reassembled there. A tester and an ECU
//! are two nodes on one bus — in process, the SDK's simulated one — so they
//! round-trip with no hardware, which is what [`UdsTransport::loopback`]
//! stands up (ADR-0051).
//!
//! **The tester is answered after the whole receive cycle.** The request
//! that completes a write waits for its response until the cycle has
//! ended: the positive response on [`transport::Verdict::Accepted`]; on
//! [`transport::Verdict::Refused`] a negative response the tester does not
//! repeat (ISO 14229-1, Annex A.1) — *security access denied*
//! ([`code::SECURITY_ACCESS_DENIED`]) for a tester not identified or not
//! permitted, *request out of range* ([`code::REQUEST_OUT_OF_RANGE`]) for
//! content refused; the negative response *busy, repeat request*
//! ([`code::BUSY_REPEAT_REQUEST`]) on [`transport::Verdict::Failed`], which
//! fails the tester's write as retryable so it writes again. Each written Stream arrives whole.
//!
//! The origin URI names the identifier that was written:
//! `uds://<bus>/0x<did>`.

pub mod ecu;
pub mod loopback;
pub mod service;

use std::sync::Arc;

use can_bus::loopback::Session;
use codec::hex::prefixed_number;
use iso_tp::{ECU_ID, IsoTpTransport, TESTER_ID};
use net::Target;
use transport::error::{Result, TransportError, protocol_error};
use transport::standing::Standing;
use transport::{Acknowledgement, Arrived, Configured, Directions, Refusal, Transport, Verdict};
use xcore::settings::{Applies, Fixed, Kind, Presence, Setting, Settings};

pub use ecu::{Ecu, MAX_BLOCK_LENGTH};
pub use service::{Negative, Request, Response, code};

/// The data identifier a Location writes or serves unless told otherwise:
/// the vehicle identification number of ISO 14229-1 annex C.
pub const DEFAULT_DID: u16 = 0xf190;

/// One end of a diagnostic session: a tester when it sends, an ECU when it
/// receives.
#[derive(Clone)]
pub struct UdsTransport {
    link: IsoTpTransport,
    did: u16,
    ecu: Arc<Ecu>,
    standing: Standing<Session>,
}

impl UdsTransport {
    /// A session over `link`, writing and serving [`DEFAULT_DID`] as an
    /// ECU holding nothing.
    #[must_use]
    pub fn new(link: IsoTpTransport) -> Self {
        Self {
            link,
            did: DEFAULT_DID,
            ecu: Arc::new(Ecu::new()),
            standing: Standing::default(),
        }
    }

    /// Write to and serve `did`.
    #[must_use]
    pub const fn at(mut self, did: u16) -> Self {
        self.did = did;
        self
    }

    /// Serve as `ecu` when receiving.
    #[must_use]
    pub fn serving(mut self, ecu: Ecu) -> Self {
        self.ecu = Arc::new(ecu);
        self
    }

    /// The ECU this end serves as.
    #[must_use]
    pub fn ecu(&self) -> &Ecu {
        &self.ecu
    }

    /// Ask `request` of the ECU and take the data of its positive answer.
    ///
    /// # Errors
    /// A negative response, an answer to another service, or a link that
    /// failed.
    pub fn exchange(&self, request: &Request) -> Result<Vec<u8>> {
        self.link.deliver(&request.encode())?;
        loop {
            match Response::parse(&self.link.collect()?.bytes)? {
                Response::Positive { service, data } if service == request.service() => {
                    return Ok(data);
                }
                Response::Positive { service, .. } => {
                    return Err(protocol_error(format!(
                        "an answer to service {service:#04x}, not {:#04x}",
                        request.service()
                    )));
                }
                Response::Negative(Negative {
                    code: code::RESPONSE_PENDING,
                    ..
                }) => {}
                Response::Negative(Negative {
                    code: code::BUSY_REPEAT_REQUEST,
                    service,
                }) => {
                    return Err(TransportError::retryable(format!(
                        "service {service:#04x}: the ECU was busy, repeat the request"
                    )));
                }
                Response::Negative(negative) => {
                    return Err(protocol_error(format!(
                        "service {:#04x} refused with code {:#04x}",
                        negative.service, negative.code
                    )));
                }
            }
        }
    }

    /// Read what the ECU holds at `did`.
    ///
    /// # Errors
    /// As [`Self::exchange`].
    pub fn read(&self, did: u16) -> Result<Vec<u8>> {
        let data = self.exchange(&Request::ReadDataByIdentifier { did })?;
        Ok(data.get(2..).unwrap_or(&[]).to_vec())
    }

    /// Write `bytes` to `did`: in one message when they fit, as a download
    /// in the blocks the ECU sizes when they do not.
    ///
    /// # Errors
    /// As [`Self::exchange`].
    pub fn write(&self, did: u16, bytes: &[u8]) -> Result<()> {
        if bytes.len() + 3 <= self.link.ceiling() {
            let write = Request::WriteDataByIdentifier {
                did,
                data: bytes.to_vec(),
            };
            return self.exchange(&write).map(drop);
        }
        let open = Request::RequestDownload {
            address: did,
            size: u32::try_from(bytes.len())
                .map_err(|_| protocol_error("a download longer than four bytes can size"))?,
        };
        let block_length = usize::from(Response::block_length(&self.exchange(&open)?)?);
        if block_length < 3 {
            return Err(protocol_error("a block length that holds no data"));
        }
        let mut block: u8 = 1;
        for data in bytes.chunks(block_length - 2) {
            let transfer = Request::TransferData {
                block,
                data: data.to_vec(),
            };
            self.exchange(&transfer)?;
            block = block.wrapping_add(1);
        }
        self.exchange(&Request::RequestTransferExit).map(drop)
    }

    /// Start, stop or take the results of `routine`.
    ///
    /// # Errors
    /// As [`Self::exchange`].
    pub fn routine(&self, control: u8, routine: u16, data: &[u8]) -> Result<Vec<u8>> {
        let request = Request::RoutineControl {
            control,
            routine,
            data: data.to_vec(),
        };
        let answer = self.exchange(&request)?;
        Ok(answer.get(3..).unwrap_or(&[]).to_vec())
    }

    /// Serve as the ECU until a tester completes a write: what it wrote,
    /// as a Stream, whole. Every request is answered as it comes but the
    /// one that completes the write — `WriteDataByIdentifier`, or
    /// `RequestTransferExit` after a download — whose response is the
    /// arrival's verdict: the positive response on accepted; the negative
    /// response *security access denied* or *request out of range* on
    /// refused, which the tester does not write again; the negative
    /// response *busy, repeat request* on failed, after which it writes
    /// again.
    ///
    /// # Errors
    /// A tester that stops, hangs up, or a link that failed.
    pub fn serve(&self) -> Result<Arrived> {
        loop {
            let request = self.link.collect()?;
            let Some(&service) = request.bytes.first() else {
                return Err(protocol_error("the tester hung up"));
            };
            let (response, written) = self.ecu.answer(&request.bytes);
            let Some(did) = written else {
                self.link.deliver(&response.encode())?;
                continue;
            };
            let bus = iso_tp::bus_of(&request.origin_uri);
            let bytes = self.ecu.held(did).unwrap_or_default();
            let link = self.link.clone();
            let positive = response.encode();
            let acknowledgement = Acknowledgement::deferred(move |verdict| {
                let code = match verdict {
                    Verdict::Accepted => return link.deliver(&positive),
                    Verdict::Refused(Refusal::Unidentified | Refusal::Forbidden) => {
                        code::SECURITY_ACCESS_DENIED
                    }
                    Verdict::Refused(Refusal::Unacceptable) => code::REQUEST_OUT_OF_RANGE,
                    Verdict::Failed => code::BUSY_REPEAT_REQUEST,
                };
                link.deliver(&Negative { service, code }.encode())
            });
            return Ok(Arrived::whole(
                format!("uds://{bus}/{did:#06x}"),
                bytes,
                acknowledgement,
            ));
        }
    }

    /// `uds://<bus>/0x<did>`: what `target` overrides of this end's
    /// identifier.
    fn addressed(&self, target: &str) -> Result<u16> {
        let Some((_, path)) =
            Target::under(&["uds"], target).map(|named| (named.authority(), named.path()))
        else {
            return Ok(self.did);
        };
        if path.is_empty() {
            return Ok(self.did);
        }
        prefixed_number(path)
            .map_err(|_| protocol_error(format!("{path} is not a data identifier")))
    }
}

impl Transport for UdsTransport {
    fn name(&self) -> &'static str {
        "uds"
    }

    fn directions(&self) -> Directions {
        Directions::BOTH
    }

    fn arrivals(&self) -> transport::Arrivals {
        transport::Arrivals::Ordered("one line or bus, answered in the order it speaks")
    }

    /// Serve as the ECU: one Stream per completed write, whose response
    /// waits for the receive cycle — positive on accepted, *busy, repeat
    /// request* on refused.
    fn receive(&self) -> Result<Vec<Arrived>> {
        Ok(vec![self.serve()?])
    }

    /// Write as the tester; `target` may name the identifier.
    fn send(&self, target: &str, bytes: &[u8]) -> Result<()> {
        self.write(self.addressed(target)?, bytes)
    }
}

impl Configured for UdsTransport {
    /// The address names the bus, as can-bus opens it: the kernel interface,
    /// `can0`, where the build has one; a simulated bus of that name
    /// otherwise.
    const SETTINGS: &'static Settings = &Settings {
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
            Setting {
                name: "timeout",
                kind: Kind::Duration,
                presence: Presence::Optional,
                meaning: "How long a peer that stops mid-transfer is waited on; ISO-TP's own \
                          when left out.",
                applies: Applies::Both,
            },
        ],
    };

    fn configured(address: &str, settings: &xcore::settings::Read) -> Result<Self> {
        let node = can_bus::open_bus(address)?;
        // Each side's identifier is read on its own side only: the tester's
        // where it sends, the ECU's where it receives.
        let id = settings
            .optional_integer("tester_id")
            .or_else(|| settings.optional_integer("ecu_id"))
            .unwrap_or_default();
        let id = u32::try_from(id).map_err(|_| protocol_error("a CAN identifier out of range"))?;
        let mut link = IsoTpTransport::new(Arc::clone(&node), node, id);
        if let Some(timeout) = settings.optional_duration("timeout") {
            link = link.timing_out_after(timeout);
        }
        let did = u16::try_from(settings.integer("did"))
            .map_err(|_| protocol_error("a data identifier over 0xffff"))?;
        Ok(Self::new(link).at(did))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use xcore::settings::Given;

    #[test]
    fn uds_declares_its_settings_and_reads_through_them() {
        assert_eq!(UdsTransport::SETTINGS.problems(), Vec::<String>::new());
        let given = [("did".to_string(), Given::Integer(0xf1a0))];
        let built = UdsTransport::open("vcan0", Applies::Send, &given).expect("built");
        assert_eq!(built.did, 0xf1a0);
        let served = UdsTransport::open("vcan0", Applies::Receive, &[]).expect("built");
        assert_eq!(served.did, DEFAULT_DID);
        let tester = [("tester_id".to_string(), Given::Integer(0x7e1))];
        let Err(refused) = UdsTransport::open("vcan0", Applies::Receive, &tester) else {
            panic!("an ECU reads no tester identifier");
        };
        assert!(refused.message.contains("tester_id"), "{}", refused.message);
    }

    /// A tester and an ECU, nodes on one simulated bus, on this thread: what the
    /// tester asks sits on the bus until the ECU is asked to serve.
    fn pair() -> (UdsTransport, UdsTransport) {
        let Session {
            near: at_tester,
            far: at_ecu,
        } = Session::fresh();
        let quick = Duration::from_millis(20);
        let tester = IsoTpTransport::new(Arc::clone(&at_tester), at_tester, TESTER_ID)
            .timing_out_after(quick);
        let ecu = IsoTpTransport::new(Arc::clone(&at_ecu), at_ecu, ECU_ID).timing_out_after(quick);
        (UdsTransport::new(tester), UdsTransport::new(ecu))
    }

    #[test]
    fn a_negative_response_names_the_service_and_the_code() {
        let (tester, ecu) = pair();
        tester
            .link
            .deliver(&Request::ReadDataByIdentifier { did: 0x0001 }.encode())
            .expect("asking");
        let (response, written) = ecu
            .ecu()
            .answer(&ecu.link.collect().expect("request").bytes);
        assert!(written.is_none());
        ecu.link.deliver(&response.encode()).expect("answering");
        let error = tester.read(0x0001).expect_err("unknown");
        assert!(
            error.message.contains("0x22") && error.message.contains("0x31"),
            "{error}"
        );
        assert!(tester.read(0x0001).is_err(), "nobody answers");
    }

    #[test]
    fn a_write_is_refused_for_good_or_answered_busy_and_written_again_then_positively() {
        let Session {
            near: at_tester,
            far: at_ecu,
        } = Session::fresh();
        let wait = Duration::from_secs(2);
        let tester = UdsTransport::new(
            IsoTpTransport::new(Arc::clone(&at_tester), at_tester, TESTER_ID)
                .timing_out_after(wait),
        );
        let ecu = UdsTransport::new(
            IsoTpTransport::new(Arc::clone(&at_ecu), at_ecu, ECU_ID).timing_out_after(wait),
        );
        let writing = std::thread::spawn(move || {
            let forbidden = tester.write(0xf190, b"R1").expect_err("denied");
            let unacceptable = tester.write(0xf190, b"R2").expect_err("out of range");
            let failed = tester.write(0xf190, b"C1").expect_err("busy");
            tester.write(0xf190, b"C1")?;
            Ok::<_, TransportError>([forbidden, unacceptable, failed])
        });
        let forbidden = ecu.receive().expect("first").remove(0);
        assert!(forbidden.defers(), "the tester waits for its response");
        forbidden
            .refused(transport::Refusal::Forbidden)
            .expect("denied");
        let unacceptable = ecu.receive().expect("second").remove(0);
        unacceptable
            .refused(transport::Refusal::Unacceptable)
            .expect("out of range");
        let failed = ecu.receive().expect("third").remove(0);
        failed.failed().expect("busy");
        let again = ecu.receive().expect("again").remove(0);
        assert_eq!(again.taken().expect("answered").bytes, b"C1");
        let [forbidden, unacceptable, failed] =
            writing.join().expect("thread").expect("written again");
        for (refused, code) in [(forbidden, "0x33"), (unacceptable, "0x31")] {
            assert!(!refused.retryable, "{refused}");
            assert!(refused.message.contains(code), "{refused}");
        }
        assert!(failed.retryable, "{failed}");
    }

    #[test]
    fn a_pending_answer_is_waited_for_and_a_wrong_one_refused() {
        let (tester, ecu) = pair();
        let pending = Negative {
            service: 0x22,
            code: code::RESPONSE_PENDING,
        };
        ecu.link.deliver(&pending.encode()).expect("pending");
        ecu.link.deliver(&[0x62, 0xf1, 0x90, 7]).expect("answer");
        assert_eq!(tester.read(0xf190).expect("read"), [7]);
        ecu.link
            .deliver(&[0x6e, 0xf1, 0x90])
            .expect("a write's answer");
        let error = tester.read(0xf190).expect_err("another service");
        assert!(error.message.contains("0x2e"), "{error}");
        tester.link.deliver(&[]).expect("hang up");
        let error = ecu.serve().expect_err("the tester hung up");
        assert!(error.message.contains("hung up"), "{error}");
    }

    #[test]
    fn a_target_names_the_identifier_and_the_ends_name_themselves() {
        let (tester, _) = pair();
        assert_eq!(tester.addressed("uds://can0/0xf1a0").expect("did"), 0xf1a0);
        assert_eq!(
            tester.addressed("uds://can0/").expect("default"),
            DEFAULT_DID
        );
        assert_eq!(tester.addressed("elsewhere").expect("default"), DEFAULT_DID);
        assert!(tester.addressed("uds://can0/vin").is_err());
        assert_eq!(tester.name(), "uds");
        assert!(tester.claims().is_none());
        assert!(tester.directions().receives() && tester.directions().sends());
        assert_eq!(tester.at(0x0100).did, 0x0100);
    }
}
