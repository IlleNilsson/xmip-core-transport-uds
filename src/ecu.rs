//! One electronic control unit's worth of ISO 14229: the identifiers it
//! holds, the routines it runs, and the download it has open.
//!
//! An ECU answers every request with exactly one response. A write to a
//! data identifier that fits one message lands at once; a longer one is
//! opened with `RequestDownload` to the identifier's address, arrives in
//! `TransferData` blocks the ECU sized, and lands on `RequestTransferExit`.
//! Either way the ECU says which identifier was written, so whoever serves
//! as it can hand the bytes on as a Stream.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use crate::service::{Negative, Request, Response, code};

/// The block length an ECU takes unless told otherwise: what one ISO-TP
/// message holds, counting the service and the counter.
pub const MAX_BLOCK_LENGTH: u16 = 4095;

/// A download in progress.
struct Download {
    address: u16,
    size: u32,
    expected: u8,
    bytes: Vec<u8>,
}

/// An ECU: what it holds by data identifier, and the download it has open.
pub struct Ecu {
    identifiers: Mutex<HashMap<u16, Vec<u8>>>,
    block_length: u16,
    download: Mutex<Option<Download>>,
}

impl Default for Ecu {
    fn default() -> Self {
        Self::new()
    }
}

impl Ecu {
    /// An ECU holding nothing, taking blocks of [`MAX_BLOCK_LENGTH`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            identifiers: Mutex::new(HashMap::new()),
            block_length: MAX_BLOCK_LENGTH,
            download: Mutex::new(None),
        }
    }

    /// Hold `bytes` at `did`.
    #[must_use]
    pub fn holding(self, did: u16, bytes: &[u8]) -> Self {
        self.identifiers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(did, bytes.to_vec());
        self
    }

    /// Take download blocks of `block_length`, service and counter counted;
    /// three is the least that carries a byte.
    #[must_use]
    pub const fn in_blocks_of(mut self, block_length: u16) -> Self {
        self.block_length = if block_length < 3 { 3 } else { block_length };
        self
    }

    /// What `did` holds.
    #[must_use]
    pub fn held(&self, did: u16) -> Option<Vec<u8>> {
        self.identifiers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&did)
            .cloned()
    }

    /// The one response to `request`, and the identifier a write to it has
    /// just completed, if one has.
    #[must_use]
    pub fn answer(&self, request: &[u8]) -> (Response, Option<u16>) {
        let request = match Request::parse(request) {
            Ok(request) => request,
            Err(negative) => return (Response::Negative(negative), None),
        };
        let service = request.service();
        let refuse = |code| (Response::Negative(Negative { service, code }), None);
        match request {
            Request::ReadDataByIdentifier { did } => match self.held(did) {
                Some(bytes) => (positive(service, &did.to_be_bytes(), &bytes), None),
                None => refuse(code::REQUEST_OUT_OF_RANGE),
            },
            Request::WriteDataByIdentifier { did, data } => {
                self.store(did, data);
                (positive(service, &did.to_be_bytes(), &[]), Some(did))
            }
            Request::RoutineControl {
                control, routine, ..
            } => (positive(service, &[control], &routine.to_be_bytes()), None),
            Request::RequestDownload { address, size } => {
                *self.download.lock().unwrap_or_else(PoisonError::into_inner) = Some(Download {
                    address,
                    size,
                    expected: 1,
                    bytes: Vec::with_capacity(usize::try_from(size).unwrap_or(0)),
                });
                (Response::download(self.block_length), None)
            }
            Request::TransferData { block, data } => match self.take_block(block, &data) {
                Ok(()) => (positive(service, &[block], &[]), None),
                Err(code) => refuse(code),
            },
            Request::RequestTransferExit => match self.close_download() {
                Ok(address) => (positive(service, &[], &[]), Some(address)),
                Err(code) => refuse(code),
            },
        }
    }

    fn store(&self, did: u16, bytes: Vec<u8>) {
        self.identifiers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(did, bytes);
    }

    fn take_block(&self, block: u8, data: &[u8]) -> std::result::Result<(), u8> {
        let mut open = self.download.lock().unwrap_or_else(PoisonError::into_inner);
        let download = open.as_mut().ok_or(code::REQUEST_SEQUENCE_ERROR)?;
        if block != download.expected {
            return Err(code::WRONG_BLOCK_SEQUENCE_COUNTER);
        }
        if data.len() + 2 > usize::from(self.block_length) {
            return Err(code::INCORRECT_MESSAGE_LENGTH);
        }
        if download.bytes.len() + data.len() > usize::try_from(download.size).unwrap_or(0) {
            return Err(code::REQUEST_OUT_OF_RANGE);
        }
        download.bytes.extend_from_slice(data);
        download.expected = download.expected.wrapping_add(1);
        Ok(())
    }

    fn close_download(&self) -> std::result::Result<u16, u8> {
        let download = self
            .download
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .ok_or(code::REQUEST_SEQUENCE_ERROR)?;
        if download.bytes.len() != usize::try_from(download.size).unwrap_or(0) {
            return Err(code::REQUEST_SEQUENCE_ERROR);
        }
        self.store(download.address, download.bytes);
        Ok(download.address)
    }
}

