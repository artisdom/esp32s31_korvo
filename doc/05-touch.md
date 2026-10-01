# GT1151 Capacitive Touch Controller

## Physical connection

- I2C bus shared with codec (GPIO0/GPIO1)
- **No INT or RST pins wired** — polling only
- Touch works independently of the LCD state

## I2C protocol

**Probe addresses**: try 0x14 first (answers on this board), then 0x5D.
16-bit **big-endian** register addresses.

### Product ID (register 0x8140)

11 bytes; first 4 must be alphanumeric, byte 10 ≠ 0xFF.
This unit reports "1158".

### Point data (register 0x814E)

```
Byte 0: status
  bits [3:0] = point count (0-5)
  bit 7      = data ready (always observed set when count > 0)

Then, for each point (8 bytes, repeating):
  Byte 0:    track ID (bits 3:0) + flags (bits 7:4)
  Bytes 1-2: X coordinate — **LITTLE-ENDIAN** u16
  Bytes 3-4: Y coordinate — **LITTLE-ENDIAN** u16
  Bytes 5-6: touch strength/area — **LITTLE-ENDIAN** u16
  Byte 7:    padding

After all points:
  Next byte: checksum
  Next byte: padding

The ENTIRE block (1 + count×8 + 2 bytes) must sum to 0 (mod 256).
```

⚠ **Critical protocol details** (all verified on hardware):
1. Coordinates are **little-endian** (the vendor C code uses packed
   structs on a LE CPU — easy to get wrong when porting)
2. Each touch record is **8 bytes** (not 7)
3. The checksum covers **1 + 8×n + 2 bytes** (status + records +
   checksum byte + padding byte) — not 1 + 8×n + 1

### After reading

Write 0 to register 0x814E to clear the buffer status, regardless of
whether points were found. This keeps the controller happy for the next
poll.

## Polling driver (src/touch.rs)

```rust
pub fn poll(&mut self) -> Option<TouchPoint> {
    // 1. Read status byte from 0x814E
    // 2. If count == 0: write 0 to clear, return None
    // 3. Read (1 + count*8 + 2) bytes from 0x814E
    // 4. Write 0 to 0x814E to clear
    // 5. Verify checksum (entire block sums to 0)
    // 6. Parse first point: x = LE16(buf[2..4]), y = LE16(buf[4..6])
    // 7. Return Some(TouchPoint { x, y, strength })
}
```

Poll at ≥20 ms intervals. The controller updates asynchronously; the
clear-then-read pattern prevents stale data.

## Coordinate mapping for LCD calibration

The LCD panel locks to the DPI stream with a **random phase each boot**.
The boot calibration (07-lcd-intro.md) measures the offset from a single
touch:

```
Crosshair drawn at FB (400, 240)
User touches visible position (tx, ty)
Panel phase: Sx = tx - 400, Sy = ty - 240 (mod 800, 480)
Render offsets: offset_x = -Sx mod 800, offset_y = -Sy mod 480
Touch→FB mapping: fb_x = (tx - Sx) mod 800, fb_y = (ty - Sy) mod 480
```

The touch controller reports coordinates in **panel glass space** (the
controller is glued to the physical glass). The calibration converts
between glass space and framebuffer space.
