//! FAT adapter for the async native SDMMC card. Creates recordings and deletes explicitly confirmed root files.
use crate::{fat_layout::Layout, sdcard::CardDevice};
use aligned::{A4, Aligned};
use core::{cell::RefCell, fmt, fmt::Write};
use embedded_sdmmc::{
    Block, BlockCount, BlockDevice, BlockIdx, Mode, RawDirectory, RawFile, TimeSource, Timestamp,
    VolumeIdx, VolumeManager,
};

#[derive(Debug)]
pub struct IoError;
impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SDMMC I/O")
    }
}
impl core::error::Error for IoError {}
pub struct Device {
    card: RefCell<&'static mut CardDevice>,
    layout: Layout,
}
impl Device {
    pub fn new(card: &'static mut CardDevice) -> Result<Self, &'static str> {
        let blocks = card.card().csd.block_count() as u32;
        let mut b = Aligned::<A4, _>([0u8; 512]);
        embassy_futures::block_on(block_device_driver::BlockDevice::read(
            card,
            0,
            core::slice::from_mut(&mut b),
        ))
        .map_err(|_| "SD read")?;
        let layout = Layout::detect(&b, blocks);
        Ok(Self {
            card: RefCell::new(card),
            layout,
        })
    }
}
impl BlockDevice for Device {
    type Error = IoError;
    fn read(&self, blocks: &mut [Block], start: BlockIdx) -> Result<(), IoError> {
        for (i, b) in blocks.iter_mut().enumerate() {
            let index = start.0.checked_add(i as u32).ok_or(IoError)?;
            if self.layout.overlay && index == 0 {
                b.contents = self.layout.mbr();
            } else {
                let index = self.layout.physical(index).ok_or(IoError)?;
                let mut aligned = Aligned::<A4, _>([0u8; 512]);
                embassy_futures::block_on(block_device_driver::BlockDevice::read(
                    &mut **self.card.borrow_mut(),
                    index,
                    core::slice::from_mut(&mut aligned),
                ))
                .map_err(|_| IoError)?;
                b.contents.copy_from_slice(&aligned[..]);
            }
        }
        Ok(())
    }
    fn write(&self, blocks: &[Block], start: BlockIdx) -> Result<(), IoError> {
        for (i, b) in blocks.iter().enumerate() {
            let index = start.0.checked_add(i as u32).ok_or(IoError)?;
            if self.layout.overlay && index == 0 {
                return Err(IoError);
            }
            let index = self.layout.physical(index).ok_or(IoError)?;
            let aligned = Aligned::<A4, _>(b.contents);
            embassy_futures::block_on(block_device_driver::BlockDevice::write(
                &mut **self.card.borrow_mut(),
                index,
                core::slice::from_ref(&aligned),
            ))
            .map_err(|_| IoError)?;
        }
        Ok(())
    }
    fn num_blocks(&self) -> Result<BlockCount, IoError> {
        Ok(BlockCount(
            self.layout.blocks + u32::from(self.layout.overlay),
        ))
    }
}
pub struct Clock;
impl TimeSource for Clock {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp {
            year_since_1970: 56,
            zero_indexed_month: 9,
            zero_indexed_day: 1,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}
pub type Name = heapless::String<13>;
pub struct Storage {
    pub fs: VolumeManager<Device, Clock>,
    pub root: RawDirectory,
}
impl Storage {
    pub fn new(card: &'static mut CardDevice) -> Result<Self, &'static str> {
        let fs = VolumeManager::new(Device::new(card)?, Clock);
        let volume = fs.open_raw_volume(VolumeIdx(0)).map_err(|e| {
            esp_println::println!("FAT mount: {:?}", e);
            "FAT16/32 required"
        })?;
        let root = fs.open_root_dir(volume).map_err(|_| "FAT root")?;
        Ok(Self { fs, root })
    }
    pub fn tracks(&self) -> Result<heapless::Vec<Name, 64>, &'static str> {
        self.list_files(1)
    }
    pub fn files(&self) -> Result<heapless::Vec<Name, 64>, &'static str> {
        self.list_files(0)
    }
    pub fn videos(&self) -> Result<heapless::Vec<Name, 64>, &'static str> {
        self.list_files(2)
    }
    fn list_files(&self, filter: u8) -> Result<heapless::Vec<Name, 64>, &'static str> {
        let mut out = heapless::Vec::new();
        self.fs
            .iterate_dir(self.root, |entry| {
                if !entry.attributes.is_directory() && !entry.attributes.is_volume() {
                    let mut name = Name::new();
                    let _ = write!(name, "{}", entry.name);
                    if filter == 0
                        || (filter == 1 && (name.ends_with(".WAV") || name.ends_with(".MP3")))
                        || (filter == 2 && name.ends_with(".AVI"))
                    {
                        let _ = out.push(name);
                    }
                }
                core::ops::ControlFlow::Continue(())
            })
            .map_err(|_| "directory read")?;
        Ok(out)
    }
    pub fn delete(&self, name: &str) -> Result<(), &'static str> {
        let entry = self
            .fs
            .find_directory_entry(self.root, name)
            .map_err(|_| "File no longer available")?;
        if entry.attributes.is_directory() || entry.attributes.is_volume() {
            return Err("Only root files can be deleted");
        }
        self.fs.delete_entry_in_dir(self.root, name).map_err(|e| {
            esp_println::println!("FAT delete {}: {:?}", name, e);
            "Delete failed - RESCAN SD"
        })
    }
    pub fn create_recording(&self) -> Result<(RawFile, Name), &'static str> {
        self.create_numbered("REC", "WAV")
    }
    pub fn create_video(&self) -> Result<(RawFile, Name), &'static str> {
        self.create_numbered("VID", "AVI")
    }
    fn create_numbered(
        &self,
        prefix: &str,
        extension: &str,
    ) -> Result<(RawFile, Name), &'static str> {
        let mut next = 1u32;
        self.fs
            .iterate_dir(self.root, |entry| {
                let mut name = Name::new();
                let _ = write!(name, "{}", entry.name);
                if name.len() == 12 && name.starts_with(prefix) && name[9..] == *extension {
                    if let Ok(index) = name[3..8].parse::<u32>() {
                        next = next.max(index + 1);
                    }
                }
                core::ops::ControlFlow::Continue(())
            })
            .map_err(|_| "Recording directory read failed")?;
        if next > 99999 {
            return Err("recording names full");
        }
        let mut name = Name::new();
        let _ = write!(name, "{}{:05}.{}", prefix, next, extension);
        self.fs
            .open_file_in_dir(self.root, name.as_str(), Mode::ReadWriteCreate)
            .map(|file| (file, name))
            .map_err(|e| {
                esp_println::println!("FAT create: {:?}", e);
                "Cannot create recording"
            })
    }
}
