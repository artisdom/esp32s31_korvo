//! Mirror the bootloader's core-zero PMA layout on the newly started S31 core.
//! ROM starts core one directly, bypassing the bootloader's writable PSRAM setup.
//! The register protocol follows ESP-IDF cpu_region_protect.c and riscv/csr.h.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pma {
    config: [u32; 16],
    address: [u32; 16],
}
impl Pma {
    pub fn snapshot() -> Self {
        let mut p = Self {
            config: [0; 16],
            address: [0; 16],
        };
        unsafe {
            core::arch::asm!("csrr {0}, 0xbc0", out(reg) p.config[0]);
            core::arch::asm!("csrr {0}, 0xbd0", out(reg) p.address[0]);
            core::arch::asm!("csrr {0}, 0xbc1", out(reg) p.config[1]);
            core::arch::asm!("csrr {0}, 0xbd1", out(reg) p.address[1]);
            core::arch::asm!("csrr {0}, 0xbc2", out(reg) p.config[2]);
            core::arch::asm!("csrr {0}, 0xbd2", out(reg) p.address[2]);
            core::arch::asm!("csrr {0}, 0xbc3", out(reg) p.config[3]);
            core::arch::asm!("csrr {0}, 0xbd3", out(reg) p.address[3]);
            core::arch::asm!("csrr {0}, 0xbc4", out(reg) p.config[4]);
            core::arch::asm!("csrr {0}, 0xbd4", out(reg) p.address[4]);
            core::arch::asm!("csrr {0}, 0xbc5", out(reg) p.config[5]);
            core::arch::asm!("csrr {0}, 0xbd5", out(reg) p.address[5]);
            core::arch::asm!("csrr {0}, 0xbc6", out(reg) p.config[6]);
            core::arch::asm!("csrr {0}, 0xbd6", out(reg) p.address[6]);
            core::arch::asm!("csrr {0}, 0xbc7", out(reg) p.config[7]);
            core::arch::asm!("csrr {0}, 0xbd7", out(reg) p.address[7]);
            core::arch::asm!("csrr {0}, 0xbc8", out(reg) p.config[8]);
            core::arch::asm!("csrr {0}, 0xbd8", out(reg) p.address[8]);
            core::arch::asm!("csrr {0}, 0xbc9", out(reg) p.config[9]);
            core::arch::asm!("csrr {0}, 0xbd9", out(reg) p.address[9]);
            core::arch::asm!("csrr {0}, 0xbca", out(reg) p.config[10]);
            core::arch::asm!("csrr {0}, 0xbda", out(reg) p.address[10]);
            core::arch::asm!("csrr {0}, 0xbcb", out(reg) p.config[11]);
            core::arch::asm!("csrr {0}, 0xbdb", out(reg) p.address[11]);
            core::arch::asm!("csrr {0}, 0xbcc", out(reg) p.config[12]);
            core::arch::asm!("csrr {0}, 0xbdc", out(reg) p.address[12]);
            core::arch::asm!("csrr {0}, 0xbcd", out(reg) p.config[13]);
            core::arch::asm!("csrr {0}, 0xbdd", out(reg) p.address[13]);
            core::arch::asm!("csrr {0}, 0xbce", out(reg) p.config[14]);
            core::arch::asm!("csrr {0}, 0xbde", out(reg) p.address[14]);
            core::arch::asm!("csrr {0}, 0xbcf", out(reg) p.config[15]);
            core::arch::asm!("csrr {0}, 0xbdf", out(reg) p.address[15]);
        }
        p
    }
    pub fn install(self) {
        let previous = Self::snapshot();
        assert!(
            previous
                .config
                .iter()
                .zip(self.config.iter())
                .all(|(old, new)| old & (1 << 29) == 0 || old == new),
            "core-one PMA entry is locked to a different policy"
        );
        critical_section::with(|_| {
            // Disable every entry before changing TOR boundaries. Enabled PMA
            // entries constrain machine mode even when their lock bit is clear.
            unsafe {
                core::arch::asm!("csrw 0xbcf, zero");
                core::arch::asm!("csrw 0xbce, zero");
                core::arch::asm!("csrw 0xbcd, zero");
                core::arch::asm!("csrw 0xbcc, zero");
                core::arch::asm!("csrw 0xbcb, zero");
                core::arch::asm!("csrw 0xbca, zero");
                core::arch::asm!("csrw 0xbc9, zero");
                core::arch::asm!("csrw 0xbc8, zero");
                core::arch::asm!("csrw 0xbc7, zero");
                core::arch::asm!("csrw 0xbc6, zero");
                core::arch::asm!("csrw 0xbc5, zero");
                core::arch::asm!("csrw 0xbc4, zero");
                core::arch::asm!("csrw 0xbc3, zero");
                core::arch::asm!("csrw 0xbc2, zero");
                core::arch::asm!("csrw 0xbc1, zero");
                core::arch::asm!("csrw 0xbc0, zero");
                core::arch::asm!("csrw 0xbd0, {0}", in(reg) self.address[0]);
                core::arch::asm!("csrw 0xbc0, {0}", in(reg) self.config[0]);
                core::arch::asm!("csrw 0xbd1, {0}", in(reg) self.address[1]);
                core::arch::asm!("csrw 0xbc1, {0}", in(reg) self.config[1]);
                core::arch::asm!("csrw 0xbd2, {0}", in(reg) self.address[2]);
                core::arch::asm!("csrw 0xbc2, {0}", in(reg) self.config[2]);
                core::arch::asm!("csrw 0xbd3, {0}", in(reg) self.address[3]);
                core::arch::asm!("csrw 0xbc3, {0}", in(reg) self.config[3]);
                core::arch::asm!("csrw 0xbd4, {0}", in(reg) self.address[4]);
                core::arch::asm!("csrw 0xbc4, {0}", in(reg) self.config[4]);
                core::arch::asm!("csrw 0xbd5, {0}", in(reg) self.address[5]);
                core::arch::asm!("csrw 0xbc5, {0}", in(reg) self.config[5]);
                core::arch::asm!("csrw 0xbd6, {0}", in(reg) self.address[6]);
                core::arch::asm!("csrw 0xbc6, {0}", in(reg) self.config[6]);
                core::arch::asm!("csrw 0xbd7, {0}", in(reg) self.address[7]);
                core::arch::asm!("csrw 0xbc7, {0}", in(reg) self.config[7]);
                core::arch::asm!("csrw 0xbd8, {0}", in(reg) self.address[8]);
                core::arch::asm!("csrw 0xbc8, {0}", in(reg) self.config[8]);
                core::arch::asm!("csrw 0xbd9, {0}", in(reg) self.address[9]);
                core::arch::asm!("csrw 0xbc9, {0}", in(reg) self.config[9]);
                core::arch::asm!("csrw 0xbda, {0}", in(reg) self.address[10]);
                core::arch::asm!("csrw 0xbca, {0}", in(reg) self.config[10]);
                core::arch::asm!("csrw 0xbdb, {0}", in(reg) self.address[11]);
                core::arch::asm!("csrw 0xbcb, {0}", in(reg) self.config[11]);
                core::arch::asm!("csrw 0xbdc, {0}", in(reg) self.address[12]);
                core::arch::asm!("csrw 0xbcc, {0}", in(reg) self.config[12]);
                core::arch::asm!("csrw 0xbdd, {0}", in(reg) self.address[13]);
                core::arch::asm!("csrw 0xbcd, {0}", in(reg) self.config[13]);
                core::arch::asm!("csrw 0xbde, {0}", in(reg) self.address[14]);
                core::arch::asm!("csrw 0xbce, {0}", in(reg) self.config[14]);
                core::arch::asm!("csrw 0xbdf, {0}", in(reg) self.address[15]);
                core::arch::asm!("csrw 0xbcf, {0}", in(reg) self.config[15]);
                core::arch::asm!("fence iorw, iorw", "fence.i");
            }
        });
        assert_eq!(
            Self::snapshot(),
            self,
            "core-one PMA layout was not installed"
        );
    }
}
