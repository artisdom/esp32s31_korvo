//! microSD card: SDMMC 4-bit host (GPIO20..25, power switch on GPIO39).
//!
//! The esp-hal SDMMC slot implements the async `sdio::MmcBus` protocol;
//! the `sdio` crate turns it into a `block_device_driver::BlockDevice`.
//! Boot inspection shows card information and a root listing. The same native
//! block device is then handed to the FAT media layer for playback and creation
//! of new recordings; existing files are never opened for writing.

use aligned::Aligned;
use esp_hal::{
    gpio::Output,
    peripherals::{GPIO20, GPIO21, GPIO22, GPIO23, GPIO24, GPIO25},
    sdmmc::{Config as HostConfig, SdHostController, SlotConfig},
};
use static_cell::StaticCell;

const BLOCK: usize = 512;

pub struct SdPins {
    pub clk: GPIO24<'static>,
    pub cmd: GPIO25<'static>,
    pub d0: GPIO20<'static>,
    pub d1: GPIO21<'static>,
    pub d2: GPIO22<'static>,
    pub d3: GPIO23<'static>,
}

pub struct SdReport {
    /// Card identification from CMD10/CID (debug-formatted where useful).
    pub card_ok: bool,
    pub capacity_mb: u32,
    pub freq_khz: u32,
    pub manuf_id: u8,
    pub partition: &'static str,
    pub fs_type: heapless::String<8>,
    pub volume_label: heapless::String<12>,
    pub entries: heapless::Vec<DirEntry, 10>,
    pub preview: heapless::String<160>,
    pub error: Option<&'static str>,
}

#[derive(Clone)]
pub struct DirEntry {
    pub name: heapless::String<13>,
    pub size: u32,
    pub is_dir: bool,
}

pub(crate) type CardDevice = sdio::DefaultBlockDevice<
    sdio::sd::Card,
    esp_hal::sdmmc::Slot<'static, 0, esp_hal::Async>,
    embassy_time::Delay,
>;

pub async fn mount_and_inspect(
    sdhost: esp_hal::peripherals::SDHOST<'static>,
    pins: SdPins,
    mut power: Output<'static>,
) -> (SdReport, Option<&'static mut CardDevice>) {
    let mut report = SdReport {
        card_ok: false,
        capacity_mb: 0,
        freq_khz: 0,
        manuf_id: 0,
        partition: "?",
        fs_type: heapless::String::new(),
        volume_label: heapless::String::new(),
        entries: heapless::Vec::new(),
        preview: heapless::String::new(),
        error: None,
    };

    // Card power switch is active-low (GPIO39).
    power.set_low();

    // The controller must outlive the slot we hand to the card stack.
    static CTRL: StaticCell<SdHostController<'static>> = StaticCell::new();
    let controller = match SdHostController::new(sdhost, HostConfig::default()) {
        Ok(c) => CTRL.init(c),
        Err(_) => {
            report.error = Some("host clock");
            return (report, None);
        }
    };

    let slot = match controller.slot::<0>(SlotConfig::default()) {
        Ok(s) => s,
        Err(_) => {
            report.error = Some("slot");
            return (report, None);
        }
    };

    let slot = slot
        .with_clk(pins.clk)
        .with_cmd(pins.cmd)
        .with_data0(pins.d0)
        .with_data1(pins.d1)
        .with_data2(pins.d2)
        .with_data3(pins.d3)
        .into_async();

    static DEVICE: StaticCell<CardDevice> = StaticCell::new();
    let device =
        match sdio::DefaultBlockDevice::new_sd_card(slot, 20_000_000, embassy_time::Delay {}).await
        {
            Ok(d) => DEVICE.init(d),
            Err(_) => {
                report.error = Some("no card");
                return (report, None);
            }
        };

    report.card_ok = true;
    let card = device.card();
    report.capacity_mb = (card.csd.block_count() / 2048) as u32;
    report.freq_khz = device.freq() / 1000;
    report.manuf_id = card.cid.manufacturer_id();

    if let Err(e) = inspect_filesystem(device, &mut report).await {
        report.error = Some(e);
    }
    (report, Some(device))
}

