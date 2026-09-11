//! The services of ISO 14229-1 that move data: what a tester asks, and
//! what an ECU answers.
//!
//! A request is a service identifier and its parameters. A positive
//! response echoes the identifier with `0x40` added; a negative one is
//! `0x7f`, the identifier, and a response code that says why. Six services
//! are here: `ReadDataByIdentifier` and `WriteDataByIdentifier` for a data
//! identifier that fits one message, `RoutineControl` for a routine, and
//! `RequestDownload`, `TransferData` and `RequestTransferExit` for a
//! download in blocks — how a tester writes what one message cannot hold.

use transport::error::{Result, protocol_error};

/// `ReadDataByIdentifier`.
pub const READ_DATA_BY_IDENTIFIER: u8 = 0x22;
/// `WriteDataByIdentifier`.
pub const WRITE_DATA_BY_IDENTIFIER: u8 = 0x2e;
/// `RoutineControl`: start, stop or request the results of a routine.
pub const ROUTINE_CONTROL: u8 = 0x31;
/// `RequestDownload`: open a download to a memory address.
pub const REQUEST_DOWNLOAD: u8 = 0x34;
/// `TransferData`: one block of an open download.
pub const TRANSFER_DATA: u8 = 0x36;
/// `RequestTransferExit`: close the download.
pub const REQUEST_TRANSFER_EXIT: u8 = 0x37;
/// What a positive response adds to the service identifier.
pub const POSITIVE: u8 = 0x40;
/// The service identifier of every negative response.
pub const NEGATIVE: u8 = 0x7f;

/// The negative response codes an ECU answers with.
pub mod code {
    /// The service is not one the ECU offers.
    pub const SERVICE_NOT_SUPPORTED: u8 = 0x11;
    /// The sub-function is not one the service offers.
    pub const SUB_FUNCTION_NOT_SUPPORTED: u8 = 0x12;
    /// The request is the wrong length or shape.
    pub const INCORRECT_MESSAGE_LENGTH: u8 = 0x13;
    /// The request came out of order: a transfer with no download open.
    pub const REQUEST_SEQUENCE_ERROR: u8 = 0x24;
    /// A parameter the ECU does not have: an unknown identifier.
    pub const REQUEST_OUT_OF_RANGE: u8 = 0x31;
    /// A block out of sequence.
    pub const WRONG_BLOCK_SEQUENCE_COUNTER: u8 = 0x73;
    /// The answer takes longer; the tester waits for another.
    pub const RESPONSE_PENDING: u8 = 0x78;
}

/// The address-and-length format of a download here: a two-byte memory
/// address and a four-byte size.
const ADDRESS_AND_LENGTH: u8 = 0x42;
/// The length format of a download's answer: a two-byte block length.
const LENGTH_FORMAT: u8 = 0x20;

/// What a tester asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    ReadDataByIdentifier {
        did: u16,
    },
    WriteDataByIdentifier {
        did: u16,
        data: Vec<u8>,
    },
    /// `control` is 1 to start, 2 to stop, 3 for the results.
    RoutineControl {
        control: u8,
        routine: u16,
        data: Vec<u8>,
    },
    /// Open a download of `size` bytes to `address`.
    RequestDownload {
        address: u16,
        size: u32,
    },
    /// Block `block` of the open download; the counter starts at one and
    /// wraps.
    TransferData {
        block: u8,
        data: Vec<u8>,
    },
    RequestTransferExit,
}

/// A negative response: the service refused, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Negative {
    pub service: u8,
    pub code: u8,
}

impl Negative {
    /// The three bytes on the wire.
    #[must_use]
    pub const fn encode(self) -> [u8; 3] {
        [NEGATIVE, self.service, self.code]
    }
}

impl Request {
    /// The service identifier.
    #[must_use]
    pub const fn service(&self) -> u8 {
        match self {
            Self::ReadDataByIdentifier { .. } => READ_DATA_BY_IDENTIFIER,
            Self::WriteDataByIdentifier { .. } => WRITE_DATA_BY_IDENTIFIER,
            Self::RoutineControl { .. } => ROUTINE_CONTROL,
            Self::RequestDownload { .. } => REQUEST_DOWNLOAD,
            Self::TransferData { .. } => TRANSFER_DATA,
            Self::RequestTransferExit => REQUEST_TRANSFER_EXIT,
        }
    }

