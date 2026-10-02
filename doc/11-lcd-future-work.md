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
scope or logic analyzer; the console PCLK value is inferred from VSYNC.

## 2. Remove legacy display-phase calibration when alignment is verified

Determine whether a fixed offset persists with underruns eliminated.
Do not assume the panel starts at an inherently random phase. If correct
scan-out consistently aligns, remove the boot crosshair and rendering
rotation rather than retaining an unnecessary user calibration step.

## 3. Verify dirty-region writeback under calibration

Rendering currently rotates pixels by the calibration offsets. Ensure
cache writeback covers the actual modified addresses, including wraparound
and the unrotated touch cursor. Then reduce writeback to the affected
cache-aligned row spans to avoid unnecessary PSRAM traffic.

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
