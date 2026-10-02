# Board Hardware Reference

## Module

ESP32-S31-WROOM-3: dual-core RISC-V @ 320 MHz, 16 MB SPI flash, 16 MB hex
PSRAM @ 250 MHz. Wi-Fi 6 / Bluetooth 5.4 LE and Classic / 802.15.4.
Optional Rust radio demonstrations and current host-stack limits are documented
in [radio support](16-radio-support.md).

## Serial console

The USB-C port has a **CP2102N** USB-UART bridge (not FTDI as the docs
imply — match boards by MAC address, not bridge chip). Console runs at
115200 8N1. The board has a physical **power switch** — check it before
debugging "disconnected" boards.

Reset via serial: DTR=low (run mode), pulse RTS high→low (EN toggle).

## Verified I2C bus (GPIO0=SDA, GPIO1=SCL)

| Address | Device | Notes |
|---|---|---|
| **0x10** | ES8389 audio codec | Vendor macro says 0x20 — that's the 8-bit write form. Wire ACKs at 0x10 7-bit. |
| **0x14** | GT1151 touch controller | Alternate 0x5D also valid. Product ID at this unit: "1158". |
| 0x3C | OV3660 camera (if present) | SCCB — did not answer without power/XCLK bring-up |
| **0x68** | **SC101IOT camera** | Answers once the 20 MHz XCLK runs; PID 0xda4a. Paged SCCB (reg 0xf0 = page) |

## Pin map (from vendor BSP, verified against schematic)

### Shared I2C
```
SDA = GPIO0, SCL = GPIO1
```

### Audio (I2S0)
```
MCLK  = GPIO2    (not required — codec derives from SCLK)
SCLK  = GPIO3    (BCLK, SoC master)
LRCLK = GPIO4    (WS)
SDOUT = GPIO5    (SoC → codec, playback)
DSIN  = GPIO6    (codec → SoC, mic capture)
PA_CTRL = GPIO7  (NS4150B enable, active-high)
```

### WS2812 status LED
```
DATA = GPIO37    (RMT-driven)
```

### ADC button ladder
```
GPIO42 = ADC1_CH0_N — the NEGATIVE input of the differential SAR channel

Because the ladder sits on the negative input, the code is inverted: the
2 V idle level is the BOTTOM of the code range and reads raw 0. Pressing a
key lowers the ladder voltage and RAISES the code.

Voltage ladder (0 dB attenuation, 2 V full-scale):
  idle (no press) = 2000 mV   → raw 0
  VOL+            =  380 mV
  VOL-            =  820 mV
  MODE            = 1340 mV
  SET             = 1870 mV

The HAL returns the already weighted code (0..4393), so apply directly:
    mv = 2000 - 4000 * code / 4393
Do not interpret this result as a comparator bit pattern and weight it again.
The GPIO42 voltage mapping is inverted because it is the negative input.
```

### microSD (SDMMC 4-bit)
```
CLK  = GPIO24
CMD  = GPIO25
D0   = GPIO20
D1   = GPIO21
D2   = GPIO22
D3   = GPIO23
PWR  = GPIO39    (active-low power switch)
```

### RGB LCD (16-bit DPI bus)
```
DATA0..7  = GPIO8..15    (B3..B7, G2..G3 — byte 0)
DATA8..15 = GPIO16..19, GPIO33..36  (G4..G7, R3..R7 — byte 1)

PCLK  = GPIO40   (boot-mode strapping pin)
DE    = GPIO43
HSYNC = GPIO44
VSYNC = GPIO45

Not used by RGB driver but routed to the sub-board connector:
  LCD_CS  = GPIO38   (boot-mode strapping pin)
  MOSI    = GPIO60   (boot-mode strapping pin)
  SCK     = GPIO61   (boot-mode strapping pin)
```

### DVP camera
```
D0..D7 = GPIO46..53
PCLK   = GPIO54
XCLK   = GPIO55   (20 MHz, needs generation for sensor to respond)
VSYNC  = GPIO56
HREF   = GPIO57
SCCB   = shared I2C bus (GPIO0/1)
GM_FK  = GPIO38   (shared with LCD_CS)
```

### USB
```
USB_HS  = native USB 2.0 HS pins (Type-A port, no GPIO muxing needed)
USB Dev = USB-Serial-JTAG peripheral (not used — CP2102N handles console)
```

## Panel: ST7262E43 (4.3″ 800×480 RGB TFT)

This is the same panel as ESP32-S3-LCD-EV-Board SUB3. It is a "dumb" TTL
RGB panel — no init sequence, no register interface. It locks to the DPI
timing (HSYNC/VSYNC/DE/PCLK) with a **random phase each boot**.

Correct timing (from Espressif's `SUB_BOARD3_800_480_PANEL_35HZ_RGB_TIMING`):
```
PCLK       = 18 MHz  → ~35 Hz refresh (verified on hardware)
HSYNC      = 40 PCLK pulse, 40 back porch, 48 front porch
VSYNC      = 23 lines pulse, 32 back porch, 13 front porch
Data latch = falling PCLK edge (pclk_active_neg = true)
HSYNC/VSYNC idle = high (active-low pulses)
DE idle     = low
```

⚠ The Korvo S31 BSP (`display.h`) carries **wrong** timing values
(26 MHz, 1/40/20 H, 1/10/5 V) — these do not lock the ST7262E43.
Use the values above.