    /// The bytes on the wire.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![self.service()];
        match self {
            Self::ReadDataByIdentifier { did } => out.extend_from_slice(&did.to_be_bytes()),
            Self::WriteDataByIdentifier { did, data } => {
                out.extend_from_slice(&did.to_be_bytes());
                out.extend_from_slice(data);
            }
            Self::RoutineControl {
                control,
                routine,
                data,
            } => {
                out.push(*control);
                out.extend_from_slice(&routine.to_be_bytes());
                out.extend_from_slice(data);
            }
            Self::RequestDownload { address, size } => {
                out.extend_from_slice(&[0x00, ADDRESS_AND_LENGTH]);
                out.extend_from_slice(&address.to_be_bytes());
                out.extend_from_slice(&size.to_be_bytes());
            }
            Self::TransferData { block, data } => {
                out.push(*block);
                out.extend_from_slice(data);
            }
            Self::RequestTransferExit => {}
        }
        out
    }

    /// The request `bytes` carry, or the negative response that refuses it:
    /// an unknown service, or a shape the service does not take.
    ///
    /// # Errors
    /// The negative response, ready to send.
    pub fn parse(bytes: &[u8]) -> std::result::Result<Self, Negative> {
        let Some((&service, rest)) = bytes.split_first() else {
            return Err(Negative {
                service: 0,
                code: code::INCORRECT_MESSAGE_LENGTH,
            });
        };
        let short = Negative {
            service,
            code: code::INCORRECT_MESSAGE_LENGTH,
        };
        match service {
            READ_DATA_BY_IDENTIFIER => match rest {
                [hi, lo] => Ok(Self::ReadDataByIdentifier {
                    did: u16::from_be_bytes([*hi, *lo]),
                }),
                _ => Err(short),
            },
            WRITE_DATA_BY_IDENTIFIER => match rest {
                [hi, lo, data @ ..] => Ok(Self::WriteDataByIdentifier {
                    did: u16::from_be_bytes([*hi, *lo]),
                    data: data.to_vec(),
                }),
                _ => Err(short),
            },
            ROUTINE_CONTROL => match rest {
                [control @ 1..=3, hi, lo, data @ ..] => Ok(Self::RoutineControl {
                    control: *control,
                    routine: u16::from_be_bytes([*hi, *lo]),
                    data: data.to_vec(),
                }),
                [_, _, _, ..] => Err(Negative {
                    service,
                    code: code::SUB_FUNCTION_NOT_SUPPORTED,
                }),
                _ => Err(short),
            },
            REQUEST_DOWNLOAD => match rest {
                [_, ADDRESS_AND_LENGTH, a, b, s0, s1, s2, s3] => Ok(Self::RequestDownload {
                    address: u16::from_be_bytes([*a, *b]),
                    size: u32::from_be_bytes([*s0, *s1, *s2, *s3]),
                }),
                [_, _, ..] => Err(Negative {
                    service,
                    code: code::REQUEST_OUT_OF_RANGE,
                }),
                _ => Err(short),
            },
            TRANSFER_DATA => match rest {
                [block, data @ ..] => Ok(Self::TransferData {
                    block: *block,
                    data: data.to_vec(),
                }),
                [] => Err(short),
            },
            REQUEST_TRANSFER_EXIT if rest.is_empty() => Ok(Self::RequestTransferExit),
            REQUEST_TRANSFER_EXIT => Err(short),
            _ => Err(Negative {
                service,
                code: code::SERVICE_NOT_SUPPORTED,
            }),
        }
    }
}

/// What an ECU answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    /// The service, as requested, and what follows the identifier.
    Positive {
        service: u8,
        data: Vec<u8>,
    },
    Negative(Negative),
}

impl Response {
    /// The positive answer to a download: the block length the ECU takes,
    /// counting the two bytes of service and counter.
    #[must_use]
    pub fn download(block_length: u16) -> Self {
        let [hi, lo] = block_length.to_be_bytes();
        Self::Positive {
            service: REQUEST_DOWNLOAD,
            data: vec![LENGTH_FORMAT, hi, lo],
        }
    }

    /// The block length a download's positive answer names.
    ///
    /// # Errors
    /// An answer of another shape.
    pub fn block_length(data: &[u8]) -> Result<u16> {
        match data {
            [LENGTH_FORMAT, hi, lo] => Ok(u16::from_be_bytes([*hi, *lo])),
            _ => Err(protocol_error("a download answer without a block length")),
        }
    }

