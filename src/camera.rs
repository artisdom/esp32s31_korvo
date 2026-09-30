//! DVP camera sensor probe.
//!
//! Full DVP capture needs a sensor register driver (OV3660/SC101IOT init
//! tables), which doesn't exist in Rust yet. The demo does what a board
//! bring-up would: identify the sensor over SCCB (I2C, 16-bit registers,
//! auto-incremented) and report it. Capture is future work (see README).

use crate::board::{SharedI2c, CAM_SENSORS};

#[derive(Clone, Copy)]
pub struct CameraProbe {
    /// (name, PID) on an exact match, or ("unknown", PID) if some chip
    /// answered with an unexpected id.
    pub sensor: Option<(&'static str, u16)>,
    /// How many candidate addresses acknowledged.
    pub acked: u8,
}

pub fn probe(i2c: &'static SharedI2c) -> CameraProbe {
    let mut out = CameraProbe {
        sensor: None,
        acked: 0,
    };
    let mut unknown: Option<u16> = None;
    for (name, sccb, id_reg, expect) in CAM_SENSORS {
        let mut id = [0u8; 2];
        let reg_be = id_reg.to_be_bytes();
        if i2c
            .lock(|i2c| i2c.borrow_mut().write_read(sccb, &reg_be, &mut id))
            .is_ok()
        {
            out.acked += 1;
            let pid = ((id[0] as u16) << 8) | id[1] as u16;
            if pid == expect {
                out.sensor = Some((name, pid));
                return out;
            }
            if pid != 0 && pid != 0xffff {
                unknown = Some(pid);
            }
        }
    }
    if let Some(pid) = unknown {
        out.sensor = Some(("unknown", pid));
    }
    out
}