fn positive(service: u8, head: &[u8], tail: &[u8]) -> Response {
    let mut data = head.to_vec();
    data.extend_from_slice(tail);
    Response::Positive { service, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answered(ecu: &Ecu, request: &Request) -> (Vec<u8>, Option<u16>) {
        let (response, done) = ecu.answer(&request.encode());
        (response.encode(), done)
    }

    #[test]
    fn a_write_lands_at_once_and_reads_back() {
        let ecu = Ecu::new().holding(0xf190, b"WVW");
        let read = Request::ReadDataByIdentifier { did: 0xf190 };
        assert_eq!(answered(&ecu, &read), (b"\x62\xf1\x90WVW".to_vec(), None));
        let write = Request::WriteDataByIdentifier {
            did: 0xf190,
            data: b"XYZ".to_vec(),
        };
        assert_eq!(
            answered(&ecu, &write),
            (vec![0x6e, 0xf1, 0x90], Some(0xf190))
        );
        assert_eq!(ecu.held(0xf190).expect("held"), b"XYZ");
        let unknown = Request::ReadDataByIdentifier { did: 0x0001 };
        assert_eq!(answered(&ecu, &unknown), (vec![0x7f, 0x22, 0x31], None));
        let routine = Request::RoutineControl {
            control: 1,
            routine: 0xff00,
            data: vec![],
        };
        assert_eq!(answered(&ecu, &routine), (vec![0x71, 1, 0xff, 0x00], None));
        assert_eq!(ecu.answer(&[0x10, 1]).0.encode(), [0x7f, 0x10, 0x11]);
    }

    #[test]
    fn a_download_lands_on_exit_in_the_blocks_the_ecu_sized() {
        let ecu = Ecu::new().in_blocks_of(6);
        let open = Request::RequestDownload {
            address: 0x0100,
            size: 10,
        };
        assert_eq!(answered(&ecu, &open), (vec![0x74, 0x20, 0, 6], None));
        for (block, data) in [(1u8, &b"abcd"[..]), (2, b"efgh"), (3, b"ij")] {
            let transfer = Request::TransferData {
                block,
                data: data.to_vec(),
            };
            assert_eq!(answered(&ecu, &transfer), (vec![0x76, block], None));
        }
        assert_eq!(
            answered(&ecu, &Request::RequestTransferExit),
            (vec![0x77], Some(0x0100))
        );
        assert_eq!(ecu.held(0x0100).expect("held"), b"abcdefghij");
    }

    #[test]
    fn a_download_out_of_order_is_refused_with_the_code_that_says_so() {
        let ecu = Ecu::default().in_blocks_of(6);
        let transfer = |block, data: &[u8]| Request::TransferData {
            block,
            data: data.to_vec(),
        };
        assert_eq!(answered(&ecu, &transfer(1, b"ab")).0, [0x7f, 0x36, 0x24]);
        assert_eq!(
            answered(&ecu, &Request::RequestTransferExit).0,
            [0x7f, 0x37, 0x24]
        );
        let open = Request::RequestDownload {
            address: 0x0100,
            size: 4,
        };
        answered(&ecu, &open);
        assert_eq!(answered(&ecu, &transfer(2, b"ab")).0, [0x7f, 0x36, 0x73]);
        assert_eq!(answered(&ecu, &transfer(1, b"abcde")).0, [0x7f, 0x36, 0x13]);
        assert_eq!(answered(&ecu, &transfer(1, b"abc")).0, [0x76, 1]);
        assert_eq!(answered(&ecu, &transfer(2, b"de")).0, [0x7f, 0x36, 0x31]);
        assert_eq!(
            answered(&ecu, &Request::RequestTransferExit).0,
            [0x7f, 0x37, 0x24]
        );
        assert!(ecu.held(0x0100).is_none(), "a short download does not land");
    }
}