    /// The bytes on the wire.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Self::Positive { service, data } => {
                let mut out = vec![service | POSITIVE];
                out.extend_from_slice(data);
                out
            }
            Self::Negative(negative) => negative.encode().to_vec(),
        }
    }

    /// The response `bytes` carry.
    ///
    /// # Errors
    /// An empty message, or a negative response without its code.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        match bytes {
            [] => Err(protocol_error("an empty response")),
            [NEGATIVE, service, code] => Ok(Self::Negative(Negative {
                service: *service,
                code: *code,
            })),
            [NEGATIVE, ..] => Err(protocol_error("a negative response without its code")),
            [service, data @ ..] if service & POSITIVE != 0 => Ok(Self::Positive {
                service: service & !POSITIVE,
                data: data.to_vec(),
            }),
            _ => Err(protocol_error("a response that is not one")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_request_encodes_and_parses_back() {
        let requests = [
            Request::ReadDataByIdentifier { did: 0xf190 },
            Request::WriteDataByIdentifier {
                did: 0xf190,
                data: b"WVWZZZ".to_vec(),
            },
            Request::RoutineControl {
                control: 1,
                routine: 0xff00,
                data: vec![1, 2],
            },
            Request::RequestDownload {
                address: 0xf190,
                size: 70_000,
            },
            Request::TransferData {
                block: 1,
                data: vec![9; 10],
            },
            Request::RequestTransferExit,
        ];
        for request in requests {
            let bytes = request.encode();
            assert_eq!(bytes[0], request.service());
            assert_eq!(
                Request::parse(&bytes).expect("parse"),
                request,
                "{request:?}"
            );
        }
        assert_eq!(
            Request::RequestDownload {
                address: 0x1234,
                size: 5
            }
            .encode(),
            [0x34, 0x00, 0x42, 0x12, 0x34, 0, 0, 0, 5]
        );
    }

    #[test]
    fn a_request_the_ecu_cannot_take_names_why() {
        let refused = |bytes: &[u8]| Request::parse(bytes).expect_err("refused").code;
        assert_eq!(refused(&[]), code::INCORRECT_MESSAGE_LENGTH);
        assert_eq!(refused(&[0x22, 0xf1]), code::INCORRECT_MESSAGE_LENGTH);
        assert_eq!(refused(&[0x2e, 0xf1]), code::INCORRECT_MESSAGE_LENGTH);
        assert_eq!(
            refused(&[0x31, 9, 0xff, 0x00]),
            code::SUB_FUNCTION_NOT_SUPPORTED
        );
        assert_eq!(
            refused(&[0x34, 0, 0x44, 0, 0, 0, 0]),
            code::REQUEST_OUT_OF_RANGE
        );
        assert_eq!(refused(&[0x36]), code::INCORRECT_MESSAGE_LENGTH);
        assert_eq!(refused(&[0x37, 1]), code::INCORRECT_MESSAGE_LENGTH);
        assert_eq!(refused(&[0x10, 1]), code::SERVICE_NOT_SUPPORTED);
        assert_eq!(
            Negative {
                service: 0x10,
                code: 0x11
            }
            .encode(),
            [0x7f, 0x10, 0x11]
        );
    }

    #[test]
    fn a_response_is_positive_or_says_why_not() {
        let positive = Response::Positive {
            service: READ_DATA_BY_IDENTIFIER,
            data: vec![0xf1, 0x90, 1],
        };
        assert_eq!(positive.encode(), [0x62, 0xf1, 0x90, 1]);
        assert_eq!(
            Response::parse(&[0x62, 0xf1, 0x90, 1]).expect("parse"),
            positive
        );
        let negative = Response::parse(&[0x7f, 0x22, 0x31]).expect("parse");
        assert_eq!(
            negative,
            Response::Negative(Negative {
                service: 0x22,
                code: 0x31
            })
        );
        assert_eq!(negative.encode(), [0x7f, 0x22, 0x31]);
        assert!(Response::parse(&[]).is_err());
        assert!(Response::parse(&[0x7f, 0x22]).is_err());
        assert!(
            Response::parse(&[0x22, 0xf1, 0x90]).is_err(),
            "a request is no response"
        );
        let download = Response::download(4095);
        assert_eq!(download.encode(), [0x74, 0x20, 0x0f, 0xff]);
        assert_eq!(
            Response::block_length(&[0x20, 0x0f, 0xff]).expect("length"),
            4095
        );
        assert!(Response::block_length(&[0x10, 0xff]).is_err());
    }
}
