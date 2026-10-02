# microSD Card: SDMMC Host + FAT File Access

## Hardware

SDMMC 4-bit bus at 20 MHz on GPIO20-25 (see 01-board.md). Power switch
on GPIO39 (active-low — drive LOW to power the card).

## Driver stack

```
esp_hal::sdmmc::SdHostController     — SD host peripheral driver
  └── Slot<0> (async)                — slot with pin muxing
      └── sdio::DefaultBlockDevice   — SD card protocol (CMD0/8/41/2/3...)
          ├── read-only boot inspector — card/partition/volume information
          └── embedded-sdmmc FAT16/32 — track reads and new WAV recording files
```

The `sdio` crate (v0.5) provides the card protocol on top of the
esp-hal slot. Card enumeration is `async` (`.await`).

## Implementation notes

- The controller must outlive the slot — store it in a `StaticCell`
- The slot borrows the controller, so pin muxing happens after creation
- Power the card BEFORE enumeration (`power.set_low()`)
- 128 GB card verified working ( superfloppy FAT32, label "NO NAME")

## Read-only FAT inspector

The demo includes a hand-rolled read-only FAT16/FAT32/exFAT detector:

1. Read block 0 → check for MBR partition table or superfloppy
2. Read the FAT boot sector from the partition LBA
3. Detect FAT type from sectors-per-FAT and total sector count
4. Parse the root directory (FAT16: fixed region; FAT32: cluster chain)
5. Extract volume label, file entries, first .TXT file preview

The boot inspector itself is read-only. The media layer now writes new
recording files and optional demo tracks; existing files are never opened
for writing. It does not format or repartition the card.

`storage.rs` adapts native SDMMC to `embedded-sdmmc`. For superfloppy cards,
`fat_layout.rs` supplies a virtual MBR at logical sector 0 and maps logical
sector 1 to physical sector 0. That MBR exists only in RAM; writes to it are
rejected. A host integration test verifies recording across clusters, reopening,
existing-file preservation, and an unchanged physical boot sector.

See [SD audio and recording](14-sd-audio-recording.md) for controls and limits.

## Key constants

```rust
// 4-bit bus, 20 MHz, no card-detect pin
SdHostController::new(SDHOST, HostConfig::default())
let slot = controller.slot::<0>(SlotConfig::default())?
    .with_clk(gpio24).with_cmd(gpio25)
    .with_data0(gpio20).with_data1(gpio21)
    .with_data2(gpio22).with_data3(gpio23)
    .into_async();

let device = sdio::DefaultBlockDevice::new_sd_card(
    slot, 20_000_000, embassy_time::Delay {}
).await?;
```

## Block device API

The `sdio` crate's `BlockDevice` uses the `block_device_driver` trait:
```rust
dev.read(lba, &mut [Aligned<A4, [u8; 512]>]).await
```

Buffers must be 4-byte aligned (use the `aligned` crate's `Aligned`
wrapper). Read one block at a time for the inspector.
