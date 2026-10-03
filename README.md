# xmip-core-transport-uds

UDS transport: ISO 14229 diagnostics over ISO-TP — a Send Location writes a data identifier, by WriteDataByIdentifier or a download in TransferData blocks; a Receive Location serves one as an ECU would, and the bytes written to it arrive as a Stream. A technology of [xmip-core-transport](https://github.com/IlleNilsson/xmip-core-transport).

A send target is read by `net::Target` in [xmip-core-library-net](https://github.com/IlleNilsson/xmip-core-library-net), the one reading of a URI every technology calls: scheme, authority, path and decoded query. Until 2026-09-28 it was read through the transport capability's `socket::target`, which split it on its first slash and left the query in the path.

A `0x` number in a target is read by `codec::hex::prefixed_number` in [xmip-core-library-codec](https://github.com/IlleNilsson/xmip-core-library-codec), which refuses a sign; until 2026-09-28 it was read with `from_str_radix`, which took `0x+7e8`.

## Acknowledgement

The tester is answered after the whole receive cycle. A Receive Location
serves as the ECU and answers every request as it comes but the one that
completes a write, `WriteDataByIdentifier` or `RequestTransferExit` after a
download, whose response waits for the verdict: the positive response on
Accepted. On Refused it is a negative response the tester does not repeat (ISO
14229-1, Annex A.1): security access denied (0x33) for a tester not identified
or not permitted, request out of range (0x31) for content refused; a write
answered either fails as permanent. On Failed it is the negative response busy,
repeat request (0x21), which fails the tester's write as retryable so it
writes again. Each written Stream
arrives whole.

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
