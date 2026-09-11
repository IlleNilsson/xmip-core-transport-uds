# xmip-core-transport-uds

UDS transport: ISO 14229 diagnostics over ISO-TP — a Send Location writes a data identifier, by WriteDataByIdentifier or a download in TransferData blocks; a Receive Location serves one as an ECU would, and the bytes written to it arrive as a Stream. A technology of [xmip-core-transport](https://github.com/IlleNilsson/xmip-core-transport).

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
