# LCD: Future Work & Improvement Roadmap

Updated 2026-10-02 after reproducing the reported drift.

## Completed: enable the RGB transfer buffer

The clock patch alone did not stop horizontal drift. The missing
`LCD_TRANS_BUFF_CFG.LCD_TRANS_BUFFER_ENA` initialization was added to
`Display::new`, following ESP-IDF's `rgb_panel_create()`.

On the connected board, enabling the buffer changed the reported underrun
count from hundreds during live redraws to zero over more than 1,800
VSYNC events. The user confirmed that the image stayed stationary while
live values updated and tabs changed. See [known issues](10-lcd-known-issues.md)
for evidence and validation limits.

LCD VSYNC now supplies the refresh measurement; DMA EOF does not.

## 1. Extend hardware validation

Run for an extended period with touch, page changes, audio and USB traffic.
Check cumulative underruns in the UART heartbeat. Test warm resets and
cold starts separately. Measure GPIO40 PCLK and sync/DE alignment with a
scope or logic analyzer; VSYNC counts are not a physical PCLK measurement.

## Completed: remove title-wrapping display calibration

After drift stopped, the title split across the top and bottom. Boot touch
calibration had set a vertical rendering offset of 463 (equivalent to -17
rows), wrapping the title around the screen. The crosshair calibration and
rendering rotation are now removed. Rendering, touch and dirty regions use
the same screen coordinates; no calibration touch is required at startup.
The user confirmed correct title and touch-tab alignment after flashing.

## 2. Validate physical alignment

Confirm the complete title stays at the top and touch tabs line up after
warm and cold starts. If a repeatable touch offset remains, investigate
the touch controller's coordinate mapping independently of LCD rendering.
A touch at a crosshair must not rotate the entire framebuffer.

## 3. Optimize dirty-region writeback

There are no rendering offsets to transform. Narrow cache writeback to the
modified cache-aligned row spans, including the touch cursor, to avoid
unnecessary PSRAM traffic.

## 4. Camera capture and display

Keep the verified SC101IOT paged SCCB probe and XCLK setup. Port its vendor
sensor initialization to Rust, configure DVP receive, and add preview.
Monitor underruns when camera DMA and LCD DMA share PSRAM bandwidth.

## 5. GUI and touch polish

Dynamic widgets already run every 50 ms. Improve interactions and drawing
only after the alignment/cache checks. Choose Rust GUI components if the
fully Rust implementation remains a requirement; LVGL would introduce C.

## 6. Radio support

Check the local esp-radio implementation and current S31 support before
choosing dependencies for Wi-Fi, BLE or 802.15.4. Do not assume an unreleased
crate version or that enabling features alone completes board support.

## Deferred approaches

Uncacheable framebuffer mappings and SRAM bounce buffers are not required
for the observed drift fix. Revisit them only if measured bandwidth or
latency under additional workloads warrants it. Uncacheable and
write-through mappings are different cache policies; neither guarantees
that DMA underruns cannot occur.
