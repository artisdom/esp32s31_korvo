# LCD: Current Architecture

Updated 2026-10-02 after correcting drift and removing the title-wrapping calibration.

The CPU renders an 800×480 RGB565 framebuffer (768,000 bytes) in PSRAM.
AXI GDMA channel 0 scans it through a circular list of 191 descriptors.
The descriptors are statically allocated in internal RAM; their pixel
buffers point into PSRAM. LCD_CAM generates the panel timing and outputs
a 16-bit RGB bus through the GPIO matrix.

## Transfer buffer

`Display::new` enables `LCD_TRANS_BUFF_CFG.LCD_TRANS_BUFFER_ENA` before
starting DMA. This was the missing initialization: ESP-IDF's
`rgb_panel_create()` enables the transfer buffer to improve performance
and prevent underruns. It is disabled at reset, and the previous Rust
implementation never enabled it. The existing esp-hal clock setup keeps
the LCD memory block powered.

With the buffer disabled, the board reported hundreds of underruns while
the UI updated. Enabling it eliminated reported underruns in the observed
run, and the user confirmed the display stayed stationary during live
updates and tab changes. Correcting the pixel clock alone had not fixed
this problem.

## Timing and diagnostics

The requested PCLK is 18 MHz. The local esp-hal clock patch requests a
36 MHz LCD module clock and divides it by two. Frame timing is:

- Horizontal: 800 active + 40 sync + 40 back porch + 48 front porch.
- Vertical: 480 active + 23 sync + 32 back porch + 13 front porch.
- Total: 928×548 = 508,544 PCLK cycles per frame; nominal refresh 35.395 Hz.
- HSYNC/VSYNC active low, DE active high; configured falling PCLK edge.

An LCD_CAM interrupt counts actual VSYNC and underrun events. The handler
acknowledges only those LCD events because camera shares this peripheral.
It does not print from the interrupt. UART heartbeat logs print cumulative
VSYNC and underrun counts, the transfer-buffer enable bit and the number
of clock cycles per frame. Count differences over elapsed time give the
scan rate; this is not a physical measurement of the GPIO40 clock. DMA EOF
events are not used as a frame-rate measurement.

## Rendering and cache maintenance

The UI redraws a full page on entry and dynamic widgets every 50 ms.
Full redraws use chunked framebuffer cache writeback; dynamic redraws
write back dirty regions. No uncacheable MMU mapping or bounce buffers
are required by the current implementation.

The boot crosshair calibration and framebuffer rotation have been removed.
It treated a touch position as an exact display alignment measurement. The
last recorded touch (404,257) produced offsets (796,463), shifting rendering
upward by 17 pixels with wraparound. This split the title across the top
and bottom. A touch measurement cannot establish a hardware scan offset.

Rendering, touch hit-testing, cursor drawing and dirty regions now share
the same screen coordinates. Drawing clips at screen edges rather than
wrapping. Startup proceeds directly from the splash to the home page.

## Remaining checks

Long-duration operation, repeated resets and exact edge alignment need
further validation. Dirty-region cache writeback can be narrowed to
cache-aligned row spans; there are no calibration offsets to transform.
Camera capture and additional memory traffic will need new underrun checks.