async fn inspect_filesystem(
    dev: &mut CardDevice,
    report: &mut SdReport,
) -> Result<(), &'static str> {
    let mut block0: Aligned<aligned::A4, [u8; BLOCK]> = aligned::Aligned([0u8; BLOCK]);
    read(dev, 0, &mut block0).await?;
    let b: &[u8] = &block0[..];

    let mut fat_lba: u32 = 0;
    if b[510] == 0x55 && b[511] == 0xAA && b[0] != 0xEB && b[0] != 0xE9 {
        // MBR with partition table
        for i in 0..4 {
            let p = &b[446 + i * 16..446 + i * 16 + 16];
            let ptype = p[4];
            if ptype == 0 || ptype == 0xEE {
                continue;
            }
            fat_lba = u32::from_le_bytes([p[8], p[9], p[10], p[11]]);
            report.partition = match ptype {
                0x01 | 0x04 | 0x06 => "FAT12/16",
                0x0B | 0x0C => "FAT32",
                0x07 => "exFAT/NTFS",
                _ => "other",
            };
            break;
        }
    } else {
        // Superfloppy: filesystem starts at block 0.
        report.partition = "superfloppy";
    }

    let mut bs: Aligned<aligned::A4, [u8; BLOCK]> = aligned::Aligned([0u8; BLOCK]);
    read(dev, fat_lba, &mut bs).await?;
    let bs: &[u8] = &bs[..];
    if &bs[82..90] == b"EXFAT   " || &bs[3..8] == b"EXFAT" {
        report.fs_type.push_str("exFAT").ok();
        // exFAT root listing needs the FAT + allocation bitmap dance; skip.
        report
            .preview
            .push_str("exFAT detected (listing not shown)")
            .ok();
        return Ok(());
    }

    let bytes_per_sector = u16::from_le_bytes([bs[11], bs[12]]) as u32;
    let sectors_per_cluster = bs[13] as u32;
    let reserved = u16::from_le_bytes([bs[14], bs[15]]) as u32;
    let num_fats = bs[16] as u32;
    let root_entries = u16::from_le_bytes([bs[17], bs[18]]) as u32;
    let total16 = u16::from_le_bytes([bs[19], bs[20]]) as u32;
    let fat16_sectors = u16::from_le_bytes([bs[22], bs[23]]) as u32;
    let total32 = u32::from_le_bytes([bs[32], bs[33], bs[34], bs[35]]);
    let fat32_sectors = u32::from_le_bytes([bs[36], bs[37], bs[38], bs[39]]);
    let root_cluster32 = u32::from_le_bytes([bs[44], bs[45], bs[46], bs[47]]);

    if bytes_per_sector == 0 || sectors_per_cluster == 0 {
        return Err("bad boot sector");
    }

    let total_sectors = if total16 != 0 { total16 } else { total32 };
    let fat_sectors = if fat16_sectors != 0 {
        fat16_sectors
    } else {
        fat32_sectors
    };
    let is_fat32 = fat16_sectors == 0 && fat32_sectors != 0;
    report
        .fs_type
        .push_str(if is_fat32 { "FAT32" } else { "FAT16" })
        .ok();

    // Volume label: root-dir copy if present, else boot-sector copy.
    let mut label_from_root = false;
    if !is_fat32 {
        // FAT16 root dir follows the FATs.
        let root_lba = fat_lba + reserved + num_fats * fat_sectors;
        let root_blocks = (root_entries * 32 + bytes_per_sector - 1) / bytes_per_sector;
        let mut rb: Aligned<aligned::A4, [u8; BLOCK]> = aligned::Aligned([0u8; BLOCK]);
        for i in 0..root_blocks.min(1) {
            read(dev, root_lba + i, &mut rb).await?;
            parse_dir_block(&rb[..], &mut report.entries);
        }
        // Volume label entry (attr 0x08)
        let mut off = 0;
        while off + 32 <= 512 {
            let e = &rb[..][off..off + 32];
            if e[11] == 0x08 && e[0] != 0xE5 {
                push_trimmed(&mut report.volume_label, &e[0..11]);
                label_from_root = true;
                break;
            }
            off += 32;
        }
    } else {
        // FAT32: walk the root cluster chain.
        let mut cluster = root_cluster32;
        let data_start = fat_lba + reserved + num_fats * fat_sectors;
        let mut guard = 0;
        while cluster >= 2 && cluster < 0x0FFFFFF8 && guard < 8 {
            let lba = data_start + (cluster - 2) * sectors_per_cluster;
            for s in 0..sectors_per_cluster.min(2) {
                let mut cb: Aligned<aligned::A4, [u8; BLOCK]> = aligned::Aligned([0u8; BLOCK]);
                read(dev, lba + s, &mut cb).await?;
                parse_dir_block(&cb[..], &mut report.entries);
                if !label_from_root && report.volume_label.is_empty() {
                    let mut off = 0;
                    while off + 32 <= 512 {
                        let e = &cb[..][off..off + 32];
                        if e[11] == 0x08 && e[0] != 0xE5 {
                            push_trimmed(&mut report.volume_label, &e[0..11]);
                            label_from_root = true;
                            break;
                        }
                        off += 32;
                    }
                }
            }
            cluster = next_cluster(dev, cluster, fat_lba + reserved, is_fat32).await?;
            guard += 1;
        }
    }
    if !label_from_root && report.volume_label.is_empty() {
        push_trimmed(&mut report.volume_label, &bs[71..82]); // BS_VolLab
    }
    let _ = total_sectors;

    // Preview the first small text file found.
    for e in report.entries.iter() {
        if e.is_dir || !e.name.ends_with("TXT") {
            continue;
        }
        // Read the file's first directory entry again for the first cluster:
        // we stored only the name; re-scan is overkill — instead read block 0
        // of the data region heuristic is wrong. Skip preview unless the file
        // was found in the single parsed block (see parse_dir_block cache).
        if let Some((_, _, cluster)) = LAST_TXT.lock(|c| c.get()) {
            let data_start = fat_lba + reserved + num_fats * fat_sectors;
            let lba = data_start + (cluster - 2) * sectors_per_cluster;
            let mut fb: Aligned<aligned::A4, [u8; BLOCK]> = aligned::Aligned([0u8; BLOCK]);
            read(dev, lba, &mut fb).await?;
            for &byte in fb.iter() {
                if byte == 0 {
                    break;
                }
                if byte.is_ascii_graphic() || byte == b' ' || byte == b'\n' || byte == b'\r' {
                    if report.preview.push(byte as char).is_err() {
                        break;
                    }
                }
            }
        }
        break;
    }

    Ok(())
}

