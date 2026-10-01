# microSD Card: SDMMC Host + FAT Inspection

## Hardware

SDMMC 4-bit bus at 20 MHz on GPIO20-25 (see 01-board.md). Power switch
on GPIO39 (active-low — drive LOW to power the card).

## Driver stack

```
esp_hal::sdmmc::SdHostController     — SD host peripheral driver
  └── Slot<0> (async)                — slot with pin muxing
      └── sdio::DefaultBlockDevice   — SD card protocol (CMD0/8/41/2/3...)
          └── read-only FAT inspector — MBR/boot-sector/dir parsing
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

**Never writes to the card** — safe for user media.

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
