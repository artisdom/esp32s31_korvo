//! GT1151 capacitive touch controller (4.3" LCD sub-board) over I2C.
//!
//! No INT/RST pins are wired on this board, so the controller is polled.
//! Registers are 16-bit, coordinates big-endian, and every report block
//! (including the trailing checksum byte) sums to zero.

use crate::board::{SharedI2c, GT1151_I2C_ADDRS};

const REG_PRODUCT_ID: u16 = 0x8140;
const REG_POINT: u16 = 0x814E;

#[derive(Clone, Copy, Default)]
pub struct TouchPoint {
    pub x: u16,
    pub y: u16,
    pub strength: u16,
}

pub struct Touch {
    i2c: &'static SharedI2c,
    addr: u8,
    pub product_id: [u8; 4],
    pub last: Option<TouchPoint>,
}

impl Touch {
    /// Probe 0x14 and 0x5D (the two addresses a GT1151 can land on after
    /// different reset strap timings).
    pub fn probe(i2c: &'static SharedI2c) -> Option<Self> {
        for addr in GT1151_I2C_ADDRS {
            let mut id = [0u8; 11];
            if read_regs(i2c, addr, REG_PRODUCT_ID, &mut id).is_err() {
                continue;
            }
            // First bytes must be printable, last byte a valid sensor id.
            let printable = id[0].is_ascii_alphanumeric()
                && id[1].is_ascii_alphanumeric()
                && id[2].is_ascii_alphanumeric()
                && id[10] != 0xff;
            if printable {
                let mut product_id = [0u8; 4];
                product_id.copy_from_slice(&id[0..4]);
                return Some(Self {
                    i2c,
                    addr,
                    product_id,
                    last: None,
                });
            }
        }
        None
    }

    /// Poll the current touch point (single-touch is all the demo needs).
    pub fn poll(&mut self) -> Option<TouchPoint> {
        let mut status = [0u8];
        if read_regs(self.i2c, self.addr, REG_POINT, &mut status).is_err() {
            self.last = None;
            return None;
        }
        let count = status[0] & 0x0f;
        if count == 0 || count > 5 {
            let _ = write_reg(self.i2c, self.addr, REG_POINT, 0);
            self.last = None;
            return None;
        }

        // status byte + per-point record + checksum byte
        let len = 1 + count as usize * 7 + 1;
        let mut buf = [0u8; 1 + 5 * 7 + 1];
        if read_regs(self.i2c, self.addr, REG_POINT, &mut buf[..len]).is_err() {
            self.last = None;
            return None;
        }
        let _ = write_reg(self.i2c, self.addr, REG_POINT, 0);

        let checksum_ok = buf[..len].iter().fold(0u8, |a, x| a.wrapping_add(*x)) == 0;
        if !checksum_ok {
            return self.last;
        }

        let x = u16::from_be_bytes([buf[2], buf[3]]);
        let y = u16::from_be_bytes([buf[4], buf[5]]);
        let strength = u16::from_be_bytes([buf[6], buf[7]]);
        // Clamp to the panel (the controller is configured for 800x480).
        let point = TouchPoint {
            x: x.min(crate::board::LCD_H_RES as u16 - 1),
            y: y.min(crate::board::LCD_V_RES as u16 - 1),
            strength,
        };
        self.last = Some(point);
        self.last
    }
}

fn read_regs(
    i2c: &'static SharedI2c,
    addr: u8,
    reg: u16,
    out: &mut [u8],
) -> Result<(), esp_hal::i2c::master::Error> {
    let reg_be = reg.to_be_bytes();
    i2c.lock(|i2c| i2c.borrow_mut().write_read(addr, &reg_be, out))
}

fn write_reg(
    i2c: &'static SharedI2c,
    addr: u8,
    reg: u16,
    val: u8,
) -> Result<(), esp_hal::i2c::master::Error> {
    let reg_be = reg.to_be_bytes();
    i2c.lock(|i2c| i2c.borrow_mut().write(addr, &[reg_be[0], reg_be[1], val]))
}