/// Remembers the first .TXT entry (offset, size, first cluster) from the
/// last parsed directory block so the preview code can find it again.
static LAST_TXT: embassy_sync::blocking_mutex::Mutex<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    core::cell::Cell<Option<(u32, u32, u32)>>,
> = embassy_sync::blocking_mutex::Mutex::new(core::cell::Cell::new(None));

fn parse_dir_block(block: &[u8], out: &mut heapless::Vec<DirEntry, 10>) {
    let mut off = 0;
    while off + 32 <= block.len() {
        let e = &block[off..off + 32];
        let first = e[0];
        if first == 0x00 {
            break; // end of directory
        }
        if first == 0xE5 || (e[11] & 0x3F) == 0x0F || (e[11] & 0x08) != 0 {
            off += 32;
            continue; // deleted, LFN, or volume label
        }
        let mut name = heapless::String::new();
        let base = &e[0..8];
        let ext = &e[8..11];
        for &c in base.iter() {
            if c == b' ' {
                break;
            }
            name.push(c as char).ok();
        }
        if ext[0] != b' ' {
            name.push('.').ok();
            for &c in ext.iter() {
                if c == b' ' {
                    break;
                }
                name.push(c as char).ok();
            }
        }
        let cluster = u16::from_le_bytes([e[26], e[27]]) as u32
            | (u16::from_le_bytes([e[20], e[21]]) as u32) << 16;
        let size = u32::from_le_bytes([e[28], e[29], e[30], e[31]]);
        let is_dir = e[11] & 0x10 != 0;
        if name.ends_with("TXT") {
            LAST_TXT.lock(|c| c.set(Some((1, size, cluster))));
        }
        if out.len() < out.capacity() {
            out.push(DirEntry { name, size, is_dir }).ok();
        }
        off += 32;
    }
}

async fn next_cluster(
    dev: &mut CardDevice,
    cluster: u32,
    fat_lba: u32,
    fat32: bool,
) -> Result<u32, &'static str> {
    if fat32 {
        let idx = cluster as usize / 128;
        let mut fb: Aligned<aligned::A4, [u8; BLOCK]> = aligned::Aligned([0u8; BLOCK]);
        read(dev, fat_lba + idx as u32, &mut fb).await?;
        let off = (cluster as usize % 128) * 4;
        Ok(u32::from_le_bytes([fb[off], fb[off + 1], fb[off + 2], fb[off + 3]]) & 0x0FFF_FFFF)
    } else {
        let idx = cluster as usize / 256;
        let mut fb: Aligned<aligned::A4, [u8; BLOCK]> = aligned::Aligned([0u8; BLOCK]);
        read(dev, fat_lba + idx as u32, &mut fb).await?;
        let off = (cluster as usize % 256) * 2;
        Ok(u16::from_le_bytes([fb[off], fb[off + 1]]) as u32)
    }
}

async fn read(
    dev: &mut CardDevice,
    lba: u32,
    block: &mut Aligned<aligned::A4, [u8; BLOCK]>,
) -> Result<(), &'static str> {
    use block_device_driver::BlockDevice as _;
    dev.read(lba, core::slice::from_mut(block))
        .await
        .map_err(|_| "block read")
}

fn push_trimmed(dst: &mut heapless::String<12>, src: &[u8]) {
    for &c in src.iter() {
        if c.is_ascii_graphic() || c == b' ' {
            dst.push(c as char).ok();
        }
    }
    while dst.ends_with(' ') {
        dst.pop();
    }
}
