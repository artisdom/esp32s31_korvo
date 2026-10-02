# LCD: Current Architecture

Updated 2026-10-02 after the user reported that horizontal drift persisted.

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
It does not print from the interrupt. Calibration logs derive the scan
rate and inferred PCLK using actual elapsed time. UART heartbeat logs
print cumulative VSYNC and underrun counts and the transfer-buffer enable
bit. Inferred PCLK is not a physical measurement of the GPIO40 clock.
DMA EOF events are not used as a frame-rate measurement.

## Rendering and cache maintenance

The UI redraws a full page on entry and dynamic widgets every 50 ms.
Full redraws use chunked framebuffer cache writeback; dynamic redraws
write back dirty regions. No uncacheable MMU mapping or bounce buffers
are required by the current implementation.

The old boot crosshair calibration remains as an alignment workaround.
It measures a fixed offset and rotates rendering; it cannot correct ongoing
drift. A claim that the panel inherently locks at a random phase has not
been established independently of the previous underrun problem.

## Remaining checks

Long-duration operation, repeated resets, exact edge alignment and removal
of the legacy calibration need further validation. Dirty-region cache
writeback should also be checked with nonzero calibration offsets.
Camera capture and additional memory traffic will need new underrun checks.
